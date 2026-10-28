//! The event bus: one listener table, five dispatch modes (protocol spec
//! §4–§7, §12.1).

use crate::host::CtxHost;
use crate::internal::CtxInternal;
pub(crate) mod run;
pub(crate) mod table;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use run::{
    filter_any, filter_each, filter_sync, first_poll, report_listener, run_bail, run_parallel,
    run_serial, AsyncNext, ErasedSyncTerminal, ErasedTerminal, FilterSpec, Inv, NoTerminal,
    OwnedInv, RawTerm, Started, SyncNext,
};
use table::{Body, Entry, Table};

use crate::ctx::Ctx;
use crate::error::{Error, ErrorKind};
use crate::events::{
    Dispatch, EntryCall, Event, EventOptions, JsonTranscoder, ListenerInfo, ListenerKind, Mode,
    Payload, PayloadRef, RemoteEntry, RemoteListener, TableChange, TableEntry, Transcoder, Value,
    RESERVED_PREFIX,
};
use crate::fiber::PluginId;
use crate::key::EventKey;
use crate::{BoxFuture, Disposer, Effect};

type Watcher = Arc<dyn Fn(&TableChange) + Send + Sync>;

/// A test hook called after a dispatch selected its listeners (feature
/// `testing`).
#[cfg(feature = "testing")]
type SelectedHook = Arc<dyn Fn(&str, &(dyn std::any::Any + Send + Sync)) + Send + Sync>;

/// A current registration, without its callback or fiber.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct EventSubscription {
    pub id: u64,
    /// The name, or the prefix of a prefix entry.
    pub name: String,
    pub prefix: bool,
    /// The fiber that registered it; `None` once it is gone.
    pub owner: Option<PluginId>,
    pub kind: ListenerKind,
    pub remote: bool,
    pub once: bool,
    pub prepend: bool,
    pub global: bool,
    /// Calls started so far.
    pub calls: u64,
}

