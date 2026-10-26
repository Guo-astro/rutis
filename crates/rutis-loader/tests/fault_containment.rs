//! One row's or one runtime's problem stays its own (#682): a row providing
//! a service the host already provides fails alone; rows of a runtime whose
//! process exits wait for it instead of failing, and the runtime says how
//! its process ended.
#![cfg(any(feature = "node", feature = "python", feature = "bun"))]

mod common;

use std::future::IntoFuture;
use std::path::Path;
use std::sync::Arc;

use common::runtimes::{layer, repo, row_state, Probe, COLD};
use rutis::host::{CtxHost, FiberState, FiberView};
use rutis::Ctx;
use rutis_bridge::runtime::{LocalRuntime, RuntimeHandle};
use rutis_bridge::session::{host_key, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// The host service `notes`: `get` answers `"host"`.
struct Notes;

impl HostDispatch for Notes {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "get");
        Ok(json!("host").into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "get": "sync" }))
    }
}

/// A host service with no methods, provided when a test lets a row start.
struct Gate;

impl HostDispatch for Gate {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        Err(rutis_bridge::session::Error::Value(format!(
            "gate has no {method}"
        )))
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({}))
    }
}

/// The language a fixture runs its rows in.
#[derive(Clone, Copy, Debug)]
enum Language {
    #[cfg(feature = "node")]
    Node,
    #[cfg(feature = "python")]
    Python,
    #[cfg(all(unix, feature = "bun"))]
    Bun,
}

impl Language {
    /// The row name of the plugin `stem` written in `dir`.
    fn row_name(self, dir: &Path, stem: &str) -> String {
        let _ = dir;
        match self {
            #[cfg(feature = "node")]
            Language::Node => dir
                .join(format!("{stem}.mjs"))
                .to_string_lossy()
                .into_owned(),
            #[cfg(feature = "python")]
            Language::Python => format!("py:{stem}"),
            #[cfg(all(unix, feature = "bun"))]
            Language::Bun => format!("bun:./{stem}.mjs"),
        }
    }

    /// Write the plugin `stem` into `dir`: `js` for the JavaScript
    /// runtimes (a `definePlugin` argument), `py` for Python.
    fn write(self, dir: &Path, stem: &str, js: &str, py: &str) {
        let _ = (js, py);
        match self {
            #[cfg(feature = "node")]
            Language::Node => std::fs::write(dir.join(format!("{stem}.mjs")), sdk_plugin(js)),
            #[cfg(feature = "python")]
            Language::Python => std::fs::write(dir.join(format!("{stem}.py")), py),
            #[cfg(all(unix, feature = "bun"))]
            Language::Bun => std::fs::write(dir.join(format!("{stem}.mjs")), sdk_plugin(js)),
        }
        .unwrap();
    }

    /// The runtime, on `dir`.
    fn runtime(self, dir: &Path) -> (LocalRuntime, RuntimeResolver) {
        let _ = dir;
        match self {
            #[cfg(feature = "node")]
            Language::Node => {
                let package = repo().join("node/rutis-runtime");
                let runtime = LocalRuntime::node(&package, package.join("package.json"));
                let resolver = RuntimeResolver::node(runtime.handle());
                (runtime, resolver)
            }
            #[cfg(feature = "python")]
            Language::Python => {
                let runtime = LocalRuntime::python(dir).python_path(repo().join("python/rutis"));
                let resolver = RuntimeResolver::modules(runtime.handle());
                (runtime, resolver)
            }
            #[cfg(all(unix, feature = "bun"))]
            Language::Bun => {
                std::fs::write(dir.join("package.json"), "{}").unwrap();
                let runtime = LocalRuntime::bun(repo().join("bun/rutis-bun"), dir);
                let resolver = RuntimeResolver::modules(runtime.handle());
                (runtime, resolver)
            }
        }
    }
}

/// A leaf plugin written with the SDK, imported from this checkout.
#[cfg(any(feature = "node", all(unix, feature = "bun")))]
fn sdk_plugin(body: &str) -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    // A file URL: an absolute Windows path is no module specifier.
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    format!("import {{ definePlugin }} from {sdk:?}\nexport default definePlugin({body})\n")
}

/// A root providing `probe` and `notes`, one runtime of `language` on
/// `dir`, a loader sharing every name, as rutis-host's does, and the rows'
/// second stage.
struct Fixture {
    root: Ctx,
    loader: Loader,
    runtime: FiberView,
    handle: RuntimeHandle,
    probe: Probe,
}

async fn fixture(language: Language, dir: &Path) -> Fixture {
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    root.provide_at(host_key("notes"), Arc::new(Notes)).unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.share_by_name();
    let (runtime, resolver) = language.runtime(dir);
    let handle = runtime.handle();
    let resolver = Arc::new(resolver.with_catalog(&catalog));
    let runtime = root.plugin(runtime);
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(resolver.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(resolver));
    COLD.within("the runtime to start", (&runtime).into_future())
        .await
        .unwrap();
    Fixture {
        root,
        loader,
        runtime,
        handle,
        probe,
    }
}

