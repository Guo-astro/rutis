//! The event bus: one listener table, five dispatch modes (protocol spec
//! §4–§7, §12.1).

pub(crate) mod run;
pub(crate) mod table;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Instant;

use run::{
    filter_any, filter_each, filter_sync, first_poll, report_listener, run_bail, run_parallel,
    run_serial, AsyncNext, ErasedSyncTerminal, ErasedTerminal, FilterSpec, Inv, NoTerminal,
    OwnedInv, RawTerm, Started, SyncNext,
};
use table::{Body, Entry, Table};

use crate::ctx::Ctx;
use crate::error::{panic_error, CordisError};
use crate::events::{
    AsyncAdapter, AsyncPatternAdapter, Dispatch, EntryCall, Event, EventOptions, JsonTranscoder,
    Listener, ListenerInfo, ListenerKind, Mode, PatternListener, Payload, PayloadRef, RemoteEntry,
    RemoteListener, Scope, SyncAdapter, SyncListener, SyncPatternAdapter, SyncPatternListener,
    TableChange, TableEntry, Terminal, Transcoder, Value, RESERVED_PREFIX,
};
use crate::fiber::{FiberInner, PluginId};
use crate::key::{EventKey, EventPattern, InstanceId};
use crate::{BoxFuture, Disposer, Effect};

/// A dispatch before its listener snapshot is selected. Observers see it
/// synchronously and cannot veto it.
#[non_exhaustive]
pub struct DispatchAttempt<'a> {
    pub name: &'a str,
    pub mode: Mode,
    pub emitter: PluginId,
    pub emitter_instance: InstanceId,
    pub event: &'a (dyn std::any::Any + Send + Sync),
}

type ObserverCall = dyn for<'a> Fn(&DispatchAttempt<'a>) + Send + Sync;

struct DispatchObserver {
    call: Arc<ObserverCall>,
    owner: Weak<FiberInner>,
}

type Watcher = Arc<dyn Fn(&TableChange) + Send + Sync>;

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
    observers: Vec<Arc<DispatchObserver>>,
    watchers: Vec<(u64, Watcher)>,
    next_watcher: u64,
    tails: HashMap<Arc<str>, Tail>,
    /// Accepted emits whose calls have not all finished, by name.
    backlog: HashMap<Arc<str>, BTreeMap<u64, Instant>>,
    next_emit: u64,
}

struct BusCore {
    state: Mutex<BusState>,
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

impl Drop for EmitTicket {
    fn drop(&mut self) {
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
    pub scope: Option<Vec<Scope>>,
    pub filter: Option<FilterSpec>,
    pub origin: Option<u64>,
    pub framework: bool,
}

thread_local! {
    static ACTIVE_SYNC: RefCell<Vec<(usize, Arc<str>)>> = const { RefCell::new(Vec::new()) };
}

struct ReentryGuard;

impl ReentryGuard {
    fn enter(bus: &EventBus, name: &Arc<str>) -> Result<Self, CordisError> {
        let identity = Arc::as_ptr(&bus.core) as usize;
        ACTIVE_SYNC.with(|active| {
            let mut active = active.borrow_mut();
            if active.iter().any(|(bus, n)| *bus == identity && n == name) {
                return Err(CordisError::ReentrantEvent {
                    name: name.to_string(),
                });
            }
            active.push((identity, name.clone()));
            Ok(Self)
        })
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        ACTIVE_SYNC.with(|active| {
            active.borrow_mut().pop();
        });
    }
}

struct TypedTerminal<E, T>(Mutex<Option<T>>, PhantomData<fn(E) -> E>);

impl<E: Event, T: Terminal<E>> ErasedTerminal for TypedTerminal<E, T> {
    fn call<'a>(&'a self, inv: &'a Inv<'a>) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        let terminal = self.0.lock().unwrap().take();
        Box::pin(async move {
            let Some(terminal) = terminal else {
                return Ok(None);
            };
            let event = crate::events::typed::<E>(inv)?;
            let out = crate::events::CatchUnwind::new(terminal.call(inv.emitter, &event))
                .await
                .unwrap_or_else(|panic| Err(panic_error(panic)))?;
            Ok(out.map(Value::new))
        })
    }
}