/// The `emit` chain of one name: each emit starts its calls after the one
/// before it did (§5.5).
struct Tail {
    generation: u64,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct BusState {
    table: Table,
    #[cfg(feature = "testing")]
    selected: Vec<SelectedHook>,
    watchers: Vec<(u64, Watcher)>,
    next_watcher: u64,
    tails: HashMap<Arc<str>, Tail>,
    /// Accepted emits whose calls have not all finished, by name.
    backlog: HashMap<Arc<str>, BTreeMap<u64, Instant>>,
    next_emit: u64,
}

struct BusCore {
    state: Mutex<BusState>,
    /// Taken while `state` is held, never the other way round.
    notices: Mutex<Notices>,
    delivered: std::sync::Condvar,
    transcoder: RwLock<Arc<dyn Transcoder>>,
    next_dispatch: AtomicU64,
}

/// The event bus of one root: one listener table for every mode.
///
/// Listeners are found by name. A Rust listener is called with the context
/// that registered it, so `ctx.cancelled()` is its own unload signal.
#[derive(Clone)]
pub struct EventBus {
    core: Arc<BusCore>,
}

/// Holds an emit in its name's backlog until every call of it finished.
pub(crate) struct EmitTicket {
    bus: EventBus,
    name: Arc<str>,
    seq: u64,
}

std::thread_local! {
    /// This thread holds the bus state while it spawns an emit's task: a
    /// runtime shutting down drops the task's future inside `spawn`, and its
    /// guards must not take the state again (a deadlock). They say so in
    /// [`DROPPED_INLINE`]; the spawner, still holding the state, cleans up.
    static SPAWNING_EMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The emit task's future was dropped inside `spawn`.
    static DROPPED_INLINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Drop for EmitTicket {
    fn drop(&mut self) {
        if SPAWNING_EMIT.with(std::cell::Cell::get) {
            return;
        }
        let mut state = self.bus.core.state.lock().unwrap();
        if let Some(list) = state.backlog.get_mut(&self.name) {
            list.remove(&self.seq);
            if list.is_empty() {
                state.backlog.remove(&self.name);
            }
        }
    }
}

struct TailCleanup {
    bus: EventBus,
    name: Arc<str>,
    generation: u64,
}

impl Drop for TailCleanup {
    fn drop(&mut self) {
        if SPAWNING_EMIT.with(std::cell::Cell::get) {
            DROPPED_INLINE.with(|dropped| dropped.set(true));
            return;
        }
        let mut state = self.bus.core.state.lock().unwrap();
        if state
            .tails
            .get(&self.name)
            .is_some_and(|tail| tail.generation == self.generation)
        {
            state.tails.remove(&self.name);
            if state.tails.capacity() > 64 && state.tails.len() * 4 < state.tails.capacity() {
                state.tails.shrink_to_fit();
            }
        }
    }
}

/// Options of one dispatch, beyond its name, mode and event.
#[derive(Default)]
pub(crate) struct Opts {
    pub filter: Option<FilterSpec>,
    pub origin: Option<u64>,
    pub framework: bool,
}

thread_local! {
    /// The buses whose table changes this thread is delivering: a change a
    /// watcher makes is delivered after the one it is hearing, by the same
    /// delivery, so it does not wait for itself.
    static DELIVERING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Table changes not yet heard by their watchers, in the order they were
/// made (protocol spec §4.4): one delivery at a time, so a watcher never
/// hears a removal before the addition it follows.
#[derive(Default)]
struct Notices {
    queue: std::collections::VecDeque<(u64, Vec<Watcher>, Vec<TableChange>)>,
    /// The sequence number of the last change queued.
    queued: u64,
    /// The sequence number of the last change delivered.
    delivered: u64,
    delivering: bool,
}

impl EventBus {
    pub(crate) fn new() -> Self {
        Self {
            core: Arc::new(BusCore {
                state: Mutex::new(BusState::default()),
                notices: Mutex::default(),
                delivered: std::sync::Condvar::new(),
                transcoder: RwLock::new(Arc::new(JsonTranscoder)),
                next_dispatch: AtomicU64::new(1),
            }),
        }
    }

    fn ensure_bus(&self, ctx: &Ctx) -> Result<(), Error> {
        if Arc::ptr_eq(&self.core, &ctx.events().core) {
            Ok(())
        } else {
            Err(ErrorKind::Validation {
                issues: vec!["the context belongs to another event bus".into()],
            }
            .into())
        }
    }

    /// The transcoder that converts between event and value types.
    pub fn transcoder(&self) -> Arc<dyn Transcoder> {
        self.core.transcoder.read().unwrap().clone()
    }

    /// Replace the transcoder (the default is [`JsonTranscoder`]).
    pub fn set_transcoder(&self, transcoder: Arc<dyn Transcoder>) {
        *self.core.transcoder.write().unwrap() = transcoder;
    }

    // ── registration ─────────────────────────────────────────────

    /// Change the table under its lock; the watchers hear the changes after,
    /// in the order the changes were made, before this returns.
    fn change<R>(&self, f: impl FnOnce(&mut Table) -> (R, Vec<TableChange>)) -> R {
        let (out, seq) = {
            let mut state = self.core.state.lock().unwrap();
            let (out, changes) = f(&mut state.table);
            let seq = self.enqueue(&state, changes);
            (out, seq)
        };
        if let Some(seq) = seq {
            self.deliver(seq);
        }
        out
    }

    /// Queue `changes` for the current watchers, under the state's lock
    /// (`state`), so their order is the table's. `None` for no changes.
    fn enqueue(&self, state: &BusState, changes: Vec<TableChange>) -> Option<u64> {
        if changes.is_empty() {
            return None;
        }
        let watchers: Vec<Watcher> = state.watchers.iter().map(|(_, w)| w.clone()).collect();
        let mut notices = self.core.notices.lock().unwrap();
        notices.queued += 1;
        let seq = notices.queued;
        notices.queue.push_back((seq, watchers, changes));
        Some(seq)
    }

    /// Return once change `seq` and those before it were heard: delivered
    /// here, one at a time and in order, or by the thread delivering now. A
    /// change made by a watcher while it hears one is delivered right after
    /// by the same delivery.
    fn deliver(&self, seq: u64) {
        let identity = Arc::as_ptr(&self.core) as usize;
        if DELIVERING.with(|buses| buses.borrow().contains(&identity)) {
            return;
        }
        let mut notices = self.core.notices.lock().unwrap();
        loop {
            if !notices.delivering {
                let Some((at, watchers, changes)) = notices.queue.pop_front() else {
                    return;
                };
                notices.delivering = true;
                drop(notices);
                DELIVERING.with(|buses| buses.borrow_mut().push(identity));
                notify(&watchers, &changes);
                DELIVERING.with(|buses| {
                    buses.borrow_mut().retain(|bus| *bus != identity);
                });
                notices = self.core.notices.lock().unwrap();
                notices.delivered = at;
                notices.delivering = false;
                self.core.delivered.notify_all();
                continue;
            }
            if notices.delivered >= seq {
                return;
            }
            notices = self.core.delivered.wait(notices).unwrap();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn register(
        &self,
        ctx: &Ctx,
        name: Arc<str>,
        prefix: bool,
        body: Body,
        opts: EventOptions,
        remote: Option<(ListenerInfo, u64)>,
        label: &str,
    ) -> Result<Disposer, Error> {
        ctx.registration_preflight()?;
        if !Arc::ptr_eq(&self.core, &ctx.events().core) {
            return Err(ErrorKind::Validation {
                issues: vec!["listener belongs to another event bus".into()],
            }
            .into());
        }
        let (info, origin) = match remote {
            Some((mut info, origin)) => {
                info.name = name.to_string();
                info.prefix = prefix;
                info.global = opts.global;
                (info, Some(origin))
            }
            None => (
                ListenerInfo {
                    name: name.to_string(),
                    prefix,
                    plugin: ctx.plugin_name(),
                    row: ctx.row(),
                    runtime: None,
                    node: None,
                    scope: ctx.scope(),
                    global: opts.global,
                },
                None,
            ),
        };
        let info = Arc::new(info);
        let label = format!("{label}: {}{name}", if prefix { "prefix " } else { "" });
        let bus = self.clone();
        let owner_ctx = ctx.clone();
        let mut added: Option<u64> = None;
        let pending = &mut added;
        let registered = ctx.register_internal_effect_named(label, move |fiber, generation, _| {
            let mut state = bus.core.state.lock().unwrap();
            let id = state.table.next_id();
            let position = state.table.position(opts.prepend);
            let entry = Arc::new(Entry {
                id,
                position,
                name,
                prefix,
                once: opts.once,
                prepend: opts.prepend,
                info,
                origin,
                body,
                ctx: owner_ctx,
                owner: Arc::downgrade(fiber),
                generation,
                withdrawn: Default::default(),
                claimed: Default::default(),
                calls: AtomicU64::new(0),
            });
            state.table.insert(entry.clone());
            *pending = bus.enqueue(&state, vec![TableChange::Added(entry.table_entry())]);
            drop(state);
            let bus = bus.clone();
            Ok(Effect::Disposer(Box::new(move || {
                entry.withdrawn.store(true, Ordering::SeqCst);
                bus.change(|table| {
                    let removed = table.remove(&entry);
                    let changes = if removed {
                        vec![TableChange::Removed(entry.table_entry())]
                    } else {
                        Vec::new()
                    };
                    ((), changes)
                });
                Ok(())
            })))
        });
        // The entry is in the table; its watchers hear it before `on` returns.
        if let Some(seq) = added {
            self.deliver(seq);
        }
        registered
    }

    /// Register a remote entry: its calls go to `listener` (protocol spec
    /// §4). Owned by `ctx`'s fiber like any listener; its disposer
    /// withdraws it.
    pub fn on_remote(
        &self,
        ctx: &Ctx,
        entry: RemoteEntry,
        listener: Arc<dyn RemoteListener>,
    ) -> Result<Disposer, Error> {
        let RemoteEntry {
            name,
            prefix,
            info,
            options,
            origin,
        } = entry;
        self.register(
            ctx,
            Arc::from(name),
            prefix,
            Body::Remote(listener),
            options,
            Some((info, origin)),
            "remote event listener",
        )
    }

    /// The current table, and every later change, to `watcher`. Changes
    /// made before this returns are in the snapshot; later ones reach the
    /// watcher (a Rust registration before its `on` returns).
    pub fn watch_table(
        &self,
        owner: &Ctx,
        watcher: impl Fn(&TableChange) + Send + Sync + 'static,
    ) -> Result<(Vec<TableEntry>, Disposer), Error> {
        let bus = self.clone();
        let watcher: Watcher = Arc::new(watcher);
        let mut snapshot = Vec::new();
        let out = &mut snapshot;
        let disposer = owner.register_internal_effect_named(
            "event table watcher".into(),
            move |_, _, _| {
                let mut state = bus.core.state.lock().unwrap();
                *out = state
                    .table
                    .entries()
                    .iter()
                    .map(|e| e.table_entry())
                    .collect();
                state.next_watcher += 1;
                let id = state.next_watcher;
                state.watchers.push((id, watcher));
                drop(state);
                Ok(Effect::Disposer(Box::new(move || {
                    bus.core
                        .state
                        .lock()
                        .unwrap()
                        .watchers
                        .retain(|(watcher, _)| *watcher != id);
                    Ok(())
                })))
            },
        )?;
        Ok((snapshot, disposer))
    }

    /// Current registrations, in registration order.
    pub fn subscriptions(&self) -> Vec<EventSubscription> {
        let state = self.core.state.lock().unwrap();
        state
            .table
            .entries()
            .iter()
            .map(|entry| EventSubscription {
                id: entry.id,
                name: entry.name.to_string(),
                prefix: entry.prefix,
                owner: entry.owner.upgrade().map(|fiber| fiber.id),
                kind: entry.kind(),
                remote: matches!(entry.body, Body::Remote(_)),
                once: entry.once,
                prepend: entry.prepend,
                global: entry.info.global,
                calls: entry.calls.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// (entries, exact names, emit chains).
    #[cfg(test)]
    pub(crate) fn table_counts(&self) -> (usize, usize, usize) {
        let state = self.core.state.lock().unwrap();
        (state.table.len(), state.table.names(), state.tails.len())
    }

    /// Allocated slots: (exact names, entries by id, emit chains).
    #[cfg(test)]
    pub(crate) fn table_capacities(&self) -> (usize, usize, usize) {
        let state = self.core.state.lock().unwrap();
        let (names, ids) = state.table.capacities();
        (names, ids, state.tails.capacity())
    }

    #[cfg(test)]
    pub(crate) fn max_list_capacity(&self) -> usize {
        self.core.state.lock().unwrap().table.max_list_capacity()
    }

    /// Accepted `emit`s whose calls have not all finished, per name.
    pub(crate) fn backlogs(&self) -> Vec<crate::EventBacklog> {
        let now = Instant::now();
        let state = self.core.state.lock().unwrap();
        let mut backlogs: Vec<_> = state
            .backlog
            .iter()
            .filter_map(|(name, list)| {
                let oldest = list.values().min()?;
                Some(crate::EventBacklog {
                    name: name.to_string(),
                    pending: list.len(),
                    oldest: now.saturating_duration_since(*oldest),
                })
            })
            .collect();
        backlogs.sort_by_key(|backlog| std::cmp::Reverse(backlog.oldest));
        backlogs
    }

    // ── dispatch ─────────────────────────────────────────────────

    /// §5.1 step 1, after the reserved-name check.
    #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
    fn snapshot(
        &self,
        ctx: &Ctx,
        name: &str,
        event: &(dyn std::any::Any + Send + Sync),
        opts: &mut Opts,
    ) -> Result<Vec<Arc<Entry>>, Error> {
        self.ensure_bus(ctx)?;
        if !opts.framework && name.starts_with(RESERVED_PREFIX) {
            return Err(ErrorKind::ReservedEvent {
                name: name.to_owned(),
            }
            .into());
        }
        let state = self.core.state.lock().unwrap();
        let entries = state.table.select(name);
        #[cfg(feature = "testing")]
        let hooks = state.selected.clone();
        drop(state);
        #[cfg(feature = "testing")]
        for hook in hooks {
            hook(name, event);
        }
        Ok(entries)
    }

    /// Test hook (feature `testing`): call `hook` with the name and event
    /// of every dispatch on this bus, right after it selected its
    /// listeners. A listener registered after the hook ran for a dispatch
    /// is not in that dispatch. The hook stays as long as the bus.
    #[cfg(feature = "testing")]
    #[doc(hidden)]
    pub fn on_selected_for_testing(
        &self,
        hook: impl Fn(&str, &(dyn std::any::Any + Send + Sync)) + Send + Sync + 'static,
    ) {
        self.core
            .state
            .lock()
            .unwrap()
            .selected
            .push(Arc::new(hook));
    }

    /// Begin a call of `entry` (§5.1 step 3): `None` skips it. A `once`
    /// entry is claimed here, by the first call that starts, not when a
    /// dispatch selects it: one a filter dropped, or a `serial` / `bail`
    /// ended before, stays for the next dispatch.
    pub(crate) fn start(&self, entry: &Arc<Entry>) -> Option<table::CallFlight> {
        let flight = entry.start()?;
        if entry.once {
            if entry.claimed.swap(true, Ordering::SeqCst) {
                return None;
            }
            entry.withdrawn.store(true, Ordering::SeqCst);
            self.change(|table| {
                let changes = match table.remove(entry) {
                    true => vec![TableChange::Removed(entry.table_entry())],
                    false => Vec::new(),
                };
                ((), changes)
            });
        }
        entry.calls.fetch_add(1, Ordering::Relaxed);
        Some(flight)
    }

    fn dispatch_id(&self) -> u64 {
        self.core.next_dispatch.fetch_add(1, Ordering::Relaxed)
    }

    /// `emit`: every call starts in registration order, after the calls of
    /// the name's previous emit started; nothing waits for them. The
    /// receiver fires once every call started.
    pub(crate) fn emit_erased(
        &self,
        ctx: &Ctx,
        name: Arc<str>,
        payload: Payload,
        mut opts: Opts,
    ) -> Result<tokio::sync::oneshot::Receiver<()>, Error> {
        let (started, receiver) = tokio::sync::oneshot::channel();
        let mut entries = self.snapshot(ctx, &name, &**payload.arc(), &mut opts)?;
        let mut batch = None;
        match opts.filter.take() {
            Some(FilterSpec::Each(keep)) => match filter_each(entries, &*keep) {
                Ok(kept) => entries = kept,
                Err(error) => {
                    report_filter(ctx, &name, error);
                    let _ = started.send(());
                    return Ok(receiver);
                }
            },
            Some(batch_filter) => batch = Some(batch_filter),
            None => {}
        }
        if entries.is_empty() {
            let _ = started.send(());
            return Ok(receiver);
        }
        let owned = Arc::new(OwnedInv {
            bus: self.clone(),
            emitter: ctx.clone(),
            name: name.clone(),
            mode: Mode::Emit,
            payload,
            dispatch: self.dispatch_id(),
            origin: opts.origin,
        });
        let handle = ctx.handle().clone();
        let mut state = self.core.state.lock().unwrap();
        state.next_emit += 1;
        let seq = state.next_emit;
        state
            .backlog
            .entry(name.clone())
            .or_default()
            .insert(seq, Instant::now());
        let ticket = Arc::new(EmitTicket {
            bus: self.clone(),
            name: name.clone(),
            seq,
        });
        let (generation, prev) = match state.tails.remove(&name) {
            Some(tail) => (tail.generation + 1, Some(tail.task)),
            None => (0, None),
        };
        let cleanup = TailCleanup {
            bus: self.clone(),
            name: name.clone(),
            generation,
        };
        let spawn_handle = handle.clone();
        SPAWNING_EMIT.with(|spawning| spawning.set(true));
        let task = handle.spawn(async move {
            let _cleanup = cleanup;
            if let Some(prev) = prev {
                let _ = prev.await;
            }
            let entries = if batch.is_some() {
                match filter_any(&owned.name, entries, batch).await {
                    Ok(entries) => entries,
                    Err(error) => {
                        report_filter(&owned.emitter, &owned.name, error);
                        let _ = started.send(());
                        return;
                    }
                }
            } else {
                entries
            };
            for entry in entries {
                let Some(flight) = owned.bus.start(&entry) else {
                    continue;
                };
                let call = run::emit_call(owned.clone(), entry, flight, Some(ticket.clone()));
                if let Started::Pending(rest) = first_poll(call).await {
                    spawn_handle.spawn(rest);
                }
            }
            drop(ticket);
            let _ = started.send(());
        });
        SPAWNING_EMIT.with(|spawning| spawning.set(false));
        if DROPPED_INLINE.with(|dropped| dropped.replace(false)) {
            // The runtime is shutting down: the task never runs. Its
            // guards left the backlog record and the chain to this frame.
            if let Some(list) = state.backlog.get_mut(&name) {
                list.remove(&seq);
                if list.is_empty() {
                    state.backlog.remove(&name);
                }
            }
            return Ok(receiver);
        }
        state.tails.insert(name, Tail { generation, task });
        Ok(receiver)
    }

    pub(crate) fn parallel_erased<'a>(
        &self,
        ctx: &'a Ctx,
        name: Arc<str>,
        payload: Payload,
        mut opts: Opts,
    ) -> BoxFuture<'a, Result<(), Error>> {
        let bus = self.clone();
        Box::pin(async move {
            let entries = bus.snapshot(ctx, &name, &**payload.arc(), &mut opts)?;
            let entries = filter_any(&name, entries, opts.filter.take()).await?;
            if entries.is_empty() {
                return Ok(());
            }
            let owned = Arc::new(OwnedInv {
                bus: bus.clone(),
                emitter: ctx.clone(),
                name,
                mode: Mode::Parallel,
                payload,
                dispatch: bus.dispatch_id(),
                origin: opts.origin,
            });
            run_parallel(owned, entries, ctx.handle()).await
        })
    }

    pub(crate) fn serial_erased<'a>(
        &self,
        ctx: &'a Ctx,
        name: Arc<str>,
        payload: Payload,
        mut opts: Opts,
    ) -> BoxFuture<'a, Result<Option<Value>, Error>> {
        let bus = self.clone();
        Box::pin(async move {
            let entries = bus.snapshot(ctx, &name, &**payload.arc(), &mut opts)?;
            let entries = filter_any(&name, entries, opts.filter.take()).await?;
            let inv = Inv {
                bus: &bus,
                emitter: ctx,
                name: &name,
                mode: Mode::Serial,
                event: payload.as_ref(),
                dispatch: bus.dispatch_id(),
                origin: opts.origin,
            };
            run_serial(&inv, &entries).await
        })
    }

    pub(crate) fn waterfall_erased<'a>(
        &self,
        ctx: &'a Ctx,
        name: Arc<str>,
        payload: Payload,
        terminal: Box<dyn ErasedTerminal + Send>,
        mut opts: Opts,
    ) -> BoxFuture<'a, Result<Option<Value>, Error>> {
        let bus = self.clone();
        Box::pin(async move {
            let entries = bus.snapshot(ctx, &name, payload.as_ref().as_any(), &mut opts)?;
            let entries = filter_any(&name, entries, opts.filter.take()).await?;
            let dispatch = bus.dispatch_id();
            if entries.is_empty() {
                // No listener: the terminal alone, with no chain to share.
                let inv = Inv {
                    bus: &bus,
                    emitter: ctx,
                    name: &name,
                    mode: Mode::Waterfall,
                    event: payload.as_ref(),
                    dispatch,
                    origin: opts.origin,
                };
                return terminal.call(&inv).await;
            }
            let inv = OwnedInv {
                bus,
                emitter: ctx.clone(),
                name,
                mode: Mode::Waterfall,
                payload,
                dispatch,
                origin: opts.origin,
            };
            AsyncNext::start(inv, entries, terminal).invoke().await
        })
    }

    pub(crate) fn bail_erased(
        &self,
        ctx: &Ctx,
        name: Arc<str>,
        event: PayloadRef<'_>,
        mut opts: Opts,
    ) -> Result<Option<Value>, Error> {
        ctx.registration_preflight()?;
        let entries = self.snapshot(ctx, &name, event.as_any(), &mut opts)?;
        let entries = filter_sync(&name, entries, opts.filter.take())?;
        let inv = Inv {
            bus: self,
            emitter: ctx,
            name: &name,
            mode: Mode::Bail,
            event,
            dispatch: self.dispatch_id(),
            origin: opts.origin,
        };
        run_bail(&inv, &entries)
    }

    pub(crate) fn waterfall_sync_erased(
        &self,
        ctx: &Ctx,
        name: Arc<str>,
        event: PayloadRef<'_>,
        terminal: &dyn ErasedSyncTerminal,
        mut opts: Opts,
    ) -> Result<Option<Value>, Error> {
        ctx.registration_preflight()?;
        let entries = self.snapshot(ctx, &name, event.as_any(), &mut opts)?;
        let entries = filter_sync(&name, entries, opts.filter.take())?;
        let inv = Inv {
            bus: self,
            emitter: ctx,
            name: &name,
            mode: Mode::WaterfallSync,
            event,
            dispatch: self.dispatch_id(),
            origin: opts.origin,
        };
        SyncNext {
            inv: &inv,
            chain: &entries,
            terminal,
        }
        .invoke()
    }

    // ── typed dispatch ───────────────────────────────────────────

    /// `emit` of a framework event: only rutis emits `rutis/*`.
    pub fn emit_framework<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> Result<(), Error> {
        let opts = Opts {
            framework: true,
            ..Opts::default()
        };
        self.emit_erased(ctx, key.checked_name()?, Payload::new(event), opts)
            .map(drop)
    }

    // ── raw dispatch ─────────────────────────────────────────────

    fn raw_opts(d: &mut Dispatch) -> Opts {
        Opts {
            filter: d.filter.take().map(FilterSpec::Batch),
            origin: d.origin,
            framework: d.framework,
        }
    }

    /// Dispatch an erased event (`emit`, `parallel`, `serial`,
    /// `waterfall`). For `emit` it resolves once every call started; its
    /// failures are reported, the result is `Ok(None)`.
    pub fn dispatch<'a>(
        &self,
        ctx: &'a Ctx,
        mut d: Dispatch,
    ) -> BoxFuture<'a, Result<Option<Value>, Error>> {
        let opts = Self::raw_opts(&mut d);
        let name: Arc<str> = Arc::from(d.name.as_str());
        match d.mode {
            Mode::Emit => match self.emit_erased(ctx, name, d.payload, opts) {
                Ok(started) => Box::pin(async move {
                    let _ = started.await;
                    Ok(None)
                }),
                Err(error) => Box::pin(std::future::ready(Err(error))),
            },
            Mode::Parallel => {
                let run = self.parallel_erased(ctx, name, d.payload, opts);
                Box::pin(async move { run.await.map(|()| None) })
            }
            Mode::Serial => self.serial_erased(ctx, name, d.payload, opts),
            Mode::Waterfall => {
                let bus = self.clone();
                let terminal = d.terminal.take();
                let payload = d.payload;
                Box::pin(async move {
                    let terminal: Box<dyn ErasedTerminal + Send> = match terminal {
                        Some(terminal) => Box::new(RawTerm(Mutex::new(Some(terminal)))),
                        None => Box::new(NoTerminal),
                    };
                    bus.waterfall_erased(ctx, name, payload, terminal, opts)
                        .await
                })
            }
            Mode::Bail | Mode::WaterfallSync => Box::pin(std::future::ready(Err(Error::from(
                ErrorKind::Validation {
                    issues: vec![format!("{} is synchronous: use dispatch_sync", d.mode)],
                },
            )))),
        }
    }

