use crate::host::CtxHost;
use crate::internal::CtxInternal;
use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::ctx::{Ctx, Shared};
use crate::diagnostics::ServiceAccess;
use crate::effect::{Effect, EffectMeta, EffectRecord};
use crate::error::{aggregate_arcs, panic_error, Error, ErrorKind, ServiceReadFailure};
use crate::events::{CatchUnwind, Event};
use crate::key::{InstanceId, ScopeId, TypeKey};
use crate::{BoxFuture, DynPlugin, PluginFactory};

/// The states of a fiber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FiberState {
    /// Waiting for its declared dependencies.
    Pending,
    /// `apply` is running.
    Loading,
    /// Loaded and providing.
    Active,
    /// Its last load failed (`build`, `validate` or `apply`), or its driver
    /// panicked.
    Failed,
    /// Removed; cannot be restarted, except the root.
    Disposed,
    /// Its cleanups are running.
    Unloading,
}

/// A fiber's state at one moment, as published on its watch channel.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub generation: u64,
    pub state: FiberState,
    pub error: Option<Arc<ErrorKind>>,
}

/// A plugin's identity: the id returned when it is registered.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct PluginId(pub u64);

/// The result of waiting for an unload. A timeout ends only this wait: the
/// driver keeps running the cleanup, and calling `dispose()` again joins the
/// same task. Tokio cannot interrupt synchronous code that does not yield,
/// and the deadline cannot fire on time on the runtime thread it blocks.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DisposeWaitError {
    /// The time ran out. `plugin_id`, `generation` and `state` are those
    /// of the plugin waited for (the root, for a shutdown); `stuck` says
    /// which plugins below it are still busy, and with what (#678).
    #[error("{}", timed_out(*.elapsed, .stuck, *.state))]
    TimedOut {
        plugin_id: PluginId,
        generation: u64,
        state: FiberState,
        elapsed: Duration,
        stuck: Vec<Stuck>,
    },
    #[error("fiber disposal failed: {0}")]
    Failed(Arc<ErrorKind>),
}

/// A plugin that a shutdown or dispose that ran out of time still waits
/// for, and what it is doing ([`DisposeWaitError::TimedOut`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stuck {
    /// Its row's name (`Ctx::mark_row`), else its plugin's.
    pub plugin: String,
    /// What it is still doing: `still running the cleanup registered at
    /// src/lib.rs:12:9` (a `ctx.effect`), `still withdrawing calendar`,
    /// `still running the cleanup its apply returned`, `its apply has not
    /// returned`.
    pub doing: String,
}

impl std::fmt::Display for Stuck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.plugin, self.doing)
    }
}

fn timed_out(elapsed: Duration, stuck: &[Stuck], state: FiberState) -> String {
    // Whole milliseconds: `3s`, `20ms`.
    let elapsed = Duration::from_millis(elapsed.as_millis() as u64);
    if stuck.is_empty() {
        return format!("not finished after {elapsed:?}: still {state:?}");
    }
    let each: Vec<String> = stuck.iter().map(ToString::to_string).collect();
    format!("not finished after {elapsed:?}: {}", each.join("; "))
}

/// What the plugins at and below `top` are still busy with, for a
/// shutdown or dispose that ran out of time: each plugin whose own work is
/// unfinished (a child plugin's unload is left to the child), in tree
/// order.
///
/// The root is not a plugin: only a cleanup the host registered on it with
/// `ctx.effect` is listed, as `the host`. rutis's own plugins (a link, a
/// runtime) are listed only when no plugin of the developer's is stuck:
/// they are then the only lead there is.
pub(crate) fn stuck_below(top: &Arc<FiberInner>) -> Vec<Stuck> {
    use crate::effect::Origin;
    let name = waiting::key_names(top.ctx.shared());
    // (rutis's own, what) in tree order.
    let mut found: Vec<(bool, Stuck)> = Vec::new();
    let mut pending = vec![top.clone()];
    while let Some(fiber) = pending.pop() {
        let mut children: Vec<Arc<FiberInner>> = fiber
            .children
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        children.reverse();
        pending.extend(children);
        let root = fiber.is_root;
        let plugin = if root {
            "the host".to_owned()
        } else {
            fiber.display_name()
        };
        let own_platform = fiber.platform && !root;
        let mut push = |doing: String| {
            found.push((
                own_platform,
                Stuck {
                    plugin: plugin.clone(),
                    doing,
                },
            ))
        };
        match fiber.state_snapshot().state {
            FiberState::Loading if !root => push("its apply has not returned".into()),
            FiberState::Unloading => {
                let records: Vec<Arc<EffectRecord>> = fiber
                    .effect_index
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(Weak::upgrade)
                    .collect();
                let mut busy = false;
                for record in &records {
                    let Some((origin, label)) = record.draining() else {
                        continue;
                    };
                    busy = true;
                    let doing = match origin {
                        Origin::Mount => continue,
                        Origin::Effect(at) => format!(
                            "still running the cleanup registered at {}:{}:{}",
                            at.file(),
                            at.line(),
                            at.column()
                        ),
                        // The host's other cleanups wait for plugins,
                        // which are named themselves.
                        _ if root => continue,
                        Origin::Provide(key) => format!("still withdrawing {}", name(key)),
                        Origin::Apply => "still running the cleanup its apply returned".into(),
                        Origin::Other => format!("still running the cleanup {label}"),
                    };
                    push(doing);
                }
                // Before its cleanups run, an unload waits for the event
                // calls still running and for the copies of the plugin
                // definitions it made to end.
                if !busy && !root {
                    push(
                        "still waiting for its event calls, or the copies of its plugin \
                         definitions, to end"
                            .into(),
                    );
                }
            }
            _ => {}
        }
    }
    let developers = found.iter().any(|(platform, _)| !platform);
    found
        .into_iter()
        .filter(|(platform, _)| !developers || !platform)
        .map(|(_, stuck)| stuck)
        .collect()
}

/// A fiber state change. Events are queued in order under the transition
/// lock and emitted after it is released; `seq` gives the order in which
/// changes were made, not the order in which listeners finish.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct FiberStatusChanged {
    pub plugin_id: PluginId,
    pub seq: u64,
    pub generation: u64,
    pub from: FiberState,
    pub to: FiberState,
}

#[diagnostic::do_not_recommend]
impl Event for FiberStatusChanged {
    const NAME: &'static str = "rutis::FiberStatusChanged";
    type Value = ();
}

pub(crate) enum TaskDone {
    Running,
    Done(Option<Arc<ErrorKind>>),
}

/// A transition that runs exactly once: one `Arc` kept under the lock, which
/// every caller for the same transition joins. The result is cached as an
/// `Arc<ErrorKind>`, so every joiner gets the same `Arc`.
pub(crate) struct TransitionTask {
    pub done_tx: watch::Sender<TaskDone>,
    /// A receiver kept alive: with no receiver the watch channel counts as
    /// closed and `send` fails silently. If the consumer finished quickly (for
    /// example, skipped an unchanged reload) before a joiner subscribed, the
    /// result would be lost and the join would wait forever.
    _keepalive: watch::Receiver<TaskDone>,
}

impl TransitionTask {
    pub(crate) fn new() -> Arc<Self> {
        let (done_tx, done_rx) = watch::channel(TaskDone::Running);
        Arc::new(Self {
            done_tx,
            _keepalive: done_rx,
        })
    }

    pub(crate) fn complete(&self, err: Option<Arc<ErrorKind>>) {
        let _ = self.done_tx.send(TaskDone::Done(err));
    }
}

pub(crate) enum Intent {
    RefreshDeps,
    /// Re-check dependencies, then complete the task: a withdrawal waits on
    /// it for each consumer.
    RefreshDepsJoin(Arc<TransitionTask>),
    /// A barrier: when the FIFO mailbox reaches it, every intent queued
    /// before it has been handled, and the task completes with the state at
    /// that point. The mailbox order answers whether the fiber has settled.
    Settle(Arc<TransitionTask>),
    Restart(Arc<TransitionTask>),
    Dispose,
    Shutdown(Arc<TransitionTask>),
}

impl Intent {
    fn task(&self) -> Option<&Arc<TransitionTask>> {
        match self {
            Intent::RefreshDepsJoin(task)
            | Intent::Settle(task)
            | Intent::Restart(task)
            | Intent::Shutdown(task) => Some(task),
            _ => None,
        }
    }
}

/// [`FiberInner::post_join`] on a driver's `alive` flag and mailbox; apart
/// from the fiber so the shuttle model of the driver's exit
/// (`shuttle_tests.rs`, #179) runs this very code.
pub(crate) fn post_join_to(
    alive: &crate::sync::AtomicBool,
    intents: &mpsc::UnboundedSender<Intent>,
    task: Arc<TransitionTask>,
    make: impl FnOnce(Arc<TransitionTask>) -> Intent,
    stopped_error: impl Fn() -> Option<Arc<ErrorKind>>,
) -> bool {
    if !alive.load(Ordering::SeqCst) {
        task.complete(stopped_error());
        return false;
    }
    // The window the check after sending closes: the driver may stop and
    // drain here.
    crate::sync::switch_point();
    match intents.send(make(task.clone())) {
        Ok(()) => {
            if !alive.load(Ordering::SeqCst) {
                task.complete(stopped_error());
            }
            true
        }
        Err(_) => {
            task.complete(stopped_error());
            false
        }
    }
}

/// Complete the task an intent carries at once, so a join does not wait
/// forever when no driver will handle the intent.
fn complete_intent(intent: &Intent, err: Option<Arc<ErrorKind>>) {
    if let Some(task) = intent.task() {
        task.complete(err);
    }
}