struct TypedSyncTerminal<E, F>(RefCell<Option<F>>, PhantomData<fn(E) -> E>);

impl<E, F> ErasedSyncTerminal for TypedSyncTerminal<E, F>
where
    E: Event,
    F: FnOnce(&Ctx, &E) -> Result<Option<E::Value>, CordisError>,
{
    fn call(&self, inv: &Inv<'_>) -> Result<Option<Value>, CordisError> {
        let Some(terminal) = self.0.borrow_mut().take() else {
            return Ok(None);
        };
        let event = crate::events::typed::<E>(inv)?;
        Ok(terminal(inv.emitter, &event)?.map(Value::new))
    }
}

fn typed_value<V: serde::Serialize + serde::de::DeserializeOwned + Send + 'static>(
    bus: &EventBus,
    value: Option<Value>,
) -> Result<Option<V>, CordisError> {
    value
        .map(|value| value.into_typed::<V>(&*bus.transcoder()))
        .transpose()
}

impl EventBus {
    pub(crate) fn new() -> Self {
        Self {
            core: Arc::new(BusCore {
                state: Mutex::new(BusState::default()),
                transcoder: RwLock::new(Arc::new(JsonTranscoder)),
                next_dispatch: AtomicU64::new(1),
            }),
        }
    }

    fn ensure_bus(&self, ctx: &Ctx) -> Result<(), CordisError> {
        if Arc::ptr_eq(&self.core, &ctx.events().core) {
            Ok(())
        } else {
            Err(CordisError::Validation {
                issues: vec!["the context belongs to another event bus".into()],
            })
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

    /// Change the table under its lock; the watchers hear the changes after.
    fn change<R>(&self, f: impl FnOnce(&mut Table) -> (R, Vec<TableChange>)) -> R {
        let (out, changes, watchers) = {
            let mut state = self.core.state.lock().unwrap();
            let (out, changes) = f(&mut state.table);
            let watchers: Vec<Watcher> = if changes.is_empty() {
                Vec::new()
            } else {
                state.watchers.iter().map(|(_, w)| w.clone()).collect()
            };
            (out, changes, watchers)
        };
        notify(&watchers, &changes);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn register(
        &self,
        ctx: &Ctx,
        name: Arc<str>,
        prefix: bool,
        body: Body,
        opts: EventOptions,
        remote: Option<(ListenerInfo, u64)>,
        label: &str,
    ) -> Result<Disposer, CordisError> {
        ctx.registration_preflight()?;
        if !Arc::ptr_eq(&self.core, &ctx.events().core) {
            return Err(CordisError::Validation {
                issues: vec!["listener belongs to another event bus".into()],
            });
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
        let mut added: Option<(Vec<Watcher>, TableChange)> = None;
        let pending = &mut added;
        let disposer = ctx.register_internal_effect_named(label, move |fiber, generation, _| {
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
                calls: AtomicU64::new(0),
            });
            state.table.insert(entry.clone());
            *pending = Some((
                state.watchers.iter().map(|(_, w)| w.clone()).collect(),
                TableChange::Added(entry.table_entry()),
            ));
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
        })?;
        // The entry is in the table; its watchers hear it before `on` returns.
        if let Some((watchers, change)) = added {
            notify(&watchers, std::slice::from_ref(&change));
        }
        Ok(disposer)
    }

    /// Register an async listener of `key`'s name.
    pub fn on<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        listener: impl Listener<E>,
    ) -> Result<Disposer, CordisError> {
        self.on_opt(ctx, key, listener, EventOptions::default())
    }

    pub fn on_opt<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        listener: impl Listener<E>,
        opts: EventOptions,
    ) -> Result<Disposer, CordisError> {
        let body = Body::Async(Arc::new(AsyncAdapter(listener, PhantomData)));
        self.register(
            ctx,
            key.name_arc(),
            false,
            body,
            opts,
            None,
            "event listener",
        )
    }

