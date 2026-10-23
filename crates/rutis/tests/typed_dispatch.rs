use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::task::Poll;

use rutis::{
    BoxFuture, Call, CordisError, Ctx, Effect, Event, EventKey, EventOptions, EventPattern,
    Listener, Mode, PatternListener, Plugin, SyncCall, SyncListener,
};
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

struct PanicCreatingFuture;
impl Listener<Ping> for PanicCreatingFuture {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: &'a Ping,
        _: Call<'a, u64>,
    ) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
        panic!("panic creating listener future")
    }
}
impl PatternListener<Ping> for PanicCreatingFuture {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: EventKey<Ping>,
        _: &'a Ping,
        _: Call<'a, u64>,
    ) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
        panic!("panic creating listener future")
    }
}

#[tokio::test]
async fn serial_catches_user_panic_while_creating_exact_and_pattern_futures() {
    let root = Ctx::root().unwrap();
    root.events()
        .on(&root, &EventKey::of(), PanicCreatingFuture)
        .unwrap();
    root.events()
        .on_pattern(&root, &EventPattern::prefix("panic/"), PanicCreatingFuture)
        .unwrap();
    for key in [EventKey::of(), EventKey::named("panic/created")] {
        let error = root
            .events()
            .serial(&root, &key, Arc::new(Ping(0)))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("panic creating listener future"));
    }
    // The pattern's flight is released even when construction panics.
    root.shutdown().await.unwrap();
}

struct Capture(Mutex<Option<oneshot::Sender<Ctx>>>);
impl Plugin for Capture {
    fn name(&self) -> &str {
        "capture"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            if let Some(sender) = self.0.lock().unwrap().take() {
                assert!(sender.send(ctx.clone()).is_ok());
            }
            Ok(Effect::Done)
        })
    }
}
async fn child(root: &Ctx) -> (rutis::FiberView, Ctx) {
    let (sender, receiver) = oneshot::channel();
    let view = root.plugin(Capture(Mutex::new(Some(sender))));
    (&view).await.unwrap();
    (view, receiver.await.unwrap())
}
/// A child marked as a scope of `group`.
async fn scope(parent: &Ctx, group: &str) -> (rutis::FiberView, Ctx) {
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
impl Listener<Ping> for Record {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: &'a Ping,
        _: Call<'a, u64>,
    ) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
        Box::pin(async move {
            self.log.lock().unwrap().push(self.label.into());
            Ok(self.result)
        })
    }
}
impl PatternListener<Ping> for Record {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        key: EventKey<Ping>,
        _: &'a Ping,
        _: Call<'a, u64>,
    ) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
        Box::pin(async move {
            self.log
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.label, key.name()));
            Ok(self.result)
        })
    }
}
/// A waterfall listener: wraps the rest of the chain and adds one.
struct Around(Log, &'static str);
impl Listener<Ping> for Around {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: &'a Ping,
        call: Call<'a, u64>,
    ) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(format!("{} in", self.1));
            let value = call.next().await?;
            self.0.lock().unwrap().push(format!("{} out", self.1));
            Ok(value.map(|value| value + 1))
        })
    }
}
impl PatternListener<Ping> for Around {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        key: EventKey<Ping>,
        event: &'a Ping,
        call: Call<'a, u64>,
    ) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
        assert_eq!(key.name(), "room/1");
        Listener::call(self, ctx, event, call)
    }
}
impl SyncListener<Ping> for Around {
    fn call(&self, _: &Ctx, _: &Ping, call: SyncCall<'_, u64>) -> Result<Option<u64>, CordisError> {
        self.0.lock().unwrap().push(format!("{} in", self.1));
        let value = call.next()?;
        self.0.lock().unwrap().push(format!("{} out", self.1));
        Ok(value.map(|value| value + 1))
    }
}

fn terminal<'a>(_: &'a Ctx, event: &'a Ping) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
    Box::pin(async move { Ok(Some(event.0)) })
}