enum NextState {
    Pending,
    Disposed,
}

pub(crate) struct Trans {
    pub generation: u64,
    pub state: FiberState,
    pub error: Option<Arc<ErrorKind>>,
    pub seq: u64,
    pub status_queue: Vec<FiberStatusChanged>,
    pub terminal_task: Option<Arc<TransitionTask>>,
    /// Restarts (and updates) called and not yet taken up by the driver.
    /// They cancel when called; a generation that starts before the
    /// driver reaches them starts cancelled (as a dispose stops it from
    /// starting), so its `apply` does not keep them waiting.
    pub pending_restarts: usize,
}

pub(crate) struct FiberInner {
    pub id: PluginId,
    pub instance: InstanceId,
    pub name: String,
    pub is_root: bool,
    /// The root, or a plugin of rutis itself ([`DynPlugin::platform`]).
    pub platform: bool,
    pub plugin: Option<Arc<dyn DynPlugin>>,
    /// Factory mode: set instead of `plugin` (`plugin` is `None` then).
    pub factory: Option<Arc<dyn ErasedFactory>>,
    /// The current config in factory mode, stored as an `Arc` for cheap
    /// snapshots; `update` and `set_config` replace it.
    pub config: Mutex<Option<Arc<dyn Any + Send + Sync>>>,
    pub ctx: Ctx,
    pub parent_fiber: Option<Weak<FiberInner>>,
    pub children: Mutex<Vec<Weak<FiberInner>>>,
    pub closing: Arc<AtomicBool>,
    pub event_flights: Mutex<usize>,
    pub event_flights_tx: watch::Sender<usize>,
    _event_flights_rx: watch::Receiver<usize>,
    pub shutdown_task: Mutex<Option<Arc<TransitionTask>>>,
    pub shutdown_inner: Mutex<Option<Arc<TransitionTask>>>,
    pub driver: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The cancellation token of the current generation: each load creates a
    /// new one, and an unload cancels it. Dispose, restart and withdrawal cancel
    /// it before queueing their intent, so a running `apply` can return through
    /// `ctx.cancelled()`: the driver is serial, and without this the intent
    /// would wait for `apply` forever.
    pub token: Mutex<CancellationToken>,
    pub transition: Mutex<Trans>,
    pub snapshot_tx: watch::Sender<Snapshot>,
    /// A receiver kept alive so the watch channel stays open (with no
    /// receiver, `send` fails silently); [`FiberView::watch`] clones it.
    pub snapshot_rx: watch::Receiver<Snapshot>,
    pub effects: Mutex<Vec<Arc<EffectRecord>>>,
    /// The plugin definitions this fiber's current generation made or
    /// handed over: their copies end before its cleanups run.
    pub definitions: Mutex<Vec<Weak<crate::definition::Inner>>>,
    /// Weak index keeps draining records visible after `drain_effects` takes
    /// the active list, without retaining cleanups or finished child fibers.
    pub effect_index: Mutex<Vec<Weak<EffectRecord>>>,
    /// Cleanup errors of effect records drained early. A drained record removes
    /// itself, and its error waits here for the fiber's next unload or restart
    /// to report it; a single error keeps its `Arc` identity.
    pub drained_errors: Mutex<Vec<Arc<ErrorKind>>>,
    /// This fiber's mount record on its parent: the effect that disposes this
    /// fiber with the parent. It is drained when this fiber reaches a terminal
    /// state, so the record removes itself and a long-lived parent keeps nothing.
    pub mount: Mutex<Option<Arc<EffectRecord>>>,
    /// The dependency declarations read once at spawn. Every dependency check
    /// uses them, and they unregister the fiber from `inject_index` when it
    /// ends, without calling the plugin's `injects()` again.
    pub declared_injects: Vec<TypeKey>,
    pub intents_tx: mpsc::UnboundedSender<Intent>,
    /// Whether the driver is running. Once it is false, `post` refuses intents
    /// and completes their tasks at once, so an intent queued after the driver
    /// exits does not leave a join waiting forever.
    pub alive: crate::sync::AtomicBool,
    /// The `(key, scope)` pairs this fiber provides.
    pub provided: Mutex<Vec<(TypeKey, Option<crate::key::ScopeId>)>>,
    /// The dependencies the last load resolved, including a load that failed:
    /// `(provider, provider generation, key, scope)`. A refresh that resolves
    /// the same set is skipped, so a Failed fiber retries only when its
    /// dependencies change. It is also how a withdrawal finds consumers: a
    /// consumer is a fiber whose `last_deps` contains the binding. The scope is
    /// part of each entry, so bindings of one key that a fiber provides in
    /// different scopes stay apart. A Failed fiber counts as a consumer too: the
    /// withdrawal re-checks it, and with a dependency missing it stays Failed.
    pub last_deps: Mutex<Option<HashSet<(PluginId, u64, TypeKey, Option<crate::key::ScopeId>)>>>,
    pub accesses: Mutex<Vec<ServiceAccess>>,
    /// The loader group this fiber is an instance of (`Ctx::mark_scope`).
    pub scope_group: std::sync::OnceLock<Arc<str>>,
    /// The row this fiber is (`Ctx::mark_row`).
    pub row: std::sync::OnceLock<Arc<str>>,
    /// The generation of the library whose code this fiber runs
    /// (`service::set_generation`); 0 when none was set.
    pub code_generation: std::sync::atomic::AtomicU64,
    /// The generation whose wait for missing services is watched
    /// (`waiting`): reported once per wait.
    pub waiting_watch: Mutex<Option<u64>>,
}

/// The type-erased interface of a generic `PluginFactory<C>`, used inside
/// the kernel.
pub(crate) trait ErasedFactory: Send + Sync + 'static {
    fn config_type_id(&self) -> TypeId;
    fn name(&self) -> &str;
    fn injects(&self) -> Vec<TypeKey>;
    fn validate_config_erased(&self, config: &dyn Any) -> Result<(), Error>;
    fn build_erased(&self, config: &dyn Any) -> Result<Box<dyn DynPlugin>, Error>;
}

/// Wraps a generic factory behind [`ErasedFactory`] (the same approach as
/// the registry's `StoredValue`).
struct FactoryAdapter<F, C> {
    factory: F,
    _marker: std::marker::PhantomData<fn() -> C>,
}

