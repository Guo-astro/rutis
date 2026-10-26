//! JavaScript plugins as loader rows (P6): one shared Cordis Context,
//! services resolved between rows natively, per-row load/update/unload,
//! isolate and inject forwarded, schemastery schema exported; services
//! shared with rutis by name (multilanguage M1).
#![cfg(feature = "node")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::runtimes::{layer, row_state, Probe};
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, FiberView, TypeKey,
};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{settle, Reply, Value as RpcValue};
use rutis_loader::report::{EntryStatus, LoaderChanged, RowStatus, Supply};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRows, RuntimeRowsPlugin};
use rutis_loader::{
    Chain, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog,
};
use rutis_test_support::{until, within};
use serde_json::{json, Value};

const PROVIDER: &str = r#"
export const name = 'provider'
// A schemastery-shaped schema, with the standard-schema hook Cordis calls.
export const Config = {
  type: 'object', meta: {},
  dict: {
    who: { type: 'string', meta: { default: 'world', description: 'who to greet' } },
    level: { type: 'number', meta: { default: 1, volatile: true } },
  },
  '~standard': { validate: value => ({ value }) },
}
export function apply(ctx, config) {
  ctx.provide('greeter', { hello: () => `hello ${config.who}` })
}
"#;

const CONSUMER: &str = r#"
export const name = 'consumer'
export const inject = ['greeter', 'probe']
export function apply(ctx, config) {
  ctx.probe.record(`${config.tag}: ${ctx.greeter.hello()}`)
  ctx.effect(() => () => ctx.probe.record(`${config.tag}: bye`))
}
"#;

const FLAG: &str = r#"
export const name = 'flag'
export function apply(ctx) { ctx.provide('late', { on: true }) }
"#;

fn node_package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime")
}

fn row(id: &str, file: &Path, config: Value, extra: Value) -> Value {
    let mut row = json!({ "id": id, "name": file.to_string_lossy(), "config": config });
    if let Value::Object(extra) = extra {
        row.as_object_mut().unwrap().extend(extra);
    }
    row
}

#[tokio::test(flavor = "multi_thread")]
async fn javascript_rows() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let provider = write("provider.mjs", PROVIDER);
    let consumer = write("consumer.mjs", CONSUMER);
    let flag = write("flag.mjs", FLAG);

    let probe = Probe::default();
    let (root, loader, _runtime) = interop_loader(&probe).await;

    let layer = |rows: Vec<Value>| -> Vec<Layer> {
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
        vec![Layer::new("rows", patches)]
    };
    let base = vec![
        row("p", &provider, json!({ "who": "rust" }), json!(null)),
        row("c", &consumer, json!({ "tag": "c" }), json!(null)),
        // A separate scope for `greeter`: its own provider and consumer.
        row(
            "p2",
            &provider,
            json!({ "who": "boxed" }),
            json!({ "isolate": { "greeter": "box" } }),
        ),
        row(
            "c2",
            &consumer,
            json!({ "tag": "c2" }),
            json!({ "isolate": { "greeter": "box" } }),
        ),
        // An empty private scope: the consumer finds no greeter.
        row(
            "c3",
            &consumer,
            json!({ "tag": "c3" }),
            json!({ "isolate": { "greeter": true } }),
        ),
        // Waits for `late`, which no row provides yet.
        row(
            "c4",
            &consumer,
            json!({ "tag": "c4" }),
            json!({ "inject": ["late"] }),
        ),
    ];
    let report = loader.reconcile(layer(base.clone()), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    probe.wait_for("c: hello rust").await;
    probe.wait_for("c2: hello boxed").await;
    // A row runs in rutis once its `rows.load` returned, and the runner
    // answers that only after the Cordis fiber settled (`loadRow` awaits
    // `fiber.await()`): applied, or waiting for its inject. `record` is a
    // sync call, so a start of c3 or c4 is recorded before their rows run.
    until(
        || {
            row_state(&loader, "c3") == Some(FiberState::Active)
                && row_state(&loader, "c4") == Some(FiberState::Active)
        },
        "c3 and c4 loaded in Cordis",
    )
    .await;
    let lines = probe.take();
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("c3") || l.starts_with("c4")),
        "{lines:?}"
    );

    // The schemastery schema arrives as JSON Schema.
    let schema = loader.get("p").unwrap().schema.unwrap();
    assert_eq!(schema["properties"]["who"]["default"], "world");
    assert_eq!(schema["properties"]["level"]["x-volatile"], true);

    // A config update reloads the provider; Cordis reloads its consumer.
    let mut updated = base.clone();
    updated[0] = row("p", &provider, json!({ "who": "there" }), json!(null));
    // `late` arrives: the gated consumer starts.
    updated.push(row("f", &flag, json!({}), json!(null)));
    loader
        .reconcile(layer(updated.clone()), None)
        .await
        .unwrap();
    probe.wait_for("c: hello there").await;
    probe.wait_for("c4: hello there").await;
    assert!(probe.take().contains(&"c: bye".to_owned()));

    // Removing a row disposes it on the Cordis side.
    updated.retain(|r| r["id"] != "c");
    loader.reconcile(layer(updated), None).await.unwrap();
    probe.wait_for("c: bye").await;

    // A name that is no package here does not resolve.
    loader
        .reconcile(
            layer(vec![json!({ "id": "x", "name": "no-such-package" })]),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        loader.get("x").unwrap().status,
        EntryStatus::Unresolved(LoaderError::NotFound { .. })
    ));

    root.shutdown().await.unwrap();
}

