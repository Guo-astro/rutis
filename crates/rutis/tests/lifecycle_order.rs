//! The order of the unload steps (D7) and what dropping a `restart` /
//! `update` future does to work already under way (#177).

mod common;

use std::future::IntoFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView};
use rutis::{Ctx, Error, Plugin};
use rutis_test_support::within;
use tokio::sync::Notify;

use common::still_pending;

type Log = Arc<Mutex<Vec<String>>>;

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// The fiber's state as its own cleanup sees it.
type Slot = Arc<OnceLock<FiberView>>;

fn state_of(slot: &Slot) -> FiberState {
    slot.get().expect("view stored before unload").state().state
}

/// Polls `f` once; it must not be ready yet.
async fn poll_once<F: std::future::Future + Unpin>(f: &mut F)
where
    F::Output: std::fmt::Debug,
{
    tokio::select! {
        biased;
        output = f => panic!("finished on its first poll: {output:?}"),
        () = std::future::ready(()) => {}
    }
}

// ── D7: the unload steps ────────────────────────────────────────

/// Two cleanups; the later one (`second`, drained first) waits for `hold`
/// after telling `started`. Each records the state and whether this
/// generation's token was cancelled when it ran.
struct ActiveSteps {
    log: Log,
    view: Slot,
    started: Arc<Notify>,
    hold: Arc<Notify>,
}