impl<F, C> ErasedFactory for FactoryAdapter<F, C>
where
    F: PluginFactory<C>,
    C: Send + Sync + 'static,
{
    fn config_type_id(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn name(&self) -> &str {
        self.factory.name()
    }

    fn injects(&self) -> Vec<TypeKey> {
        (self.factory.injects()).to_vec()
    }

    fn validate_config_erased(&self, config: &dyn Any) -> Result<(), Error> {
        config
            .downcast_ref::<C>()
            .ok_or_else(|| {
                Error::from(ErrorKind::Validation {
                    issues: vec!["config type mismatch".into()],
                })
            })
            .and_then(|c| self.factory.validate_config(c))
    }

    fn build_erased(&self, config: &dyn Any) -> Result<Box<dyn DynPlugin>, Error> {
        config
            .downcast_ref::<C>()
            .ok_or_else(|| {
                Error::from(ErrorKind::Validation {
                    issues: vec!["config type mismatch".into()],
                })
            })
            .and_then(|c| self.factory.build(c))
    }
}

impl FiberInner {
    /// The name a report gives it: its row (`Ctx::mark_row`), else its
    /// plugin's name.
    pub(crate) fn display_name(&self) -> String {
        match self.row.get() {
            Some(row) => row.to_string(),
            None => self.name.clone(),
        }
    }

    pub(crate) fn push_effect(&self, record: Arc<EffectRecord>) {
        self.effects.lock().unwrap().push(record.clone());
        self.effect_index
            .lock()
            .unwrap()
            .push(Arc::downgrade(&record));
    }

    /// The plugin instance for the current generation. A static plugin is
    /// cloned; a factory builds a new instance from the current config each
    /// generation, which is how a config update takes effect. `build` is user
    /// code: a panic in it becomes an error, which the caller passes to
    /// `fail_load` as it does for `validate`, so it does not kill the driver.
    fn current_plugin(&self) -> Result<Arc<dyn DynPlugin>, Error> {
        if let Some(plugin) = &self.plugin {
            return Ok(plugin.clone());
        }
        let factory = self.factory.as_ref().ok_or_else(|| {
            Error::from(ErrorKind::PluginFailed(
                "fiber has no plugin or factory".into(),
            ))
        })?;
        let config = self.config.lock().unwrap().clone().ok_or_else(|| {
            Error::from(ErrorKind::PluginFailed(
                "factory fiber has no config".into(),
            ))
        })?;
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            factory.build_erased(config.as_ref())
        }))
        .unwrap_or_else(|p| Err(panic_error(p)));
        built.map(Arc::from)
    }

    /// The current generation's token, which `ctx.cancellation_token()` and
    /// `ctx.cancelled()` expose to the plugin.
    pub(crate) fn current_token(&self) -> CancellationToken {
        self.token.lock().unwrap().clone()
    }

    pub(crate) fn cancel_current(&self) {
        self.token.lock().unwrap().cancel();
    }

    /// Cancel the current token of every live descendant, so callbacks that
    /// cooperate with their owner's cancellation can end before the drain.
    pub(crate) fn cancel_descendants(&self) {
        let mut pending: Vec<_> = self
            .children
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        while let Some(fiber) = pending.pop() {
            fiber.cancel_current();
            pending.extend(
                fiber
                    .children
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(Weak::upgrade),
            );
        }
    }

    pub(crate) fn begin_event(&self) {
        let mut count = self.event_flights.lock().unwrap();
        *count += 1;
        let _ = self.event_flights_tx.send(*count);
    }

    pub(crate) fn finish_event(&self) {
        let mut count = self.event_flights.lock().unwrap();
        *count -= 1;
        let _ = self.event_flights_tx.send(*count);
    }

    /// Wait for the event calls counted on this fiber (listener calls,
    /// observer calls) to finish, at most the shutdown deadline
    /// (`RUTIS_SHUTDOWN_TIMEOUT`, protocol spec §7.1): past it, the
    /// `listeners of … still running after <s>s; unloading anyway` error
    /// goes to the error sink and the unload goes on without them.
    pub(crate) async fn wait_events(&self) {
        let mut rx = self.event_flights_tx.subscribe();
        if *rx.borrow_and_update() == 0 {
            return;
        }
        let settled = async move {
            loop {
                if *rx.borrow_and_update() == 0 {
                    return;
                }
                if rx.changed().await.is_err() {
                    return;
                }
            }
        };
        let limit = shutdown_deadline();
        // A thread keeps the time, not the runtime's timer: the kernel runs
        // on whatever runtime it is given, timers enabled or not. It ends as
        // soon as the calls finish (`done` dropped).
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let (late, past) = tokio::sync::oneshot::channel::<()>();
        let timer = std::thread::Builder::new()
            .name("rutis-unload-deadline".into())
            .spawn(move || {
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    finished.recv_timeout(limit)
                {
                    let _ = late.send(());
                }
            });
        if timer.is_err() {
            settled.await;
            return;
        }
        tokio::select! {
            () = settled => drop(done),
            Ok(()) = past => {
                let listeners = match self.ctx.row() {
                    Some(row) => format!("row {row}"),
                    None => format!("plugin {}", self.ctx.plugin_name()),
                };
                let error = ErrorKind::ListenersStillRunning {
                    listeners,
                    seconds: limit.as_secs_f64(),
                };
                let sink = self.ctx.error_sink();
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    sink(Arc::new(error))
                }));
            }
        }
    }

    fn new_generation_token(&self) -> CancellationToken {
        let token = CancellationToken::new();
        *self.token.lock().unwrap() = token.clone();
        token
    }

    /// Post an intent. Returns false if the driver has stopped; a task the
    /// intent carries (a `Shutdown`'s) then completes at once with the fiber's
    /// error, or `Closed` while the instance is closing. Unlike `post_join`, it
    /// does not check `alive` again after sending.
    pub(crate) fn post(&self, intent: Intent) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            complete_intent(&intent, self.stopped_error());
            return false;
        }
        match self.intents_tx.send(intent) {
            Ok(()) => true,
            Err(err) => {
                complete_intent(&err.0, self.stopped_error());
                false
            }
        }
    }

    fn stopped_error(&self) -> Option<Arc<ErrorKind>> {
        self.transition.lock().unwrap().error.clone().or_else(|| {
            self.ctx
                .shared()
                .closing
                .load(Ordering::SeqCst)
                .then(|| Arc::new(ErrorKind::Closed))
        })
    }

    /// Post an intent that carries a completion task. `alive` is checked again
    /// after sending: if the send landed after the driver drained its mailbox
    /// (so nothing will handle the intent), the task completes at once.
    /// Completing twice leaves no waiter behind. This closes the race "alive
    /// checked, driver exits and drains, send lands". The task completes with
    /// the fiber's error (or `Closed` while the instance is closing), not with
    /// `Ok`: a driver that stopped in Failed must not report success.
    pub(crate) fn post_join(
        &self,
        task: Arc<TransitionTask>,
        make: impl FnOnce(Arc<TransitionTask>) -> Intent,
    ) -> bool {
        post_join_to(&self.alive, &self.intents_tx, task, make, || {
            self.stopped_error()
        })
    }

    pub(crate) fn state(&self) -> FiberState {
        self.transition.lock().unwrap().state
    }

    /// Whether a dispose of this (non-root) fiber is registered: its
    /// queued Dispose ends the fiber, so nothing starts or reloads it.
    fn terminal_registered(&self) -> bool {
        !self.is_root && self.transition.lock().unwrap().terminal_task.is_some()
    }

    /// Unload for a registered dispose ahead of its queued Dispose: a
    /// withdrawal waiting for this fiber sees it stop now, and cleanup
    /// errors reach `dispose()` (the Dispose reports `tr.error`) instead
    /// of the error sink of an `unload(Pending)`.
    async fn unload_for_dispose(this: &Arc<Self>) {
        if this.state() != FiberState::Disposed {
            Self::unload(this, NextState::Disposed).await;
        }
    }

    pub(crate) fn sequenced_snapshot(&self) -> (u64, Snapshot) {
        let tr = self.transition.lock().unwrap();
        let snapshot = Snapshot {
            generation: tr.generation,
            state: tr.state,
            error: tr.error.clone(),
        };
        (tr.seq, snapshot)
    }

    pub(crate) fn state_snapshot(&self) -> Snapshot {
        let tr = self.transition.lock().unwrap();
        Snapshot {
            generation: tr.generation,
            state: tr.state,
            error: tr.error.clone(),
        }
    }

    /// Under the transition lock: change the state, queue the status event in
    /// order and publish the snapshot. The lock is never held across an await.
    fn set_state(&self, tr: &mut Trans, new: FiberState) {
        let old = tr.state;
        if old == new {
            return;
        }
        tr.state = new;
        tr.seq += 1;
        tr.status_queue.push(FiberStatusChanged {
            plugin_id: self.id,
            seq: tr.seq,
            generation: tr.generation,
            from: old,
            to: new,
        });
        let _ = self.snapshot_tx.send(Snapshot {
            generation: tr.generation,
            state: new,
            error: tr.error.clone(),
        });
    }

    /// Outside the lock: take the queued status events and emit them in order;
    /// an emit error goes to the error sink.
    fn flush_status(&self) {
        let queue: Vec<FiberStatusChanged> = {
            let mut tr = self.transition.lock().unwrap();
            std::mem::take(&mut tr.status_queue)
        };
        for event in queue {
            if let Err(error) = self.ctx.emit(event) {
                self.ctx.error_sink()(error.shared());
            }
        }
    }

    fn resolve_deps(
        &self,
    ) -> (
        HashSet<(PluginId, u64, TypeKey, Option<ScopeId>)>,
        Vec<TypeKey>,
    ) {
        let mut satisfied: HashSet<(PluginId, u64, TypeKey, Option<ScopeId>)> = HashSet::new();
        let mut missing: Vec<TypeKey> = Vec::new();
        // Reuse the declarations captured at registration. The index, gate,
        // and diagnostics must all observe the same keys.
        let registry = &self.ctx.shared().registry;
        for key in self.declared_injects.iter().cloned() {
            if self.ctx.check_instance(&key).is_err() {
                missing.push(key);
                continue;
            }
            let scope = self.ctx.scope_for(&key);
            match registry.resolve_dep(&key, scope.as_ref()) {
                Some((provider_id, provider_gen)) => {
                    satisfied.insert((provider_id, provider_gen, key, scope));
                }
                None => missing.push(key),
            }
        }
        (satisfied, missing)
    }

    /// Whether this generation's strict reads include a declared
    /// dependency found unavailable (removed, provider inactive, check
    /// rejected). The access record is cleared when a generation loads.
    fn read_a_lost_dependency(&self) -> bool {
        self.accesses.lock().unwrap().iter().any(|access| {
            access.strict
                && matches!(access.failure, Some(ServiceReadFailure::Unavailable(_)))
                && self.declared_injects.contains(&access.key)
        })
    }

    /// Whether `apply`'s error says a declared dependency was lost after
    /// the gate, with this generation's failed strict read as evidence:
    /// `InjectUnsatisfied`, or the strict read's own error returned as is.
    fn reports_a_lost_dependency(&self, error: &Error) -> bool {
        let claims = match &**error {
            ErrorKind::InjectUnsatisfied(_) => true,
            ErrorKind::ServiceRead(read) => {
                matches!(read.reason, ServiceReadFailure::Unavailable(_))
                    && self.declared_injects.contains(&read.key)
            }
            _ => false,
        };
        claims && self.read_a_lost_dependency()
    }

    async fn refresh_deps(this: &Arc<Self>) {
        if this.closing.load(Ordering::SeqCst) || this.ctx.shared().closing.load(Ordering::SeqCst) {
            return;
        }
        if this.plugin.is_none() && this.factory.is_none() {
            return;
        }
        if this.state() == FiberState::Disposed {
            return;
        }
        if this.terminal_registered() {
            Self::unload_for_dispose(this).await;
            return;
        }
        let (satisfied, missing) = this.resolve_deps();
        // 被取消的 Loading 代视同"已装载":排干后再定去留
        let loaded = !matches!(this.state(), FiberState::Pending | FiberState::Disposed);
        if !missing.is_empty() {
            // 依赖缺失:Active/Loading 卸载回 Pending;缺依赖长期 Pending
            // 不报错(D22)。**Failed 保持 Failed**(cordis FAILED 粘性:
            // epoch 已是 INACTIVE,`_setEpoch` 早退不迁移,错误持续可见
            // ——fiber.ts:611-639;审计 #2:此前被算作 loaded 降级 Pending,
            // 错误隐入 settle 通道)。依赖恢复走下方装载路径,Failed 照常
            // 重试(cordis 同:epoch 变化触发 reload)。
            if matches!(this.state(), FiberState::Active | FiberState::Loading) {
                Self::unload(this, NextState::Pending).await;
            }
            Self::watch_waiting(this);
            return;
        }
        // 等值合并:依赖三元组集未变则跳过(§〇 epoch 内容派生相等键)
        let unchanged = {
            let last = this.last_deps.lock().unwrap();
            last.as_ref() == Some(&satisfied)
        };
        if unchanged {
            return;
        }
        if loaded {
            Self::unload(this, NextState::Pending).await;
        }
        Self::load(this, satisfied).await;
    }

    /// Waiting for services nothing provides: once per wait, have it
    /// reported if it lasts ([`waiting`]).
    fn watch_waiting(this: &Arc<Self>) {
        if this.is_root || this.state() != FiberState::Pending {
            return;
        }
        let generation = this.transition.lock().unwrap().generation;
        {
            let mut watched = this.waiting_watch.lock().unwrap();
            if *watched == Some(generation) {
                return;
            }
            *watched = Some(generation);
        }
        waiting::watch(this, generation);
    }

    async fn load(this: &Arc<Self>, deps: HashSet<(PluginId, u64, TypeKey, Option<ScopeId>)>) {
        let shared = this.ctx.shared().clone();
        let _admission = shared.admission.lock().unwrap();
        if this.closing.load(Ordering::SeqCst) || shared.closing.load(Ordering::SeqCst) {
            return;
        }
        // The terminal check and the new token share the lock
        // `register_dispose` takes (#619): a dispose registered before this
        // point stops the generation from starting (its queued Dispose
        // runs next); one registered after it cancels the new token.
        let generation = {
            let mut tr = this.transition.lock().unwrap();
            if tr.terminal_task.is_some() {
                return;
            }
            let token = this.new_generation_token();
            if tr.pending_restarts > 0 {
                token.cancel();
            }
            tr.generation += 1;
            tr.error = None;
            Self::set_state(this, &mut tr, FiberState::Loading);
            tr.generation
        };
        this.accesses.lock().unwrap().clear();
        this.flush_status();
        drop(_admission);

        // 依赖快照(D21:装载窗口内即"绑定中",驱逐判定读它)。Recorded
        // before any plugin callback, after the new token, then the gate is
        // checked again (#620): a withdrawal marks the binding removed and
        // then reads `last_deps`, this side writes `last_deps` and then
        // resolves, so at least one sees the other. A withdrawal that finds
        // this fiber cancels the token of this generation.
        *this.last_deps.lock().unwrap() = Some(deps.clone());
        let (now, missing) = this.resolve_deps();
        if !missing.is_empty() || now != deps {
            // Not run: back to Pending, as if the first check had refused.
            // A binding replaced in between counts too: a later withdrawal
            // of the new one would not find this fiber in `last_deps`.
            // A withdrawal, a replacement or a provider leaving notifies
            // the key, so a refresh is queued (a `check` that refuses
            // waits for the next notification, as when the first check
            // refused); the unload clears `last_deps`, so that refresh is
            // not skipped as unchanged.
            Self::unload(this, NextState::Pending).await;
            this.flush_status();
            if !missing.is_empty() {
                Self::watch_waiting(this);
            }
            return;
        }

        // 当前代实例(D32):静态模式既有实例;工厂模式从当前 config 构造,
        // 构造失败按装载失败回滚(fail_load 保持装配原子性)。
        let plugin = match this.current_plugin() {
            Ok(p) => p,
            Err(e) => {
                Self::fail_load(this, e, deps).await;
                return;
            }
        };

        // validate-before-store(D12):validate 失败 → Failed;
        // validate 是用户回调,panic 同样转入 Failed(评审 #6:不杀驱动)
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.validate())) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                Self::fail_load(this, e, deps).await;
                return;
            }
            Err(p) => {
                Self::fail_load(this, panic_error(p), deps).await;
                return;
            }
        }

        // apply:直接等待退出(D7 第③步"等 apply 退出",不中止)。
        // 预取消(dispose/restart/驱逐)使观察 token 的插件经 ctx.cancelled()
        // 协作返回;不观察则 dispose 无限等待(协作取消限制,D27)。
        let ctx = this.ctx.for_generation(generation);
        // async block 把创建 Future 的同步回调也放进 unwind 边界。
        let outcome = CatchUnwind::new(async { plugin.apply(&ctx).await }).await;
        let result: Result<Effect, Error> = outcome.unwrap_or_else(|p| Err(panic_error(p)));
        match result {
            Ok(effect) => {
                let record = EffectRecord::from(
                    effect,
                    format!("plugin apply: {}", this.name),
                    crate::effect::Origin::Apply,
                    Arc::downgrade(this),
                );
                this.push_effect(record);
                if this.closing.load(Ordering::SeqCst) {
                    Self::unload(this, NextState::Disposed).await;
                    return;
                }
                // 惯性锁 2(fiber.spec:LOADING 期同 fiber 被重新 provide,in-flight
                // 加载直接完成进 ACTIVE):装载窗口内依赖集若已整体翻新且无缺失,
                // 就地采纳新三元组集——排队的重查意图将看到"未变化"而跳过,
                // 不触发换代重载。仍有缺失则不采纳,按原语义卸载(惯性锁 1)。
                let (fresh, missing) = this.resolve_deps();
                if missing.is_empty() {
                    *this.last_deps.lock().unwrap() = Some(fresh);
                }
                {
                    let mut tr = this.transition.lock().unwrap();
                    tr.error = None;
                    Self::set_state(this, &mut tr, FiberState::Active);
                }
                // 通知本 fiber 提供的服务键:后到消费者激活
                let provided: Vec<TypeKey> = this
                    .provided
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(k, _)| k.clone())
                    .collect();
                for key in provided {
                    shared.registry.notify_key_changed(&key);
                }
            }
            // apply 报告声明的依赖在门控之后、取用之前消失。证据是本代的
            // 访问记录:确有一次对声明依赖的严格读取因"不可用"而失败(被摘除、
            // 提供者失活、check 拒绝)——门控之后依赖确实失效过,哪怕此刻已
            // 恢复。这一代视同被驱逐,回滚后回到 Pending(不进粘性 Failed);
            // 已恢复则立即重查装载。没有这样的读取时不认这个理由,按普通
            // 失败处理,避免装载循环。apply 原样返回这次严格读取的错误
            // (`require(..)?`)与 `InjectUnsatisfied` 同样处理(#438)。
            Err(e) if this.reports_a_lost_dependency(&e) => {
                Self::unload(this, NextState::Pending).await;
                if this.resolve_deps().1.is_empty() {
                    this.post(Intent::RefreshDeps);
                }
            }
            Err(e) => Self::fail_load(this, e, deps).await,
        }
        this.flush_status();
    }

    /// Roll back a failed load: through Unloading, drain what `apply` had
    /// registered so far (listeners, services, child plugins), then enter
    /// Failed, so a failed start leaves nothing behind. Cleanup errors go to
    /// the error sink and are also aggregated into the fiber's error after the
    /// load error, so a joiner sees the whole failure.
    /// The failed generation records the dependencies it loaded with (`deps`):
    /// it is retried only when they change, so repeated notifications for the
    /// same set (such as one per key when a provider activates) do not reload
    /// it. Unless the load was being stopped or the plugin is rutis's own, a
    /// `StartFailed` error also goes to the error sink.
    async fn fail_load(
        this: &Arc<Self>,
        error: Error,
        deps: HashSet<(PluginId, u64, TypeKey, Option<ScopeId>)>,
    ) {
        {
            let _admission = this.ctx.shared().admission.lock().unwrap();
            let mut tr = this.transition.lock().unwrap();
            Self::set_state(this, &mut tr, FiberState::Unloading);
        }
        this.flush_status();
        // A load that failed because it was being stopped (a dispose,
        // restart or shutdown cancelled it) is no failed start to report.
        let stopping = this.token.lock().unwrap().is_cancelled()
            || this.closing.load(Ordering::SeqCst)
            || this.ctx.shared().closing.load(Ordering::SeqCst);
        this.cancel_current();

        this.wait_events().await;

        let cleanup_errors = Self::drain_effects(this).await;
        *this.last_deps.lock().unwrap() = Some(deps);
        let sink = this.ctx.error_sink();
        for e in &cleanup_errors {
            sink(e.clone());
        }
        let mut combined = vec![error.shared()];
        combined.extend(cleanup_errors);
        let arc = aggregate_arcs(combined).expect("failure present");
        {
            let mut tr = this.transition.lock().unwrap();
            tr.error = Some(arc.clone());
            Self::set_state(this, &mut tr, FiberState::Failed);
        }
        this.flush_status();
        // Seen without anyone reading the result (#678): an embedding's
        // default sink prints it. rutis's own plugins report their own.
        if !this.platform && !stopping {
            let error = ErrorKind::StartFailed {
                plugin: this.display_name(),
                error: arc,
            };
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(Arc::new(error))));
        }
    }

    /// Run this generation's cleanups: end the copies of the plugin
    /// definitions it made and wait for them, drain the effect records one at
    /// a time in reverse order, add the errors of records drained early, then
    /// clear `last_deps` and the provided table. Returns the cleanup errors.
    /// Cordis cleans up top-level effects concurrently (LIFO only within one
    /// effect); here they run one after another across effects too, so the
    /// completion order is fixed and errors aggregate predictably.
    async fn drain_effects(this: &Arc<Self>) -> Vec<Arc<ErrorKind>> {
        let handle = this.ctx.handle().clone();
        // The copies of the plugin definitions this generation made end
        // first, and are waited for: they may use what the cleanups below
        // close (design-plugin-values R6).
        let definitions = std::mem::take(&mut *this.definitions.lock().unwrap());
        let definitions = crate::definition::end_all(this, definitions).await;
        let effects: Vec<Arc<EffectRecord>> = std::mem::take(&mut *this.effects.lock().unwrap());
        let mut errors: Vec<Arc<ErrorKind>> = Vec::new();
        for record in effects.into_iter().rev() {
            if let Err(e) = record.drain(&handle).await {
                errors.push(e);
            }
        }
        // 已自摘记录的寄存错误并入(0.2.1):fiber 级观察等价于记录留在
        // 列表中被再次 join——单一错误同一 Arc,多错误进入聚合
        errors.extend(std::mem::take(&mut *this.drained_errors.lock().unwrap()));
        *this.last_deps.lock().unwrap() = None;
        this.provided.lock().unwrap().clear();
        crate::definition::ended(definitions);
        errors
    }

    /// Unload: mark Unloading, cancel the current generation's token, wait for
    /// event calls (`apply` has already returned, as the driver is serial),
    /// drain the effect records in reverse order, then publish the next state.
    /// For `Pending` (restart or dependency refresh) cleanup errors go to the
    /// error sink; for `Disposed` they become the fiber's error.
    async fn unload(this: &Arc<Self>, next: NextState) {
        {
            // Close dispatch admission and transition together. A snapshot
            // must either register its flight first or see Unloading.
            let _admission = this.ctx.shared().admission.lock().unwrap();
            let mut tr = this.transition.lock().unwrap();
            Self::set_state(this, &mut tr, FiberState::Unloading);
        }
        this.flush_status();

        // ② 取消当前代 token(插件后台任务经 ctx.cancelled() 协作退出)
        this.cancel_current();

        // Includes synchronous terminals with no listener cleanup record.
        this.wait_events().await;

        // ④ EffectRecord 严格 LIFO 串行清理
        let errors = Self::drain_effects(this).await;
        let err = aggregate_arcs(errors);

        match next {
            // 非终止卸载(restart/依赖刷新):清理错误路由 ErrorSink,
            // 不改变下一代状态(评审 P2:不得静默吞掉)
            NextState::Pending => {
                if let Some(e) = err {
                    (this.ctx.error_sink())(e);
                }
                let mut tr = this.transition.lock().unwrap();
                Self::set_state(this, &mut tr, FiberState::Pending);
            }
            NextState::Disposed => {
                let mut tr = this.transition.lock().unwrap();
                tr.error = err;
                Self::set_state(this, &mut tr, FiberState::Disposed);
            }
        }
        this.flush_status();
    }
}

