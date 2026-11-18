//! The host as the hub of the logical bus (events protocol §4, §5, §7):
//! each runtime session's listeners are remote entries of the kernel's one
//! table, and its dispatches run over that table.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The locks the shuttle models interleave (#628): a session's rows, its
/// entries and its emit queue. A plain build is std's.
#[cfg(rutis_shuttle)]
use crate::sync::Mutex as SeamMutex;
#[cfg(not(rutis_shuttle))]
use std::sync::Mutex as SeamMutex;
use std::task::{Context, Poll};

use rutis::events::{ListenerInfo, Mode, Scope};
use rutis::host::BoxFuture;
use rutis::host::{CtxHost, ErrorKind, ListenerKind};
use rutis::internal::events::{
    BatchFilter, Candidate, Dispatch, FilterAnswer, PayloadRef, RawTerminal, RemoteCall,
    RemoteEntry, RemoteListener, RemoteNext, RemoteSyncCall,
};
use rutis::internal::{CtxInternal, EventOptions};
use rutis::{Ctx, Disposer, Error as RutisError};
use serde_json::{json, Value as Json};

use super::args::{from_wire, result_from_wire, result_to_wire, to_wire, SessionTranscoder};
use super::errors::{binding, promise_of_waiting, to_cordis, to_session};
use crate::session::rpc::with_caller;
use crate::session::{settle, Connection, Error, Reply, Value, WeakConnection};

/// The logical bus of one kernel bus, as the sessions on it see it.
pub(crate) struct Hub {
    /// The bus's root: holds the bus (and its address) while the hub lives.
    #[allow(dead_code)]
    ctx: Ctx,
    /// The listener turning `ServiceChanged` into `rutis/service`.
    services: Mutex<Option<Disposer>>,
    /// The test hooks of the bus, when tapped.
    #[cfg(feature = "testing")]
    tap: Mutex<Option<Arc<super::testing::TapState>>>,
}

/// The hubs, by the address of their kernel bus. A hub lives while a
/// session uses it, and keeps its bus (its context) alive meanwhile, so an
/// address is never another bus's while its hub lives.
static HUBS: Mutex<Vec<(usize, Weak<Hub>)>> = Mutex::new(Vec::new());

/// Origins of entries, unique in this process (sessions and links of every
/// bus): `0` is never one.
static ORIGINS: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_origin() -> u64 {
    ORIGINS.fetch_add(1, Ordering::Relaxed)
}

impl Hub {
    /// The hub of `ctx`'s bus, made when none lives.
    pub(crate) fn of(ctx: &Ctx) -> Result<Arc<Hub>, Error> {
        // The bus's root: the `rutis/service` listener lasts as long as the
        // bus.
        let ctx = &ctx.root_ctx();
        let address = ctx.events() as *const _ as usize;
        let mut hubs = HUBS.lock().unwrap();
        hubs.retain(|(_, hub)| hub.strong_count() > 0);
        if let Some(hub) = hubs
            .iter()
            .find(|(at, _)| *at == address)
            .and_then(|(_, hub)| hub.upgrade())
        {
            return Ok(hub);
        }
        let hub = Arc::new(Hub {
            ctx: ctx.clone(),
            services: Mutex::new(None),
            #[cfg(feature = "testing")]
            tap: Mutex::new(None),
        });
        ctx.events().set_transcoder(Arc::new(SessionTranscoder));
        let services = ctx
            .on::<rutis::host::diagnostics::ServiceChanged>(super::framework::service_events)
            .map_err(|error| to_session(&error))?;
        *hub.services.lock().unwrap() = Some(services);
        hubs.push((address, Arc::downgrade(&hub)));
        Ok(hub)
    }

    #[cfg(feature = "testing")]
    pub(crate) fn tap(&self, tap: Arc<super::testing::TapState>) {
        *self.tap.lock().unwrap() = Some(tap);
    }
}

/// The kernel's process-local diagnostic events (`rutis::ServiceChanged`,
/// `rutis::FiberStatusChanged`): they name Rust types and cannot be decoded
/// elsewhere, so their listeners are never announced to nodes.
pub(crate) fn process_local(name: &str) -> bool {
    name.starts_with("rutis::")
}

/// Withdraw an entry now: the kernel takes it out of the table when its
/// disposer runs; what is left of the disposal finishes on its own. Hands
/// back the end of the withdrawal, or `None` when the entry is out already.
fn withdrawing(disposer: Disposer) -> Option<BoxFuture<'static, Result<(), Arc<ErrorKind>>>> {
    let mut disposal = disposer.dispose();
    let mut context = Context::from_waker(std::task::Waker::noop());
    disposal
        .as_mut()
        .poll(&mut context)
        .is_pending()
        .then_some(disposal)
}

/// Withdraw an entry now, and let the disposal finish on its own.
pub(crate) fn dispose_now(disposer: Disposer) {
    let Some(disposal) = withdrawing(disposer) else {
        return;
    };
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            let _ = disposal.await;
        });
    }
}

/// One runtime session's side of the logical bus: its entries, its rows'
/// scopes, its dispatches (events protocol §3–§7).
pub(crate) struct SessionEvents {
    /// The hub, held so it lives while the session does (read by the test
    /// hooks only); `None` when the runtime has no bus to join (a process
    /// mounted without a context).
    #[cfg_attr(not(feature = "testing"), allow(dead_code))]
    hub: Option<Arc<Hub>>,
    /// The context its entries are registered and its dispatches emitted
    /// through (the runtime plugin's): they go with it.
    ctx: Option<Ctx>,
    origin: u64,
    connection: WeakConnection,
    /// The session's runtime, which runs the emits waiting their turn and
    /// the answers awaited: whatever thread a dispatch arrives on.
    executor: tokio::runtime::Handle,
    runtime: String,
    /// The scope of the mount's own plugins (`row: null`).
    mount_scope: Vec<Scope>,
    /// Where each row loaded on the session is, by key (see [`RowPlace`]).
    rows: SeamMutex<HashMap<String, RowPlace>>,
    /// The session's entries, by the runtime's id.
    entries: SeamMutex<HashMap<String, Registered>>,
    /// `emit` dispatches waiting for those before them to start (§5.5).
    emits: SeamMutex<Emits>,
    /// The synchronous waits of the runtime in its `waterfall` dispatches
    /// (`sync: true`, §5.4); its entries read it.
    waits: Waits,
}

struct Registered {
    row: Option<String>,
    disposer: Disposer,
}

/// Where a row loaded on the session is: the scope chain its entries were
/// registered in, the id its configuration gives it, and whether it is
/// unloading.
///
/// An unloading row keeps this place, so its own cleanups still emit (§7.1
/// step 5: the other rows hear them), but it keeps no listeners: the ones
/// it had left the table when the unload started, and the ones it registers
/// meanwhile are not kept. Being unloading is also what tells its cleanups'
/// emissions apart from the emits it queued before them: those are delivered
/// even after the row ends, these are let go with it (#1327).
struct RowPlace {
    scope: Vec<Scope>,
    id: String,
    unloading: bool,
}

#[derive(Default)]
struct Emits {
    busy: bool,
    /// The dispatch running, and its cancel, while one runs.
    running: Option<(Emitted, Arc<Cancel>)>,
    /// The dispatches queued behind it, in arrival order, and the run.
    queue: VecDeque<(Emitted, EmitRun)>,
    /// The rows whose entries are leaving the table: nothing queued starts
    /// until none is (§7.1 step 1).
    leaving: Arc<Leaving>,
}

/// A dispatch waiting its turn in a session's emit queue, or running: who
/// emitted it, and whether that row was unloading when the session took it.
#[derive(Clone)]
struct Emitted {
    /// The row that emitted it: its key, or `None` for the mount's own.
    row: Option<String>,
    /// Handed over while its row was unloading: an emission of that row's
    /// unload stack (§7.1 step 5), which the session delivers even after the
    /// row ends, unlike the emits it queued before it started unloading
    /// (#1327).
    cleanup: bool,
}

impl Emitted {
    /// Whether this is an ordinary emit of row `key`: one queued before the
    /// row started unloading, which the row's unload and its end let go
    /// (§5.5).
    fn ordinary_of(&self, key: &str) -> bool {
        !self.cleanup && self.row.as_deref() == Some(key)
    }
}