/// `level` is a volatile reference, as schemastery's `meta.volatile` makes it.
const TUNABLE: &str = r#"
import { createVolatile } from 'COSMOKIT'
export const name = 'tunable'
export const inject = ['probe']
export const Config = {
  type: 'object', meta: {},
  dict: {
    tag: { type: 'string', meta: {} },
    level: { type: 'number', meta: { default: 1, volatile: true } },
  },
  '~standard': { validate: value => ({ value: { ...value, level: createVolatile(value.level ?? 1) } }) },
}
export function apply(ctx, config) {
  ctx.probe.record(`${config.tag}: start ${config.level.get()}`)
  ctx.on('loader/volatile-update', paths => {
    ctx.probe.record(`${config.tag}: ${JSON.stringify(paths)} ${config.level.get()}`)
  })
  ctx.effect(() => () => ctx.probe.record(`${config.tag}: bye`))
}
"#;

/// guarantee: loader/volatile-in-place
#[tokio::test(flavor = "multi_thread")]
async fn volatile_changes_reach_cordis_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let cosmokit = node_package()
        .join("node_modules/@deepseek-ai/cosmokit/lib/index.js")
        .canonicalize()
        .unwrap();
    let tunable = dir.path().join("tunable.mjs");
    std::fs::write(
        &tunable,
        TUNABLE.replace(
            "COSMOKIT",
            url::Url::from_file_path(&cosmokit).unwrap().as_str(),
        ),
    )
    .unwrap();

    let probe = Probe::default();
    let (root, loader, _runtime) = interop_loader(&probe).await;
    let layer = |level: u32| -> Vec<Layer> {
        let rows = vec![
            row(
                "t",
                &tunable,
                json!({ "tag": "t", "level": level }),
                json!(null),
            ),
            // Behind an inject gate, the plugin runs one fiber deeper.
            row(
                "g",
                &tunable,
                json!({ "tag": "g", "level": level }),
                json!({ "inject": ["probe"] }),
            ),
        ];
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
        vec![Layer::new("rows", patches)]
    };

    let report = loader.reconcile(layer(1), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    probe.wait_for("t: start 1").await;
    probe.wait_for("g: start 1").await;
    assert_eq!(
        loader.get("t").unwrap().schema.unwrap()["properties"]["level"]["x-volatile"],
        true
    );
    let fibers = (
        loader.get("t").unwrap().plugin,
        loader.get("g").unwrap().plugin,
    );
    probe.take();

    loader.reconcile(layer(5), None).await.unwrap();
    probe.wait_for(r#"t: [["level"]] 5"#).await;
    probe.wait_for(r#"g: [["level"]] 5"#).await;
    // The next change of the same rows as the marker: a row's volatile
    // updates are one event key, dispatched in order, each listener awaited
    // (D31), and the forward awaits `rows.update`, which the runner answers
    // only after a restart it made settled. So once level 6 arrived in
    // place, whatever level 5 did is recorded.
    loader.reconcile(layer(6), None).await.unwrap();
    probe.wait_for(r#"t: [["level"]] 6"#).await;
    probe.wait_for(r#"g: [["level"]] 6"#).await;
    let lines = probe.take();
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("bye") || l.contains("start")),
        "{lines:?}"
    );
    assert_eq!(
        (
            loader.get("t").unwrap().plugin,
            loader.get("g").unwrap().plugin
        ),
        fibers,
        "no restart on the rutis side either"
    );

    root.shutdown().await.unwrap();
}

/// Same schema, but the parsed config holds plain values: nothing to commit
/// into, so a volatile change must take an ordinary update.
const PLAIN: &str = r#"
export const name = 'plain'
export const inject = ['probe']
export const Config = {
  type: 'object', meta: {},
  dict: {
    tag: { type: 'string', meta: {} },
    level: { type: 'number', meta: { default: 1, volatile: true } },
  },
  '~standard': { validate: value => ({ value }) },
}
export function apply(ctx, config) {
  ctx.probe.record(`${config.tag}: start ${config.level}`)
}
"#;

/// The probe host, the Cordis runtime, then the loader with its rows.
async fn interop_loader(probe: &Probe) -> (Ctx, Loader, FiberView) {
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let (loader, runtime) = mount(&root, node_package(), &["probe"]).await;
    (&runtime).await.unwrap();
    (root, loader, runtime)
}

/// The runtime, the loader and the rows' second stage, with `shared` as
/// the catalog's shared names. Waits for the loader only.
async fn mount(root: &Ctx, package: PathBuf, shared: &[&str]) -> (Loader, FiberView) {
    let mut catalog = ServiceCatalog::new();
    for name in shared {
        catalog.register_shared(*name);
    }
    let runtime = LocalRuntime::node(package, node_package().join("package.json"))
        .host("probe", json!({ "record": "sync" }));
    let resolver = Arc::new(RuntimeResolver::node(runtime.handle()).with_catalog(&catalog));
    let runtime = root.plugin(runtime);
    let options = LoaderOptions {
        catalog,
        ..LoaderOptions::default()
    };
    let plugin = LoaderPlugin::new(Chain::new().with_shared(resolver.clone()), options);
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(resolver));
    (loader, runtime)
}

/// guarantee: loader/volatile-in-place
#[tokio::test(flavor = "multi_thread")]
async fn volatile_changes_are_never_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let cosmokit = node_package()
        .join("node_modules/@deepseek-ai/cosmokit/lib/index.js")
        .canonicalize()
        .unwrap();
    let write = |name: &str, text: String| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    // Waits on its own inject for `late`.
    let waiting = write(
        "waiting.mjs",
        TUNABLE
            .replace(
                "COSMOKIT",
                url::Url::from_file_path(&cosmokit).unwrap().as_str(),
            )
            .replace("['probe']", "['probe', 'late']"),
    );
    let plain = write("plain.mjs", PLAIN.to_owned());
    let flag = write("flag.mjs", FLAG.to_owned());

    let probe = Probe::default();
    let (root, loader, _runtime) = interop_loader(&probe).await;
    let layer = |level: u32, late: bool| -> Vec<Layer> {
        let mut rows = vec![
            row(
                "w",
                &waiting,
                json!({ "tag": "w", "level": level }),
                json!(null),
            ),
            row(
                "p",
                &plain,
                json!({ "tag": "p", "level": level }),
                json!(null),
            ),
        ];
        if late {
            rows.push(row("f", &flag, json!({}), json!(null)));
        }
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
        vec![Layer::new("rows", patches)]
    };

    let report = loader.reconcile(layer(1, false), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    probe.wait_for("p: start 1").await;

    // No volatile reference to commit into: the plain row restarts with it.
    loader.reconcile(layer(5, false), None).await.unwrap();
    probe.wait_for("p: start 5").await;
    // The waiting row got the update while pending (sent before p's, on the
    // same connection); once `late` arrives it starts from it.
    loader.reconcile(layer(5, true), None).await.unwrap();
    probe.wait_for("w: start 5").await;
    let lines = probe.take();
    assert!(!lines.contains(&"w: start 1".to_owned()), "{lines:?}");

    root.shutdown().await.unwrap();
}

// ── The runtime is a plugin ─────────────────────────────────────

const EXIT: &str = r#"
export const name = 'exit'
export function apply() { setTimeout(() => process.exit(17), 200) }
"#;

fn write_plugins(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let write = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    (
        write("provider.mjs", PROVIDER),
        write("consumer.mjs", CONSUMER),
        write("exit.mjs", EXIT),
    )
}

/// guarantee: loader/row-unload-leaves-nothing
#[tokio::test(flavor = "multi_thread")]
async fn rows_unload_before_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, consumer, _) = write_plugins(dir.path());
    let probe = Probe::default();
    let (root, loader, runtime) = interop_loader(&probe).await;
    let base = vec![
        row("p", &provider, json!({ "who": "rust" }), json!(null)),
        row("c", &consumer, json!({ "tag": "c" }), json!(null)),
    ];
    loader.reconcile(layer(base), None).await.unwrap();
    probe.wait_for("c: hello rust").await;

    // The row's own cleanup still reaches Cordis: the process outlives it.
    runtime.dispose().await.unwrap();
    probe.wait_for("c: bye").await;
    until(
        || {
            row_state(&loader, "c") == Some(FiberState::Pending)
                && row_state(&loader, "p") == Some(FiberState::Pending)
        },
        "rows waiting for the runtime",
    )
    .await;

    root.shutdown().await.unwrap();
}