impl DynPlugin for ActiveSteps {
    fn name(&self) -> &str {
        "active-steps"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let token = ctx.cancellation_token();
            let (log, view) = (self.log.clone(), self.view.clone());
            let first_token = token.clone();
            ctx.effect(move || async move {
                log.lock().unwrap().push(format!(
                    "cleanup first: {:?}, cancelled {}",
                    state_of(&view),
                    first_token.is_cancelled()
                ));
                Ok(())
            })?;
            let (log, view) = (self.log.clone(), self.view.clone());
            let (started, hold) = (self.started.clone(), self.hold.clone());
            ctx.effect(move || async move {
                log.lock().unwrap().push(format!(
                    "cleanup second: {:?}, cancelled {}",
                    state_of(&view),
                    token.is_cancelled()
                ));
                started.notify_one();
                hold.notified().await;
                log.lock().unwrap().push("cleanup second done".into());
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// Unloading an active fiber: Unloading is published and the generation is
/// cancelled before any cleanup runs; cleanups run one at a time, last
/// registered first; the final state is published, and `dispose` returns,
/// only after the last cleanup has finished.
///
/// guarantee: core/D7, core/D27
#[tokio::test]
async fn unload_publishes_unloading_and_cancels_before_cleanup_and_disposed_after() {
    let ctx = Ctx::root().unwrap();
    let log = Log::default();
    let slot = Slot::default();
    let started = Arc::new(Notify::new());
    let hold = Arc::new(Notify::new());
    let view = ctx.plugin(ActiveSteps {
        log: log.clone(),
        view: slot.clone(),
        started: started.clone(),
        hold: hold.clone(),
    });
    assert!(slot.set(view.clone()).is_ok());
    within("load", (&view).into_future()).await.unwrap();
    assert_eq!(view.state().state, FiberState::Active);

    let mut dispose = view.dispose();
    within("the first cleanup to start", started.notified()).await;
    // ① and ② came before ④; ⑤ has not happened.
    assert_eq!(entries(&log), ["cleanup second: Unloading, cancelled true"]);
    assert_eq!(view.state().state, FiberState::Unloading);
    poll_once(&mut dispose).await;

    hold.notify_one();
    within("dispose", dispose).await.unwrap();
    assert_eq!(
        entries(&log),
        [
            "cleanup second: Unloading, cancelled true",
            "cleanup second done",
            "cleanup first: Unloading, cancelled true",
        ]
    );
    assert_eq!(view.state().state, FiberState::Disposed);
}

/// Registers a cleanup, then waits in `apply` for cancellation and for
/// `exit` before returning.
struct LoadingSteps {
    log: Log,
    view: Slot,
    entered: Arc<Notify>,
    exit: Arc<Notify>,
}

impl DynPlugin for LoadingSteps {
    fn name(&self) -> &str {
        "loading-steps"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let token = ctx.cancellation_token();
            let (log, view) = (self.log.clone(), self.view.clone());
            ctx.effect(move || async move {
                log.lock().unwrap().push(format!(
                    "cleanup: {:?}, cancelled {}",
                    state_of(&view),
                    token.is_cancelled()
                ));
                Ok(())
            })?;
            self.entered.notify_one();
            ctx.cancelled().await;
            self.log
                .lock()
                .unwrap()
                .push(format!("apply cancelled: {:?}", state_of(&self.view)));
            self.exit.notified().await;
            self.log.lock().unwrap().push("apply exits".into());
            Ok(Effect::Done)
        })
    }
}

/// Disposing a loading fiber cancels its generation at once, but no
/// cleanup runs while `apply` is still running: the unload waits for it to
/// return (step ③), however long that takes, then cleans up.
///
/// guarantee: core/D7, core/D27
#[tokio::test(start_paused = true)]
async fn unload_waits_for_apply_to_exit_before_cleanup() {
    let ctx = Ctx::root().unwrap();
    let log = Log::default();
    let slot = Slot::default();
    let entered = Arc::new(Notify::new());
    let exit = Arc::new(Notify::new());
    let view = ctx.plugin(LoadingSteps {
        log: log.clone(),
        view: slot.clone(),
        entered: entered.clone(),
        exit: exit.clone(),
    });
    assert!(slot.set(view.clone()).is_ok());
    within("apply to start", entered.notified()).await;

    let mut dispose = view.dispose();
    assert!(still_pending(&mut dispose).await, "apply has not returned");
    // Cancelled (②) while still loading; nothing cleaned up yet.
    assert_eq!(entries(&log), ["apply cancelled: Loading"]);

    exit.notify_one();
    within("dispose", dispose).await.unwrap();
    assert_eq!(
        entries(&log),
        [
            "apply cancelled: Loading",
            "apply exits",
            "cleanup: Unloading, cancelled true",
        ]
    );
    assert_eq!(view.state().state, FiberState::Disposed);
}

// ── K8: dropping a restart / update future ──────────────────────

/// Its first generation's cleanup tells `started`, then waits for `hold`
/// (when given) before counting itself; later generations clean up at
/// once. Records the config of every generation it applies.
#[derive(Clone)]
struct Held {
    config: u32,
    applied: Arc<Mutex<Vec<u32>>>,
    cleaned: Arc<AtomicUsize>,
    started: Arc<Notify>,
    hold: Option<Arc<Notify>>,
}

impl Held {
    fn new(hold: bool) -> Self {
        Self {
            config: 1,
            applied: Arc::default(),
            cleaned: Arc::default(),
            started: Arc::new(Notify::new()),
            hold: hold.then(|| Arc::new(Notify::new())),
        }
    }

    fn applied(&self) -> Vec<u32> {
        self.applied.lock().unwrap().clone()
    }

    fn cleaned(&self) -> usize {
        self.cleaned.load(Ordering::SeqCst)
    }
}

impl Plugin for Held {
    type Deps = ();

    fn name(&self) -> &str {
        "held"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        let first = {
            let mut applied = self.applied.lock().unwrap();
            applied.push(self.config);
            applied.len() == 1
        };
        let (started, cleaned) = (self.started.clone(), self.cleaned.clone());
        let hold = self.hold.clone().filter(|_| first);
        ctx.effect(move || async move {
            started.notify_one();
            if let Some(hold) = hold {
                hold.notified().await;
            }
            cleaned.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })?;
        Ok(())
    }
}

/// Mounts `plugin` as a factory plugin (config `u32`) and waits until it
/// is active.
async fn mount_factory(ctx: &Ctx, plugin: &Held) -> FiberView {
    let template = plugin.clone();
    let view = ctx.plugin_from(
        move |config: &u32| {
            let mut plugin = template.clone();
            plugin.config = *config;
            Ok(plugin)
        },
        1u32,
    );
    within("load", (&view).into_future()).await.unwrap();
    view
}

async fn reach_generation(view: &FiberView, generation: u64) {
    let mut rx = view.watch();
    within(
        "the next generation to be active",
        rx.wait_for(|s| s.generation == generation && s.state == FiberState::Active),
    )
    .await
    .expect("fiber dropped");
}

/// A restart whose future is dropped after the cleanup started still runs
/// that cleanup to the end and reloads: dropping the future only stops the
/// waiting.
///
/// guarantee: core/features.errors
#[tokio::test]
async fn a_restart_dropped_after_its_cleanup_started_still_completes() {
    let ctx = Ctx::root().unwrap();
    let plugin = Held::new(true);
    let view = ctx.plugin(plugin.clone());
    within("load", (&view).into_future()).await.unwrap();
    let generation = view.state().generation;

    let mut restart = view.restart();
    tokio::select! {
        result = &mut restart => panic!("restart finished before its cleanup: {result:?}"),
        () = plugin.started.notified() => {}
    }
    drop(restart);
    plugin.hold.as_ref().unwrap().notify_one();

    reach_generation(&view, generation + 1).await;
    assert_eq!(plugin.cleaned(), 1);
    assert_eq!(plugin.applied(), [1, 1]);
    assert!(view.state().error.is_none());
    within("dispose", view.dispose()).await.unwrap();
    assert_eq!(plugin.cleaned(), 2);
}

/// A restart future dropped before it was ever polled asks for nothing:
/// the fiber keeps its generation and nothing is cleaned up. A later
/// restart works as usual.
///
/// guarantee: core/features.errors
#[tokio::test(start_paused = true)]
async fn a_restart_dropped_before_its_first_poll_starts_nothing() {
    let ctx = Ctx::root().unwrap();
    let plugin = Held::new(false);
    let view = ctx.plugin(plugin.clone());
    within("load", (&view).into_future()).await.unwrap();
    let generation = view.state().generation;

    drop(view.restart());
    let mut idle = std::future::pending::<()>();
    assert!(still_pending(&mut idle).await);
    assert_eq!(view.state().generation, generation);
    assert_eq!(view.state().state, FiberState::Active);
    assert_eq!(plugin.cleaned(), 0);
    assert_eq!(plugin.applied(), [1]);

    within("restart", view.restart()).await.unwrap();
    assert_eq!(view.state().generation, generation + 1);
    assert_eq!(plugin.cleaned(), 1);
    assert_eq!(plugin.applied(), [1, 1]);
}

/// An update whose future is dropped after the old generation's cleanup
/// started still finishes that cleanup and loads the new config.
///
/// guarantee: core/D32, core/features.errors
#[tokio::test]
async fn an_update_dropped_after_its_cleanup_started_still_completes() {
    let ctx = Ctx::root().unwrap();
    let plugin = Held::new(true);
    let view = mount_factory(&ctx, &plugin).await;
    let generation = view.state().generation;

    let mut update = view.update(2u32);
    tokio::select! {
        result = &mut update => panic!("update finished before its cleanup: {result:?}"),
        () = plugin.started.notified() => {}
    }
    drop(update);
    plugin.hold.as_ref().unwrap().notify_one();

    reach_generation(&view, generation + 1).await;
    assert_eq!(plugin.cleaned(), 1);
    assert_eq!(plugin.applied(), [1, 2]);
    assert_eq!(view.current_config::<u32>().as_deref(), Some(&2));
    within("dispose", view.dispose()).await.unwrap();
    assert_eq!(plugin.cleaned(), 2);
}

/// An update future dropped before it was ever polled changes nothing: the
/// config is not stored and the fiber is not restarted.
///
/// guarantee: core/D32, core/features.errors
#[tokio::test(start_paused = true)]
async fn an_update_dropped_before_its_first_poll_changes_nothing() {
    let ctx = Ctx::root().unwrap();
    let plugin = Held::new(false);
    let view = mount_factory(&ctx, &plugin).await;
    let generation = view.state().generation;

    drop(view.update(2u32));
    let mut idle = std::future::pending::<()>();
    assert!(still_pending(&mut idle).await);
    assert_eq!(view.current_config::<u32>().as_deref(), Some(&1));
    assert_eq!(view.state().generation, generation);
    assert_eq!(view.state().state, FiberState::Active);
    assert_eq!(plugin.cleaned(), 0);
    assert_eq!(plugin.applied(), [1]);

    within("update", view.update(3u32)).await.unwrap();
    assert_eq!(view.current_config::<u32>().as_deref(), Some(&3));
    assert_eq!(plugin.applied(), [1, 3]);
    assert_eq!(plugin.cleaned(), 1);
}
