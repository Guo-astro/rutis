//! The dispatch algorithm (protocol spec §5.1, §5.4): snapshot, filter,
//! then the mode, the same for every entry wherever it lives.

use std::cell::RefCell;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use super::table::{Body, CallFlight, Entry};
use super::EventBus;
use crate::ctx::Ctx;
use crate::error::{join_panic_error, panic_error, CordisError, NextMisuse};
use crate::events::remote::{SyncNextFn, SyncNextState};
use crate::events::{
    CatchUnwind, FilterAnswer, ListenerInfo, ListenerKind, Mode, Payload, PayloadRef, RawTerminal,
    RemoteCall, RemoteNext, RemoteSyncCall, RemoteSyncNext, Value,
};
use crate::BoxFuture;

/// One call's view of its dispatch.
pub(crate) struct Inv<'a> {
    pub bus: &'a EventBus,
    pub emitter: &'a Ctx,
    pub name: &'a str,
    pub mode: Mode,
    pub event: PayloadRef<'a>,
    pub dispatch: u64,
    pub origin: Option<u64>,
}

/// An [`Inv`] that owns what it refers to, for calls that outlive the
/// emitting call (`emit`, `parallel`).
pub(crate) struct OwnedInv {
    pub bus: EventBus,
    pub emitter: Ctx,
    pub name: Arc<str>,
    pub mode: Mode,
    pub payload: Payload,
    pub dispatch: u64,
    pub origin: Option<u64>,
}

impl OwnedInv {
    pub fn inv(&self) -> Inv<'_> {
        Inv {
            bus: &self.bus,
            emitter: &self.emitter,
            name: &self.name,
            mode: self.mode,
            event: self.payload.as_ref(),
            dispatch: self.dispatch,
            origin: self.origin,
        }
    }
}

// ── filters ───────────────────────────────────────────────────────

pub(crate) enum FilterSpec {
    /// A predicate in this process, called once per non-global candidate.
    Each(Arc<dyn Fn(&ListenerInfo) -> bool + Send + Sync>),
    Batch(crate::events::BatchFilter),
}

pub(crate) fn filter_each(
    entries: Vec<Arc<Entry>>,
    keep: &(dyn Fn(&ListenerInfo) -> bool + Send + Sync),
) -> Result<Vec<Arc<Entry>>, CordisError> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.info.global {
            out.push(entry);
            continue;
        }
        match catch_unwind(AssertUnwindSafe(|| keep(&entry.info))) {
            Ok(true) => out.push(entry),
            Ok(false) => {}
            Err(panic) => return Err(panic_error(panic)),
        }
    }
    Ok(out)
}

fn candidates(entries: &[Arc<Entry>]) -> Vec<ListenerInfo> {
    entries
        .iter()
        .filter(|e| !e.info.global)
        .map(|e| (*e.info).clone())
        .collect()
}

fn apply_answers(
    name: &str,
    entries: Vec<Arc<Entry>>,
    answers: Vec<bool>,
) -> Result<Vec<Arc<Entry>>, CordisError> {
    let asked = entries.iter().filter(|e| !e.info.global).count();
    if answers.len() != asked {
        return Err(CordisError::Validation {
            issues: vec![format!(
                "filter of {name} returned {} answers for {asked} listeners",
                answers.len()
            )],
        });
    }
    let mut answers = answers.into_iter();
    Ok(entries
        .into_iter()
        .filter(|e| e.info.global || answers.next().unwrap_or(false))
        .collect())
}

pub(crate) async fn filter_any(
    name: &str,
    entries: Vec<Arc<Entry>>,
    filter: Option<FilterSpec>,
) -> Result<Vec<Arc<Entry>>, CordisError> {
    match filter {
        None => Ok(entries),
        Some(FilterSpec::Each(keep)) => filter_each(entries, &*keep),
        Some(FilterSpec::Batch(batch)) => {
            let asked = candidates(&entries);
            let answers = match catch_unwind(AssertUnwindSafe(|| batch(asked))) {
                Ok(FilterAnswer::Ready(answers)) => answers?,
                Ok(FilterAnswer::Pending(answers)) => answers.await?,
                Err(panic) => return Err(panic_error(panic)),
            };
            apply_answers(name, entries, answers)
        }
    }
}

