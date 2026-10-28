use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::task::Poll;

use rutis::events::{Call, Listener, Mode, SyncCall};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind};
use rutis::internal::{CtxInternal, EventKey};
use rutis::{Ctx, Error, Event};
use rutis_test_support::until;
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Notify, Semaphore};

#[derive(Serialize, Deserialize)]
struct Ping(u64);
impl Event for Ping {
    const NAME: &'static str = "ping";
    type Value = u64;
}
/// Another type, with the same shape as `Ping`: dispatched under a name a
/// `Ping` listener hears, it reaches that listener converted through serde.
#[derive(Serialize, Deserialize)]
struct Other(u64);
impl Event for Other {
    const NAME: &'static str = "other";
    type Value = u64;
}
/// A listener that panics while creating its future.
fn panic_creating_future(
    _: Arc<Ping>,
    _: Call<Ping>,
) -> std::future::Ready<Result<Option<u64>, Error>> {
    panic!("panic creating listener future")
}

#[tokio::test]
async fn serial_catches_user_panic_while_creating_exact_and_pattern_futures() {
    let root = Ctx::root().unwrap();
    root.on::<Ping>(panic_creating_future).unwrap();
    root.listen::<Ping>()
        .prefix("panic/")
        .on(panic_creating_future)
        .unwrap();
    for error in [
        root.serial(Ping(0)).await.unwrap_err(),
        root.named("panic/created")
            .serial(Ping(0))
            .await
            .unwrap_err(),
    ] {
        assert!(error.to_string().contains("panic creating listener future"));
    }
    // The pattern's flight is released even when construction panics.
    root.shutdown().await.unwrap();
}

struct Capture(Mutex<Option<oneshot::Sender<Ctx>>>);
impl DynPlugin for Capture {
    fn name(&self) -> &str {
        "capture"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if let Some(sender) = self.0.lock().unwrap().take() {
                assert!(sender.send(ctx.clone()).is_ok());
            }
            Ok(Effect::Done)
        })
    }
}
async fn child(root: &Ctx) -> (rutis::host::FiberView, Ctx) {
    let (sender, receiver) = oneshot::channel();
    let view = root.plugin(Capture(Mutex::new(Some(sender))));
    (&view).await.unwrap();
    (view, receiver.await.unwrap())
}
/// A child marked as a scope of `group`.
async fn scope(parent: &Ctx, group: &str) -> (rutis::host::FiberView, Ctx) {
    let (view, ctx) = child(parent).await;
    ctx.mark_scope(group).unwrap();
    (view, ctx)
}
async fn pending<F: Future>(future: std::pin::Pin<&mut F>) {
    let mut future = future;
    poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("completed before the accepted callback exited"),
    })
    .await;
}

