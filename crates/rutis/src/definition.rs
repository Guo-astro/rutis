//! Plugin definitions: a plugin as a value, mounted as many times as
//! needed, in any language (design `docs/design-plugin-values-2026-10-25.md`).
//!
//! A [`Plugin`] is an instance: its config is in its fields, and mounting it
//! uses it up. A [`Definition`] is what Cordis calls a plugin: a value that
//! builds a copy each time it is mounted, with that copy's config. One made
//! in Rust and one handed over by another language are the same type, and
//! both are mounted with `ctx.plugin(def.at(&ctx, config)?)`.

use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, Weak};

use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use tokio_util::sync::CancellationToken;

use crate::ctx::Ctx;
use crate::fiber::FiberInner;
use crate::host::CtxHost;
use crate::{BoxFuture, DynPlugin, Effect, Error, ErrorKind, FiberView, Plugin, Scope, TypeKey};

/// A plugin definition: what can be mounted, many times, each time with its
/// own config.
///
/// Make one in Rust with [`Definition::new`]; one a plugin in another
/// language hands to a Rust service arrives as a `Definition` too (a
/// `#[rutis::service]` parameter of this type). Mount a copy with
/// `ctx.plugin(def.at(&ctx, config)?)`.
///
/// A definition belongs to its origin: the plugin that made it, or the row
/// that handed it over. When the origin unloads, restarts or ends, every copy
/// mounted from the definition ends first, and the origin's own cleanups run
/// after; then [`Definition::ended`] completes and [`Definition::at`] fails.
///
/// Clones are the same definition: they compare equal, and their copies are
/// recorded together.
#[derive(Clone)]
pub struct Definition(Arc<Inner>);

pub(crate) struct Inner {
    name: String,
    source: Box<dyn Source>,
    /// The origin's scope chain when the definition was made: copies mount
    /// inside it (or deeper).
    scope: Vec<Scope>,
    copies: Mutex<Copies>,
    ended: CancellationToken,
}

#[derive(Default)]
struct Copies {
    /// The origin is stopping: nothing more is mounted.
    ending: bool,
    fibers: Vec<Weak<FiberInner>>,
}

/// How a definition builds its copies: from a Rust build function, or by
/// loading the definition in another language's runtime (rutis-bridge).
pub trait Source: Send + Sync + 'static {
    /// The plugin for one copy at `at`, with `config`.
    fn build(&self, at: &Ctx, config: serde_json::Value) -> Result<Box<dyn DynPlugin>, Error>;
    /// What the definition is sent as to another language, or why it cannot
    /// be sent.
    fn encode(&self) -> Result<&dyn erased_serde::Serialize, String>;
    fn as_any(&self) -> &dyn Any;
}

impl Definition {
    /// A definition made in Rust. Each copy is `build(ctx, config)`: `ctx` is
    /// where the copy is mounted (so `build` can read the instance it is in),
    /// `config` the copy's config, decoded into `C`. The definition belongs
    /// to the plugin whose `ctx` is passed here.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use rutis::{Ctx, Definition, Plugin};
    ///
    /// struct Part { depth: u32 }
    /// impl Plugin for Part {
    ///     type Deps = ();
    ///     async fn apply(&self, _: &Ctx, (): ()) -> rutis::Result { Ok(()) }
    /// }
    ///
    /// fn part(ctx: &Ctx) -> Definition {
    ///     Definition::new(ctx, |_at: &Ctx, depth: u32| Ok(Part { depth }))
    /// }
    /// ```
    pub fn new<C, P, B>(ctx: &Ctx, build: B) -> Self
    where
        C: DeserializeOwned + 'static,
        P: Plugin,
        B: Fn(&Ctx, C) -> Result<P, Error> + Send + Sync + 'static,
    {
        Self::from_source(
            Some(ctx),
            None,
            short_name::<P>(),
            Box::new(Built {
                build,
                _types: PhantomData,
            }),
        )
    }

    /// A definition built by `source`, belonging to `origin` (none: to the
    /// process `source` stands for, ended with [`end`]). Copies mount inside
    /// `scope`, or the origin's scope chain when `scope` is `None`.
    pub(crate) fn from_source(
        origin: Option<&Ctx>,
        scope: Option<Vec<Scope>>,
        name: String,
        source: Box<dyn Source>,
    ) -> Self {
        let scope =
            scope.unwrap_or_else(|| origin.map(crate::host::CtxHost::scope).unwrap_or_default());
        let definition = Definition(Arc::new(Inner {
            name,
            source,
            scope,
            copies: Mutex::new(Copies::default()),
            ended: CancellationToken::new(),
        }));
        if let Some(ctx) = origin {
            if !ctx.register_definition(Arc::downgrade(&definition.0)) {
                // The origin is already stopping: nothing can be mounted.
                definition.0.copies.lock().unwrap().ending = true;
                definition.0.ended.cancel();
            }
        }
        definition
    }