/// How long an unload waits for the event calls still running on a fiber
/// (protocol spec §7.1): `RUTIS_SHUTDOWN_TIMEOUT` in seconds (fractions
/// allowed, above 0; rutis-host sets it from `--shutdown-timeout`), 10 s
/// otherwise.
pub fn shutdown_deadline() -> Duration {
    std::env::var("RUTIS_SHUTDOWN_TIMEOUT")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .unwrap_or(Duration::from_secs(10))
}

/// The fiber's driver task. It handles intents one at a time, so Loading →
/// Unloading → Loading loses no wakeup, `apply` and an unload never overlap,
/// and results of different generations never mix. A non-root driver exits
/// after a dispose or shutdown.
pub(crate) async fn drive(this: Arc<FiberInner>, mut rx: mpsc::UnboundedReceiver<Intent>) {
    while let Some(intent) = rx.recv().await {
        let current_task = intent.task().cloned();
        let terminal_dispose = matches!(intent, Intent::Dispose) && !this.is_root;
        let shutdown_task = match &intent {
            Intent::Shutdown(task) => Some(task.clone()),
            _ => None,
        };
        let outcome = CatchUnwind::new(async {
            match intent {
                Intent::RefreshDeps => FiberInner::refresh_deps(&this).await,
                Intent::RefreshDepsJoin(task) => {
                    FiberInner::refresh_deps(&this).await;
                    complete_task(&this, task);
                }
                // 稳定性栅栏:FIFO 排到这里时,此前入队的意图已全部处理完。
                // 错误只认 Failed(红线):dispose 聚合错误经 dispose() 的
                // 任务通道返回,不从 settle 漏出。
                Intent::Settle(task) => {
                    let err = {
                        let tr = this.transition.lock().unwrap();
                        (tr.state == FiberState::Failed)
                            .then(|| tr.error.clone())
                            .flatten()
                    };
                    task.complete(err);
                }
                Intent::Restart(task) => {
                    // Taken up: generations from here on start with a live
                    // token unless another restart is still pending.
                    {
                        let mut tr = this.transition.lock().unwrap();
                        tr.pending_restarts = tr.pending_restarts.saturating_sub(1);
                    }
                    if this.closing.load(Ordering::SeqCst)
                        || this.ctx.shared().closing.load(Ordering::SeqCst)
                    {
                        task.complete(Some(Arc::new(ErrorKind::Closed)));
                        return;
                    }
                    // A dispose registered first wins: this restart does not
                    // happen and reports what `restart()` reports once a
                    // dispose is registered. Checked before the Disposed
                    // branch: a restart queued behind a second restart or
                    // a refresh that already unloaded for the dispose finds
                    // the fiber Disposed and must not report the dispose's
                    // result as its own. (A non-root fiber is Disposed only
                    // with a dispose registered.)
                    if this.terminal_registered() {
                        FiberInner::unload_for_dispose(&this).await;
                        task.complete(Some(Arc::new(ErrorKind::InactiveEffect)));
                        return;
                    }
                    let state = this.state();
                    if state == FiberState::Disposed {
                        // 仅 root 可重启(§五 root_restart);换新代 token,
                        // 并清除终态任务:重启后的 dispose 必须真正再卸载一轮
                        if this.is_root {
                            this.new_generation_token();
                            {
                                let mut tr = this.transition.lock().unwrap();
                                tr.error = None;
                                tr.terminal_task = None;
                                FiberInner::set_state(&this, &mut tr, FiberState::Active);
                            }
                            this.flush_status();
                        }
                        complete_task(&this, task);
                        return;
                    }
                    if !matches!(state, FiberState::Pending) {
                        FiberInner::unload(&this, NextState::Pending).await;
                    }
                    let before = this.transition.lock().unwrap().generation;
                    FiberInner::refresh_deps(&this).await;
                    let started = this.transition.lock().unwrap().generation != before;
                    if !started && this.terminal_registered() {
                        // Registered while this restart was unloading: no
                        // new generation started (`load()` checks it).
                        task.complete(Some(Arc::new(ErrorKind::InactiveEffect)));
                        return;
                    }
                    if !started {
                        // Waiting for its dependencies: no generation ran,
                        // so none failed. The error a Failed fiber keeps is
                        // the last generation's, not this restart's (#665).
                        task.complete(None);
                        return;
                    }
                    complete_task(&this, task);
                }
                Intent::Dispose => {
                    if this.closing.load(Ordering::SeqCst) {
                        this.wait_events().await;
                    }
                    if this.state() != FiberState::Disposed {
                        FiberInner::unload(&this, NextState::Disposed).await;
                    }
                    let (task, err) = {
                        let tr = this.transition.lock().unwrap();
                        (tr.terminal_task.clone(), tr.error.clone())
                    };
                    if let Some(task) = task {
                        let _ = task.done_tx.send(TaskDone::Done(err));
                    }
                }
                Intent::Shutdown(_) => {
                    this.wait_events().await;
                    let prior = if !this.is_root && this.state() == FiberState::Failed {
                        this.transition.lock().unwrap().error.clone()
                    } else {
                        None
                    };
                    if this.state() != FiberState::Disposed {
                        FiberInner::unload(&this, NextState::Disposed).await;
                    }
                    if let Some(prior) = prior {
                        let mut tr = this.transition.lock().unwrap();
                        tr.error =
                            aggregate_arcs(std::iter::once(prior).chain(tr.error.take()).collect());
                    }
                }
            }
            this.flush_status();
        })
        .await;
        if let Err(panic) = outcome {
            recover_driver_panic(&this, &mut rx, current_task, panic).await;
            return;
        }
        if let Some(task) = shutdown_task {
            this.alive.store(false, Ordering::SeqCst);
            // A registered terminal task must end with this result. Its
            // Dispose intent carries no task. Since #234 a root dispose cannot
            // register behind a queued Shutdown (both take the admission
            // lock); this stays as the guard for that state.
            let (terminal, err) = {
                let tr = this.transition.lock().unwrap();
                (tr.terminal_task.clone(), tr.error.clone())
            };
            while let Ok(intent) = rx.try_recv() {
                let completion = match &intent {
                    // Settle reports Failed only; a clean terminal state is
                    // stable even if its request was queued after Shutdown.
                    Intent::Settle(_) => None,
                    // A second internal shutdown must observe the actual
                    // terminal result, including cleanup errors.
                    Intent::Shutdown(_) => err.clone(),
                    _ => Some(Arc::new(ErrorKind::Closed)),
                };
                complete_intent(&intent, completion);
            }
            if !this.is_root {
                // The mount disposer may join this internal result. Publish it
                // before release_transient drains that same mount record.
                task.complete(err.clone());
            }
            if !this.is_root {
                release_transient(&this).await;
            }
            if let Some(terminal) = terminal {
                terminal.complete(err.clone());
            }
            drop(this);
            task.complete(err);
            return;
        }
        if terminal_dispose {
            // 退出:先置 false(此后 post_join 的迟到投递自完成),
            // 单遍排空已入队的残留并完成其任务(评审 #2/#3)
            this.alive.store(false, Ordering::SeqCst);
            let terminal_error = this.transition.lock().unwrap().error.clone();
            while let Ok(intent) = rx.try_recv() {
                // Settle 只观察 Failed；其他 join 应拿到 dispose 的
                // 清理错误，而非被排空时伪报成功。
                let error = if matches!(intent, Intent::Settle(_)) {
                    None
                } else {
                    terminal_error.clone()
                };
                complete_intent(&intent, error);
            }
            // 瞬态残留释放(0.2.1):依赖声明注销 + parent mount 记录
            // drain(终态后清理幂等,记录 Done 自摘)。TaskDone 已发,
            // mount 清理里的 dispose() join 缓存终态,即刻完成。
            release_transient(&this).await;
            return; // 非 root 终态后驱动退出(句柄仍可 join 缓存终态)
        }
    }
}