type Log = Arc<Mutex<Vec<String>>>;
struct Record {
    label: &'static str,
    log: Log,
    result: Option<u64>,
}
fn record(label: &'static str, log: &Log) -> Record {
    Record {
        label,
        log: log.clone(),
        result: None,
    }
}
impl Record {
    /// As an exact listener: logs its label.
    fn exact(self) -> impl Listener<Ping> {
        let record = Arc::new(self);
        move |_, _| {
            let record = record.clone();
            async move {
                record.log.lock().unwrap().push(record.label.into());
                Ok(record.result)
            }
        }
    }
    /// As a prefix listener: logs its label and the dispatched name.
    fn pattern(self) -> impl Listener<Ping> {
        let record = Arc::new(self);
        move |_, call: Call<Ping>| {
            let record = record.clone();
            async move {
                record
                    .log
                    .lock()
                    .unwrap()
                    .push(format!("{}:{}", record.label, call.name()));
                Ok(record.result)
            }
        }
    }
}
/// A waterfall listener: wraps the rest of the chain and adds one.
struct Around(Log, &'static str);
impl Around {
    /// As an exact async listener.
    fn exact(self) -> impl Listener<Ping> {
        let around = Arc::new(self);
        move |_, call: Call<Ping>| {
            let around = around.clone();
            async move {
                around.0.lock().unwrap().push(format!("{} in", around.1));
                let value = call.next().await?;
                around.0.lock().unwrap().push(format!("{} out", around.1));
                Ok(value.map(|value| value + 1))
            }
        }
    }
    /// As a prefix async listener, which only hears "room/1".
    fn pattern(self) -> impl Listener<Ping> {
        let exact = self.exact();
        move |event, call: Call<Ping>| {
            assert_eq!(call.name(), "room/1");
            exact(event, call)
        }
    }
}

async fn terminal(event: Arc<Ping>) -> Result<Option<u64>, Error> {
    Ok(Some(event.0))
}

/// Was `deprecated_named_wrappers_share_keys_order_once_and_waterfall`: the
/// `*_keyed` wrappers are gone; the order, `once` and waterfall nesting of a
/// named key remain.
#[tokio::test]
async fn named_key_keeps_order_once_and_waterfall_nesting() {
    let root = Ctx::root().unwrap();
    let key = "legacy";
    let log: Log = Arc::default();
    let normal = root
        .listen::<Ping>()
        .name(key)
        .on(record("normal", &log).exact())
        .unwrap();
    let prepend = root
        .listen::<Ping>()
        .name(key)
        .prepend()
        .on(record("prepend", &log).exact())
        .unwrap();
    root.listen::<Ping>()
        .name(key)
        .once()
        .on(record("once", &log).exact())
        .unwrap();
    assert_eq!(root.named(key).serial(Ping(0)).await.unwrap(), None);
    assert_eq!(*log.lock().unwrap(), ["prepend", "normal", "once"]);
    log.lock().unwrap().clear();
    assert_eq!(
        root.named(String::from("legacy"))
            .serial(Ping(0))
            .await
            .unwrap(),
        None
    );
    assert_eq!(*log.lock().unwrap(), ["prepend", "normal"]);
    log.lock().unwrap().clear();
    root.named(key).parallel(Ping(0)).await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 2);
    log.lock().unwrap().clear();
    root.named(key).emit(Ping(0)).unwrap();
    until(|| log.lock().unwrap().len() == 2, "both emit calls ran").await;
    for label in ["normal", "prepend"] {
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.as_str() == label)
                .count(),
            1
        );
    }
    assert_eq!(root.named("other").serial(Ping(0)).await.unwrap(), None);
    // Every listener is called by every mode: withdraw the plain ones, which
    // would end a waterfall without calling `next`.
    normal.dispose().await.unwrap();
    prepend.dispose().await.unwrap();
    log.lock().unwrap().clear();
    root.listen::<Ping>()
        .name(key)
        .on(Around(log.clone(), "old").exact())
        .unwrap();
    root.listen::<Ping>()
        .name(String::from("legacy"))
        .on(Around(log.clone(), "new").exact())
        .unwrap();
    assert_eq!(
        root.named(String::from("legacy"))
            .waterfall(Ping(3))
            .terminal(terminal)
            .await
            .unwrap(),
        Some(5)
    );
    assert_eq!(
        *log.lock().unwrap(),
        ["old in", "new in", "new out", "old out"]
    );
    log.lock().unwrap().clear();
    assert_eq!(
        root.named(key)
            .waterfall(Ping(3))
            .terminal(terminal)
            .await
            .unwrap(),
        Some(5)
    );
    assert_eq!(
        *log.lock().unwrap(),
        ["old in", "new in", "new out", "old out"]
    );
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn typed_identity_and_prefix_scope_are_distinct() {
    const KEY: EventKey<Ping> = EventKey::named("room/1");
    assert_eq!(KEY, EventKey::dynamic("room/1"));
    // A key is its name: `of()` is `named(E::NAME)`.
    assert_eq!(EventKey::<Ping>::of(), EventKey::named("ping"));
    assert_ne!(EventKey::<Ping>::of(), KEY);
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    root.listen::<Ping>()
        .prefix("room/")
        .on(Record {
            label: "all",
            log: log.clone(),
            result: None,
        }
        .pattern())
        .unwrap();
    root.named(KEY.name()).serial(Ping(0)).await.unwrap();
    root.serial(Ping(0)).await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["all:room/1"]);
    // Listeners are found by name: another type dispatched under a name
    // the prefix covers reaches the `Ping` listener, converted through serde.
    root.named("room/2").serial(Other(5)).await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["all:room/1", "all:room/2"]);
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn exact_and_pattern_share_registration_and_prepend_order() {
    let root = Ctx::root().unwrap();
    let key = "room/1";
    let log: Log = Arc::default();
    root.listen::<Ping>()
        .name(key)
        .on(Record {
            label: "exact",
            log: log.clone(),
            result: None,
        }
        .exact())
        .unwrap();
    root.listen::<Ping>()
        .prefix("room/")
        .on(Record {
            label: "group",
            log: log.clone(),
            result: None,
        }
        .pattern())
        .unwrap();
    root.listen::<Ping>()
        .prefix("room/")
        .prepend()
        .on(Record {
            label: "first",
            log: log.clone(),
            result: None,
        }
        .pattern())
        .unwrap();
    assert_eq!(root.named(key).serial(Ping(0)).await.unwrap(), None);
    assert_eq!(
        *log.lock().unwrap(),
        ["first:room/1", "exact", "group:room/1"]
    );
    let subscriptions = root.subscriptions();
    assert_eq!(subscriptions.len(), 3);
    assert_eq!(
        subscriptions
            .iter()
            .map(|entry| (entry.name.as_str(), entry.prefix, entry.prepend))
            .collect::<Vec<_>>(),
        [
            ("room/", true, true),
            ("room/1", false, false),
            ("room/", true, false)
        ]
    );
    assert!(subscriptions.iter().all(|entry| entry.calls == 1));
    log.lock().unwrap().clear();
    root.named(key).parallel(Ping(0)).await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 3);
    root.shutdown().await.unwrap();
    assert!(root.subscriptions().is_empty());
}