/// Was `deprecated_named_wrappers_share_keys_order_once_and_waterfall`: the
/// `*_keyed` wrappers are gone; the order, `once` and waterfall nesting of a
/// named key remain.
#[tokio::test]
async fn named_key_keeps_order_once_and_waterfall_nesting() {
    let root = Ctx::root().unwrap();
    let bus = root.events();
    let key = EventKey::<Ping>::named("legacy");
    let log: Log = Arc::default();
    let normal = bus.on(&root, &key, record("normal", &log)).unwrap();
    let prepend = bus
        .on_opt(
            &root,
            &key,
            record("prepend", &log),
            EventOptions::default().prepend(true),
        )
        .unwrap();
    bus.once(&root, &key, record("once", &log)).unwrap();
    assert_eq!(
        bus.serial(&root, &key, Arc::new(Ping(0))).await.unwrap(),
        None
    );
    assert_eq!(*log.lock().unwrap(), ["prepend", "normal", "once"]);
    log.lock().unwrap().clear();
    assert_eq!(
        bus.serial(
            &root,
            &EventKey::dynamic(String::from("legacy")),
            Arc::new(Ping(0))
        )
        .await
        .unwrap(),
        None
    );
    assert_eq!(*log.lock().unwrap(), ["prepend", "normal"]);
    log.lock().unwrap().clear();
    bus.parallel(&root, &key, Arc::new(Ping(0))).await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 2);
    log.lock().unwrap().clear();
    bus.emit(&root, &key, Arc::new(Ping(0))).unwrap();
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
    assert_eq!(
        bus.serial(&root, &EventKey::named("other"), Arc::new(Ping(0)))
            .await
            .unwrap(),
        None
    );
    // Every listener is called by every mode: withdraw the plain ones, which
    // would end a waterfall without calling `next`.
    normal.dispose().await.unwrap();
    prepend.dispose().await.unwrap();
    log.lock().unwrap().clear();
    bus.on(&root, &key, Around(log.clone(), "old")).unwrap();
    bus.on(
        &root,
        &EventKey::dynamic("legacy"),
        Around(log.clone(), "new"),
    )
    .unwrap();
    assert_eq!(
        bus.waterfall(&root, &EventKey::dynamic("legacy"), &Ping(3), terminal)
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
        bus.waterfall(&root, &key, &Ping(3), terminal)
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
    root.events()
        .on_pattern(
            &root,
            &EventPattern::prefix("room/"),
            Record {
                label: "all",
                log: log.clone(),
                result: None,
            },
        )
        .unwrap();
    root.events()
        .serial(&root, &KEY, Arc::new(Ping(0)))
        .await
        .unwrap();
    root.events()
        .serial(&root, &EventKey::of(), Arc::new(Ping(0)))
        .await
        .unwrap();
    assert_eq!(*log.lock().unwrap(), ["all:room/1"]);
    // Listeners are found by name: another type dispatched under a name
    // the prefix covers reaches the `Ping` listener, converted through serde.
    root.events()
        .serial(&root, &EventKey::named("room/2"), Arc::new(Other(5)))
        .await
        .unwrap();
    assert_eq!(*log.lock().unwrap(), ["all:room/1", "all:room/2"]);
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn exact_and_pattern_share_registration_and_prepend_order() {
    let root = Ctx::root().unwrap();
    let key = EventKey::named("room/1");
    let log: Log = Arc::default();
    let bus = root.events();
    bus.on(
        &root,
        &key,
        Record {
            label: "exact",
            log: log.clone(),
            result: None,
        },
    )
    .unwrap();
    bus.on_pattern(
        &root,
        &EventPattern::prefix("room/"),
        Record {
            label: "group",
            log: log.clone(),
            result: None,
        },
    )
    .unwrap();
    bus.on_pattern_opt(
        &root,
        &EventPattern::prefix("room/"),
        Record {
            label: "first",
            log: log.clone(),
            result: None,
        },
        EventOptions::default().prepend(true),
    )
    .unwrap();
    assert_eq!(
        bus.serial(&root, &key, Arc::new(Ping(0))).await.unwrap(),
        None
    );
    assert_eq!(
        *log.lock().unwrap(),
        ["first:room/1", "exact", "group:room/1"]
    );
    let subscriptions = bus.subscriptions();
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
    bus.parallel(&root, &key, Arc::new(Ping(0))).await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 3);
    root.shutdown().await.unwrap();
    assert!(bus.subscriptions().is_empty());
}