/// guarantee: loader/runtime-down-stops-its-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_process_stops_the_rows_until_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, consumer, exit) = write_plugins(dir.path());
    let probe = Probe::default();
    let (root, loader, runtime) = interop_loader(&probe).await;
    let base = vec![
        row("p", &provider, json!({ "who": "rust" }), json!(null)),
        row("c", &consumer, json!({ "tag": "c" }), json!(null)),
    ];
    loader.reconcile(layer(base.clone()), None).await.unwrap();
    probe.wait_for("c: hello rust").await;
    probe.take();

    let mut dying = base.clone();
    dying.push(row("x", &exit, json!({}), json!(null)));
    loader.reconcile(layer(dying), None).await.unwrap();
    // The runtime withdraws its service; rows wait instead of holding a
    // dead process, and the runtime itself stays up for a restart.
    until(
        || {
            ["p", "c", "x"]
                .iter()
                .all(|id| row_state(&loader, id) == Some(FiberState::Pending))
        },
        "rows waiting after the crash",
    )
    .await;
    assert_eq!(runtime.state().state, FiberState::Active);
    let runtime_key = RuntimeRows::key("node");
    let waiting = root
        .diagnostics()
        .plugins
        .into_iter()
        .find(|plugin| plugin.name.ends_with("consumer.mjs"))
        .unwrap();
    assert_eq!(waiting.state, FiberState::Pending);
    assert!(
        waiting.injects.iter().any(|dep| dep.key == runtime_key),
        "the row shows it waits for the runtime: {:?}",
        waiting.injects
    );

    loader.reconcile(layer(base), None).await.unwrap();
    runtime.restart().await.unwrap();
    probe.wait_for("c: hello rust").await;
    until(
        || row_state(&loader, "c") == Some(FiberState::Active),
        "rows running again",
    )
    .await;

    root.shutdown().await.unwrap();
}