/// Was `grouped_once_is_claimed_once_across_concurrent_names`: multi-prefix
/// patterns (`any_prefix`) are gone; a single prefix covering both names
/// keeps the property.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefix_once_is_claimed_once_across_concurrent_names() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    root.listen::<Ping>()
        .prefix("room/")
        .once()
        .on(Record {
            label: "once",
            log: log.clone(),
            result: None,
        }
        .pattern())
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    std::thread::scope(|scope| {
        for name in ["room/a", "room/b"] {
            let root = root.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                root.handle()
                    .block_on(root.named(name).serial(Ping(0)))
                    .unwrap();
            });
        }
        barrier.wait();
    });
    assert_eq!(log.lock().unwrap().len(), 1);
    assert!(root.subscriptions().is_empty());
    root.shutdown().await.unwrap();
}

/// Was `waterfall_pattern_wraps_and_instance_options_work`: instance keys
/// are gone, so the options are checked on the plain key.
#[tokio::test]
async fn waterfall_pattern_wraps_and_options_work() {
    let root = Ctx::root().unwrap();
    let key = "room/1";
    let log: Log = Arc::default();
    root.listen::<Ping>()
        .name(key)
        .on(Around(log.clone(), "exact").exact())
        .unwrap();
    root.listen::<Ping>()
        .prefix("room/")
        .prepend()
        .on(Around(log.clone(), "pattern").pattern())
        .unwrap();
    assert_eq!(
        root.named(key)
            .waterfall(Ping(1))
            .terminal(terminal)
            .await
            .unwrap(),
        Some(3)
    );
    assert_eq!(
        *log.lock().unwrap(),
        ["pattern in", "exact in", "exact out", "pattern out"]
    );
    log.lock().unwrap().clear();
    root.listen::<Ping>()
        .name(key)
        .prepend()
        .once()
        .on(Around(log.clone(), "once").exact())
        .unwrap();
    assert_eq!(
        root.named(key)
            .waterfall(Ping(1))
            .terminal(terminal)
            .await
            .unwrap(),
        Some(4)
    );
    assert_eq!(
        *log.lock().unwrap(),
        [
            "once in",
            "pattern in",
            "exact in",
            "exact out",
            "pattern out",
            "once out"
        ]
    );
    assert_eq!(
        root.named(key)
            .waterfall(Ping(1))
            .terminal(terminal)
            .await
            .unwrap(),
        Some(3)
    );
    // No terminal: the innermost `next` gives `None`.
    assert_eq!(root.named(key).waterfall(Ping(1)).await.unwrap(), None);
    root.shutdown().await.unwrap();
}

