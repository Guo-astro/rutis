//! A generation the driver pre-cancelled (eviction, restart, update,
//! dispose, shutdown) is not a failed load: it unloads to `Pending` and the
//! queued intent or the dependency's return reloads it (#968). A returned
//! error is the plugin answering the cancellation and is not reported; a
//! panic is a bug and still goes to the error sink. A genuine failure — the
//! token was not cancelled by the driver — stays sticky `Failed`, and a
//! plugin that cancels its own token does not count.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, ErrorSink, FiberState, FiberStatusChanged,
    FiberView, PluginFactory, TypeKey,
};
use rutis::{Ctx, Disposer, Error};
use tokio::sync::Notify;

const LIMIT: Duration = Duration::from_secs(5);

struct Svc;

/// How the first generation's `apply` returns after the cancellation.
#[derive(Clone, Copy, PartialEq)]
enum OnCancel {
    Err,
    Ok,
    Panic,
}

/// Provides `Svc` and keeps its context and the binding's [`Disposer`], so a
/// test can withdraw the binding in place and provide it again without a new
/// provider generation.
struct Provider {
    ctx: Arc<Mutex<Option<Ctx>>>,
    disposer: Arc<Mutex<Option<Disposer>>>,
}

impl DynPlugin for Provider {
    fn name(&self) -> &str {
        "evicted-apply-provider"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.ctx.lock().unwrap() = Some(ctx.clone());
            let disposer = ctx.provide(Svc)?;
            *self.disposer.lock().unwrap() = Some(disposer);
            Ok(Effect::Done)
        })
    }
}

/// Requires `Svc`. Its first generation waits for the cancellation and then
/// returns `Err` (or `Ok`, the control); later generations return `Ok`.
struct Consumer {
    on_cancel: OnCancel,
    applies: Arc<AtomicUsize>,
    entered: Arc<Notify>,
}

impl DynPlugin for Consumer {
    fn name(&self) -> &str {
        "evicted-apply-consumer"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if self.applies.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.notify_one();
                ctx.cancelled().await;
                match self.on_cancel {
                    OnCancel::Err => {
                        return Err(Error::from(ErrorKind::PluginFailed(
                            "cancelled while waiting".into(),
                        )))
                    }
                    OnCancel::Panic => panic!("apply panicked after the cancellation"),
                    OnCancel::Ok => {}
                }
            }
            Ok(Effect::Done)
        })
    }
}

/// Requires `Svc` and loads at once: the no-eviction control.
struct Immediate(Arc<AtomicUsize>);

impl DynPlugin for Immediate {
    fn name(&self) -> &str {
        "evicted-apply-immediate"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Effect::Done)
        })
    }
}

/// Fails its first load at once, without waiting and without being
/// cancelled: a genuine failed load.
struct Failing(Arc<AtomicUsize>);

impl DynPlugin for Failing {
    fn name(&self) -> &str {
        "evicted-apply-failing"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(Error::from(ErrorKind::PluginFailed(
                "does not start".into(),
            )))
        })
    }
}

/// Cancels its own generation token and then fails. `Ctx` hands the plugin
/// the generation's token itself, not a child, so this is possible; the
/// driver did not cancel it, so the failure is still a failed load.
struct SelfCancel(Arc<AtomicUsize>);

impl DynPlugin for SelfCancel {
    fn name(&self) -> &str {
        "evicted-apply-self-cancel"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            ctx.cancellation_token().cancel();
            Err(Error::from(ErrorKind::PluginFailed(
                "I cancelled myself".into(),
            )))
        })
    }
}

struct ProviderSlots {
    view: FiberView,
    ctx: Arc<Mutex<Option<Ctx>>>,
    disposer: Arc<Mutex<Option<Disposer>>>,
}

async fn mount_provider(root: &Ctx) -> ProviderSlots {
    let ctx = Arc::new(Mutex::new(None));
    let disposer = Arc::new(Mutex::new(None));
    let view = root.plugin(Provider {
        ctx: ctx.clone(),
        disposer: disposer.clone(),
    });
    tokio::time::timeout(LIMIT, &view)
        .await
        .expect("provider loads")
        .expect("provider loads");
    ProviderSlots {
        view,
        ctx,
        disposer,
    }
}

