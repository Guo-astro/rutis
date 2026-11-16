//! guarantee: bridge/cancel-reaches-callee
//! Dropping an async call cancels it: the AbortSignal the Cordis method
//! received aborts, the late reply is discarded, and the session goes on.

use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::{Ctx, Event};
use rutis_bridge::runtime::{Mount, Process};
use rutis_bridge::session::{settle, Value};
use rutis_test_support::until;
use serde_json::json;

const PLUGIN: &str = r#"
export function apply(ctx) {
  let aborted = false
  let started = false
  ctx.provide('work', {
    async wait(signal) {
      started = true
      await new Promise((resolve, reject) => {
        signal.addEventListener('abort', () => { aborted = true; reject(signal.reason) })
      })
    },
    aborted() { return aborted },
    started() { return started },
    ping() { return 'pong' },
  })
}
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_call_aborts_its_signal_and_the_session_continues() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let mut plugin = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    plugin.write_all(PLUGIN.as_bytes()).unwrap();
    let process = Process::mount(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime"),
        Mount {
            plugins: vec![(plugin.path(), json!({}))],
            services: json!({ "work": ["wait", "aborted", "started", "ping"] }),
            ..Mount::default()
        },
    )
    .await
    .unwrap();
    let work = process.service("work").unwrap();

    {
        // The invoke answers at once with the method's Promise; awaiting it
        // is the call that is dropped (between the two is the next test).
        let promise = process
            .connection()
            .invoke_async(&work, "wait", Value::List(vec![Value::Signal]))
            .await
            .unwrap();
        let awaited = settle(promise);
        let mut awaited = std::pin::pin!(awaited);
        // Dropped once the method runs (and waits for its signal), while its
        // Promise is awaited.
        tokio::select! {
            // The call first: polled, it sends what is then dropped.
            biased;
            ended = &mut awaited => panic!("the call only ends when cancelled: {ended:?}"),
            () = until(
                || process.call(&work, "started", json!([])).unwrap() == json!(true),
                "the method started",
            ) => {}
        }
    }

    // The abort reaches the method, whose rejection arrives after the caller
    // gave up: it is discarded instead of breaking the session.
    until(
        || process.call(&work, "aborted", json!([])).unwrap() == json!(true),
        "the Cordis method's AbortSignal aborted",
    )
    .await;
    until(
        || process.connection().orphans() > 0,
        "its late reply discarded",
    )
    .await;
    assert_eq!(
        process.call(&work, "ping", json!([])).unwrap(),
        json!("pong")
    );
    process.dispose().await.unwrap();
}

// PR #73 review of 6794a3a: the reply to the invoke has been delivered (a
// Promise reference) but the caller has not polled again to await it; the
// call is dropped in that window and must still be cancelled.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_between_the_reply_and_its_await_still_cancels() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let mut plugin = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    plugin.write_all(PLUGIN.as_bytes()).unwrap();
    let process = Process::mount(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime"),
        Mount {
            plugins: vec![(plugin.path(), json!({}))],
            services: json!({ "work": ["wait", "aborted", "ping"] }),
            ..Mount::default()
        },
    )
    .await
    .unwrap();
    let work = process.service("work").unwrap();
    {
        let call = process.invoke_async(&work, "wait", vec![Value::Signal]);
        let mut call = std::pin::pin!(call);
        // One poll sends the invoke; the reply then arrives unpolled.
        std::future::poll_fn(|cx| {
            let _ = call.as_mut().poll(cx);
            std::task::Poll::Ready(())
        })
        .await;
        // clock: widen — gives the reply time to arrive before the drop, the
        // window the test is about; the assertion holds whether it did.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    until(
        || process.call(&work, "aborted", json!([])).unwrap() == json!(true),
        "the cancel of the dropped call",
    )
    .await;
    process.dispose().await.unwrap();
}

/// A JavaScript listener has no signal for one call (events protocol §5.7):
/// the caller giving up cancels the call, the listener runs on, and its
/// reply is discarded. Only the row's unload aborts `ctx.signal`.
const LISTENER: &str = r#"
export function apply(ctx) {
  let started = false
  let finished = false
  ctx.on('t/slow', async () => {
    started = true
    await new Promise(resolve => setTimeout(resolve, 300))
    finished = true
  })
  ctx.provide('probe', {
    started() { return started },
    finished() { return finished },
    aborted() { return ctx.signal.aborted },
    ping() { return 'pong' },
  })
}
"#;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Slow;

impl Event for Slow {
    const NAME: &'static str = "t/slow";
    type Value = ();
}

/// rule: EVP-5.7-7
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_dispatch_does_not_stop_a_javascript_listener() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let mut plugin = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    plugin.write_all(LISTENER.as_bytes()).unwrap();
    let root = Ctx::root().unwrap();
    let process = Process::mount(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime"),
        Mount {
            plugins: vec![(plugin.path(), json!({}))],
            services: json!({ "probe": ["started", "finished", "aborted", "ping"] }),
            ctx: Some(root.clone()),
            ..Mount::default()
        },
    )
    .await
    .unwrap();
    let probe = process.service("probe").unwrap();
    until(
        || {
            root.events()
                .subscriptions()
                .iter()
                .any(|entry| entry.name == Slow::NAME)
        },
        "the listener on the bus",
    )
    .await;

    // `serial`: the listener's call runs on the dispatch itself, so the
    // dispatch's own poll takes the answer to it. `parallel` starts each
    // listener in a task of its own, and the drop could land between that
    // answer and the await the task then starts: nothing would be outstanding,
    // and no reply would be discarded.
    let mut dispatch = root.named(Slow::NAME).serial(Slow);
    tokio::select! {
        biased;
        ended = &mut dispatch => panic!("the dispatch only ends when cancelled: {ended:?}"),
        () = until(
            || process.call(&probe, "started", json!([])).ok() == Some(json!(true)),
            "the listener started",
        ) => {}
    }
    // A session's frames are handled in the order they were written, so the
    // invoke's answer (the listener's Promise) was handled before the `started`
    // answer above: this poll takes it, and the call awaits the Promise — the
    // call the drop then cancels.
    let polled =
        std::future::poll_fn(|cx| std::task::Poll::Ready(dispatch.as_mut().poll(cx))).await;
    assert!(
        polled.is_pending(),
        "the dispatch waits on the listener's Promise: {polled:?}"
    );
    drop(dispatch);

    until(
        || process.call(&probe, "finished", json!([])).ok() == Some(json!(true)),
        "the listener ran to its end",
    )
    .await;
    assert_eq!(
        process.call(&probe, "aborted", json!([])).unwrap(),
        json!(false),
        "no signal for the call: only an unload aborts ctx.signal"
    );
    until(
        || process.connection().orphans() > 0,
        "the late reply discarded",
    )
    .await;
    assert_eq!(
        process.call(&probe, "ping", json!([])).unwrap(),
        json!("pong"),
        "the session goes on"
    );
    process.dispose().await.unwrap();
}