/// The rows whose entries are leaving the table (§7.1 step 1). The kernel
/// withdraws an entry on a task of its own, so starting a row's disposers in
/// `row_ended` is not having the entries out: an `emit` that started
/// meanwhile would still select them (§5.1).
#[derive(Default)]
struct Leaving {
    /// How many rows are being withdrawn.
    count: AtomicUsize,
    /// Wakes the session's task when one of them is through.
    through: tokio::sync::Notify,
}

/// What the session's emit chain does after the run it drove ended (§5.5).
enum Next {
    /// The queued emit whose turn came, now the running one.
    Run(EmitRun),
    /// Rows' entries are leaving the table: wait for them, then look again
    /// (§7.1 step 1).
    Leaving,
    /// Nothing waits: the session's emits are idle.
    Idle,
}

/// Lets an emit go: its run and its filter (on the bus's task for the
/// emit) end at their next poll.
#[derive(Default)]
struct Cancel {
    cancelled: std::sync::atomic::AtomicBool,
    /// The tasks polling the run and the filter.
    wakers: Mutex<Vec<std::task::Waker>>,
}

impl Cancel {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let wakers = std::mem::take(&mut *self.wakers.lock().unwrap());
        for waker in wakers {
            waker.wake();
        }
    }

    /// Whether it was cancelled; if not, `cx` is woken when it is.
    fn cancelled(&self, cx: &Context<'_>) -> bool {
        {
            let mut wakers = self.wakers.lock().unwrap();
            if !wakers.iter().any(|waker| waker.will_wake(cx.waker())) {
                wakers.push(cx.waker().clone());
            }
        }
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// `filter`, ending when `cancel` is: then it keeps no listener and
/// reports nothing, and its call is dropped (the emit's later emits of the
/// name, which the bus starts after it, go on).
fn cancelling(filter: BatchFilter, cancel: Arc<Cancel>) -> BatchFilter {
    Box::new(move |candidates: Vec<Candidate>| {
        let wanted = candidates.len();
        match filter(candidates) {
            FilterAnswer::Pending(mut answer) => {
                FilterAnswer::Pending(Box::pin(std::future::poll_fn(move |cx| {
                    if cancel.cancelled(cx) {
                        return Poll::Ready(Ok(vec![false; wanted]));
                    }
                    answer.as_mut().poll(cx)
                })))
            }
            ready => ready,
        }
    })
}

/// An emit's run: it ends when the run does, when it is cancelled (the run
/// is dropped, and its filter keeps no listener: nothing starts or is
/// reported after) or when the run panics, so the session's queue moves on
/// in every case. Its dispatch's reply completes when the run is dropped.
struct EmitRun {
    run: Option<BoxFuture<'static, ()>>,
    cancel: Arc<Cancel>,
}

impl EmitRun {
    fn new(run: BoxFuture<'static, ()>, cancel: Arc<Cancel>) -> Self {
        Self {
            run: Some(run),
            cancel,
        }
    }

    /// Drop the run; a panic in its drop is caught too.
    fn finish(&mut self) {
        let run = self.run.take();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(run)));
    }
}

impl Future for EmitRun {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        if this.run.is_none() {
            return Poll::Ready(());
        }
        if this.cancel.cancelled(cx) {
            this.finish();
            return Poll::Ready(());
        }
        let run = this.run.as_mut().expect("a run");
        let polled =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.as_mut().poll(cx)));
        match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(())) | Err(_) => {
                this.finish();
                Poll::Ready(())
            }
        }
    }
}

/// `events.on`'s argument (events protocol §4.2).
#[derive(serde::Deserialize)]
struct Registration {
    id: String,
    name: String,
    row: Option<String>,
    plugin: String,
    kind: ListenerKind,
    #[serde(default)]
    prepend: bool,
    #[serde(default)]
    global: bool,
}

impl SessionEvents {
    /// The runtime on `connection`, named `runtime`, whose mount's plugins
    /// are at `scope`, joins `ctx`'s bus. It leaves the bus when the
    /// session ends or this is dropped.
    pub(crate) fn join(
        ctx: Option<&Ctx>,
        connection: Connection,
        runtime: String,
        scope: Vec<Scope>,
    ) -> Result<Arc<Self>, Error> {
        let hub = ctx.map(Hub::of).transpose()?;
        let session = Arc::new(Self {
            hub,
            ctx: ctx.cloned(),
            origin: next_origin(),
            connection: connection.downgrade(),
            executor: connection.executor(),
            runtime,
            mount_scope: scope,
            rows: SeamMutex::default(),
            entries: SeamMutex::default(),
            emits: SeamMutex::default(),
            waits: Waits::default(),
        });
        let weak = Arc::downgrade(&session);
        session.executor.spawn(async move {
            connection.closed().await;
            if let Some(session) = weak.upgrade() {
                session.leave();
            }
        });
        Ok(session)
    }

    /// Withdraw everything of the session (its end, events protocol §7.2).
    fn leave(&self) {
        self.rows.lock().unwrap().clear();
        // The running emit is let go too. Dropped after the lock: a future
        // may write a frame on drop.
        let (queued, running) = {
            let mut emits = self.emits.lock().unwrap();
            (std::mem::take(&mut emits.queue), emits.running.take())
        };
        drop(queued);
        if let Some((_, cancel)) = running {
            cancel.cancel();
        }
        let entries: Vec<Registered> = self
            .entries
            .lock()
            .unwrap()
            .drain()
            .map(|(_, e)| e)
            .collect();
        for entry in entries {
            dispose_now(entry.disposer);
        }
    }

    /// Row `key` is being loaded in `scope`.
    pub(crate) fn row_loading(&self, key: &str, scope: Vec<Scope>, id: String) {
        self.rows.lock().unwrap().insert(
            key.to_owned(),
            RowPlace {
                scope,
                id,
                unloading: false,
            },
        );
    }

    /// Row `key` is gone: it left the session, so it emits nothing more and
    /// its entries leave the table (events protocol §7.1). What it handed
    /// over while it was unloading is the session's to deliver, even now
    /// (§7.1 step 5, #1327).
    pub(crate) fn row_ended(&self, key: &str) {
        self.rows.lock().unwrap().remove(key);
        // The entries leave the table before the emits are let go: letting
        // go is what starts the session's next emit, on its own task, and
        // that emit's snapshot (§5.1) must not find a row that has ended.
        // The kernel withdraws an entry on a task of its own, so the order
        // alone is not enough: `withdraw_entries` holds the session's emits
        // until they are out (EVP-7.1-1).
        self.withdraw_entries(key);
        #[cfg(test)]
        tests::between_the_steps();
        self.let_go_of_its_emits(key);
    }

    /// Row `key` is unloading (`rows.unload` was sent): its entries leave
    /// the table now, and the emits it queued before are dropped (§7.1 step 1,
    /// §5.5). The row itself stays loaded, so its unload stack's cleanups
    /// still emit, and the other rows hear them until the runtime answers
    /// `rows.unload` (§7.1 step 5); then the host calls
    /// [`SessionEvents::row_ended`], which keeps those emissions: they are
    /// delivered in their turn even after the row is off the session
    /// (#1327).
    pub(crate) fn row_unloading(&self, key: &str) {
        match self.rows.lock().unwrap().get_mut(key) {
            Some(place) => place.unloading = true,
            // Not loaded on this session (an unload of a row that never
            // loaded, or one already ended): nothing to withdraw.
            None => return,
        }
        // As in [`SessionEvents::row_ended`]: the entries first.
        self.withdraw_entries(key);
        #[cfg(test)]
        tests::between_the_steps();
        self.let_go_of_its_emits(key);
    }