/// guarantee: loader/runtimes-do-not-wait-for-each-other
#[tokio::test(flavor = "multi_thread")]
async fn rows_wait_for_their_hosts_not_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, consumer, _) = write_plugins(dir.path());
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    let (loader, runtime) = mount(&root, node_package(), &["probe"]).await;
    (&runtime).await.unwrap();

    let base = vec![
        row("p", &provider, json!({ "who": "rust" }), json!(null)),
        row("c", &consumer, json!({ "tag": "c" }), json!(null)),
    ];
    let report = loader.reconcile(layer(base), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    // The consumer injects `probe`, which nobody provides yet: only it waits.
    until(
        || row_state(&loader, "p") == Some(FiberState::Active),
        "the provider to run",
    )
    .await;
    assert_eq!(row_state(&loader, "c"), Some(FiberState::Pending));
    assert_eq!(runtime.state().state, FiberState::Active);

    let host = root
        .provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    probe.wait_for("c: hello rust").await;

    // The host goes: its user stops; the runtime and the provider stay.
    host.dispose().await.unwrap();
    probe.wait_for("c: bye").await;
    until(
        || row_state(&loader, "c") == Some(FiberState::Pending),
        "the consumer to wait for the host",
    )
    .await;
    assert_eq!(row_state(&loader, "p"), Some(FiberState::Active));
    assert_eq!(runtime.state().state, FiberState::Active);

    root.shutdown().await.unwrap();
}

/// `greeter` served from Rust, with the methods it declares itself.
struct RustGreeter;

impl HostDispatch for RustGreeter {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "hello");
        Ok(json!("hello from rust").into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "hello": "sync" }))
    }
}

/// Resolved before the runtime runs, a row learns what its plugin injects
/// only once the runtime is up; it must not start before that service.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn rows_resolved_offline_wait_for_what_they_inject() {
    let dir = tempfile::tempdir().unwrap();
    let (_, consumer, _) = write_plugins(dir.path());
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    let transitions = common::Transitions::on(&root);
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe").register_shared("greeter");
    let runtime = LocalRuntime::node(node_package(), node_package().join("package.json"))
        .host("probe", json!({ "record": "sync" }));
    let resolver = Arc::new(RuntimeResolver::node(runtime.handle()).with_catalog(&catalog));
    let options = LoaderOptions {
        catalog,
        ..LoaderOptions::default()
    };
    let plugin = LoaderPlugin::new(Chain::new().with_shared(resolver.clone()), options);
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(resolver));

    // No runtime yet: the row resolves without its declarations.
    let base = vec![row("c", &consumer, json!({ "tag": "c" }), json!(null))];
    loader.reconcile(layer(base), None).await.unwrap();
    assert!(loader.get("c").unwrap().meta["schema"].is_string());
    let offline = loader.get("c").unwrap().plugin.unwrap();

    // The runtime starts: the row is resolved again before it may start,
    // and now also waits for the shared `greeter`, which nobody provides.
    // Its new injects replace its fiber: the new resolution shows first,
    // then the old fiber goes and, once it is gone, the new one comes. So
    // wait for the new fiber, not for the injects (#316).
    root.plugin(runtime).await.unwrap();
    until(
        || {
            loader.get("c").is_some_and(|entry| {
                entry.meta["inject"].is_array()
                    && entry.plugin.is_some_and(|plugin| plugin != offline)
            })
        },
        "the row to run again with its injects",
    )
    .await;
    // The state now; that it stays so until `greeter` comes, `loads` checks below.
    assert_eq!(row_state(&loader, "c"), Some(FiberState::Pending));
    assert!(probe.take().is_empty());

    // `greeter` from Rust: the JavaScript consumer calls it through Cordis.
    let greeter = root
        .provide_at(host_key("greeter"), Arc::new(RustGreeter))
        .unwrap();
    probe.wait_for("c: hello from rust").await;
    // No start before that: the fiber resolved offline went without ever
    // loading, and the one resolved again loaded once, for `greeter`. Had
    // either started early, its Loading came before the changes waited
    // for here (one fiber's changes arrive in order).
    let resolved = loader.get("c").unwrap().plugin.unwrap();
    let fibers = [offline, resolved];
    assert_eq!(
        transitions.loads(&fibers).await,
        1,
        "{:?}",
        fibers.map(|fiber| transitions.of(fiber))
    );
    greeter.dispose().await.unwrap();
    probe.wait_for("c: bye").await;

    root.shutdown().await.unwrap();
}

/// guarantee: loader/runtime-start-failure-isolated
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_cannot_start_does_not_block_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, _, _) = write_plugins(dir.path());
    let root = Ctx::root().unwrap();
    // No Node runtime here: the mount fails.
    let runtime = LocalRuntime::node(dir.path(), node_package().join("package.json"));
    let resolver = RuntimeResolver::node(runtime.handle());
    let runtime = root.plugin(runtime);
    let _ = (&runtime).await;
    assert_eq!(runtime.state().state, FiberState::Failed);
    let plugin = LoaderPlugin::new(Chain::new().with(resolver), LoaderOptions::default());
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();

    let report = within(
        "resolution, which does not wait for a failed runtime",
        loader.reconcile(
            layer(vec![row("p", &provider, json!({}), json!(null))]),
            None,
        ),
    )
    .await
    .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(row_state(&loader, "p"), Some(FiberState::Pending));
    assert!(loader.get("p").unwrap().meta["schema"].is_string());

    root.shutdown().await.unwrap();
}

// ── Unloading a runtime that is still starting ──────────────────

/// A runtime package whose runner records its PID and never connects.
#[cfg(unix)]
fn hanging_runtime(dir: &Path) -> PathBuf {
    let package = dir.join("hanging");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::os::unix::fs::symlink(
        node_package().join("node_modules").canonicalize().unwrap(),
        package.join("node_modules"),
    )
    .unwrap();
    std::fs::write(
        package.join("src/runner.mjs"),
        "import { appendFileSync } from 'node:fs'\n\
         appendFileSync('pids', `${process.pid}\\n`)\n\
         setInterval(() => {}, 1000)\n",
    )
    .unwrap();
    package
}