struct GatePattern {
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    log: Arc<Mutex<Vec<(String, u64)>>>,
}
impl GatePattern {
    /// As a prefix listener: a call of `room/a` with `Ping(0)` blocks until
    /// released; every call logs its name and value.
    fn pattern(self) -> impl Listener<Ping> {
        let gate = Arc::new(self);
        move |event: Arc<Ping>, call: Call<Ping>| {
            let gate = gate.clone();
            async move {
                if call.name() == "room/a" && event.0 == 0 {
                    gate.entered.notify_one();
                    gate.release.acquire().await.unwrap().forget();
                }
                gate.log.lock().unwrap().push((call.name().into(), event.0));
                Ok(None)
            }
        }
    }
}

/// New behaviour: the calls of one name's emits only *start* in emit order;
/// a slow call no longer holds up the later emits of its own name either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pattern_only_emit_preserves_each_name_without_global_tail() {
    let root = Ctx::root().unwrap();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    root.listen::<Ping>()
        .prefix("room/")
        .on(GatePattern {
            entered: entered.clone(),
            release: release.clone(),
            log: log.clone(),
        }
        .pattern())
        .unwrap();
    root.named("room/a").emit(Ping(0)).unwrap();
    entered.notified().await;
    root.named("room/a").emit(Ping(1)).unwrap();
    root.named("room/b").emit(Ping(2)).unwrap();
    until(
        || log.lock().unwrap().len() == 2,
        "the later emits ran past the blocked call",
    )
    .await;
    {
        let mut done = log.lock().unwrap().clone();
        done.sort();
        assert_eq!(done, [("room/a".into(), 1), ("room/b".into(), 2)]);
    }
    release.add_permits(1);
    until(|| log.lock().unwrap().len() == 3, "the blocked call ran").await;
    assert_eq!(log.lock().unwrap()[2], ("room/a".into(), 0));
    root.shutdown().await.unwrap();
}

/// A `once` listener a short-circuited `bail` never reached is still there
/// for the next dispatch: it is claimed when its call starts, not when a
/// dispatch selects it.
#[tokio::test]
async fn sync_bail_short_circuits_and_a_once_listener_stays_until_called() {
    let root = Ctx::root().unwrap();
    let key = "room/1";
    let skipped = Arc::new(AtomicUsize::new(0));
    let stop = root
        .listen::<Ping>()
        .name(key)
        .on_sync(|_, _| Ok(Some(7)))
        .unwrap();
    let calls = skipped.clone();
    root.listen::<Ping>()
        .prefix("room/")
        .once()
        .on_sync(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        })
        .unwrap();
    assert_eq!(root.named(key).bail(Ping(0)).unwrap(), Some(7));
    assert_eq!(skipped.load(Ordering::SeqCst), 0);
    assert_eq!(root.subscriptions().len(), 2, "the once listener stays");
    stop.dispose().await.unwrap();
    assert_eq!(root.named(key).bail(Ping(0)).unwrap(), None);
    assert_eq!(skipped.load(Ordering::SeqCst), 1, "called now");
    assert!(root.subscriptions().is_empty(), "and gone");
    assert_eq!(root.named(key).bail(Ping(0)).unwrap(), None);
    assert_eq!(skipped.load(Ordering::SeqCst), 1, "only once");
    root.shutdown().await.unwrap();
}

