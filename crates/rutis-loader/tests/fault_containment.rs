//! guarantee: host/row-failure-output
//! guarantee: sdk/error-codes
//! One row's or one runtime's problem stays its own (#682): a row providing
//! a service the host already provides fails alone; rows of a runtime whose
//! process exits wait for it instead of failing, and the runtime says how
//! its process ended. An error a row's code leaves unhandled fails that row
//! alone where the runtime traces it, and is reported as from an unknown
//! row where it does not; the runtime's process goes on (#688).
#![cfg(feature = "runtimes")]

mod common;

use std::future::IntoFuture;
use std::path::Path;
use std::sync::Arc;

use common::runtimes::{
    greet_as_main, layer, repo, row_state, rows_stage, Probe, TestRuntime, COLD,
};
use rutis::host::{CtxHost, FiberState, FiberView};
use rutis::Ctx;
use rutis_bridge::runtime::RuntimeHandle;
use rutis_bridge::session::{host_key, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::SessionResolver;
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
    Node,
    /// Node as `rutis-host dev` runs it (`RUTIS_DEV=1`): it traces rows.
    NodeDev,
    Python,
    #[cfg(unix)]
    Bun,
}

impl Language {
    /// The row name of the plugin `stem` written in `dir`.
    fn row_name(self, dir: &Path, stem: &str) -> String {
        match self {
            Language::Node | Language::NodeDev => dir
                .join(format!("{stem}.mjs"))
                .to_string_lossy()
                .into_owned(),
            Language::Python => format!("py:{stem}"),
            #[cfg(unix)]
            Language::Bun => format!("bun:./{stem}.mjs"),
        }
    }

    /// Write the plugin `stem` into `dir`: `js` for the JavaScript
    /// runtimes (a `definePlugin` argument), `py` for Python.
    fn write(self, dir: &Path, stem: &str, js: &str, py: &str) {
        match self {
            Language::Node | Language::NodeDev => {
                std::fs::write(dir.join(format!("{stem}.mjs")), sdk_plugin(js))
            }
            Language::Python => std::fs::write(dir.join(format!("{stem}.py")), py),
            #[cfg(unix)]
            Language::Bun => std::fs::write(dir.join(format!("{stem}.mjs")), sdk_plugin(js)),
        }
        .unwrap();
    }

    /// The runtime, on `dir`.
    fn runtime(self, dir: &Path) -> TestRuntime {
        match self {
            Language::Node => TestRuntime::node(&repo().join("node/rutis-runtime/package.json")),
            Language::NodeDev => TestRuntime::node(&repo().join("node/rutis-runtime/package.json"))
                .launcher(|launcher| launcher.env("RUTIS_DEV", "1")),
            Language::Python => TestRuntime::python(dir),
            #[cfg(unix)]
            Language::Bun => {
                std::fs::write(dir.join("package.json"), "{}").unwrap();
                TestRuntime::bun(dir)
            }
        }
    }

    /// The resolver of the runtime's rows.
    fn sessions(self, catalog: &ServiceCatalog) -> SessionResolver {
        match self {
            Language::Node | Language::NodeDev => SessionResolver::npm(catalog, "node"),
            _ => SessionResolver::new(catalog),
        }
    }
}

/// A leaf plugin written with the SDK, imported from this checkout.
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
    greet_as_main(&root);
    let catalog = ServiceCatalog::new();
    let runtime = language.runtime(dir);
    let handle = runtime.handle();
    let sessions = Arc::new(language.sessions(&catalog));
    sessions.runtime(handle.clone());
    let runtime = root.plugin(runtime);
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
    rows_stage(&root, &sessions, handle.name()).await;
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