    /// Register an async listener called at most once.
    pub fn once<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        listener: impl Listener<E>,
    ) -> Result<Disposer, CordisError> {
        self.on_opt(ctx, key, listener, EventOptions::default().once(true))
    }

    /// Register a synchronous listener: every mode can call it.
    pub fn on_sync<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        listener: impl SyncListener<E>,
    ) -> Result<Disposer, CordisError> {
        self.on_sync_opt(ctx, key, listener, EventOptions::default())
    }

    pub fn on_sync_opt<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        listener: impl SyncListener<E>,
        opts: EventOptions,
    ) -> Result<Disposer, CordisError> {
        let body = Body::Sync(Arc::new(SyncAdapter(listener, PhantomData)));
        self.register(
            ctx,
            key.name_arc(),
            false,
            body,
            opts,
            None,
            "sync event listener",
        )
    }

    /// Register an async listener of every name starting with the prefix.
    pub fn on_pattern<E: Event>(
        &self,
        ctx: &Ctx,
        pattern: &EventPattern<E>,
        listener: impl PatternListener<E>,
    ) -> Result<Disposer, CordisError> {
        self.on_pattern_opt(ctx, pattern, listener, EventOptions::default())
    }

    pub fn on_pattern_opt<E: Event>(
        &self,
        ctx: &Ctx,
        pattern: &EventPattern<E>,
        listener: impl PatternListener<E>,
        opts: EventOptions,
    ) -> Result<Disposer, CordisError> {
        let body = Body::Async(Arc::new(AsyncPatternAdapter(listener, PhantomData)));
        self.register(
            ctx,
            pattern.prefix_arc(),
            true,
            body,
            opts,
            None,
            "event pattern",
        )
    }

    pub fn on_sync_pattern<E: Event>(
        &self,
        ctx: &Ctx,
        pattern: &EventPattern<E>,
        listener: impl SyncPatternListener<E>,
    ) -> Result<Disposer, CordisError> {
        self.on_sync_pattern_opt(ctx, pattern, listener, EventOptions::default())
    }

    pub fn on_sync_pattern_opt<E: Event>(
        &self,
        ctx: &Ctx,
        pattern: &EventPattern<E>,
        listener: impl SyncPatternListener<E>,
        opts: EventOptions,
    ) -> Result<Disposer, CordisError> {
        let body = Body::Sync(Arc::new(SyncPatternAdapter(listener, PhantomData)));
        self.register(
            ctx,
            pattern.prefix_arc(),
            true,
            body,
            opts,
            None,
            "sync event pattern",
        )
    }

    /// Register a remote entry: its calls go to `listener` (protocol spec
    /// §4). Owned by `ctx`'s fiber like any listener; its disposer
    /// withdraws it.
    pub fn on_remote(
        &self,
        ctx: &Ctx,
        entry: RemoteEntry,
        listener: Arc<dyn RemoteListener>,
    ) -> Result<Disposer, CordisError> {
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
    ) -> Result<(Vec<TableEntry>, Disposer), CordisError> {
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

    #[cfg(test)]
    pub(crate) fn observer_count(&self) -> usize {
        self.core.state.lock().unwrap().observers.len()
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

    /// Observe each dispatch before its listeners are selected. Observers
    /// are trusted code: they see the event. Only dispatches from the
    /// owner's subtree are observed.
    pub fn observe_dispatch(
        &self,
        owner: &Ctx,
        observer: impl for<'a> Fn(&DispatchAttempt<'a>) + Send + Sync + 'static,
    ) -> Result<Disposer, CordisError> {
        owner.registration_preflight()?;
        if !Arc::ptr_eq(&self.core, &owner.events().core) {
            return Err(CordisError::Validation {
                issues: vec!["dispatch observer belongs to another event bus".into()],
            });
        }
        let hook = Arc::new(DispatchObserver {
            call: Arc::new(observer),
            owner: owner.weak_fiber(),
        });
        let bus = self.clone();
        owner.register_internal_effect_named("dispatch observer".into(), move |_, _, _| {
            bus.core.state.lock().unwrap().observers.push(hook.clone());
            Ok(Effect::AsyncDisposer(Box::new(move || {
                {
                    let mut state = bus.core.state.lock().unwrap();
                    state.observers.retain(|entry| !Arc::ptr_eq(entry, &hook));
                    if state.observers.capacity() > 64
                        && state.observers.len() * 4 < state.observers.capacity()
                    {
                        state.observers.shrink_to_fit();
                    }
                }
                Box::pin(async move {
                    // A call that selected it before the removal finishes first.
                    if let Some(owner) = hook.owner.upgrade() {
                        owner.wait_events().await;
                    }
                    Ok(())
                })
            })))
        })
    }

    fn observe(
        &self,
        ctx: &Ctx,
        name: &str,
        mode: Mode,
        event: &(dyn std::any::Any + Send + Sync),
    ) {
        let observers: Vec<Arc<DispatchObserver>> = {
            let state = self.core.state.lock().unwrap();
            if state.observers.is_empty() {
                return;
            }
            state.observers.clone()
        };
        if ctx.dispatch_preflight().is_err() {
            return;
        }
        let mut ancestry = Vec::new();
        let mut current = ctx.weak_fiber().upgrade();
        while let Some(fiber) = current {
            current = fiber.parent_fiber.as_ref().and_then(Weak::upgrade);
            ancestry.push(fiber);
        }
        let attempt = DispatchAttempt {
            name,
            mode,
            emitter: ctx.plugin_id(),
            emitter_instance: ctx.instance(),
            event,
        };
        let sink = ctx.error_sink();
        for observer in observers {
            let Some(owner) = observer.owner.upgrade() else {
                continue;
            };
            // Counted first: the owner's unload, and the observer's own
            // disposal, wait for this call.
            let _flight = table::CallFlight::begin(owner.clone());
            let registered = self
                .core
                .state
                .lock()
                .unwrap()
                .observers
                .iter()
                .any(|entry| Arc::ptr_eq(entry, &observer));
            if !registered
                || !owner.alive.load(Ordering::SeqCst)
                || owner.closing.load(Ordering::SeqCst)
                || !ancestry.iter().any(|fiber| Arc::ptr_eq(fiber, &owner))
            {
                continue;
            }
            if let Err(panic) = catch_unwind(AssertUnwindSafe(|| (observer.call)(&attempt))) {
                let error = Arc::new(panic_error(panic));
                let _ = catch_unwind(AssertUnwindSafe(|| sink(error)));
            }
        }
    }

    // ── dispatch ─────────────────────────────────────────────────

    /// §5.1 step 1, after the reserved-name check and the observers.
    fn snapshot(
        &self,
        ctx: &Ctx,
        name: &str,
        mode: Mode,
        event: &(dyn std::any::Any + Send + Sync),
        opts: &mut Opts,
    ) -> Result<Vec<Arc<Entry>>, CordisError> {
        self.ensure_bus(ctx)?;
        if !opts.framework && name.starts_with(RESERVED_PREFIX) {
            return Err(CordisError::ReservedEvent {
                name: name.to_owned(),
            });
        }
        self.observe(ctx, name, mode, event);
        let scope = opts.scope.take().unwrap_or_else(|| ctx.scope());
        Ok(self.change(|table| {
            let mut claimed = Vec::new();
            let entries = table.select(name, &scope, &mut claimed);
            let changes = claimed
                .iter()
                .map(|entry| TableChange::Removed(entry.table_entry()))
                .collect();
            (entries, changes)
        }))
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
    ) -> Result<tokio::sync::oneshot::Receiver<()>, CordisError> {
        let (started, receiver) = tokio::sync::oneshot::channel();
        let mut entries = self.snapshot(ctx, &name, Mode::Emit, &**payload.arc(), &mut opts)?;
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
                let Some(flight) = entry.start() else {
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
        state.tails.insert(name, Tail { generation, task });
        Ok(receiver)
    }

    pub(crate) fn parallel_erased<'a>(
        &self,
        ctx: &'a Ctx,
        name: Arc<str>,
        payload: Payload,
        mut opts: Opts,
    ) -> BoxFuture<'a, Result<(), CordisError>> {
        let bus = self.clone();
        Box::pin(async move {
            let entries = bus.snapshot(ctx, &name, Mode::Parallel, &**payload.arc(), &mut opts)?;
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
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        let bus = self.clone();
        Box::pin(async move {
            let entries = bus.snapshot(ctx, &name, Mode::Serial, &**payload.arc(), &mut opts)?;
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
        event: PayloadRef<'a>,
        terminal: Box<dyn ErasedTerminal + Send + 'a>,
        mut opts: Opts,
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        let bus = self.clone();
        Box::pin(async move {
            let entries = bus.snapshot(ctx, &name, Mode::Waterfall, event.as_any(), &mut opts)?;
            let entries = filter_any(&name, entries, opts.filter.take()).await?;
            let inv = Inv {
                bus: &bus,
                emitter: ctx,
                name: &name,
                mode: Mode::Waterfall,
                event,
                dispatch: bus.dispatch_id(),
                origin: opts.origin,
            };
            AsyncNext {
                inv: &inv,
                chain: &entries,
                terminal: &*terminal,
            }
            .invoke()
            .await
        })
    }

    pub(crate) fn bail_erased(
        &self,
        ctx: &Ctx,
        name: Arc<str>,
        event: PayloadRef<'_>,
        mut opts: Opts,
    ) -> Result<Option<Value>, CordisError> {
        ctx.registration_preflight()?;
        let _reentry = ReentryGuard::enter(self, &name)?;
        let entries = self.snapshot(ctx, &name, Mode::Bail, event.as_any(), &mut opts)?;
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
    ) -> Result<Option<Value>, CordisError> {
        ctx.registration_preflight()?;
        let _reentry = ReentryGuard::enter(self, &name)?;
        let entries = self.snapshot(ctx, &name, Mode::WaterfallSync, event.as_any(), &mut opts)?;
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

    /// `emit`: returns at once. Every listener's call starts in
    /// registration order; the calls of one name's emits start in emit
    /// order. A listener's failure goes to the error sink
    /// (`ListenerFailed`); it never stops the other listeners. `Err` only
    /// for a rejected emit (`ReservedEvent`, another bus).
    pub fn emit<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> Result<(), CordisError> {
        self.emit_erased(ctx, key.name_arc(), Payload::new(event), Opts::default())
            .map(drop)
    }

    /// `emit` of a framework event: only rutis emits `rutis/*`.
    pub fn emit_framework<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> Result<(), CordisError> {
        let opts = Opts {
            framework: true,
            ..Opts::default()
        };
        self.emit_erased(ctx, key.name_arc(), Payload::new(event), opts)
            .map(drop)
    }

    /// `parallel`: starts every call in registration order and waits for
    /// all. Failures: `ListenersFailed` (AggregateError), in registration
    /// order, also for one. Dropping the future cancels the calls.
    pub fn parallel<'a, E: Event>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> BoxFuture<'a, Result<(), CordisError>> {
        self.parallel_erased(ctx, key.name_arc(), Payload::new(event), Opts::default())
    }

    /// `serial`: one call at a time; the first result that is not "no
    /// result" (`None`, `Some(false)`) ends it. An error stops it.
    pub fn serial<'a, E: Event>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        self.serial_with(ctx, key, event, Opts::default())
    }

    fn serial_with<'a, E: Event>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
        opts: Opts,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        let bus = self.clone();
        let run = self.serial_erased(ctx, key.name_arc(), Payload::new(event), opts);
        Box::pin(async move { typed_value(&bus, run.await?) })
    }

    /// `waterfall`: the first listener is called with `next`; each `next`
    /// calls the following listener, the last one's calls `terminal` (`()`:
    /// none, `next` gives `None`). Returns the outermost listener's value as
    /// it is.
    pub fn waterfall<'a, E: Event, T: Terminal<E> + 'a>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: &'a E,
        terminal: T,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        self.waterfall_with(ctx, key, event, terminal, Opts::default())
    }

    fn waterfall_with<'a, E: Event, T: Terminal<E> + 'a>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: &'a E,
        terminal: T,
        opts: Opts,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        let bus = self.clone();
        let terminal = Box::new(TypedTerminal::<E, T>(
            Mutex::new(Some(terminal)),
            PhantomData,
        ));
        let run = self.waterfall_erased(
            ctx,
            key.name_arc(),
            PayloadRef::borrowed(event),
            terminal,
            opts,
        );
        Box::pin(async move { typed_value(&bus, run.await?) })
    }

    /// `bail`, synchronously: as `serial`, inline. An async listener is an
    /// `AsyncListener` error.
    pub fn bail_sync<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: &E,
    ) -> Result<Option<E::Value>, CordisError> {
        self.bail_with(ctx, key, event, Opts::default())
    }

    fn bail_with<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: &E,
        opts: Opts,
    ) -> Result<Option<E::Value>, CordisError> {
        let out = self.bail_erased(ctx, key.name_arc(), PayloadRef::borrowed(event), opts)?;
        typed_value(self, out)
    }

    /// `waterfall`, synchronously. The terminal can borrow the caller's
    /// stack; it needs neither `Send` nor `'static`.
    pub fn waterfall_sync<E: Event, F>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: &E,
        terminal: F,
    ) -> Result<Option<E::Value>, CordisError>
    where
        F: FnOnce(&Ctx, &E) -> Result<Option<E::Value>, CordisError>,
    {
        self.waterfall_sync_with(ctx, key, event, terminal, Opts::default())
    }

    fn waterfall_sync_with<E: Event, F>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: &E,
        terminal: F,
        opts: Opts,
    ) -> Result<Option<E::Value>, CordisError>
    where
        F: FnOnce(&Ctx, &E) -> Result<Option<E::Value>, CordisError>,
    {
        let terminal = TypedSyncTerminal::<E, F>(RefCell::new(Some(terminal)), PhantomData);
        let out = self.waterfall_sync_erased(
            ctx,
            key.name_arc(),
            PayloadRef::borrowed(event),
            &terminal,
            opts,
        )?;
        typed_value(self, out)
    }

    /// The same six emitting methods, filtered (§6): `keep` sees each
    /// non-global candidate's descriptor, in this process, before any call.
    pub fn filter(
        &self,
        keep: impl Fn(&ListenerInfo) -> bool + Send + Sync + 'static,
    ) -> Filtered<'_> {
        Filtered {
            bus: self,
            keep: Arc::new(keep),
        }
    }

    // ── raw dispatch ─────────────────────────────────────────────

    fn raw_opts(d: &mut Dispatch) -> Opts {
        Opts {
            scope: d.scope.take(),
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
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
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
                    bus.waterfall_erased(ctx, name, payload.as_ref(), terminal, opts)
                        .await
                })
            }
            Mode::Bail | Mode::WaterfallSync => {
                Box::pin(std::future::ready(Err(CordisError::Validation {
                    issues: vec![format!("{} is synchronous: use dispatch_sync", d.mode)],
                })))
            }
        }
    }

    /// Dispatch an erased event synchronously (`bail`, `waterfall-sync`).
    pub fn dispatch_sync(&self, ctx: &Ctx, mut d: Dispatch) -> Result<Option<Value>, CordisError> {
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
            mode => Err(CordisError::Validation {
                issues: vec![format!("{mode} is asynchronous: use dispatch")],
            }),
        }
    }

    fn live_entry(&self, id: u64, name: &str) -> Result<Arc<Entry>, CordisError> {
        let entry = self.core.state.lock().unwrap().table.get(id);
        entry
            .filter(|e| e.live())
            .ok_or_else(|| CordisError::ListenerWithdrawn {
                id,
                name: name.to_owned(),
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
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        let bus = self.clone();
        Box::pin(async move {
            let entry = bus.live_entry(id, &call.name)?;
            let Some(flight) = entry.start() else {
                return Err(CordisError::ListenerWithdrawn {
                    id,
                    name: call.name,
                });
            };
            let inv = Inv {
                bus: &bus,
                emitter: ctx,
                name: &call.name,
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
                    let next = AsyncNext {
                        inv: &inv,
                        chain: &[],
                        terminal: &*terminal,
                    };
                    run::call_async(&entry, &inv, Some(next)).await
                }
                Mode::Bail | Mode::WaterfallSync => Err(CordisError::Validation {
                    issues: vec![format!(
                        "{} is synchronous: use invoke_entry_sync",
                        call.mode
                    )],
                }),
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
    ) -> Result<Option<Value>, CordisError> {
        let entry = self.live_entry(id, &call.name)?;
        let Some(flight) = entry.start() else {
            return Err(CordisError::ListenerWithdrawn {
                id,
                name: call.name,
            });
        };
        let inv = Inv {
            bus: self,
            emitter: ctx,
            name: &call.name,
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
            mode => Err(CordisError::Validation {
                issues: vec![format!("{mode} is asynchronous: use invoke_entry")],
            }),
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

fn report_filter(ctx: &Ctx, name: &str, error: CordisError) {
    let emitter = match ctx.row() {
        Some(row) => format!("row {row}"),
        None => format!("plugin {}", ctx.plugin_name()),
    };
    let sink = ctx.error_sink();
    let report = CordisError::FilterFailed {
        name: name.to_owned(),
        emitter,
        error: Arc::new(error),
    };
    let _ = catch_unwind(AssertUnwindSafe(|| sink(Arc::new(report))));
}

/// The emitting methods of [`EventBus`], with a filter (§6).
pub struct Filtered<'b> {
    bus: &'b EventBus,
    keep: Arc<dyn Fn(&ListenerInfo) -> bool + Send + Sync>,
}

impl Filtered<'_> {
    fn opts(&self) -> Opts {
        Opts {
            filter: Some(FilterSpec::Each(self.keep.clone())),
            ..Opts::default()
        }
    }

    pub fn emit<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> Result<(), CordisError> {
        self.bus
            .emit_erased(ctx, key.name_arc(), Payload::new(event), self.opts())
            .map(drop)
    }

    pub fn parallel<'a, E: Event>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> BoxFuture<'a, Result<(), CordisError>> {
        self.bus
            .parallel_erased(ctx, key.name_arc(), Payload::new(event), self.opts())
    }

    pub fn serial<'a, E: Event>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: Arc<E>,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        self.bus.serial_with(ctx, key, event, self.opts())
    }

    pub fn waterfall<'a, E: Event, T: Terminal<E> + 'a>(
        &self,
        ctx: &'a Ctx,
        key: &EventKey<E>,
        event: &'a E,
        terminal: T,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        self.bus
            .waterfall_with(ctx, key, event, terminal, self.opts())
    }

    pub fn bail_sync<E: Event>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: &E,
    ) -> Result<Option<E::Value>, CordisError> {
        self.bus.bail_with(ctx, key, event, self.opts())
    }

    pub fn waterfall_sync<E: Event, F>(
        &self,
        ctx: &Ctx,
        key: &EventKey<E>,
        event: &E,
        terminal: F,
    ) -> Result<Option<E::Value>, CordisError>
    where
        F: FnOnce(&Ctx, &E) -> Result<Option<E::Value>, CordisError>,
    {
        self.bus
            .waterfall_sync_with(ctx, key, event, terminal, self.opts())
    }
}

#[cfg(test)]
mod transient_tests;
