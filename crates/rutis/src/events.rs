//! Events: one listener table, five dispatch modes (design
//! `docs/design-events-2026-10-18.md`, protocol spec
//! `docs/design-events-protocol-2026-10-18.md` §12.1).
//!
//! A listener registered once is called by every mode; the emitter chooses
//! the mode. Listeners are found by the event's **name** (`E::NAME` for
//! [`EventKey::of`](crate::EventKey::of)), so listeners of another Rust
//! type, or in another process, hear the same events; serde converts
//! between the types.

use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::bus::run::{AsyncNext, Inv, SyncNextArg};
use crate::ctx::Ctx;
use crate::error::{CordisError, NextMisuse};
use crate::BoxFuture;

mod args;
pub(crate) mod remote;
pub(crate) mod wire;

pub use remote::{
    BatchFilter, Candidate, Dispatch, EntryCall, FilterAnswer, RawTerminal, RemoteCall,
    RemoteEntry, RemoteListener, RemoteNext, RemoteSyncCall, RemoteSyncNext, TableChange,
    TableEntry,
};
pub use wire::{Codec, JsonTranscoder, Payload, PayloadRef, Transcoder, Value};

/// The reserved namespace of framework events (protocol spec §8). Plugins
/// listen to them; only rutis emits them.
pub const RESERVED_PREFIX: &str = "rutis/";

/// A typed event. `NAME` is the event's name in every language and process
/// ([`EventKey::of`](crate::EventKey::of) dispatches under it). The event and
/// its `Value` are serde, so it crosses to listeners of other types and to
/// other processes automatically (derive `Serialize` and `Deserialize`).
///
/// An empty `NAME` says the type has no name of its own (a dynamic event,
/// `rutis_bridge::events::Args`): only keys that give one
/// ([`EventKey::dynamic`](crate::EventKey::dynamic), `named`) register or
/// dispatch it; `EventKey::of()` fails with `EventNeedsName`.
pub trait Event: Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
    /// The result type: what `serial`, `bail_sync` and a waterfall return.
    type Value: Serialize + DeserializeOwned + Send + 'static;
}

/// A dispatch mode (protocol spec §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Emit,
    Parallel,
    Serial,
    Waterfall,
    Bail,
    WaterfallSync,
}

impl Mode {
    /// The protocol name: `emit`, `parallel`, `serial`, `waterfall`,
    /// `bail`, `waterfall-sync`.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Emit => "emit",
            Mode::Parallel => "parallel",
            Mode::Serial => "serial",
            Mode::Waterfall => "waterfall",
            Mode::Bail => "bail",
            Mode::WaterfallSync => "waterfall-sync",
        }
    }

    /// `bail` and `waterfall-sync`: every call is synchronous.
    pub fn is_sync(self) -> bool {
        matches!(self, Mode::Bail | Mode::WaterfallSync)
    }

    pub fn is_waterfall(self) -> bool {
        matches!(self, Mode::Waterfall | Mode::WaterfallSync)
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One element of a scope chain: a loader group instance (protocol spec §3).
/// It is a descriptor for filters; event delivery is by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    /// Unique among all instances, past and present (Rust: the decimal
    /// `InstanceId` of the group's fiber).
    pub id: String,
    pub group: String,
}

/// `inner` is `outer` or below it: `outer` is a prefix of `inner`, by id.
pub fn scope_contains(outer: &[Scope], inner: &[Scope]) -> bool {
    outer.len() <= inner.len() && outer.iter().zip(inner).all(|(a, b)| a.id == b.id)
}

/// What the bus says about a listener: what a filter sees (protocol spec
/// §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerInfo {
    /// The event name, or the prefix of a prefix entry.
    pub name: String,
    pub prefix: bool,
    /// The plugin that registered it.
    pub plugin: String,
    /// The row it belongs to ([`Ctx::mark_row`]), if any.
    pub row: Option<String>,
    /// The runtime it lives in; `None` for a Rust listener of this host.
    pub runtime: Option<String>,
    /// The node it was imported from; `None` for this node's own.
    pub node: Option<String>,
    /// Its scope chain, outermost first; `[]` at the root. A descriptor for
    /// filters, not a delivery rule.
    pub scope: Vec<Scope>,
    /// Custom filters skip it.
    pub global: bool,
}