/// The error of the row `id`, once it failed, as a host prints it: its
/// text, then where it came from (the plugin's frames).
async fn failure(loader: &Loader, id: &str) -> String {
    COLD.eventually_with(
        || match loader.get(id)?.status {
            EntryStatus::Running(snapshot) if snapshot.state == FiberState::Failed => Some(
                snapshot
                    .error
                    .map(|e| rutis_bridge::report::report(&e))
                    .unwrap_or_default(),
            ),
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
        error.contains("notes is already provided by the host"),
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

#[tokio::test(flavor = "multi_thread")]
async fn node_a_clashing_provide_fails_only_its_row() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    a_clashing_provide_fails_only_its_row(Language::Node).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_a_clashing_provide_fails_only_its_row() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    a_clashing_provide_fails_only_its_row(Language::Python).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_a_clashing_provide_fails_only_its_row() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
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

/// A name rows isolate with one label is that isolate's: a row of the box
/// injects what a row of the box provides there, not the host's `notes`
/// outside; once that row has gone, the box has none and its row waits,
/// while a row outside the box has the host's all along.
/// guarantee: loader/announced-services, loader/isolate-and-inject
async fn a_name_a_row_isolates_is_seen_in_its_isolate_only(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    language.write(dir.path(), "keeper", KEEPS_NOTES_JS, KEEPS_NOTES_PY);
    language.write(dir.path(), "user", USES_NOTES_JS, USES_NOTES_PY);
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe) = (&fixture.loader, &fixture.probe);
    let boxed = |id: &str| {
        json!({
            "id": id,
            "name": language.row_name(dir.path(), id),
            "isolate": { "notes": "box" },
        })
    };
    // The same reader, outside the box.
    let outside = json!({ "id": "outsider", "name": language.row_name(dir.path(), "user") });
    let _gate = fixture
        .root
        .provide_at(host_key("gate"), Arc::new(Gate))
        .unwrap();
    loader
        .reconcile(
            layer(vec![boxed("keeper"), boxed("user"), outside.clone()]),
            None,
        )
        .await
        .unwrap();
    probe.wait_for("keeper: provided").await;
    probe.wait_for("user: kept").await;
    probe.wait_for("user: host").await;
    // The keeper goes: the box has no `notes`, and its user waits.
    loader
        .reconcile(layer(vec![boxed("user"), outside]), None)
        .await
        .unwrap();
    COLD.until(
        || row_state(loader, "user") == Some(FiberState::Pending),
        "the box's user waiting",
    )
    .await;
    assert_eq!(
        row_state(loader, "outsider"),
        Some(FiberState::Active),
        "{language:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn node_a_name_a_row_isolates_is_seen_in_its_isolate_only() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    a_name_a_row_isolates_is_seen_in_its_isolate_only(Language::Node).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_a_name_a_row_isolates_is_seen_in_its_isolate_only() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    a_name_a_row_isolates_is_seen_in_its_isolate_only(Language::Python).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_a_name_a_row_isolates_is_seen_in_its_isolate_only() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
    a_name_a_row_isolates_is_seen_in_its_isolate_only(Language::Bun).await;
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
        .expect_err("the call cannot succeed");
    // The call says the runtime went away during it (`peer-gone`); how the
    // process ended is in the runtime's reason and in the call's data.
    let runtime = match language {
        Language::Node | Language::NodeDev => "node",
        Language::Python => "py",
        #[cfg(unix)]
        Language::Bun => "bun",
    };
    assert_eq!(
        error.to_string(),
        format!("runtime {runtime} exited during the call"),
        "{language:?}"
    );
    assert_eq!(error.code(), Some("peer-gone"), "{language:?}");
    // Printed, it names the call (§8.1 row 10).
    let printed = rutis_bridge::report::report(&rutis::host::ErrorKind::PluginFailed(Box::new(
        error.clone(),
    )));
    assert_eq!(
        printed,
        format!(
            "call to work.run failed: runtime {runtime} exited during the call\n    in worker:"
        ),
        "{language:?}"
    );
    let error = error
        .data()
        .map(|data| data.to_string())
        .unwrap_or_default();
    let status = match language {
        Language::Python => exited(3),
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

#[tokio::test(flavor = "multi_thread")]
async fn node_a_runtime_that_exits_blames_no_other_row() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    a_runtime_that_exits_blames_no_other_row(Language::Node).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_a_runtime_that_exits_blames_no_other_row() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    a_runtime_that_exits_blames_no_other_row(Language::Python).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_a_runtime_that_exits_blames_no_other_row() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
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
        setImmediate(() => probe.record('reported: ' + text.trimEnd().split('\n').join(' | ')))
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
    let (exception, extension) = match language {
        Language::Python => ("ValueError", ".py"),
        _ => ("Error", ".mjs"),
    };
    // What failed, and where: the plugin's own file, among the frames
    // printed under it (#711's error structure).
    for (error, stem, what) in [
        (&early, "early", "boom while loading"),
        (&faulty, "faulty", "boom after load"),
        (&prompt, "prompt", "boom right after load"),
    ] {
        assert!(
            error
                .lines()
                .next()
                .unwrap_or_default()
                .contains(&format!("{exception}: {what}")),
            "{language:?}: {error}"
        );
        let file = format!("{stem}{extension}");
        assert!(
            error.lines().skip(1).any(|line| line.contains(&file)),
            "{language:?}: {error}"
        );
    }
    // Failed while loading: in its apply, as #711 says of a load.
    assert!(
        early.starts_with("failed in apply: "),
        "{language:?}: {early}"
    );
    let rejection = match language {
        Language::Python => "unhandled exception",
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

#[tokio::test(flavor = "multi_thread")]
async fn node_dev_an_unhandled_error_fails_only_its_row() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    an_unhandled_error_fails_only_its_row(Language::NodeDev).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_an_unhandled_error_fails_only_its_row() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    an_unhandled_error_fails_only_its_row(Language::Python).await;
}

/// Provides `lend`: `lend` leaves an error unhandled in its own code;
/// `refuse` fails as it should, for its caller to handle.
const LENDER_JS: &str = r#"{
  provides: { lend: { lend: 'async', refuse: 'async' } },
  apply(ctx) {
    ctx.provide('lend', {
      lend() { setTimeout(() => { Promise.reject(new Error('boom in lender')) }, 10); return 'lent' },
      async refuse() { throw new Error('refused by lender') },
    })
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

    async def refuse(self):
        raise ValueError("refused by lender")


provides = {"lend": Lend}


def apply(ctx, config):
    ctx.provide("lend", Lend())
"#;

/// Calls the lender's methods directly (same runtime: the object itself)
/// once it runs, and drops what `refuse` gives back.
const BORROWER_JS: &str = r#"{
  inject: ['lend', 'probe'],
  apply(ctx) {
    ctx.use('probe').record('borrower: started')
    setTimeout(() => { ctx.use('lend').lend() }, 300)
    setTimeout(() => { ctx.use('lend').refuse() }, 400)
  },
}"#;

const BORROWER_PY: &str = r#"
import asyncio

inject = ["lend", "probe"]


def apply(ctx, config):
    ctx.use("probe").record("borrower: started")
    loop = asyncio.get_running_loop()
    loop.call_later(0.3, lambda: ctx.use("lend").lend())
    loop.call_later(0.4, lambda: loop.create_task(ctx.use("lend").refuse()))
"#;

/// Records every report of an error from an unknown row, on one line (the
/// Python runtime writes to `sys.stderr`).
const TAP_PY: &str = r#"
import sys

inject = ["probe"]


class Tap:
    def __init__(self, inner, probe):
        self.inner, self.probe = inner, probe

    def write(self, text):
        if "an unhandled error from an unknown row" in text:
            self.probe.record("reported: " + text.replace("\n", " | "))
        return self.inner.write(text)

    def __getattr__(self, name):
        return getattr(self.inner, name)


def apply(ctx, config):
    probe = ctx.use("probe")
    tap = Tap(sys.stderr, probe)
    sys.stderr = tap
    ctx.effect(lambda: setattr(sys, "stderr", tap.inner))
    probe.record("witness: started")
"#;

/// An error that comes out of another row's code a row called directly,
/// in the same runtime, is charged to neither: the stack says where it was
/// thrown, not which row left it unhandled. Both a failure the lender left
/// unhandled itself and one it raised for the borrower, which dropped it,
/// are reported as from an unknown row; no row fails (#688).
/// guarantee: loader/unhandled-errors
async fn an_error_in_a_called_rows_code_is_charged_to_no_row(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    for (stem, js, py) in [
        ("witness", WITNESS_JS, TAP_PY),
        ("lender", LENDER_JS, LENDER_PY),
        ("borrower", BORROWER_JS, BORROWER_PY),
    ] {
        language.write(dir.path(), stem, js, py);
    }
    let fixture = fixture(language, dir.path()).await;
    let (loader, probe) = (&fixture.loader, &fixture.probe);
    let row = |id: &str| json!({ "id": id, "name": language.row_name(dir.path(), id) });
    loader
        .reconcile(
            layer(vec![row("witness"), row("lender"), row("borrower")]),
            None,
        )
        .await
        .unwrap();
    probe.wait_for("borrower: started").await;
    let reported = COLD
        .eventually(
            || {
                let lines: Vec<String> = probe
                    .lines()
                    .into_iter()
                    .filter(|line| line.starts_with("reported: "))
                    .collect();
                (lines.len() >= 2).then_some(lines)
            },
            "both errors to be reported",
        )
        .await;
    let all = reported.join("\n");
    assert!(
        all.contains("boom in lender") && all.contains("refused by lender"),
        "{language:?}: {all}"
    );
    // Only the plugins' own frames are listed (#677, §4.5).
    assert!(all.contains("lender."), "{language:?}: {all}");
    for not_the_plugins in [
        "node:internal",
        "(node:",
        "rutis-runtime/src/",
        "rutis/runner.py",
        "/asyncio/",
    ] {
        assert!(
            !all.contains(not_the_plugins),
            "{language:?}: {not_the_plugins} in {all}"
        );
    }
    for id in ["witness", "lender", "borrower"] {
        assert_eq!(
            row_state(loader, id),
            Some(FiberState::Active),
            "{language:?}: {id}"
        );
    }
    fixture.root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn node_dev_an_error_in_a_called_rows_code_is_charged_to_no_row() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    an_error_in_a_called_rows_code_is_charged_to_no_row(Language::NodeDev).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_an_error_in_a_called_rows_code_is_charged_to_no_row() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    an_error_in_a_called_rows_code_is_charged_to_no_row(Language::Python).await;
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
    // Only the plugin's own frames are listed (#677, §4.5): its file, not
    // the JavaScript engine's or the runtime's.
    assert!(reported.contains("faulty.mjs"), "{language:?}: {reported}");
    for not_the_plugins in [
        "node:internal",
        "(node:",
        "native:",
        "rutis-runtime/src/",
        "rutis-bun/src/",
    ] {
        assert!(
            !reported.contains(not_the_plugins),
            "{language:?}: {not_the_plugins} in {reported}"
        );
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn node_an_unhandled_error_from_an_unknown_row_is_reported() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    an_unhandled_error_from_an_unknown_row_is_reported(Language::Node).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_an_unhandled_error_from_an_unknown_row_is_reported() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
    an_unhandled_error_from_an_unknown_row_is_reported(Language::Bun).await;
}
