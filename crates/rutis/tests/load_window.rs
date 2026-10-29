//! The window at the start of a load: a dispose or restart registered
//! there must reach the generation that starts (#619), and a dependency
//! withdrawn or replaced there must stop the consumer (#620).
//!
//! The gate tests block a plugin callback (`validate`) on a worker thread,
//! so they run on the multi-thread runtime only. The window between the
//! gate and the recording of dependencies runs no plugin code, so no test
//! stops a load there: the second gate check covers it (`load` in
//! `fiber.rs`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, FiberView, PluginFactory, TypeKey,
};
use rutis::{Ctx, Error};
use tokio::sync::Notify;

const LIMIT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct ProbeState {
    applies: AtomicUsize,
    cancelled: AtomicUsize,
    cleanup_started: Notify,
    release: Notify,
}

/// Every generation waits for `ctx.cancelled()`. With `hold`, the first
/// generation's cleanup waits for `release`.
struct Probe {
    state: Arc<ProbeState>,
    hold: bool,
}

impl DynPlugin for Probe {
    fn name(&self) -> &str {
        "load-window-probe"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let first = self.state.applies.fetch_add(1, Ordering::SeqCst) == 0;
            ctx.cancelled().await;
            self.state.cancelled.fetch_add(1, Ordering::SeqCst);
            if first && self.hold {
                let state = self.state.clone();
                return Ok(Effect::AsyncDisposer(Box::new(move || {
                    Box::pin(async move {
                        state.cleanup_started.notify_one();
                        state.release.notified().await;
                        Ok(())
                    })
                })));
            }
            Ok(Effect::Done)
        })
    }
}

/// A dispose right after mounting: the queued first load must not start a
/// generation the dispose has not cancelled.
async fn dispose_right_after_mounting() {
    let root = Ctx::root().unwrap();
    let state = Arc::new(ProbeState::default());
    let view = root.plugin(Probe {
        state: state.clone(),
        hold: false,
    });
    tokio::time::timeout(LIMIT, view.dispose())
        .await
        .expect("dispose returns")
        .unwrap();
    assert_eq!(view.state().state, FiberState::Disposed);
    // A generation that did start was cancelled and ended.
    assert_eq!(
        state.applies.load(Ordering::SeqCst),
        state.cancelled.load(Ordering::SeqCst)
    );
}

/// guarantee: core/D6, core/D7
#[tokio::test]
async fn a_dispose_right_after_mounting_ends_on_current_thread() {
    dispose_right_after_mounting().await;
}

/// guarantee: core/D6, core/D7
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dispose_right_after_mounting_ends_on_multi_thread() {
    dispose_right_after_mounting().await;
}

/// A dispose while a restart unloads the old generation: the restart does
/// not start a new generation, and the dispose ends.
async fn dispose_during_a_restart() {
    let root = Ctx::root().unwrap();
    let state = Arc::new(ProbeState::default());
    let view = root.plugin(Probe {
        state: state.clone(),
        hold: true,
    });
    tokio::time::timeout(LIMIT, async {
        let mut rx = view.watch();
        rx.wait_for(|s| s.state == FiberState::Loading)
            .await
            .unwrap();
    })
    .await
    .expect("first generation loads");
    let restart = tokio::spawn({
        let view = view.clone();
        async move { view.restart().await }
    });
    // The restart is unloading the first generation.
    tokio::time::timeout(LIMIT, state.cleanup_started.notified())
        .await
        .expect("cleanup starts");
    let dispose = view.dispose();
    state.release.notify_one();
    tokio::time::timeout(LIMIT, dispose)
        .await
        .expect("dispose returns")
        .unwrap();
    // The dispose came first: the restart reports what `restart()` reports
    // once a dispose is registered.
    let restarted = tokio::time::timeout(LIMIT, restart)
        .await
        .expect("restart ends")
        .unwrap();
    assert!(
        matches!(&restarted, Err(e) if matches!(**e, ErrorKind::InactiveEffect)),
        "{restarted:?}"
    );
    assert_eq!(state.applies.load(Ordering::SeqCst), 1);
    assert_eq!(view.state().state, FiberState::Disposed);
}

/// guarantee: core/D6, core/D7
#[tokio::test]
async fn a_dispose_during_a_restart_ends_on_current_thread() {
    dispose_during_a_restart().await;
}

/// guarantee: core/D6, core/D7
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dispose_during_a_restart_ends_on_multi_thread() {
    dispose_during_a_restart().await;
}

#[derive(Debug)]
struct Svc;