/// One table: exact and prefix listeners answer `bail` in one
/// registration order.
#[tokio::test]
async fn sync_patterns_expose_keys_and_share_order() {
    fn answer(_: SyncCall<'_, Ping>, own: u64) -> Result<Option<u64>, Error> {
        Ok(Some(own))
    }
    let root = Ctx::root().unwrap();
    let key = "room/1";
    root.listen::<Ping>()
        .name(key)
        .on_sync(|_, call| answer(call, 10))
        .unwrap();
    root.listen::<Ping>()
        .prefix("room/")
        .prepend()
        .on_sync(|_, call| {
            assert_eq!(call.name(), "room/1");
            answer(call, 20)
        })
        .unwrap();
    assert_eq!(root.named(key).bail(Ping(0)).unwrap(), Some(20));
    root.listen::<Ping>()
        .prefix("room/")
        .on_sync(|_, call| {
            assert_eq!(call.name(), "room/1");
            answer(call, 1)
        })
        .unwrap();
    assert_eq!(root.named(key).bail(Ping(0)).unwrap(), Some(20));
    root.shutdown().await.unwrap();
}

/// A synchronous listener may `bail` its own name again (as Cordis allows;
/// `ReentrantEvent` is no error of events protocol §11): the inner
/// dispatch runs on the same stack and calls the listener like any.
#[tokio::test]
async fn sync_listener_reentry_runs_in_bail() {
    let root = Ctx::root().unwrap();
    let key = "reentry";
    let observed = Arc::new(AtomicUsize::new(0));
    let count = observed.clone();
    // The outer call dispatches the same name once more, the inner one
    // answers.
    root.listen::<Ping>()
        .name(key)
        .on_sync(move |event, call| {
            count.fetch_add(1, Ordering::SeqCst);
            match event.0 {
                0 => Ok(call.ctx().named(key).bail(Ping(1))?.map(|inner| inner + 1)),
                _ => Ok(Some(10)),
            }
        })
        .unwrap();
    for attempt in 1..=2 {
        assert_eq!(root.named(key).bail(Ping(0)).unwrap(), Some(11));
        assert_eq!(observed.load(Ordering::SeqCst), attempt * 2);
    }
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_sync_key_can_execute_on_two_threads_at_once() {
    let root = Ctx::root().unwrap();
    let key = "concurrent";
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered = Mutex::new(Some(entered_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release = Mutex::new(release_rx);
    let gate = move |event: &Ping| {
        if event.0 == 0 {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
        }
    };
    root.listen::<Ping>()
        .name(key)
        .on_sync(move |event, _| {
            gate(&event);
            Ok(Some(event.0 + 7))
        })
        .unwrap();
    let caller = root.clone();
    let first = tokio::task::spawn_blocking(move || caller.named(key).bail(Ping(0)));
    entered_rx.await.unwrap();
    // The first call is still in its listener when this thread calls the same key.
    let second = root.named(key).bail(Ping(1));
    release_tx.send(()).unwrap();
    assert_eq!(first.await.unwrap().unwrap(), Some(7));
    assert_eq!(second.unwrap(), Some(8));
    root.shutdown().await.unwrap();
}

/// A synchronous dispatch of the same name from a listener runs (as Cordis
/// allows); other names and other buses too.
#[tokio::test]
async fn sync_reentry_covers_listeners_and_bus_identity() {
    let root = Ctx::root().unwrap();
    let other = Ctx::root().unwrap();
    let key = "same";
    other
        .listen::<Ping>()
        .name(key)
        .on_sync(|_, _| Ok(Some(5)))
        .unwrap();
    root.listen::<Ping>()
        .name("different")
        .on_sync(|_, _| Ok(Some(6)))
        .unwrap();
    let (inner, other_bus) = (root.clone(), other.clone());
    root.listen::<Ping>()
        .name(key)
        .on_sync(move |event, _| {
            if event.0 == 0 {
                return Ok(None);
            }
            assert!(matches!(inner.named(key).bail(Ping(0)), Ok(None)));
            assert_eq!(other_bus.named(key).bail(Ping(0))?, Some(5));
            inner.named("different").bail(Ping(0))
        })
        .unwrap();
    assert_eq!(root.named(key).bail(Ping(1)).unwrap(), Some(6));
    root.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
}

#[tokio::test]
async fn sync_error_is_preserved_and_panic_reports_once_even_with_panicking_sink() {
    let reports = Arc::new(AtomicUsize::new(0));
    let seen = reports.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            panic!("sink");
        }),
    );
    let fail = root
        .on_sync::<Ping>(|_, _| {
            Err(Error::from(ErrorKind::Validation {
                issues: vec!["original".into()],
            }))
        })
        .unwrap();
    assert!(
        matches!(&*root.bail(Ping(0)).unwrap_err(), ErrorKind::Validation { issues } if *issues == ["original"])
    );
    assert_eq!(reports.load(Ordering::SeqCst), 0);
    fail.dispose().await.unwrap();
    root.on_sync::<Ping>(|_, _| -> Result<Option<u64>, Error> { panic!("inner") })
        .unwrap();
    assert!(matches!(
        &*root.bail(Ping(0)).unwrap_err(),
        ErrorKind::SyncEventPanicked(_)
    ));
    assert_eq!(reports.load(Ordering::SeqCst), 1);
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// Was `thousand_early_sync_disposals_reject_new_selection_and_wait_for_old_calls`.
/// New behaviour: disposing one listener withdraws it at once (no new
/// selection) but does not wait for its running call; only a fiber's unload
/// waits for its listeners' calls.
async fn thousand_early_sync_disposals_reject_new_selection_while_old_calls_finish() {
    let root = Ctx::root().unwrap();
    for _ in 0..1000 {
        let key = "blocking";
        let (entered_tx, entered_rx) = oneshot::channel();
        let entered = Mutex::new(Some(entered_tx));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release = Mutex::new(release_rx);
        let listener = root
            .listen::<Ping>()
            .name(key)
            .on_sync(move |_, _| {
                if let Some(sender) = entered.lock().unwrap().take() {
                    sender.send(()).unwrap();
                }
                release.lock().unwrap().recv().unwrap();
                Ok(Some(9))
            })
            .unwrap();
        let caller = root.clone();
        let dispatch = tokio::task::spawn_blocking(move || caller.named("blocking").bail(Ping(0)));
        entered_rx.await.unwrap();
        listener.dispose().await.unwrap();
        assert!(root.subscriptions().is_empty());
        assert_eq!(root.named(key).bail(Ping(0)).unwrap(), None);
        assert!(!dispatch.is_finished());
        release_tx.send(()).unwrap();
        assert_eq!(dispatch.await.unwrap().unwrap(), Some(9));
    }
    root.shutdown().await.unwrap();
}

/// Was an instance-key test: the listener lives in a scope; the bus is
/// flat, but disposing the listener or shutting down its owner's subtree
/// still drains a running synchronous call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_sync_bail_drains_during_removal_and_subtree_shutdown() {
    for shutdown in [false, true] {
        let root = Ctx::root().unwrap();
        let (view, owner) = scope(&root, "owner").await;
        let (_, emitter) = child(&owner).await;
        let key = "scoped";
        let (entered_tx, entered_rx) = oneshot::channel();
        let entered = Mutex::new(Some(entered_tx));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release = Mutex::new(release_rx);
        let gate = move || {
            if let Some(sender) = entered.lock().unwrap().take() {
                sender.send(()).unwrap();
            }
            release.lock().unwrap().recv().unwrap();
        };
        let listener = owner
            .listen::<Ping>()
            .name(key)
            .on_sync(move |_, _| {
                gate();
                Ok(Some(10))
            })
            .unwrap();
        let caller = emitter.clone();
        let dispatch = tokio::task::spawn_blocking(move || caller.named(key).bail(Ping(0)));
        entered_rx.await.unwrap();
        let finish = if shutdown {
            view.shutdown()
        } else {
            listener.dispose()
        };
        let mut finish = pin!(finish);
        if shutdown {
            // The owner's unload waits for its listener's running call.
            pending(finish.as_mut()).await;
        } else {
            // New behaviour: disposing one listener does not wait for
            // its running call.
            finish.as_mut().await.unwrap();
        }
        if shutdown {
            assert!(emitter.named(key).bail(Ping(0)).is_err());
        } else {
            until(
                || root.subscriptions().is_empty(),
                "the listener left the table",
            )
            .await;
            assert_eq!(emitter.named(key).bail(Ping(0)).unwrap(), None);
        }
        if shutdown {
            pending(finish.as_mut()).await;
        }
        release_tx.send(()).unwrap();
        assert_eq!(dispatch.await.unwrap().unwrap(), Some(10));
        if shutdown {
            finish.await.unwrap();
        }
        view.shutdown().await.unwrap();
        assert!(root.subscriptions().is_empty());
        root.shutdown().await.unwrap();
    }
}

