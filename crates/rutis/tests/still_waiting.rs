//! A plugin waiting a long time for services nothing provides is reported
//! once to the error sink, naming them (#676): an embedding application
//! that mounts a plugin before its provider (or never mounts one) sees why
//! nothing happens, instead of nothing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{CtxHost, ErrorKind, ErrorSink, FiberState};
use rutis::{Ctx, Plugin};
use rutis_test_support::within;

struct Greeting;

struct Printer;

impl Plugin for Printer {
    type Deps = (Arc<Greeting>,);

    fn name(&self) -> &str {
        "printer"
    }

    async fn apply(&self, _: &Ctx, _: Self::Deps) -> rutis::Result {
        Ok(())
    }
}

/// A root whose error sink records what it is sent, reporting waits after
/// `after`.
fn root(after: Option<Duration>) -> (Ctx, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let sunk = Arc::new(Mutex::new(Vec::new()));
    let sink: ErrorSink = {
        let sunk = sunk.clone();
        Arc::new(move |error| sunk.lock().unwrap().push(error))
    };
    let ctx = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
    ctx.report_waiting_after(after);
    (ctx, sunk)
}

/// The waits reported so far, as their messages.
fn reported(sunk: &Mutex<Vec<Arc<ErrorKind>>>) -> Vec<String> {
    sunk.lock()
        .unwrap()
        .iter()
        .filter(|error| matches!(***error, ErrorKind::StillWaiting { .. }))
        .map(|error| error.to_string())
        .collect()
}

/// Waiting past the delay is reported once, naming the type; the plugin
/// goes on waiting and starts when the service comes.
/// guarantee: core/features.still-waiting
#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_waiting_long_for_its_deps_is_reported_once() {
    let (ctx, sunk) = root(Some(Duration::from_millis(100)));
    let view = ctx.plugin(Printer);
    within("the report", async {
        while reported(&sunk).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    // Once: not again while it goes on waiting.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let said = reported(&sunk);
    assert_eq!(
        said,
        [format!(
            "plugin printer is still waiting after 0.1s for {}: it starts once that is provided",
            std::any::type_name::<Greeting>()
        )]
    );
    assert_eq!(view.state().state, FiberState::Pending);
    ctx.provide(Greeting).unwrap();
    within("printer to start", async {
        while view.state().state != FiberState::Active {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(reported(&sunk).len(), 1);
    ctx.shutdown().await.unwrap();
}

/// Waiting again (its service gone after it started) is a new wait,
/// reported again.
/// guarantee: core/features.still-waiting
#[tokio::test(flavor = "multi_thread")]
async fn waiting_again_is_reported_again() {
    let (ctx, sunk) = root(Some(Duration::from_millis(100)));
    let view = ctx.plugin(Printer);
    let reports = |n: usize| {
        let sunk = sunk.clone();
        within("the reports", async move {
            while reported(&sunk).len() < n {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
    };
    reports(1).await;
    let greeting = ctx.provide(Greeting).unwrap();
    within("printer to start", async {
        while view.state().state != FiberState::Active {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    greeting.dispose().await.unwrap();
    reports(2).await;
    assert_eq!(view.state().state, FiberState::Pending);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(reported(&sunk).len(), 2);
    ctx.shutdown().await.unwrap();
}

/// A wait shorter than the delay, or with reports off, is not reported.
/// guarantee: core/features.still-waiting
#[tokio::test(flavor = "multi_thread")]
async fn a_short_wait_or_reports_off_say_nothing() {
    let (ctx, sunk) = root(Some(Duration::from_millis(300)));
    let view = ctx.plugin(Printer);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let greeting = ctx.provide(Greeting).unwrap();
    within("printer to start", async {
        while view.state().state != FiberState::Active {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(reported(&sunk).is_empty(), "{:?}", reported(&sunk));
    drop(greeting);
    ctx.shutdown().await.unwrap();

    let (ctx, sunk) = root(None);
    ctx.plugin(Printer);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(reported(&sunk).is_empty(), "{:?}", reported(&sunk));
    ctx.shutdown().await.unwrap();
}
