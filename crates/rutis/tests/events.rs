//! Contract tests of the unified event model in the kernel (#464): one
//! listener table, five modes, scope, filter, liveness, remote entries
//! (design `docs/design-events-2026-10-18.md`, protocol spec
//! `docs/design-events-protocol-2026-10-18.md`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{
    Candidate, Dispatch, EntryCall, FilterAnswer, Payload, RawTerminal, RemoteCall, RemoteEntry,
    RemoteListener, RemoteSyncCall, TableChange, Value,
};
use rutis::{
    BoxFuture, Call, CordisError, Ctx, Effect, ErrorSink, Event, EventKey, EventOptions,
    EventPattern, FiberView, ListenerInfo, ListenerKind, Mode, NextMisuse, Plugin, Scope, SyncCall,
};
use serde::{Deserialize, Serialize};

// ── fixtures ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Ping(i64);
impl Event for Ping {
    const NAME: &'static str = "t/ping";
    type Value = String;
}

/// Another Rust type under the same name: it hears `Ping`s.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PingAlias(i64);
impl Event for PingAlias {
    const NAME: &'static str = "t/ping";
    type Value = String;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Flag;
impl Event for Flag {
    const NAME: &'static str = "t/flag";
    type Value = serde_json::Value;
}

type Log = Arc<Mutex<Vec<String>>>;

fn log() -> Log {
    Arc::default()
}

fn push(log: &Log, entry: impl Into<String>) {
    log.lock().unwrap().push(entry.into());
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// Gives an async closure listener its higher-ranked signature.
fn on<E: Event, F>(f: F) -> F
where
    F: for<'a> Fn(
            &'a Ctx,
            &'a E,
            Call<'a, E::Value>,
        ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>
        + Send
        + Sync
        + 'static,
{
    f
}

/// The same for a prefix listener.
fn on_prefix<E: Event, F>(f: F) -> F
where
    F: for<'a> Fn(
            &'a Ctx,
            EventKey<E>,
            &'a E,
            Call<'a, E::Value>,
        ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>
        + Send
        + Sync
        + 'static,
{
    f
}

/// A listener that logs `label` and returns `value`.
fn returns<E: Event>(
    log: &Log,
    label: &'static str,
    value: Option<E::Value>,
) -> impl rutis::Listener<E>
where
    E::Value: Clone + Sync,
{
    let log = log.clone();
    on::<E, _>(move |_, _, _| {
        push(&log, label);
        let value = value.clone();
        Box::pin(async move { Ok(value) })
    })
}

struct Apply<F>(&'static str, F);
impl<F> Plugin for Apply<F>
where
    F: Fn(&Ctx) -> Result<(), CordisError> + Send + Sync + 'static,
{
    fn name(&self) -> &str {
        self.0
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move { (self.1)(ctx).map(|()| Effect::Done) })
    }
}

/// A child plugin of `parent` and its context.
async fn child(parent: &Ctx, name: &'static str) -> (FiberView, Ctx) {
    let slot: Arc<Mutex<Option<Ctx>>> = Arc::default();
    let out = slot.clone();
    let view = parent.plugin(Apply(name, move |ctx: &Ctx| {
        *out.lock().unwrap() = Some(ctx.clone());
        Ok(())
    }));
    (&view).await.unwrap();
    let ctx = slot.lock().unwrap().take().unwrap();
    (view, ctx)
}

/// A child marked as a scope of `group`.
async fn scope(parent: &Ctx, group: &'static str) -> (FiberView, Ctx) {
    let (view, ctx) = child(parent, group).await;
    ctx.mark_scope(group).unwrap();
    (view, ctx)
}

fn sink() -> (ErrorSink, Arc<Mutex<Vec<Arc<CordisError>>>>) {
    let errors: Arc<Mutex<Vec<Arc<CordisError>>>> = Arc::default();
    let out = errors.clone();
    let sink: ErrorSink = Arc::new(move |error| out.lock().unwrap().push(error));
    (sink, errors)
}

fn root_with_sink() -> (Ctx, Arc<Mutex<Vec<Arc<CordisError>>>>) {
    let (sink, errors) = sink();
    (
        Ctx::root_with_sink(tokio::runtime::Handle::current(), sink),
        errors,
    )
}

const PING: EventKey<Ping> = EventKey::of();

// ── one table, every mode ───────────────────────────────────────

/// A listener registered once is called by every mode, and sees which.
///
/// guarantee: events/one-table
#[tokio::test]
async fn one_listener_is_called_by_every_mode() {
    let root = Ctx::root().unwrap();
    let modes = log();
    let seen = modes.clone();
    root.events()
        .on_sync(
            &root,
            &PING,
            move |_: &Ctx, _: &Ping, call: SyncCall<'_, String>| {
                push(&seen, call.mode().as_str());
                if call.mode() == Mode::WaterfallSync {
                    call.next()
                } else {
                    Ok(None)
                }
            },
        )
        .unwrap();
    let bus = root.events();
    bus.emit(&root, &PING, Arc::new(Ping(1))).unwrap();
    rutis_test_support::until(|| entries(&modes).len() == 1, "emit called it").await;
    bus.parallel(&root, &PING, Arc::new(Ping(1))).await.unwrap();
    bus.serial(&root, &PING, Arc::new(Ping(1))).await.unwrap();
    bus.waterfall(&root, &PING, &Ping(1), ()).await.unwrap();
    bus.bail_sync(&root, &PING, &Ping(1)).unwrap();
    bus.waterfall_sync(&root, &PING, &Ping(1), |_, _| Ok(None))
        .unwrap();
    assert_eq!(
        entries(&modes),
        [
            "emit",
            "parallel",
            "serial",
            "waterfall",
            "bail",
            "waterfall-sync"
        ]
    );
    root.shutdown().await.unwrap();
}

/// `next` is only in a waterfall; a synchronous listener cannot call it in
/// an async waterfall.
///
/// guarantee: events/next-misuse
#[tokio::test]
async fn next_outside_a_waterfall_is_an_error() {
    let root = Ctx::root().unwrap();
    root.events()
        .on(
            &root,
            &PING,
            on::<Ping, _>(|_, _, call| Box::pin(async move { call.next().await })),
        )
        .unwrap();
    let error = root
        .events()
        .serial(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            CordisError::WaterfallNext {
                misuse: NextMisuse::NotWaterfall(Mode::Serial),
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(error.wire_name(), Some("WaterfallNext"));
    assert_eq!(
        error.to_string(),
        "next is available only in a waterfall (this is serial)"
    );

    let key = EventKey::<Ping>::named("t/sync-next");
    root.events()
        .on_sync(
            &root,
            &key,
            |_: &Ctx, _: &Ping, call: SyncCall<'_, String>| call.next(),
        )
        .unwrap();
    let error = root
        .events()
        .waterfall(&root, &key, &Ping(0), ())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "a synchronous listener cannot call next in an asynchronous waterfall; register it with on"
    );
    root.shutdown().await.unwrap();
}

// ── emit ────────────────────────────────────────────────────────

/// `emit` returns at once, starts every listener in registration order
/// without waiting for any, and a failing listener neither stops the others
/// nor reaches the emitter: it goes to the error sink.
///
/// guarantee: events/emit
#[tokio::test]
async fn emit_does_not_wait_and_contains_failures() {
    let (root, errors) = root_with_sink();
    let order = log();
    let (release, gate) = tokio::sync::watch::channel(false);
    let first = order.clone();
    root.events()
        .on(
            &root,
            &PING,
            on::<Ping, _>(move |_, _, _| {
                push(&first, "a:start");
                let mut gate = gate.clone();
                let first = first.clone();
                Box::pin(async move {
                    let _ = gate.wait_for(|open| *open).await;
                    push(&first, "a:end");
                    Ok(None)
                })
            }),
        )
        .unwrap();
    let failing = order.clone();
    root.events()
        .on(
            &root,
            &PING,
            on::<Ping, _>(move |_, _, _| {
                push(&failing, "b");
                Box::pin(async { Err(CordisError::PluginFailed("b failed".into())) })
            }),
        )
        .unwrap();
    root.events()
        .on(&root, &PING, returns::<Ping>(&order, "c", None))
        .unwrap();
    root.events().emit(&root, &PING, Arc::new(Ping(1))).unwrap();
    rutis_test_support::until(|| entries(&order).len() == 3, "every listener started").await;
    assert_eq!(
        entries(&order),
        ["a:start", "b", "c"],
        "a did not hold up b and c"
    );
    release.send(true).unwrap();
    rutis_test_support::until(|| entries(&order).len() == 4, "a finished").await;
    let errors = errors.lock().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    match &*errors[0] {
        CordisError::ListenerFailed {
            name,
            listener,
            error,
        } => {
            assert_eq!(name, "t/ping");
            assert_eq!(listener, "plugin root");
            assert!(error.to_string().contains("b failed"));
        }
        other => panic!("{other:?}"),
    }
}

/// The calls of one name's emits start in emit order at each listener, also
/// when a listener is still busy with an earlier one.
///
/// guarantee: events/emit-order
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emits_of_one_name_start_in_emit_order() {
    let root = Ctx::root().unwrap();
    let seen = log();
    let out = seen.clone();
    root.events()
        .on(
            &root,
            &PING,
            on::<Ping, _>(move |_, event, _| {
                push(&out, event.0.to_string());
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    Ok(None)
                })
            }),
        )
        .unwrap();
    for i in 0..200 {
        root.events().emit(&root, &PING, Arc::new(Ping(i))).unwrap();
    }
    rutis_test_support::until(|| entries(&seen).len() == 200, "all delivered").await;
    let expected: Vec<String> = (0..200).map(|i| i.to_string()).collect();
    assert_eq!(entries(&seen), expected);
    root.shutdown().await.unwrap();
}