    /// The plugin's name: its declared name, or its type's.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The plugin for one copy mounted on `ctx`, with `config`: pass it to
    /// `ctx.plugin`. Fails once the definition has ended, when `ctx` is
    /// outside the instance the definition's origin is in, or when the config
    /// does not fit.
    pub fn at(&self, ctx: &Ctx, config: impl Serialize) -> Result<impl DynPlugin, Error> {
        if self.0.copies.lock().unwrap().ending {
            return Err(failed(format!(
                "plugin definition {} has ended with its origin",
                self.0.name
            )));
        }
        let here = crate::host::CtxHost::scope(ctx);
        if !here.starts_with(&self.0.scope) {
            return Err(failed(format!(
                "plugin definition {} can only be mounted inside {} or deeper, not inside {}",
                self.0.name,
                describe(&self.0.scope),
                describe(&here),
            )));
        }
        let config = serde_json::to_value(config).map_err(|error| {
            failed(format!(
                "the config of plugin definition {}: {error}",
                self.0.name
            ))
        })?;
        let inner = self.0.source.build(ctx, config)?;
        Ok(Copy {
            inner,
            definition: self.clone(),
            at: ctx.weak_fiber(),
        })
    }

    /// Completes once the definition has ended: its origin stopped, and every
    /// copy mounted from it is gone.
    pub fn ended(&self) -> impl Future<Output = ()> + Send + 'static {
        self.0.ended.clone().cancelled_owned()
    }

    /// Whether the definition has ended ([`Definition::ended`]).
    pub fn is_ended(&self) -> bool {
        self.0.ended.is_cancelled()
    }

    pub(crate) fn source(&self) -> &dyn Source {
        &*self.0.source
    }

    /// A copy was mounted as `view`: record it, or end it at once when the
    /// origin is already stopping.
    pub(crate) fn mounted(&self, view: &FiberView) {
        if self.record(Arc::downgrade(&view.inner)).is_err() {
            drop(view.dispose());
        }
    }

    fn record(&self, fiber: Weak<FiberInner>) -> Result<(), Error> {
        let mut copies = self.0.copies.lock().unwrap();
        if copies.ending {
            return Err(failed(format!(
                "plugin definition {} has ended with its origin",
                self.0.name
            )));
        }
        copies.fibers.retain(|known| known.strong_count() > 0);
        if !copies.fibers.iter().any(|known| known.ptr_eq(&fiber)) {
            copies.fibers.push(fiber);
        }
        Ok(())
    }
}

impl PartialEq for Definition {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Definition {}

impl std::fmt::Debug for Definition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Definition").field(&self.0.name).finish()
    }
}

/// A definition from another language is sent back to the runtime that
/// defined it; one made in Rust cannot be sent.
impl Serialize for Definition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.source.encode() {
            Ok(value) => erased_serde::serialize(value, serializer),
            Err(why) => Err(serde::ser::Error::custom(why)),
        }
    }
}

/// A definition held without keeping it: what rutis-bridge remembers per
/// value another language handed over, so the same value is one definition.
pub struct WeakDefinition(Weak<Inner>);

impl WeakDefinition {
    pub fn new(definition: &Definition) -> Self {
        Self(Arc::downgrade(&definition.0))
    }

    pub fn upgrade(&self) -> Option<Definition> {
        self.0.upgrade().map(Definition)
    }
}

/// End `definition` now (one belonging to a process that is gone): its
/// copies end, then it has ended.
pub(crate) fn end(definition: &Definition) -> BoxFuture<'static, ()> {
    let inner = definition.0.clone();
    Box::pin(async move {
        end_copies(&inner, None, None).await;
        inner.ended.cancel();
    })
}

/// The origin fiber `origin` is stopping: end the copies of its
/// `definitions`, and wait until they are cleaned up. Returns the
/// definitions, to mark ended once the origin's own cleanups ran.
pub(crate) async fn end_all(
    origin: &Arc<FiberInner>,
    definitions: Vec<Weak<Inner>>,
) -> Vec<Arc<Inner>> {
    let sink = origin.ctx.error_sink();
    let mut ending = Vec::new();
    for definition in definitions.iter().filter_map(Weak::upgrade) {
        end_copies(&definition, Some(origin), Some(&sink)).await;
        ending.push(definition);
    }
    ending
}