impl ListenerInfo {
    /// How errors name it: `row <row>`, `runtime <runtime>`, `node <node>`
    /// or `plugin <plugin>`.
    pub fn describe(&self) -> String {
        if let Some(row) = &self.row {
            format!("row {row}")
        } else if let Some(runtime) = &self.runtime {
            format!("runtime {runtime}")
        } else if let Some(node) = &self.node {
            format!("node {node}")
        } else {
            format!("plugin {}", self.plugin)
        }
    }
}

/// Whether a listener is known to be synchronous (protocol spec §4.2
/// `kind`). Rust `on` listeners are `Async`, `on_sync` ones `Sync`; `Any`
/// only comes from remote listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenerKind {
    Sync,
    Async,
    Any,
}

/// Registration options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EventOptions {
    /// Take a position before every existing listener.
    pub prepend: bool,
    /// Called at most once: the first dispatch that selects it claims it.
    pub once: bool,
    /// Custom filters skip it.
    pub global: bool,
}

impl EventOptions {
    pub const fn prepend(mut self, prepend: bool) -> Self {
        self.prepend = prepend;
        self
    }

    pub const fn once(mut self, once: bool) -> Self {
        self.once = once;
        self
    }

    pub const fn global(mut self, global: bool) -> Self {
        self.global = global;
        self
    }
}

fn into_typed<V: Serialize + DeserializeOwned + Send + 'static>(
    inv: &Inv<'_>,
    value: Option<Value>,
) -> Result<Option<V>, CordisError> {
    value
        .map(|value| value.into_typed::<V>(&*inv.bus.transcoder()))
        .transpose()
}

/// One call of an async listener: which mode, and the rest of a waterfall.
pub struct Call<'a, V> {
    pub(crate) inv: &'a Inv<'a>,
    pub(crate) next: Option<AsyncNext<'a>>,
    pub(crate) marker: PhantomData<fn() -> V>,
}

impl<'a, V: Serialize + DeserializeOwned + Send + 'static> Call<'a, V> {
    pub fn mode(&self) -> Mode {
        self.inv.mode
    }

    /// The dispatched name (for a prefix listener, the full name).
    pub fn name(&self) -> &str {
        self.inv.name
    }

    /// The rest of the chain (`waterfall` only; otherwise `Err(WaterfallNext)`).
    /// Its value is the next listener's, as is: `None` when it had none.
    pub fn next(self) -> BoxFuture<'a, Result<Option<V>, CordisError>> {
        let inv = self.inv;
        match self.next {
            Some(next) => Box::pin(async move { into_typed(inv, next.invoke().await?) }),
            None => Box::pin(std::future::ready(Err(CordisError::WaterfallNext {
                name: inv.name.to_owned(),
                misuse: NextMisuse::NotWaterfall(inv.mode),
            }))),
        }
    }
}

/// One call of a synchronous listener.
pub struct SyncCall<'a, V> {
    pub(crate) inv: &'a Inv<'a>,
    pub(crate) next: SyncNextArg<'a>,
    pub(crate) marker: PhantomData<fn() -> V>,
}

impl<V: Serialize + DeserializeOwned + Send + 'static> SyncCall<'_, V> {
    pub fn mode(&self) -> Mode {
        self.inv.mode
    }

    pub fn name(&self) -> &str {
        self.inv.name
    }

    /// The rest of the chain: `waterfall_sync` only. In an async
    /// `waterfall`, and in every other mode, `Err(WaterfallNext)`.
    pub fn next(self) -> Result<Option<V>, CordisError> {
        let inv = self.inv;
        let misuse = match self.next {
            SyncNextArg::Next(next) => return into_typed(inv, next.invoke()?),
            SyncNextArg::InAsyncWaterfall => NextMisuse::SyncInAsync,
            SyncNextArg::None => NextMisuse::NotWaterfall(inv.mode),
        };
        Err(CordisError::WaterfallNext {
            name: inv.name.to_owned(),
            misuse,
        })
    }
}