// ── parallel ────────────────────────────────────────────────────

/// `parallel` starts in registration order, waits for all, and aggregates
/// failures in registration order, also a single one.
///
/// guarantee: events/parallel
#[tokio::test]
async fn parallel_aggregates_in_registration_order() {
    let root = Ctx::root().unwrap();
    let started = log();
    let finished = Arc::new(AtomicUsize::new(0));
    for (label, delay, fails) in [("a", 50, true), ("b", 0, true), ("c", 10, false)] {
        let started = started.clone();
        let finished = finished.clone();
        root.events()
            .on(
                &root,
                &PING,
                on::<Ping, _>(move |_, _, _| {
                    push(&started, label);
                    let finished = finished.clone();
                    Box::pin(async move {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        finished.fetch_add(1, Ordering::SeqCst);
                        if fails {
                            Err(CordisError::PluginFailed(label.into()))
                        } else {
                            Ok(Some("ignored".into()))
                        }
                    })
                }),
            )
            .unwrap();
    }
    let error = root
        .events()
        .parallel(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap_err();
    assert_eq!(entries(&started), ["a", "b", "c"]);
    assert_eq!(finished.load(Ordering::SeqCst), 3, "waited for all");
    match &error {
        CordisError::ListenersFailed { name, errors } => {
            assert_eq!(name, "t/ping");
            let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            assert_eq!(messages, ["plugin failed: a", "plugin failed: b"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(error.wire_name(), Some("AggregateError"));
    assert_eq!(error.to_string(), "2 listeners of t/ping failed");

    let one = EventKey::<Ping>::named("t/one");
    root.events()
        .on(
            &root,
            &one,
            on::<Ping, _>(|_, _, _| Box::pin(async { Err(CordisError::Closed) })),
        )
        .unwrap();
    let error = root
        .events()
        .parallel(&root, &one, Arc::new(Ping(0)))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, CordisError::ListenersFailed { errors, .. } if errors.len() == 1),
        "{error:?}"
    );
    root.shutdown().await.unwrap();
}

// ── serial / bail ───────────────────────────────────────────────

/// `None` and `false` are no result; anything else is (`0`, `""`).
///
/// guarantee: events/no-result
#[tokio::test]
async fn serial_and_bail_skip_no_result() {
    let root = Ctx::root().unwrap();
    let order = log();
    let key = EventKey::<Flag>::of();
    use serde_json::json;
    for (label, value) in [
        ("a", None),
        ("b", Some(json!(false))),
        ("c", Some(json!(null))),
        ("d", Some(json!(0))),
        ("e", Some(json!("E"))),
    ] {
        let order = order.clone();
        root.events()
            .on_sync(&root, &key, move |_: &Ctx, _: &Flag, _: SyncCall<'_, _>| {
                push(&order, label);
                Ok(value.clone())
            })
            .unwrap();
    }
    let value = root
        .events()
        .serial(&root, &key, Arc::new(Flag))
        .await
        .unwrap();
    assert_eq!(value, Some(json!(0)));
    assert_eq!(entries(&order), ["a", "b", "c", "d"]);
    let value = root.events().bail_sync(&root, &key, &Flag).unwrap();
    assert_eq!(value, Some(json!(0)));

    // A Rust `bool` value: `Some(false)` is no result too.
    #[derive(Serialize, Deserialize)]
    struct Ask;
    impl Event for Ask {
        const NAME: &'static str = "t/ask";
        type Value = bool;
    }
    let ask = EventKey::<Ask>::of();
    root.events()
        .on_sync(&root, &ask, |_: &Ctx, _: &Ask, _: SyncCall<'_, bool>| {
            Ok(Some(false))
        })
        .unwrap();
    assert_eq!(root.events().bail_sync(&root, &ask, &Ask).unwrap(), None);
    root.events()
        .on_sync(&root, &ask, |_: &Ctx, _: &Ask, _: SyncCall<'_, bool>| {
            Ok(Some(true))
        })
        .unwrap();
    assert_eq!(
        root.events()
            .serial(&root, &ask, Arc::new(Ask))
            .await
            .unwrap(),
        Some(true)
    );
    root.shutdown().await.unwrap();
}

