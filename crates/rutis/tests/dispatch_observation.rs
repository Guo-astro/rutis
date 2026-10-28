mod common;

use std::future::IntoFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{Listener, Mode};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error, Event};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use common::{on_thread, still_pending};

#[derive(Serialize, Deserialize)]
struct Ping(u32);

impl Event for Ping {
    const NAME: &'static str = "dispatch-observation-ping";
    type Value = u32;
}

/// Counts its calls; no result.
fn counting(calls: Arc<AtomicUsize>) -> impl Listener<Ping> {
    move |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(None))
    }
}

struct Capture(Arc<Mutex<Option<Ctx>>>);

impl DynPlugin for Capture {
    fn name(&self) -> &str {
        "observation-child"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.0.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

async fn child(parent: &Ctx) -> (rutis::host::FiberView, Ctx) {
    let slot = Arc::new(Mutex::new(None));
    let view = parent.plugin(Capture(slot.clone()));
    (&view).into_future().await.unwrap();
    let ctx = slot.lock().unwrap().take().unwrap();
    (view, ctx)
}

/// Was `old_context_still_notifies_non_instance_dispatch_observers`
/// (instance dispatch is gone).
#[tokio::test]
async fn old_context_still_notifies_dispatch_observers() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    let (view, old) = child(&root).await;
    tokio::time::timeout(Duration::from_secs(10), view.restart())
        .await
        .expect("restart timed out")
        .unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    let observer = bus
        .observe_dispatch(&root, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    old.emit(Ping(1)).expect("default event dispatch");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    tokio::time::timeout(Duration::from_secs(10), view.dispose())
        .await
        .expect("view dispose timed out")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), observer.dispose())
        .await
        .expect("observer dispose timed out")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), root.shutdown())
        .await
        .expect("root shutdown timed out")
        .unwrap();
}

#[tokio::test]
async fn all_modes_observe_before_listener_selection_even_when_empty() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_hook = seen.clone();
    let observer = bus
        .observe_dispatch(&root, move |attempt| {
            let value = attempt.event.downcast_ref::<Ping>().unwrap().0;
            seen_hook.lock().unwrap().push((
                attempt.mode,
                attempt.name.to_owned(),
                attempt.emitter,
                attempt.emitter_instance,
                value,
            ));
        })
        .unwrap();

    root.emit(Ping(1)).expect("default event dispatch");
    assert_eq!(seen.lock().unwrap().len(), 1); // no business listener or task

    let serial = root.serial(Ping(2));
    assert_eq!(seen.lock().unwrap().len(), 1); // future not polled
    assert_eq!(serial.await.unwrap(), None);
    assert_eq!(seen.lock().unwrap().len(), 2);

    let parallel = root.parallel(Ping(3));
    assert_eq!(seen.lock().unwrap().len(), 2);
    parallel.await.unwrap();

    let waterfall = root
        .waterfall(Ping(4))
        .terminal(|_| async { Ok(Some(7)) })
        .into_future();
    assert_eq!(seen.lock().unwrap().len(), 3);
    assert_eq!(waterfall.await.unwrap(), Some(7));
    assert_eq!(seen.lock().unwrap().len(), 4);

    assert_eq!(root.bail(Ping(5)).unwrap(), None);

    root.named("dynamic")
        .emit(Ping(6))
        .expect("default event dispatch");
    {
        let entries = seen.lock().unwrap();
        assert_eq!(entries.len(), 6);
        assert_eq!(
            entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            [
                Mode::Emit,
                Mode::Serial,
                Mode::Parallel,
                Mode::Waterfall,
                Mode::Bail,
                Mode::Emit,
            ]
        );
        assert_eq!(entries[5].1, "dynamic");
        assert!(entries[..5].iter().all(|e| e.1 == Ping::NAME));
        assert!(entries.iter().all(|e| e.2 == root.root_view().unwrap().id));
        assert!(entries.iter().all(|e| e.3 == root.instance()));
        assert_eq!(
            entries.iter().map(|e| e.4).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6]
        );
    }
    observer.dispose().await.unwrap();
}

#[tokio::test]
async fn observer_can_register_listener_before_snapshot() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    let count = Arc::new(AtomicUsize::new(0));
    let root_hook = root.clone();
    let count_hook = count.clone();
    bus.observe_dispatch(&root, move |attempt| {
        if attempt.event.downcast_ref::<Ping>().unwrap().0 == 9 {
            root_hook.on::<Ping>(counting(count_hook.clone())).unwrap();
        }
    })
    .unwrap();
    root.serial(Ping(9)).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// Was `observer_scope_and_instance_admission_are_isolated`: instance keys
