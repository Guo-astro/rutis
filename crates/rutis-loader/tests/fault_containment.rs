//! One row's or one runtime's problem stays its own (#682): a row providing
//! a service the host already provides fails alone; rows of a runtime whose
//! process exits wait for it instead of failing, and the runtime says how
//! its process ended. An error a row's code leaves unhandled fails that row
//! alone where the runtime traces it, and is reported as from an unknown
//! row where it does not; the runtime's process goes on (#688).
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
    /// Node as `rutis-host dev` runs it (`RUTIS_DEV=1`): it traces rows.
    #[cfg(feature = "node")]
    NodeDev,
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
            Language::Node | Language::NodeDev => dir
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
            Language::Node | Language::NodeDev => {
                std::fs::write(dir.join(format!("{stem}.mjs")), sdk_plugin(js))
            }
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
            #[cfg(feature = "node")]
            Language::NodeDev => {
                let package = repo().join("node/rutis-runtime");
                let launcher =
                    rutis_bridge::runtime::Launcher::node(&package).env("RUTIS_DEV", "1");
                let runtime =
                    LocalRuntime::launcher("node", launcher, package.join("package.json"));
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

/// Makes its runtime exit once the slowpoke loads: `process.exit`
/// (JavaScript), `os._exit` (Python). (An error nobody handles no longer
/// ends a runtime, #688.)
const CRASHER_JS: &str = r#"{
  inject: ['loading', 'probe'],
  apply(ctx) {
    ctx.use('probe').record('crasher: started')
    setTimeout(() => { process.exit(1) }, 200)
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
    // Its service reaches the host after its `apply` has returned, a moment
    // after the line it prints there.
    COLD.until(
        || root.get_at::<dyn HostDispatch>(&host_key("work")).is_some(),
        "the worker's service",
    )
    .await;
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

// ── An error a row's code leaves unhandled (#688) ───────────────

/// Runs on, and records the first line of every report of an error from an
/// unknown row the runtime writes to its standard error.
const WITNESS_JS: &str = r#"{
  inject: ['probe'],
  apply(ctx) {
    const probe = ctx.use('probe')
    const write = process.stderr.write
    process.stderr.write = function (chunk, ...rest) {
      const text = String(chunk)
      if (text.includes('an unhandled error from an unknown row')) {
        setImmediate(() => probe.record('reported: ' + text.split('\n')[0]))
      }
      return write.call(this, chunk, ...rest)
    }
    ctx.effect(() => { process.stderr.write = write })
    probe.record('witness: started')
  },
}"#;

const WITNESS_PY: &str = r#"
inject = ["probe"]


def apply(ctx, config):
    ctx.use("probe").record("witness: started")
"#;

/// Leaves a promise rejection (an exception in a task) unhandled once it
/// has loaded.
const FAULTY_JS: &str = r#"{
  inject: ['probe'],
  apply(ctx) {
    ctx.use('probe').record('faulty: started')
    setTimeout(() => { Promise.reject(new Error('boom after load')) }, 300)
  },
}"#;

const FAULTY_PY: &str = r#"
import asyncio

inject = ["probe"]


async def boom():
    await asyncio.sleep(0.3)
    raise ValueError("boom after load")


def apply(ctx, config):
    ctx.use("probe").record("faulty: started")
    asyncio.get_running_loop().create_task(boom())
"#;

/// Leaves an exception unhandled while it loads (a timer's, a task's).
const EARLY_JS: &str = r#"{
  inject: ['probe'],
  async apply(ctx) {
    ctx.use('probe').record('early: loading')
    setTimeout(() => { throw new Error('boom while loading') }, 50)
    await new Promise(resolve => setTimeout(resolve, 10000))
  },
}"#;

const EARLY_PY: &str = r#"
import asyncio

inject = ["probe"]


async def boom():
    raise ValueError("boom while loading")


async def apply(ctx, config):
    ctx.use("probe").record("early: loading")
    asyncio.get_running_loop().create_task(boom())
    await asyncio.sleep(10)
"#;

/// Leaves an error unhandled at once after its apply: the moment the
/// runtime answers the load, or right after.
const PROMPT_JS: &str = r#"{
  inject: ['probe'],
  apply(ctx) {
    ctx.use('probe').record('prompt: started')
    setTimeout(() => { Promise.reject(new Error('boom right after load')) }, 0)
  },
}"#;