    /// Dispatch an erased event synchronously (`bail`, `waterfall-sync`).
    pub fn dispatch_sync(&self, ctx: &Ctx, mut d: Dispatch) -> Result<Option<Value>, Error> {
        let opts = Self::raw_opts(&mut d);
        let name: Arc<str> = Arc::from(d.name.as_str());
        match d.mode {
            Mode::Bail => self.bail_erased(ctx, name, d.payload.as_ref(), opts),
            Mode::WaterfallSync => {
                let terminal = d.terminal.take();
                match terminal {
                    Some(terminal) => {
                        let terminal = RawTerm(Mutex::new(Some(terminal)));
                        self.waterfall_sync_erased(ctx, name, d.payload.as_ref(), &terminal, opts)
                    }
                    None => {
                        self.waterfall_sync_erased(ctx, name, d.payload.as_ref(), &NoTerminal, opts)
                    }
                }
            }
            mode => Err(ErrorKind::Validation {
                issues: vec![format!("{mode} is asynchronous: use dispatch")],
            }
            .into()),
        }
    }

    fn live_entry(&self, id: u64, name: &str) -> Result<Arc<Entry>, Error> {
        let entry = self.core.state.lock().unwrap().table.get(id);
        // An id the far end kept from before (its link restarted, this
        // entry's id reused) must not reach an entry of another name.
        let named = |e: &Arc<Entry>| match e.prefix {
            true => name.starts_with(&*e.name),
            false => *e.name == *name,
        };
        entry.filter(|e| named(e) && e.live()).ok_or_else(|| {
            Error::from(ErrorKind::ListenerWithdrawn {
                id,
                name: name.to_owned(),
            })
        })
    }