/// Recover from an unexpected panic in the driver instead of letting the
/// driver vanish: withdraw what was registered, enter Failed, and complete
/// the current task, the terminal task and every queued intent with the same
/// error. A panic in `build`, `validate` or `apply` does not come here: the
/// load's rollback handles it.
async fn recover_driver_panic(
    this: &Arc<FiberInner>,
    rx: &mut mpsc::UnboundedReceiver<Intent>,
    current_task: Option<Arc<TransitionTask>>,
    panic: Box<dyn Any + Send>,
) {
    this.cancel_current();
    {
        let _admission = this.ctx.shared().admission.lock().unwrap();
        let mut tr = this.transition.lock().unwrap();
        FiberInner::set_state(this, &mut tr, FiberState::Unloading);
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.flush_status()));
    this.wait_events().await;
    let mut errors = vec![panic_error(panic).shared()];
    let cleanup_errors = match CatchUnwind::new(FiberInner::drain_effects(this)).await {
        Ok(cleanup_errors) => cleanup_errors,
        Err(cleanup_panic) => vec![panic_error(cleanup_panic).shared()],
    };
    let sink = this.ctx.error_sink();
    for error in &cleanup_errors {
        // A sink panic must not prevent the original waiters from settling.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(error.clone())));
    }
    errors.extend(cleanup_errors);
    let error = aggregate_arcs(errors).expect("driver panic present");
    let terminal_task = {
        let mut tr = this.transition.lock().unwrap();
        tr.error = Some(error.clone());
        FiberInner::set_state(this, &mut tr, FiberState::Failed);
        this.alive.store(false, Ordering::SeqCst);
        tr.terminal_task.clone()
    };
    if let Some(task) = current_task {
        task.complete(Some(error.clone()));
    }
    if let Some(task) = terminal_task {
        task.complete(Some(error.clone()));
    }
    while let Ok(intent) = rx.try_recv() {
        complete_intent(&intent, Some(error.clone()));
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.flush_status()));
    if !this.is_root {
        let _ = CatchUnwind::new(release_transient(this)).await;
    }
}