const PROMPT_PY: &str = r#"
import asyncio

inject = ["probe"]


async def boom():
    raise ValueError("boom right after load")


def apply(ctx, config):
    ctx.use("probe").record("prompt: started")
    loop = asyncio.get_running_loop()
    loop.call_soon(lambda: loop.create_task(boom()))
"#;

/// Where the runtime traces rows (Node under `dev`, Python): an error a
/// row's code leaves unhandled fails that row, saying what and where, also
/// while it loads and right after; the other rows of the runtime run on,
/// and so does its process (#688).
/// guarantee: loader/unhandled-errors
async fn an_unhandled_error_fails_only_its_row(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    for (stem, js, py) in [
        ("witness", WITNESS_JS, WITNESS_PY),
        ("faulty", FAULTY_JS, FAULTY_PY),
        ("early", EARLY_JS, EARLY_PY),
        ("prompt", PROMPT_JS, PROMPT_PY),
    ] {
        language.write(dir.path(), stem, js, py);
    }
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe) = (&fixture.loader, &fixture.probe);
    let row = |id: &str| json!({ "id": id, "name": language.row_name(dir.path(), id) });
    let reconcile = tokio::spawn({
        let loader = loader.clone();
        let rows = vec![row("witness"), row("faulty"), row("early"), row("prompt")];
        async move { loader.reconcile(layer(rows), None).await }
    });
    let early = failure(loader, "early").await;
    let faulty = failure(loader, "faulty").await;
    let prompt = failure(loader, "prompt").await;
    reconcile.await.unwrap().unwrap();
    let (exception, file) = match language {
        #[cfg(feature = "python")]
        Language::Python => ("ValueError", ".py:"),
        #[allow(unreachable_patterns)]
        _ => ("Error", ".mjs:"),
    };
    // What failed and where: the plugin's own file.
    for (error, what) in [
        (&early, "boom while loading"),
        (&faulty, "boom after load"),
        (&prompt, "boom right after load"),
    ] {
        assert!(
            error.contains(&format!("{exception}: {what}")),
            "{language:?}: {error}"
        );
        assert!(
            error.contains("(at ") && error.contains(file),
            "{language:?}: {error}"
        );
    }
    let rejection = match language {
        #[cfg(feature = "python")]
        Language::Python => "unhandled exception",
        #[allow(unreachable_patterns)]
        _ => "unhandled rejection",
    };
    assert!(faulty.contains(rejection), "{language:?}: {faulty}");
    // The others run on, in a runtime that is up.
    assert!(probe.has("witness: started"));
    assert_eq!(row_state(loader, "witness"), Some(FiberState::Active));
    assert!(
        matches!(
            fixture.handle.state(),
            rutis_bridge::runtime::RuntimeState::Ready(_)
        ),
        "{language:?}: the runtime is up"
    );
    assert!(
        !probe
            .lines()
            .iter()
            .any(|line| line.starts_with("reported: ")),
        "{:?}",
        probe.lines()
    );
    fixture.root.shutdown().await.unwrap();
}

#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn node_dev_an_unhandled_error_fails_only_its_row() {
    an_unhandled_error_fails_only_its_row(Language::NodeDev).await;
}

#[cfg(feature = "python")]
#[tokio::test(flavor = "multi_thread")]
async fn python_an_unhandled_error_fails_only_its_row() {
    an_unhandled_error_fails_only_its_row(Language::Python).await;
}

/// Provides `lend`, whose `lend` leaves an error unhandled in its own code.
const LENDER_JS: &str = r#"{
  provides: { lend: { lend: 'async' } },
  apply(ctx) {
    ctx.provide('lend', { lend() { setTimeout(() => { Promise.reject(new Error('boom in lender')) }, 10); return 'lent' } })
  },
}"#;

const LENDER_PY: &str = r#"
import asyncio


class Lend:
    def lend(self):
        async def boom():
            raise ValueError("boom in lender")

        asyncio.get_running_loop().create_task(boom())
        return "lent"


provides = {"lend": Lend}


def apply(ctx, config):
    ctx.provide("lend", Lend())
"#;