#[cfg(unix)]
fn pids(package: &Path) -> Vec<i32> {
    std::fs::read_to_string(package.join("pids"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.parse().unwrap())
        .collect()
}

/// Gone, or a zombie waiting to be reaped: it no longer runs.
#[cfg(unix)]
fn stopped(pid: i32) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    stat.trim().is_empty() || stat.trim().starts_with('Z')
}

#[cfg(unix)]
async fn starting(runtime: &FiberView, package: &Path, count: usize) {
    until(
        || pids(package).len() >= count && runtime.state().state == FiberState::Loading,
        "the runner to start",
    )
    .await;
}

/// Watches the runner processes with `ps`.
/// guarantee: loader/runtime-start-failure-isolated
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_starting_runtime_can_be_disposed_or_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let package = hanging_runtime(dir.path());
    let root = Ctx::root().unwrap();
    let runtime = LocalRuntime::node(&package, node_package().join("package.json"));
    let handle = runtime.handle();
    let runtime = root.plugin(runtime);

    // Restart: the hanging start is abandoned and its process killed.
    starting(&runtime, &package, 1).await;
    let restart = tokio::spawn(runtime.restart());
    starting(&runtime, &package, 2).await;
    let first = pids(&package)[0];
    until(|| stopped(first), "the first runner to stop").await;

    restart.abort();

    // Dispose while starting.
    within(
        "dispose, which does not wait for the hanging start",
        runtime.dispose(),
    )
    .await
    .unwrap();
    let second = pids(&package)[1];
    until(|| stopped(second), "the second runner to stop").await;
    within("the handle to settle", handle.ready()).await;

    root.shutdown().await.unwrap();
}

// ── Services shared by name ─────────────────────────────────────

const WEATHER: &str = r#"
export const name = 'weather'
export function apply(ctx) {
  ctx.provide('weather', {
    today() { return 'sunny' },
    // Not declared in `rutis.provides`: only a native user can call it.
    secret() { return 'native' },
  })
}
"#;

const FORECASTER: &str = r#"
export const name = 'forecaster'
export const inject = ['weather', 'probe']
export function apply(ctx) {
  ctx.probe.record(`js: ${ctx.weather.today()} ${ctx.weather.secret()}`)
}
"#;

fn weather_package(dir: &Path) -> PathBuf {
    let package = dir.join("weather");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{ "version": "1.0.0", "rutis": { "provides": { "weather": { "today": "sync" } } } }"#,
    )
    .unwrap();
    std::fs::write(package.join("index.mjs"), WEATHER).unwrap();
    package.join("index.mjs")
}

/// A Rust plugin using `weather` by name, whoever provides it.
struct RustUser(Probe);

impl DynPlugin for RustUser {
    fn name(&self) -> &str {
        "rust-user"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![host_key("weather").erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let weather = ctx.require_at(&host_key("weather"))?;
            let today: String = rutis_bridge::session::decode_value(
                weather.invoke("today", json!([]).into()).unwrap(),
            )
            .unwrap();
            let probe = self.0.clone();
            probe.0.lock().unwrap().push(format!("rust: {today}"));
            Ok(Effect::Disposer(Box::new(move || {
                probe.0.lock().unwrap().push("rust: bye".into());
                Ok(())
            })))
        })
    }
}

/// guarantee: loader/services-by-name
#[tokio::test(flavor = "multi_thread")]
async fn row_services_are_shared_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let weather = weather_package(dir.path());
    let forecaster = dir.path().join("forecaster.mjs");
    std::fs::write(&forecaster, FORECASTER).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let (loader, runtime) = mount(&root, node_package(), &["probe", "weather"]).await;
    (&runtime).await.unwrap();
    root.plugin(RustUser(probe.clone()));

    let base = vec![
        row("w", &weather, json!({}), json!(null)),
        row("f", &forecaster, json!({}), json!(null)),
    ];
    let report = loader.reconcile(layer(base.clone()), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    // Rust injects the row's service by name and calls it.
    probe.wait_for("rust: sunny").await;
    // A row of the same process gets the native object, not a proxy.
    probe.wait_for("js: sunny native").await;
    assert_eq!(
        loader.get("w").unwrap().meta["provides"],
        json!({ "weather": { "today": "sync" } })
    );

    // The provider goes: its users stop.
    let remaining: Vec<Value> = base.into_iter().filter(|r| r["id"] != "w").collect();
    loader.reconcile(layer(remaining), None).await.unwrap();
    probe.wait_for("rust: bye").await;
    until(
        || row_state(&loader, "f") == Some(FiberState::Pending),
        "the forecaster to wait",
    )
    .await;

    root.shutdown().await.unwrap();
}

// ── Services rows provide without declaring them ────────────────

/// A published Cordis plugin: no `rutis.provides` anywhere, a service
/// class with a method of its own, an inherited one, and a field.
const STORE: &str = r#"
export const name = 'store'
class Base {
  keys() { return ['a'] }
}
class Store extends Base {
  constructor(prefix) { super(); this.prefix = prefix }
  get(key) { return `${this.prefix}${key}` }
}
export function apply(ctx, config) {
  ctx.provide('store', new Store(config.prefix ?? ''))
}
"#;