pub(crate) fn filter_sync(
    name: &str,
    entries: Vec<Arc<Entry>>,
    filter: Option<FilterSpec>,
) -> Result<Vec<Arc<Entry>>, CordisError> {
    match filter {
        None => Ok(entries),
        Some(FilterSpec::Each(keep)) => filter_each(entries, &*keep),
        Some(FilterSpec::Batch(batch)) => {
            let asked = candidates(&entries);
            let answers = match catch_unwind(AssertUnwindSafe(|| batch(asked))) {
                Ok(FilterAnswer::Ready(answers)) => answers?,
                Ok(FilterAnswer::Pending(_)) => {
                    return Err(CordisError::Validation {
                        issues: vec![format!(
                            "the filter of the synchronous dispatch of {name} must answer synchronously"
                        )],
                    })
                }
                Err(panic) => return Err(panic_error(panic)),
            };
            apply_answers(name, entries, answers)
        }
    }
}

// ── starting a call ──────────────────────────────────────────────

pub(crate) enum Started<F: Future> {
    Done(F::Output),
    Pending(Pin<Box<F>>),
}

/// Poll `fut` once in the caller's task: a call "starts" in this order
/// (§5.5), then the rest of it may run elsewhere.
pub(crate) async fn first_poll<F: Future>(fut: F) -> Started<F> {
    let mut fut = Some(Box::pin(fut));
    std::future::poll_fn(move |cx| {
        let mut f = fut.take().expect("polled once");
        Poll::Ready(match f.as_mut().poll(cx) {
            Poll::Ready(out) => Started::Done(out),
            Poll::Pending => Started::Pending(f),
        })
    })
    .await
}

fn caught<T>(
    out: Result<Result<T, CordisError>, Box<dyn std::any::Any + Send>>,
) -> Result<T, CordisError> {
    out.unwrap_or_else(|panic| Err(panic_error(panic)))
}

// ── async calls ──────────────────────────────────────────────────

pub(crate) trait ErasedTerminal: Sync {
    fn call<'a>(&'a self, inv: &'a Inv<'a>) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
}

pub(crate) struct NoTerminal;

impl ErasedTerminal for NoTerminal {
    fn call<'a>(&'a self, _: &'a Inv<'a>) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}

impl ErasedSyncTerminal for NoTerminal {
    fn call(&self, _: &Inv<'_>) -> Result<Option<Value>, CordisError> {
        Ok(None)
    }
}

/// A raw terminal (or a far end's `next`), called at most once.
pub(crate) struct RawTerm(pub Mutex<Option<RawTerminal>>);

impl ErasedTerminal for RawTerm {
    fn call<'a>(&'a self, inv: &'a Inv<'a>) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        match self.0.lock().unwrap().take() {
            Some(RawTerminal::Async(f)) => {
                let fut = f(inv.event);
                Box::pin(async move { caught(CatchUnwind::new(fut).await) })
            }
            Some(RawTerminal::Sync(f)) => Box::pin(std::future::ready(
                catch_unwind(AssertUnwindSafe(|| f(inv.event)))
                    .unwrap_or_else(|panic| Err(panic_error(panic))),
            )),
            None => Box::pin(std::future::ready(Ok(None))),
        }
    }
}

impl ErasedSyncTerminal for RawTerm {
    fn call(&self, inv: &Inv<'_>) -> Result<Option<Value>, CordisError> {
        match self.0.lock().unwrap().take() {
            Some(RawTerminal::Sync(f)) => catch_unwind(AssertUnwindSafe(|| f(inv.event)))
                .unwrap_or_else(|panic| Err(panic_error(panic))),
            Some(RawTerminal::Async(_)) => Err(CordisError::Validation {
                issues: vec![format!(
                    "the terminal of the synchronous waterfall of {} must be synchronous",
                    inv.name
                )],
            }),
            None => Ok(None),
        }
    }
}