    /// The ordinary emits row `key` left queued are dropped, and the one
    /// running (waiting for its filter) is let go (§5.5): the session's later
    /// emits go on. What the row handed over while it was unloading stays:
    /// those are its unload stack's emissions, and the session delivers them
    /// even after the row ends (§7.1 step 5, #1327). Dropped after the lock:
    /// a future may write a frame on drop.
    fn let_go_of_its_emits(&self, key: &str) {
        let (queued, running) = {
            let mut emits = self.emits.lock().unwrap();
            let (queued, kept): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut emits.queue)
                .into_iter()
                .partition(|(emitted, _)| emitted.ordinary_of(key));
            emits.queue = kept;
            let running = match &emits.running {
                Some((emitted, _)) if emitted.ordinary_of(key) => emits.running.take(),
                _ => None,
            };
            (queued, running)
        };
        drop(queued);
        if let Some((_, cancel)) = running {
            cancel.cancel();
        }
    }

    /// Row `key`'s entries leave the table. The kernel takes each out when
    /// its disposer finishes, on a task of the session's executor: being
    /// started is not being gone, so the session's emits are held until they
    /// are out (§7.1 step 1) — an emit that started meanwhile would still
    /// select them (§5.1), while the row's own emits are already let go
    /// (§5.5).
    fn withdraw_entries(&self, key: &str) {
        let gone: Vec<Registered> = {
            let mut entries = self.entries.lock().unwrap();
            let ids: Vec<String> = entries
                .iter()
                .filter(|(_, entry)| entry.row.as_deref() == Some(key))
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| entries.remove(&id))
                .collect()
        };
        if gone.is_empty() {
            return;
        }
        let leaving = {
            let emits = self.emits.lock().unwrap();
            // Counted before the disposers are started: one may be out
            // before the next is started, and an emit must not start in
            // between.
            emits.leaving.count.fetch_add(1, Ordering::SeqCst);
            emits.leaving.clone()
        };
        let disposals: Vec<_> = gone
            .into_iter()
            .filter_map(|entry| withdrawing(entry.disposer))
            .collect();
        self.executor.spawn(async move {
            for disposal in disposals {
                let _ = disposal.await;
            }
            // Every entry is out of the table: the session's emits go on.
            leaving.count.fetch_sub(1, Ordering::SeqCst);
            leaving.through.notify_one();
        });
    }

    /// The `events.*` operations a runtime sends.
    pub(crate) fn invoke(
        self: &Arc<Self>,
        connection: &Connection,
        method: &str,
        args: Value,
    ) -> Reply {
        let first = args.list()?.into_iter().next().unwrap_or(Value::Undefined);
        match method {
            "events.on" => self.on(first),
            "events.off" => self.off(first),
            "events.dispatch" => {
                #[cfg(feature = "testing")]
                if let Some(tap) = self
                    .hub
                    .as_ref()
                    .and_then(|hub| hub.tap.lock().unwrap().clone())
                {
                    tap.starting();
                }
                self.dispatch(connection, first)
            }
            _ => Err(binding(format!("{method} is not offered here"))),
        }
    }

    /// The id of row `key` on this session; `None` for the mount. An
    /// unloading row still has one: its cleanups' emissions are dispatched
    /// while it unloads (§7.1 step 5).
    fn row_id(&self, key: Option<&str>) -> Result<Option<String>, Error> {
        match key {
            None => Ok(None),
            Some(key) => self
                .rows
                .lock()
                .unwrap()
                .get(key)
                .map(|place| Some(place.id.clone()))
                .ok_or_else(|| binding("the calling row is not loaded in this runtime".to_owned())),
        }
    }

    fn on(self: &Arc<Self>, registration: Value) -> Reply {
        let registration: Registration = crate::session::decode(registration.json()?)?;
        if registration.name.starts_with("internal/") {
            return Err(binding(format!(
                "event {} is internal to its runtime",
                registration.name
            )));
        }
        // Held until the entry is recorded: a row unloading meanwhile
        // (`row_unloading` or `row_ended`, on another thread) waits, then
        // withdraws it too, instead of missing an entry registered after
        // its look.
        let rows = self.rows.lock().unwrap();
        let (scope, id) = match registration.row.as_deref() {
            None => (self.mount_scope.clone(), None),
            Some(key) => match rows.get(key) {
                Some(place) if !place.unloading => (place.scope.clone(), Some(place.id.clone())),
                // An unloading row registers no listener (§7.1): one its
                // cleanup registers is not kept and hears nothing. That is
                // not a failure of the unload, so it is answered as usual;
                // the entry the runtime holds is withdrawn with the row.
                Some(_) => return Ok(Value::Undefined),
                None => {
                    return Err(binding(
                        "the calling row is not loaded in this runtime".to_owned(),
                    ))
                }
            },
        };
        #[cfg(test)]
        tests::REGISTERING.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        let Some(ctx) = &self.ctx else {
            return Ok(Value::Undefined);
        };
        if self.entries.lock().unwrap().contains_key(&registration.id) {
            return Err(binding(format!(
                "listener {} is already registered",
                registration.id
            )));
        }
        let info = ListenerInfo {
            name: registration.name.clone(),
            prefix: false,
            plugin: registration.plugin,
            row: id,
            runtime: Some(self.runtime.clone()),
            node: None,
            scope,
            global: registration.global,
        };
        let entry = RemoteEntry {
            name: registration.name.clone(),
            prefix: false,
            info,
            options: EventOptions::default()
                .prepend(registration.prepend)
                .global(registration.global),
            origin: self.origin,
        };
        let listener = Arc::new(WireEntry {
            connection: self.connection.clone(),
            id: registration.id.clone(),
            kind: registration.kind,
            origin: self.origin,
            owner: ctx.clone(),
            waits: self.waits.clone(),
        });
        let disposer = ctx
            .events()
            .on_remote(ctx, entry, listener)
            .map_err(|error| to_session(&error))?;
        self.entries.lock().unwrap().insert(
            registration.id,
            Registered {
                row: registration.row,
                disposer,
            },
        );
        drop(rows);
        Ok(Value::Undefined)
    }

    fn off(&self, ids: Value) -> Reply {
        let ids: Vec<String> = crate::session::decode(ids.json()?)?;
        let gone: Vec<Registered> = {
            let mut entries = self.entries.lock().unwrap();
            ids.iter().filter_map(|id| entries.remove(id)).collect()
        };
        for entry in gone {
            dispose_now(entry.disposer);
        }
        Ok(Value::Undefined)
    }

    /// A dispatch from the runtime on `source`: what it calls in other
    /// sessions is forwarded from `source` (§5.3), so their call chains
    /// stay apart.
    fn dispatch(self: &Arc<Self>, source: &Connection, request: Value) -> Reply {
        let mut fields = record(request)?;
        let name = text(&mut fields, "name")?;
        let mode = match text(&mut fields, "mode")?.as_str() {
            "emit" => Mode::Emit,
            "parallel" => Mode::Parallel,
            "serial" => Mode::Serial,
            "waterfall" => Mode::Waterfall,
            "bail" => Mode::Bail,
            other => return Err(binding(format!("events.dispatch: unknown mode {other}"))),
        };
        let row = match fields.remove("row") {
            None | Some(Value::Undefined) | Some(Value::Data(Json::Null)) => None,
            Some(row) => Some(crate::session::decode::<String>(row.json()?)?),
        };
        let row_key = row.clone();
        if name.starts_with(rutis::internal::RESERVED_PREFIX) {
            return Err(Error::Remote {
                name: "ReservedEvent".into(),
                message: format!("rutis/* events are emitted by rutis only: {name}"),
                graph: None,
            });
        }
        if name.starts_with("internal/") {
            return Err(binding(format!("event {name} is internal to its runtime")));
        }
        let row = self.row_id(row.as_deref())?;
        let args = match fields.remove("args") {
            None | Some(Value::Undefined) => Vec::new(),
            Some(args) => args.list()?,
        };
        let filter = reference(&mut fields, "filter")?;
        let terminal = reference(&mut fields, "terminal")?;
        // A Cordis plugin's `emit` or `waterfall` (§5.4): the runtime waits
        // for the reply synchronously, so whatever this dispatch calls in
        // its process (the filter, the terminal, its own listeners) is
        // called synchronously; a `waterfall`'s reply is its value.
        let sync = match fields.remove("sync") {
            None | Some(Value::Undefined) => false,
            Some(sync) => crate::session::decode::<bool>(sync.json()?)?,
        };
        let Some(ctx) = self.ctx.clone() else {
            // No bus: `events.on` registered nothing, so no listener hears it.
            return Ok(Value::Undefined);
        };
        let mut dispatch = Dispatch::new(name.clone(), mode, from_wire(args));
        dispatch.origin = Some(self.origin);
        dispatch.filter = filter
            .map(|filter| selector(filter, name.clone(), mode.is_sync() || sync, self.origin));
        dispatch.terminal = match sync {
            true => terminal.map(|terminal| sync_terminal_of(terminal, self.waits.clone())),
            false => terminal.map(terminal_of),
        };

        match mode {
            Mode::Waterfall if sync => {
                // The whole chain, waited for here: listeners elsewhere run
                // as they always do (an asynchronous one is awaited where it
                // runs), and the runtime handles what reaches it meanwhile in
                // its synchronous wait.
                let _waiting = Waiting::new(&self.waits);
                let result = wait_here(with_caller(
                    source.clone(),
                    ctx.events().dispatch(&ctx, dispatch),
                ))
                .map_err(|error| to_session(&error))?;
                result_to_wire(result)
            }
            Mode::Bail => {
                let result = ctx
                    .events()
                    .dispatch_sync(&ctx, dispatch)
                    .map_err(|error| to_session(&error))?;
                result_to_wire(result)
            }
            Mode::Emit => {
                // The filter's failure is reported here, naming the
                // emitter's row (§5.6): the bus would name the runtime
                // plugin, whose context this dispatch runs in. Keeping no
                // listener is what a failed filter does. One its row's end
                // lets go keeps none and is not reported (§5.5).
                let cancel = Arc::new(Cancel::default());
                dispatch.filter = dispatch.filter.take().map(|filter| {
                    cancelling(
                        reporting(filter, ctx.clone(), name.clone(), row.clone()),
                        cancel.clone(),
                    )
                });
                let reporter = ctx.clone();
                let (started, answered) = tokio::sync::oneshot::channel();
                let row = row.clone();
                let run: BoxFuture<'static, ()> =
                    Box::pin(with_caller(source.clone(), async move {
                        let dispatched = ctx.events().dispatch(&ctx, dispatch).await;
                        if let Err(error) = dispatched {
                            report_filter(&reporter, &name, row.as_deref(), error);
                        }
                        let _ = started.send(());
                    }));
                match self.run_emit(row_key, EmitRun::new(run, cancel)) {
                    true => Ok(Value::Undefined),
                    false => Ok(Value::future(async move {
                        let _ = answered.await;
                        Ok(Value::Undefined)
                    })),
                }
            }
            // Cancelled when the emitter gives up (§5.7): the dispatch stops
            // and cancels its calls under way.
            _ => Ok(Value::cancellable_future(with_caller(
                source.clone(),
                async move {
                    let result = ctx
                        .events()
                        .dispatch(&ctx, dispatch)
                        .await
                        .map_err(|error| to_session(&error))?;
                    result_to_wire(result)
                },
            ))),
        }
    }

    /// Start an `emit` dispatch after those of the session before it
    /// started (events protocol §5.5): here, when none waits and it starts
    /// at once (`true`); otherwise in turn, on a task.
    ///
    /// What a row hands over while it is unloading is marked then, while the
    /// row's place is held: such an emit is a cleanup's emission, which the
    /// session delivers even after the row ends (§7.1 step 5, #1327).
    ///
    /// A run that is let go (its row ended, or the session) or panics ends
    /// like one that finished: the next one starts. A queued emit waits for
    /// the rows whose entries are leaving the table too (§7.1 step 1).
    fn run_emit(self: &Arc<Self>, row: Option<String>, mut run: EmitRun) -> bool {
        // The row's place is held while the emit takes its place in the
        // queue: the row unloading meanwhile (on another thread) is what
        // makes this one of its cleanups'.
        let rows = self.rows.lock().unwrap();
        let emitted = Emitted {
            cleanup: row
                .as_deref()
                .is_some_and(|key| rows.get(key).is_some_and(|place| place.unloading)),
            row,
        };
        let mut emits = self.emits.lock().unwrap();
        if emits.busy {
            emits.queue.push_back((emitted, run));
            drop(emits);
            drop(rows);
            return false;
        }
        if emits.leaving.count.load(Ordering::SeqCst) != 0 {
            // A row's entries are leaving the table: this emit starts once
            // they are out (§7.1 step 1), so the session's task holds the
            // queue until then.
            emits.busy = true;
            emits.queue.push_back((emitted, run));
            drop(emits);
            drop(rows);
            let session = self.clone();
            self.executor
                .spawn(async move { session.drive(None).await });
            return false;
        }
        emits.busy = true;
        emits.running = Some((emitted, run.cancel.clone()));
        drop(emits);
        drop(rows);
        let mut context = Context::from_waker(std::task::Waker::noop());
        let first = match Pin::new(&mut run).poll(&mut context) {
            Poll::Pending => Some(run),
            // It started and finished at once: the next one waits its turn.
            Poll::Ready(()) => match self.next_emit() {
                Next::Idle => return true,
                Next::Run(next) => Some(next),
                // The queue is held for a row's entries leaving the table.
                Next::Leaving => None,
            },
        };
        let session = self.clone();
        self.executor
            .spawn(async move { session.drive(first).await });
        false
    }

    /// Drives the session's emits from now on: `first`, once it starts, then
    /// the queue in turn (§5.5).
    async fn drive(self: Arc<Self>, first: Option<EmitRun>) {
        let mut run = first;
        loop {
            if let Some(mut current) = run.take() {
                (&mut current).await;
            }
            run = loop {
                match self.next_emit() {
                    Next::Run(next) => break Some(next),
                    Next::Idle => return,
                    Next::Leaving => self.wait_through().await,
                }
            };
        }
    }

    /// Waits until no row's entries are leaving the table (§7.1 step 1).
    async fn wait_through(&self) {
        loop {
            let through = self.emits.lock().unwrap().leaving.clone();
            // Registered before the count is read: a withdrawal that is
            // through in between leaves a wakeup, not a wait missed.
            let notified = through.through.notified();
            if through.count.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// The emit running ended: the next queued one, now running; or, while
    /// rows' entries are leaving the table, them (§7.1 step 1); or none (the
    /// session's emits are idle).
    fn next_emit(&self) -> Next {
        let mut emits = self.emits.lock().unwrap();
        if emits.leaving.count.load(Ordering::SeqCst) != 0 {
            return Next::Leaving;
        }
        match emits.queue.pop_front() {
            Some((emitted, next)) => {
                emits.running = Some((emitted, next.cancel.clone()));
                Next::Run(next)
            }
            None => {
                emits.busy = false;
                emits.running = None;
                Next::Idle
            }
        }
    }
}

/// The logical bus of a context's tree, held while this lives
/// ([`join_bus`]).
pub struct BusHandle(#[allow(dead_code)] Arc<Hub>);

/// Keep the logical bus of `ctx`'s tree (its root's) for as long as the
/// handle lives: the framework events rutis-bridge emits (`rutis/service`
/// for every host service, Rust or not) follow its table from now on. A
/// runtime session or node link holds it too; a host whose services are
/// all Rust holds it with this.
pub fn join_bus(ctx: &Ctx) -> Result<BusHandle, Error> {
    Hub::of(ctx).map(BusHandle)
}

impl Drop for Hub {
    /// Its listener leaves the bus with it: a later hub of the same bus
    /// registers its own.
    fn drop(&mut self) {
        if let Some(disposer) = self.services.get_mut().unwrap().take() {
            dispose_now(disposer);
        }
    }
}

impl Drop for SessionEvents {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Report a failed `emit` dispatch (its filter, events protocol §5.6) to
/// the host's error sink.
fn report_filter(ctx: &Ctx, name: &str, row: Option<&str>, error: RutisError) {
    let emitter = match row {
        Some(row) => format!("row {row}"),
        None => "the mount".to_owned(),
    };
    let report = ErrorKind::FilterFailed {
        name: name.to_owned(),
        emitter,
        error: Arc::new(error.into_kind()),
    };
    (ctx.error_sink())(Arc::new(report));
}

/// A request's fields.
fn record(value: Value) -> Result<BTreeMap<String, Value>, Error> {
    match value {
        Value::Record(fields) => Ok(fields),
        Value::Data(Json::Object(fields)) => Ok(fields
            .into_iter()
            .map(|(key, value)| (key, Value::Data(value)))
            .collect()),
        _ => Err(binding("events.dispatch needs its dispatch")),
    }
}

fn text(fields: &mut BTreeMap<String, Value>, field: &str) -> Result<String, Error> {
    match fields.remove(field) {
        Some(Value::Data(Json::String(text))) => Ok(text),
        _ => Err(binding(format!("events.dispatch needs a {field}"))),
    }
}

fn reference(
    fields: &mut BTreeMap<String, Value>,
    field: &str,
) -> Result<Option<crate::session::Reference>, Error> {
    match fields.remove(field) {
        None | Some(Value::Undefined) | Some(Value::Data(Json::Null)) => Ok(None),
        Some(Value::Reference(reference)) if reference.is_function() => Ok(Some(reference)),
        Some(_) => Err(binding(format!(
            "events.dispatch: {field} is not a function"
        ))),
    }
}

/// The batch filter that calls the emitter's selector (events protocol §6):
/// once, with every candidate's descriptor; synchronously in `bail`. The
/// descriptor of an entry of the emitter's own session (`origin`) also
/// carries `listener`, its `events.on` id: the runtime finds its own
/// listener by it (two copies of a row in two instances, or two Cordis
/// contexts of one plugin, have equal descriptors) and does not show it to
/// the plugin's filter.
fn selector(
    selector: crate::session::Reference,
    name: String,
    sync: bool,
    origin: u64,
) -> BatchFilter {
    // A synchronous filter is called along the chain of the dispatch that
    // made it, whatever thread the bus calls it on: the emitter takes it as
    // part of its synchronous wait.
    let chain = crate::session::rpc::chain();
    Box::new(move |candidates: Vec<Candidate>| {
        let wanted = candidates.len();
        let described = candidates
            .iter()
            .map(|candidate| {
                let mut described = serde_json::to_value(&candidate.info).unwrap_or(Json::Null);
                if let (Some(id), Some(true), Json::Object(fields)) = (
                    &candidate.id,
                    candidate.origin.map(|from| from == origin),
                    &mut described,
                ) {
                    fields.insert("listener".into(), Json::String(id.clone()));
                }
                Value::Data(described)
            })
            .collect();
        let argument = Value::List(vec![Value::List(described)]);
        let answers = move |reply: Value| -> Result<Vec<bool>, RutisError> {
            let wrong = |got: usize| {
                to_cordis(binding(format!(
                    "filter of {name} returned {got} answers for {wanted} listeners"
                )))
            };
            let answers = match reply.json() {
                Ok(Json::Array(answers)) => answers,
                _ => return Err(wrong(0)),
            };
            if answers.len() != wanted {
                return Err(wrong(answers.len()));
            }
            answers
                .iter()
                .map(|answer| answer.as_bool().ok_or_else(|| wrong(answers.len())))
                .collect()
        };
        match sync {
            true => {
                let reply =
                    crate::session::rpc::on_chain(chain.clone(), || selector.call(argument));
                FilterAnswer::Ready(reply.map_err(to_cordis).and_then(answers))
            }
            false => FilterAnswer::Pending(Box::pin(async move {
                let reply = selector.call_async(argument).await.map_err(to_cordis)?;
                let reply = settle(reply).await.map_err(to_cordis)?;
                answers(reply)
            })),
        }
    })
}

/// `filter`, its failure reported as the failure of `emit` `name` from
/// `row` and answered as keeping no listener.
fn reporting(filter: BatchFilter, ctx: Ctx, name: String, row: Option<String>) -> BatchFilter {
    Box::new(move |candidates: Vec<Candidate>| {
        let wanted = candidates.len();
        let settle = move |answer: Result<Vec<bool>, RutisError>| {
            answer.or_else(|error| {
                report_filter(&ctx, &name, row.as_deref(), error);
                Ok(vec![false; wanted])
            })
        };
        match filter(candidates) {
            FilterAnswer::Ready(answer) => FilterAnswer::Ready(settle(answer)),
            FilterAnswer::Pending(answer) => {
                FilterAnswer::Pending(Box::pin(async move { settle(answer.await) }))
            }
        }
    })
}

/// A waterfall's terminal in the emitter's process: called with the event
/// arguments, its value as is.
fn terminal_of(terminal: crate::session::Reference) -> RawTerminal {
    RawTerminal::Async(Box::new(move |event: PayloadRef<'_>| {
        let args = to_wire(event);
        Box::pin(async move {
            let args = args.map_err(to_cordis)?;
            let reply = terminal
                .call_async(Value::List(args))
                .await
                .map_err(to_cordis)?;
            let value = settle(reply).await.map_err(to_cordis)?;
            Ok(result_from_wire(value, Mode::Waterfall))
        })
    }))
}

/// A waterfall's terminal in the emitter's process while it waits
/// synchronously (`sync: true`): called synchronously, along the chain of
/// its innermost wait, its value as is.
fn sync_terminal_of(terminal: crate::session::Reference, waits: Waits) -> RawTerminal {
    RawTerminal::Sync(Box::new(move |event: PayloadRef<'_>| {
        let args = to_wire(event).map_err(to_cordis)?;
        let reply = waits
            .along(|| terminal.call(Value::List(args)))
            .map_err(to_cordis)?;
        Ok(result_from_wire(reply, Mode::Waterfall))
    }))
}

/// The synchronous waits of a session in its `waterfall` dispatches: the
/// chain of each, innermost last (the dispatch's own, then each `next` its
/// listeners call meanwhile). A call into the session while it waits is
/// made along the innermost, so the session takes it as part of its wait.
#[derive(Clone, Default)]
pub(crate) struct Waits(Arc<Mutex<Vec<Vec<String>>>>);

impl Waits {
    fn innermost(&self) -> Option<Vec<String>> {
        self.0.lock().unwrap().last().cloned()
    }

    /// Run `call` along the innermost wait's chain.
    fn along<R>(&self, call: impl FnOnce() -> R) -> R {
        match self.innermost() {
            Some(chain) => crate::session::rpc::on_chain(chain, call),
            None => call(),
        }
    }
}

/// One synchronous wait of a session, counted while it lasts.
struct Waiting(Waits);

impl Waiting {
    fn new(waits: &Waits) -> Self {
        waits.0.lock().unwrap().push(crate::session::rpc::chain());
        Self(waits.clone())
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().pop();
    }
}

/// An entry whose listener is across a session: a runtime's (its
/// `events.on` id) or a linked node's (the id it announced). Each call is
/// one `events.call` (events protocol §5.3).
pub(crate) struct WireEntry {
    pub(crate) connection: WeakConnection,
    pub(crate) id: String,
    pub(crate) kind: ListenerKind,
    /// The origin of the session it came over.
    pub(crate) origin: u64,
    /// The context that owns the entry (the runtime plugin's, the link's):
    /// once it is cancelled (its unload began), a call still waiting for
    /// its answer fails with the session's end instead of holding the
    /// unload, which ends that session only after its wait (§7.1, §7.2).
    pub(crate) owner: Ctx,
    /// The synchronous waits of its session (`sync: true`): meanwhile a
    /// call of one of its own waterfalls is synchronous.
    pub(crate) waits: Waits,
}

/// What an `events.call` carries.
struct Invocation<'a> {
    name: &'a str,
    mode: Mode,
    dispatch: u64,
    own: bool,
    event: PayloadRef<'a>,
    next: Option<Value>,
    /// A call in a waterfall its own runtime waits for synchronously: the
    /// listener's `next` is called synchronously there (§5.4).
    sync: bool,
}