/// Mark ended: called once the origin's cleanups ran.
pub(crate) fn ended(definitions: Vec<Arc<Inner>>) {
    for definition in definitions {
        definition.ended.cancel();
    }
}

async fn end_copies(
    definition: &Arc<Inner>,
    origin: Option<&Arc<FiberInner>>,
    sink: Option<&crate::error::ErrorSink>,
) {
    let fibers = {
        let mut copies = definition.copies.lock().unwrap();
        copies.ending = true;
        std::mem::take(&mut copies.fibers)
    };
    for fiber in fibers.iter().filter_map(Weak::upgrade) {
        let view = FiberView::from_inner(fiber.clone());
        // A copy mounted above its own origin ends after it: waiting here
        // would wait for the origin's own stop.
        if origin.is_some_and(|origin| is_ancestor(&fiber, origin)) {
            drop(view.dispose());
            continue;
        }
        let delivered = matches!(view.state().state, crate::FiberState::Disposed);
        if let Err(error) = view.dispose().await {
            if !delivered {
                match sink {
                    Some(sink) => sink(error),
                    None => (fiber.ctx.error_sink())(error),
                }
            }
        }
    }
}

fn is_ancestor(fiber: &Arc<FiberInner>, of: &Arc<FiberInner>) -> bool {
    let mut current = of.parent_fiber.as_ref().and_then(Weak::upgrade);
    while let Some(parent) = current {
        if Arc::ptr_eq(&parent, fiber) {
            return true;
        }
        current = parent.parent_fiber.as_ref().and_then(Weak::upgrade);
    }
    false
}

/// The definition `plugin` is a copy of, if it is one: so `ctx.plugin`
/// records the copy as it mounts it, before it applies (or waits).
pub(crate) fn copy_of(plugin: &dyn Any) -> Option<Definition> {
    plugin
        .downcast_ref::<Copy>()
        .map(|copy| copy.definition.clone())
}

/// One copy of a definition: the built plugin, checked where it mounts.
struct Copy {
    inner: Box<dyn DynPlugin>,
    definition: Definition,
    /// The fiber of the `ctx` given to `at`: the copy mounts there.
    at: Weak<FiberInner>,
}

impl DynPlugin for Copy {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.inner.injects()
    }

    fn validate(&self) -> Result<(), Error> {
        self.inner.validate()
    }

    fn platform(&self) -> bool {
        self.inner.platform()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let fiber = ctx.weak_fiber();
            let mounted_at = fiber
                .upgrade()
                .and_then(|fiber| fiber.parent_fiber.clone())
                .unwrap_or_default();
            if !mounted_at.ptr_eq(&self.at) {
                return Err(failed(format!(
                    "a copy of plugin definition {} was built for another place: \
                     mount it on the ctx given to `at`",
                    self.definition.name()
                )));
            }
            // Mounted through a box, `ctx.plugin` did not see it: record it here.
            self.definition.record(fiber)?;
            self.inner.apply(ctx).await
        })
    }
}

/// The source of a definition made in Rust.
struct Built<C, P, B> {
    build: B,
    _types: PhantomData<fn(C) -> P>,
}

impl<C, P, B> Source for Built<C, P, B>
where
    C: DeserializeOwned + 'static,
    P: Plugin,
    B: Fn(&Ctx, C) -> Result<P, Error> + Send + Sync + 'static,
{
    fn build(&self, at: &Ctx, config: serde_json::Value) -> Result<Box<dyn DynPlugin>, Error> {
        let config: C = serde_json::from_value(config).map_err(|error| {
            failed(format!(
                "the config of plugin definition {}: {error}",
                short_name::<P>()
            ))
        })?;
        Ok(Box::new((self.build)(at, config)?))
    }

    fn encode(&self) -> Result<&dyn erased_serde::Serialize, String> {
        Err(format!(
            "plugin definition {} was made in Rust: it cannot be handed to another language",
            short_name::<P>()
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn short_name<P>() -> String {
    let full = std::any::type_name::<P>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base).to_owned()
}

fn describe(scope: &[Scope]) -> String {
    if scope.is_empty() {
        return "the root".to_owned();
    }
    let chain: Vec<String> = scope
        .iter()
        .map(|scope| format!("{} {}", scope.group, scope.id))
        .collect();
    format!("instance {}", chain.join(" / "))
}

fn failed(message: String) -> Error {
    ErrorKind::PluginFailed(message.into()).into()
}