/// Complete a transition task with the fiber's current error (every joiner
/// gets the same `Arc`).
fn complete_task(this: &Arc<FiberInner>, task: Arc<TransitionTask>) {
    let err = {
        let tr = this.transition.lock().unwrap();
        tr.error.clone()
    };
    task.complete(err);
}

/// Release what a fiber in a terminal state still holds (a transient child
/// under a long-lived parent):
/// 1. unregister its dependency declarations from `inject_index`, so the
///    stopped driver gets no more dependency notifications;
/// 2. drain its mount record on the parent: the `dispose()` in that cleanup
///    returns the cached terminal result at once, and the record, now done,
///    removes itself from the parent's effects (if the parent unloaded
///    first, the record is already done and this does nothing);
/// 3. remove it, and dead entries, from the parent's children.
async fn release_transient(this: &Arc<FiberInner>) {
    let shared = this.ctx.shared().clone();
    shared
        .registry
        .unregister_injects(this, &this.declared_injects);
    let record = this.mount.lock().unwrap().take();
    if let Some(record) = record {
        let handle = this.ctx.handle().clone();
        if let Err(e) = record.drain(&handle).await {
            let sink = this.ctx.error_sink();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(e)));
        }
    }
    if let Some(parent) = this.parent_fiber.as_ref().and_then(Weak::upgrade) {
        let self_weak = Arc::downgrade(this);
        let mut children = parent.children.lock().unwrap();
        children.retain(|child| !Weak::ptr_eq(child, &self_weak) && child.strong_count() > 0);
        if children.capacity() > 64 && children.len() * 4 < children.capacity() {
            children.shrink_to_fit();
        }
    }
}

pub(crate) fn spawn_fiber(
    shared: &Arc<Shared>,
    parent_ctx: Option<&Ctx>,
    plugin: Option<Arc<dyn DynPlugin>>,
    is_root: bool,
) -> Arc<FiberInner> {
    spawn_fiber_inner(shared, parent_ctx, plugin, None, None, is_root)
}

/// Spawn a fiber in factory mode (used by `Ctx::plugin_with`).
pub(crate) fn spawn_factory_fiber<F, C>(
    shared: &Arc<Shared>,
    parent_ctx: Option<&Ctx>,
    factory: F,
    config: C,
    is_root: bool,
) -> Arc<FiberInner>
where
    F: PluginFactory<C>,
    C: Send + Sync + 'static,
{
    let erased: Arc<dyn ErasedFactory> = Arc::new(FactoryAdapter {
        factory,
        _marker: std::marker::PhantomData,
    });
    // 依赖声明静态(D32f):spawn 注册一次,终身不变——由
    // spawn_fiber_inner 从擦除面一次性读取并注册,与静态模式完全同形。
    let boxed: Arc<dyn Any + Send + Sync> = Arc::new(config);
    spawn_fiber_inner(shared, parent_ctx, None, Some(erased), Some(boxed), is_root)
}

fn spawn_fiber_inner(
    shared: &Arc<Shared>,
    parent_ctx: Option<&Ctx>,
    plugin: Option<Arc<dyn DynPlugin>>,
    factory: Option<Arc<dyn ErasedFactory>>,
    config: Option<Arc<dyn Any + Send + Sync>>,
    is_root: bool,
) -> Arc<FiberInner> {
    let is_closed = || {
        !is_root
            && (shared.closing.load(Ordering::SeqCst)
                || parent_ctx
                    .is_none_or(|p| p.subtree_closing() || p.weak_fiber().upgrade().is_none()))
    };
    let known_closed = is_closed();
    // Capture user metadata before taking the root admission lock.
    let name = if known_closed {
        "closed".to_string()
    } else {
        plugin
            .as_ref()
            .map(|p| p.name().to_string())
            .or_else(|| factory.as_ref().map(|f| f.name().to_string()))
            .unwrap_or_else(|| "root".to_string())
    };
    let platform = is_root || (!known_closed && plugin.as_ref().is_some_and(|p| p.platform()));
    let declared_injects: Vec<TypeKey> = if known_closed {
        Vec::new()
    } else if let Some(plugin) = &plugin {
        plugin.injects()
    } else if let Some(factory) = &factory {
        factory.injects()
    } else {
        Vec::new()
    };
    let _admission = shared.admission.lock().unwrap();
    let closed = is_closed();
    let (plugin, factory, config, declared_injects) = if closed {
        (None, None, None, Vec::new())
    } else {
        (plugin, factory, config, declared_injects)
    };
    let id = PluginId(
        shared
            .next_plugin_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
    );
    let instance = InstanceId::allocate();
    let initial = if closed {
        FiberState::Disposed
    } else if is_root {
        FiberState::Active
    } else {
        FiberState::Pending
    };
    let closed_error = closed.then(|| Arc::new(ErrorKind::Closed));
    let closed_task = closed.then(|| {
        let task = TransitionTask::new();
        task.complete(closed_error.clone());
        task
    });
    let (snapshot_tx, snapshot_rx) = watch::channel(Snapshot {
        generation: 0,
        state: initial,
        error: closed_error.clone(),
    });
    let (event_flights_tx, event_flights_rx) = watch::channel(0);
    let (tx, rx) = mpsc::unbounded_channel();
    let token = CancellationToken::new();
    let closing = Arc::new(AtomicBool::new(closed));
    let parent_fiber = parent_ctx.map(|p| p.weak_fiber());

    let this = Arc::new_cyclic(|weak: &Weak<FiberInner>| {
        let ctx = match parent_ctx {
            Some(parent) => Ctx::new_child(
                shared.clone(),
                parent,
                weak.clone(),
                None,
                instance,
                id,
                closing.clone(),
            ),
            None => Ctx::new_root(shared.clone(), weak.clone(), instance, id, closing.clone()),
        };
        FiberInner {
            id,
            instance,
            name: if closed { "closed".to_string() } else { name },
            is_root,
            platform,
            plugin,
            factory,
            config: Mutex::new(config),
            ctx,
            parent_fiber,
            children: Mutex::new(Vec::new()),
            closing,
            event_flights: Mutex::new(0),
            event_flights_tx,
            _event_flights_rx: event_flights_rx,
            token: Mutex::new(token),
            transition: Mutex::new(Trans {
                generation: 0,
                state: initial,
                error: closed_error,
                seq: 0,
                status_queue: Vec::new(),
                terminal_task: closed_task,
                pending_restarts: 0,
            }),
            snapshot_tx: snapshot_tx.clone(),
            snapshot_rx,
            effects: Mutex::new(Vec::new()),
            definitions: Mutex::new(Vec::new()),
            effect_index: Mutex::new(Vec::new()),
            drained_errors: Mutex::new(Vec::new()),
            mount: Mutex::new(None),
            declared_injects,
            intents_tx: tx.clone(),
            alive: crate::sync::AtomicBool::new(!closed),
            provided: Mutex::new(Vec::new()),
            last_deps: Mutex::new(None),
            accesses: Mutex::new(Vec::new()),
            scope_group: std::sync::OnceLock::new(),
            row: std::sync::OnceLock::new(),
            code_generation: std::sync::atomic::AtomicU64::new(0),
            waiting_watch: Mutex::new(None),
            shutdown_task: Mutex::new(None),
            shutdown_inner: Mutex::new(None),
            driver: Mutex::new(None),
        }
    });

    if !closed {
        if let Some(parent) = this.parent_fiber.as_ref().and_then(Weak::upgrade) {
            parent.children.lock().unwrap().push(Arc::downgrade(&this));
        }
    }

    if !closed {
        for key in &this.declared_injects {
            // A foreign instance can never enter this fiber's ancestry, so
            // changes to that key cannot make its Pending gate succeed.
            if this.ctx.in_instance_key(key) {
                shared.registry.register_inject(key.clone(), &this);
            }
        }
        let driver = shared.handle.spawn(drive(this.clone(), rx));
        *this.driver.lock().unwrap() = Some(driver);
    }
    this
}