/// Was `grouped_once_is_claimed_once_across_concurrent_names`: multi-prefix
/// patterns (`any_prefix`) are gone; a single prefix covering both names
/// keeps the property.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefix_once_is_claimed_once_across_concurrent_names() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    root.events()
        .on_pattern_opt(
            &root,
            &EventPattern::prefix("room/"),
            Record {
                label: "once",
                log: log.clone(),
                result: None,
            },
            EventOptions::default().once(true),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    std::thread::scope(|scope| {
        for name in ["room/a", "room/b"] {
            let root = root.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                root.handle()
                    .block_on(root.events().serial(
                        &root,
                        &EventKey::named(name),
                        Arc::new(Ping(0)),
                    ))
                    .unwrap();
            });
        }
        barrier.wait();
    });
    assert_eq!(log.lock().unwrap().len(), 1);
    assert!(root.events().subscriptions().is_empty());
    root.shutdown().await.unwrap();
}

/// Was `waterfall_pattern_wraps_and_instance_options_work`: instance keys
/// are gone, so the options are checked on the plain key.
#[tokio::test]
async fn waterfall_pattern_wraps_and_options_work() {
    let root = Ctx::root().unwrap();
    let key = EventKey::named("room/1");
    let log: Log = Arc::default();
    root.events()
        .on(&root, &key, Around(log.clone(), "exact"))
        .unwrap();
    root.events()
        .on_pattern_opt(
            &root,
            &EventPattern::prefix("room/"),
            Around(log.clone(), "pattern"),
            EventOptions::default().prepend(true),
        )
        .unwrap();
    assert_eq!(
        root.events()
            .waterfall(&root, &key, &Ping(1), terminal)
            .await
            .unwrap(),
        Some(3)
    );
    assert_eq!(
        *log.lock().unwrap(),
        ["pattern in", "exact in", "exact out", "pattern out"]
    );
    log.lock().unwrap().clear();
    root.events()
        .on_opt(
            &root,
            &key,
            Around(log.clone(), "once"),
            EventOptions::default().prepend(true).once(true),
        )
        .unwrap();
    assert_eq!(
        root.events()
            .waterfall(&root, &key, &Ping(1), terminal)
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
        root.events()
            .waterfall(&root, &key, &Ping(1), terminal)
            .await
            .unwrap(),
        Some(3)
    );
    // No terminal: the innermost `next` gives `None`.
    assert_eq!(
        root.events()
            .waterfall(&root, &key, &Ping(1), ())
            .await
            .unwrap(),
        None
    );
    root.shutdown().await.unwrap();
}

