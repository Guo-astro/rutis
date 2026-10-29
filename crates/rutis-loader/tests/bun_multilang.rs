//! Bun rows among the other languages: Bun, Python and Node plugins use each
//! other's services across three processes. All start cold together, and
//! their `apply` call each other's services synchronously while the others
//! start: the Bun runtime runs incoming calls while its own synchronous call
//! waits, so no two runtimes wait for each other for ever.
#![cfg(all(unix, feature = "runtimes"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{
    greet_as_main, layer, repo, row_state, rows_stage, Probe, TestRuntime, COLD,
};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::json;

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
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
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
    greet_as_main(&root);
    let bun_runtime = TestRuntime::bun(&bun);
    let python = TestRuntime::python(&py);
    let node = TestRuntime::node(&repo().join("node/rutis-runtime/package.json"));
    let sessions = Arc::new(SessionResolver::npm(&catalog, "node"));
    for runtime in [&bun_runtime, &python, &node] {
        sessions.runtime(runtime.handle());
    }
    root.plugin(bun_runtime);
    root.plugin(python);
    root.plugin(node);
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
    for name in ["bun", "py", "node"] {
        rows_stage(&root, &sessions, name).await;
    }
    Fixture {
        root,
        loader,
        probe,
        js,
        _dir: dir,
    }
}

/// guarantee: loader/runtimes-do-not-wait-for-each-other
#[tokio::test(flavor = "multi_thread")]
async fn three_runtimes_start_cold_and_use_each_other() {
    if !rutis_test_support::runtimes::all_enabled(&["bun", "python", "node"]) {
        return;
    }
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
    assert_eq!(row_state(&fixture.loader, "bc"), Some(FiberState::Active));
    fixture.root.shutdown().await.unwrap();
}

/// One synchronous call that crosses into Python and back into Bun while
/// the first Bun call still waits.
#[tokio::test(flavor = "multi_thread")]
async fn a_synchronous_chain_crosses_bun_and_python_and_back() {
    if !rutis_test_support::runtimes::all_enabled(&["bun", "python", "node"]) {
        return;
    }
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
    let relay = COLD
        .eventually(
            || {
                let relay = fixture
                    .root
                    .get_at::<dyn HostDispatch>(&host_key("relay"))?;
                (row_state(&fixture.loader, "r") == Some(FiberState::Active)).then_some(relay)
            },
            "the relay row and its service",
        )
        .await;
    let answer = COLD
        .within(
            "the chain (a cycle of waits hangs)",
            tokio::task::spawn_blocking(move || relay.invoke("ask", json!([]).into())),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.json().unwrap(), json!("py heard from bun"));
    fixture.root.shutdown().await.unwrap();
}