/// An async listener: called by every mode except `bail` / `waterfall-sync`
/// (there it is an `AsyncListener` error).
///
/// `Ok(None)` and `Ok(Some(false))` are "no result"; `serial` goes on.
pub trait Listener<E: Event>: Send + Sync + 'static {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        event: &'a E,
        call: Call<'a, E::Value>,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
}

impl<E: Event, F> Listener<E> for F
where
    F: for<'a> Fn(
            &'a Ctx,
            &'a E,
            Call<'a, E::Value>,
        ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>
        + Send
        + Sync
        + 'static,
{
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        event: &'a E,
        call: Call<'a, E::Value>,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        self(ctx, event, call)
    }
}

/// A synchronous listener: called by every mode, inline.
pub trait SyncListener<E: Event>: Send + Sync + 'static {
    fn call(
        &self,
        ctx: &Ctx,
        event: &E,
        call: SyncCall<'_, E::Value>,
    ) -> Result<Option<E::Value>, CordisError>;
}

impl<E: Event, F> SyncListener<E> for F
where
    F: Fn(&Ctx, &E, SyncCall<'_, E::Value>) -> Result<Option<E::Value>, CordisError>
        + Send
        + Sync
        + 'static,
{
    fn call(
        &self,
        ctx: &Ctx,
        event: &E,
        call: SyncCall<'_, E::Value>,
    ) -> Result<Option<E::Value>, CordisError> {
        self(ctx, event, call)
    }
}

/// An async prefix listener: also gets the dispatched name as a key.
pub trait PatternListener<E: Event>: Send + Sync + 'static {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        key: crate::EventKey<E>,
        event: &'a E,
        call: Call<'a, E::Value>,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
}

impl<E: Event, F> PatternListener<E> for F
where
    F: for<'a> Fn(
            &'a Ctx,
            crate::EventKey<E>,
            &'a E,
            Call<'a, E::Value>,
        ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>
        + Send
        + Sync
        + 'static,
{
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        key: crate::EventKey<E>,
        event: &'a E,
        call: Call<'a, E::Value>,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        self(ctx, key, event, call)
    }
}

/// A synchronous prefix listener.
pub trait SyncPatternListener<E: Event>: Send + Sync + 'static {
    fn call(
        &self,
        ctx: &Ctx,
        key: crate::EventKey<E>,
        event: &E,
        call: SyncCall<'_, E::Value>,
    ) -> Result<Option<E::Value>, CordisError>;
}

impl<E: Event, F> SyncPatternListener<E> for F
where
    F: Fn(
            &Ctx,
            crate::EventKey<E>,
            &E,
            SyncCall<'_, E::Value>,
        ) -> Result<Option<E::Value>, CordisError>
        + Send
        + Sync
        + 'static,
{
    fn call(
        &self,
        ctx: &Ctx,
        key: crate::EventKey<E>,
        event: &E,
        call: SyncCall<'_, E::Value>,
    ) -> Result<Option<E::Value>, CordisError> {
        self(ctx, key, event, call)
    }
}

/// The end of a waterfall: what the last listener's `next` returns. `()`
/// is "no terminal": its `next` gets `None`.
pub trait Terminal<E: Event>: Send + 'static {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        event: &'a E,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
}

impl<E: Event, F> Terminal<E> for F
where
    F: for<'a> Fn(&'a Ctx, &'a E) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>
        + Send
        + 'static,
{
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        event: &'a E,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        self(ctx, event)
    }
}

impl<E: Event> Terminal<E> for () {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: &'a E,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}

// ── typed adapters ───────────────────────────────────────────────

/// The event as `E`: itself when the dispatch carries an `E`, otherwise
/// decoded through the bus's transcoder.
pub(crate) enum Typed<'a, E> {
    Borrowed(&'a E),
    Owned(Box<E>),
}

impl<E> std::ops::Deref for Typed<'_, E> {
    type Target = E;
    fn deref(&self) -> &E {
        match self {
            Typed::Borrowed(e) => e,
            Typed::Owned(e) => e,
        }
    }
}

