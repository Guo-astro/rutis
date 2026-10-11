//! Bun rows among the other languages: Bun, Python and Node plugins use each
//! other's services across three processes. All start cold together, and
//! their `apply` call each other's services synchronously while the others
//! start: the Bun runtime runs incoming calls while its own synchronous call
//! waits, so no two runtimes wait for each other for ever.
#![cfg(all(unix, feature = "bun", feature = "node", feature = "python"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::{Ctx, FiberState};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{Reply, Value as RpcValue};
use rutis_loader::{
    Chain, EntryStatus, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, RuntimeResolver,
    RuntimeRowsPlugin, ServiceCatalog,
};
use rutis_test_support::{eventually, eventually_with, within};
use serde_json::{json, Value};

const BUN_PROVIDER: &str = r#"
export const provides = { bun_weather: { today: 'sync', later: 'async' } }
export function apply(ctx) {
  ctx.provide('bun_weather', { today: () => 'bun sunny', later: async () => 'bun later' })
}
"#;

const BUN_CONSUMER: &str = r#"
export const inject = ['py_weather', 'node_weather', 'probe']
export async function apply(ctx) {
  const probe = ctx.use('probe')
  const py = ctx.use('py_weather'), node = ctx.use('node_weather')
  probe.record(`bun consumer: ${py.today()} / ${await py.later()} / ${node.today()}`)
  return () => probe.record('bun consumer: bye')
}
"#;

const PY_PROVIDER: &str = r#"
class Weather:
    def today(self):
        return "py sunny"

    async def later(self):
        return "py later"


provides = {"py_weather": Weather}


def apply(ctx, config):
    ctx.provide("py_weather", Weather())
"#;

const PY_CONSUMER: &str = r#"
inject = ["bun_weather", "probe"]


async def apply(ctx, config):
    probe = ctx.use("probe")
    weather = ctx.use("bun_weather")
    probe.record(f"py consumer: {weather.today()} / {await weather.later()}")
    return lambda: probe.record("py consumer: bye")
"#;

const NODE_PROVIDER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { node_weather: { today: 'sync' } },
  apply(ctx) { ctx.provide('node_weather', { today: () => 'node sunny' }) },
})
"#;

const NODE_CONSUMER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['bun_weather', 'probe'],
  apply(ctx) {
    const probe = ctx.use('probe')
    probe.record(`node consumer: ${ctx.use('bun_weather').today()}`)
    return () => probe.record('node consumer: bye')
  },
})
"#;

// A Bun service whose synchronous method calls Python synchronously, which
// calls back into Bun: a chain across both runtimes, during one call.
const BUN_RELAY: &str = r#"
export const inject = ['py_echo']
export const provides = { relay: { ask: 'sync' } }
export function apply(ctx) {
  const echo = ctx.use('py_echo')
  ctx.provide('relay', { ask: () => echo.through(() => 'from bun') })
}
"#;

const PY_ECHO: &str = r#"
class Echo:
    def through(self, callback):
        return f"py heard {callback()}"


provides = {"py_echo": Echo}


def apply(ctx, config):
    ctx.provide("py_echo", Echo())
"#;

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<String>>>);

impl HostDispatch for Probe {
    fn invoke(&self, _method: &str, args: RpcValue) -> Reply {
        let [line]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "record": "sync" }))
    }
}