/// Uses `store` in Cordis, natively.
const STORE_USER: &str = r#"
export const name = 'store-user'
export const inject = ['store', 'probe']
export function apply(ctx) {
  // `prefix` is a field: only the native object has it, not a proxy.
  ctx.probe.record(`user: ${ctx.store.get('x')} prefix=${ctx.store.prefix}`)
}
"#;

/// A Rust plugin calling `store` by name, whoever provides it.
struct StoreUser(Probe);

impl DynPlugin for StoreUser {
    fn name(&self) -> &str {
        "store-user"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![host_key("store").erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let store = ctx.require_at(&host_key("store"))?;
            let failed = |e: rutis_bridge::session::Error| {
                rutis::Error::from(ErrorKind::PluginFailed(e.to_string().into()))
            };
            let methods = store.methods().unwrap_or_default();
            let got = settle(store.invoke("get", json!(["y"]).into()).map_err(failed)?)
                .await
                .and_then(RpcValue::json)
                .map_err(failed)?;
            let line = format!("rust: {} {methods}", got.as_str().unwrap_or_default());
            self.0 .0.lock().unwrap().push(line);
            Ok(Effect::Done)
        })
    }
}

/// The services a Cordis row provides without declaring them reach rutis
/// by name, with the methods the object has (own and inherited, async;
/// not its fields): Rust calls them. A row of the same runtime injecting
/// one gets the native object, its dependency resolved in Cordis.
/// risk: F8 (#428)
/// guarantee: loader/announced-services
#[tokio::test(flavor = "multi_thread")]
async fn cordis_rows_offer_what_they_provide_without_declaring_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store.mjs");
    std::fs::write(&store, STORE).unwrap();
    let user = dir.path().join("user.mjs");
    std::fs::write(&user, STORE_USER).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    // As rutis-host: every name crosses by name.
    let (loader, runtime) = mount_by_name(&root).await;
    (&runtime).await.unwrap();
    root.plugin(StoreUser(probe.clone()));

    let rows = vec![
        row("s", &store, json!({ "prefix": "v:" }), json!(null)),
        row("u", &user, json!({}), json!(null)),
    ];
    let report = loader.check(&layer(rows.clone())).await;
    let requires = report
        .rows
        .iter()
        .find(|row| row.id == "u")
        .map(|row| match &row.status {
            RowStatus::Checked { requires, .. } => requires.clone(),
            other => panic!("{other:?}"),
        })
        .unwrap();
    assert_eq!(
        requires
            .iter()
            .find(|requirement| requirement.service == "store")
            .unwrap()
            .supply,
        Supply::Announced(vec!["s".into()])
    );
    let report = loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    probe.wait_for("user: v:x prefix=v:").await;
    probe
        .wait_for(r#"rust: v:y {"get":"async","keys":"async"}"#)
        .await;

    root.shutdown().await.unwrap();
}

/// A row providing a service another provider has already fails, as the
/// later one, and the error names both.
/// risk: F8 (#428)
/// guarantee: loader/announced-services
#[tokio::test(flavor = "multi_thread")]
async fn a_service_provided_twice_fails_the_later_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store.mjs");
    std::fs::write(&store, STORE).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    // The application provides `store` itself.
    root.provide_at(host_key("store"), Arc::new(probe.clone()))
        .unwrap();
    let (loader, runtime) = mount_by_name(&root).await;
    (&runtime).await.unwrap();

    loader
        .reconcile(layer(vec![row("s", &store, json!({}), json!(null))]), None)
        .await
        .unwrap();
    until(
        || row_state(&loader, "s") == Some(FiberState::Failed),
        "the row to fail",
    )
    .await;
    let EntryStatus::Running(snapshot) = loader.get("s").unwrap().status else {
        panic!("s has no fiber");
    };
    let error = snapshot.error.expect("why it failed").to_string();
    assert!(
        error.contains(
            "service store is already provided by the application: row s cannot provide it too"
        ),
        "{error}"
    );

    root.shutdown().await.unwrap();
}

/// [`mount`] with a catalog sharing every name, as rutis-host's does.
async fn mount_by_name(root: &Ctx) -> (Loader, FiberView) {
    let mut catalog = ServiceCatalog::new();
    catalog.share_by_name();
    let runtime = LocalRuntime::node(node_package(), node_package().join("package.json"))
        .host("probe", json!({ "record": "sync" }));
    let resolver = Arc::new(RuntimeResolver::node(runtime.handle()).with_catalog(&catalog));
    let runtime = root.plugin(runtime);
    let options = LoaderOptions {
        catalog,
        ..LoaderOptions::default()
    };
    let plugin = LoaderPlugin::new(Chain::new().with_shared(resolver.clone()), options);
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(resolver));
    (loader, runtime)
}

// ── A row removed while it loads ────────────────────────────────

const SLOW: &str = r#"
export const name = 'slow'
export const inject = ['probe']
export async function apply(ctx) {
  await new Promise(resolve => setTimeout(resolve, 300))
  ctx.probe.record('slow: start')
  ctx.effect(() => () => ctx.probe.record('slow: bye'))
}
"#;