/// Calls the lender's `lend` directly (same runtime: the object itself),
/// once it runs.
const BORROWER_JS: &str = r#"{
  inject: ['lend', 'probe'],
  apply(ctx) {
    ctx.use('probe').record('borrower: started')
    setTimeout(() => { ctx.use('lend').lend() }, 300)
  },
}"#;

const BORROWER_PY: &str = r#"
import asyncio

inject = ["lend", "probe"]


def apply(ctx, config):
    ctx.use("probe").record("borrower: started")
    asyncio.get_running_loop().call_later(0.3, lambda: ctx.use("lend").lend())
"#;

/// An error left unhandled in one row's code that another row of the same
/// runtime called directly is charged to the row whose code it is, not to
/// the caller (#688).
/// guarantee: loader/unhandled-errors
async fn an_error_in_a_called_rows_code_is_charged_to_that_row(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    for (stem, js, py) in [
        ("lender", LENDER_JS, LENDER_PY),
        ("borrower", BORROWER_JS, BORROWER_PY),
    ] {
        language.write(dir.path(), stem, js, py);
    }
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe) = (&fixture.loader, &fixture.probe);
    let row = |id: &str| json!({ "id": id, "name": language.row_name(dir.path(), id) });
    let reconcile = tokio::spawn({
        let loader = loader.clone();
        let rows = vec![row("lender"), row("borrower")];
        async move { loader.reconcile(layer(rows), None).await }
    });
    probe.wait_for("borrower: started").await;
    let lender = failure(loader, "lender").await;
    reconcile.await.unwrap().unwrap();
    assert!(lender.contains("boom in lender"), "{language:?}: {lender}");
    // The borrower waits for the service it uses; it did not fail.
    COLD.until(
        || row_state(loader, "borrower") == Some(FiberState::Pending),
        "the borrower to wait for lend",
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn node_dev_an_error_in_a_called_rows_code_is_charged_to_that_row() {
    an_error_in_a_called_rows_code_is_charged_to_that_row(Language::NodeDev).await;
}

#[cfg(feature = "python")]
#[tokio::test(flavor = "multi_thread")]
async fn python_an_error_in_a_called_rows_code_is_charged_to_that_row() {
    an_error_in_a_called_rows_code_is_charged_to_that_row(Language::Python).await;
}

/// Where the runtime does not trace rows (Node outside `dev`, Bun): an
/// error a row's code leaves unhandled is reported as from an unknown row;
/// no row fails, and the runtime's process goes on (#688).
/// guarantee: loader/unhandled-errors
async fn an_unhandled_error_from_an_unknown_row_is_reported(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    for (stem, js, py) in [
        ("witness", WITNESS_JS, WITNESS_PY),
        ("faulty", FAULTY_JS, FAULTY_PY),
    ] {
        language.write(dir.path(), stem, js, py);
    }
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe) = (&fixture.loader, &fixture.probe);
    let row = |id: &str| json!({ "id": id, "name": language.row_name(dir.path(), id) });
    loader
        .reconcile(layer(vec![row("witness"), row("faulty")]), None)
        .await
        .unwrap();
    probe.wait_for("faulty: started").await;
    let reported = COLD
        .eventually(
            || {
                probe
                    .lines()
                    .into_iter()
                    .find(|line| line.starts_with("reported: "))
            },
            "the runtime to report the error",
        )
        .await;
    assert!(
        reported.contains(": an unhandled error from an unknown row: Error: boom after load"),
        "{language:?}: {reported}"
    );
    // Nothing failed, and the runtime is up.
    for id in ["witness", "faulty"] {
        assert_eq!(
            row_state(loader, id),
            Some(FiberState::Active),
            "{language:?}: {id}"
        );
    }
    assert!(
        matches!(
            fixture.handle.state(),
            rutis_bridge::runtime::RuntimeState::Ready(_)
        ),
        "{language:?}: the runtime is up"
    );
    fixture.root.shutdown().await.unwrap();
}

#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn node_an_unhandled_error_from_an_unknown_row_is_reported() {
    an_unhandled_error_from_an_unknown_row_is_reported(Language::Node).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_an_unhandled_error_from_an_unknown_row_is_reported() {
    an_unhandled_error_from_an_unknown_row_is_reported(Language::Bun).await;
}