/// The rest of an async waterfall after one entry.
pub(crate) struct AsyncNext<'a> {
    pub inv: &'a Inv<'a>,
    pub chain: &'a [Arc<Entry>],
    pub terminal: &'a (dyn ErasedTerminal + 'a),
}

impl<'a> AsyncNext<'a> {
    pub fn invoke(self) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        Box::pin(async move {
            for (index, entry) in self.chain.iter().enumerate() {
                // Withdrawn since the snapshot: skipped, as if never selected.
                let Some(flight) = entry.start() else {
                    continue;
                };
                let next = AsyncNext {
                    inv: self.inv,
                    chain: &self.chain[index + 1..],
                    terminal: self.terminal,
                };
                let out = call_async(entry, self.inv, Some(next)).await;
                drop(flight);
                return out;
            }
            self.terminal.call(self.inv).await
        })
    }
}

/// Call one entry in an async mode. `next` only in a waterfall.
pub(crate) fn call_async<'a>(
    entry: &'a Entry,
    inv: &'a Inv<'a>,
    next: Option<AsyncNext<'a>>,
) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
    match &entry.body {
        Body::Async(listener) => Box::pin(async move {
            caught(
                CatchUnwind::new(async move { listener.call(&entry.ctx, inv, next).await }).await,
            )
        }),
        Body::Sync(listener) => {
            let arg = if next.is_some() {
                SyncNextArg::InAsyncWaterfall
            } else {
                SyncNextArg::None
            };
            let out = catch_unwind(AssertUnwindSafe(|| listener.call(&entry.ctx, inv, arg)))
                .unwrap_or_else(|panic| Err(panic_error(panic)));
            Box::pin(std::future::ready(out))
        }
        Body::Remote(remote) => match next {
            None => Box::pin(async move {
                caught(
                    CatchUnwind::new(
                        async move { remote.call(RemoteCall { inv, next: None }).await },
                    )
                    .await,
                )
            }),
            Some(next) => remote_waterfall(&**remote, inv, next),
        },
    }
}

type NextReply = tokio::sync::oneshot::Sender<Result<Option<Value>, CordisError>>;

/// A remote entry in an async waterfall: its `next` is a `'static` handle;
/// a request from it runs the rest of the chain here, beside the entry's
/// call. Once the call ends, the handle answers "ended".
fn remote_waterfall<'a>(
    remote: &'a dyn crate::events::RemoteListener,
    inv: &'a Inv<'a>,
    next: AsyncNext<'a>,
) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
    Box::pin(async move {
        let (requests, mut rx) = tokio::sync::mpsc::unbounded_channel::<NextReply>();
        let handle = RemoteNext {
            name: Arc::from(inv.name),
            called: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            requests,
        };
        let mut call = CatchUnwind::new(async move {
            remote
                .call(RemoteCall {
                    inv,
                    next: Some(handle),
                })
                .await
        });
        let mut next = Some(next);
        let mut rest: Option<BoxFuture<'a, Result<Option<Value>, CordisError>>> = None;
        let mut reply: Option<NextReply> = None;
        let out = std::future::poll_fn(|cx| loop {
            if let Poll::Ready(out) = Pin::new(&mut call).poll(cx) {
                return Poll::Ready(out);
            }
            if let Some(running) = rest.as_mut() {
                if let Poll::Ready(value) = running.as_mut().poll(cx) {
                    rest = None;
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(value);
                    }
                    continue;
                }
            }
            if next.is_some() {
                if let Poll::Ready(Some(request)) = rx.poll_recv(cx) {
                    rest = Some(next.take().expect("checked").invoke());
                    reply = Some(request);
                    continue;
                }
            }
            return Poll::Pending;
        })
        .await;
        caught(out)
    })
}

