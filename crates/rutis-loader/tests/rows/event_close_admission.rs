//! Close-admission races for the listener marker (#1025): a listener selected
//! before a subtree's marker pass must be skipped once the marker commits, and
//! a call admitted before the commit must still drain.
//!
//! guarantee: events/lifecycle
//!
//! The dispatch hook runs after selection and before `Entry::start`, so a
//! shutdown called from it is a deterministic race, not a timing guess.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rutis::events::SyncCall;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, FiberView};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error, Event};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Ping(i64);

impl Event for Ping {
    const NAME: &'static str = "t/close-admission";
    type Value = String;
}

struct Apply<F>(&'static str, F);

impl<F> DynPlugin for Apply<F>
where
    F: Fn(&Ctx) -> Result<(), Error> + Send + Sync + 'static,
{
    fn name(&self) -> &str {
        self.0
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move { (self.1)(ctx).map(|()| Effect::Done) })
    }
}

type Shutdown = Arc<Mutex<Option<BoxFuture<'static, Result<(), Arc<ErrorKind>>>>>>;

fn listener(
    calls: Arc<AtomicUsize>,
) -> impl for<'a> Fn(Arc<Ping>, SyncCall<'a, Ping>) -> Result<Option<String>, Error>
       + Send
       + Sync
       + 'static {
    move |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some("heard".to_string()))
    }
}

fn counter() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

/// A child whose `apply` runs `register` on its own context.
async fn child(
    root: &Ctx,
    name: &'static str,
    register: impl Fn(&Ctx) -> Result<(), Error> + Send + Sync + 'static,
) -> FiberView {
    let view = root.plugin(Apply(name, register));
    (&view).await.unwrap();
    view
}

/// Shut `view` down once, from inside the first `Ping` dispatch, after its
/// listeners were selected and before any `Entry::start`.
fn close_on_first_ping(root: &Ctx, view: FiberView) -> (Shutdown, Arc<Notify>) {
    let shutdown: Shutdown = Arc::default();
    let fired = Arc::new(Notify::new());
    let seen = Arc::new(AtomicBool::new(true));
    let slot = shutdown.clone();
    let notify = fired.clone();
    root.events().on_selected_for_testing(move |name, event| {
        if name == Ping::NAME
            && event.downcast_ref::<Ping>().is_some()
            && seen.swap(false, Ordering::SeqCst)
        {
            *slot.lock().unwrap() = Some(view.shutdown());
            notify.notify_one();
        }
    });
    (shutdown, fired)
}

async fn take_shutdown(shutdown: &Shutdown) {
    let future = shutdown.lock().unwrap().take();
    if let Some(future) = future {
        future.await.unwrap();
    }
}