/// A listener's error stops `serial` and is returned as it is.
///
/// guarantee: events/serial-error
#[tokio::test]
async fn serial_stops_at_an_error() {
    let root = Ctx::root().unwrap();
    let order = log();
    let failing = order.clone();
    root.events()
        .on(
            &root,
            &PING,
            on::<Ping, _>(move |_, _, _| {
                push(&failing, "a");
                Box::pin(async { Err(CordisError::Closed) })
            }),
        )
        .unwrap();
    root.events()
        .on(&root, &PING, returns::<Ping>(&order, "b", Some("B".into())))
        .unwrap();
    let error = root
        .events()
        .serial(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap_err();
    assert!(matches!(error, CordisError::Closed), "{error:?}");
    assert_eq!(entries(&order), ["a"]);
    root.shutdown().await.unwrap();
}

/// A synchronous dispatch meeting an async listener fails with
/// `AsyncListener`; listeners before it ran.
///
/// guarantee: events/async-listener
#[tokio::test]
async fn bail_meets_an_async_listener() {
    let root = Ctx::root().unwrap();
    let order = log();
    let first = order.clone();
    root.events()
        .on_sync(
            &root,
            &PING,
            move |_: &Ctx, _: &Ping, _: SyncCall<'_, String>| {
                push(&first, "a");
                Ok(None)
            },
        )
        .unwrap();
    root.events()
        .on(&root, &PING, returns::<Ping>(&order, "b", Some("B".into())))
        .unwrap();
    let error = root.events().bail_sync(&root, &PING, &Ping(0)).unwrap_err();
    assert_eq!(entries(&order), ["a"]);
    assert_eq!(error.wire_name(), Some("AsyncListener"));
    assert_eq!(
        error.to_string(),
        "listener of t/ping in plugin root is asynchronous and cannot run in a synchronous bail"
    );
    let only_async = EventKey::<Ping>::named("t/only-async");
    root.events()
        .on(&root, &only_async, returns::<Ping>(&order, "c", None))
        .unwrap();
    let error = root
        .events()
        .waterfall_sync(&root, &only_async, &Ping(0), |_, _| Ok(None))
        .unwrap_err();
    assert!(
        error.to_string().ends_with("synchronous waterfall-sync"),
        "{error}"
    );
    root.shutdown().await.unwrap();
}

// ── waterfall ───────────────────────────────────────────────────

fn wrap(log: &Log, label: &'static str) -> impl rutis::Listener<Ping> {
    let log = log.clone();
    on::<Ping, _>(move |_, _, call| {
        push(&log, label);
        Box::pin(async move {
            let inner = call.next().await?.unwrap_or_default();
            Ok(Some(format!("{label}{inner}")))
        })
    })
}

/// Outer to inner with `next`; not calling it vetoes the rest; the last
/// `next` is the terminal; the outermost value comes back as it is.
///
/// guarantee: events/waterfall
#[tokio::test]
async fn waterfall_chains_vetoes_and_ends_in_the_terminal() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.events().on(&root, &PING, wrap(&order, "a")).unwrap();
    root.events().on(&root, &PING, wrap(&order, "b")).unwrap();
    let terminal_log = order.clone();
    let terminal = Terminal(
        move |_: &Ctx, event: &Ping| -> BoxFuture<'static, Result<Option<String>, CordisError>> {
            push(&terminal_log, "terminal");
            let n = event.0;
            Box::pin(async move { Ok(Some(format!("t{n}"))) })
        },
    );
    let out = root
        .events()
        .waterfall(&root, &PING, &Ping(7), terminal)
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("abt7"));
    assert_eq!(entries(&order), ["a", "b", "terminal"]);

    // Without a terminal, the last next gives None.
    let out = root
        .events()
        .waterfall(&root, &PING, &Ping(7), ())
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("ab"));

    // A veto.
    let veto = EventKey::<Ping>::named("t/veto");
    root.events().on(&root, &veto, wrap(&order, "x")).unwrap();
    root.events()
        .on(&root, &veto, returns::<Ping>(&order, "v", Some("v".into())))
        .unwrap();
    root.events()
        .on(&root, &veto, wrap(&order, "never"))
        .unwrap();
    let out = root
        .events()
        .waterfall(&root, &veto, &Ping(0), ())
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("xv"));
    assert!(!entries(&order).contains(&"never".to_owned()));

    // No listeners: the terminal's value.
    let empty = EventKey::<Ping>::named("t/empty");
    let out = root
        .events()
        .waterfall(
            &root,
            &empty,
            &Ping(3),
            Terminal(
                |_: &Ctx, e: &Ping| -> BoxFuture<'static, Result<Option<String>, CordisError>> {
                    let n = e.0;
                    Box::pin(async move { Ok(Some(format!("t{n}"))) })
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("t3"));

    // The outermost value as it is: `false` stays `false`.
    let flag = EventKey::<Flag>::of();
    root.events()
        .on_sync(&root, &flag, |_: &Ctx, _: &Flag, _: SyncCall<'_, _>| {
            Ok(Some(serde_json::json!(false)))
        })
        .unwrap();
    let out = root
        .events()
        .waterfall(&root, &flag, &Flag, ())
        .await
        .unwrap();
    assert_eq!(out, Some(serde_json::json!(false)));
    root.shutdown().await.unwrap();
}

/// A terminal from a closure returning a `'static` future.
struct Terminal<F>(F);
impl<F> rutis::Terminal<Ping> for Terminal<F>
where
    F: Fn(&Ctx, &Ping) -> BoxFuture<'static, Result<Option<String>, CordisError>> + Send + 'static,
{
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        event: &'a Ping,
    ) -> BoxFuture<'a, Result<Option<String>, CordisError>> {
        (self.0)(ctx, event)
    }
}

/// `waterfall_sync` chains synchronous listeners into a borrowed terminal.
///
/// guarantee: events/waterfall-sync
#[tokio::test]
async fn waterfall_sync_chains_into_a_borrowed_terminal() {
    let root = Ctx::root().unwrap();
    for label in ["a", "b"] {
        root.events()
            .on_sync(
                &root,
                &PING,
                move |_: &Ctx, _: &Ping, call: SyncCall<'_, String>| {
                    Ok(Some(format!("{label}{}", call.next()?.unwrap_or_default())))
                },
            )
            .unwrap();
    }
    let local = String::from("t");
    let out = root
        .events()
        .waterfall_sync(&root, &PING, &Ping(0), |_, _| Ok(Some(local.clone())))
        .unwrap();
    assert_eq!(out.as_deref(), Some("abt"));
    root.shutdown().await.unwrap();
}

// ── names, types, prefixes, order ───────────────────────────────