/// The error of the row `id`, once it failed.
async fn failure(loader: &Loader, id: &str) -> String {
    COLD.eventually_with(
        || match loader.get(id)?.status {
            EntryStatus::Running(snapshot) if snapshot.state == FiberState::Failed => {
                Some(snapshot.error.map(|e| e.to_string()).unwrap_or_default())
            }
            _ => None,
        },
        &format!("row {id} to fail"),
        || format!("{:?}", loader.get(id).map(|entry| entry.status)),
    )
    .await
}

// ── A row providing what the host provides ──────────────────────

/// Provides `notes`, which the host provides, without declaring it, then
/// keeps loading for a while: the name is taken in the runtime meanwhile.
const CLAIMS_NOTES_JS: &str = r#"{
  inject: ['probe'],
  async apply(ctx) {
    ctx.provide('notes', { get: () => 'claimed' })
    ctx.use('probe').record('claimer: provided')
    await new Promise(resolve => setTimeout(resolve, 1500))
  },
}"#;

const CLAIMS_NOTES_PY: &str = r#"
import asyncio

inject = ["probe"]


class Claimed:
    def get(self):
        return "claimed"


async def apply(ctx, config):
    ctx.provide("notes", Claimed())
    ctx.use("probe").record("claimer: provided")
    await asyncio.sleep(1.5)
"#;

/// Uses the host's `notes`, once `gate` lets it start.
const USES_NOTES_JS: &str = r#"{
  inject: ['notes', 'gate', 'probe'],
  apply(ctx) {
    ctx.use('probe').record(`user: ${ctx.use('notes').get()}`)
  },
}"#;

const USES_NOTES_PY: &str = r#"
inject = ["notes", "gate", "probe"]


def apply(ctx, config):
    ctx.use("probe").record(f"user: {ctx.use('notes').get()}")
"#;

/// Looks `notes` up without injecting it: it finds the host's.
const PEEKS_JS: &str = r#"{
  inject: ['probe'],
  apply(ctx) {
    ctx.use('probe').record(`peeker: ${ctx.use('notes').get()}`)
  },
}"#;

const PEEKS_PY: &str = r#"
inject = ["probe"]


def apply(ctx, config):
    ctx.use("probe").record(f"peeker: {ctx.use('notes').get()}")
"#;

/// A row providing a service the host provides fails, and only it: a row
/// of the same runtime using the host's service starts while the name is
/// taken in the runtime, and keeps running (#682).
/// guarantee: loader/announced-services
async fn a_clashing_provide_fails_only_its_row(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    language.write(dir.path(), "claimer", CLAIMS_NOTES_JS, CLAIMS_NOTES_PY);
    language.write(dir.path(), "user", USES_NOTES_JS, USES_NOTES_PY);
    language.write(dir.path(), "peeker", PEEKS_JS, PEEKS_PY);
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe) = (&fixture.loader, &fixture.probe);
    let mut rows = vec![
        json!({ "id": "claimer", "name": language.row_name(dir.path(), "claimer") }),
        json!({ "id": "user", "name": language.row_name(dir.path(), "user") }),
    ];
    // Not awaited: the claimer's start takes its while.
    let reconcile = tokio::spawn({
        let (loader, rows) = (loader.clone(), rows.clone());
        async move { loader.reconcile(layer(rows), None).await }
    });
    probe.wait_for("claimer: provided").await;
    // The user starts while the claimer holds the name in the runtime.
    let _gate = fixture
        .root
        .provide_at(host_key("gate"), Arc::new(Gate))
        .unwrap();
    probe.wait_for("user: host").await;
    let error = failure(loader, "claimer").await;
    assert!(
        error.contains("notes is already provided by the application"),
        "{language:?}: {error}"
    );
    reconcile.await.unwrap().unwrap();
    assert_eq!(
        row_state(loader, "user"),
        Some(FiberState::Active),
        "{language:?}"
    );
    // The name is the host's again in the runtime, also where the user's
    // lease registered it while the claimer held it.
    rows.push(json!({ "id": "peeker", "name": language.row_name(dir.path(), "peeker") }));
    loader.reconcile(layer(rows), None).await.unwrap();
    probe.wait_for("peeker: host").await;
    fixture.root.shutdown().await.unwrap();
}

#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn node_a_clashing_provide_fails_only_its_row() {
    a_clashing_provide_fails_only_its_row(Language::Node).await;
}