/// The row restarts while its plugin is still starting in Node: once the
/// old generation's load returns, that plugin is unloaded there too, not
/// left running next to the new one.
/// guarantee: loader/row-unload-leaves-nothing
#[tokio::test(flavor = "multi_thread")]
async fn a_row_restarted_while_loading_is_unloaded_in_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let slow = dir.path().join("slow.mjs");
    std::fs::write(&slow, SLOW).unwrap();
    let probe = Probe::default();
    let (root, loader, _runtime) = interop_loader(&probe).await;
    let adding = {
        let loader = loader.clone();
        let slow = slow.clone();
        tokio::spawn(async move {
            loader
                .reconcile(layer(vec![row("s", &slow, json!({}), json!(null))]), None)
                .await
        })
    };
    until(
        || row_state(&loader, "s") == Some(FiberState::Loading),
        "the row to start loading",
    )
    .await;
    // Restarted while its apply still waits for Node: that generation's
    // plugin must not stay loaded once its load returns.
    let view = loader.get("s").unwrap().view.unwrap();
    view.restart().await.unwrap();
    let _ = adding.await;
    until(
        || row_state(&loader, "s") == Some(FiberState::Active),
        "the new generation to run",
    )
    .await;
    loader.reconcile(layer(Vec::new()), None).await.unwrap();
    until(
        || {
            probe
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|l| *l == "slow: bye")
                .count()
                == 2
        },
        "both generations to be unloaded",
    )
    .await;
    root.shutdown().await.unwrap();
}

// ── Leases of a row that fails to load ──────────────────────────

const BROKEN: &str = r#"
export const name = 'broken'
export const inject = ['probe']
export function apply() { throw new Error('broken on purpose') }
"#;

// Reads the Cordis Context directly, without injecting what it looks at.
const INSPECTOR: &str = r#"
export const name = 'inspector'
export function apply(ctx) {
  ctx.provide('inspector', { has(name) { return ctx.get(name, false) !== undefined } })
}
"#;

/// A row whose plugin fails to load gives back the host services it leased:
/// nothing of it stays registered in the runtime.
/// guarantee: loader/row-unload-leaves-nothing
#[tokio::test(flavor = "multi_thread")]
async fn a_row_that_fails_to_load_releases_its_leases() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.mjs");
    std::fs::write(&broken, BROKEN).unwrap();
    let inspector = dir.path().join("inspector/index.mjs");
    std::fs::create_dir_all(inspector.parent().unwrap()).unwrap();
    std::fs::write(&inspector, INSPECTOR).unwrap();
    std::fs::write(
        dir.path().join("inspector/package.json"),
        r#"{ "rutis": { "provides": { "inspector": { "has": "sync" } } } }"#,
    )
    .unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let (loader, runtime) = mount(&root, node_package(), &["probe", "inspector"]).await;
    (&runtime).await.unwrap();
    loader
        .reconcile(
            layer(vec![
                row("i", &inspector, json!({}), json!(null)),
                row("b", &broken, json!({}), json!(null)),
            ]),
            None,
        )
        .await
        .unwrap();
    until(
        || row_state(&loader, "b") == Some(FiberState::Failed),
        "the broken row to fail",
    )
    .await;
    let key = host_key("inspector");
    until(|| root.get_at(&key).is_some(), "the inspector").await;
    let inspector = root.get_at(&key).unwrap();
    let has_probe = inspector
        .invoke("has", json!(["probe"]).into())
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        has_probe,
        json!(false),
        "the failed row's lease was given back"
    );
    drop(inspector);
    root.shutdown().await.unwrap();
}

/// A row with an `inject` list runs its plugin behind a gate; when the
/// plugin's apply fails, the row fails with that error, as the same row
/// without the gate does (#584).
/// guarantee: loader/gated-row-failure
#[tokio::test(flavor = "multi_thread")]
async fn a_gated_row_fails_with_its_plugins_error() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.mjs");
    std::fs::write(&broken, BROKEN).unwrap();
    let probe = Probe::default();
    let (root, loader, _runtime) = interop_loader(&probe).await;
    loader
        .reconcile(
            layer(vec![
                row("u", &broken, json!({}), json!(null)),
                row("g", &broken, json!({}), json!({ "inject": ["probe"] })),
            ]),
            None,
        )
        .await
        .unwrap();
    let error_of = |id: &str| match loader.get(id).map(|entry| entry.status) {
        Some(EntryStatus::Running(snapshot)) if snapshot.state == FiberState::Failed => {
            snapshot.error.map(|error| error.to_string())
        }
        _ => None,
    };
    until(
        || error_of("u").is_some() && error_of("g").is_some(),
        "both rows to fail",
    )
    .await;
    let (ungated, gated) = (error_of("u").unwrap(), error_of("g").unwrap());
    assert!(ungated.contains("broken on purpose"), "{ungated}");
    assert_eq!(gated, ungated);
    root.shutdown().await.unwrap();
}

/// A row whose plugin fails keeps the plugin's error as it was thrown: the
/// host reads its name, message and the plugin's own stack, and no prefix
/// of rutis's comes before the message (#677).
/// guarantee: loader/row-failure-keeps-the-error
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_row_keeps_the_plugins_error() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.mjs");
    std::fs::write(&broken, BROKEN).unwrap();
    let probe = Probe::default();
    let (root, loader, _runtime) = interop_loader(&probe).await;
    loader
        .reconcile(layer(vec![row("b", &broken, json!({}), json!(null))]), None)
        .await
        .unwrap();
    let error = || match loader.get("b").map(|entry| entry.status) {
        Some(EntryStatus::Running(snapshot)) if snapshot.state == FiberState::Failed => {
            snapshot.error
        }
        _ => None,
    };
    until(|| error().is_some(), "the row to fail").await;
    let error = error().unwrap();
    assert_eq!(error.to_string(), "Error: broken on purpose");
    let thrown = rutis_bridge::thrown(&error).expect("the plugin's own error");
    let rutis_bridge::session::Error::Remote { name, message, .. } = thrown else {
        unreachable!("thrown gives remote errors only")
    };
    assert_eq!(
        (name.as_str(), message.as_str()),
        ("Error", "broken on purpose")
    );
    let stack = thrown.stack().expect("a stack");
    assert!(stack.contains("broken.mjs"), "{stack}");
    root.shutdown().await.unwrap();
}