/// Listeners are found by name: a listener of another Rust type with the
/// same name hears the event, converted by serde, and its result comes back
/// as the emitter's type.
///
/// guarantee: events/by-name
#[tokio::test]
async fn listeners_of_another_type_hear_the_same_name() {
    let root = Ctx::root().unwrap();
    root.events()
        .on(
            &root,
            &EventKey::<PingAlias>::of(),
            on::<PingAlias, _>(|_, event, _| {
                let n = event.0;
                Box::pin(async move { Ok(Some(format!("alias {n}"))) })
            }),
        )
        .unwrap();
    let out = root
        .events()
        .serial(&root, &PING, Arc::new(Ping(5)))
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("alias 5"));
    // A name of its own does not cross.
    let other = EventKey::<Ping>::named("t/other");
    assert_eq!(
        root.events()
            .serial(&root, &other, Arc::new(Ping(5)))
            .await
            .unwrap(),
        None
    );
    root.shutdown().await.unwrap();
}

/// Prefix entries match every name starting with the prefix and take their
/// place in registration order beside exact ones; `prepend` goes first;
/// `once` is called once.
///
/// guarantee: events/order
#[tokio::test]
async fn prefixes_prepend_and_once_keep_registration_order() {
    let root = Ctx::root().unwrap();
    let order = log();
    let key = EventKey::<Ping>::named("room/1");
    root.events()
        .on(&root, &key, returns::<Ping>(&order, "exact-1", None))
        .unwrap();
    let names = order.clone();
    root.events()
        .on_pattern(
            &root,
            &EventPattern::prefix("room/"),
            on_prefix::<Ping, _>(move |_, key, _, _| {
                push(&names, format!("prefix:{}", key.name()));
                Box::pin(async { Ok(None) })
            }),
        )
        .unwrap();
    root.events()
        .on(&root, &key, returns::<Ping>(&order, "exact-2", None))
        .unwrap();
    root.events()
        .on_opt(
            &root,
            &key,
            returns::<Ping>(&order, "first", None),
            EventOptions::default().prepend(true),
        )
        .unwrap();
    root.events()
        .once(&root, &key, returns::<Ping>(&order, "once", None))
        .unwrap();
    root.events()
        .serial(&root, &key, Arc::new(Ping(0)))
        .await
        .unwrap();
    root.events()
        .serial(&root, &key, Arc::new(Ping(0)))
        .await
        .unwrap();
    assert_eq!(
        entries(&order),
        [
            "first",
            "exact-1",
            "prefix:room/1",
            "exact-2",
            "once",
            "first",
            "exact-1",
            "prefix:room/1",
            "exact-2"
        ]
    );
    root.shutdown().await.unwrap();
}

/// Only rutis emits `rutis/*`: every mode refuses it, listening is fine.
///
/// guarantee: events/reserved
#[tokio::test]
async fn rutis_events_are_reserved() {
    let root = Ctx::root().unwrap();
    let order = log();
    let key = EventKey::<Ping>::named("rutis/row");
    root.events()
        .on_sync(&root, &key, {
            let order = order.clone();
            move |_: &Ctx, _: &Ping, _: SyncCall<'_, String>| {
                push(&order, "heard");
                Ok(None)
            }
        })
        .unwrap();
    let reserved = |error: CordisError| {
        assert_eq!(error.wire_name(), Some("ReservedEvent"));
        assert_eq!(
            error.to_string(),
            "rutis/* events are emitted by rutis only: rutis/row"
        );
    };
    let bus = root.events();
    reserved(bus.emit(&root, &key, Arc::new(Ping(0))).unwrap_err());
    reserved(
        bus.parallel(&root, &key, Arc::new(Ping(0)))
            .await
            .unwrap_err(),
    );
    reserved(
        bus.serial(&root, &key, Arc::new(Ping(0)))
            .await
            .unwrap_err(),
    );
    reserved(bus.waterfall(&root, &key, &Ping(0), ()).await.unwrap_err());
    reserved(bus.bail_sync(&root, &key, &Ping(0)).unwrap_err());
    reserved(
        bus.waterfall_sync(&root, &key, &Ping(0), |_, _| Ok(None))
            .unwrap_err(),
    );
    assert!(entries(&order).is_empty());
    bus.emit_framework(&root, &key, Arc::new(Ping(0))).unwrap();
    rutis_test_support::until(|| entries(&order) == ["heard"], "framework emit heard").await;
    root.shutdown().await.unwrap();
}

// ── scope ───────────────────────────────────────────────────────

/// Events are delivered by name on one flat bus: every live listener hears
/// every dispatch, wherever its scope is (Cordis's rule). A listener's scope
/// is a descriptor for filters, not a routing rule.
///
/// guarantee: events/flat
#[tokio::test]
async fn events_reach_every_listener_on_the_bus() {
    let root = Ctx::root().unwrap();
    let (_t1, t1) = scope(&root, "tenant").await;
    let (_s1, s1) = scope(&t1, "session").await;
    let (_t2, t2) = scope(&root, "tenant").await;
    assert_eq!(root.scope(), Vec::<Scope>::new());
    assert_eq!(
        s1.scope(),
        vec![
            Scope {
                id: t1.instance().to_string(),
                group: "tenant".into()
            },
            Scope {
                id: s1.instance().to_string(),
                group: "session".into()
            },
        ]
    );
    // A plugin inside a scope is in it.
    let (_inner, inner) = child(&s1, "inner").await;
    assert_eq!(inner.scope(), s1.scope());

    let heard = log();
    for (label, ctx) in [("r", &root), ("u", &t1), ("s", &s1), ("v", &t2)] {
        ctx.events()
            .on(ctx, &PING, returns::<Ping>(&heard, label, None))
            .unwrap();
    }
    let round = |ctx: Ctx| {
        let heard = heard.clone();
        async move {
            heard.lock().unwrap().clear();
            ctx.events()
                .parallel(&ctx, &PING, Arc::new(Ping(0)))
                .await
                .unwrap();
            let mut got = entries(&heard);
            got.sort();
            got
        }
    };
    assert_eq!(round(inner.clone()).await, ["r", "s", "u", "v"]);
    assert_eq!(round(t1.clone()).await, ["r", "s", "u", "v"]);
    assert_eq!(round(t2.clone()).await, ["r", "s", "u", "v"]);
    assert_eq!(round(root.clone()).await, ["r", "s", "u", "v"]);
    assert!(t1.mark_scope("tenant").is_ok(), "same group again is fine");
    assert!(t1.mark_scope("session").is_err());
    assert!(root.mark_scope("tenant").is_err(), "the root is no scope");
    root.shutdown().await.unwrap();
}

// ── filter ──────────────────────────────────────────────────────