/// A gate step the test controls: `validate` tells the test it was called
/// and blocks until released (first call only).
#[derive(Default)]
struct Gate {
    entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl Gate {
    fn new() -> (
        Arc<Self>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let gate = Arc::new(Gate {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(Some(release_rx)),
        });
        (gate, entered_rx, release_tx)
    }

    fn pass(&self) {
        let Some(entered) = self.entered.lock().unwrap().take() else {
            return;
        };
        let _ = entered.send(());
        if let Some(release) = self.release.lock().unwrap().take() {
            let _ = release.recv();
        }
    }
}

/// Requires `Svc`; its first `validate` waits at the gate.
struct Consumer {
    gate: Arc<Gate>,
    applies: Arc<AtomicUsize>,
}

impl DynPlugin for Consumer {
    fn name(&self) -> &str {
        "load-window-consumer"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn validate(&self) -> Result<(), Error> {
        self.gate.pass();
        Ok(())
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.applies.fetch_add(1, Ordering::SeqCst);
            ctx.cancelled().await;
            Ok(Effect::Done)
        })
    }
}

/// Provides `Svc` and hands the disposer of that one service out.
struct Provider {
    disposer: Arc<Mutex<Option<rutis::Disposer>>>,
}

impl DynPlugin for Provider {
    fn name(&self) -> &str {
        "load-window-provider"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.disposer.lock().unwrap() = Some(ctx.provide(Svc)?);
            Ok(Effect::Done)
        })
    }
}

/// Withdraws the service while the consumer waits at the gate in
/// `validate`, then releases it: the withdrawal returns only after the
/// consumer stopped, and the consumer ends Pending.
async fn withdrawn_at_the_gate(
    root: &Ctx,
    view: &rutis::host::FiberView,
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
    disposer: rutis::Disposer,
) {
    tokio::task::spawn_blocking(move || entered.recv_timeout(LIMIT))
        .await
        .unwrap()
        .expect("consumer reaches validate");
    let returned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let withdrawal = tokio::spawn({
        let returned = returned.clone();
        async move {
            let result = disposer.dispose().await;
            returned.store(true, Ordering::SeqCst);
            result
        }
    });
    // The withdrawal has marked the service removed.
    tokio::time::timeout(LIMIT, async {
        while root.get::<Svc>().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("service marked removed");
    // The consumer is held in validate: the withdrawal must wait for it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !returned.load(Ordering::SeqCst),
        "withdrawal returned before its consumer stopped"
    );
    release.send(()).unwrap();
    tokio::time::timeout(LIMIT, withdrawal)
        .await
        .expect("withdrawal returns")
        .unwrap()
        .unwrap();
    assert_eq!(view.state().state, FiberState::Pending);
}

/// A first load past the gate, before its dependencies are recorded.
///
/// guarantee: core/D14
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dependency_withdrawn_at_the_gate_of_a_first_load_stops_the_consumer() {
    let root = Ctx::root().unwrap();
    let disposer = Arc::new(Mutex::new(None));
    let provider = root.plugin(Provider {
        disposer: disposer.clone(),
    });
    tokio::time::timeout(LIMIT, &provider)
        .await
        .unwrap()
        .unwrap();
    let (gate, entered, release) = Gate::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(Consumer {
        gate,
        applies: applies.clone(),
    });
    let disposer = disposer.lock().unwrap().take().unwrap();
    withdrawn_at_the_gate(&root, &view, entered, release, disposer).await;
    // The withdrawal came after the second gate check, so the generation
    // ran, with the token the withdrawal cancelled.
    assert_eq!(applies.load(Ordering::SeqCst), 1);
}

/// A Failed consumer retried because its provider got a new generation,
/// past the gate, before its dependencies are recorded.
///
/// guarantee: core/D14
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dependency_withdrawn_at_the_gate_of_a_retry_stops_the_consumer() {
    let root = Ctx::root().unwrap();
    let disposer = Arc::new(Mutex::new(None));
    let provider = root.plugin(Provider {
        disposer: disposer.clone(),
    });
    tokio::time::timeout(LIMIT, &provider)
        .await
        .unwrap()
        .unwrap();
    let (gate, entered, release) = Gate::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(RetryConsumer {
        gate,
        applies: applies.clone(),
    });
    let settled = tokio::time::timeout(LIMIT, &view).await.unwrap();
    assert!(settled.is_err());
    assert_eq!(view.state().state, FiberState::Failed);
    // The provider gets a new generation: the Failed consumer retries.
    tokio::time::timeout(LIMIT, provider.restart())
        .await
        .unwrap()
        .unwrap();
    let disposer = disposer.lock().unwrap().take().unwrap();
    withdrawn_at_the_gate(&root, &view, entered, release, disposer).await;
    // The retry ran with the token the withdrawal cancelled.
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

/// Requires `Svc`; the first generation fails at once, the retry's
/// `validate` waits at the gate.
struct RetryConsumer {
    gate: Arc<Gate>,
    applies: Arc<AtomicUsize>,
}

impl DynPlugin for RetryConsumer {
    fn name(&self) -> &str {
        "load-window-retry-consumer"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn validate(&self) -> Result<(), Error> {
        if self.applies.load(Ordering::SeqCst) > 0 {
            self.gate.pass();
        }
        Ok(())
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if self.applies.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(Error::from(ErrorKind::PluginFailed("first fails".into())));
            }
            ctx.cancelled().await;
            Ok(Effect::Done)
        })
    }
}

