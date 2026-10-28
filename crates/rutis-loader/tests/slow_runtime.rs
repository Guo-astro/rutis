//! A runtime that does not get ready (one waiting for its debugger) holds
//! up only its own rows (#678, from #682): the loader resolves every row
//! without waiting for it, the other runtime's rows start, and its own
//! rows wait for it.
#![cfg(all(feature = "python", unix))]

mod common;

use std::path::Path;
use std::sync::Arc;

use common::runtimes::{layer, row_state, Probe, COLD};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::{Launcher, LocalRuntime, RuntimeState};
use rutis_bridge::session::host_key;
use rutis_loader::report::WaitReason;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
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
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fast.py"), FAST).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/rutis");
    let python = LocalRuntime::python(dir.path()).python_path(sdk);
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()));
    // Never greets: it stays starting (a local runtime has no handshake
    // time limit).
    let stuck = LocalRuntime::launcher(
        "stuck",
        Launcher::new("sh").arg("-c").arg("sleep 600").arg("sh"),
        dir.path(),
    );
    let stuck_handle = stuck.handle();
    let stuck_rows = Arc::new(RuntimeResolver::modules(stuck.handle()));
    root.plugin(python).await.unwrap();
    // Not awaited: it is starting.
    let _stuck = root.plugin(stuck);
    COLD.until(
        || matches!(stuck_handle.state(), RuntimeState::Starting),
        "the stuck runtime starting",
    )
    .await;
    let plugin_loader = LoaderPlugin::new(
        Chain::new()
            .with_shared(stuck_rows.clone())
            .with_shared(python_rows.clone()),
        LoaderOptions::default(),
    );
    let loader = plugin_loader.handle();
    root.plugin(plugin_loader).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(python_rows));
    root.plugin(RuntimeRowsPlugin::new(stuck_rows));

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