impl Probe {
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

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    js: PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let (bun, py, js) = (
        dir.path().join("bun"),
        dir.path().join("py"),
        dir.path().join("js"),
    );
    for path in [&bun, &py, &js] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(bun.join("package.json"), "{}").unwrap();
    for (name, text) in [
        ("provider", BUN_PROVIDER),
        ("consumer", BUN_CONSUMER),
        ("relay", BUN_RELAY),
    ] {
        std::fs::write(bun.join(format!("{name}.ts")), text).unwrap();
    }
    for (name, text) in [
        ("py_provider", PY_PROVIDER),
        ("py_consumer", PY_CONSUMER),
        ("py_echo", PY_ECHO),
    ] {
        std::fs::write(py.join(format!("{name}.py")), text).unwrap();
    }
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    for (name, text) in [("provider", NODE_PROVIDER), ("consumer", NODE_CONSUMER)] {
        std::fs::write(js.join(format!("{name}.mjs")), text.replace("PLUGIN", &sdk)).unwrap();
    }

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in [
        "probe",
        "bun_weather",
        "py_weather",
        "node_weather",
        "relay",
        "py_echo",
    ] {
        catalog.register_shared(name);
    }
    let bun_runtime = LocalRuntime::bun(repo().join("bun/rutis-bun"), &bun);
    let python = LocalRuntime::python(&py).python_path(repo().join("python/rutis"));
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let bun_rows = Arc::new(RuntimeResolver::modules(bun_runtime.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    root.plugin(bun_runtime);
    root.plugin(python);
    root.plugin(node);
    let chain = Chain::new()
        .with_shared(bun_rows.clone())
        .with_shared(python_rows.clone())
        .with_shared(node_rows.clone());
    let plugin = LoaderPlugin::new(
        chain,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    for rows in [bun_rows, python_rows, node_rows] {
        root.plugin(RuntimeRowsPlugin::new(rows));
    }
    Fixture {
        root,
        loader,
        probe,
        js,
        _dir: dir,
    }
}

fn layer(rows: Vec<Value>) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

fn state(loader: &Loader, id: &str) -> Option<FiberState> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn three_runtimes_start_cold_and_use_each_other() {
    let fixture = fixture().await;
    let js = |file: &str| fixture.js.join(file).to_string_lossy().into_owned();
    let report = fixture
        .loader
        .reconcile(
            layer(vec![
                json!({ "id": "bc", "name": "bun:./consumer.ts" }),
                json!({ "id": "pc", "name": "py:py_consumer" }),
                json!({ "id": "nc", "name": js("consumer.mjs") }),
                json!({ "id": "bp", "name": "bun:./provider.ts" }),
                json!({ "id": "pp", "name": "py:py_provider" }),
                json!({ "id": "np", "name": js("provider.mjs") }),
            ]),
            None,
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    let probe = &fixture.probe;
    probe
        .wait_for("bun consumer: py sunny / py later / node sunny")
        .await;
    probe.wait_for("py consumer: bun sunny / bun later").await;
    probe.wait_for("node consumer: bun sunny").await;

    // The Bun provider goes: its users in Python and Node stop, the Bun
    // consumer (which does not use it) keeps running.
    fixture
        .loader
        .reconcile(
            layer(vec![
                json!({ "id": "bc", "name": "bun:./consumer.ts" }),
                json!({ "id": "pc", "name": "py:py_consumer" }),
                json!({ "id": "nc", "name": js("consumer.mjs") }),
                json!({ "id": "pp", "name": "py:py_provider" }),
                json!({ "id": "np", "name": js("provider.mjs") }),
            ]),
            None,
        )
        .await
        .unwrap();
    probe.wait_for("py consumer: bye").await;
    probe.wait_for("node consumer: bye").await;
    assert_eq!(state(&fixture.loader, "bc"), Some(FiberState::Active));
    fixture.root.shutdown().await.unwrap();
}

/// One synchronous call that crosses into Python and back into Bun while
/// the first Bun call still waits.
#[tokio::test(flavor = "multi_thread")]
async fn a_synchronous_chain_crosses_bun_and_python_and_back() {
    let fixture = fixture().await;
    let report = fixture
        .loader
        .reconcile(
            layer(vec![
                json!({ "id": "r", "name": "bun:./relay.ts" }),
                json!({ "id": "e", "name": "py:py_echo" }),
            ]),
            None,
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    let relay = eventually(
        || {
            let relay = fixture.root.get_as::<dyn HostDispatch>(host_key("relay"))?;
            (state(&fixture.loader, "r") == Some(FiberState::Active)).then_some(relay)
        },
        "the relay row and its service",
    )
    .await;
    let answer = within(
        "the chain (a cycle of waits hangs)",
        tokio::task::spawn_blocking(move || relay.invoke("ask", json!([]).into())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(answer.json().unwrap(), json!("py heard from bun"));
    fixture.root.shutdown().await.unwrap();
}
