//! `FiberView::running` (#667): awaiting a view returns once what is
//! queued is handled, with a plugin whose dependency is missing still
//! `Pending`; `running()` waits until it runs, and says so when it never
//! will (failed, disposed, the root shut down).

use std::sync::Arc;
use std::time::Duration;

use rutis::host::{CtxHost, ErrorKind, FiberState};
use rutis::{Ctx, Plugin, Result};

pub struct Greeting(pub String);

pub struct Greeter;

impl Plugin for Greeter {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Greeting("hello".into()))?;
        Ok(())
    }
}

pub struct Printer;

impl Plugin for Printer {
    type Deps = Arc<Greeting>;

    async fn apply(&self, _: &Ctx, _: Arc<Greeting>) -> Result {
        Ok(())
    }
}

pub struct Broken;

impl Plugin for Broken {
    type Deps = Arc<Greeting>;

    async fn apply(&self, _: &Ctx, _: Arc<Greeting>) -> Result {
        Err(rutis::Error::msg("broken on purpose"))
    }
}

/// How long a wait that must not end is given before it counts as waiting.
const WHILE: Duration = Duration::from_millis(100);

/// The consumer mounted before its provider: awaiting the view returns with
/// it `Pending`; `running()` waits, and returns once the provider came.
/// guarantee: core/features.running
#[tokio::test]
async fn running_waits_for_the_dependencies() {
    let root = Ctx::root().unwrap();
    let printer = root.plugin(Printer);
    (&printer).await.unwrap();
    assert_eq!(printer.state().state, FiberState::Pending);
    assert!(
        tokio::time::timeout(WHILE, printer.running())
            .await
            .is_err(),
        "running() returned while Greeting is missing"
    );

    let waiting = tokio::spawn(printer.running());
    root.plugin(Greeter);
    tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("running() returns once Greeting is there")
        .unwrap()
        .unwrap();
    assert_eq!(printer.state().state, FiberState::Active);
    // Running already: at once.
    printer.running().await.unwrap();
    root.shutdown().await.unwrap();
}

/// A plugin that fails to load: `running()` returns the load's error.
/// guarantee: core/features.running
#[tokio::test]
async fn running_returns_the_load_error() {
    let root = Ctx::root().unwrap();
    let broken = root.plugin(Broken);
    let waiting = tokio::spawn(broken.running());
    root.plugin(Greeter);
    let error = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("running() returns once the load failed")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("broken on purpose"), "{error}");
    assert_eq!(broken.state().state, FiberState::Failed);
    root.shutdown().await.unwrap();
}

/// A plugin disposed, or its root shut down, while it waits: `running()`
/// returns `InactiveEffect` instead of waiting forever.
/// guarantee: core/features.running
#[tokio::test]
async fn running_ends_when_the_plugin_never_will() {
    let root = Ctx::root().unwrap();
    let disposed = root.plugin(Printer);
    let waiting = tokio::spawn(disposed.running());
    disposed.dispose().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("running() returns once the plugin is disposed")
        .unwrap()
        .unwrap_err();
    assert!(matches!(*error, ErrorKind::InactiveEffect), "{error}");

    let pending = root.plugin(Printer);
    let waiting = tokio::spawn(pending.running());
    tokio::time::sleep(WHILE).await;
    root.shutdown().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("running() returns once the root shut down")
        .unwrap()
        .unwrap_err();
    assert!(matches!(*error, ErrorKind::InactiveEffect), "{error}");
}