/// A handle to a fiber, returned on registration. Awaiting it
/// (`IntoFuture`) waits until the fiber settles; a failed start comes back
/// as `Err`.
///
/// Awaiting a view waits until what is already queued for the fiber has
/// been handled, not until the plugin runs: a plugin waiting for a
/// dependency that is not there is `Pending`, and the await returns
/// `Ok(())`. To wait until it runs, watch its state ([`FiberView::watch`]).
pub struct FiberView {
    /// The plugin's identity.
    pub id: PluginId,
    pub(crate) inner: Arc<FiberInner>,
}

impl Clone for FiberView {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            inner: self.inner.clone(),
        }
    }
}

impl FiberView {
    pub(crate) fn from_inner(inner: Arc<FiberInner>) -> Self {
        Self {
            id: inner.id,
            inner,
        }
    }

    /// The current snapshot.
    pub fn state(&self) -> Snapshot {
        self.inner.state_snapshot()
    }

    /// The current snapshot, with the `seq` of the `FiberStatusChanged` that
    /// produced it (0 if there was no change yet): compare it with events to
    /// know which ones came before this snapshot.
    pub fn sequenced_state(&self) -> (u64, Snapshot) {
        self.inner.sequenced_snapshot()
    }

    /// Current cleanup ownership tree for this fiber. Records being drained
    /// remain visible until their cleanup completes. This is a point-in-time
    /// read of metadata only; it never calls user code.
    pub fn effects(&self) -> Vec<EffectMeta> {
        let records = self.inner.effect_index.lock().unwrap().clone();
        records
            .into_iter()
            .filter_map(|entry| entry.upgrade())
            .filter_map(|record| record.snapshot())
            .collect()
    }

    /// The plugin's display name.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Take this fiber's completed, early-disposed effect errors. Taken
    /// errors are excluded from a later unload result; see
    /// [`Ctx::take_cleanup_errors`].
    pub fn take_cleanup_errors(&self) -> Vec<Arc<ErrorKind>> {
        std::mem::take(&mut *self.inner.drained_errors.lock().unwrap())
    }

    /// Permanently close this fiber and its descendants. Admission closes at
    /// the call site; dropping the returned future does not stop cleanup.
    /// A callback or finalizer must not await shutdown of its own subtree.
    pub fn shutdown(&self) -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> {
        if self.inner.is_root {
            return self.inner.ctx.shutdown();
        }
        let shared = self.inner.ctx.shared().clone();
        let task = {
            let _admission = shared.admission.lock().unwrap();
            let mut pending = vec![self.inner.clone()];
            while let Some(fiber) = pending.pop() {
                fiber.closing.store(true, Ordering::SeqCst);
                fiber.cancel_current();
                pending.extend(
                    fiber
                        .children
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(Weak::upgrade),
                );
            }
            begin_subtree_shutdown_locked(&self.inner)
        };
        Box::pin(async move { join_task(&task).await })
    }

    /// Subscribe to state changes. The channel keeps the last value: a late
    /// subscriber reads `borrow()` first, then waits on `changed()`.
    pub fn watch(&self) -> watch::Receiver<Snapshot> {
        self.inner.snapshot_rx.clone()
    }

    /// Dispose the fiber, exactly once: repeated or concurrent calls join the
    /// same `Arc<ErrorKind>`. The current generation's token is cancelled
    /// before the intent is queued, so a running `apply` can return early.
    ///
    /// Root (#234): the call is ordered against `Ctx::shutdown()` by the
    /// admission lock. Before shutdown closes admission it registers its own
    /// dispose, which the shutdown then waits behind and shares; after, it
    /// joins the shutdown task. Either way it shares the one cleanup result.
    pub fn dispose(&self) -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> {
        let task = if self.inner.is_root {
            #[cfg(test)]
            test_hooks::root_dispose();
            // Deciding (closing or not) and registering must be one step:
            // checked apart, a whole shutdown can finish in between and the
            // dispose would register on a stopped driver and get `Closed`.
            let shared = self.inner.ctx.shared().clone();
            let _admission = shared.admission.lock().unwrap();
            if shared.closing.load(Ordering::SeqCst) {
                shared
                    .shutdown_task
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("shutdown stores its task before closing admission")
            } else {
                self.register_unconditional_dispose()
            }
        } else {
            self.register_unconditional_dispose()
        };
        Box::pin(async move { join_task(&task).await })
    }

    fn register_unconditional_dispose(&self) -> Arc<TransitionTask> {
        match self.register_dispose(|_| Ok(())) {
            Ok(task) => task,
            Err(_) => unreachable!("an unconditional dispose is always registered"),
        }
    }

    /// Register the dispose (or join the one registered) if `check` accepts
    /// the transition state. Check and registration share one lock: no new
    /// generation can start between them, and a restart that comes after
    /// sees the terminal task and is refused.
    pub(crate) fn register_dispose(
        &self,
        check: impl FnOnce(&Trans) -> Result<(), Error>,
    ) -> Result<Arc<TransitionTask>, Error> {
        // 登记在调用点同步完成(评审 #2):dispose() 返回后,并发的
        // restart() 立刻可见终态任务并拒绝,不依赖本 future 被 poll
        let (task, newly_registered) = {
            let mut tr = self.inner.transition.lock().unwrap();
            check(&tr)?;
            match &tr.terminal_task {
                Some(task) => (task.clone(), false),
                None => {
                    self.inner.cancel_current();
                    let task = TransitionTask::new();
                    tr.terminal_task = Some(task.clone());
                    (task, true)
                }
            }
        };
        if newly_registered {
            self.inner.post(Intent::Dispose);
            if !self.inner.alive.load(Ordering::SeqCst) {
                task.complete(self.inner.stopped_error());
            }
        }
        Ok(task)
    }

