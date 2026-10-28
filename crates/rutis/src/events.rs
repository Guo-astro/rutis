//! Events: one listener table, five dispatch modes (design
//! `docs/design-events-2026-10-18.md`, protocol spec
//! `docs/design-events-protocol-2026-10-18.md` §12.1).
//!
//! A listener registered once is called by every mode; the emitter chooses
//! the mode. Listeners are found by the event's **name** (`E::NAME`, or
//! the value's own [`Event::name`]), so listeners of another Rust type, or
//! in another process, hear the same events; serde converts between the
//! types.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::bus::run::{AsyncNext, Inv, SyncNextArg};
use crate::ctx::Ctx;
use crate::error::Error;
use crate::BoxFuture;

mod api;
mod args;
pub(crate) mod kinds;
pub(crate) mod remote;
pub(crate) mod wire;

pub use api::{Call, Filtered, Listen, Listener, SyncCall, Waterfall};
pub(crate) use into_event::IntoEvent;

/// What the dispatch methods take: an event or an `Arc` of one, so the
/// event type is inferred from either. Not nameable outside rutis (it
/// lives in a private module), so it adds no name to the API.
mod into_event {
    use std::sync::Arc;

    #[diagnostic::on_unimplemented(
        message = "`{Self}` is not an event: it does not implement `rutis::Event`",
        label = "not an event",
        note = "implement `rutis::Event` for the type (a `NAME` and a `Value`) and derive serde `Serialize` and `Deserialize`; then pass the event or an `Arc` of it"
    )]
    pub trait IntoEvent<E: super::Event> {
        #[doc(hidden)]
        fn into_event(self) -> Arc<E>;
    }

    impl<E: super::Event> IntoEvent<E> for E {
        fn into_event(self) -> Arc<E> {
            Arc::new(self)
        }
    }

    #[diagnostic::do_not_recommend]
    impl<E: super::Event> IntoEvent<E> for Arc<E> {
        fn into_event(self) -> Arc<E> {
            self
        }
    }
}

pub(crate) use kinds::{EventOptions, ListenerKind, RESERVED_PREFIX};
#[allow(unused_imports)]
pub(crate) use remote::{
    BatchFilter, Candidate, Dispatch, EntryCall, FilterAnswer, RawTerminal, RemoteCall,
    RemoteEntry, RemoteListener, RemoteNext, RemoteSyncCall, RemoteSyncNext, TableChange,
    TableEntry,
};
#[allow(unused_imports)]
pub(crate) use wire::{Codec, JsonTranscoder, Payload, PayloadRef, Transcoder, Value};

/// A typed event. `NAME` is the event's name in every language and process:
/// `ctx.on::<E>()` listens under it and `ctx.emit(e)` dispatches under it.
/// The event and its `Value` are serde, so it crosses to listeners of other
/// types and to other processes automatically (derive `Serialize` and
/// `Deserialize`).
///
/// ```
/// use rutis::serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct Created(String, u32); // emit("session/created", id, size) elsewhere
///
/// impl rutis::Event for Created {
///     const NAME: &'static str = "session/created";
///     type Value = String;
/// }
/// ```
///
/// A type with no name of its own (an empty `NAME`, such as
/// `rutis_bridge::events::Args`) overrides [`Event::name`] so each value
/// carries its name; listen to it with `ctx.listen::<E>().name(..)`.
/// Dispatching or listening under an empty name fails with
/// `EventNeedsName`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an event: it does not implement `rutis::Event`",
    label = "not an event",
    note = "implement `rutis::Event` for `{Self}` (a `NAME` and a `Value`) and derive serde `Serialize` and `Deserialize`"
)]
pub trait Event: Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
    /// The result type: what `serial`, `bail` and a waterfall return.
    type Value: Serialize + DeserializeOwned + Send + 'static;

    /// The name this value is dispatched under: `NAME` unless the type
    /// carries its own.
    fn name(&self) -> &str {
        Self::NAME
    }
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

/// What the bus says about a listener: what a filter sees (protocol spec
/// §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerInfo {
    /// The event name, or the prefix of a prefix entry.
    pub name: String,
    pub prefix: bool,
    /// The plugin that registered it.
    pub plugin: String,
    /// The row it belongs to ([`CtxInternal::mark_row`](crate::internal::CtxInternal::mark_row)), if any.
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

pub(crate) fn into_typed<V: Serialize + DeserializeOwned + Send + 'static>(
    inv: &Inv<'_>,
    value: Option<Value>,
) -> Result<Option<V>, Error> {
    value
        .map(|value| value.into_typed::<V>(&*inv.bus.transcoder()))
        .transpose()
}

/// The event as an `Arc<E>`: the dispatch's own when it carries an `E`,
/// otherwise decoded through the bus's transcoder.
pub(crate) fn typed_arc<E: Event>(inv: &Inv<'_>) -> Result<Arc<E>, Error> {
    if let Some(event) = inv.event.shared::<E>() {
        return Ok(event);
    }
    wire::decode_event::<E>(&*inv.bus.transcoder(), inv.event).map(|event| Arc::new(*event))
}

pub(crate) trait ErasedAsync: Send + Sync + 'static {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        inv: &'a Inv<'a>,
        next: Option<AsyncNext>,
    ) -> BoxFuture<'a, Result<Option<Value>, Error>>;
}

pub(crate) trait ErasedSync: Send + Sync + 'static {
    fn call(&self, ctx: &Ctx, inv: &Inv<'_>, next: SyncNextArg<'_>)
        -> Result<Option<Value>, Error>;
}

/// Catches a panic while polling a future inline. `apply` runs inline in the
/// driver task, so there is no `JoinError` to catch it.
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