struct GatePattern {
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    log: Arc<Mutex<Vec<(String, u64)>>>,
}
impl PatternListener<Ping> for GatePattern {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        key: EventKey<Ping>,
        event: &'a Ping,
        _: Call<'a, u64>,
    ) -> BoxFuture<'a, Result<Option<u64>, CordisError>> {
        Box::pin(async move {
            if key.name() == "room/a" && event.0 == 0 {
                self.entered.notify_one();
                self.release.acquire().await.unwrap().forget();
            }
            self.log.lock().unwrap().push((key.name().into(), event.0));
            Ok(None)
        })
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
    root.events()
        .on_pattern(
            &root,
            &EventPattern::prefix("room/"),
            GatePattern {
                entered: entered.clone(),
                release: release.clone(),
                log: log.clone(),
            },
        )
        .unwrap();
    root.events()
        .emit(&root, &EventKey::named("room/a"), Arc::new(Ping(0)))
        .unwrap();
    entered.notified().await;
    root.events()
        .emit(&root, &EventKey::named("room/a"), Arc::new(Ping(1)))
        .unwrap();
    root.events()
        .emit(&root, &EventKey::named("room/b"), Arc::new(Ping(2)))
        .unwrap();
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
    let key = EventKey::named("room/1");
    let skipped = Arc::new(AtomicUsize::new(0));
    let stop = root
        .events()
        .on_sync(&root, &key, |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| {
            Ok(Some(7))
        })
        .unwrap();
    let calls = skipped.clone();
    root.events()
        .on_sync_pattern_opt(
            &root,
            &EventPattern::prefix("room/"),
            move |_: &Ctx, _: EventKey<Ping>, _: &Ping, _: SyncCall<'_, u64>| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            },
            EventOptions::default().once(true),
        )
        .unwrap();
    assert_eq!(
        root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
        Some(7)
    );
    assert_eq!(skipped.load(Ordering::SeqCst), 0);
    assert_eq!(
        root.events().subscriptions().len(),
        2,
        "the once listener stays"
    );
    stop.dispose().await.unwrap();
    assert_eq!(
        root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
        None
    );
    assert_eq!(skipped.load(Ordering::SeqCst), 1, "called now");
    assert!(root.events().subscriptions().is_empty(), "and gone");
    assert_eq!(
        root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
        None
    );
    assert_eq!(skipped.load(Ordering::SeqCst), 1, "only once");
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn sync_waterfall_borrows_mutex_and_veto_skips_terminal() {
    let root = Ctx::root().unwrap();
    let key = EventKey::of();
    let log: Log = Arc::default();
    root.events()
        .on_sync(&root, &key, Around(log.clone(), "outer"))
        .unwrap();
    root.events()
        .on_sync(&root, &key, Around(log.clone(), "inner"))
        .unwrap();
    let storage = Mutex::new(10);
    let mut terminal_calls = 0;
    {
        let mut locked = storage.lock().unwrap();
        let value = root
            .events()
            .waterfall_sync(&root, &key, &Ping(3), |_, event| {
                terminal_calls += 1;
                Ok(Some(*locked + event.0))
            })
            .unwrap()
            .unwrap();
        *locked = value;
        assert_eq!((*locked, terminal_calls), (15, 1));
    }
    assert_eq!(
        *log.lock().unwrap(),
        ["outer in", "inner in", "inner out", "outer out"]
    );
    root.events()
        .on_sync_opt(
            &root,
            &key,
            |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| Ok(Some(99)),
            EventOptions::default().prepend(true).once(true),
        )
        .unwrap();
    assert_eq!(
        root.events()
            .waterfall_sync(&root, &key, &Ping(0), |_, _| {
                terminal_calls += 1;
                Ok(Some(0))
            })
            .unwrap(),
        Some(99)
    );
    assert_eq!(terminal_calls, 1);
    root.shutdown().await.unwrap();
}

/// One table: the same exact and prefix listeners answer `bail` and
/// `waterfall-sync`, in one registration order.
#[tokio::test]
async fn sync_patterns_expose_keys_and_share_order_for_both_modes() {
    fn answer(call: SyncCall<'_, u64>, own: u64) -> Result<Option<u64>, CordisError> {
        if call.mode() == Mode::WaterfallSync {
            Ok(call.next()?.map(|value| value + own))
        } else {
            Ok(Some(own))
        }
    }
    let root = Ctx::root().unwrap();
    let key = EventKey::named("room/1");
    root.events()
        .on_sync(&root, &key, |_: &Ctx, _: &Ping, call: SyncCall<'_, u64>| {
            answer(call, 10)
        })
        .unwrap();
    root.events()
        .on_sync_pattern_opt(
            &root,
            &EventPattern::prefix("room/"),
            |_: &Ctx, key: EventKey<Ping>, _: &Ping, call: SyncCall<'_, u64>| {
                assert_eq!(key.name(), "room/1");
                answer(call, 20)
            },
            EventOptions::default().prepend(true),
        )
        .unwrap();
    assert_eq!(
        root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
        Some(20)
    );
    root.events()
        .on_sync_pattern(
            &root,
            &EventPattern::prefix("room/"),
            |_: &Ctx, key: EventKey<Ping>, _: &Ping, call: SyncCall<'_, u64>| {
                assert_eq!(key.name(), "room/1");
                answer(call, 1)
            },
        )
        .unwrap();
    assert_eq!(
        root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
        Some(20)
    );
    assert_eq!(
        root.events()
            .waterfall_sync(&root, &key, &Ping(0), |_, _| Ok(Some(4)))
            .unwrap(),
        Some(35)
    );
    root.shutdown().await.unwrap();
}