impl WireEntry {
    fn connection(&self) -> Result<Connection, RutisError> {
        self.connection
            .upgrade()
            .ok_or_else(|| to_cordis(Error::Transport("the connection closed".into())))
    }

    fn request(&self, call: Invocation<'_>) -> Result<Value, RutisError> {
        let mut fields = BTreeMap::new();
        fields.insert("listener".into(), Value::Data(json!(self.id)));
        fields.insert(
            "dispatch".into(),
            Value::Data(json!(format!("d{}", call.dispatch))),
        );
        fields.insert("own".into(), Value::Data(json!(call.own)));
        fields.insert("name".into(), Value::Data(json!(call.name)));
        fields.insert("mode".into(), Value::Data(json!(call.mode.as_str())));
        let args = to_wire(call.event).map_err(to_cordis)?;
        fields.insert("args".into(), Value::List(args));
        if let Some(next) = call.next {
            fields.insert("next".into(), next);
        }
        if call.sync {
            fields.insert("sync".into(), Value::Data(json!(true)));
        }
        fields.insert("signal".into(), Value::Signal);
        Ok(Value::List(vec![Value::Record(fields)]))
    }

    /// The dispatch came from this entry's own session.
    fn own(&self, origin: Option<u64>) -> bool {
        origin == Some(self.origin)
    }
}

