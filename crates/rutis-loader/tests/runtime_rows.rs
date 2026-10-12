//! JavaScript plugins as loader rows (P6): one shared Cordis Context,
//! services resolved between rows natively, per-row load/update/unload,
//! isolate and inject forwarded, schemastery schema exported; services
//! shared with rutis by name (multilanguage M1).
#![cfg(feature = "node")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::{BoxFuture, CordisError, Ctx, EventKey, FiberState, FiberView, Listener};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{Reply, Value as RpcValue};
use rutis_loader::{
    Chain, EntryStatus, Layer, Loader, LoaderChanged, LoaderError, LoaderOptions, LoaderPlugin,
    Patch, RuntimeResolver, RuntimeRows, RuntimeRowsPlugin, ServiceCatalog,
};
use rutis_test_support::{eventually_with, until, within};
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

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<String>>>);

impl HostDispatch for Probe {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        assert_eq!(method, "record");
        let [line]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(RpcValue::Undefined)
    }
}

impl Probe {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    async fn wait_for(&self, line: &str) {
        eventually_with(
            || {
                self.0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|l| l == line)
                    .then_some(())
            },
            &format!("{line:?} recorded"),
            || format!("{:?}", self.0.lock().unwrap()),
        )
        .await
    }
}

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
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
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

fn rows(rows: Vec<Value>) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

fn row_state(loader: &Loader, id: &str) -> Option<FiberState> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

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
    loader.reconcile(rows(base), None).await.unwrap();
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
    loader.reconcile(rows(base.clone()), None).await.unwrap();
    probe.wait_for("c: hello rust").await;
    probe.take();

    let mut dying = base.clone();
    dying.push(row("x", &exit, json!({}), json!(null)));
    loader.reconcile(rows(dying), None).await.unwrap();
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

    loader.reconcile(rows(base), None).await.unwrap();
    runtime.restart().await.unwrap();
    probe.wait_for("c: hello rust").await;
    until(
        || row_state(&loader, "c") == Some(FiberState::Active),
        "rows running again",
    )
    .await;

    root.shutdown().await.unwrap();
}

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
    let report = loader.reconcile(rows(base), None).await.unwrap();
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
        .provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
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
#[tokio::test(flavor = "multi_thread")]
async fn rows_resolved_offline_wait_for_what_they_inject() {
    let dir = tempfile::tempdir().unwrap();
    let (_, consumer, _) = write_plugins(dir.path());
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    let transitions = common::Transitions::on(&root);
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
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
    loader.reconcile(rows(base), None).await.unwrap();
    assert!(loader.get("c").unwrap().meta["schema"].is_string());
    let offline = loader.get("c").unwrap().plugin.unwrap();

    // The runtime starts: the row is resolved again before it may start,
    // and now also waits for the shared `greeter`, which nobody provides.
    root.plugin(runtime).await.unwrap();
    until(
        || {
            loader
                .get("c")
                .is_some_and(|entry| entry.meta["inject"].is_array())
        },
        "the row to know its injects",
    )
    .await;
    // The state now; that it stays so until `greeter` comes, `loads` checks below.
    assert_eq!(row_state(&loader, "c"), Some(FiberState::Pending));
    assert!(probe.take().is_empty());

    // `greeter` from Rust: the JavaScript consumer calls it through Cordis.
    let greeter = root
        .provide_as::<dyn HostDispatch>(host_key("greeter"), Arc::new(RustGreeter))
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
            rows(vec![row("p", &provider, json!({}), json!(null))]),
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

impl rutis::Plugin for RustUser {
    fn name(&self) -> &str {
        "rust-user"
    }

    fn injects(&self) -> &[rutis::TypeKey] {
        static KEYS: std::sync::OnceLock<[rutis::TypeKey; 1]> = std::sync::OnceLock::new();
        KEYS.get_or_init(|| [host_key("weather")])
    }

    fn apply<'a>(
        &'a self,
        ctx: &'a Ctx,
    ) -> rutis::BoxFuture<'a, Result<rutis::Effect, rutis::CordisError>> {
        Box::pin(async move {
            let weather = ctx.require_as::<dyn HostDispatch>(host_key("weather"))?;
            let today: String = rutis_bridge::session::decode_value(
                weather.invoke("today", json!([]).into()).unwrap(),
            )
            .unwrap();
            let probe = self.0.clone();
            probe.0.lock().unwrap().push(format!("rust: {today}"));
            Ok(rutis::Effect::Disposer(Box::new(move || {
                probe.0.lock().unwrap().push("rust: bye".into());
                Ok(())
            })))
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn row_services_are_shared_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let weather = weather_package(dir.path());
    let forecaster = dir.path().join("forecaster.mjs");
    std::fs::write(&forecaster, FORECASTER).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let (loader, runtime) = mount(&root, node_package(), &["probe", "weather"]).await;
    (&runtime).await.unwrap();
    root.plugin(RustUser(probe.clone()));

    let base = vec![
        row("w", &weather, json!({}), json!(null)),
        row("f", &forecaster, json!({}), json!(null)),
    ];
    let report = loader.reconcile(rows(base.clone()), None).await.unwrap();
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
    loader.reconcile(rows(remaining), None).await.unwrap();
    probe.wait_for("rust: bye").await;
    until(
        || row_state(&loader, "f") == Some(FiberState::Pending),
        "the forecaster to wait",
    )
    .await;

    root.shutdown().await.unwrap();
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
                .reconcile(rows(vec![row("s", &slow, json!({}), json!(null))]), None)
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
    loader.reconcile(rows(Vec::new()), None).await.unwrap();
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
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let (loader, runtime) = mount(&root, node_package(), &["probe", "inspector"]).await;
    (&runtime).await.unwrap();
    loader
        .reconcile(
            rows(vec![
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
    until(
        || root.get_as::<dyn HostDispatch>(key.clone()).is_some(),
        "the inspector",
    )
    .await;
    let inspector = root.get_as::<dyn HostDispatch>(key).unwrap();
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

struct Changes(Arc<Mutex<Vec<LoaderChanged>>>);

impl Listener<LoaderChanged> for Changes {
    fn call<'a>(
        &'a self,
        _ctx: &'a Ctx,
        e: &'a LoaderChanged,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        self.0.lock().unwrap().push(e.clone());
        Box::pin(async { Ok(None) })
    }
}

/// A Cordis plugin that disposes its own fiber ends its row, as a Rust
/// plugin disposing itself: while it starts, just after, later, or behind
/// the gate of the row's `inject`. A gate unloading because its inject
/// went is no end of the row, and a plugin that disposes itself and then
/// fails to load only fails.
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
    root.events()
        .on(
            &root,
            &EventKey::<LoaderChanged>::of(),
            Changes(changes.clone()),
        )
        .unwrap();
    let gated = row(
        "w",
        &quit,
        json!({ "tag": "w", "when": "never" }),
        json!({ "inject": ["late"] }),
    );
    let report = loader
        .reconcile(
            rows(vec![
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
        .reconcile(rows(vec![gated.clone()]), None)
        .await
        .unwrap();
    probe.wait_for("w: bye").await;
    // `late` comes back: the gate starts the plugin again, in the same row.
    // Had the unloading ended the row, the runner would have forgotten it
    // at once (`unloadRow` deletes it before anything else) and disposed
    // the gate, so the plugin would not start again; or rutis would have
    // disposed the row's fiber, and it would run as a new one.
    loader
        .reconcile(rows(vec![gated.clone(), flag_row]), None)
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
        .reconcile(rows(vec![gated, failing]), None)
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