/// A filter sees each non-global candidate's descriptor in registration
/// order, before any call; `global` entries skip it.
///
/// guarantee: events/filter
#[tokio::test]
async fn filters_see_descriptors_and_global_entries_skip_them() {
    let root = Ctx::root().unwrap();
    let (_a, a) = child(&root, "plugin-a").await;
    a.mark_row("row-a").unwrap();
    let (_b, b) = child(&root, "plugin-b").await;
    let (_t, t) = scope(&root, "tenant").await;
    let order = log();
    a.events()
        .on(&a, &PING, returns::<Ping>(&order, "a", None))
        .unwrap();
    b.events()
        .on(&b, &PING, returns::<Ping>(&order, "b", None))
        .unwrap();
    b.events()
        .on_opt(
            &b,
            &PING,
            returns::<Ping>(&order, "g", None),
            EventOptions::default().global(true),
        )
        .unwrap();
    t.events()
        .on(&t, &PING, returns::<Ping>(&order, "t", None))
        .unwrap();
    let seen: Arc<Mutex<Vec<ListenerInfo>>> = Arc::default();
    let into = seen.clone();
    let filtered_log = order.clone();
    root.events()
        .filter(move |info| {
            push(&filtered_log, format!("filter:{}", info.plugin));
            into.lock().unwrap().push(info.clone());
            info.row.as_deref() == Some("row-a")
        })
        .serial(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap();
    assert_eq!(
        entries(&order),
        [
            "filter:plugin-a",
            "filter:plugin-b",
            "filter:tenant",
            "a",
            "g"
        ],
        "the filter sees every non-global candidate; g skips the filter"
    );
    let first = seen.lock().unwrap()[0].clone();
    assert_eq!(
        first,
        ListenerInfo {
            name: "t/ping".into(),
            prefix: false,
            plugin: "plugin-a".into(),
            row: Some("row-a".into()),
            runtime: None,
            node: None,
            scope: vec![],
            global: false,
        }
    );

    // A failing filter fails the dispatch before any call.
    order.lock().unwrap().clear();
    let error = root
        .events()
        .filter(|_| panic!("filter broke"))
        .parallel(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("filter broke"), "{error}");
    assert!(entries(&order).is_empty(), "{:?}", entries(&order));
    root.shutdown().await.unwrap();
}

/// In `emit`, a failing filter delivers to nobody and is reported.
///
/// guarantee: events/filter-emit
#[tokio::test]
async fn a_failing_emit_filter_is_reported() {
    let (root, errors) = root_with_sink();
    let order = log();
    root.events()
        .on(&root, &PING, returns::<Ping>(&order, "a", None))
        .unwrap();
    root.events()
        .filter(|_| panic!("no"))
        .emit(&root, &PING, Arc::new(Ping(0)))
        .unwrap();
    rutis_test_support::until(|| !errors.lock().unwrap().is_empty(), "reported").await;
    assert!(
        matches!(&*errors.lock().unwrap()[0], CordisError::FilterFailed { name, .. } if name == "t/ping")
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(entries(&order).is_empty());
}

/// A filter that can fail (`try_filter`): its error, as it is, fails the
/// dispatch before any call.
///
/// guarantee: events/filter
#[tokio::test]
async fn a_try_filter_error_fails_the_dispatch_with_it() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.events()
        .on(&root, &PING, returns::<Ping>(&order, "a", None))
        .unwrap();
    let error = root
        .events()
        .try_filter(|_| {
            Err(CordisError::Validation {
                issues: vec!["no".into()],
            })
        })
        .serial(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, CordisError::Validation { issues } if issues == &["no"]),
        "{error:?}"
    );
    assert!(entries(&order).is_empty());
}

// ── lifecycle ───────────────────────────────────────────────────

/// A listener is called with the context that registered it, and its
/// fiber's unload waits for its running calls while signalling them to
/// stop (its `ctx.cancelled()`).
///
/// guarantee: events/unload-waits
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unload_cancels_and_waits_for_running_listeners() {
    let root = Ctx::root().unwrap();
    let order = log();
    let out = order.clone();
    let view = root.plugin(Apply("waiter", move |ctx: &Ctx| {
        let out = out.clone();
        let own = ctx.instance();
        ctx.events().on(
            ctx,
            &PING,
            on::<Ping, _>(move |ctx, _, _| {
                assert_eq!(ctx.instance(), own, "called with its own context");
                push(&out, "started");
                let out = out.clone();
                Box::pin(async move {
                    ctx.cancelled().await;
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    push(&out, "cancelled");
                    Ok(None)
                })
            }),
        )?;
        Ok(())
    }));
    (&view).await.unwrap();
    for mode in ["emit", "parallel"] {
        order.lock().unwrap().clear();
        if mode == "emit" {
            root.events().emit(&root, &PING, Arc::new(Ping(0))).unwrap();
        } else {
            let root = root.clone();
            tokio::spawn(async move {
                root.events()
                    .parallel(&root, &PING, Arc::new(Ping(0)))
                    .await
            });
        }
        rutis_test_support::until(|| entries(&order) == ["started"], "listener running").await;
        view.restart().await.unwrap();
        assert_eq!(
            entries(&order),
            ["started", "cancelled"],
            "{mode}: the unload waited for the call"
        );
    }
    root.shutdown().await.unwrap();
}

/// Once a listener's fiber is unloading or gone, no new call reaches it.
///
/// guarantee: events/liveness
#[tokio::test]
async fn unloaded_listeners_are_not_called() {
    let root = Ctx::root().unwrap();
    let order = log();
    let (view, ctx) = child(&root, "gone").await;
    ctx.events()
        .on_sync(&ctx, &PING, {
            let order = order.clone();
            move |_: &Ctx, _: &Ping, _: SyncCall<'_, String>| {
                push(&order, "gone");
                Ok(Some("G".into()))
            }
        })
        .unwrap();
    root.events()
        .on(&root, &PING, returns::<Ping>(&order, "root", None))
        .unwrap();
    view.dispose().await.unwrap();
    assert_eq!(
        root.events()
            .serial(&root, &PING, Arc::new(Ping(0)))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        root.events()
            .bail_sync(&root, &PING, &Ping(0))
            .unwrap_err()
            .wire_name(),
        Some("AsyncListener")
    );
    assert_eq!(entries(&order), ["root"]);
    root.shutdown().await.unwrap();
}

// ── remote entries ──────────────────────────────────────────────

struct Remote {
    label: &'static str,
    kind: ListenerKind,
    log: Log,
    /// What it does: return a value, or call next and prefix its value.
    wrap: bool,
    next_twice: bool,
}

impl Remote {
    fn new(label: &'static str, log: &Log) -> Self {
        Self {
            label,
            kind: ListenerKind::Any,
            log: log.clone(),
            wrap: false,
            next_twice: false,
        }
    }
}

impl RemoteListener for Remote {
    fn kind(&self) -> ListenerKind {
        self.kind
    }
    fn id(&self) -> Option<&str> {
        Some(self.label)
    }
    fn call<'a>(
        &'a self,
        call: RemoteCall<'a>,
    ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
        push(
            &self.log,
            format!(
                "{}:{}:{}",
                self.label,
                call.mode(),
                call.event().to_json().unwrap()
            ),
        );
        Box::pin(async move {
            match call.next() {
                Some(next) if self.wrap => {
                    let inner = next.call().await?;
                    if self.next_twice {
                        next.call().await?;
                    }
                    let inner: String = match inner {
                        Some(value) => value.into_typed(&rutis::events::JsonTranscoder)?,
                        None => String::new(),
                    };
                    Ok(Some(Value::new(format!("{}{inner}", self.label))))
                }
                _ => Ok(Some(Value::new(serde_json::json!(self.label)))),
            }
        })
    }
    fn call_sync(&self, call: RemoteSyncCall<'_>) -> Result<Option<Value>, CordisError> {
        push(&self.log, format!("{}:{}", self.label, call.mode()));
        match call.next() {
            Some(next) if self.wrap => {
                let inner = next.call()?;
                let inner: String = match inner {
                    Some(value) => value.into_typed(&rutis::events::JsonTranscoder)?,
                    None => String::new(),
                };
                Ok(Some(Value::new(format!("{}{inner}", self.label))))
            }
            _ => Ok(Some(Value::new(self.label.to_string()))),
        }
    }
}