pub(crate) fn typed<'a, E: Event>(inv: &Inv<'a>) -> Result<Typed<'a, E>, CordisError> {
    match inv.event.downcast::<E>() {
        Some(event) => Ok(Typed::Borrowed(event)),
        None => wire::decode_event::<E>(&*inv.bus.transcoder(), inv.event).map(Typed::Owned),
    }
}

pub(crate) trait ErasedAsync: Send + Sync + 'static {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        inv: &'a Inv<'a>,
        next: Option<AsyncNext<'a>>,
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
}

pub(crate) trait ErasedSync: Send + Sync + 'static {
    fn call(
        &self,
        ctx: &Ctx,
        inv: &Inv<'_>,
        next: SyncNextArg<'_>,
    ) -> Result<Option<Value>, CordisError>;
}

pub(crate) struct AsyncAdapter<L, E>(pub L, pub PhantomData<fn(E) -> E>);
pub(crate) struct AsyncPatternAdapter<L, E>(pub L, pub PhantomData<fn(E) -> E>);
pub(crate) struct SyncAdapter<L, E>(pub L, pub PhantomData<fn(E) -> E>);
pub(crate) struct SyncPatternAdapter<L, E>(pub L, pub PhantomData<fn(E) -> E>);

impl<E: Event, L: Listener<E>> ErasedAsync for AsyncAdapter<L, E> {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        inv: &'a Inv<'a>,
        next: Option<AsyncNext<'a>>,
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        Box::pin(async move {
            let event = typed::<E>(inv)?;
            let call = Call {
                inv,
                next,
                marker: PhantomData,
            };
            let out = self.0.call(ctx, &event, call).await?;
            Ok(out.map(Value::new))
        })
    }
}

impl<E: Event, L: PatternListener<E>> ErasedAsync for AsyncPatternAdapter<L, E> {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        inv: &'a Inv<'a>,
        next: Option<AsyncNext<'a>>,
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        Box::pin(async move {
            let event = typed::<E>(inv)?;
            let key = crate::EventKey::<E>::dynamic(inv.name);
            let call = Call {
                inv,
                next,
                marker: PhantomData,
            };
            let out = self.0.call(ctx, key, &event, call).await?;
            Ok(out.map(Value::new))
        })
    }
}

impl<E: Event, L: SyncListener<E>> ErasedSync for SyncAdapter<L, E> {
    fn call(
        &self,
        ctx: &Ctx,
        inv: &Inv<'_>,
        next: SyncNextArg<'_>,
    ) -> Result<Option<Value>, CordisError> {
        let event = typed::<E>(inv)?;
        let call = SyncCall {
            inv,
            next,
            marker: PhantomData,
        };
        Ok(self.0.call(ctx, &event, call)?.map(Value::new))
    }
}

impl<E: Event, L: SyncPatternListener<E>> ErasedSync for SyncPatternAdapter<L, E> {
    fn call(
        &self,
        ctx: &Ctx,
        inv: &Inv<'_>,
        next: SyncNextArg<'_>,
    ) -> Result<Option<Value>, CordisError> {
        let event = typed::<E>(inv)?;
        let key = crate::EventKey::<E>::dynamic(inv.name);
        let call = SyncCall {
            inv,
            next,
            marker: PhantomData,
        };
        Ok(self.0.call(ctx, key, &event, call)?.map(Value::new))
    }
}

/// 内联 poll panic 捕获(apply 在驱动任务内联执行,无法经 JoinError 边界)。
pub(crate) struct CatchUnwind<F>(Pin<Box<F>>);

impl<F> CatchUnwind<F> {
    pub(crate) fn new(fut: F) -> Self {
        Self(Box::pin(fut))
    }
}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let inner =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.0.as_mut().poll(cx)));
        match inner {
            Ok(poll) => poll.map(Ok),
            Err(p) => Poll::Ready(Err(p)),
        }
    }
}