// ── sync calls ───────────────────────────────────────────────────

pub(crate) trait ErasedSyncTerminal {
    fn call(&self, inv: &Inv<'_>) -> Result<Option<Value>, CordisError>;
}

pub(crate) enum SyncNextArg<'a> {
    /// Not a waterfall.
    None,
    /// A synchronous listener in an async waterfall.
    InAsyncWaterfall,
    Next(SyncNext<'a>),
}

pub(crate) struct SyncNext<'a> {
    pub inv: &'a Inv<'a>,
    pub chain: &'a [Arc<Entry>],
    pub terminal: &'a (dyn ErasedSyncTerminal + 'a),
}

impl SyncNext<'_> {
    pub fn invoke(self) -> Result<Option<Value>, CordisError> {
        for (index, entry) in self.chain.iter().enumerate() {
            let Some(flight) = entry.start() else {
                continue;
            };
            let next = SyncNext {
                inv: self.inv,
                chain: &self.chain[index + 1..],
                terminal: self.terminal,
            };
            let out = call_sync(entry, self.inv, Some(next));
            drop(flight);
            return out;
        }
        user_call(self.inv.emitter, || self.terminal.call(self.inv))
    }
}

pub(crate) fn user_call<T>(
    ctx: &Ctx,
    call: impl FnOnce() -> Result<T, CordisError>,
) -> Result<T, CordisError> {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(result) => result,
        Err(panic) => {
            let error = Arc::new(panic_error(panic));
            let _ = catch_unwind(AssertUnwindSafe(|| ctx.error_sink()(error.clone())));
            Err(CordisError::SyncEventPanicked(error))
        }
    }
}

fn async_listener(entry: &Entry, inv: &Inv<'_>) -> CordisError {
    CordisError::AsyncListener {
        name: inv.name.to_owned(),
        listener: entry.info.describe(),
        mode: inv.mode,
    }
}

/// Call one entry in a synchronous mode. `next` only in `waterfall-sync`.
pub(crate) fn call_sync(
    entry: &Entry,
    inv: &Inv<'_>,
    next: Option<SyncNext<'_>>,
) -> Result<Option<Value>, CordisError> {
    match &entry.body {
        Body::Sync(listener) => {
            let arg = next.map_or(SyncNextArg::None, SyncNextArg::Next);
            user_call(&entry.ctx, || listener.call(&entry.ctx, inv, arg))
        }
        Body::Async(_) => Err(async_listener(entry, inv)),
        Body::Remote(remote) => {
            if remote.kind() == ListenerKind::Async {
                return Err(async_listener(entry, inv));
            }
            match next {
                None => user_call(&entry.ctx, || {
                    remote.call_sync(RemoteSyncCall { inv, next: None })
                }),
                Some(next) => remote_sync_waterfall(entry, &**remote, inv, next),
            }
        }
    }
}

struct ClearOnReturn(Arc<Mutex<SyncNextState>>);

impl Drop for ClearOnReturn {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.lock() {
            state.run = None;
        }
    }
}