fn remote_info(plugin: &str, runtime: &str, scope: Vec<Scope>) -> ListenerInfo {
    ListenerInfo {
        name: String::new(),
        prefix: false,
        plugin: plugin.into(),
        row: Some(plugin.into()),
        runtime: Some(runtime.into()),
        node: None,
        scope,
        global: false,
    }
}

fn remote_entry(name: &str, info: ListenerInfo, options: EventOptions) -> RemoteEntry {
    RemoteEntry {
        name: name.into(),
        prefix: false,
        info,
        options,
        origin: 7,
    }
}

/// A remote entry is one more entry of the table: registration order,
/// payload as JSON, results converted to the emitter's type, the filter
/// sees its descriptor, its scope is a descriptor, its disposer withdraws
/// it.
///
/// guarantee: events/remote-entry
#[tokio::test]
async fn remote_entries_take_part_like_any_listener() {
    let root = Ctx::root().unwrap();
    let (_t, t) = scope(&root, "tenant").await;
    let order = log();
    root.events()
        .on(&root, &PING, returns::<Ping>(&order, "local", None))
        .unwrap();
    let withdraw = root
        .events()
        .on_remote(
            &root,
            remote_entry(
                "t/ping",
                remote_info("py-row", "py", vec![]),
                EventOptions::default(),
            ),
            Arc::new(Remote::new("py", &order)),
        )
        .unwrap();
    root.events()
        .on_remote(
            &root,
            remote_entry(
                "t/ping",
                remote_info("tenant-row", "go", t.scope()),
                EventOptions::default(),
            ),
            Arc::new(Remote::new("go", &order)),
        )
        .unwrap();
    let out = root
        .events()
        .serial(&root, &PING, Arc::new(Ping(4)))
        .await
        .unwrap();
    assert_eq!(
        out.as_deref(),
        Some("py"),
        "JSON result converted to String"
    );
    assert_eq!(
        entries(&order),
        ["local", "py:serial:4"],
        "registration order: py before the tenant entry"
    );

    // From inside the tenant, both; a filter sees the remote descriptor.
    order.lock().unwrap().clear();
    let seen = log();
    let into = seen.clone();
    t.events()
        .filter(move |info| {
            push(&into, format!("{}@{:?}", info.plugin, info.runtime));
            info.runtime.as_deref() == Some("go")
        })
        .parallel(&t, &PING, Arc::new(Ping(1)))
        .await
        .unwrap();
    assert_eq!(
        entries(&seen),
        [
            "root@None",
            "py-row@Some(\"py\")",
            "tenant-row@Some(\"go\")"
        ]
    );
    assert_eq!(entries(&order), ["go:parallel:1"]);

    // Withdrawn: no longer called.
    withdraw.dispose().await.unwrap();
    order.lock().unwrap().clear();
    root.events()
        .serial(&root, &PING, Arc::new(Ping(2)))
        .await
        .unwrap();
    assert_eq!(entries(&order), ["local", "go:serial:2"]);
    root.shutdown().await.unwrap();
}

/// A remote entry in a waterfall gets a `'static` next: it runs the rest of
/// the chain once; a second call fails, as does a call after the entry's
/// call ended.
///
/// guarantee: events/remote-next
#[tokio::test]
async fn remote_waterfall_next_runs_once() {
    let root = Ctx::root().unwrap();
    let order = log();
    let mut remote = Remote::new("r", &order);
    remote.wrap = true;
    root.events()
        .on_remote(
            &root,
            remote_entry(
                "t/ping",
                remote_info("r", "node", vec![]),
                EventOptions::default(),
            ),
            Arc::new(remote),
        )
        .unwrap();
    root.events().on(&root, &PING, wrap(&order, "l")).unwrap();
    let out = root
        .events()
        .waterfall(&root, &PING, &Ping(0), ())
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("rl"));

    let twice = EventKey::<Ping>::named("t/twice");
    let mut remote = Remote::new("r", &order);
    remote.wrap = true;
    remote.next_twice = true;
    root.events()
        .on_remote(
            &root,
            remote_entry(
                "t/twice",
                remote_info("r", "node", vec![]),
                EventOptions::default(),
            ),
            Arc::new(remote),
        )
        .unwrap();
    let error = root
        .events()
        .waterfall(&root, &twice, &Ping(0), ())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "next of t/twice was already called");

    // Kept past its call: "ended".
    struct Keeper(Mutex<Option<rutis::events::RemoteNext>>);
    impl RemoteListener for Keeper {
        fn kind(&self) -> ListenerKind {
            ListenerKind::Async
        }
        fn call<'a>(
            &'a self,
            call: RemoteCall<'a>,
        ) -> BoxFuture<'a, Result<Option<Value>, CordisError>> {
            *self.0.lock().unwrap() = call.next();
            Box::pin(async { Ok(None) })
        }
        fn call_sync(&self, _: RemoteSyncCall<'_>) -> Result<Option<Value>, CordisError> {
            unreachable!()
        }
    }
    let keeper = Arc::new(Keeper(Mutex::new(None)));
    let kept = EventKey::<Ping>::named("t/kept");
    root.events()
        .on_remote(
            &root,
            remote_entry(
                "t/kept",
                remote_info("k", "node", vec![]),
                EventOptions::default(),
            ),
            keeper.clone(),
        )
        .unwrap();
    root.events()
        .waterfall(&root, &kept, &Ping(0), ())
        .await
        .unwrap();
    let next = keeper.0.lock().unwrap().take().unwrap();
    let error = next.call().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "next of t/kept was called after its waterfall ended"
    );
    // An async remote entry in bail: AsyncListener, not called.
    let error = root.events().bail_sync(&root, &kept, &Ping(0)).unwrap_err();
    assert_eq!(error.wire_name(), Some("AsyncListener"));
    root.shutdown().await.unwrap();
}

