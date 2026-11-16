//! guarantee: core/D13, core/D21b
//! Property test of the lookup, scope and strict-read decision table (#631):
//! generated reader / declaration / provider scenes are compared with a
//! simple table-driven reference.
//!
//! The scene fixes, for one service key of type `u64`:
//!
//! - the key: the plain key, the reader's own instance key, an ancestor's
//!   instance key, or a sibling plugin's instance key;
//! - the reader's isolate scope (`isolate(key, label)`, same label = same
//!   scope);
//! - the declaration: none, on the reader's own fiber, on an ancestor that
//!   reads the same scope, or on an ancestor that reads another scope;
//! - the provider: none, in the reader's scope, in another scope, or in the
//!   reader's scope but restarting (Loading, with the new binding already
//!   registered);
//! - whether the reader provides the service itself, is disposed, and
//!   whether the strict read uses a key of another type (TypeMismatch).
//!
//! The reference is the table in [`strict_result`] / [`optional_result`]:
//!
//! | order | condition | strict read |
//! | --- | --- | --- |
//! | 1 | key on an instance outside the caller's fiber ancestry | `OutOfScope` |
//! | 2 | key of another value type | `TypeMismatch` |
//! | 3 | caller unloading/disposed and it is not its own service | `Inactive` |
//! | 4 | not declared (own service counts) | `Undeclared` |
//! | 5 | own service | value |
//! | 6 | binding missing / provider not active | `Unavailable(..)` |
//! | 7 | otherwise | value |
//!
//! An optional `get` checks none of the declaration rules: it answers with
//! the value when the instance is in scope and the binding is visible to the
//! caller, and with `None` otherwise. The provider's subtree sees its own
//! binding even while the caller unloads, as does the provider itself.
//!
//! Both reads are performed on every scene. `PROPTEST_CASES` cases (256 by
//! default); a failure prints the seed and the shrunk scene, rerun with
//! `RUTIS_PROPTEST_SEED=<seed>` to replay it (Q7.2).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use tokio::sync::Notify;

use crate::host::diagnostics::DependencyStatus;
use crate::host::{
    CtxHost, DynPlugin, Effect, FiberState, FiberView, Key, ServiceReadFailure, TypeKey,
};
use crate::internal::CtxInternal;
use crate::service::Provision;
use crate::{Ctx, Error};

// ── the scene ───────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Root,
    A,
    B,
}