/// A synchronous listener may dispatch its own name again, synchronously,
/// in every combination of `bail` and `waterfall-sync` (as Cordis allows;
/// `ReentrantEvent` is no error of events protocol §11): the inner
/// dispatch runs on the same stack and is observed like any.
#[tokio::test]
async fn sync_listener_reentry_runs_in_all_bail_and_waterfall_combinations() {
    fn nested(
        ctx: &Ctx,
        key: &EventKey<Ping>,
        waterfall: bool,
    ) -> Result<Option<u64>, CordisError> {
        if waterfall {
            ctx.events()
                .waterfall_sync(ctx, key, &Ping(1), |_, _| Ok(Some(99)))
        } else {
            ctx.events().bail_sync(ctx, key, &Ping(1))
        }
    }
    for outer_waterfall in [false, true] {
        for inner_waterfall in [false, true] {
            let root = Ctx::root().unwrap();
            let key = EventKey::<Ping>::named("reentry");
            let observed = Arc::new(AtomicUsize::new(0));
            let count = observed.clone();
            root.events()
                .observe_dispatch(&root, move |_| {
                    count.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap();
            let inner_key = key.clone();
            // One listener answers both modes: the outer call dispatches
            // the same name once more, the inner one answers.
            root.events()
                .on_sync(
                    &root,
                    &key,
                    move |ctx: &Ctx, event: &Ping, _: SyncCall<'_, u64>| match event.0 {
                        0 => Ok(nested(ctx, &inner_key, inner_waterfall)?.map(|inner| inner + 1)),
                        _ => Ok(Some(10)),
                    },
                )
                .unwrap();
            for attempt in 1..=2 {
                let result = if outer_waterfall {
                    root.events()
                        .waterfall_sync(&root, &key, &Ping(0), |_, _| Ok(Some(0)))
                } else {
                    root.events().bail_sync(&root, &key, &Ping(0))
                };
                assert_eq!(result.unwrap(), Some(11));
                assert_eq!(observed.load(Ordering::SeqCst), attempt * 2);
            }
            root.shutdown().await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_sync_key_can_execute_on_two_threads_at_once() {
    for waterfall in [false, true] {
        let root = Ctx::root().unwrap();
        let key = EventKey::<Ping>::named("concurrent");
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
        root.events()
            .on_sync(
                &root,
                &key,
                move |_: &Ctx, event: &Ping, call: SyncCall<'_, u64>| {
                    gate(event);
                    if call.mode() == Mode::WaterfallSync {
                        call.next()
                    } else {
                        Ok(Some(event.0 + 7))
                    }
                },
            )
            .unwrap();
        let caller = root.clone();
        let first_key = key.clone();
        let first = tokio::task::spawn_blocking(move || {
            if waterfall {
                caller
                    .events()
                    .waterfall_sync(&caller, &first_key, &Ping(0), |_, event| {
                        Ok(Some(event.0 + 7))
                    })
            } else {
                caller.events().bail_sync(&caller, &first_key, &Ping(0))
            }
        });
        entered_rx.await.unwrap();
        // The first call is still in its listener when this thread calls the same key.
        let second = if waterfall {
            root.events()
                .waterfall_sync(&root, &key, &Ping(1), |_, event| Ok(Some(event.0 + 7)))
        } else {
            root.events().bail_sync(&root, &key, &Ping(1))
        };
        release_tx.send(()).unwrap();
        assert_eq!(first.await.unwrap().unwrap(), Some(7));
        assert_eq!(second.unwrap(), Some(8));
        root.shutdown().await.unwrap();
    }
}

/// A synchronous dispatch of the same name from an observer and from a
/// terminal runs (as Cordis allows); other names and other buses too.
#[tokio::test]
async fn sync_reentry_covers_observer_terminal_modes_and_bus_identity() {
    let root = Ctx::root().unwrap();
    let other = Ctx::root().unwrap();
    let key = EventKey::<Ping>::named("same");
    let checks = Arc::new(AtomicUsize::new(0));
    let checked = checks.clone();
    root.events()
        .observe_dispatch(&root, move |attempt| {
            if attempt.mode == Mode::WaterfallSync {
                assert!(matches!(root_call(attempt.event), Some(0)));
                checked.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
    fn root_call(event: &(dyn std::any::Any + Send + Sync)) -> Option<u64> {
        event.downcast_ref::<Ping>().map(|event| event.0)
    }
    let observed_ctx = root.clone();
    let observed_key = key.clone();
    let guard_seen = Arc::new(AtomicUsize::new(0));
    let guard_check = guard_seen.clone();
    root.events()
        .observe_dispatch(&root, move |attempt| {
            // Once per waterfall: the inner bail is observed too.
            if attempt.mode == Mode::WaterfallSync
                && matches!(
                    observed_ctx
                        .events()
                        .bail_sync(&observed_ctx, &observed_key, &Ping(0)),
                    Ok(None)
                )
            {
                guard_check.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
    let result = root
        .events()
        .waterfall_sync(&root, &key, &Ping(0), |_, _| {
            assert!(matches!(
                root.events().bail_sync(&root, &key, &Ping(0)),
                Ok(None)
            ));
            assert_eq!(
                other
                    .events()
                    .waterfall_sync(&other, &key, &Ping(0), |_, _| Ok(Some(5)))?,
                Some(5)
            );
            root.events()
                .waterfall_sync(&root, &EventKey::named("different"), &Ping(0), |_, _| {
                    Ok(Some(6))
                })
        })
        .unwrap();
    assert_eq!(result, Some(6));
    assert_eq!(guard_seen.load(Ordering::SeqCst), 2);
    assert_eq!(checks.load(Ordering::SeqCst), 2);
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
    let key = EventKey::of();
    let fail = root
        .events()
        .on_sync(&root, &key, |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| {
            Err(CordisError::Validation {
                issues: vec!["original".into()],
            })
        })
        .unwrap();
    assert!(
        matches!(root.events().bail_sync(&root, &key, &Ping(0)), Err(CordisError::Validation { issues }) if issues == ["original"])
    );
    assert_eq!(reports.load(Ordering::SeqCst), 0);
    fail.dispose().await.unwrap();
    root.events()
        .on_sync(&root, &key, Around(Arc::default(), "outer"))
        .unwrap();
    root.events()
        .on_sync(
            &root,
            &key,
            |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| -> Result<Option<u64>, CordisError> {
                panic!("inner")
            },
        )
        .unwrap();
    assert!(matches!(
        root.events()
            .waterfall_sync(&root, &key, &Ping(0), |_, _| Ok(Some(0))),
        Err(CordisError::SyncEventPanicked(_))
    ));
    assert_eq!(reports.load(Ordering::SeqCst), 1);
    assert!(matches!(
        root.events().waterfall_sync(
            &root,
            &EventKey::named("terminal"),
            &Ping(0),
            |_, _| -> Result<Option<u64>, CordisError> { panic!("terminal") }
        ),
        Err(CordisError::SyncEventPanicked(_))
    ));
    assert_eq!(reports.load(Ordering::SeqCst), 2);
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
        let key = EventKey::named("blocking");
        let (entered_tx, entered_rx) = oneshot::channel();
        let entered = Mutex::new(Some(entered_tx));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release = Mutex::new(release_rx);
        let listener = root
            .events()
            .on_sync(
                &root,
                &key,
                move |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| {
                    if let Some(sender) = entered.lock().unwrap().take() {
                        sender.send(()).unwrap();
                    }
                    release.lock().unwrap().recv().unwrap();
                    Ok(Some(9))
                },
            )
            .unwrap();
        let caller = root.clone();
        let dispatch = tokio::task::spawn_blocking(move || {
            caller
                .events()
                .bail_sync(&caller, &EventKey::named("blocking"), &Ping(0))
        });
        entered_rx.await.unwrap();
        listener.dispose().await.unwrap();
        assert!(root.events().subscriptions().is_empty());
        assert_eq!(
            root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
            None
        );
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
async fn scoped_sync_bail_and_waterfall_drain_during_removal_and_subtree_shutdown() {
    for waterfall in [false, true] {
        for shutdown in [false, true] {
            let root = Ctx::root().unwrap();
            let (view, owner) = scope(&root, "owner").await;
            let (_, emitter) = child(&owner).await;
            let key = EventKey::<Ping>::named("scoped");
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
            let listener = root
                .events()
                .on_sync(
                    &owner,
                    &key,
                    move |_: &Ctx, _: &Ping, call: SyncCall<'_, u64>| {
                        gate();
                        if call.mode() == Mode::WaterfallSync {
                            Ok(call.next()?.map(|value| value + 9))
                        } else {
                            Ok(Some(10))
                        }
                    },
                )
                .unwrap();
            let dispatch_key = key.clone();
            let caller = emitter.clone();
            let dispatch = tokio::task::spawn_blocking(move || {
                if waterfall {
                    caller
                        .events()
                        .waterfall_sync(&caller, &dispatch_key, &Ping(0), |_, _| Ok(Some(1)))
                } else {
                    caller.events().bail_sync(&caller, &dispatch_key, &Ping(0))
                }
            });
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
                assert!(emitter
                    .events()
                    .bail_sync(&emitter, &key, &Ping(0))
                    .is_err());
            } else {
                until(
                    || root.events().subscriptions().is_empty(),
                    "the listener left the table",
                )
                .await;
                assert_eq!(
                    emitter
                        .events()
                        .bail_sync(&emitter, &key, &Ping(0))
                        .unwrap(),
                    None
                );
                assert_eq!(
                    emitter
                        .events()
                        .waterfall_sync(&emitter, &key, &Ping(0), |_, _| Ok(Some(1)))
                        .unwrap(),
                    Some(1)
                );
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
            assert!(root.events().subscriptions().is_empty());
            root.shutdown().await.unwrap();
        }
    }
}

/// Was `sync_instance_isolation_stale_context_and_self_shutdown`: instance
/// keys are gone. Events are one flat bus, so every listener hears every
/// dispatch; registration order still decides `bail` / `waterfall`.
#[tokio::test]
async fn sync_flat_bus_stale_context_and_self_shutdown() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = scope(&root, "a").await;
    let (_, sibling) = scope(&root, "b").await;
    let key = EventKey::<Ping>::named("scoped");
    ctx.events()
        .on_sync(&ctx, &key, |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| {
            Ok(Some(8))
        })
        .unwrap();
    assert_eq!(
        ctx.events().bail_sync(&ctx, &key, &Ping(0)).unwrap(),
        Some(8)
    );
    assert_eq!(
        root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
        Some(8)
    );
    assert_eq!(
        sibling
            .events()
            .bail_sync(&sibling, &key, &Ping(0))
            .unwrap(),
        Some(8)
    );
    sibling
        .events()
        .on_sync(&sibling, &key, |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| {
            Ok(Some(9))
        })
        .unwrap();
    assert_eq!(
        sibling
            .events()
            .bail_sync(&sibling, &key, &Ping(0))
            .unwrap(),
        Some(8)
    );
    assert_eq!(
        ctx.events().bail_sync(&ctx, &key, &Ping(0)).unwrap(),
        Some(8)
    );
    assert_eq!(
        sibling
            .events()
            .waterfall_sync(&sibling, &key, &Ping(0), |_, _| {
                panic!("the sibling's own listener answers without next")
            })
            .unwrap(),
        Some(8)
    );
    view.restart().await.unwrap();
    assert!(matches!(
        ctx.events().bail_sync(&ctx, &key, &Ping(0)),
        Err(CordisError::StaleGeneration { .. })
    ));
    assert!(ctx
        .events()
        .on_sync(&ctx, &key, |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| Ok(
            None
        ))
        .is_err());
    view.shutdown().await.unwrap();
    let own = root.clone();
    root.events()
        .on_sync(
            &root,
            &EventKey::of(),
            move |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| {
                drop(own.shutdown());
                Ok(Some(1))
            },
        )
        .unwrap();
    assert_eq!(
        root.events()
            .bail_sync(&root, &EventKey::of(), &Ping(0))
            .unwrap(),
        Some(1)
    );
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn thousand_registrations_and_disposals_leave_no_subscriptions() {
    let root = Ctx::root().unwrap();
    for i in 0..1000 {
        let key = EventKey::<Ping>::dynamic(format!("room/{i}"));
        let a = root
            .events()
            .on_sync(&root, &key, |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| {
                Ok(None)
            })
            .unwrap();
        let b = root
            .events()
            .on_sync_pattern(
                &root,
                &EventPattern::prefix("room/"),
                |_: &Ctx, _: EventKey<Ping>, _: &Ping, _: SyncCall<'_, u64>| Ok(None),
            )
            .unwrap();
        assert_eq!(
            root.events().bail_sync(&root, &key, &Ping(0)).unwrap(),
            None
        );
        a.dispose().await.unwrap();
        b.dispose().await.unwrap();
        assert!(root.events().subscriptions().is_empty());
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
        root.events()
            .on_pattern(
                &owner,
                &EventPattern::prefix("room/"),
                GatePattern {
                    entered: entered.clone(),
                    release: release.clone(),
                    log: log.clone(),
                },
            )
            .unwrap();
        root.events()
            .emit(&root, &EventKey::named("room/a"), Arc::new(Ping(0)))
            .unwrap();
        entered.notified().await;
        let mut shutdown = pin!(if root_owner {
            root.shutdown()
        } else {
            view.shutdown()
        });
        pending(shutdown.as_mut()).await;
        // Async dispatch still returns Ok for an old context, but the
        // closing pattern owner cannot admit another call.
        root.events()
            .serial(&root, &EventKey::named("room/new"), Arc::new(Ping(1)))
            .await
            .unwrap();
        assert!(root
            .events()
            .subscriptions()
            .iter()
            .all(|entry| entry.calls == 1));
        pending(shutdown.as_mut()).await;
        release.add_permits(1);
        shutdown.await.unwrap();
        assert_eq!(log.lock().unwrap().len(), 1);
        assert!(root.events().subscriptions().is_empty());
        root.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn sync_observer_can_register_before_snapshot_or_close_admission() {
    let root = Ctx::root().unwrap();
    let observed = root.clone();
    let once = AtomicUsize::new(0);
    root.events()
        .observe_dispatch(&root, move |attempt| {
            if attempt.mode == Mode::Bail && once.fetch_add(1, Ordering::SeqCst) == 0 {
                observed
                    .events()
                    .on_sync(
                        &observed,
                        &EventKey::of(),
                        |_: &Ctx, _: &Ping, _: SyncCall<'_, u64>| Ok(Some(33)),
                    )
                    .unwrap();
            }
        })
        .unwrap();
    assert_eq!(
        root.events()
            .bail_sync(&root, &EventKey::of(), &Ping(0))
            .unwrap(),
        Some(33)
    );
    let body_ctx = root.clone();
    root.events()
        .on_sync(
            &root,
            &EventKey::named("body"),
            move |_: &Ctx, event: &Ping, _: SyncCall<'_, u64>| match event.0 {
                // The body dispatches its own name once more.
                0 => body_ctx
                    .events()
                    .bail_sync(&body_ctx, &EventKey::named("body"), &Ping(1))
                    .map(|inner| inner.map(|inner| inner + 1)),
                _ => Ok(Some(40)),
            },
        )
        .unwrap();
    assert_eq!(
        root.events()
            .waterfall_sync(&root, &EventKey::named("body"), &Ping(0), |_, _| Ok(Some(
                0
            )))
            .unwrap(),
        Some(41)
    );
    let closing = root.clone();
    root.events()
        .observe_dispatch(&root, move |attempt| {
            if attempt.mode == Mode::WaterfallSync {
                drop(closing.shutdown());
            }
        })
        .unwrap();
    // New behaviour: admission is checked before the observers, which cannot
    // veto the dispatch; the listener of the root that is now closing is
    // skipped (liveness), so only the terminal answers. A dispatch started
    // after that is rejected.
    let mut called = false;
    assert_eq!(
        root.events()
            .waterfall_sync(&root, &EventKey::of(), &Ping(0), |_, _| {
                called = true;
                Ok(Some(0))
            })
            .unwrap(),
        Some(0)
    );
    assert!(called);
    assert!(matches!(
        root.events().bail_sync(&root, &EventKey::of(), &Ping(0)),
        Err(CordisError::Closed)
    ));
    root.shutdown().await.unwrap();
}
