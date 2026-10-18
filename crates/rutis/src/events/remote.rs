//! Remote listener entries and the raw dispatch API: what rutis-bridge
//! builds the logical bus on (protocol spec §4, §5, §9).
//!
//! A **remote listener** is an entry of the one listener table whose calls
//! are delegated to an external dispatcher (a runtime process, a node link).
//! It takes a position like any entry (registration order, `prepend`), is
//! selected by the scope rule from its [`ListenerInfo::scope`], is seen by
//! filters through its [`ListenerInfo`], skips custom filters when
//! `global`, and is withdrawn by its [`Disposer`](crate::Disposer).
//!
//! Synchronous modes (`bail`, `waterfall-sync`) call
//! [`RemoteListener::call_sync`] inline on the dispatching thread with no
//! kernel lock held, so a bridge's thread-local sync-wait path (cycle
//! detection, `SyncWaitCycle`) applies to it as to any synchronous call. A
//! remote entry of kind [`ListenerKind::Async`] is never called by them:
//! the dispatch fails with `AsyncListener`.

use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use super::{EventOptions, ListenerInfo, ListenerKind, Mode, PayloadRef, Scope, Value};
use crate::bus::run::Inv;
use crate::error::NextMisuse;
use crate::{BoxFuture, CordisError, Ctx};

/// A listener whose calls go to an external dispatcher.
pub trait RemoteListener: Send + Sync + 'static {
    /// `Async` entries are never called by `bail` / `waterfall-sync`.
    fn kind(&self) -> ListenerKind;

    /// One call by `emit`, `parallel`, `serial` or `waterfall`.
    ///
    /// The function itself is called in registration order when the call
    /// starts (for `emit`, before the raw dispatch future resolves), so a
    /// frame written eagerly inside `call` keeps the start order. Dropping
    /// the future cancels the call (protocol spec §5.7). In `emit`, an error
    /// is reported to the error sink.
    fn call<'a>(
        &'a self,
        call: RemoteCall<'a>,
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;

    /// One call by `bail` or `waterfall-sync`, on the dispatching thread.
    fn call_sync(&self, call: RemoteSyncCall<'_>) -> Result<Option<Value>, CordisError>;
}

/// One async call of a remote entry.
pub struct RemoteCall<'a> {
    pub(crate) inv: &'a Inv<'a>,
    pub(crate) next: Option<RemoteNext>,
}

impl<'a> RemoteCall<'a> {
    /// The dispatched name (for a prefix entry, the full name).
    pub fn name(&self) -> &'a str {
        self.inv.name
    }
    pub fn mode(&self) -> Mode {
        self.inv.mode
    }
    /// The same for every call of one dispatch; unique in this bus.
    pub fn dispatch(&self) -> u64 {
        self.inv.dispatch
    }
    /// The `origin` of a raw [`Dispatch`]; `None` for a typed one.
    pub fn origin(&self) -> Option<u64> {
        self.inv.origin
    }
    /// The emitter's context.
    pub fn emitter(&self) -> &'a Ctx {
        self.inv.emitter
    }
    pub fn event(&self) -> PayloadRef<'a> {
        self.inv.event
    }
    /// The rest of the chain: `waterfall` only.
    pub fn next(&self) -> Option<RemoteNext> {
        self.next.clone()
    }
}

/// One synchronous call of a remote entry.
pub struct RemoteSyncCall<'a> {
    pub(crate) inv: &'a Inv<'a>,
    pub(crate) next: Option<RemoteSyncNext>,
}

impl<'a> RemoteSyncCall<'a> {
    pub fn name(&self) -> &'a str {
        self.inv.name
    }
    pub fn mode(&self) -> Mode {
        self.inv.mode
    }
    pub fn dispatch(&self) -> u64 {
        self.inv.dispatch
    }
    pub fn origin(&self) -> Option<u64> {
        self.inv.origin
    }
    pub fn emitter(&self) -> &'a Ctx {
        self.inv.emitter
    }
    pub fn event(&self) -> PayloadRef<'a> {
        self.inv.event
    }
    /// The rest of the chain: `waterfall-sync` only.
    pub fn next(&self) -> Option<RemoteSyncNext> {
        self.next.clone()
    }
}

type NextReply = tokio::sync::oneshot::Sender<Result<Option<Value>, CordisError>>;

/// The rest of an async waterfall, for a remote entry. `'static`: it can
/// be handed out as a function reference. The first call runs the rest of
/// the chain; a second fails with `WaterfallNext` ("already called"), and so
/// does a call after the entry's call ended or the dispatch was cancelled.
#[derive(Clone)]
pub struct RemoteNext {
    pub(crate) name: Arc<str>,
    pub(crate) called: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) requests: tokio::sync::mpsc::UnboundedSender<NextReply>,
}

impl RemoteNext {
    pub fn call(&self) -> BoxFuture<'static, Result<Option<Value>, CordisError>> {
        let misuse = |misuse| CordisError::WaterfallNext {
            name: self.name.to_string(),
            misuse,
        };
        if self.called.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(std::future::ready(Err(misuse(NextMisuse::AlreadyCalled))));
        }
        let (reply, answer) = tokio::sync::oneshot::channel();
        if self.requests.send(reply).is_err() {
            return Box::pin(std::future::ready(Err(misuse(NextMisuse::Ended))));
        }
        let ended = misuse(NextMisuse::Ended);
        Box::pin(async move { answer.await.unwrap_or(Err(ended)) })
    }
}

pub(crate) type SyncNextFn = dyn Fn() -> Result<Option<Value>, CordisError>;