/// The session whose dispatch a call into `connection` serves, if that is
/// another one ([`crate::session::caller`]).
fn caller_of(connection: &Connection) -> Option<Connection> {
    crate::session::caller().filter(|source| source.tag() != connection.tag())
}

/// Run `call`, a call into `connection`, forwarded from the session whose
/// dispatch it serves ([`caller_of`]).
fn forwarded<R>(connection: &Connection, call: impl FnOnce() -> R) -> R {
    match caller_of(connection) {
        Some(source) => connection.forward(&source, call),
        None => call(),
    }
}

/// `next` as the far end gets it: a function taking no arguments,
/// returning the rest of the chain.
/// `next` for a listener whose runtime waits for this waterfall
/// synchronously: the rest of the chain, waited for here (a wait of the
/// session while it lasts).
fn sync_next_function(next: RemoteNext, waits: Waits) -> Value {
    Value::callback(move |_| {
        let _waiting = Waiting::new(&waits);
        let rest = next.call();
        let rest = match crate::session::caller() {
            Some(source) => wait_here(with_caller(source, rest)),
            None => wait_here(rest),
        };
        result_to_wire(rest.map_err(|error| to_session(&error))?)
    })
}

pub(crate) fn next_function(next: RemoteNext) -> Value {
    Value::callback(move |_| {
        let next = next.clone();
        let rest = async move {
            let rest = next.call().await.map_err(|error| to_session(&error))?;
            match result_to_wire(rest)? {
                // Only a listener of a runtime waiting for this waterfall
                // synchronously answers a promise unsettled (`sync: true`,
                // §5.4): it settles once that wait ends, which waits for
                // this listener.
                Value::Reference(value) if value.is_future() => Err(promise_of_waiting()),
                rest => Ok(rest),
            }
        };
        // The rest of the chain runs on behalf of the session calling it.
        Ok(match crate::session::caller() {
            Some(source) => Value::future(with_caller(source, rest)),
            None => Value::future(rest),
        })
    })
}

