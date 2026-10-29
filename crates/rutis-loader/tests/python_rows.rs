//! Python rows alone: no Node runtime is compiled or started.
#![cfg(feature = "python")]

mod common;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::runtimes::{greet_as_main, rows_stage, Probe, TestRuntime};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::session::host_key;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch};
use serde_json::json;

fn plugin(version: &str) -> String {
    format!(
        "inject = [\"probe\"]\n\n\ndef apply(ctx, config):\n    probe = ctx.use(\"probe\")\n    probe.record(\"{version}: start\")\n    return lambda: probe.record(\"{version}: bye\")\n"
    )
}

/// After the module's source changes, reloading the row runs the new code.
/// guarantee: loader/reload-all-or-nothing
#[tokio::test(flavor = "multi_thread")]
async fn reloading_a_row_runs_the_edited_module() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("edited.py");
    std::fs::write(&module, plugin("v1")).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    greet_as_main(&root);
    let python = TestRuntime::python(dir.path());
    let options = LoaderOptions::default();
    let sessions = Arc::new(SessionResolver::new(&options.catalog));
    sessions.runtime(python.handle());
    root.plugin(python).await.unwrap();
    let plugin_loader = LoaderPlugin::new(Chain::new().with_shared(sessions.clone()), options);
    let loader = plugin_loader.handle();
    sessions.attach(&loader);
    root.plugin(plugin_loader).await.unwrap();
    rows_stage(&root, &sessions, "py").await;

    let patches: Vec<Patch> = serde_json::from_value(
        json!([{ "insert": [{ "id": "e", "name": "py:edited", "config": {} }] }]),
    )
    .unwrap();
    loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    probe.wait_for("v1: start").await;

    std::fs::write(&module, plugin("v2")).unwrap();
    // A later modification time, even within the same clock tick.
    std::fs::File::options()
        .write(true)
        .open(&module)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    loader.reload("e").await.unwrap();
    probe.wait_for("v1: bye").await;
    probe.wait_for("v2: start").await;
    root.shutdown().await.unwrap();
}