    /// Dispose, waiting at most `limit`. It does not stop the plugin or the
    /// cleanup that is running: after a timeout, calling `dispose()` again keeps
    /// waiting for the same terminal task and its error.
    pub fn dispose_with_timeout(
        &self,
        limit: Duration,
    ) -> BoxFuture<'static, Result<(), DisposeWaitError>> {
        let pending = self.dispose();
        let inner = self.inner.clone();
        Box::pin(async move {
            let started = Instant::now();
            match tokio::time::timeout(limit, pending).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(DisposeWaitError::Failed(error)),
                Err(_) => {
                    let snapshot = inner.state_snapshot();
                    Err(DisposeWaitError::TimedOut {
                        plugin_id: inner.id,
                        generation: snapshot.generation,
                        state: snapshot.state,
                        elapsed: started.elapsed(),
                        stuck: stuck_below(&inner),
                    })
                }
            }
        })
    }

    /// Unload cleanly and load again; no state is carried over. Returns
    /// `Closed` while the fiber or instance is closing, and `InactiveEffect` for
    /// a non-root fiber that is disposed, has a dispose registered or whose
    /// driver has stopped. Waits until this restart is handled: returns the new
    /// generation's error, or `Ok(())` if it is waiting for dependencies. A
    /// disposed root becomes Active again.
    pub fn restart(&self) -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> {
        let this = self.inner.clone();
        Box::pin(async move {
            {
                let mut tr = this.transition.lock().unwrap();
                if this.ctx.shared().closing.load(Ordering::SeqCst)
                    || this.closing.load(Ordering::SeqCst)
                {
                    return Err(Arc::new(ErrorKind::Closed));
                }
                if !this.is_root
                    && (tr.terminal_task.is_some() || !this.alive.load(Ordering::SeqCst))
                {
                    return Err(Arc::new(ErrorKind::InactiveEffect));
                }
                // 预取消:运行中的 apply 协作退出。Counted under the same
                // lock: a generation `load()` starts before the driver takes
                // this restart up starts cancelled too.
                tr.pending_restarts += 1;
                this.cancel_current();
            }
            let task = TransitionTask::new();
            this.post_join(task.clone(), Intent::Restart);
            join_task(&task).await
        })
    }

    /// Replace the config, after a dry run, and restart with it.
    ///
    /// - The dry run is `validate_config` + `build` + the instance's
    ///   `validate`; if any fails, the error is returned and nothing is stored
    ///   or restarted.
    /// - Once it passes, the config is stored and a restart follows: an
    ///   Active, Loading or Failed fiber unloads and loads with an instance
    ///   built from the new config; a Pending one re-checks its dependencies
    ///   and loads if they are ready, else keeps waiting with the new config.
    /// - A static plugin (not from a factory), or a config of another type,
    ///   returns a `Validation` error.
    /// - Refused like `restart` when closing or disposed.
    pub fn update<C: Send + Sync + 'static>(
        &self,
        new_config: C,
    ) -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> {
        let this = self.inner.clone();
        Box::pin(async move {
            {
                let tr = this.transition.lock().unwrap();
                if this.ctx.shared().closing.load(Ordering::SeqCst)
                    || this.closing.load(Ordering::SeqCst)
                {
                    return Err(Arc::new(ErrorKind::Closed));
                }
                if !this.is_root
                    && (tr.terminal_task.is_some() || !this.alive.load(Ordering::SeqCst))
                {
                    return Err(Arc::new(ErrorKind::InactiveEffect));
                }
            }
            let factory = this
                .factory
                .as_ref()
                .and_then(|f| (f.config_type_id() == TypeId::of::<C>()).then(|| f.clone()));
            let Some(factory) = factory else {
                let reason = if this.factory.is_some() {
                    "config type mismatch"
                } else {
                    "fiber has no factory (static plugin cannot update)"
                };
                return Err(Arc::new(ErrorKind::Validation {
                    issues: vec![reason.into()],
                }));
            };
            // dry-run(D32b):validate_config → build → 实例 validate,产物丢弃
            // (build 纯构造契约)。用户回调 panic 转错误不杀调用方。
            // 对照声明(审计):cordis 在非 ACTIVE 态**延迟** config 校验到激活时
            // (fiber.ts:739 只存 `_config`);此处无条件 dry-run(D32b,尽早暴露)。
            // 通过后:二次终态检查(dry-run 无锁同步,期间并发 dispose 可完整
            // 执行,评审 3 探针实测窗口存在)→ 存 config → restart。
            // 依赖声明静态(D32f),update 不触碰注册表。
            let boxed: Arc<dyn Any + Send + Sync> = Arc::new(new_config);
            let dry = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                factory.validate_config_erased(boxed.as_ref())?;
                let instance = factory.build_erased(boxed.as_ref())?;
                instance.validate()?;
                Ok::<(), Error>(())
            }));
            match dry {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e.shared()),
                Err(p) => return Err(panic_error(p).shared()),
            }
            // 二次终态检查:dry-run 期间并发 dispose 可能已登记 terminal_task
            // 并完成卸载——此时不存,现状交由 dispose 收敛(评审 3 探针:dry-run
            // 后存 config 会落进已死 fiber)。
            {
                let mut tr = this.transition.lock().unwrap();
                if this.ctx.shared().closing.load(Ordering::SeqCst)
                    || this.closing.load(Ordering::SeqCst)
                {
                    return Err(Arc::new(ErrorKind::Closed));
                }
                if !this.is_root
                    && (tr.terminal_task.is_some() || !this.alive.load(Ordering::SeqCst))
                {
                    return Err(Arc::new(ErrorKind::InactiveEffect));
                }
                // 预取消(同 restart),counted under the same lock.
                tr.pending_restarts += 1;
                this.cancel_current();
            }
            *this.config.lock().unwrap() = Some(boxed);
            let task = TransitionTask::new();
            this.post_join(task.clone(), Intent::Restart);
            join_task(&task).await
        })
    }

    /// The current config, for diagnostics. `None` for a static plugin or a
    /// different type.
    pub fn current_config<C: Send + Sync + 'static>(&self) -> Option<Arc<C>> {
        let boxed = self.inner.config.lock().unwrap().clone()?;
        boxed.downcast::<C>().ok()
    }

    /// The identity of this fiber, as its contexts report in
    /// [`Ctx::instance`]; use it to address instance events to the plugin.
    pub fn instance(&self) -> crate::InstanceId {
        self.inner.instance
    }

    /// Store `new_config` without restarting: the running instance keeps
    /// going and later loads (restart, dependency reload) build from the new
    /// config. Only `validate_config` runs. For changes the plugin applies
    /// in place, such as cordis's volatile fields; everything else should
    /// use [`FiberView::update`]. Refused like `update` for closed or
    /// disposed fibers, static plugins and a mismatched config type.
    pub fn set_config<C: Send + Sync + 'static>(
        &self,
        new_config: C,
    ) -> Result<(), Arc<ErrorKind>> {
        let this = &self.inner;
        {
            let tr = this.transition.lock().unwrap();
            if this.ctx.shared().closing.load(Ordering::SeqCst)
                || this.closing.load(Ordering::SeqCst)
            {
                return Err(Arc::new(ErrorKind::Closed));
            }
            if !this.is_root && (tr.terminal_task.is_some() || !this.alive.load(Ordering::SeqCst)) {
                return Err(Arc::new(ErrorKind::InactiveEffect));
            }
        }
        let factory = this
            .factory
            .as_ref()
            .filter(|f| f.config_type_id() == TypeId::of::<C>())
            .ok_or_else(|| {
                Arc::new(ErrorKind::Validation {
                    issues: vec![if this.factory.is_some() {
                        "config type mismatch".into()
                    } else {
                        "fiber has no factory (static plugin has no config)".into()
                    }],
                })
            })?;
        let boxed: Arc<dyn Any + Send + Sync> = Arc::new(new_config);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            factory.validate_config_erased(boxed.as_ref())
        })) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e.shared()),
            Err(p) => return Err(panic_error(p).shared()),
        }
        *this.config.lock().unwrap() = Some(boxed);
        Ok(())
    }
}

/// Called with the root admission lock held, after every member has been
/// marked closing. Parent ownership is captured before any child can detach.
fn begin_subtree_shutdown_locked(fiber: &Arc<FiberInner>) -> Arc<TransitionTask> {
    let mut slot = fiber.shutdown_task.lock().unwrap();
    if let Some(task) = slot.as_ref() {
        return task.clone();
    }
    let public = TransitionTask::new();
    *slot = Some(public.clone());
    drop(slot);

    let children: Vec<Arc<FiberInner>> = fiber
        .children
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    let child_tasks: Vec<Arc<TransitionTask>> =
        children.iter().map(begin_subtree_shutdown_locked).collect();

    let (inner, needs_post) = {
        let mut tr = fiber.transition.lock().unwrap();
        match tr.terminal_task.as_ref() {
            Some(task) => (task.clone(), false),
            None => {
                let task = TransitionTask::new();
                tr.terminal_task = Some(task.clone());
                (task, true)
            }
        }
    };
    *fiber.shutdown_inner.lock().unwrap() = Some(inner.clone());
    if needs_post {
        fiber.post(Intent::Shutdown(inner.clone()));
    }
    let owner = fiber.clone();
    let result = public.clone();
    fiber.ctx.handle().spawn(async move {
        let mut errors = Vec::new();
        if let Err(error) = join_task(&inner).await {
            errors.push(error);
        }
        let driver = { owner.driver.lock().unwrap().take() };
        if let Some(driver) = driver {
            if let Err(error) = driver.await {
                errors.push(crate::error::join_panic_error(error).shared());
            }
        }
        for child in child_tasks {
            if let Err(error) = join_task(&child).await {
                errors.push(error);
            }
        }
        drop(owner);
        result.complete(aggregate_arcs(errors));
    });
    public
}

pub(crate) async fn join_task(task: &Arc<TransitionTask>) -> Result<(), Arc<ErrorKind>> {
    let mut rx = task.done_tx.subscribe();
    loop {
        match &*rx.borrow() {
            TaskDone::Done(err) => return err.clone().map_or(Ok(()), Err),
            TaskDone::Running => {}
        }
        if rx.changed().await.is_err() {
            // 发送端随 fiber 析构:不得伪装成功(评审 #8)
            return Err(Arc::new(ErrorKind::PluginFailed(
                "fiber dropped before transition completed".into(),
            )));
        }
    }
}

/// Awaiting a view (`IntoFuture`): a `Settle` barrier plus a join. The
/// mailbox's FIFO order answers whether the fiber has settled: when `Settle`
/// is reached, the intents queued before it (initial load, reload
/// notifications, restarts) have been handled. Only a Failed state reports
/// an error; a dispose's errors come back through `dispose()`.
fn settle(this: &FiberInner) -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> {
    let task = TransitionTask::new();
    this.post_join(task.clone(), Intent::Settle);
    Box::pin(async move { join_task(&task).await })
}

impl std::future::IntoFuture for FiberView {
    type Output = Result<(), Arc<ErrorKind>>;
    type IntoFuture = BoxFuture<'static, Result<(), Arc<ErrorKind>>>;

    fn into_future(self) -> Self::IntoFuture {
        settle(&self.inner)
    }
}

impl std::future::IntoFuture for &FiberView {
    type Output = Result<(), Arc<ErrorKind>>;
    type IntoFuture = BoxFuture<'static, Result<(), Arc<ErrorKind>>>;

    fn into_future(self) -> Self::IntoFuture {
        settle(&self.inner)
    }
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::RefCell;

    thread_local! {
        static ROOT_DISPOSE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    /// Run `f` once, on this thread, inside the next root `dispose()` call:
    /// after the call has started and before it registers or joins anything.
    pub(crate) fn on_root_dispose(f: impl FnOnce() + 'static) {
        ROOT_DISPOSE.with(|hook| *hook.borrow_mut() = Some(Box::new(f)));
    }

    pub(super) fn root_dispose() {
        if let Some(f) = ROOT_DISPOSE.with(|hook| hook.borrow_mut().take()) {
            f();
        }
    }
}

pub(crate) mod waiting;

#[cfg(test)]
mod transient_tests;

// The shuttle models (#179), built only with `--cfg rutis_shuttle`
// (`crate::sync`; CONTRIBUTING.md says how to run them).
#[cfg(all(test, rutis_shuttle))]
mod shuttle_tests;
