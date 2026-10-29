//! A runtime that does not get ready (one waiting for its debugger) holds
//! up only its own rows (#678, from #682): the loader resolves every row
//! without waiting for it, the other runtime's rows start, and its own
//! rows wait for it.
#![cfg(all(feature = "runtimes", unix))]

mod common;

use std::sync::Arc;

use common::runtimes::{greet_as_main, layer, row_state, Probe, TestRuntime, COLD};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::{Launcher, RuntimeState};
use rutis_bridge::session::host_key;
use rutis_loader::report::WaitReason;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, LoaderOptions, LoaderPlugin};
use serde_json::json;

/// Records that it started.
const FAST: &str = "inject = [\"probe\"]\n\ndef apply(ctx, config):\n    ctx.use(\"probe\").record(\"fast: started\")\n";

/// While the runtime `stuck` (a process that never greets) is starting,
/// `reconcile` returns, the Python row starts, and the stuck runtime's row
/// waits for its runtime.
/// guarantee: loader/runtimes-do-not-wait-for-each-other
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_still_starting_holds_up_only_its_own_rows() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fast.py"), FAST).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    greet_as_main(&root);
    let python = TestRuntime::python(dir.path());
    // Never connects: it stays starting (no time limit applies until the
    // process connects).
    let stuck = TestRuntime::new(
        "stuck",
        Launcher::new("sh").arg("-c").arg("sleep 600").arg("sh"),
        dir.path(),
    );
    let stuck_handle = stuck.handle();
    let options = LoaderOptions::default();
    let sessions = Arc::new(SessionResolver::new(&options.catalog));
    sessions.runtime(python.handle());
    sessions.runtime(stuck.handle());
    root.plugin(python).await.unwrap();
    // Not awaited: it is starting.
    let _stuck = root.plugin(stuck);
    COLD.until(
        || matches!(stuck_handle.state(), RuntimeState::Starting),
        "the stuck runtime starting",
    )
    .await;
    let plugin_loader = LoaderPlugin::new(Chain::new().with_shared(sessions.clone()), options);
    let loader = plugin_loader.handle();
    sessions.attach(&loader);
    root.plugin(plugin_loader).await.unwrap();

    // The stuck runtime's row first: resolving it does not wait.
    COLD.within(
        "reconcile",
        loader.reconcile(
            layer(vec![
                json!({ "id": "held", "name": "stuck:thing" }),
                json!({ "id": "fast", "name": "py:fast" }),
            ]),
            None,
        ),
    )
    .await
    .unwrap();
    probe.wait_for("fast: started").await;
    // `started` is recorded inside `apply`, before the row runs in rutis.
    COLD.until(
        || row_state(&loader, "fast") == Some(FiberState::Active),
        "the fast row running in rutis",
    )
    .await;
    assert!(matches!(stuck_handle.state(), RuntimeState::Starting));
    assert_eq!(row_state(&loader, "held"), Some(FiberState::Pending));
    let held = loader.get("held").unwrap();
    let waits = loader.waits(&held);
    assert!(
        waits
            .iter()
            .any(|wait| wait.service == "stuck" && wait.reason == WaitReason::OwnRuntime),
        "{waits:?}"
    );
    root.shutdown().await.unwrap();
}
