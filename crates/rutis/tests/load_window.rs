//! The window at the start of a load: a dispose registered there must not
//! let a new generation start (#619), and a dependency withdrawn there must
//! stop the consumer (#620).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, TypeKey};
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
    let _ = tokio::time::timeout(LIMIT, restart)
        .await
        .expect("restart ends");
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