/// (and their `InstanceOutOfScope` rejection) are gone; an observer still
/// sees only dispatches from its owner's subtree.
#[tokio::test]
async fn observer_scope_admission_is_isolated() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    let (a_view, a) = child(&root).await;
    let (_b_view, b) = child(&root).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    for (label, owner) in [("root", &root), ("a", &a), ("b", &b)] {
        let seen_hook = seen.clone();
        bus.observe_dispatch(owner, move |attempt| {
            seen_hook
                .lock()
                .unwrap()
                .push((label, attempt.event.downcast_ref::<Ping>().unwrap().0));
        })
        .unwrap();
    }
    a.emit(Ping(1)).expect("default event dispatch");
    b.emit(Ping(2)).expect("default event dispatch");
    root.emit(Ping(3)).expect("default event dispatch");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [("root", 1), ("a", 1), ("root", 2), ("b", 2), ("root", 3)]
    );
    a_view.shutdown().await.unwrap();
    b.emit(Ping(5)).expect("default event dispatch");
    assert_eq!(seen.lock().unwrap().last(), Some(&("b", 5)));
}

#[tokio::test]
async fn observer_panic_and_sink_panic_do_not_stop_dispatch() {
    let failures = Arc::new(AtomicUsize::new(0));
    let failures_sink = failures.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |_| {
            failures_sink.fetch_add(1, Ordering::SeqCst);
            panic!("sink panic");
        }),
    );
    let bus = root.events().clone();
    bus.observe_dispatch(&root, |_| panic!("observer panic"))
        .unwrap();
    let reached = Arc::new(AtomicUsize::new(0));
    root.on::<Ping>(counting(reached.clone())).unwrap();
    root.serial(Ping(1)).await.unwrap();
    assert_eq!(failures.load(Ordering::SeqCst), 1);
    assert_eq!(reached.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_selected_synchronous_observer() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    let (view, ctx) = child(&root).await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release = Arc::new(Mutex::new(release_rx));
    let entered_hook = entered.clone();
    let release_hook = release.clone();
    bus.observe_dispatch(&ctx, move |_| {
        if let Some(tx) = entered_hook.lock().unwrap().take() {
            tx.send(()).unwrap();
        }
        release_hook.lock().unwrap().recv().unwrap();
    })
    .unwrap();
    let ctx_emit = ctx.clone();
    let emit = on_thread(move || ctx_emit.emit(Ping(1)));
    entered_rx.await.unwrap();
    let shutdown = view.shutdown();
    tokio::pin!(shutdown);
    assert!(
        still_pending(&mut shutdown).await,
        "shutdown waits for the observer already running"
    );
    release_tx.send(()).unwrap();
    // New behaviour: the emit is accepted (there is no instance check after
    // the observers any more); it just finds no live listener.
    emit.await.unwrap().unwrap();
    shutdown.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn early_disposal_waits_for_in_flight_observer_and_prevents_new_calls() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release = Arc::new(Mutex::new(release_rx));
    let hits = Arc::new(AtomicUsize::new(0));
    let entered_hook = entered.clone();
    let release_hook = release.clone();
    let hits_hook = hits.clone();
    let observer = bus
        .observe_dispatch(&root, move |_| {
            hits_hook.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = entered_hook.lock().unwrap().take() {
                tx.send(()).unwrap();
            }
            release_hook.lock().unwrap().recv().unwrap();
        })
        .unwrap();
    let root_emit = root.clone();
    let emit = on_thread(move || root_emit.emit(Ping(1)).expect("default event dispatch"));
    entered_rx.await.unwrap();
    let disposal = observer.dispose();
    tokio::pin!(disposal);
    assert!(
        still_pending(&mut disposal).await,
        "disposal waits for the observer call in flight"
    );
    release_tx.send(()).unwrap();
    emit.await.unwrap();
    disposal.await.unwrap();
    root.emit(Ping(2)).expect("default event dispatch");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn observer_reentry_observes_nested_dispatch_and_business_listener_runs() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));

    // A business listener: both the nested emit inside the observer and the
    // outer serial select it.
    root.on::<Ping>(counting(hits.clone())).unwrap();

    {
        let attempts = attempts.clone();
        let root_obs = root.clone();
        bus.observe_dispatch(&root, move |attempt| {
            let value = attempt.event.downcast_ref::<Ping>().unwrap().0;
            attempts.lock().unwrap().push(value);
            if value == 1 {
                root_obs.emit(Ping(2)).expect("default event dispatch");
            }
        })
        .unwrap();
    }

    // The observer runs before the outer serial's snapshot; the nested emit
    // does not deadlock it.
    let result = tokio::time::timeout(Duration::from_secs(10), root.serial(Ping(1)))
        .await
        .expect("outer serial did not deadlock");
    assert!(result.unwrap().is_none(), "the listener has no result");

    // Both outer (1) and nested (2) dispatch attempts were observed.
    assert_eq!(*attempts.lock().unwrap(), vec![1, 2]);

    // The outer serial called the listener; the nested emit's call runs in
    // its own task.
    rutis_test_support::until(
        || hits.load(Ordering::SeqCst) == 2,
        "both dispatches called the listener",
    )
    .await;
}