/// Mounts the consumer and waits until its first `apply` runs.
async fn mount_waiting_consumer(root: &Ctx, on_cancel: OnCancel) -> (FiberView, Arc<AtomicUsize>) {
    let applies = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let view = root.plugin(Consumer {
        on_cancel,
        applies: applies.clone(),
        entered: entered.clone(),
    });
    tokio::time::timeout(LIMIT, entered.notified())
        .await
        .expect("first apply starts");
    (view, applies)
}

/// Withdraws the binding in place, as a provider can; the withdrawal cancels
/// the loading generation and waits for the consumer to stop.
async fn withdraw(provider: &ProviderSlots) {
    let disposer = provider.disposer.lock().unwrap().take().unwrap();
    disposer.dispose().await.unwrap();
}

/// Provides `Svc` again from the same provider generation.
fn provide_again(provider: &ProviderSlots, root: &Ctx) {
    provider
        .ctx
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .provide(Svc)
        .expect("provide again");
    assert!(root.get::<Svc>().is_some(), "the service is provided again");
}

async fn wait_for(view: &FiberView, state: FiberState) -> bool {
    tokio::time::timeout(LIMIT, async {
        let mut rx = view.watch();
        let _ = rx.wait_for(|snapshot| snapshot.state == state).await;
    })
    .await
    .is_ok()
}

/// Fails with the consumer's state, generation and error when it does not
/// reach `Active`.
async fn expect_active(view: &FiberView, what: &str) {
    if !wait_for(view, FiberState::Active).await {
        let snapshot = view.state();
        panic!(
            "{what}: never reached Active; state: {:?} (generation {}), error: {:?}",
            snapshot.state, snapshot.generation, snapshot.error
        );
    }
}

/// The status transitions of a fiber, as `FiberStatusChanged` events.
fn status_path(root: &Ctx, id: rutis::host::PluginId) -> Arc<Mutex<Vec<(FiberState, FiberState)>>> {
    let seen: Arc<Mutex<Vec<(FiberState, FiberState)>>> = Arc::default();
    let out = seen.clone();
    root.on::<FiberStatusChanged>(move |event, _call| {
        let out = out.clone();
        let event = (*event).clone();
        async move {
            if event.plugin_id == id {
                out.lock().unwrap().push((event.from, event.to));
            }
            Ok(None)
        }
    })
    .unwrap();
    seen
}

fn no_failed(path: &[(FiberState, FiberState)]) -> bool {
    path.iter().all(|(_, to)| *to != FiberState::Failed)
}

/// An error sink that collects what the fiber reports instead of printing it.
fn sink() -> (ErrorSink, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let errors: Arc<Mutex<Vec<Arc<ErrorKind>>>> = Arc::default();
    let out = errors.clone();
    let sink: ErrorSink = Arc::new(move |error| out.lock().unwrap().push(error));
    (sink, errors)
}