#[cfg(feature = "python")]
#[tokio::test(flavor = "multi_thread")]
async fn python_a_clashing_provide_fails_only_its_row() {
    a_clashing_provide_fails_only_its_row(Language::Python).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_a_clashing_provide_fails_only_its_row() {
    a_clashing_provide_fails_only_its_row(Language::Bun).await;
}

/// Provides `notes` and stays: with `notes` isolated in its config, the
/// service is the row's own scope's, not rutis's to refuse.
const KEEPS_NOTES_JS: &str = r#"{
  inject: ['probe'],
  apply(ctx) {
    ctx.provide('notes', { get: () => 'kept' })
    ctx.use('probe').record('keeper: provided')
  },
}"#;

const KEEPS_NOTES_PY: &str = r#"
inject = ["probe"]


class Kept:
    def get(self):
        return "kept"


def apply(ctx, config):
    ctx.provide("notes", Kept())
    ctx.use("probe").record("keeper: provided")
"#;

/// The same in a scope rows share by isolating `notes` with one label: the
/// host's `notes` there waits for the name while a row holds it, the rows
/// using the host's run meanwhile, and the host's is there once that row
/// has gone, whose withdrawal only that scope hears (#682).
/// guarantee: loader/announced-services
async fn a_name_held_in_an_isolated_scope_comes_back_to_the_host(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    language.write(dir.path(), "keeper", KEEPS_NOTES_JS, KEEPS_NOTES_PY);
    language.write(dir.path(), "user", USES_NOTES_JS, USES_NOTES_PY);
    language.write(dir.path(), "peeker", PEEKS_JS, PEEKS_PY);
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe) = (&fixture.loader, &fixture.probe);
    let row = |id: &str| {
        json!({
            "id": id,
            "name": language.row_name(dir.path(), id),
            "isolate": { "notes": "box" },
        })
    };
    loader
        .reconcile(layer(vec![row("keeper"), row("user")]), None)
        .await
        .unwrap();
    probe.wait_for("keeper: provided").await;
    // The user starts while the keeper holds the name in the box.
    let _gate = fixture
        .root
        .provide_at(host_key("gate"), Arc::new(Gate))
        .unwrap();
    probe.wait_for("user: host").await;
    // The keeper goes: in the box, the name is the host's again.
    loader
        .reconcile(layer(vec![row("user"), row("peeker")]), None)
        .await
        .unwrap();
    probe.wait_for("peeker: host").await;
    assert_eq!(
        row_state(loader, "user"),
        Some(FiberState::Active),
        "{language:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn node_a_name_held_in_an_isolated_scope_comes_back_to_the_host() {
    a_name_held_in_an_isolated_scope_comes_back_to_the_host(Language::Node).await;
}

#[cfg(feature = "python")]
#[tokio::test(flavor = "multi_thread")]
async fn python_a_name_held_in_an_isolated_scope_comes_back_to_the_host() {
    a_name_held_in_an_isolated_scope_comes_back_to_the_host(Language::Python).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_a_name_held_in_an_isolated_scope_comes_back_to_the_host() {
    a_name_held_in_an_isolated_scope_comes_back_to_the_host(Language::Bun).await;
}

// ── A runtime whose process exits ───────────────────────────────

/// Running for a while already when the runtime exits.
const BYSTANDER_JS: &str = r#"{
  inject: ['probe'],
  apply(ctx) { ctx.use('probe').record('bystander: started') },
}"#;

const BYSTANDER_PY: &str = r#"
inject = ["probe"]


def apply(ctx, config):
    ctx.use("probe").record("bystander: started")
"#;

/// Provides `work`, whose `run` never answers: a call in progress when the
/// runtime exits.
const WORKER_JS: &str = r#"{
  inject: ['probe'],
  provides: { work: { run: 'async' } },
  apply(ctx) {
    ctx.provide('work', { run: () => new Promise(() => {}) })
    ctx.use('probe').record('worker: started')
  },
}"#;

const WORKER_PY: &str = r#"
import asyncio

inject = ["probe"]


class Work:
    async def run(self):
        await asyncio.Event().wait()


provides = {"work": Work}


def apply(ctx, config):
    ctx.provide("work", Work())
    ctx.use("probe").record("worker: started")
"#;

/// Still loading when the runtime exits.
const SLOWPOKE_JS: &str = r#"{
  inject: ['probe'],
  async apply(ctx) {
    ctx.use('probe').record('slowpoke: loading')
    await new Promise(resolve => setTimeout(resolve, 10000))
  },
}"#;

const SLOWPOKE_PY: &str = r#"
import asyncio

inject = ["probe"]


async def apply(ctx, config):
    ctx.use("probe").record("slowpoke: loading")
    await asyncio.sleep(10)
"#;