type Slot = Arc<Mutex<Option<rutis::Disposer>>>;

/// Mounts a provider and returns the disposer of its service.
async fn mounted<P: DynPlugin>(root: &Ctx, plugin: impl FnOnce(Slot) -> P) -> rutis::Disposer {
    let disposer = Slot::default();
    let view = root.plugin(plugin(disposer.clone()));
    tokio::time::timeout(LIMIT, &view).await.unwrap().unwrap();
    let taken = disposer.lock().unwrap().take();
    taken.expect("provided")
}

// ── Restart and update called before the first load ────────────────────

/// Polls `future` once, here, before anything else runs on this task, and
/// returns it to be awaited later.
async fn polled_once<T>(mut future: BoxFuture<'static, T>) -> BoxFuture<'static, T> {
    let mut done = false;
    std::future::poll_fn(|cx| {
        done = future.as_mut().poll(cx).is_ready();
        std::task::Poll::Ready(())
    })
    .await;
    assert!(!done, "it waits for the driver");
    future
}

/// Waits until `applies` generations have started.
async fn started(state: &ProbeState, applies: usize) {
    tokio::time::timeout(LIMIT, async {
        while state.applies.load(Ordering::SeqCst) < applies {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the generation starts");
}

/// A restart right after mounting: the first generation starts cancelled,
/// so the queued restart is taken up and starts the second one. The
/// restart returns once that generation's `apply` does; here, on the
/// dispose's cancellation.
async fn restart_right_after_mounting() {
    let root = Ctx::root().unwrap();
    let state = Arc::new(ProbeState::default());
    let view = root.plugin(Probe {
        state: state.clone(),
        hold: false,
    });
    // Called before the driver ran the first load.
    let restart = tokio::spawn(polled_once(view.restart()).await);
    started(&state, 2).await;
    assert_eq!(state.applies.load(Ordering::SeqCst), 2);
    assert_eq!(state.cancelled.load(Ordering::SeqCst), 1);
    tokio::time::timeout(LIMIT, view.dispose())
        .await
        .expect("dispose returns")
        .unwrap();
    tokio::time::timeout(LIMIT, restart)
        .await
        .expect("restart returns")
        .unwrap()
        .unwrap();
}

/// guarantee: core/D7
#[tokio::test]
async fn a_restart_right_after_mounting_is_taken_up_on_current_thread() {
    restart_right_after_mounting().await;
}

/// Builds a [`Probe`] for any config.
struct ProbeFactory(Arc<ProbeState>, Vec<TypeKey>);

impl PluginFactory<u32> for ProbeFactory {
    fn name(&self) -> &str {
        "load-window-probe-factory"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.1.clone()
    }

    fn build(&self, _: &u32) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Probe {
            state: self.0.clone(),
            hold: false,
        }))
    }
}

/// An update right after mounting, like a restart.
async fn update_right_after_mounting() {
    let root = Ctx::root().unwrap();
    let state = Arc::new(ProbeState::default());
    let view = root.plugin_with(ProbeFactory(state.clone(), Vec::new()), 1u32);
    let update = tokio::spawn(polled_once(view.update(2u32)).await);
    started(&state, 2).await;
    assert_eq!(state.applies.load(Ordering::SeqCst), 2);
    assert_eq!(state.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(view.current_config::<u32>().as_deref(), Some(&2));
    tokio::time::timeout(LIMIT, view.dispose())
        .await
        .expect("dispose returns")
        .unwrap();
    tokio::time::timeout(LIMIT, update)
        .await
        .expect("update returns")
        .unwrap()
        .unwrap();
}

/// guarantee: core/D7
#[tokio::test]
async fn an_update_right_after_mounting_is_taken_up_on_current_thread() {
    update_right_after_mounting().await;
}

// ── A dispose registered behind a queued restart or refresh ────────────

type Errors = Arc<Mutex<Vec<Arc<ErrorKind>>>>;

fn root_collecting() -> (Ctx, Errors) {
    let errors = Errors::default();
    let to = errors.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |e: Arc<ErrorKind>| to.lock().unwrap().push(e)),
    );
    (root, errors)
}

