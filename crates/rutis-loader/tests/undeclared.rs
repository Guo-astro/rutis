//! guarantee: loader/announced-services
//! A host that opens itself to third parties refuses the service names a
//! runtime row provides without declaring them (#1040): the name is not
//! published, the row fails saying which name and row, and the runtime's
//! other rows go on. By default every announced name is published, as
//! Cordis does; `UndeclaredPolicy::Allow` publishes the names it lists and
//! refuses the others.
#![cfg(feature = "runtimes")]

mod common;

use std::path::Path;

use common::runtimes::{
    active, greet_as_main, layer, repo, row_state, rows_stage, TestRuntime, COLD,
};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::report;
use rutis_bridge::runtime::UndeclaredPolicy;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// The plugin `store`: it does not declare what it provides, so the runtime
/// announces `store` when it runs, as a published Cordis package does.
const STORE: &str = r#"
class Store {
  get() { return 'stored' }
}

export function apply(ctx) {
  ctx.provide('store', new Store())
}
"#;

/// A plugin providing `llm` without declaring it: the name a first-party
/// row is meant to provide.
const IMPOSTOR: &str = r#"
class Llm {
  ask() { return 'impostor' }
}

export function apply(ctx) {
  ctx.provide('llm', new Llm())
}
"#;

/// A row of the runtime that provides nothing: it keeps running when
/// another row is refused.
const CALM: &str = r#"
export function apply() {}
"#;

struct Fixture {
    root: Ctx,
    loader: Loader,
    _dir: tempfile::TempDir,
}

/// A loader with one Node runtime `name`, its rows `<name>:<file>`, under
/// `policy` for the services they provide without declaring it.
async fn fixture(name: &str, policy: UndeclaredPolicy, files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    for (file, text) in files {
        std::fs::write(dir.path().join(file), text).unwrap();
    }
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    let catalog = ServiceCatalog::new();
    let runtime = TestRuntime::node(&repo().join("node/rutis-runtime/package.json"))
        .named(name)
        .undeclared(policy);
    let handle = runtime.handle();
    let sessions = std::sync::Arc::new(SessionResolver::new(&catalog));
    sessions.runtime(handle);
    root.plugin(runtime);
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    sessions.attach(&loader);
    root.plugin(plugin).await.unwrap();
    rows_stage(&root, &sessions, name).await;
    Fixture {
        root,
        loader,
        _dir: dir,
    }
}

/// The row `name` at `runtime:file`.
fn row(name: &str, runtime: &str, dir: &Path, file: &str) -> Value {
    json!({ "id": name, "name": format!("{runtime}:{}", dir.join(file).display()) })
}

/// The error the row `id` failed with, as a host prints it.
async fn failure(loader: &Loader, id: &str) -> String {
    COLD.eventually_with(
        || match loader.get(id)?.status {
            EntryStatus::Running(snapshot) if snapshot.state == rutis::host::FiberState::Failed => {
                Some(
                    snapshot
                        .error
                        .map(|error| report::report(&error))
                        .unwrap_or_default(),
                )
            }
            _ => None,
        },
        &format!("row {id} to fail"),
        || format!("{:?}", loader.get(id).map(|entry| entry.status)),
    )
    .await
}

/// The default is `Publish`: a name the row does not declare is published,
/// as Cordis does.
/// guarantee: loader/announced-services
#[tokio::test(flavor = "multi_thread")]
async fn the_default_publishes_a_name_the_row_does_not_declare() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let fixture = fixture(
        "open",
        UndeclaredPolicy::Publish,
        &[("impostor.mjs", IMPOSTOR)],
    )
    .await;
    let dir = fixture._dir.path();
    fixture
        .loader
        .reconcile(
            layer(vec![row("impostor", "open", dir, "impostor.mjs")]),
            None,
        )
        .await
        .unwrap();
    COLD.until(
        || active(&fixture.loader.get("impostor").unwrap()),
        "the row to run",
    )
    .await;
    COLD.until(
        || {
            fixture
                .root
                .get_at::<dyn HostDispatch>(&host_key("llm"))
                .is_some()
        },
        "the undeclared announcement",
    )
    .await;
    assert_eq!(
        fixture
            .root
            .get_at::<dyn HostDispatch>(&host_key("llm"))
            .unwrap()
            .methods(),
        Some(json!({ "ask": "async" }))
    );
    fixture.root.shutdown().await.unwrap();
}

/// A row announcing a name it does not declare is refused: the name is not
/// published, the row fails naming the name and the row, and the other row
/// of the same runtime runs on.
/// guarantee: loader/announced-services
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_announcement_fails_its_row_and_is_not_published() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let fixture = fixture(
        "strict",
        UndeclaredPolicy::Refuse,
        &[("impostor.mjs", IMPOSTOR), ("calm.mjs", CALM)],
    )
    .await;
    let dir = fixture._dir.path();
    let report = fixture
        .loader
        .reconcile(
            layer(vec![
                row("calm", "strict", dir, "calm.mjs"),
                row("impostor", "strict", dir, "impostor.mjs"),
            ]),
            None,
        )
        .await
        .unwrap();
    assert!(
        report.failures.iter().all(|failed| failed.id != "calm"),
        "{report:?}"
    );
    let error = failure(&fixture.loader, "impostor").await;
    assert_eq!(
        error,
        "llm is provided by row impostor without declaring it; the host does not let a runtime \
         publish names it does not declare"
    );
    assert!(
        fixture
            .root
            .get_at::<dyn HostDispatch>(&host_key("llm"))
            .is_none(),
        "the refused name is not a service of the host"
    );
    assert_eq!(
        row_state(&fixture.loader, "calm"),
        Some(rutis::host::FiberState::Active),
        "the other row of the runtime runs on: {}",
        fixture.loader.status()
    );
    fixture.root.shutdown().await.unwrap();
}

/// An allowed announcement is published, comes back after the row reloads,
/// and goes with the row.
/// guarantee: loader/announced-services
#[tokio::test(flavor = "multi_thread")]
async fn an_allowed_announcement_follows_its_row_across_reload_and_removal() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let fixture = fixture(
        "scoped",
        UndeclaredPolicy::allow(["store"]),
        &[("store.mjs", STORE)],
    )
    .await;
    let dir = fixture._dir.path();
    let rows = vec![row("store", "scoped", dir, "store.mjs")];
    fixture
        .loader
        .reconcile(layer(rows.clone()), None)
        .await
        .unwrap();
    COLD.until(
        || active(&fixture.loader.get("store").unwrap()),
        "the store row to run",
    )
    .await;
    COLD.until(
        || {
            fixture
                .root
                .get_at::<dyn HostDispatch>(&host_key("store"))
                .is_some()
        },
        "the allowed announcement",
    )
    .await;
    assert_eq!(
        fixture
            .root
            .get_at::<dyn HostDispatch>(&host_key("store"))
            .unwrap()
            .methods(),
        Some(json!({ "get": "async" }))
    );

    // Reloading the row withdraws its service and the new object's comes.
    fixture.loader.reload("store").await.unwrap();
    COLD.until(
        || {
            active(&fixture.loader.get("store").unwrap())
                && fixture
                    .root
                    .get_at::<dyn HostDispatch>(&host_key("store"))
                    .is_some()
        },
        "the store row reloaded and its service back",
    )
    .await;

    // The row goes: its announced service goes with it.
    fixture
        .loader
        .reconcile(layer(Vec::new()), None)
        .await
        .unwrap();
    COLD.until(
        || {
            fixture
                .root
                .get_at::<dyn HostDispatch>(&host_key("store"))
                .is_none()
        },
        "the withdrawn announcement",
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}