/// Makes its runtime exit once the slowpoke loads: a promise rejection
/// nobody handles (JavaScript), `os._exit` (Python).
const CRASHER_JS: &str = r#"{
  inject: ['loading', 'probe'],
  apply(ctx) {
    ctx.use('probe').record('crasher: started')
    setTimeout(() => { Promise.reject(new Error('boom')) }, 200)
  },
}"#;

const CRASHER_PY: &str = r#"
import os
import threading

inject = ["loading", "probe"]


def apply(ctx, config):
    ctx.use("probe").record("crasher: started")
    threading.Timer(0.2, lambda: os._exit(3)).start()
"#;

/// How `ExitStatus` reads an exit code: `exit status: 3` on Unix, `exit
/// code: 3` on Windows.
fn exited(code: i32) -> String {
    match cfg!(windows) {
        true => format!("exit code: {code}"),
        false => format!("exit status: {code}"),
    }
}

/// The state of every row of `ids`, once each has settled in a state other
/// than starting.
fn states(loader: &Loader, ids: &[&str]) -> Vec<(String, Option<FiberState>)> {
    ids.iter()
        .map(|id| (id.to_string(), row_state(loader, id)))
        .collect()
}

/// A runtime whose process exits says how it ended; the rows that only ran
/// there, loading or running, wait for it and none of them fails, and a
/// call in progress there fails saying how the process ended (#682).
/// guarantee: loader/runtime-down-stops-its-rows
async fn a_runtime_that_exits_blames_no_other_row(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    for (stem, js, py) in [
        ("bystander", BYSTANDER_JS, BYSTANDER_PY),
        ("worker", WORKER_JS, WORKER_PY),
        ("slowpoke", SLOWPOKE_JS, SLOWPOKE_PY),
        ("crasher", CRASHER_JS, CRASHER_PY),
    ] {
        language.write(dir.path(), stem, js, py);
    }
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe, root) = (&fixture.loader, &fixture.probe, &fixture.root);
    let row = |id: &str| json!({ "id": id, "name": language.row_name(dir.path(), id) });
    loader
        .reconcile(layer(vec![row("bystander"), row("worker")]), None)
        .await
        .unwrap();
    probe.wait_for("worker: started").await;
    let work = root
        .get_at::<dyn HostDispatch>(&host_key("work"))
        .expect("the worker's service");
    let call = tokio::spawn(async move {
        rutis_bridge::session::settle(work.invoke("run", RpcValue::List(Vec::new()))?).await
    });

    // The crasher starts once the slowpoke loads: the runtime exits while
    // the slowpoke still does.
    let rows = vec![
        row("bystander"),
        row("worker"),
        row("slowpoke"),
        row("crasher"),
    ];
    let reconcile = tokio::spawn({
        let loader = loader.clone();
        async move { loader.reconcile(layer(rows), None).await }
    });
    probe.wait_for("slowpoke: loading").await;
    let _loading = root
        .provide_at(host_key("loading"), Arc::new(Gate))
        .unwrap();
    let reason = COLD
        .eventually(
            || match fixture.handle.state() {
                rutis_bridge::runtime::RuntimeState::Down(reason) => Some(reason),
                _ => None,
            },
            "the runtime to be down",
        )
        .await;
    let error = COLD
        .within("the call in progress to fail", call)
        .await
        .unwrap()
        .expect_err("the call cannot succeed")
        .to_string();
    // How the process ended, in the runtime's reason and in the call's error.
    let status = match language {
        #[cfg(feature = "python")]
        Language::Python => exited(3),
        #[allow(unreachable_patterns)]
        _ => exited(1),
    };
    assert!(reason.contains(&status), "{language:?}: {reason}");
    assert!(error.contains(&status), "{language:?}: {error}");
    // Every row waits for the runtime; none failed, the slowpoke that was
    // loading included.
    let ids = ["bystander", "worker", "slowpoke", "crasher"];
    COLD.until(
        || {
            states(loader, &ids)
                .iter()
                .all(|(_, state)| *state == Some(FiberState::Pending))
        },
        "every row to wait for the runtime",
    )
    .await;
    reconcile.await.unwrap().unwrap();
    for (id, state) in states(loader, &ids) {
        assert_eq!(state, Some(FiberState::Pending), "{language:?}: {id}");
    }
    assert_eq!(fixture.runtime.state().state, FiberState::Active);
    fixture.root.shutdown().await.unwrap();
}

#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn node_a_runtime_that_exits_blames_no_other_row() {
    a_runtime_that_exits_blames_no_other_row(Language::Node).await;
}

#[cfg(feature = "python")]
#[tokio::test(flavor = "multi_thread")]
async fn python_a_runtime_that_exits_blames_no_other_row() {
    a_runtime_that_exits_blames_no_other_row(Language::Python).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_a_runtime_that_exits_blames_no_other_row() {
    a_runtime_that_exits_blames_no_other_row(Language::Bun).await;
}