impl RemoteListener for WireEntry {
    fn kind(&self) -> ListenerKind {
        self.kind
    }

    fn id(&self) -> Option<&str> {
        Some(&self.id)
    }

    fn call<'a>(
        &'a self,
        call: RemoteCall<'a>,
    ) -> BoxFuture<'a, Result<Option<rutis::internal::events::Value>, RutisError>> {
        let mode = call.mode();
        let own = self.own(call.origin());
        let connection = match self.connection() {
            Ok(connection) => connection,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        // The call, the `next` it hands over, and the wait for what it
        // answers go out with the chain of the session whose dispatch this
        // is, rebased: a function made here records the chain it is made in.
        // A waterfall of this entry's own runtime, which waits for it
        // synchronously: the call is synchronous, and what the listener
        // returns (a promise, for an asynchronous one) is its value.
        if own && mode == Mode::Waterfall {
            if let Some(chain) = self.waits.innermost() {
                // On a thread of its own, so the chain is polled meanwhile:
                // the listener's `next` runs the rest of it.
                let request = self.request(Invocation {
                    name: call.name(),
                    mode,
                    dispatch: call.dispatch(),
                    own,
                    event: call.event(),
                    next: call
                        .next()
                        .map(|next| sync_next_function(next, self.waits.clone())),
                    sync: true,
                });
                let answer = tokio::task::spawn_blocking(move || {
                    let request = request?;
                    crate::session::rpc::on_chain(chain, || {
                        connection.invoke("", "events.call", request)
                    })
                    .map_err(to_cordis)
                });
                return Box::pin(async move {
                    let reply = answer
                        .await
                        .map_err(|error| to_cordis(Error::Transport(error.to_string())))??;
                    Ok(result_from_wire(reply, mode))
                });
            }
        }
        let source = caller_of(&connection);
        let build = || {
            self.request(Invocation {
                name: call.name(),
                mode,
                dispatch: call.dispatch(),
                own,
                event: call.event(),
                next: call.next().map(next_function),
                sync: false,
            })
        };
        let request = match &source {
            Some(source) => connection.forward(source, build),
            None => build(),
        };
        // Queued now, so calls started one after another arrive in that
        // order (an `emit`'s calls, §5.5); only the answer is awaited.
        let send = || {
            request.and_then(|request| {
                connection
                    .invoke_settled("", "events.call", request)
                    .map_err(to_cordis)
            })
        };
        let started = match &source {
            Some(source) => connection.forward(source, send),
            None => send(),
        };
        let unloading = self.owner.cancelled();
        let run = async move {
            // Until the listener's answer settles: given up before, the
            // call is cancelled where it runs (§5.7).
            let answer = async move {
                let value = started?.await.map_err(to_cordis)?;
                Ok(result_from_wire(value, mode))
            };
            // The owner's unload waits for its calls before it ends the
            // session: a call still unanswered then fails once, as when
            // the session ends (§7.2), and is cancelled where it runs.
            tokio::select! {
                biased;
                out = answer => out,
                () = unloading => Err(to_cordis(Error::Transport("the connection closed".into()))),
            }
        };
        match source {
            Some(source) => Box::pin(connection.forward_async(&source, run)),
            None => Box::pin(run),
        }
    }

    fn call_sync(
        &self,
        call: RemoteSyncCall<'_>,
    ) -> Result<Option<rutis::internal::events::Value>, RutisError> {
        let mode = call.mode();
        let own = self.own(call.origin());
        let connection = self.connection()?;
        let reply = forwarded(&connection, || {
            let request = self.request(Invocation {
                name: call.name(),
                mode,
                dispatch: call.dispatch(),
                own,
                event: call.event(),
                next: None,
                sync: false,
            })?;
            connection
                .invoke("", "events.call", request)
                .map_err(to_cordis)
        })?;
        Ok(result_from_wire(reply, mode))
    }
}

/// Poll `future` to completion on this thread, a runtime's worker or not:
/// on a worker of a multi-thread runtime, the worker's queued tasks are
/// handed to another thread meanwhile (`block_in_place`), so a task woken
/// here still runs.
fn wait_here<F: Future>(future: F) -> F::Output {
    let worker = tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
    match worker {
        true => tokio::task::block_in_place(|| block_on(future)),
        false => block_on(future),
    }
}

/// Poll `future` to completion on this thread.
#[allow(dead_code)]
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let thread = std::thread::current();
    let waker = std::task::Waker::from(Arc::new(Unpark(thread)));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = Pin::as_mut(&mut future).poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

#[cfg(test)]
mod tests {
    // MARKER

    use std::cell::RefCell;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use super::*;

    /// The endpoint the near ends of these sessions open as.
    fn rust() -> crate::session::rpc::Endpoint {
        crate::session::rpc::Endpoint::rust(crate::channel::PeerId::new("rust").unwrap())
    }