fn remote_sync_waterfall(
    entry: &Entry,
    remote: &dyn crate::events::RemoteListener,
    inv: &Inv<'_>,
    next: SyncNext<'_>,
) -> Result<Option<Value>, CordisError> {
    let cell = RefCell::new(Some(next));
    let name = inv.name;
    let run = || -> Result<Option<Value>, CordisError> {
        let next = cell.borrow_mut().take();
        match next {
            Some(next) => next.invoke(),
            None => Err(CordisError::WaterfallNext {
                name: name.to_owned(),
                misuse: NextMisuse::AlreadyCalled,
            }),
        }
    };
    let run: &(dyn Fn() -> Result<Option<Value>, CordisError> + '_) = &run;
    // SAFETY: only the lifetime bound changes. `RemoteSyncNext::call`
    // dereferences it only on this thread while `run` is set, and `_clear`
    // unsets it before this frame (which owns the closure) returns.
    let run: *const SyncNextFn = unsafe {
        std::mem::transmute::<
            *const (dyn Fn() -> Result<Option<Value>, CordisError> + '_),
            *const SyncNextFn,
        >(run)
    };
    let state = Arc::new(Mutex::new(SyncNextState {
        run: Some(run),
        thread: std::thread::current().id(),
        called: false,
    }));
    let _clear = ClearOnReturn(state.clone());
    let handle = RemoteSyncNext {
        name: Arc::from(inv.name),
        state,
    };
    user_call(&entry.ctx, || {
        remote.call_sync(RemoteSyncCall {
            inv,
            next: Some(handle),
        })
    })
}

// ── modes ────────────────────────────────────────────────────────

/// Collect `parallel`'s results: errors in registration order.
pub(crate) async fn run_parallel(
    owned: Arc<OwnedInv>,
    entries: Vec<Arc<Entry>>,
    handle: &tokio::runtime::Handle,
) -> Result<(), CordisError> {
    let mut results: Vec<(usize, Result<Option<Value>, CordisError>)> = Vec::new();
    let mut set = tokio::task::JoinSet::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let Some(flight) = entry.start() else {
            continue;
        };
        let owned = owned.clone();
        let call = async move {
            let _flight: CallFlight = flight;
            let inv = owned.inv();
            (index, call_async(&entry, &inv, None).await)
        };
        match first_poll(call).await {
            Started::Done(result) => results.push(result),
            Started::Pending(rest) => {
                set.spawn_on(rest, handle);
            }
        }
    }
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(result) => results.push(result),
            Err(error) => results.push((usize::MAX, Err(join_panic_error(error)))),
        }
    }
    results.sort_by_key(|(index, _)| *index);
    let errors: Vec<Arc<CordisError>> = results
        .into_iter()
        .filter_map(|(_, result)| result.err().map(Arc::new))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CordisError::ListenersFailed {
            name: owned.name.to_string(),
            errors,
        })
    }
}

pub(crate) async fn run_serial(
    inv: &Inv<'_>,
    entries: &[Arc<Entry>],
) -> Result<Option<Value>, CordisError> {
    for entry in entries {
        let Some(flight) = entry.start() else {
            continue;
        };
        let out = call_async(entry, inv, None).await;
        drop(flight);
        match out? {
            Some(value) if !value.is_no_result() => return Ok(Some(value)),
            _ => {}
        }
    }
    Ok(None)
}

pub(crate) fn run_bail(
    inv: &Inv<'_>,
    entries: &[Arc<Entry>],
) -> Result<Option<Value>, CordisError> {
    for entry in entries {
        let Some(flight) = entry.start() else {
            continue;
        };
        let out = call_sync(entry, inv, None);
        drop(flight);
        match out? {
            Some(value) if !value.is_no_result() => return Ok(Some(value)),
            _ => {}
        }
    }
    Ok(None)
}

/// One `emit` call, in its own task when it does not finish at once; its
/// failure is reported, never returned (§5.6).
pub(crate) async fn emit_call(
    owned: Arc<OwnedInv>,
    entry: Arc<Entry>,
    flight: CallFlight,
    ticket: Option<Arc<super::EmitTicket>>,
) {
    let _flight = flight;
    let _ticket = ticket;
    let inv = owned.inv();
    if let Err(error) = call_async(&entry, &inv, None).await {
        report_listener(&owned.emitter, &owned.name, &entry, error);
    }
}

pub(crate) fn report_listener(ctx: &Ctx, name: &str, entry: &Entry, error: CordisError) {
    let report = CordisError::ListenerFailed {
        name: name.to_owned(),
        listener: entry.info.describe(),
        error: Arc::new(error),
    };
    let sink = ctx.error_sink();
    let _ = catch_unwind(AssertUnwindSafe(|| sink(Arc::new(report))));
}