async fn run_mode(mode: &str) {
    let root = Ctx::root().unwrap();
    let calls = counter();
    let heard = calls.clone();
    let view = child(&root, "closed-before-start", move |ctx| {
        ctx.on_sync::<Ping>(listener(heard.clone()))?;
        Ok(())
    })
    .await;
    let (shutdown, fired) = close_on_first_ping(&root, view);

    match mode {
        "emit" => {
            root.emit(Ping(0)).unwrap();
            fired.notified().await;
            // `backlogs` is crate-private; the hook has already selected the
            // listener, and `Entry::start` is the next step of the same task.
            // Give the dispatch a bounded window to reach it.
            // clock: widen — a bounded window for the dispatch
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        "serial" => {
            root.serial(Ping(0)).await.unwrap();
        }
        "parallel" => {
            root.parallel(Ping(0)).await.unwrap();
        }
        "bail" => {
            root.bail(Ping(0)).unwrap();
        }
        "waterfall" => {
            root.waterfall(Ping(0)).await.unwrap();
        }
        other => panic!("unknown mode {other}"),
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "{mode}: a listener selected before the marker pass must not start after it"
    );
    take_shutdown(&shutdown).await;
    root.shutdown().await.unwrap();
}

/// The marker pass commits after selection and before `Entry::start`; no mode
/// admits the selected listener.
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subtree_shutdown_before_start_skips_listener_in_every_mode() {
    for mode in ["emit", "serial", "parallel", "bail", "waterfall"] {
        run_mode(mode).await;
    }
}

/// Closing one child does not silence its sibling, and a nested child is
/// covered by the same marker pass.
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subtree_shutdown_skips_descendant_but_not_sibling() {
    let root = Ctx::root().unwrap();
    let closed_calls = counter();
    let nested_calls = counter();
    let sibling_calls = counter();

    let closed_slot: Arc<Mutex<Option<Ctx>>> = Arc::default();
    let closed_out = closed_slot.clone();
    let closed = child(&root, "closed", {
        let closed_calls = closed_calls.clone();
        move |ctx| {
            ctx.on_sync::<Ping>(listener(closed_calls.clone()))?;
            *closed_out.lock().unwrap() = Some(ctx.clone());
            Ok(())
        }
    })
    .await;

    let closed_ctx = closed_slot.lock().unwrap().take().unwrap();
    let nested_calls_for_child = nested_calls.clone();
    let nested = closed_ctx.plugin(Apply("nested", move |ctx: &Ctx| {
        ctx.on_sync::<Ping>(listener(nested_calls_for_child.clone()))?;
        Ok(())
    }));
    (&nested).await.unwrap();

    child(&root, "sibling", {
        let sibling_calls = sibling_calls.clone();
        move |ctx| {
            ctx.on_sync::<Ping>(listener(sibling_calls.clone()))?;
            Ok(())
        }
    })
    .await;

    let (shutdown, fired) = close_on_first_ping(&root, closed);
    root.emit(Ping(0)).unwrap();
    fired.notified().await;
    // clock: weak-negative — after the close, listeners do not run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(closed_calls.load(Ordering::SeqCst), 0, "closed child ran");
    assert_eq!(nested_calls.load(Ordering::SeqCst), 0, "nested child ran");
    assert_eq!(
        sibling_calls.load(Ordering::SeqCst),
        1,
        "the sibling must still hear the event"
    );

    take_shutdown(&shutdown).await;
    root.shutdown().await.unwrap();
}

/// Root shutdown uses the same marker pass: root and child listeners selected
/// before the commit are skipped.
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_shutdown_before_start_skips_root_and_child_listeners() {
    let root = Ctx::root().unwrap();
    let root_calls = counter();
    let child_calls = counter();
    root.on_sync::<Ping>(listener(root_calls.clone())).unwrap();
    child(&root, "child", {
        let child_calls = child_calls.clone();
        move |ctx| {
            ctx.on_sync::<Ping>(listener(child_calls.clone()))?;
            Ok(())
        }
    })
    .await;

    let shutdown: Shutdown = Arc::default();
    let fired = Arc::new(Notify::new());
    let seen = Arc::new(AtomicBool::new(true));
    let slot = shutdown.clone();
    let notify = fired.clone();
    let root_for_hook = root.clone();
    root.events().on_selected_for_testing(move |name, event| {
        if name == Ping::NAME
            && event.downcast_ref::<Ping>().is_some()
            && seen.swap(false, Ordering::SeqCst)
        {
            *slot.lock().unwrap() = Some(root_for_hook.shutdown());
            notify.notify_one();
        }
    });

    root.emit(Ping(0)).unwrap();
    fired.notified().await;
    // clock: weak-negative — after the close, listeners do not run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(root_calls.load(Ordering::SeqCst), 0, "root listener ran");
    assert_eq!(child_calls.load(Ordering::SeqCst), 0, "child listener ran");
    take_shutdown(&shutdown).await;
}

/// A call admitted before the commit keeps running and is drained by the
/// subtree shutdown; the marker only stops new calls.
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_listener_drains_after_subtree_shutdown() {
    let root = Ctx::root().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let done = Arc::new(AtomicBool::new(false));

    let view = child(&root, "in-flight", {
        let started = started.clone();
        let release = release.clone();
        let done = done.clone();
        move |ctx| {
            ctx.on::<Ping>({
                let started = started.clone();
                let release = release.clone();
                let done = done.clone();
                move |_, _| {
                    let started = started.clone();
                    let release = release.clone();
                    let done = done.clone();
                    async move {
                        started.notify_one();
                        release.notified().await;
                        done.store(true, Ordering::SeqCst);
                        Ok(Some("drained".to_string()))
                    }
                }
            })?;
            Ok(())
        }
    })
    .await;

    let dispatch = {
        let root = root.clone();
        tokio::spawn(async move { root.parallel(Ping(0)).await })
    };
    started.notified().await;

    let shutdown = view.shutdown();
    assert!(
        !done.load(Ordering::SeqCst),
        "the listener still waits for its release"
    );
    release.notify_one();
    dispatch.await.unwrap().unwrap();
    shutdown.await.unwrap();
    assert!(done.load(Ordering::SeqCst), "the admitted call drained");
    root.shutdown().await.unwrap();
}

/// An unload waits for the callback its dispatch started, and the listeners
/// that dispatch reaches after the close are skipped (#1125: one count per
/// dispatch and owner, released when the dispatch is done).
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unload_waits_for_a_callback_its_dispatch_started() {
    let root = Ctx::root().unwrap();
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let (entered, blocked) = std::sync::mpsc::channel::<()>();
    let release = Arc::new(std::sync::Mutex::new(false));
    let wake = Arc::new(std::sync::Condvar::new());

    let view = child(&root, "in-dispatch", {
        let order = order.clone();
        let release = release.clone();
        let wake = wake.clone();
        let entered = entered.clone();
        move |ctx| {
            ctx.on_sync::<Ping>({
                let order = order.clone();
                let release = release.clone();
                let wake = wake.clone();
                let entered = entered.clone();
                move |_, _| {
                    order.lock().unwrap().push("first");
                    entered.send(()).unwrap();
                    // A plain thread outside the runtime waits for the release,
                    // so the dispatch really is in flight.
                    let mut released = release.lock().unwrap();
                    while !*released {
                        released = wake.wait(released).unwrap();
                    }
                    Ok(None)
                }
            })?;
            ctx.on_sync::<Ping>({
                let order = order.clone();
                move |_, _| {
                    order.lock().unwrap().push("second");
                    Ok(None)
                }
            })?;
            Ok(())
        }
    })
    .await;

    let dispatch = {
        let root = root.clone();
        std::thread::spawn(move || root.bail(Ping(0)).map(|_| ()))
    };
    blocked
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the first listener started");

    let mut shutdown = view.shutdown();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut shutdown)
            .await
            .is_err(),
        "the unload did not wait for the dispatch's callback"
    );

    *release.lock().unwrap() = true;
    wake.notify_all();
    dispatch.join().unwrap().unwrap();
    shutdown.await.unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        ["first"],
        "a listener the dispatch reached after the close ran"
    );
    root.shutdown().await.unwrap();
}

