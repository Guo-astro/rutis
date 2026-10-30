//! Contract tests of the unified event model in the kernel (#464): one
//! listener table, five modes, scope, filter, liveness, remote entries
//! (design `docs/design-events-2026-10-18.md`, protocol spec
//! `docs/design-events-protocol-2026-10-18.md`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{Call, Listener, ListenerInfo, Mode, Scope};
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, ErrorSink, FiberView, ListenerKind,
    NextMisuse,
};
use rutis::internal::events::{
    Candidate, Dispatch, EntryCall, FilterAnswer, JsonTranscoder, Payload, RawTerminal, RemoteCall,
    RemoteEntry, RemoteListener, RemoteNext, RemoteSyncCall, TableChange, Value,
};
use rutis::internal::{CtxInternal, EventKey, EventOptions};
use rutis::{Ctx, Error, Event};
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

/// A listener that logs `label` and returns `value`.
fn returns<E: Event>(log: &Log, label: &'static str, value: Option<E::Value>) -> impl Listener<E>
where
    E::Value: Clone + Sync,
{
    let log = log.clone();
    move |_: Arc<E>, _: Call<E>| {
        push(&log, label);
        std::future::ready(Ok(value.clone()))
    }
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

fn sink() -> (ErrorSink, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let errors: Arc<Mutex<Vec<Arc<ErrorKind>>>> = Arc::default();
    let out = errors.clone();
    let sink: ErrorSink = Arc::new(move |error| out.lock().unwrap().push(error));
    (sink, errors)
}

fn root_with_sink() -> (Ctx, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let (sink, errors) = sink();
    (
        Ctx::root_with_sink(tokio::runtime::Handle::current(), sink),
        errors,
    )
}

// ── one table, every mode ───────────────────────────────────────

/// A listener registered once is called by every mode, and sees which.
///
/// guarantee: events/one-table, core/D16
#[tokio::test]
async fn one_listener_is_called_by_every_mode() {
    let root = Ctx::root().unwrap();
    let modes = log();
    let seen = modes.clone();
    root.on_sync::<Ping>(move |_, call| {
        push(&seen, call.mode().as_str());
        Ok(None)
    })
    .unwrap();
    root.emit(Ping(1)).unwrap();
    rutis_test_support::until(|| entries(&modes).len() == 1, "emit called it").await;
    root.parallel(Ping(1)).await.unwrap();
    root.serial(Ping(1)).await.unwrap();
    root.waterfall(Ping(1)).await.unwrap();
    root.bail(Ping(1)).unwrap();
    assert_eq!(
        entries(&modes),
        ["emit", "parallel", "serial", "waterfall", "bail"]
    );
    root.shutdown().await.unwrap();
}

/// `next` is only in a waterfall; a synchronous listener cannot call it in
/// an async waterfall.
///
/// guarantee: events/one-table
#[tokio::test]
async fn next_outside_a_waterfall_is_an_error() {
    let root = Ctx::root().unwrap();
    root.on::<Ping>(|_, call| call.next()).unwrap();
    let error = root.serial(Ping(0)).await.unwrap_err();
    assert!(
        matches!(
            &*error,
            ErrorKind::WaterfallNext {
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

    root.listen::<Ping>()
        .name("t/sync-next")
        .on_sync(|_, call| call.next())
        .unwrap();
    let error = root
        .named("t/sync-next")
        .waterfall(Ping(0))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "a synchronous listener cannot call next in an asynchronous waterfall; register it with on"
    );
    root.shutdown().await.unwrap();
}

/// A long waterfall (more listeners than one word of ended slots holds):
/// every listener runs, in order, each through the one before it.
#[tokio::test]
async fn a_long_waterfall_passes_through_every_listener() {
    let root = Ctx::root().unwrap();
    for i in 0..70 {
        root.on::<Ping>(move |_, call| async move {
            let rest = call.next().await?.unwrap_or_default();
            Ok(Some(format!("{i},{rest}")))
        })
        .unwrap();
    }
    let out = root
        .waterfall(Ping(0))
        .terminal(|_| async { Ok(Some("end".to_owned())) })
        .await
        .unwrap()
        .unwrap();
    let expected: Vec<String> = (0..70)
        .map(|i| i.to_string())
        .chain(["end".to_owned()])
        .collect();
    assert_eq!(out, expected.join(","));
    root.shutdown().await.unwrap();
}

/// A listener's `next`, called once its call is over (from a task the
/// listener left behind), is `Ended`: the waterfall already has its result.
///
/// guarantee: events/one-table
#[tokio::test]
async fn next_after_the_call_ended_is_an_error() {
    let root = Ctx::root().unwrap();
    let kept: Arc<Mutex<Option<Call<Ping>>>> = Arc::default();
    let keep = kept.clone();
    root.on::<Ping>(move |_, call| {
        let keep = keep.clone();
        async move {
            *keep.lock().unwrap() = Some(call);
            Ok(Some("first".to_owned()))
        }
    })
    .unwrap();
    let rest = log();
    let heard = rest.clone();
    root.on::<Ping>(move |_, _| {
        let heard = heard.clone();
        async move {
            push(&heard, "second");
            Ok(None)
        }
    })
    .unwrap();
    assert_eq!(
        root.waterfall(Ping(0)).await.unwrap(),
        Some("first".to_owned())
    );
    let call = kept.lock().unwrap().take().expect("the call was kept");
    let error = call.next().await.unwrap_err();
    assert!(
        matches!(
            &*error,
            ErrorKind::WaterfallNext {
                misuse: NextMisuse::Ended,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(
        entries(&rest).is_empty(),
        "the rest of the chain did not run"
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
    root.on::<Ping>(move |_, _| {
        push(&first, "a:start");
        let mut gate = gate.clone();
        let first = first.clone();
        async move {
            let _ = gate.wait_for(|open| *open).await;
            push(&first, "a:end");
            Ok(None)
        }
    })
    .unwrap();
    let failing = order.clone();
    root.on::<Ping>(move |_, _| {
        push(&failing, "b");
        async { Err(ErrorKind::PluginFailed("b failed".into()).into()) }
    })
    .unwrap();
    root.on::<Ping>(returns::<Ping>(&order, "c", None)).unwrap();
    root.emit(Ping(1)).unwrap();
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
        ErrorKind::ListenerFailed {
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
/// guarantee: events/emit, core/D31
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emits_of_one_name_start_in_emit_order() {
    let root = Ctx::root().unwrap();
    let seen = log();
    let out = seen.clone();
    root.on::<Ping>(move |event, _| {
        push(&out, event.0.to_string());
        async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(None)
        }
    })
    .unwrap();
    for i in 0..200 {
        root.emit(Ping(i)).unwrap();
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
        root.on::<Ping>(move |_, _| {
            push(&started, label);
            let finished = finished.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                finished.fetch_add(1, Ordering::SeqCst);
                if fails {
                    Err(ErrorKind::PluginFailed(label.into()).into())
                } else {
                    Ok(Some("ignored".into()))
                }
            }
        })
        .unwrap();
    }
    let error = root.parallel(Ping(0)).await.unwrap_err();
    assert_eq!(entries(&started), ["a", "b", "c"]);
    assert_eq!(finished.load(Ordering::SeqCst), 3, "waited for all");
    match &*error {
        ErrorKind::ListenersFailed { name, errors } => {
            assert_eq!(name, "t/ping");
            let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            assert_eq!(messages, ["a", "b"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(error.wire_name(), Some("AggregateError"));
    assert_eq!(error.to_string(), "2 listeners of t/ping failed");

    root.listen::<Ping>()
        .name("t/one")
        .on(|_, _| async { Err(ErrorKind::Closed.into()) })
        .unwrap();
    let error = root.named("t/one").parallel(Ping(0)).await.unwrap_err();
    assert!(
        matches!(&*error, ErrorKind::ListenersFailed { errors, .. } if errors.len() == 1),
        "{error:?}"
    );
    root.shutdown().await.unwrap();
}

// ── serial / bail ───────────────────────────────────────────────

/// `None` and `false` are no result; anything else is (`0`, `""`).
///
/// guarantee: events/serial-bail
#[tokio::test]
async fn serial_and_bail_skip_no_result() {
    let root = Ctx::root().unwrap();
    let order = log();
    use serde_json::json;
    for (label, value) in [
        ("a", None),
        ("b", Some(json!(false))),
        ("c", Some(json!(null))),
        ("d", Some(json!(0))),
        ("e", Some(json!("E"))),
    ] {
        let order = order.clone();
        root.on_sync::<Flag>(move |_, _| {
            push(&order, label);
            Ok(value.clone())
        })
        .unwrap();
    }
    let value = root.serial(Flag).await.unwrap();
    assert_eq!(value, Some(json!(0)));
    assert_eq!(entries(&order), ["a", "b", "c", "d"]);
    let value = root.bail(Flag).unwrap();
    assert_eq!(value, Some(json!(0)));

    // A Rust `bool` value: `Some(false)` is no result too.
    #[derive(Serialize, Deserialize)]
    struct Ask;
    impl Event for Ask {
        const NAME: &'static str = "t/ask";
        type Value = bool;
    }
    root.on_sync::<Ask>(|_, _| Ok(Some(false))).unwrap();
    assert_eq!(root.bail(Ask).unwrap(), None);
    root.on_sync::<Ask>(|_, _| Ok(Some(true))).unwrap();
    assert_eq!(root.serial(Ask).await.unwrap(), Some(true));
    root.shutdown().await.unwrap();
}

/// A listener's error stops `serial` and is returned as it is.
///
/// guarantee: events/serial-bail
#[tokio::test]
async fn serial_stops_at_an_error() {
    let root = Ctx::root().unwrap();
    let order = log();
    let failing = order.clone();
    root.on::<Ping>(move |_, _| {
        push(&failing, "a");
        async { Err(ErrorKind::Closed.into()) }
    })
    .unwrap();
    root.on::<Ping>(returns::<Ping>(&order, "b", Some("B".into())))
        .unwrap();
    let error = root.serial(Ping(0)).await.unwrap_err();
    assert!(matches!(&*error, ErrorKind::Closed), "{error:?}");
    assert_eq!(entries(&order), ["a"]);
    root.shutdown().await.unwrap();
}

/// A synchronous dispatch meeting an async listener fails with
/// `AsyncListener`; listeners before it ran.
///
/// guarantee: events/serial-bail
#[tokio::test]
async fn bail_meets_an_async_listener() {
    let root = Ctx::root().unwrap();
    let order = log();
    let first = order.clone();
    root.on_sync::<Ping>(move |_, _| {
        push(&first, "a");
        Ok(None)
    })
    .unwrap();
    root.on::<Ping>(returns::<Ping>(&order, "b", Some("B".into())))
        .unwrap();
    let error = root.bail(Ping(0)).unwrap_err();
    assert_eq!(entries(&order), ["a"]);
    assert_eq!(error.wire_name(), Some("AsyncListener"));
    assert_eq!(
        error.to_string(),
        "listener of t/ping in plugin root is asynchronous and cannot run in a synchronous bail"
    );
    root.shutdown().await.unwrap();
}

// ── waterfall ───────────────────────────────────────────────────

fn wrap(log: &Log, label: &'static str) -> impl Listener<Ping> {
    let log = log.clone();
    move |_: Arc<Ping>, call: Call<Ping>| {
        push(&log, label);
        async move {
            let inner = call.next().await?.unwrap_or_default();
            Ok(Some(format!("{label}{inner}")))
        }
    }
}

/// Outer to inner with `next`; not calling it vetoes the rest; the last
/// `next` is the terminal; the outermost value comes back as it is.
///
/// guarantee: events/waterfall, core/D17
#[tokio::test]
async fn waterfall_chains_vetoes_and_ends_in_the_terminal() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.on::<Ping>(wrap(&order, "a")).unwrap();
    root.on::<Ping>(wrap(&order, "b")).unwrap();
    let terminal_log = order.clone();
    let out = root
        .waterfall(Ping(7))
        .terminal(move |event| {
            push(&terminal_log, "terminal");
            let n = event.0;
            async move { Ok(Some(format!("t{n}"))) }
        })
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("abt7"));
    assert_eq!(entries(&order), ["a", "b", "terminal"]);

    // Without a terminal, the last next gives None.
    let out = root.waterfall(Ping(7)).await.unwrap();
    assert_eq!(out.as_deref(), Some("ab"));

    // A veto.
    root.listen::<Ping>()
        .name("t/veto")
        .on(wrap(&order, "x"))
        .unwrap();
    root.listen::<Ping>()
        .name("t/veto")
        .on(returns::<Ping>(&order, "v", Some("v".into())))
        .unwrap();
    root.listen::<Ping>()
        .name("t/veto")
        .on(wrap(&order, "never"))
        .unwrap();
    let out = root.named("t/veto").waterfall(Ping(0)).await.unwrap();
    assert_eq!(out.as_deref(), Some("xv"));
    assert!(!entries(&order).contains(&"never".to_owned()));

    // No listeners: the terminal's value.
    let out = root
        .named("t/empty")
        .waterfall(Ping(3))
        .terminal(|e| {
            let n = e.0;
            async move { Ok(Some(format!("t{n}"))) }
        })
        .await
        .unwrap();
    assert_eq!(out.as_deref(), Some("t3"));

    // The outermost value as it is: `false` stays `false`.
    root.on_sync::<Flag>(|_, _| Ok(Some(serde_json::json!(false))))
        .unwrap();
    let out = root.waterfall(Flag).await.unwrap();
    assert_eq!(out, Some(serde_json::json!(false)));
    root.shutdown().await.unwrap();
}

// ── names, types, prefixes, order ───────────────────────────────

/// Listeners are found by name: a listener of another Rust type with the
/// same name hears the event, converted by serde, and its result comes back
/// as the emitter's type.
///
/// guarantee: events/by-name, core/D2, core/D33
#[tokio::test]
async fn listeners_of_another_type_hear_the_same_name() {
    let root = Ctx::root().unwrap();
    root.on::<PingAlias>(|event, _| {
        let n = event.0;
        async move { Ok(Some(format!("alias {n}"))) }
    })
    .unwrap();
    let out = root.serial(Ping(5)).await.unwrap();
    assert_eq!(out.as_deref(), Some("alias 5"));
    // A name of its own does not cross.
    assert_eq!(root.named("t/other").serial(Ping(5)).await.unwrap(), None);
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
    let name = "room/1";
    root.listen::<Ping>()
        .name(name)
        .on(returns::<Ping>(&order, "exact-1", None))
        .unwrap();
    let names = order.clone();
    root.listen::<Ping>()
        .prefix("room/")
        .on(move |_, call| {
            push(&names, format!("prefix:{}", call.name()));
            async { Ok(None) }
        })
        .unwrap();
    root.listen::<Ping>()
        .name(name)
        .on(returns::<Ping>(&order, "exact-2", None))
        .unwrap();
    root.listen::<Ping>()
        .name(name)
        .prepend()
        .on(returns::<Ping>(&order, "first", None))
        .unwrap();
    root.listen::<Ping>()
        .name(name)
        .once()
        .on(returns::<Ping>(&order, "once", None))
        .unwrap();
    root.named(name).serial(Ping(0)).await.unwrap();
    root.named(name).serial(Ping(0)).await.unwrap();
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
    let name = "rutis/row";
    root.listen::<Ping>()
        .name(name)
        .on_sync({
            let order = order.clone();
            move |_, _| {
                push(&order, "heard");
                Ok(None)
            }
        })
        .unwrap();
    let reserved = |error: Error| {
        assert_eq!(error.wire_name(), Some("ReservedEvent"));
        assert_eq!(
            error.to_string(),
            "rutis/* events are emitted by rutis only: rutis/row"
        );
    };
    reserved(root.named(name).emit(Ping(0)).unwrap_err());
    reserved(root.named(name).parallel(Ping(0)).await.unwrap_err());
    reserved(root.named(name).serial(Ping(0)).await.unwrap_err());
    reserved(root.named(name).waterfall(Ping(0)).await.unwrap_err());
    reserved(root.named(name).bail(Ping(0)).unwrap_err());
    assert!(entries(&order).is_empty());
    root.events()
        .emit_framework(&root, &EventKey::<Ping>::named(name), Arc::new(Ping(0)))
        .unwrap();
    rutis_test_support::until(|| entries(&order) == ["heard"], "framework emit heard").await;
    root.shutdown().await.unwrap();
}

// ── scope ───────────────────────────────────────────────────────

/// Events are delivered by name on one flat bus: every live listener hears
/// every dispatch, wherever its scope is (Cordis's rule). A listener's scope
/// is a descriptor for filters, not a routing rule.
///
/// guarantee: events/flat, core/D29
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
        ctx.on::<Ping>(returns::<Ping>(&heard, label, None))
            .unwrap();
    }
    let round = |ctx: Ctx| {
        let heard = heard.clone();
        async move {
            heard.lock().unwrap().clear();
            ctx.parallel(Ping(0)).await.unwrap();
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
    assert_eq!(
        t1.mark_scope("session").unwrap_err().to_string(),
        r#"config validation failed: ["this fiber is already a scope of group tenant, not session"]"#
    );
    assert_eq!(
        root.mark_scope("tenant").unwrap_err().to_string(),
        r#"config validation failed: ["the root is not a scope of a group"]"#,
        "the root is no scope"
    );
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
    a.on::<Ping>(returns::<Ping>(&order, "a", None)).unwrap();
    b.on::<Ping>(returns::<Ping>(&order, "b", None)).unwrap();
    b.listen::<Ping>()
        .global()
        .on(returns::<Ping>(&order, "g", None))
        .unwrap();
    t.on::<Ping>(returns::<Ping>(&order, "t", None)).unwrap();
    let seen: Arc<Mutex<Vec<ListenerInfo>>> = Arc::default();
    let into = seen.clone();
    let filtered_log = order.clone();
    root.filter(move |info| {
        push(&filtered_log, format!("filter:{}", info.plugin));
        into.lock().unwrap().push(info.clone());
        info.row.as_deref() == Some("row-a")
    })
    .serial(Ping(0))
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
        .filter(|_| panic!("filter broke"))
        .parallel(Ping(0))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("filter broke"), "{error}");
    assert!(entries(&order).is_empty(), "{:?}", entries(&order));
    root.shutdown().await.unwrap();
}

/// In `emit`, a failing filter delivers to nobody and is reported.
///
/// guarantee: events/filter
#[tokio::test]
async fn a_failing_emit_filter_is_reported() {
    let (root, errors) = root_with_sink();
    let order = log();
    root.on::<Ping>(returns::<Ping>(&order, "a", None)).unwrap();
    root.filter(|_| panic!("no")).emit(Ping(0)).unwrap();
    rutis_test_support::until(|| !errors.lock().unwrap().is_empty(), "reported").await;
    assert!(
        matches!(&*errors.lock().unwrap()[0], ErrorKind::FilterFailed { name, .. } if name == "t/ping")
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(entries(&order).is_empty());
}

// ── lifecycle ───────────────────────────────────────────────────

/// A listener is called with the context that registered it, and its
/// fiber's unload waits for its running calls while signalling them to
/// stop (its `ctx.cancelled()`).
///
/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unload_cancels_and_waits_for_running_listeners() {
    let root = Ctx::root().unwrap();
    let order = log();
    let out = order.clone();
    let view = root.plugin(Apply("waiter", move |ctx: &Ctx| {
        let out = out.clone();
        let own = ctx.instance();
        ctx.on::<Ping>(move |_, call| {
            assert_eq!(call.ctx().instance(), own, "called with its own context");
            push(&out, "started");
            let out = out.clone();
            async move {
                call.ctx().cancelled().await;
                tokio::time::sleep(Duration::from_millis(30)).await;
                push(&out, "cancelled");
                Ok(None)
            }
        })?;
        Ok(())
    }));
    (&view).await.unwrap();
    for mode in ["emit", "parallel"] {
        order.lock().unwrap().clear();
        if mode == "emit" {
            root.emit(Ping(0)).unwrap();
        } else {
            let root = root.clone();
            tokio::spawn(async move { root.parallel(Ping(0)).await });
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
/// guarantee: events/lifecycle, core/D28
#[tokio::test]
async fn unloaded_listeners_are_not_called() {
    let root = Ctx::root().unwrap();
    let order = log();
    let (view, ctx) = child(&root, "gone").await;
    ctx.on_sync::<Ping>({
        let order = order.clone();
        move |_, _| {
            push(&order, "gone");
            Ok(Some("G".into()))
        }
    })
    .unwrap();
    root.on::<Ping>(returns::<Ping>(&order, "root", None))
        .unwrap();
    view.dispose().await.unwrap();
    assert_eq!(root.serial(Ping(0)).await.unwrap(), None);
    assert_eq!(
        root.bail(Ping(0)).unwrap_err().wire_name(),
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
    fn call<'a>(&'a self, call: RemoteCall<'a>) -> BoxFuture<'a, Result<Option<Value>, Error>> {
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
                        Some(value) => value.into_typed(&JsonTranscoder)?,
                        None => String::new(),
                    };
                    Ok(Some(Value::new(format!("{}{inner}", self.label))))
                }
                _ => Ok(Some(Value::new(serde_json::json!(self.label)))),
            }
        })
    }
    fn call_sync(&self, call: RemoteSyncCall<'_>) -> Result<Option<Value>, Error> {
        push(&self.log, format!("{}:{}", self.label, call.mode()));
        Ok(Some(Value::new(self.label.to_string())))
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
/// guarantee: events/remote
#[tokio::test]
async fn remote_entries_take_part_like_any_listener() {
    let root = Ctx::root().unwrap();
    let (_t, t) = scope(&root, "tenant").await;
    let order = log();
    root.on::<Ping>(returns::<Ping>(&order, "local", None))
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
    let out = root.serial(Ping(4)).await.unwrap();
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
    t.filter(move |info| {
        push(&into, format!("{}@{:?}", info.plugin, info.runtime));
        info.runtime.as_deref() == Some("go")
    })
    .parallel(Ping(1))
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
    root.serial(Ping(2)).await.unwrap();
    assert_eq!(entries(&order), ["local", "go:serial:2"]);
    root.shutdown().await.unwrap();
}

/// A remote entry in a waterfall gets a `'static` next: it runs the rest of
/// the chain once; a second call fails, as does a call after the entry's
/// call ended.
///
/// guarantee: events/remote
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
    root.on::<Ping>(wrap(&order, "l")).unwrap();
    let out = root.waterfall(Ping(0)).await.unwrap();
    assert_eq!(out.as_deref(), Some("rl"));

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
    let error = root.named("t/twice").waterfall(Ping(0)).await.unwrap_err();
    assert_eq!(error.to_string(), "next of t/twice was already called");

    // Kept past its call: "ended".
    struct Keeper(Mutex<Option<RemoteNext>>);
    impl RemoteListener for Keeper {
        fn kind(&self) -> ListenerKind {
            ListenerKind::Async
        }
        fn call<'a>(&'a self, call: RemoteCall<'a>) -> BoxFuture<'a, Result<Option<Value>, Error>> {
            *self.0.lock().unwrap() = call.next();
            Box::pin(async { Ok(None) })
        }
        fn call_sync(&self, _: RemoteSyncCall<'_>) -> Result<Option<Value>, Error> {
            unreachable!()
        }
    }
    let keeper = Arc::new(Keeper(Mutex::new(None)));
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
    root.named("t/kept").waterfall(Ping(0)).await.unwrap();
    let next = keeper.0.lock().unwrap().take().unwrap();
    let error = next.call().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "next of t/kept was called after its waterfall ended"
    );
    // An async remote entry in bail: AsyncListener, not called.
    let error = root.named("t/kept").bail(Ping(0)).unwrap_err();
    assert_eq!(error.wire_name(), Some("AsyncListener"));
    root.shutdown().await.unwrap();
}

/// `bail` calls a remote entry's `call_sync` inline.
///
/// guarantee: events/remote
#[tokio::test]
async fn remote_entries_in_synchronous_modes() {
    let root = Ctx::root().unwrap();
    let order = log();
    let remote = Remote::new("r", &order);
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
    root.on_sync::<Ping>(|_, _| Ok(Some("s".into()))).unwrap();
    let out = root.bail(Ping(0)).unwrap();
    assert_eq!(out.as_deref(), Some("r"));
    assert_eq!(entries(&order), ["r:bail"]);
    root.shutdown().await.unwrap();
}

/// The table watcher gets the current entries, then every change; a Rust
/// registration is reported before `on` returns.
///
/// guarantee: events/one-table
#[tokio::test]
async fn the_table_is_watched() {
    let root = Ctx::root().unwrap();
    root.on::<Ping>(returns::<Ping>(&log(), "a", None)).unwrap();
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
        .listen::<Ping>()
        .name("t/b")
        .on(returns::<Ping>(&log(), "b", None))
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
/// guarantee: events/one-table
#[tokio::test]
async fn raw_dispatch_and_entry_calls() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.on::<Ping>(wrap(&order, "a")).unwrap();
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
    root.on::<Ping>(returns::<Ping>(&order, "first", Some("1".into())))
        .unwrap();
    root.listen::<Ping>()
        .once()
        .on(returns::<Ping>(&order, "once", Some("2".into())))
        .unwrap();
    // The filter drops it before any call.
    let filtered = root.filter(|info| info.plugin.is_empty()).parallel(Ping(0));
    filtered.await.unwrap();
    assert!(entries(&order).is_empty(), "{:?}", entries(&order));
    // `serial` ends at the first result.
    let first = root.serial(Ping(0)).await;
    assert_eq!(first.unwrap().as_deref(), Some("1"));
    assert_eq!(entries(&order), ["first"]);
    assert_eq!(root.subscriptions().len(), 2, "the once listener stays");
    // Reached now: called, then gone.
    root.parallel(Ping(0)).await.unwrap();
    assert_eq!(entries(&order), ["first", "first", "once"]);
    root.parallel(Ping(0)).await.unwrap();
    assert_eq!(entries(&order), ["first", "first", "once", "first"]);
    assert_eq!(root.subscriptions().len(), 1);
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
        ctx.listen::<Ping>()
            .name("t/a")
            .on(returns::<Ping>(&log(), "a", None))
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
        ctx.listen::<Ping>()
            .name("t/b")
            .on(returns::<Ping>(&log(), "b", None))
            .map(std::mem::forget)
            .unwrap();
    });
    for _ in 0..500 {
        if root.subscriptions().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(root.subscriptions().len(), 2);
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
/// `rutis_bridge::events::Args`) cannot be listened to or dispatched under
/// its type alone: registering and dispatching fail clearly (events
/// protocol §12.1).
#[tokio::test]
async fn an_event_without_a_name_needs_one() {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct Unnamed;
    impl Event for Unnamed {
        const NAME: &'static str = "";
        type Value = ();
    }
    let root = Ctx::root().unwrap();
    let message = "Unnamed events need a name: EventKey::dynamic(name)";
    let refused = root.on::<Unnamed>(|_, _| async { Ok(None) }).err().unwrap();
    assert_eq!(refused.to_string(), message);
    assert_eq!(root.emit(Unnamed).unwrap_err().to_string(), message);
    assert_eq!(root.serial(Unnamed).await.unwrap_err().to_string(), message);
    assert_eq!(root.bail(Unnamed).unwrap_err().to_string(), message);
    // Given a name, it is an event like any.
    root.listen::<Unnamed>()
        .name("t/named")
        .on_sync(|_, _| Ok(Some(())))
        .unwrap();
    assert_eq!(root.named("t/named").bail(Unnamed).unwrap(), Some(()));
    root.shutdown().await.unwrap();
}

/// An entry called by id (a node running the other node's `events.call`)
/// is called only under its own name: an id the far end kept from before
/// that now names an entry of another name is withdrawn.
#[tokio::test]
async fn an_entry_called_by_id_must_have_the_name() {
    let root = Ctx::root().unwrap();
    let order = log();
    root.on::<Ping>(returns::<Ping>(&order, "ping", Some("p".into())))
        .unwrap();
    root.listen::<Ping>()
        .prefix("t/room/")
        .on(|_, _| async { Ok(Some("room".to_string())) })
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