    /// Answers every call with nothing.
    struct Quiet;
    impl crate::session::Dispatch for Quiet {
        fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
            Ok(Value::Undefined)
        }
    }

    thread_local! {
        /// Runs in `events.on` between the row's check and the entry's
        /// record.
        pub(super) static REGISTERING: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
        /// Runs in `row_ended` and `row_unloading` between the row's entries
        /// leaving the table and its emits being let go (§7.1 step 1).
        pub(super) static BETWEEN: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }

    /// Run the hook of [`BETWEEN`], when a test set one on this thread.
    pub(super) fn between_the_steps() {
        BETWEEN.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
    }

    /// The remote entries of `name` a dispatch of it would select now
    /// (§5.1).
    fn selectable(ctx: &Ctx, name: &str) -> usize {
        ctx.events()
            .subscriptions()
            .into_iter()
            .filter(|entry| entry.remote && entry.name == name)
            .count()
    }

    /// Drives the tasks of the session's runtime until they have nothing
    /// left to do: the emits it was woken for start, and the disposals it
    /// spawned finish.
    fn pump(runtime: &tokio::runtime::Runtime) {
        runtime.block_on(async {
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        });
    }

    /// guarantee: events/hub-lifecycle
    /// #1133: a row's end takes its entries out of the table as one step
    /// with letting go of its emits. Letting go starts the session's next
    /// emit on its task (§5.5), and the kernel withdraws an entry on a task
    /// of its own, so an emit started by the let-go could still select the
    /// entries of the row that ended (events protocol §5.1, §7.1 step 1).
    ///
    /// Pinned where the window was: the hook holds the end between its two
    /// steps while the session's emits are driven, so what an emit of the
    /// other row selects there is read, not raced for.
    fn a_rows_end_leaves_nothing_of_it_to_select(end: fn(&SessionEvents, &str)) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let mut entered = runtime.enter();
        let root = Ctx::root_with(runtime.handle().clone());
        let (near, far) = crate::memory::pair();
        let near = Connection::open(near, Arc::new(Quiet), rust()).unwrap();
        let session =
            SessionEvents::join(Some(&root), near.clone(), "rt".into(), Vec::new()).unwrap();
        // Both rows listen to one name: an emit of row `b` that is offered
        // row `a`'s listener too is the row outliving its end.
        for row in ["a", "b"] {
            session.row_loading(row, Vec::new(), row.into());
        }
        for (id, row) in [("1", "a"), ("2", "b")] {
            session
                .on(Value::Data(json!({
                    "id": id, "name": "t/x", "row": row, "plugin": "p", "kind": "any",
                })))
                .expect("the listener registers");
        }
        assert_eq!(selectable(&root, "t/x"), 2, "both rows listen");

        // Row `a`'s emit waits for its filter: the session's emits are busy.
        let waiting = Box::pin(std::future::pending::<()>()) as BoxFuture<'static, ()>;
        assert!(!session.run_emit(Some("a".into()), EmitRun::new(waiting, Arc::default())));
        // Row `b`'s emit waits its turn: what it would select when it starts.
        let selected: Arc<Mutex<Option<usize>>> = Arc::default();
        let run = {
            let (root, selected) = (root.clone(), selected.clone());
            Box::pin(async move {
                *selected.lock().unwrap() = Some(selectable(&root, "t/x"));
            }) as BoxFuture<'static, ()>
        };
        assert!(!session.run_emit(Some("b".into()), EmitRun::new(run, Arc::default())));

        // The end is held between its steps; the session's emits are driven
        // there, which is where the window was.
        let (inside, arrived) = std::sync::mpsc::channel::<()>();
        let (go, released) = std::sync::mpsc::channel::<()>();
        let ending = {
            let (session, end) = (session.clone(), end);
            std::thread::spawn(move || {
                BETWEEN.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        let _ = inside.send(());
                        let _ = released.recv();
                    }))
                });
                end(&session, "a");
                BETWEEN.with(|hook| hook.borrow_mut().take());
            })
        };
        arrived.recv().expect("the end reached its steps");
        drop(entered);
        pump(&runtime);
        let in_the_window = *selected.lock().unwrap();
        go.send(()).unwrap();
        ending.join().unwrap();
        entered = runtime.enter();
        pump(&runtime);

        assert!(
            in_the_window != Some(2),
            "an emit that started while the row ended selected the row's listener too"
        );
        assert_eq!(
            *selected.lock().unwrap(),
            Some(1),
            "the other row's emit never started, or saw the ended row's listener"
        );
        assert_eq!(selectable(&root, "t/x"), 1, "the row's listener stayed");
        assert!(
            session.entries.lock().unwrap().len() == 1,
            "the row's entry stayed in the host"
        );
        assert!(emits_are_idle(&session), "the session's emits stayed busy");
        drop((far, near, entered));
    }

    /// guarantee: events/hub-lifecycle
    /// #1327: the answer that ends a row takes its listeners and the emits it
    /// queued before it began unloading; the emissions its cleanups handed
    /// over while it unloaded stay in the queue and start in their turn
    /// (events protocol §5.5, §7.1 step 5).
    #[test]
    fn a_rows_end_keeps_the_cleanup_emissions_it_took_in() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let mut entered = runtime.enter();
        let root = Ctx::root_with(runtime.handle().clone());
        let (near, far) = crate::memory::pair();
        let near = Connection::open(near, Arc::new(Quiet), rust()).unwrap();
        let session =
            SessionEvents::join(Some(&root), near.clone(), "rt".into(), Vec::new()).unwrap();
        session.row_loading("a", Vec::new(), "a".into());
        session.row_loading("b", Vec::new(), "b".into());
        let started: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let marks = |label: &'static str, started: &Arc<Mutex<Vec<&'static str>>>| {
            let started = started.clone();
            Box::pin(async move { started.lock().unwrap().push(label) }) as BoxFuture<'static, ()>
        };

        // Row `a`'s emit waits for its filter: the session's emits are busy.
        let (go, wait) = tokio::sync::oneshot::channel::<()>();
        let waiting = Box::pin(async move {
            let _ = wait.await;
        }) as BoxFuture<'static, ()>;
        assert!(!session.run_emit(Some("a".into()), EmitRun::new(waiting, Arc::default())));
        // Row `b` queues one before its unload starts, one while it unloads.
        assert!(!session.run_emit(
            Some("b".into()),
            EmitRun::new(marks("before", &started), Arc::default())
        ));
        session.row_unloading("b");
        assert!(!session.run_emit(
            Some("b".into()),
            EmitRun::new(marks("cleanup", &started), Arc::default())
        ));
        // The answer: row `b` ends. Its ordinary emit was let go with the
        // unload; the cleanup's emission is the session's to deliver.
        session.row_ended("b");

        go.send(()).unwrap();
        drop(entered);
        pump(&runtime);
        entered = runtime.enter();
        assert_eq!(
            *started.lock().unwrap(),
            vec!["cleanup"],
            "the cleanup's emission did not start, or an ordinary emit did"
        );
        assert!(emits_are_idle(&session), "the session's emits stayed busy");
        drop((far, near, entered));
    }

    /// The session's emits ended idle: nothing ran or waits.
    fn emits_are_idle(session: &SessionEvents) -> bool {
        let emits = session.emits.lock().unwrap();
        !emits.busy && emits.queue.is_empty() && emits.leaving.count.load(Ordering::SeqCst) == 0
    }

    /// [`a_rows_end_leaves_nothing_of_it_to_select`] for `rows.ended`.
    #[test]
    fn a_rows_end_leaves_nothing_of_it_to_select_for_the_next_emit() {
        a_rows_end_leaves_nothing_of_it_to_select(SessionEvents::row_ended);
    }

    /// [`a_rows_end_leaves_nothing_of_it_to_select`] for `rows.unload`
    /// (§7.1 step 1).
    #[test]
    fn a_rows_unload_leaves_nothing_of_it_to_select_for_the_next_emit() {
        a_rows_end_leaves_nothing_of_it_to_select(SessionEvents::row_unloading);
    }

    /// guarantee: events/hub-lifecycle
    /// #1133: the order alone is not enough. The kernel withdraws an entry
    /// on a task of its own, so a row's entries are still selectable after
    /// `row_ended` started their disposers; an emit of another row that
    /// starts meanwhile would select them (§5.1), although the row has
    /// ended. The session's emits wait for the withdrawal (§7.1 step 1).
    #[test]
    fn an_emit_waits_for_the_entries_that_are_leaving_the_table() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let entered = runtime.enter();
        let root = Ctx::root_with(runtime.handle().clone());
        let (near, far) = crate::memory::pair();
        let near = Connection::open(near, Arc::new(Quiet), rust()).unwrap();
        let session =
            SessionEvents::join(Some(&root), near.clone(), "rt".into(), Vec::new()).unwrap();
        session.row_loading("a", Vec::new(), "a".into());
        session
            .on(Value::Data(json!({
                "id": "1", "name": "t/x", "row": "a", "plugin": "p", "kind": "any",
            })))
            .expect("the listener registers");
        // What `row_ended` leaves behind while the row's disposers run.
        let leaving = {
            let emits = session.emits.lock().unwrap();
            emits.leaving.count.fetch_add(1, Ordering::SeqCst);
            emits.leaving.clone()
        };

        let ran = Arc::new(AtomicBool::new(false));
        let run = {
            let ran = ran.clone();
            Box::pin(async move { ran.store(true, Ordering::SeqCst) }) as BoxFuture<'static, ()>
        };
        // No emit runs yet, so this one would start at once; the entries
        // leaving the table hold it (§7.1 step 1).
        assert!(!session.run_emit(Some("b".into()), EmitRun::new(run, Arc::default())));
        drop(entered);
        pump(&runtime);
        assert!(
            !ran.load(Ordering::SeqCst),
            "the emit started while the row's entries were still in the table"
        );

        // Out of the table: the queue goes on.
        leaving.count.fetch_sub(1, Ordering::SeqCst);
        leaving.through.notify_one();
        pump(&runtime);
        assert!(ran.load(Ordering::SeqCst), "the emit never started");
        assert!(emits_are_idle(&session), "the session's emits stayed busy");
        drop((far, near));
    }

    /// An emit that panics while it runs, at once or on its task, does not
    /// leave the session's emits stuck: the next one runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_emit_that_panics_does_not_stop_the_next() {
        let root = Ctx::root().unwrap();
        let (near, _far) = crate::memory::pair();
        let near = Connection::open(near, Arc::new(Quiet), rust()).unwrap();
        let session = SessionEvents::join(Some(&root), near, "rt".into(), Vec::new()).unwrap();
        let ran = Arc::new(AtomicBool::new(false));
        let run = |run: BoxFuture<'static, ()>| EmitRun::new(run, Arc::default());
        let marks = |ran: &Arc<AtomicBool>| {
            let ran = ran.clone();
            run(Box::pin(async move { ran.store(true, Ordering::SeqCst) }))
        };

        // At once, in `run_emit`.
        assert!(session.run_emit(None, run(Box::pin(async { panic!("emit panics") }))));
        assert!(session.run_emit(None, marks(&ran)));
        assert!(ran.swap(false, Ordering::SeqCst));

        // On its task, with the next one queued behind it.
        let (go, wait) = tokio::sync::oneshot::channel::<()>();
        let panics = run(Box::pin(async move {
            let _ = wait.await;
            panic!("emit panics later");
        }));
        assert!(!session.run_emit(None, panics));
        assert!(!session.run_emit(None, marks(&ran)));
        go.send(()).unwrap();
        for _ in 0..200 {
            if ran.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(ran.load(Ordering::SeqCst), "the emit after the panic ran");
        let emits = session.emits.lock().unwrap();
        assert!(!emits.busy && emits.queue.is_empty());
    }

    /// guarantee: events/hub-lifecycle
    /// A row that unloads while one of its listeners registers (on the
    /// session's thread, the unload on another) takes that listener with
    /// it: none stays on the bus after its row (events protocol §7.1).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_row_unloading_while_a_listener_registers_withdraws_it() {
        let root = Ctx::root().unwrap();
        let (near, far) = crate::memory::pair();
        // The far end says nothing: the session stays open, and what the
        // hub tells it waits.
        let near = Connection::open(near, Arc::new(Quiet), rust()).unwrap();
        let session =
            SessionEvents::join(Some(&root), near.clone(), "rt".into(), Vec::new()).unwrap();
        session.row_loading("chat", Vec::new(), "chat".into());
        let ended = Arc::new(AtomicBool::new(false));
        let unloading = {
            let (session, ended) = (session.clone(), ended.clone());
            move || {
                let (session, done) = (session.clone(), ended.clone());
                let unload = std::thread::spawn(move || {
                    // The host's side of `rows.unload`: the row stays loaded
                    // for its own cleanups, with no listeners (§7.1).
                    session.row_unloading("chat");
                    done.store(true, Ordering::SeqCst);
                });
                // Before the fix the unload ran to its end here, before the
                // entry was recorded; now it waits for it.
                for _ in 0..40 {
                    if ended.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                std::mem::forget(unload);
            }
        };
        let registered = tokio::task::spawn_blocking(move || {
            REGISTERING.with(|hook| *hook.borrow_mut() = Some(Box::new(unloading)));
            let out = session.on(Value::Data(json!({
                "id": "1", "name": "t/x", "row": "chat", "plugin": "p", "kind": "any",
            })));
            REGISTERING.with(|hook| hook.borrow_mut().take());
            (out, session)
        });
        let (out, session) = registered.await.unwrap();
        out.unwrap();
        for _ in 0..200 {
            if ended.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(ended.load(Ordering::SeqCst));
        assert!(
            session.entries.lock().unwrap().is_empty(),
            "the entry left with its row"
        );
        for _ in 0..200 {
            if root
                .events()
                .subscriptions()
                .iter()
                .all(|entry| !entry.remote)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            root.events()
                .subscriptions()
                .iter()
                .all(|entry| !entry.remote),
            "no remote entry is left on the bus"
        );
        drop((far, near));
    }

    /// guarantee: events/hub-lifecycle
    /// A listener a row registers once its unload started is not kept: the
    /// host answers `events.on` as usual and takes nothing in, so it hears
    /// no event (events protocol §7.1).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_listener_registered_while_the_row_unloads_is_not_kept() {
        let root = Ctx::root().unwrap();
        let (near, far) = crate::memory::pair();
        let near = Connection::open(near, Arc::new(Quiet), rust()).unwrap();
        let session =
            SessionEvents::join(Some(&root), near.clone(), "rt".into(), Vec::new()).unwrap();
        session.row_loading("chat", Vec::new(), "chat".into());
        session.row_unloading("chat");
        let out = session.on(Value::Data(json!({
            "id": "1", "name": "t/x", "row": "chat", "plugin": "p", "kind": "any",
        })));
        assert!(
            out.is_ok(),
            "its registration is answered as usual: {out:?}"
        );
        assert!(
            session.entries.lock().unwrap().is_empty(),
            "the entry was kept"
        );
        assert!(
            root.events()
                .subscriptions()
                .iter()
                .all(|entry| !entry.remote),
            "the entry reached the bus"
        );
        drop((far, near));
    }

    /// The `rutis/service` listeners on `ctx`'s bus.
    fn service_listeners(ctx: &Ctx) -> usize {
        ctx.events()
            .subscriptions()
            .into_iter()
            .filter(|entry| entry.name == "rutis::ServiceChanged" && !entry.remote)
            .count()
    }

    /// #628: the sessions of one bus share one hub, so the bus has one
    /// `rutis/service` listener however many join; the last one leaving takes
    /// it with it, and the next session makes a new hub. Two buses never
    /// share one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_bus_has_one_hub_and_one_service_listener() {
        fn join(ctx: &Ctx) -> (Arc<SessionEvents>, crate::channel::Channel) {
            let (near, far) = crate::memory::pair();
            let connection = Connection::open(near, Arc::new(Quiet), rust()).unwrap();
            let session =
                SessionEvents::join(Some(ctx), connection, "rt".into(), Vec::new()).unwrap();
            (session, far)
        }
        let root = Ctx::root().unwrap();
        let (first, far_first) = join(&root);
        let (second, far_second) = join(&root);
        let hub = first.hub.clone().expect("a session on a bus has a hub");
        assert!(
            Arc::ptr_eq(&hub, second.hub.as_ref().expect("a hub")),
            "one bus was given two hubs"
        );
        assert_eq!(
            service_listeners(&root),
            1,
            "the bus has one service listener"
        );

        // The last session leaving takes the hub and its listener; the next
        // session makes a new hub with its own.
        let old = Arc::downgrade(&hub);
        drop((first, second, hub, far_first, far_second));
        for _ in 0..200 {
            if old.upgrade().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            old.upgrade().is_none(),
            "the bus's hub stayed after its last session"
        );
        for _ in 0..200 {
            if service_listeners(&root) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            service_listeners(&root),
            0,
            "the service listener stayed after the last session"
        );
        let (again, far_again) = join(&root);
        assert_eq!(service_listeners(&root), 1, "the new hub listens");

        // Another bus: its own hub, its own listener.
        let other = Ctx::root().unwrap();
        let (elsewhere, far_elsewhere) = join(&other);
        assert!(
            !Arc::ptr_eq(
                again.hub.as_ref().expect("a hub"),
                elsewhere.hub.as_ref().expect("a hub")
            ),
            "two buses shared a hub"
        );
        assert_eq!(service_listeners(&other), 1, "the other bus listens");
        drop((again, far_again, elsewhere, far_elsewhere));
    }
}

/// The shuttle model of a row's listeners and its unload (#628).
#[cfg(all(test, rutis_shuttle))]
mod shuttle_tests;