/// Was `sync_instance_isolation_stale_context_and_self_shutdown`: instance
/// keys are gone. Events are one flat bus, so every listener hears every
/// dispatch; registration order still decides `bail`.
#[tokio::test]
async fn sync_flat_bus_stale_context_and_self_shutdown() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = scope(&root, "a").await;
    let (_, sibling) = scope(&root, "b").await;
    let key = "scoped";
    ctx.listen::<Ping>()
        .name(key)
        .on_sync(|_, _| Ok(Some(8)))
        .unwrap();
    assert_eq!(ctx.named(key).bail(Ping(0)).unwrap(), Some(8));
    assert_eq!(root.named(key).bail(Ping(0)).unwrap(), Some(8));
    assert_eq!(sibling.named(key).bail(Ping(0)).unwrap(), Some(8));
    sibling
        .listen::<Ping>()
        .name(key)
        .on_sync(|_, _| Ok(Some(9)))
        .unwrap();
    assert_eq!(sibling.named(key).bail(Ping(0)).unwrap(), Some(8));
    assert_eq!(ctx.named(key).bail(Ping(0)).unwrap(), Some(8));
    view.restart().await.unwrap();
    assert!(matches!(
        &*ctx.named(key).bail(Ping(0)).unwrap_err(),
        ErrorKind::StaleGeneration { .. }
    ));
    assert!(ctx
        .listen::<Ping>()
        .name(key)
        .on_sync(|_, _| Ok(None))
        .is_err());
    view.shutdown().await.unwrap();
    let own = root.clone();
    root.on_sync::<Ping>(move |_, _| {
        drop(own.shutdown());
        Ok(Some(1))
    })
    .unwrap();
    assert_eq!(root.bail(Ping(0)).unwrap(), Some(1));
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn thousand_registrations_and_disposals_leave_no_subscriptions() {
    let root = Ctx::root().unwrap();
    for i in 0..1000 {
        let key = format!("room/{i}");
        let a = root
            .listen::<Ping>()
            .name(key.clone())
            .on_sync(|_, _| Ok(None))
            .unwrap();
        let b = root
            .listen::<Ping>()
            .prefix("room/")
            .on_sync(|_, _| Ok(None))
            .unwrap();
        assert_eq!(root.named(key).bail(Ping(0)).unwrap(), None);
        a.dispose().await.unwrap();
        b.dispose().await.unwrap();
        assert!(root.subscriptions().is_empty());
    }
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pattern_shutdown_drains_old_callback_and_excludes_new_named_dispatches() {
    for root_owner in [false, true] {
        let root = Ctx::root().unwrap();
        let (view, owner) = if root_owner {
            (root.root_view().unwrap(), root.clone())
        } else {
            child(&root).await
        };
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let log = Arc::new(Mutex::new(Vec::new()));
        owner
            .listen::<Ping>()
            .prefix("room/")
            .on(GatePattern {
                entered: entered.clone(),
                release: release.clone(),
                log: log.clone(),
            }
            .pattern())
            .unwrap();
        root.named("room/a").emit(Ping(0)).unwrap();
        entered.notified().await;
        let mut shutdown = pin!(if root_owner {
            root.shutdown()
        } else {
            view.shutdown()
        });
        pending(shutdown.as_mut()).await;
        // Async dispatch still returns Ok for an old context, but the
        // closing pattern owner cannot admit another call.
        root.named("room/new").serial(Ping(1)).await.unwrap();
        assert!(root.subscriptions().iter().all(|entry| entry.calls == 1));
        pending(shutdown.as_mut()).await;
        release.add_permits(1);
        shutdown.await.unwrap();
        assert_eq!(log.lock().unwrap().len(), 1);
        assert!(root.subscriptions().is_empty());
        root.shutdown().await.unwrap();
    }
}