pub(crate) struct SyncNextState {
    /// The rest of the chain on the dispatching thread's stack; `None` once
    /// the entry's call returned.
    pub(crate) run: Option<*const SyncNextFn>,
    pub(crate) thread: ThreadId,
    pub(crate) called: bool,
}

// SAFETY: `run` is only dereferenced on `thread`, while the call that set it
// is still on that thread's stack (it clears `run` before returning).
unsafe impl Send for SyncNextState {}

/// The rest of a synchronous waterfall, for a remote entry. `'static`, but
/// it runs only on the dispatching thread while the entry's `call_sync` is
/// running (a nested call, as a sync-wait pump makes); otherwise it fails
/// with `WaterfallNext`. Called at most once.
#[derive(Clone)]
pub struct RemoteSyncNext {
    pub(crate) name: Arc<str>,
    pub(crate) state: Arc<Mutex<SyncNextState>>,
}

impl RemoteSyncNext {
    pub fn call(&self) -> Result<Option<Value>, CordisError> {
        let misuse = |misuse| CordisError::WaterfallNext {
            name: self.name.to_string(),
            misuse,
        };
        let run = {
            let mut state = self.state.lock().unwrap();
            if state.called {
                return Err(misuse(NextMisuse::AlreadyCalled));
            }
            match state.run {
                Some(run) if state.thread == std::thread::current().id() => {
                    state.called = true;
                    run
                }
                Some(_) => return Err(misuse(NextMisuse::OtherThread)),
                None => return Err(misuse(NextMisuse::Ended)),
            }
        };
        // SAFETY: see `SyncNextState`: same thread, and `run` is still set,
        // so the frame holding the closure has not returned.
        unsafe { (*run)() }
    }
}

/// A remote entry to register with [`EventBus::on_remote`](crate::EventBus::on_remote).
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    /// The event name, or the prefix when `prefix`.
    pub name: String,
    pub prefix: bool,
    /// Its descriptor. `name`, `prefix` and `global` are taken from this
    /// entry and its options.
    pub info: ListenerInfo,
    /// `prepend`, `once` and `global`.
    pub options: EventOptions,
    /// An opaque tag reported by [`TableEntry::origin`] (which session it
    /// came through).
    pub origin: u64,
}

/// A table entry, as [`EventBus::watch_table`](crate::EventBus::watch_table) reports it.
#[derive(Debug, Clone)]
pub struct TableEntry {
    /// Stable for the entry's life; [`EventBus::invoke_entry`](crate::EventBus::invoke_entry) takes it.
    pub id: u64,
    pub name: String,
    pub prefix: bool,
    /// `None` for a Rust listener of this host; the [`RemoteEntry::origin`]
    /// of a remote one.
    pub origin: Option<u64>,
    pub kind: ListenerKind,
    pub info: Arc<ListenerInfo>,
    /// Registered with `prepend`.
    pub prepend: bool,
    /// Its place in the table: smaller first (§4.3).
    pub position: i64,
}

/// A change of the listener table.
#[derive(Debug, Clone)]
pub enum TableChange {
    Added(TableEntry),
    Removed(TableEntry),
}

/// What a batch filter answers: one `bool` per listener, `true` = deliver.
pub enum FilterAnswer {
    Ready(Result<Vec<bool>, CordisError>),
    /// Not allowed in `bail` / `waterfall-sync`.
    Pending(BoxFuture<'static, Result<Vec<bool>, CordisError>>),
}

/// A filter called once per dispatch with the descriptors of the non-global
/// candidates, in registration order (protocol spec §6).
pub type BatchFilter = Box<dyn FnOnce(Vec<ListenerInfo>) -> FilterAnswer + Send>;

/// A waterfall's terminal, or a remote entry's `next` (then the payload is
/// that call's event).
pub enum RawTerminal {
    Async(
        Box<
            dyn for<'p> FnOnce(
                    PayloadRef<'p>,
                )
                    -> BoxFuture<'static, Result<Option<Value>, CordisError>>
                + Send,
        >,
    ),
    Sync(Box<dyn for<'p> FnOnce(PayloadRef<'p>) -> Result<Option<Value>, CordisError> + Send>),
}

/// A dispatch whose event is only known erased: what a host dispatches on
/// behalf of a runtime (protocol spec §5.3 `events.dispatch`).
pub struct Dispatch {
    pub name: String,
    pub mode: Mode,
    pub payload: super::Payload,
    /// The emitter's scope chain; `None`: the emitting context's.
    pub scope: Option<Vec<Scope>>,
    pub filter: Option<BatchFilter>,
    /// `waterfall` / `waterfall-sync` only.
    pub terminal: Option<RawTerminal>,
    /// Reported to remote entries by [`RemoteCall::origin`].
    pub origin: Option<u64>,
    /// Only rutis's own framework events (`rutis/*`) set it.
    pub framework: bool,
}

impl Dispatch {
    pub fn new(name: impl Into<String>, mode: Mode, payload: super::Payload) -> Self {
        Self {
            name: name.into(),
            mode,
            payload,
            scope: None,
            filter: None,
            terminal: None,
            origin: None,
            framework: false,
        }
    }
}

/// One call of one entry outside a dispatch: what a node runs when the
/// other node calls an entry it accepted (protocol spec §9 `events.call`).
pub struct EntryCall {
    pub name: String,
    pub mode: Mode,
    pub payload: super::Payload,
    /// `waterfall` / `waterfall-sync`: the rest of the far end's chain.
    pub next: Option<RawTerminal>,
    pub dispatch: u64,
    pub origin: Option<u64>,
}
