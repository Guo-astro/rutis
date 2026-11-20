//! A generation that a dispose or shutdown stops while it is loading (#983).
//! What its `apply` returns after the cancellation is the plugin answering
//! it and is not reported (#968), but its cleanups still run, and a cleanup
//! that fails is a resource this generation did not release: the error stays
//! on the fiber's final state and the dispose or shutdown returns it, as
//! when an `Active` fiber is disposed. A root shutdown ends the plugin as it
//! ends an `Active` one, and the error reaches the sink. A restart (not
//! terminal) unloads the generation to `Pending` and the cleanup error goes
//! to the error sink.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, ErrorSink, FiberState, FiberStatusChanged,
    FiberView,
};
use rutis::{Ctx, Error};
use tokio::sync::Notify;

const LIMIT: Duration = Duration::from_secs(5);

const ROLLBACK: &str = "rollback failed";

/// Its first generation registers a cleanup that fails, waits for the
/// cancellation and returns `Err`; later generations return `Ok`.
struct Rollback {
    applies: Arc<AtomicUsize>,
    entered: Arc<Notify>,
}

impl DynPlugin for Rollback {
    fn name(&self) -> &str {
        "terminal-cleanup-rollback"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if self.applies.fetch_add(1, Ordering::SeqCst) == 0 {
                ctx.effect(|| async {
                    Err(Error::from(ErrorKind::PluginFailed(
                        "rollback failed".into(),
                    )))
                })?;
                self.entered.notify_one();
                ctx.cancelled().await;
                return Err(Error::from(ErrorKind::PluginFailed(
                    "cancelled while waiting".into(),
                )));
            }
            Ok(Effect::Done)
        })
    }
}

/// An error sink that collects what the fiber reports instead of printing it.
fn sink() -> (ErrorSink, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let errors: Arc<Mutex<Vec<Arc<ErrorKind>>>> = Arc::default();
    let out = errors.clone();
    let sink: ErrorSink = Arc::new(move |error| out.lock().unwrap().push(error));
    (sink, errors)
}

/// A root with a collecting sink and the plugin, waiting in its first
/// `apply` after registering the failing cleanup.
async fn mount() -> (
    Ctx,
    FiberView,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<Arc<ErrorKind>>>>,
) {
    let (sink, errors) = sink();
    let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
    let applies = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let view = root.plugin(Rollback {
        applies: applies.clone(),
        entered: entered.clone(),
    });
    tokio::time::timeout(LIMIT, entered.notified())
        .await
        .expect("first apply starts");
    assert_eq!(view.state().state, FiberState::Loading);
    (root, view, applies, errors)
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

fn messages(errors: &Mutex<Vec<Arc<ErrorKind>>>) -> Vec<String> {
    errors
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.to_string())
        .collect()
}

/// `view.dispose()` stops the loading generation: the dispose returns the
/// cleanup's error (not the `Err` `apply` answered the cancellation with),
/// the fiber ends `Disposed` with it, and the sink does not get it.
///
/// guarantee: core/D7
#[tokio::test]
async fn disposing_a_loading_plugin_returns_its_cleanup_error() {
    let (root, view, applies, errors) = mount().await;
    let path = status_path(&root, view.id);

    let error = tokio::time::timeout(LIMIT, view.dispose())
        .await
        .expect("dispose returns")
        .expect_err("the cleanup failed");
    assert_eq!(error.to_string(), ROLLBACK);
    let snapshot = view.state();
    assert_eq!(snapshot.state, FiberState::Disposed);
    assert_eq!(
        snapshot.error.map(|e| e.to_string()).as_deref(),
        Some(ROLLBACK)
    );
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    drained(&root).await;
    let path = path.lock().unwrap().clone();
    assert!(
        path.iter().all(|(_, to)| *to != FiberState::Failed),
        "a pre-cancelled load failed: {path:?}"
    );
    assert_eq!(messages(&errors), Vec::<String>::new());

    root.shutdown().await.unwrap();
}

/// `view.shutdown()` of the loading plugin returns the cleanup's error, as
/// the dispose does.
///
/// guarantee: core/D7
#[tokio::test]
async fn shutting_down_a_loading_plugin_returns_its_cleanup_error() {
    let (root, view, applies, errors) = mount().await;

    let error = tokio::time::timeout(LIMIT, view.shutdown())
        .await
        .expect("shutdown returns")
        .expect_err("the cleanup failed");
    assert_eq!(error.to_string(), ROLLBACK);
    let snapshot = view.state();
    assert_eq!(snapshot.state, FiberState::Disposed);
    assert_eq!(
        snapshot.error.map(|e| e.to_string()).as_deref(),
        Some(ROLLBACK)
    );
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    assert_eq!(messages(&errors), Vec::<String>::new());

    root.shutdown().await.unwrap();
}

/// `root.shutdown()` with the plugin still loading ends it through the
/// root's cascading dispose, as it ends an `Active` plugin: the root's
/// result does not carry a plugin's cleanup errors, and this one reaches the
/// error sink exactly once. The plugin's `apply` returns while the root
/// drains its mounts, in either order; on a multi-thread runtime the test
/// repeats to meet both.
///
/// guarantee: core/D7
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutting_down_the_root_reports_a_loading_plugins_cleanup_error_once() {
    for round in 0..64 {
        let (root, view, applies, errors) = mount().await;
        // The states this round went through, sampled while the shutdown
        // runs: the one nightly run that failed here (`stress`, 2026-11-05)
        // said only `left != right`, which is not enough to tell a state the
        // contract does not allow from one this test stopped looking at too
        // early (#1080).
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sampler = {
            let seen = seen.clone();
            let view = view.clone();
            tokio::spawn(async move {
                loop {
                    seen.lock()
                        .unwrap()
                        .push(format!("{:?}", view.state().state));
                    tokio::time::sleep(Duration::from_micros(200)).await;
                }
            })
        };

        tokio::time::timeout(LIMIT, root.shutdown())
            .await
            .expect("shutdown returns")
            .unwrap();
        sampler.abort();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            view.state().state,
            FiberState::Disposed,
            "round {round}, the states seen: {seen:?}"
        );
        assert_eq!(applies.load(Ordering::SeqCst), 1);
        assert_eq!(messages(&errors), vec![ROLLBACK.to_string()]);
    }
}

/// A restart is not terminal: the stopped generation unloads to `Pending`
/// with no `Failed` (#968), its cleanup error goes to the sink, the restart
/// loads the plugin again, and the later shutdown has nothing to report.
///
/// guarantee: core/D7
#[tokio::test]
async fn restarting_a_loading_plugin_sends_its_cleanup_error_to_the_sink() {
    let (root, view, applies, errors) = mount().await;
    let path = status_path(&root, view.id);

    tokio::time::timeout(LIMIT, view.restart())
        .await
        .expect("restart returns")
        .unwrap();
    assert_eq!(view.state().state, FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
    assert_eq!(messages(&errors), vec![ROLLBACK.to_string()]);
    drained(&root).await;
    let path = path.lock().unwrap().clone();
    assert!(
        path.contains(&(FiberState::Unloading, FiberState::Pending))
            && path.iter().all(|(_, to)| *to != FiberState::Failed),
        "unexpected status path: {path:?}"
    );

    root.shutdown().await.unwrap();
}