/// Synchronous modes call a remote entry's `call_sync` inline; its next
/// runs the rest of the chain on the dispatching thread.
///
/// guarantee: events/remote-sync
#[tokio::test]
async fn remote_entries_in_synchronous_modes() {
    let root = Ctx::root().unwrap();
    let order = log();
    let mut remote = Remote::new("r", &order);
    remote.wrap = true;
    root.events()
        .on_remote(
            &root,
            remote_entry(
                "t/ping",
                remote_info("r", "go", vec![]),
                EventOptions::default(),
            ),
            Arc::new(remote),
        )
        .unwrap();
    root.events()
        .on_sync(
            &root,
            &PING,
            |_: &Ctx, _: &Ping, call: SyncCall<'_, String>| {
                Ok(Some(format!("s{}", call.next()?.unwrap_or_default())))
            },
        )
        .unwrap();
    let out = root
        .events()
        .waterfall_sync(&root, &PING, &Ping(0), |_, _| Ok(Some("t".into())))
        .unwrap();
    assert_eq!(out.as_deref(), Some("rst"));
    let out = root.events().bail_sync(&root, &PING, &Ping(0)).unwrap();
    assert_eq!(out.as_deref(), Some("r"));
    assert_eq!(entries(&order), ["r:waterfall-sync", "r:bail"]);
    root.shutdown().await.unwrap();
}

/// The table watcher gets the current entries, then every change; a Rust
/// registration is reported before `on` returns.
///
/// guarantee: events/table-watch
#[tokio::test]
async fn the_table_is_watched() {
    let root = Ctx::root().unwrap();
    root.events()
        .on(&root, &PING, returns::<Ping>(&log(), "a", None))
        .unwrap();
    let changes: Arc<Mutex<Vec<String>>> = Arc::default();
    let into = changes.clone();
    let (snapshot, _watch) = root
        .events()
        .watch_table(&root, move |change| {
            let line = match change {
                TableChange::Added(entry) => format!("+{}:{:?}", entry.name, entry.origin),
                TableChange::Removed(entry) => format!("-{}:{:?}", entry.name, entry.origin),
            };
            into.lock().unwrap().push(line);
        })
        .unwrap();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].name, "t/ping");
    assert_eq!(snapshot[0].kind, ListenerKind::Async);
    let added = root
        .events()
        .on(
            &root,
            &EventKey::<Ping>::named("t/b"),
            returns::<Ping>(&log(), "b", None),
        )
        .unwrap();
    assert_eq!(entries(&changes), ["+t/b:None"]);
    let remote = root
        .events()
        .on_remote(
            &root,
            remote_entry(
                "t/c",
                remote_info("r", "py", vec![]),
                EventOptions::default(),
            ),
            Arc::new(Remote::new("c", &log())),
        )
        .unwrap();
    added.dispose().await.unwrap();
    remote.dispose().await.unwrap();
    assert_eq!(
        entries(&changes),
        ["+t/b:None", "+t/c:Some(7)", "-t/b:None", "-t/c:Some(7)"]
    );
    root.shutdown().await.unwrap();
}