/// A fiber created after the root closed starts disposed; its listener would
/// never be admitted.
/// guarantee: events/lifecycle
#[tokio::test]
async fn a_fiber_created_after_root_shutdown_is_disposed() {
    let root = Ctx::root().unwrap();
    root.shutdown().await.unwrap();
    let view = root.plugin(Apply("after-close", |ctx: &Ctx| {
        ctx.on_sync::<Ping>(|_, _| Ok(Some("late".to_string())))?;
        Ok(())
    }));
    assert_eq!(view.state().state, FiberState::Disposed);
    assert!(view.state().error.is_some());
}

/// A normal generation restart does not set the permanent marker: the same
/// fiber serves the next generation until it is actually shut down.
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_generation_restart_keeps_the_marker_clear_until_shutdown() {
    let root = Ctx::root().unwrap();
    let calls = counter();
    let heard = calls.clone();
    let view = child(&root, "reload", move |ctx| {
        ctx.on_sync::<Ping>(listener(heard.clone()))?;
        Ok(())
    })
    .await;
    view.restart().await.unwrap();
    root.emit(Ping(0)).unwrap();
    rutis_test_support::until(
        || calls.load(Ordering::SeqCst) == 1,
        "the listener runs in the new generation",
    )
    .await;
    view.shutdown().await.unwrap();
    root.emit(Ping(1)).unwrap();
    // clock: weak-negative — after the close, listeners do not run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the listener ran after the permanent close"
    );
    root.shutdown().await.unwrap();
}

/// Calling shutdown twice shares one task; the listener is not called after
/// the first commit.
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_subtree_shutdown_is_idempotent() {
    let root = Ctx::root().unwrap();
    let calls = counter();
    let heard = calls.clone();
    let view = child(&root, "twice", move |ctx| {
        ctx.on_sync::<Ping>(listener(heard.clone()))?;
        Ok(())
    })
    .await;
    root.serial(Ping(0)).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let first = view.shutdown();
    let second = view.shutdown();
    first.await.unwrap();
    second.await.unwrap();
    root.emit(Ping(1)).unwrap();
    // clock: weak-negative — after the close, listeners do not run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the listener ran after the repeated shutdown"
    );
    root.shutdown().await.unwrap();
}