/// Waits until the event bus has delivered every queued call.
async fn drained(root: &Ctx) {
    for _ in 0..5_000 {
        if root.diagnostics().event_backlogs.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("the event backlog did not drain");
}

/// A consumer that awaits `ctx.cancelled()` in `apply` and returns `Err` when
/// released. Withdrawing the service in place cancels it; the generation is
/// the driver's to stop, so it unloads to `Pending`, and when the same
/// provider generation provides the service again the consumer reloads.
///
/// guarantee: core/D7
#[tokio::test]
async fn an_apply_error_after_eviction_returns_to_pending_and_reloads() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let (consumer, applies) = mount_waiting_consumer(&root, OnCancel::Err).await;
    assert_eq!(consumer.state().state, FiberState::Loading);

    withdraw(&provider).await;
    assert_eq!(consumer.state().state, FiberState::Pending);
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    provide_again(&provider, &root);
    expect_active(&consumer, "eviction, then the same provider provides again").await;
    assert_eq!(applies.load(Ordering::SeqCst), 2);

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// The same withdrawal when `apply` returns `Ok` after the cancellation:
/// unchanged from before the fix — the fiber is set `Active`, the queued
/// refresh unloads it to `Pending`, and the service's return reloads it.
///
/// guarantee: core/D7
#[tokio::test]
async fn an_apply_ok_after_eviction_returns_to_pending_and_reloads() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let (consumer, applies) = mount_waiting_consumer(&root, OnCancel::Ok).await;

    withdraw(&provider).await;
    assert_eq!(consumer.state().state, FiberState::Pending);

    provide_again(&provider, &root);
    expect_active(
        &consumer,
        "eviction, Ok from apply, provider provides again",
    )
    .await;
    assert_eq!(applies.load(Ordering::SeqCst), 2);

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// No eviction: the consumer loads normally.
///
/// guarantee: core/D7
#[tokio::test]
async fn a_consumer_loads_normally_without_eviction() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let applies = Arc::new(AtomicUsize::new(0));
    let consumer = root.plugin(Immediate(applies.clone()));
    expect_active(&consumer, "no eviction").await;
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// The dependency returns before the cancelled `apply` exits: the
/// re-checks the withdrawal queued still reload the consumer, and it loads
/// once.
///
/// guarantee: core/D7
#[tokio::test]
async fn a_service_returning_before_the_cancelled_apply_exits_reloads_once() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let (consumer, applies) = mount_waiting_consumer(&root, OnCancel::Err).await;

    let disposer = provider.disposer.lock().unwrap().take().unwrap();
    let withdrawal = tokio::spawn(async move { disposer.dispose().await });
    // The withdrawal marks the binding as being removed first; provide the
    // same generation's service again while the consumer's apply is still
    // on its way out.
    tokio::time::timeout(LIMIT, async {
        while root.get::<Svc>().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("service marked removed");
    provide_again(&provider, &root);
    tokio::time::timeout(LIMIT, withdrawal)
        .await
        .expect("withdrawal returns")
        .unwrap()
        .unwrap();

    expect_active(
        &consumer,
        "service returns before the cancelled apply exits",
    )
    .await;
    assert_eq!(applies.load(Ordering::SeqCst), 2);

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// A first `validate` that waits at the gate and then fails; later ones pass.
#[derive(Default)]
struct BlockingValidate {
    calls: AtomicUsize,
    entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl BlockingValidate {
    fn pass(&self) -> Result<(), Error> {
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
            return Ok(());
        }
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        if let Some(release) = self.release.lock().unwrap().take() {
            let _ = release.recv();
        }
        Err(Error::from(ErrorKind::PluginFailed(
            "validate rejected after the cancellation".into(),
        )))
    }
}

/// Requires `Svc`; its first `validate` waits for the cancellation and then
/// returns `Err`.
struct ValidateConsumer(Arc<BlockingValidate>);

impl DynPlugin for ValidateConsumer {
    fn name(&self) -> &str {
        "evicted-apply-validate-consumer"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn validate(&self) -> Result<(), Error> {
        self.0.pass()
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

/// The same gap one load step earlier: the withdrawal cancels the consumer
/// while `validate` runs, and the `Err` that `validate` returns must not be
/// recorded as a failed load either — `fail_load` is reached from `build`,
/// `validate` and `apply`.
///
/// guarantee: core/D7
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_validate_error_after_eviction_returns_to_pending_and_reloads() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let validate = Arc::new(BlockingValidate {
        calls: AtomicUsize::new(0),
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
    });
    let consumer = root.plugin(ValidateConsumer(validate.clone()));
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(LIMIT))
        .await
        .unwrap()
        .expect("validate entered");

    // Withdraw while `validate` is held; the withdrawal cancels this
    // generation and then waits for the consumer to stop.
    let disposer = provider.disposer.lock().unwrap().take().unwrap();
    let returned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let withdrawal = tokio::spawn({
        let returned = returned.clone();
        async move {
            let result = disposer.dispose().await;
            returned.store(true, Ordering::SeqCst);
            result
        }
    });
    tokio::time::timeout(LIMIT, async {
        while root.get::<Svc>().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("service marked removed");
    // The withdrawal has reached the consumer (it waits in `validate`).
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !returned.load(Ordering::SeqCst),
        "the withdrawal returned before its consumer stopped"
    );
    release_tx.send(()).unwrap();
    tokio::time::timeout(LIMIT, withdrawal)
        .await
        .expect("withdrawal returns")
        .unwrap()
        .unwrap();
    assert_eq!(consumer.state().state, FiberState::Pending);

    provide_again(&provider, &root);
    expect_active(&consumer, "validate error after eviction").await;
    assert_eq!(validate.calls.load(Ordering::SeqCst), 2);

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// A panic is a bug in the plugin, not a cooperative exit: when a
/// pre-cancelled generation's `apply` panics, the generation still unloads
/// to `Pending`, the panic goes to the error sink once, and the consumer
/// reloads when the dependency returns.
///
/// guarantee: core/D7
#[tokio::test]
async fn a_panic_after_eviction_goes_to_the_sink_and_the_consumer_reloads() {
    let (sink, errors) = sink();
    let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
    let provider = mount_provider(&root).await;
    let (consumer, applies) = mount_waiting_consumer(&root, OnCancel::Panic).await;

    withdraw(&provider).await;
    assert_eq!(consumer.state().state, FiberState::Pending);
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    let reported = errors.lock().unwrap().clone();
    assert_eq!(
        reported.len(),
        1,
        "the panic reaches the sink once: {reported:?}"
    );
    assert!(
        matches!(&*reported[0], ErrorKind::PluginFailed(_)),
        "the sink gets the panic as the plugin's error: {:?}",
        reported[0]
    );
    assert!(
        reported[0]
            .to_string()
            .contains("apply panicked after the cancellation"),
        "the sink gets the panic message: {}",
        reported[0]
    );

    provide_again(&provider, &root);
    expect_active(&consumer, "panic after eviction, provider provides again").await;
    assert_eq!(applies.load(Ordering::SeqCst), 2);

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// Restarting a consumer whose `apply` returns `Err` after the restart
/// pre-cancelled it: no `Failed` on the status path, and exactly one new
/// generation runs `apply` (the queued restart; nothing else reloads it).
///
/// guarantee: core/D7
#[tokio::test]
async fn restarting_a_cancelled_loading_consumer_runs_one_apply_without_failing() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let (consumer, applies) = mount_waiting_consumer(&root, OnCancel::Err).await;
    let path = status_path(&root, consumer.id);

    tokio::time::timeout(LIMIT, consumer.restart())
        .await
        .expect("restart returns")
        .unwrap();
    expect_active(&consumer, "consumer restarted").await;
    assert_eq!(applies.load(Ordering::SeqCst), 2);
    drained(&root).await;
    let path = path.lock().unwrap().clone();
    assert!(no_failed(&path), "a pre-cancelled load failed: {path:?}");

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// Builds the consumer for each generation, so `update` can restart it.
struct ConsumerFactory {
    on_cancel: OnCancel,
    applies: Arc<AtomicUsize>,
    entered: Arc<Notify>,
}

impl PluginFactory<()> for ConsumerFactory {
    fn name(&self) -> &str {
        "evicted-apply-consumer"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<Svc>()]
    }

    fn build(&self, _: &()) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Consumer {
            on_cancel: self.on_cancel,
            applies: self.applies.clone(),
            entered: self.entered.clone(),
        }))
    }
}

/// Updating a consumer whose `apply` returns `Err` after the update
/// pre-cancelled it: no `Failed` on the status path, and exactly one new
/// generation runs `apply`.
///
/// guarantee: core/D7
#[tokio::test]
async fn updating_a_cancelled_loading_consumer_runs_one_apply_without_failing() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let applies = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let consumer: FiberView = root.plugin_with(
        ConsumerFactory {
            on_cancel: OnCancel::Err,
            applies: applies.clone(),
            entered: entered.clone(),
        },
        (),
    );
    tokio::time::timeout(LIMIT, entered.notified())
        .await
        .expect("first apply starts");
    let path = status_path(&root, consumer.id);

    tokio::time::timeout(LIMIT, consumer.update(()))
        .await
        .expect("update returns")
        .unwrap();
    expect_active(&consumer, "consumer updated").await;
    assert_eq!(applies.load(Ordering::SeqCst), 2);
    drained(&root).await;
    let path = path.lock().unwrap().clone();
    assert!(no_failed(&path), "a pre-cancelled load failed: {path:?}");

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// Disposing a consumer whose `apply` returns `Err` after the disposal
/// pre-cancelled it: no `Failed` on the status path, no new `apply`, and it
/// ends `Disposed`.
///
/// guarantee: core/D7
#[tokio::test]
async fn disposing_a_cancelled_loading_consumer_disposes_without_a_new_apply() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let (consumer, applies) = mount_waiting_consumer(&root, OnCancel::Err).await;
    let path = status_path(&root, consumer.id);

    tokio::time::timeout(LIMIT, consumer.dispose())
        .await
        .expect("dispose returns")
        .unwrap();
    assert_eq!(consumer.state().state, FiberState::Disposed);
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    drained(&root).await;
    let path = path.lock().unwrap().clone();
    assert!(no_failed(&path), "a pre-cancelled load failed: {path:?}");

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// Shutting down a consumer whose `apply` returns `Err` after the shutdown
/// pre-cancelled it: no `Failed` on the status path, no new `apply`, and it
/// ends `Disposed`.
///
/// guarantee: core/D7
#[tokio::test]
async fn shutting_down_a_cancelled_loading_consumer_disposes_without_a_new_apply() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let (consumer, applies) = mount_waiting_consumer(&root, OnCancel::Err).await;
    let path = status_path(&root, consumer.id);

    tokio::time::timeout(LIMIT, consumer.shutdown())
        .await
        .expect("shutdown returns")
        .unwrap();
    assert_eq!(consumer.state().state, FiberState::Disposed);
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    drained(&root).await;
    let path = path.lock().unwrap().clone();
    assert!(no_failed(&path), "a pre-cancelled load failed: {path:?}");

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// A genuine failed load — nothing cancelled the generation — stays sticky
/// `Failed`, and the same dependency's return (a duplicate notification for
/// an unchanged dependency set) does not reload it (#327).
///
/// guarantee: core/D7, core/features.errors.failed-retry
#[tokio::test]
async fn a_genuine_apply_failure_stays_failed() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let applies = Arc::new(AtomicUsize::new(0));
    let consumer = root.plugin(Failing(applies.clone()));
    tokio::time::timeout(LIMIT, &consumer)
        .await
        .expect("the load fails")
        .expect_err("the load fails");
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    // Withdraw and provide the same binding again: the notification names
    // the same dependency set, so the Failed load is not retried.
    let disposer = provider.disposer.lock().unwrap().take().unwrap();
    disposer.dispose().await.unwrap();
    provide_again(&provider, &root);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    root.shutdown().await.unwrap();
    drop(provider.view);
}

/// A plugin that cancels its own generation token and returns `Err` is not
/// a driver-cancelled generation: it ends sticky `Failed` and a duplicate
/// notification does not reload it. (The `Ctx` it gets carries the
/// generation's token itself, so this is possible.)
///
/// guarantee: core/D7, core/features.errors.failed-retry
#[tokio::test]
async fn a_plugin_that_cancels_its_own_token_stays_failed() {
    let root = Ctx::root().unwrap();
    let provider = mount_provider(&root).await;
    let applies = Arc::new(AtomicUsize::new(0));
    let consumer = root.plugin(SelfCancel(applies.clone()));
    tokio::time::timeout(LIMIT, &consumer)
        .await
        .expect("the load fails")
        .expect_err("the load fails");
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    let disposer = provider.disposer.lock().unwrap().take().unwrap();
    disposer.dispose().await.unwrap();
    provide_again(&provider, &root);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    root.shutdown().await.unwrap();
    drop(provider.view);
}