/// The raw API: an erased dispatch with a batch filter and a terminal, an
/// emit that resolves once every call started, and calling one entry by id.
///
/// guarantee: events/raw
#[tokio::test]
async fn raw_dispatch_and_entry_calls() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.events().on(&root, &PING, wrap(&order, "a")).unwrap();
    root.events()
        .on_remote(
            &root,
            remote_entry(
                "t/ping",
                remote_info("r", "py", vec![]),
                EventOptions::default(),
            ),
            Arc::new(Remote::new("r", &order)),
        )
        .unwrap();
    let mut dispatch = Dispatch::new("t/ping", Mode::Waterfall, Payload::new(Arc::new(Ping(9))));
    // A batch filter knows which entry of which session each candidate is
    // (its origin and the id it was registered with), also when two have
    // equal descriptors.
    type Asked = Vec<(Option<u64>, Option<String>)>;
    let asked: Arc<Mutex<Asked>> = Arc::default();
    let seen = asked.clone();
    dispatch.filter = Some(Box::new(move |candidates: Vec<Candidate>| {
        *seen.lock().unwrap() = candidates
            .iter()
            .map(|c| (c.origin, c.id.clone()))
            .collect();
        FilterAnswer::Ready(Ok(candidates
            .iter()
            .map(|c| c.info.runtime.is_none())
            .collect()))
    }));
    dispatch.terminal = Some(RawTerminal::Async(Box::new(|event| {
        let n = event.downcast::<Ping>().unwrap().0;
        Box::pin(async move { Ok(Some(Value::new(format!("t{n}")))) })
    })));
    let out = root
        .events()
        .dispatch(&root, dispatch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        out.downcast_ref::<String>().map(String::as_str),
        Some("at9")
    );
    assert_eq!(
        *asked.lock().unwrap(),
        [(None, None), (Some(7), Some("r".to_owned()))]
    );

    order.lock().unwrap().clear();
    let emit = Dispatch::new("t/ping", Mode::Emit, Payload::new(Arc::new(Ping(1))));
    root.events().dispatch(&root, emit).await.unwrap();
    assert_eq!(entries(&order), ["a", "r:emit:1"], "every call started");

    let mut reserved = Dispatch::new("rutis/x", Mode::Serial, Payload::new(Arc::new(Ping(1))));
    assert!(root.events().dispatch(&root, reserved).await.is_err());
    reserved = Dispatch::new("rutis/x", Mode::Serial, Payload::new(Arc::new(Ping(1))));
    reserved.framework = true;
    assert!(root
        .events()
        .dispatch(&root, reserved)
        .await
        .unwrap()
        .is_none());

    let (snapshot, _watch) = root.events().watch_table(&root, |_| {}).unwrap();
    let local = snapshot.iter().find(|e| e.origin.is_none()).unwrap();
    order.lock().unwrap().clear();
    let out = root
        .events()
        .invoke_entry(
            &root,
            local.id,
            EntryCall {
                name: "t/ping".into(),
                mode: Mode::Waterfall,
                payload: Payload::new(Arc::new(Ping(0))),
                next: Some(RawTerminal::Async(Box::new(|_| {
                    Box::pin(async { Ok(Some(Value::new("far".to_string()))) })
                }))),
                dispatch: 1,
                origin: Some(3),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        out.downcast_ref::<String>().map(String::as_str),
        Some("afar")
    );
    let error = root
        .events()
        .invoke_entry(
            &root,
            999_999,
            EntryCall {
                name: "t/ping".into(),
                mode: Mode::Serial,
                payload: Payload::new(Arc::new(Ping(0))),
                next: None,
                dispatch: 1,
                origin: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "listener 999999 of t/ping is withdrawn");
    root.shutdown().await.unwrap();
}

// ── review of #464 ──────────────────────────────────────────────

/// A `once` listener is claimed when its call starts, not when a dispatch
/// selects it: one a filter dropped, or a `serial` that ended before it,
/// is still there for the next dispatch, and is called once.
#[tokio::test]
async fn a_once_listener_is_used_up_only_by_a_call() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.events()
        .on(
            &root,
            &PING,
            returns::<Ping>(&order, "first", Some("1".into())),
        )
        .unwrap();
    root.events()
        .on_opt(
            &root,
            &PING,
            returns::<Ping>(&order, "once", Some("2".into())),
            EventOptions::default().once(true),
        )
        .unwrap();
    // The filter drops it before any call.
    let filtered = root
        .events()
        .filter(|info| info.plugin.is_empty())
        .parallel(&root, &PING, Arc::new(Ping(0)));
    filtered.await.unwrap();
    assert!(entries(&order).is_empty(), "{:?}", entries(&order));
    // `serial` ends at the first result.
    let first = root.events().serial(&root, &PING, Arc::new(Ping(0))).await;
    assert_eq!(first.unwrap().as_deref(), Some("1"));
    assert_eq!(entries(&order), ["first"]);
    assert_eq!(
        root.events().subscriptions().len(),
        2,
        "the once listener stays"
    );
    // Reached now: called, then gone.
    root.events()
        .parallel(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap();
    assert_eq!(entries(&order), ["first", "first", "once"]);
    root.events()
        .parallel(&root, &PING, Arc::new(Ping(0)))
        .await
        .unwrap();
    assert_eq!(entries(&order), ["first", "first", "once", "first"]);
    assert_eq!(root.events().subscriptions().len(), 1);
    root.shutdown().await.unwrap();
}

/// Watchers hear the table's changes in the order they were made, also
/// when two threads change it at once: a change made while an earlier one
/// is still being heard waits for it (a watcher never hears a removal or
/// a later entry before the change before it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn table_changes_are_heard_in_the_order_they_were_made() {
    let root = Ctx::root().unwrap();
    let first_heard = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let heard: Log = log();
    {
        // Blocks in its first change until released.
        let (first_heard, release) = (first_heard.clone(), release.clone());
        let blocked = std::sync::atomic::AtomicBool::new(false);
        root.events()
            .watch_table(&root, move |_| {
                if !blocked.swap(true, Ordering::SeqCst) {
                    *first_heard.0.lock().unwrap() = true;
                    first_heard.1.notify_all();
                    let mut released = release.0.lock().unwrap();
                    while !*released {
                        released = release.1.wait(released).unwrap();
                    }
                }
            })
            .map(|(_, watching)| std::mem::forget(watching))
            .unwrap();
    }
    let into = heard.clone();
    root.events()
        .watch_table(&root, move |change| {
            let TableChange::Added(entry) = change else {
                return;
            };
            push(&into, entry.name.clone());
        })
        .map(|(_, watching)| std::mem::forget(watching))
        .unwrap();
    let ctx = root.clone();
    let a = std::thread::spawn(move || {
        ctx.events()
            .on(
                &ctx,
                &EventKey::<Ping>::named("t/a"),
                returns::<Ping>(&log(), "a", None),
            )
            .map(std::mem::forget)
            .unwrap();
    });
    {
        let mut heard = first_heard.0.lock().unwrap();
        while !*heard {
            heard = first_heard.1.wait(heard).unwrap();
        }
    }
    // `t/a` is in the table, its change half heard: `t/b` comes after it.
    let ctx = root.clone();
    let b = std::thread::spawn(move || {
        ctx.events()
            .on(
                &ctx,
                &EventKey::<Ping>::named("t/b"),
                returns::<Ping>(&log(), "b", None),
            )
            .map(std::mem::forget)
            .unwrap();
    });
    for _ in 0..500 {
        if root.events().subscriptions().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(root.events().subscriptions().len(), 2);
    // Give a delivery out of order its chance.
    tokio::time::sleep(Duration::from_millis(50)).await;
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    a.join().unwrap();
    b.join().unwrap();
    assert_eq!(entries(&heard), ["t/a", "t/b"]);
    drop(root);
}

/// An event type with no name of its own (an empty `NAME`, as
/// `rutis_bridge::events::Args`) cannot be used with `EventKey::of()`:
/// registering and dispatching fail clearly (events protocol §12.1).
#[tokio::test]
async fn an_event_without_a_name_needs_one() {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct Unnamed;
    impl Event for Unnamed {
        const NAME: &'static str = "";
        type Value = ();
    }
    let root = Ctx::root().unwrap();
    let key = EventKey::<Unnamed>::of();
    let message = "Unnamed events need a name: EventKey::dynamic(name)";
    let refused = root
        .events()
        .on(
            &root,
            &key,
            on::<Unnamed, _>(|_, _, _| Box::pin(async { Ok(None) })),
        )
        .err()
        .unwrap();
    assert_eq!(refused.to_string(), message);
    assert_eq!(
        root.events()
            .emit(&root, &key, Arc::new(Unnamed))
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        root.events()
            .serial(&root, &key, Arc::new(Unnamed))
            .await
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        root.events()
            .bail_sync(&root, &key, &Unnamed)
            .unwrap_err()
            .to_string(),
        message
    );
    // Given a name, it is an event like any.
    let named = EventKey::<Unnamed>::dynamic("t/named");
    root.events()
        .on_sync(
            &root,
            &named,
            |_: &Ctx, _: &Unnamed, _: SyncCall<'_, ()>| Ok(Some(())),
        )
        .unwrap();
    assert_eq!(
        root.events().bail_sync(&root, &named, &Unnamed).unwrap(),
        Some(())
    );
    root.shutdown().await.unwrap();
}

/// An entry called by id (a node running the other node's `events.call`)
/// is called only under its own name: an id the far end kept from before
/// that now names an entry of another name is withdrawn.
#[tokio::test]
async fn an_entry_called_by_id_must_have_the_name() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.events()
        .on(
            &root,
            &PING,
            returns::<Ping>(&order, "ping", Some("p".into())),
        )
        .unwrap();
    root.events()
        .on_pattern(
            &root,
            &EventPattern::<Ping>::prefix("t/room/"),
            on_prefix::<Ping, _>(|_, _, _, _| Box::pin(async { Ok(Some("room".to_string())) })),
        )
        .unwrap();
    let (snapshot, _watch) = root.events().watch_table(&root, |_| {}).unwrap();
    let exact = snapshot.iter().find(|e| !e.prefix).unwrap().id;
    let prefix = snapshot.iter().find(|e| e.prefix).unwrap().id;
    let call = |name: &str| EntryCall {
        name: name.into(),
        mode: Mode::Serial,
        payload: Payload::new(Arc::new(Ping(0))),
        next: None,
        dispatch: 1,
        origin: None,
    };
    let wrong = root
        .events()
        .invoke_entry(&root, exact, call("t/other"))
        .await
        .unwrap_err();
    assert_eq!(
        wrong.to_string(),
        format!("listener {exact} of t/other is withdrawn")
    );
    assert!(root
        .events()
        .invoke_entry(&root, prefix, call("t/ping"))
        .await
        .is_err());
    assert!(entries(&order).is_empty(), "no listener ran");
    let room = root
        .events()
        .invoke_entry(&root, prefix, call("t/room/1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        room.downcast_ref::<String>().map(String::as_str),
        Some("room")
    );
    let ping = root
        .events()
        .invoke_entry(&root, exact, call("t/ping"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ping.downcast_ref::<String>().map(String::as_str), Some("p"));
    root.shutdown().await.unwrap();
}