impl Scope {
    fn label(self) -> Option<&'static str> {
        match self {
            Self::Root => None,
            Self::A => Some("a"),
            Self::B => Some("b"),
        }
    }

    fn ctx(self, root: &Ctx, key: &TypeKey) -> Ctx {
        match self.label() {
            None => root.clone(),
            Some(label) => root.isolate(key.clone(), label),
        }
    }

    /// A scope that is not `self`.
    fn other(self) -> Self {
        match self {
            Self::Root => Self::A,
            Self::A | Self::B => Self::Root,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyKind {
    Plain,
    OwnInstance,
    AncestorInstance,
    ForeignInstance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Declaration {
    None,
    SelfFiber,
    AncestorSame,
    AncestorOther,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderKind {
    None,
    Same,
    Other,
    LoadingSame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    Active,
    Disposed,
}

#[derive(Clone, Debug)]
struct Scene {
    key: KeyKind,
    reader_scope: Scope,
    declaration: Declaration,
    provider: ProviderKind,
    own: bool,
    caller: Caller,
    mismatch: bool,
}

fn scopes() -> impl Strategy<Value = Scope> {
    prop_oneof![Just(Scope::Root), Just(Scope::A), Just(Scope::B)]
}

fn callers() -> impl Strategy<Value = Caller> {
    prop_oneof![Just(Caller::Active), Just(Caller::Disposed)]
}

fn providers() -> impl Strategy<Value = ProviderKind> {
    prop_oneof![
        2 => Just(ProviderKind::None),
        4 => Just(ProviderKind::Same),
        2 => Just(ProviderKind::Other),
        2 => Just(ProviderKind::LoadingSame),
    ]
}

fn declarations(reader_scope: Scope) -> impl Strategy<Value = Declaration> {
    // An ancestor in another scope needs a reader scope to isolate to.
    let other = match reader_scope {
        Scope::Root => Just(Declaration::AncestorSame),
        Scope::A | Scope::B => Just(Declaration::AncestorOther),
    };
    prop_oneof![
        2 => Just(Declaration::None),
        3 => Just(Declaration::SelfFiber),
        3 => Just(Declaration::AncestorSame),
        2 => other,
    ]
}

fn plain_scene(
    reader_scope: Scope,
    caller: Caller,
    mismatch: bool,
) -> impl Strategy<Value = Scene> {
    (any::<bool>(), providers(), declarations(reader_scope)).prop_map(
        move |(own, provider, declaration)| {
            let (provider, declaration) = match own {
                true => (ProviderKind::None, Declaration::None),
                false => match declaration {
                    // An ancestor that loses its dependency unloads, and
                    // its children are disposed with it: the reader would be
                    // inactive, not merely undeclared. Keep the provider
                    // that starts and keeps the ancestor up.
                    Declaration::AncestorSame => (ProviderKind::Same, declaration),
                    // The ancestor is mounted at the root, where it needs a
                    // provider of its own; the reader's binding lives in A
                    // or B.
                    Declaration::AncestorOther => (ProviderKind::Same, declaration),
                    declaration => (provider, declaration),
                },
            };
            Scene {
                key: KeyKind::Plain,
                reader_scope,
                declaration,
                provider,
                own,
                caller,
                mismatch,
            }
        },
    )
}

fn scenes() -> impl Strategy<Value = Scene> {
    prop_oneof![
        5 => (scopes(), callers(), any::<bool>())
            .prop_flat_map(|(scope, caller, mismatch)| plain_scene(scope, caller, mismatch)),
        1 => (callers(), any::<bool>()).prop_map(|(caller, mismatch)| Scene {
            key: KeyKind::OwnInstance,
            reader_scope: Scope::Root,
            declaration: Declaration::None,
            provider: ProviderKind::None,
            own: true,
            caller,
            mismatch,
        }),
        2 => (callers(), any::<bool>()).prop_map(|(caller, mismatch)| Scene {
            key: KeyKind::AncestorInstance,
            reader_scope: Scope::Root,
            declaration: Declaration::None,
            provider: ProviderKind::None,
            own: false,
            caller,
            mismatch,
        }),
        2 => (callers(), any::<bool>()).prop_map(|(caller, mismatch)| Scene {
            key: KeyKind::ForeignInstance,
            reader_scope: Scope::Root,
            declaration: Declaration::None,
            provider: ProviderKind::None,
            own: false,
            caller,
            mismatch,
        }),
    ]
}

// ── the reference table ─────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    Missing,
    Ready,
    /// The binding is registered and its provider is still loading (a
    /// restart whose `apply` has provided but not returned).
    ProviderLoading,
}

#[derive(Clone, Copy, Debug)]
struct Facts {
    in_instance: bool,
    type_matches: bool,
    own_service: bool,
    caller_inactive: bool,
    declared: bool,
    binding: Binding,
    /// The caller is in the provider's fiber subtree (its own service).
    self_access: bool,
    value: u64,
}

/// The strict-read column of the table, in its precedence order.
fn strict_result(facts: &Facts) -> Result<u64, ServiceReadFailure> {
    if !facts.in_instance {
        return Err(ServiceReadFailure::OutOfScope);
    }
    if !facts.type_matches {
        return Err(ServiceReadFailure::TypeMismatch);
    }
    if facts.caller_inactive && !facts.own_service {
        return Err(ServiceReadFailure::Inactive);
    }
    if !facts.declared {
        return Err(ServiceReadFailure::Undeclared);
    }
    if facts.own_service {
        return Ok(facts.value);
    }
    match facts.binding {
        Binding::Ready => Ok(facts.value),
        Binding::Missing => Err(ServiceReadFailure::Unavailable(DependencyStatus::Missing)),
        Binding::ProviderLoading => Err(ServiceReadFailure::Unavailable(
            DependencyStatus::ProviderInactive(FiberState::Loading),
        )),
    }
}

/// The optional-read column: no declaration rule, but the instance and the
/// binding's visibility still apply.
fn optional_result(facts: &Facts) -> Option<u64> {
    if !facts.in_instance || facts.binding == Binding::Missing {
        return None;
    }
    if !facts.self_access && (facts.caller_inactive || facts.binding != Binding::Ready) {
        return None;
    }
    Some(facts.value)
}

impl Scene {
    /// What the table needs to know about this scene.
    fn facts(&self) -> Facts {
        let disposed = self.caller == Caller::Disposed;
        match self.key {
            KeyKind::Plain => {
                let own_service = self.own && !disposed;
                let declared = own_service
                    || matches!(
                        self.declaration,
                        Declaration::SelfFiber | Declaration::AncestorSame
                    );
                let binding = if own_service {
                    Binding::Ready
                } else {
                    match self.provider {
                        ProviderKind::None | ProviderKind::Other => Binding::Missing,
                        ProviderKind::Same => Binding::Ready,
                        // A disposed caller is not put through the restart:
                        // it fails at the inactive row before the binding.
                        ProviderKind::LoadingSame if disposed => Binding::Ready,
                        ProviderKind::LoadingSame => Binding::ProviderLoading,
                    }
                };
                Facts {
                    in_instance: true,
                    type_matches: !self.mismatch,
                    own_service,
                    caller_inactive: disposed && !own_service,
                    declared,
                    binding,
                    self_access: own_service,
                    value: if own_service { 3 } else { 1 },
                }
            }
            KeyKind::OwnInstance => {
                let own_service = !disposed;
                Facts {
                    in_instance: true,
                    type_matches: !self.mismatch,
                    own_service,
                    caller_inactive: disposed,
                    declared: own_service,
                    binding: if own_service {
                        Binding::Ready
                    } else {
                        Binding::Missing
                    },
                    self_access: own_service,
                    value: 3,
                }
            }
            KeyKind::AncestorInstance => Facts {
                in_instance: true,
                type_matches: !self.mismatch,
                own_service: false,
                caller_inactive: disposed,
                declared: true,
                // The provider is an ancestor: disposing the reader does
                // not withdraw its binding.
                binding: Binding::Ready,
                self_access: true,
                value: 9,
            },
            KeyKind::ForeignInstance => Facts {
                in_instance: false,
                type_matches: !self.mismatch,
                own_service: false,
                caller_inactive: disposed,
                declared: false,
                binding: Binding::Missing,
                self_access: false,
                value: 0,
            },
        }
    }
}

// ── the plugins a scene is built from ───────────────────────────

#[derive(Clone, Copy, Debug)]
enum Provide {
    Nothing,
    Plain,
    OwnInstance,
    /// Provide on every apply, and on the second and later applies wait for
    /// `release` after providing (a restart that stays Loading).
    GatedPlain,
}

struct Probe {
    name: &'static str,
    injects: Vec<TypeKey>,
    provide: Provide,
    value: u64,
    slot: Arc<Mutex<Option<Ctx>>>,
    applies: Arc<AtomicUsize>,
    reached: Arc<Notify>,
    release: Arc<Notify>,
}

impl DynPlugin for Probe {
    fn name(&self) -> &str {
        self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> crate::BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.slot.lock().unwrap() = Some(ctx.clone());
            let applies = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
            match self.provide {
                Provide::Nothing => {}
                Provide::Plain => {
                    ctx.provide(self.value)?;
                }
                Provide::OwnInstance => {
                    ctx.provide_at(
                        Key::<u64>::of().instance(ctx.instance()),
                        Arc::new(self.value),
                    )?;
                }
                Provide::GatedPlain => {
                    ctx.provide(self.value)?;
                    if applies >= 2 {
                        self.reached.notify_one();
                        self.release.notified().await;
                    }
                }
            }
            Ok(Effect::Done)
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum Own {
    Plain,
    Instance,
}

struct Reader {
    injects: Vec<TypeKey>,
    own: Option<Own>,
    slot: Arc<Mutex<Option<Ctx>>>,
}

impl DynPlugin for Reader {
    fn name(&self) -> &str {
        "reader"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> crate::BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            match self.own {
                None => {}
                Some(Own::Plain) => {
                    ctx.provide(3u64)?;
                }
                Some(Own::Instance) => {
                    ctx.provide_at(Key::<u64>::of().instance(ctx.instance()), Arc::new(3u64))?;
                }
            }
            *self.slot.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

fn probe(name: &'static str, injects: Vec<TypeKey>, provide: Provide, value: u64) -> Probe {
    Probe {
        name,
        injects,
        provide,
        value,
        slot: Arc::new(Mutex::new(None)),
        applies: Arc::new(AtomicUsize::new(0)),
        reached: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    }
}

fn reader(injects: Vec<TypeKey>, own: Option<Own>) -> Reader {
    Reader {
        injects,
        own,
        slot: Arc::new(Mutex::new(None)),
    }
}

/// A fiber that started: its view, the context its `apply` saw, and the
/// notifications a gated provider uses.
struct Started {
    view: FiberView,
    ctx: Ctx,
    reached: Arc<Notify>,
    release: Arc<Notify>,
}

impl Started {
    fn clone_view(&self) -> FiberView {
        self.view.clone()
    }
}

async fn started(parent: &Ctx, probe: Probe) -> Result<Started, TestCaseError> {
    let slot = probe.slot.clone();
    let reached = probe.reached.clone();
    let release = probe.release.clone();
    let view = parent.plugin(probe);
    (&view)
        .await
        .map_err(|error| TestCaseError::fail(format!("a plugin did not start: {error}")))?;
    let ctx = slot
        .lock()
        .unwrap()
        .take()
        .expect("a plugin that started ran apply");
    Ok(Started {
        view,
        ctx,
        reached,
        release,
    })
}

async fn started_reader(parent: &Ctx, reader: Reader) -> Result<Started, TestCaseError> {
    let slot = reader.slot.clone();
    let view = parent.plugin(reader);
    (&view)
        .await
        .map_err(|error| TestCaseError::fail(format!("a reader did not start: {error}")))?;
    let ctx = slot
        .lock()
        .unwrap()
        .take()
        .expect("a plugin that started ran apply");
    Ok(Started {
        view,
        ctx,
        reached: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    })
}

async fn dispose(started: &Started) -> Result<(), TestCaseError> {
    started
        .view
        .shutdown()
        .await
        .map_err(|error| TestCaseError::fail(format!("a plugin did not stop: {error}")))
}

// ── building and reading one scene ──────────────────────────────

struct Target {
    ctx: Ctx,
    key: Key<u64>,
    /// The key of another value type, for the TypeMismatch row.
    mismatch: Option<TypeKey>,
}

/// What has to be released before the case ends: the restart future of a
/// provider that is held in Loading, and the notification it waits for.
struct Gate {
    release: Arc<Notify>,
    restart: tokio::task::JoinHandle<Result<(), Arc<crate::error::ErrorKind>>>,
}

async fn build(
    root: &Ctx,
    scene: &Scene,
    views: &mut Vec<FiberView>,
    gate: &mut Option<Gate>,
) -> Result<Target, TestCaseError> {
    let key = TypeKey::of::<u64>();
    match scene.key {
        KeyKind::Plain => build_plain(root, scene, &key, views, gate).await,
        KeyKind::OwnInstance | KeyKind::AncestorInstance | KeyKind::ForeignInstance => {
            build_instance(root, scene, views).await
        }
    }
}

async fn build_plain(
    root: &Ctx,
    scene: &Scene,
    key: &TypeKey,
    views: &mut Vec<FiberView>,
    gate: &mut Option<Gate>,
) -> Result<Target, TestCaseError> {
    let scope_ctx = scene.reader_scope.ctx(root, key);

    // The provider that starts the declared fibers. With no provider left at
    // read time, it is removed after they started; with the final provider
    // in another scope, it is moved there after they started.
    let temporary = matches!(scene.provider, ProviderKind::None | ProviderKind::Other)
        && scene.declaration != Declaration::None;
    let mut provider: Option<Started> = None;
    let mut temp: Option<Started> = None;
    match scene.provider {
        ProviderKind::Same => {
            provider =
                Some(started(&scope_ctx, probe("provider", vec![], Provide::Plain, 1)).await?);
        }
        ProviderKind::LoadingSame => {
            provider = Some(
                started(
                    &scope_ctx,
                    probe("provider", vec![], Provide::GatedPlain, 1),
                )
                .await?,
            );
        }
        ProviderKind::None | ProviderKind::Other if temporary => {
            temp = Some(
                started(
                    &scope_ctx,
                    probe("temporary provider", vec![], Provide::Plain, 1),
                )
                .await?,
            );
        }
        ProviderKind::None | ProviderKind::Other => {}
    }

    // An ancestor in another scope is mounted at the root, where it needs a
    // provider of its own to start; the reader's binding lives in its scope.
    if scene.declaration == Declaration::AncestorOther {
        let parent_scope = Scope::Root;
        let parent_provider = started(
            &parent_scope.ctx(root, key),
            probe("parent provider", vec![], Provide::Plain, 1),
        )
        .await?;
        views.push(parent_provider.clone_view());
    }

    let (reader, reader_view) = match scene.declaration {
        Declaration::None => {
            let started =
                started_reader(&scope_ctx, reader(vec![], scene.own.then_some(Own::Plain))).await?;
            (started.ctx.clone(), started.clone_view())
        }
        Declaration::SelfFiber => {
            let started = started_reader(&scope_ctx, reader(vec![key.clone()], None)).await?;
            (started.ctx.clone(), started.clone_view())
        }
        Declaration::AncestorSame => {
            let parent = started(
                &scope_ctx,
                probe("parent", vec![key.clone()], Provide::Nothing, 0),
            )
            .await?;
            views.push(parent.clone_view());
            let started = started_reader(&parent.ctx, reader(vec![], None)).await?;
            (started.ctx.clone(), started.clone_view())
        }
        Declaration::AncestorOther => {
            let parent = started(
                &Scope::Root.ctx(root, key),
                probe("parent", vec![key.clone()], Provide::Nothing, 0),
            )
            .await?;
            views.push(parent.clone_view());
            let child_scope = parent.ctx.isolate(
                key.clone(),
                scene.reader_scope.label().expect("a non-root scope"),
            );
            let started = started_reader(&child_scope, reader(vec![], None)).await?;
            (started.ctx.clone(), started.clone_view())
        }
    };

    if let Some(temp) = temp.take() {
        dispose(&temp).await?;
        views.push(temp.clone_view());
    }
    if scene.provider == ProviderKind::Other {
        let other_scope = scene.reader_scope.other();
        let other = started(
            &other_scope.ctx(root, key),
            probe("other provider", vec![], Provide::Plain, 1),
        )
        .await?;
        views.push(other.clone_view());
    }
    if let Some(provider) = &provider {
        views.push(provider.clone_view());
    }
    views.push(reader_view.clone());

    if scene.caller == Caller::Disposed {
        reader_view
            .shutdown()
            .await
            .map_err(|error| TestCaseError::fail(format!("the reader did not stop: {error}")))?;
    }

    if scene.provider == ProviderKind::LoadingSame && scene.caller == Caller::Active {
        let provider = provider.as_ref().expect("a provider for the restart");
        let view = provider.clone_view();
        let reached = provider.reached.clone();
        let release = provider.release.clone();
        let restart = tokio::spawn(async move { view.restart().await });
        reached.notified().await;
        *gate = Some(Gate { release, restart });
    }

    Ok(Target {
        ctx: reader,
        key: Key::<u64>::of(),
        mismatch: scene.mismatch.then(TypeKey::of::<u8>),
    })
}

async fn build_instance(
    root: &Ctx,
    scene: &Scene,
    views: &mut Vec<FiberView>,
) -> Result<Target, TestCaseError> {
    let plain = TypeKey::of::<u64>();
    match scene.key {
        KeyKind::OwnInstance => {
            let started = started_reader(
                &Scope::Root.ctx(root, &plain),
                reader(vec![], Some(Own::Instance)),
            )
            .await?;
            let instance = started.ctx.instance();
            let view = started.clone_view();
            if scene.caller == Caller::Disposed {
                dispose(&started).await?;
            }
            views.push(view);
            Ok(Target {
                ctx: started.ctx,
                key: Key::<u64>::of().instance(instance),
                mismatch: scene.mismatch.then(|| TypeKey::instance::<u8>(instance)),
            })
        }
        KeyKind::AncestorInstance => {
            let provider = started(
                &Scope::Root.ctx(root, &plain),
                probe("ancestor provider", vec![], Provide::OwnInstance, 9),
            )
            .await?;
            let instance = provider.ctx.instance();
            let key = TypeKey::instance::<u64>(instance);
            let started = started_reader(&provider.ctx, reader(vec![key.clone()], None)).await?;
            let view = started.clone_view();
            if scene.caller == Caller::Disposed {
                dispose(&started).await?;
            }
            views.push(provider.clone_view());
            views.push(view);
            Ok(Target {
                ctx: started.ctx,
                key: Key::<u64>::of().instance(instance),
                mismatch: scene.mismatch.then(|| TypeKey::instance::<u8>(instance)),
            })
        }
        KeyKind::ForeignInstance => {
            let other = started(
                &Scope::Root.ctx(root, &plain),
                probe("foreign provider", vec![], Provide::OwnInstance, 9),
            )
            .await?;
            let instance = other.ctx.instance();
            let started =
                started_reader(&Scope::Root.ctx(root, &plain), reader(vec![], None)).await?;
            let view = started.clone_view();
            if scene.caller == Caller::Disposed {
                dispose(&started).await?;
            }
            views.push(other.clone_view());
            views.push(view);
            Ok(Target {
                ctx: started.ctx,
                key: Key::<u64>::of().instance(instance),
                mismatch: scene.mismatch.then(|| TypeKey::instance::<u8>(instance)),
            })
        }
        KeyKind::Plain => unreachable!("plain scenes are built by build_plain"),
    }
}

fn check_reads(target: &Target, facts: &Facts) -> Result<(), TestCaseError> {
    let expected = strict_result(facts);
    let actual = match &target.mismatch {
        Some(key) => target
            .ctx
            .require_provided_as::<u64>(key.clone(), &Provision::new())
            .map(|value| *value)
            .map_err(|error| error.reason),
        None => target
            .ctx
            .require_at(&target.key)
            .map(|value| *value)
            .map_err(|error| error.reason),
    };
    if actual != expected {
        return Err(TestCaseError::fail(format!(
            "the strict read gave {actual:?}, the table {expected:?} (facts {facts:?})"
        )));
    }
    let expected = optional_result(facts);
    let actual = target.ctx.get_at(&target.key).map(|value| *value);
    if actual != expected {
        return Err(TestCaseError::fail(format!(
            "the optional read gave {actual:?}, the table {expected:?} (facts {facts:?})"
        )));
    }
    Ok(())
}

async fn run(scene: Scene) -> Result<(), TestCaseError> {
    let root = Ctx::root().map_err(|error| {
        TestCaseError::fail(format!("a root context was not available: {error}"))
    })?;
    let mut views = Vec::new();
    let mut gate = None;
    let target = build(&root, &scene, &mut views, &mut gate).await?;
    let outcome = check_reads(&target, &scene.facts());
    if let Some(gate) = gate {
        gate.release.notify_one();
        gate.restart
            .await
            .map_err(|error| TestCaseError::fail(format!("the restart task failed: {error}")))?
            .map_err(|error| {
                TestCaseError::fail(format!("the provider did not restart: {error}"))
            })?;
    }
    for view in views {
        let _ = view.shutdown().await;
    }
    let _ = root.shutdown().await;
    outcome
}

// ── the property test ───────────────────────────────────────────

/// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED` or a
/// fresh one; on failure, panic with the seed and the shrunk input.
fn check<S: Strategy>(name: &str, strategy: S, test: impl Fn(S::Value) -> Result<(), TestCaseError>)
where
    S::Value: std::fmt::Debug,
{
    let seed: u64 = match std::env::var("RUTIS_PROPTEST_SEED") {
        Ok(seed) => seed.parse().expect("RUTIS_PROPTEST_SEED is a u64"),
        Err(_) => {
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            );
            hasher.finish()
        }
    };
    let mut bytes = [0u8; 32];
    for chunk in bytes.chunks_mut(8) {
        chunk.copy_from_slice(&seed.to_le_bytes());
    }
    // `Config::default()` reads PROPTEST_CASES (256 when unset).
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let cases = config.cases;
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    let started = std::time::Instant::now();
    if let Err(error) = runner.run(&strategy, test) {
        panic!("{name}: {error}\nreplay with RUTIS_PROPTEST_SEED={seed}");
    }
    eprintln!(
        "{name}: {cases} cases in {:?} (seed {seed})",
        started.elapsed()
    );
}

#[test]
fn lookup_scope_and_strict_reads_match_the_decision_table() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    check(
        "generated readers, declarations and providers",
        scenes(),
        |scene| runtime.block_on(run(scene)),
    );
}

/// One scene per row of the table, so a default-case run (no `PROPTEST_CASES`)
/// still covers every row: the random cases are the breadth, these are the
/// rows a regression must not lose.
#[test]
fn every_row_of_the_table_is_covered() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let scene = |key, reader_scope, declaration, provider, own, caller, mismatch| Scene {
        key,
        reader_scope,
        declaration,
        provider,
        own,
        caller,
        mismatch,
    };
    use Caller::{Active, Disposed};
    use Declaration::{AncestorOther, AncestorSame, None as Undeclared, SelfFiber};
    use KeyKind::{AncestorInstance, ForeignInstance, OwnInstance, Plain};
    use ProviderKind::{LoadingSame, None as NoProvider, Other, Same};
    use Scope::{Root, A, B};

    let scenes = [
        // 1. out of the caller's instance ancestry, before every other row.
        scene(
            ForeignInstance,
            Root,
            Undeclared,
            NoProvider,
            false,
            Active,
            false,
        ), // OutOfScope
        scene(
            ForeignInstance,
            Root,
            Undeclared,
            NoProvider,
            false,
            Disposed,
            true,
        ), // OutOfScope
        // 2. another value type, before the caller's state.
        scene(Plain, Root, Undeclared, NoProvider, false, Active, true), // TypeMismatch
        scene(Plain, Root, Undeclared, NoProvider, false, Disposed, true), // TypeMismatch
        scene(
            OwnInstance,
            Root,
            Undeclared,
            NoProvider,
            true,
            Active,
            true,
        ), // TypeMismatch
        // 3. an inactive caller, before the declaration.
        scene(Plain, Root, Undeclared, NoProvider, false, Disposed, false), // Inactive
        scene(Plain, Root, SelfFiber, Same, false, Disposed, false),        // Inactive
        scene(
            AncestorInstance,
            Root,
            Undeclared,
            NoProvider,
            false,
            Disposed,
            false,
        ), // Inactive
        scene(Plain, Root, Undeclared, NoProvider, true, Disposed, false),  // Inactive
        scene(
            OwnInstance,
            Root,
            Undeclared,
            NoProvider,
            true,
            Disposed,
            false,
        ), // Inactive
        // 4. no declaration: the binding's presence does not help.
        scene(Plain, Root, Undeclared, NoProvider, false, Active, false), // Undeclared, no binding
        scene(Plain, Root, Undeclared, Same, false, Active, false), // Undeclared, binding visible
        scene(Plain, A, AncestorOther, Same, false, Active, false), // Undeclared, other scope
        // 5. own service, with and without a declaration.
        scene(Plain, Root, Undeclared, NoProvider, true, Active, false), // Ok, own
        scene(
            OwnInstance,
            Root,
            Undeclared,
            NoProvider,
            true,
            Active,
            false,
        ), // Ok, own instance
        // 6/7. declared: missing, removed, other scope, loading, active.
        scene(Plain, A, SelfFiber, NoProvider, false, Active, false), // Missing
        scene(Plain, B, SelfFiber, Other, false, Active, false),      // Missing, other scope
        scene(Plain, Root, SelfFiber, LoadingSame, false, Active, false), // ProviderLoading
        scene(Plain, Root, SelfFiber, Same, false, Active, false),    // Ok
        scene(Plain, A, AncestorSame, Same, false, Active, false),    // Ok, declared by ancestor
        scene(
            AncestorInstance,
            Root,
            Undeclared,
            NoProvider,
            false,
            Active,
            false,
        ), // Ok, ancestor instance
    ];
    for scene in scenes {
        let facts = scene.facts();
        runtime
            .block_on(run(scene.clone()))
            .unwrap_or_else(|error| panic!("scene {scene:?} (table {facts:?}): {error:?}"));
    }
}