/// Its first generation waits for `go` (ignoring cancellation), then
/// returns a cleanup that fails. With `needs`, it requires `Svc`.
struct Held {
    go: Arc<Notify>,
    applies: Arc<AtomicUsize>,
    needs: bool,
}

impl DynPlugin for Held {
    fn name(&self) -> &str {
        "load-window-held"
    }

    fn injects(&self) -> Vec<TypeKey> {
        if self.needs {
            vec![TypeKey::of::<Svc>()]
        } else {
            Vec::new()
        }
    }

    fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.applies.fetch_add(1, Ordering::SeqCst);
            self.go.notified().await;
            Ok(Effect::Disposer(Box::new(|| {
                Err(Error::from(ErrorKind::PluginFailed("cleanup boom".into())))
            })))
        })
    }
}

async fn reach(view: &FiberView, state: FiberState) {
    tokio::time::timeout(LIMIT, async {
        let mut rx = view.watch();
        rx.wait_for(|s| s.state == state).await.unwrap();
    })
    .await
    .expect("state reached");
}

/// Waits until `apply` of its first generation runs: past the second
/// dependency check, so a withdrawal from here on finds it loading.
/// Loading alone is set before that check: a withdrawal there sends the
/// load back to Pending without running `apply`.
async fn applying(applies: &AtomicUsize) {
    tokio::time::timeout(LIMIT, async {
        while applies.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("apply runs");
}

fn assert_cleanup_error(result: Result<(), Arc<ErrorKind>>) {
    let error = result.expect_err("dispose reports the cleanup error");
    assert!(error.to_string().contains("cleanup boom"), "{error}");
}

/// A restart queued behind the running load, then a dispose: the restart
/// does not happen and reports `InactiveEffect`, and the cleanup error
/// reaches `dispose()`, not the error sink.
async fn restart_queued_before_a_dispose() {
    let (root, errors) = root_collecting();
    let go = Arc::new(Notify::new());
    let applies = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(Held {
        go: go.clone(),
        applies: applies.clone(),
        needs: false,
    });
    reach(&view, FiberState::Loading).await;
    // Polled once: it registers and queues its intent.
    let restart = polled_once(view.restart()).await;
    let dispose = view.dispose();
    go.notify_one();
    let restarted = tokio::time::timeout(LIMIT, restart)
        .await
        .expect("restart returns");
    assert!(
        matches!(&restarted, Err(e) if matches!(**e, ErrorKind::InactiveEffect)),
        "{restarted:?}"
    );
    assert_cleanup_error(
        tokio::time::timeout(LIMIT, dispose)
            .await
            .expect("dispose returns"),
    );
    assert!(
        errors.lock().unwrap().is_empty(),
        "{:?}",
        errors.lock().unwrap()
    );
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    assert_eq!(view.state().state, FiberState::Disposed);
}

/// guarantee: core/D6, core/D7
#[tokio::test]
async fn a_restart_queued_before_a_dispose_leaves_the_cleanup_to_it_on_current_thread() {
    restart_queued_before_a_dispose().await;
}

/// guarantee: core/D6, core/D7
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_queued_before_a_dispose_leaves_the_cleanup_to_it_on_multi_thread() {
    restart_queued_before_a_dispose().await;
}

/// A dependency refresh queued behind the running load (a withdrawal of
/// its service), then a dispose: the consumer stops before the withdrawal
/// returns, and the cleanup error reaches `dispose()`, not the error sink.
async fn refresh_queued_before_a_dispose() {
    let (root, errors) = root_collecting();
    let service = mounted(&root, |disposer| Provider { disposer }).await;
    let go = Arc::new(Notify::new());
    let applies = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(Held {
        go: go.clone(),
        applies: applies.clone(),
        needs: true,
    });
    applying(&applies).await;
    let withdrawal = tokio::spawn(service.dispose());
    tokio::time::timeout(LIMIT, async {
        while root.get::<Svc>().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("service marked removed");
    // The withdrawal queues the consumer's refresh right after marking.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let dispose = view.dispose();
    go.notify_one();
    tokio::time::timeout(LIMIT, withdrawal)
        .await
        .expect("withdrawal returns")
        .unwrap()
        .unwrap();
    assert_cleanup_error(
        tokio::time::timeout(LIMIT, dispose)
            .await
            .expect("dispose returns"),
    );
    assert!(
        errors.lock().unwrap().is_empty(),
        "{:?}",
        errors.lock().unwrap()
    );
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    assert_eq!(view.state().state, FiberState::Disposed);
}

/// guarantee: core/D6, core/D14
#[tokio::test]
async fn a_refresh_queued_before_a_dispose_leaves_the_cleanup_to_it_on_current_thread() {
    refresh_queued_before_a_dispose().await;
}

/// guarantee: core/D6, core/D14
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_queued_before_a_dispose_leaves_the_cleanup_to_it_on_multi_thread() {
    refresh_queued_before_a_dispose().await;
}

// ── More than one thing queued ahead of the dispose ────────────────────

/// Two restarts queued behind the running load, then a dispose: neither
/// restart happens, both report `InactiveEffect` (the second finds the
/// fiber already Disposed), and the cleanup error reaches `dispose()`.
async fn two_restarts_queued_before_a_dispose() {
    let (root, errors) = root_collecting();
    let go = Arc::new(Notify::new());
    let applies = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(Held {
        go: go.clone(),
        applies: applies.clone(),
        needs: false,
    });
    reach(&view, FiberState::Loading).await;
    let first = polled_once(view.restart()).await;
    let second = polled_once(view.restart()).await;
    let dispose = view.dispose();
    go.notify_one();
    for restart in [first, second] {
        let restarted = tokio::time::timeout(LIMIT, restart)
            .await
            .expect("restart returns");
        assert!(
            matches!(&restarted, Err(e) if matches!(**e, ErrorKind::InactiveEffect)),
            "{restarted:?}"
        );
    }
    assert_cleanup_error(
        tokio::time::timeout(LIMIT, dispose)
            .await
            .expect("dispose returns"),
    );
    assert!(
        errors.lock().unwrap().is_empty(),
        "{:?}",
        errors.lock().unwrap()
    );
    assert_eq!(applies.load(Ordering::SeqCst), 1);
}

/// guarantee: core/D6, core/D7
#[tokio::test]
async fn two_restarts_queued_before_a_dispose_both_report_it_on_current_thread() {
    two_restarts_queued_before_a_dispose().await;
}

/// guarantee: core/D6, core/D7
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_restarts_queued_before_a_dispose_both_report_it_on_multi_thread() {
    two_restarts_queued_before_a_dispose().await;
}

/// A restart queued behind a refresh that unloads for a dispose: the
/// restart finds the fiber Disposed and reports `InactiveEffect`, not the
/// dispose's cleanup error.
async fn restart_behind_a_refresh_that_disposes() {
    let (root, errors) = root_collecting();
    let service = mounted(&root, |disposer| Provider { disposer }).await;
    let go = Arc::new(Notify::new());
    let applies = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(Held {
        go: go.clone(),
        applies: applies.clone(),
        needs: true,
    });
    applying(&applies).await;
    let withdrawal = tokio::spawn(service.dispose());
    tokio::time::timeout(LIMIT, async {
        while root.get::<Svc>().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("service marked removed");
    // The withdrawal queues the consumer's refresh right after marking.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let restart = polled_once(view.restart()).await;
    let dispose = view.dispose();
    go.notify_one();
    tokio::time::timeout(LIMIT, withdrawal)
        .await
        .expect("withdrawal returns")
        .unwrap()
        .unwrap();
    let restarted = tokio::time::timeout(LIMIT, restart)
        .await
        .expect("restart returns");
    assert!(
        matches!(&restarted, Err(e) if matches!(**e, ErrorKind::InactiveEffect)),
        "{restarted:?}"
    );
    assert_cleanup_error(
        tokio::time::timeout(LIMIT, dispose)
            .await
            .expect("dispose returns"),
    );
    assert!(
        errors.lock().unwrap().is_empty(),
        "{:?}",
        errors.lock().unwrap()
    );
    assert_eq!(applies.load(Ordering::SeqCst), 1);
}

/// guarantee: core/D6, core/D7
#[tokio::test]
async fn a_restart_behind_a_refresh_that_disposes_reports_it_on_current_thread() {
    restart_behind_a_refresh_that_disposes().await;
}

/// guarantee: core/D6, core/D7
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_behind_a_refresh_that_disposes_reports_it_on_multi_thread() {
    restart_behind_a_refresh_that_disposes().await;
}