    /// Call one entry, outside a dispatch, as `call.mode` would (a node
    /// running a call the other node sent, protocol spec §9). Returns its
    /// result as it is; in `emit` a failure is reported and the result is
    /// `Ok(None)`. A withdrawn or unknown id: `ListenerWithdrawn`.
    pub fn invoke_entry<'a>(
        &self,
        ctx: &'a Ctx,
        id: u64,
        call: EntryCall,
    ) -> BoxFuture<'a, Result<Option<Value>, Error>> {
        let bus = self.clone();
        Box::pin(async move {
            let entry = bus.live_entry(id, &call.name)?;
            let Some(flight) = bus.start(&entry) else {
                return Err(Error::from(ErrorKind::ListenerWithdrawn {
                    id,
                    name: call.name,
                }));
            };
            let name: Arc<str> = Arc::from(call.name.as_str());
            let inv = Inv {
                bus: &bus,
                emitter: ctx,
                name: &name,
                mode: call.mode,
                event: call.payload.as_ref(),
                dispatch: call.dispatch,
                origin: call.origin,
            };
            let out = match call.mode {
                Mode::Waterfall => {
                    let terminal: Box<dyn ErasedTerminal + Send> = match call.next {
                        Some(next) => Box::new(RawTerm(Mutex::new(Some(next)))),
                        None => Box::new(NoTerminal),
                    };
                    // The rest of the far end's chain is its `next`: the
                    // terminal of a chain of no entries here.
                    let rest = OwnedInv {
                        bus: bus.clone(),
                        emitter: ctx.clone(),
                        name: name.clone(),
                        mode: call.mode,
                        payload: call.payload.clone(),
                        dispatch: call.dispatch,
                        origin: call.origin,
                    };
                    let next = AsyncNext::start(rest, Vec::new(), terminal);
                    run::call_async(&entry, &inv, Some(next)).await
                }
                Mode::Bail | Mode::WaterfallSync => Err(Error::from(ErrorKind::Validation {
                    issues: vec![format!(
                        "{} is synchronous: use invoke_entry_sync",
                        call.mode
                    )],
                })),
                _ => run::call_async(&entry, &inv, None).await,
            };
            drop(flight);
            match (call.mode, out) {
                (Mode::Emit, Err(error)) => {
                    report_listener(ctx, &call.name, &entry, error);
                    Ok(None)
                }
                (_, out) => out,
            }
        })
    }

    /// [`EventBus::invoke_entry`] for `bail` and `waterfall-sync`.
    pub fn invoke_entry_sync(
        &self,
        ctx: &Ctx,
        id: u64,
        call: EntryCall,
    ) -> Result<Option<Value>, Error> {
        let entry = self.live_entry(id, &call.name)?;
        let Some(flight) = self.start(&entry) else {
            return Err(ErrorKind::ListenerWithdrawn {
                id,
                name: call.name,
            }
            .into());
        };
        let name: Arc<str> = Arc::from(call.name.as_str());
        let inv = Inv {
            bus: self,
            emitter: ctx,
            name: &name,
            mode: call.mode,
            event: call.payload.as_ref(),
            dispatch: call.dispatch,
            origin: call.origin,
        };
        let out = match call.mode {
            Mode::Bail => run::call_sync(&entry, &inv, None),
            Mode::WaterfallSync => {
                let terminal = RawTerm(Mutex::new(call.next));
                let next = SyncNext {
                    inv: &inv,
                    chain: &[],
                    terminal: &terminal,
                };
                run::call_sync(&entry, &inv, Some(next))
            }
            mode => Err(Error::from(ErrorKind::Validation {
                issues: vec![format!("{mode} is asynchronous: use invoke_entry")],
            })),
        };
        drop(flight);
        out
    }
}

fn notify(watchers: &[Watcher], changes: &[TableChange]) {
    for change in changes {
        for watcher in watchers {
            let _ = catch_unwind(AssertUnwindSafe(|| watcher(change)));
        }
    }
}

fn report_filter(ctx: &Ctx, name: &str, error: Error) {
    let emitter = match ctx.row() {
        Some(row) => format!("row {row}"),
        None => format!("plugin {}", ctx.plugin_name()),
    };
    let sink = ctx.error_sink();
    let report = ErrorKind::FilterFailed {
        name: name.to_owned(),
        emitter,
        error: error.shared(),
    };
    let _ = catch_unwind(AssertUnwindSafe(|| sink(Arc::new(report))));
}

#[cfg(test)]
mod transient_tests;