const QUIT: &str = r#"
export const name = 'quit'
export const inject = ['probe']
export function apply(ctx, config) {
  // A marker for the test: each start of the plugin.
  ctx.probe.record(`${config.tag}: start`)
  ctx.effect(() => () => ctx.probe.record(`${config.tag}: bye`))
  const quit = () => ctx.fiber.dispose()
  if (config.when === 'apply') quit()
  else if (config.when === 'microtask') queueMicrotask(quit)
  else if (config.when === 'later') setTimeout(quit, 300)
  else if (config.when === 'throw') { quit(); throw new Error('quit and fail') }
}
"#;

/// A Cordis plugin that disposes its own fiber ends its row, as a Rust
/// plugin disposing itself: while it starts, just after, later, or behind
/// the gate of the row's `inject`. A gate unloading because its inject
/// went is no end of the row, and a plugin that disposes itself and then
/// fails to load only fails.
/// guarantee: loader/self-dispose
#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_that_disposes_itself_ends_its_row() {
    let dir = tempfile::tempdir().unwrap();
    let quit = dir.path().join("quit.mjs");
    std::fs::write(&quit, QUIT).unwrap();
    let flag = dir.path().join("flag.mjs");
    std::fs::write(&flag, FLAG).unwrap();
    let probe = Probe::default();
    let (root, loader, _runtime) = interop_loader(&probe).await;
    let changes = Arc::new(Mutex::new(Vec::new()));
    let seen = changes.clone();
    root.on::<LoaderChanged>(move |e, _| {
        seen.lock().unwrap().push((*e).clone());
        std::future::ready(Ok(None))
    })
    .unwrap();
    let gated = row(
        "w",
        &quit,
        json!({ "tag": "w", "when": "never" }),
        json!({ "inject": ["late"] }),
    );
    let report = loader
        .reconcile(
            layer(vec![
                row(
                    "a",
                    &quit,
                    json!({ "tag": "a", "when": "apply" }),
                    json!(null),
                ),
                row(
                    "m",
                    &quit,
                    json!({ "tag": "m", "when": "microtask" }),
                    json!(null),
                ),
                row(
                    "l",
                    &quit,
                    json!({ "tag": "l", "when": "later" }),
                    json!(null),
                ),
                row(
                    "g",
                    &quit,
                    json!({ "tag": "g", "when": "later" }),
                    json!({ "inject": ["late"] }),
                ),
                gated.clone(),
                row("f", &flag, json!({}), json!(null)),
            ]),
            None,
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    let ended = || {
        let mut ids: Vec<String> = changes
            .lock()
            .unwrap()
            .iter()
            .filter_map(|change| match change {
                LoaderChanged::SelfDisposed { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        ids.sort();
        ids
    };
    until(|| ended().len() == 4, "the rows to end").await;
    assert_eq!(ended(), ["a", "g", "l", "m"]);
    for id in ["a", "g", "l", "m"] {
        assert_ne!(row_state(&loader, id), Some(FiberState::Active), "{id}");
    }
    assert_eq!(row_state(&loader, "f"), Some(FiberState::Active));

    // `late` goes: the gate of `w` unloads its plugin, and the row stays.
    let w = loader.get("w").unwrap().plugin;
    let flag_row = row("f", &flag, json!({}), json!(null));
    loader
        .reconcile(layer(vec![gated.clone()]), None)
        .await
        .unwrap();
    probe.wait_for("w: bye").await;
    // `late` comes back: the gate starts the plugin again, in the same row.
    // Had the unloading ended the row, the runner would have forgotten it
    // at once (`unloadRow` deletes it before anything else) and disposed
    // the gate, so the plugin would not start again; or rutis would have
    // disposed the row's fiber, and it would run as a new one.
    loader
        .reconcile(layer(vec![gated.clone(), flag_row]), None)
        .await
        .unwrap();
    until(
        || {
            let lines = probe.0.lock().unwrap();
            lines.iter().filter(|line| *line == "w: start").count() == 2
        },
        "w to start again",
    )
    .await;
    assert_eq!(ended(), ["a", "g", "l", "m"]);
    assert_eq!(loader.get("w").unwrap().plugin, w);
    assert_eq!(row_state(&loader, "w"), Some(FiberState::Active));

    let failing = row(
        "t",
        &quit,
        json!({ "tag": "t", "when": "throw" }),
        json!(null),
    );
    loader
        .reconcile(layer(vec![gated, failing]), None)
        .await
        .unwrap();
    // Failed is the whole answer, nothing comes after it: the runner never
    // ends a row still loading, and forgets the row before it answers the
    // load. A `rows.ended` before that answer would dispose the fiber
    // before its apply fails, and it would never be Failed; the bridge
    // drops the row's end handler before the apply returns its error, so
    // none after it reaches the fiber.
    until(
        || row_state(&loader, "t") == Some(FiberState::Failed),
        "the failing row to fail",
    )
    .await;
    assert_eq!(ended(), ["a", "g", "l", "m"]);
    assert_eq!(row_state(&loader, "t"), Some(FiberState::Failed));

    root.shutdown().await.unwrap();
}
