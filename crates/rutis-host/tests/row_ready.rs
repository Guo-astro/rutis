//! A Rust host as `docs/guide/open-host.md` §1 and §3 write one: a
//! `SpawnPlugin` per plugin, the rows `<runtime>:<plugin>` on their
//! sessions (`SessionResolver`, `follow_row`), every runtime's `ready()`
//! awaited, then `reconcile`. Once `reconcile` returns, every row has
//! loaded (#934); a runtime killed and restarted brings its rows back as
//! soon as it runs again.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{file_url, repo};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::{Launcher, RuntimeHandle};
use rutis_host::connect::SpawnPlugin;
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::json;

/// Node runtimes start cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 3));

/// The plugins: two, each run by a Node runtime of its own.
const PLUGINS: [&str; 2] = ["weather", "clock"];

const PLUGIN: &str = r#"
import { definePlugin } from 'SDK'
export default definePlugin({ apply() {} })
"#;

/// `command` (`rutis_test_support::runtimes`) as a launcher.
fn launcher(command: rutis_test_support::runtimes::Command) -> Launcher {
    let mut launcher = Launcher::new(command.program);
    launcher.args = command.args;
    launcher.env = command.env;
    launcher.cwd = command.cwd;
    launcher.inherit_fd = command.inherit_fd;
    launcher
}

/// The plugins' files in `dir`: `<plugin>.mjs` each.
fn plugins(dir: &Path) {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    for plugin in PLUGINS {
        std::fs::write(
            dir.join(format!("{plugin}.mjs")),
            PLUGIN.replace("SDK", &file_url(&sdk)),
        )
        .unwrap();
    }
}

struct Host {
    root: Ctx,
    loader: Loader,
    runtimes: Vec<(
        RuntimeHandle,
        rutis_bridge::pipe::Processes,
        rutis::host::FiberView,
    )>,
}

/// The host started as open-host.md §1 starts it, `reconcile` returned.
async fn start(dir: &Path, follow: bool) -> Host {
    let root = Ctx::root().unwrap();
    root.provide(rutis_bridge::LocalId(
        rutis_bridge::PeerId::new("main").unwrap(),
    ))
    .unwrap();
    let catalog = ServiceCatalog::new();
    let sessions = Arc::new(SessionResolver::new(&catalog));
    let mut runtimes = Vec::new();
    let mut rows = Vec::new();
    for plugin in PLUGINS {
        let runtime = SpawnPlugin::new(
            plugin,
            launcher(rutis_test_support::runtimes::node_runtime()),
            repo().join("node/rutis-runtime/package.json"),
        );
        sessions.runtime(runtime.handle());
        let (handle, processes) = (runtime.handle(), runtime.processes());
        let fiber = root.plugin(runtime);
        runtimes.push((handle, processes, fiber));
        let entry = dir.join(format!("{plugin}.mjs"));
        rows.push(json!({ "id": plugin, "name": format!("{plugin}:{}", entry.display()) }));
    }
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    sessions.attach(&loader);
    (&root.plugin(plugin)).await.unwrap();
    if follow {
        for plugin in PLUGINS {
            sessions.follow_row(plugin);
        }
    }
    for (runtime, _, _) in &runtimes {
        assert!(runtime.ready().await.is_some(), "{} runs", runtime.name());
    }
    let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    let report = loader
        .reconcile(vec![Layer::new("plugins", rows)], None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    Host {
        root,
        loader,
        runtimes,
    }
}

/// The rows not running, with their status.
fn not_running(loader: &Loader) -> Vec<String> {
    loader
        .entries()
        .into_iter()
        .filter(|entry| {
            !matches!(&entry.status, EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active)
        })
        .map(|entry| format!("{}: {:?}", entry.id, entry.status))
        .collect()
}

/// Every time, the rows have loaded once `reconcile` returns: 100 starts.
#[tokio::test(flavor = "multi_thread")]
async fn rows_have_loaded_when_reconcile_returns_after_ready() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    plugins(dir.path());
    for time in 0..100 {
        let host = COLD
            .within("the host started", start(dir.path(), true))
            .await;
        let waiting = not_running(&host.loader);
        assert!(waiting.is_empty(), "start {time}: {waiting:?}");
        host.root.shutdown().await.unwrap();
    }
}

/// A runtime killed, then restarted (`restart()` on its fiber): its row
/// runs again as soon as the runtime does.
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_runtime_brings_its_row_back_at_once() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    plugins(dir.path());
    let host = COLD
        .within("the host started", start(dir.path(), true))
        .await;
    let (runtime, processes, fiber) = &host.runtimes[0];
    let name = runtime.name().to_owned();
    for time in 0..3 {
        processes.kill();
        COLD.until(
            || {
                not_running(&host.loader)
                    .iter()
                    .any(|row| row.starts_with(&name))
            },
            "the row stopped",
        )
        .await;
        fiber.restart().await.unwrap();
        COLD.within("the runtime runs again", runtime.ready()).await;
        let ran = Instant::now();
        COLD.until(|| not_running(&host.loader).is_empty(), "the row back")
            .await;
        let took = ran.elapsed();
        assert!(
            took < Duration::from_secs(2),
            "restart {time}: the row came back {took:?} after its runtime"
        );
    }
    host.root.shutdown().await.unwrap();
}

/// Without `follow_row`, the first resolution of a row follows its runtime:
/// the rows have loaded once `reconcile` returns all the same.
#[tokio::test(flavor = "multi_thread")]
async fn rows_have_loaded_when_reconcile_returns_without_follow_row() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    plugins(dir.path());
    for time in 0..5 {
        let host = COLD
            .within("the host started", start(dir.path(), false))
            .await;
        let waiting = not_running(&host.loader);
        assert!(waiting.is_empty(), "start {time}: {waiting:?}");
        host.root.shutdown().await.unwrap();
    }
}
