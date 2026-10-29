//! The runtime conformance suite: the same leaf plugins written in
//! JavaScript (`definePlugin`, in the Node runtime) and in Python (in the
//! Python runtime) behave the same under rutis-loader, and use each other's
//! services across the two processes.
#![cfg(feature = "runtimes")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::runtimes::{
    greet_as_main, layer, repo, row_state, rows_stage, Llm, Probe, TestRuntime, COLD,
};
use rutis::host::{CtxHost, FiberState, FiberView};
use rutis::Ctx;
use rutis_bridge::session::host_key;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

// ── The plugins, once per language ──────────────────────────────

const PY_PROVIDER: &str = r#"
class Weather:
    native = True

    def today(self):
        return "py sunny"

    async def later(self):
        return "py later"


provides = {"py_weather": Weather}
inject = ["probe"]


def apply(ctx, config):
    probe = ctx.use("probe")
    ctx.provide("py_weather", Weather())
    probe.record("py provider: start")
    return lambda: probe.record("py provider: bye")
"#;

const PY_CONSUMER: &str = r#"
inject = ["js_weather", "probe"]


async def apply(ctx, config):
    probe = ctx.use("probe")
    weather = ctx.use("js_weather")
    probe.record(f"py consumer: {weather.today()} / {await weather.later()}")
    return lambda: probe.record("py consumer: bye")
"#;

const PY_LOCAL: &str = r#"
inject = ["py_weather", "probe"]


def apply(ctx, config):
    probe = ctx.use("probe")
    native = getattr(ctx.use("py_weather"), "native", False)
    probe.record(f"py local: {'native' if native else 'proxy'}")
    ctx.effect(lambda: probe.record("py local: bye"))
"#;

const PY_GATED: &str = r#"
inject = ["llm", "probe"]


def apply(ctx, config):
    probe = ctx.use("probe")
    probe.record(f"py gated: {ctx.use('llm').ask()}")
    return lambda: probe.record("py gated: bye")
"#;

const JS_PROVIDER: &str = r#"
import { definePlugin } from 'PLUGIN'
class Weather {
  native = true
  today() { return 'js sunny' }
  async later() { return 'js later' }
}
export default definePlugin({
  inject: ['probe'],
  provides: { js_weather: { today: 'sync', later: 'async' } },
  apply(ctx) {
    const probe = ctx.use('probe')
    ctx.provide('js_weather', new Weather())
    probe.record('js provider: start')
    return () => probe.record('js provider: bye')
  },
})
"#;

const JS_CONSUMER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['py_weather', 'probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    const weather = ctx.use('py_weather')
    probe.record(`js consumer: ${weather.today()} / ${await weather.later()}`)
    return () => probe.record('js consumer: bye')
  },
})
"#;

const JS_LOCAL: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['js_weather', 'probe'],
  apply(ctx) {
    const probe = ctx.use('probe')
    probe.record(`js local: ${ctx.use('js_weather').native === true ? 'native' : 'proxy'}`)
    ctx.effect(() => probe.record('js local: bye'))
  },
})
"#;

const JS_GATED: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['llm', 'probe'],
  apply(ctx) {
    const probe = ctx.use('probe')
    probe.record(`js gated: ${ctx.use('llm').ask()}`)
    return () => probe.record('js gated: bye')
  },
})
"#;

// Python → Node → Python: a Python row awaits a JS service that awaits a
// Python one (#225).
const PY_ROUND_TRIP: &str = r#"
inject = ["js_relay", "probe"]


async def apply(ctx, config):
    probe = ctx.use("probe")
    probe.record(f"py round trip: {await ctx.use('js_relay').relay()}")
    return lambda: probe.record("py round trip: bye")
"#;

const JS_RELAY: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['py_weather'],
  provides: { js_relay: { relay: 'async' } },
  apply(ctx) {
    const weather = ctx.use('py_weather')
    ctx.provide('js_relay', { async relay() { return `js relays ${await weather.later()}` } })
  },
})
"#;

// ── Harness ─────────────────────────────────────────────────────

/// Plugin files: Python modules in `py/`, JavaScript in `js/`.
fn write_plugins(dir: &Path) -> (PathBuf, PathBuf) {
    let py = dir.join("py");
    let js = dir.join("js");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::create_dir_all(&js).unwrap();
    for (name, text) in [
        ("py_provider", PY_PROVIDER),
        ("py_consumer", PY_CONSUMER),
        ("py_local", PY_LOCAL),
        ("py_gated", PY_GATED),
        ("py_round_trip", PY_ROUND_TRIP),
    ] {
        std::fs::write(py.join(format!("{name}.py")), text).unwrap();
    }
    let plugin = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let plugin = url::Url::from_file_path(&plugin).unwrap().to_string();
    for (name, text) in [
        ("js_provider", JS_PROVIDER),
        ("js_consumer", JS_CONSUMER),
        ("js_local", JS_LOCAL),
        ("js_gated", JS_GATED),
        ("js_relay", JS_RELAY),
    ] {
        std::fs::write(
            js.join(format!("{name}.mjs")),
            text.replace("PLUGIN", &plugin),
        )
        .unwrap();
    }
    (py, js)
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    node: FiberView,
    python: FiberView,
    probe: Probe,
    py: PathBuf,
    js: PathBuf,
    _dir: tempfile::TempDir,
}

/// Both runtimes, one loader, and the second stage of each runtime.
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let (py, js) = write_plugins(dir.path());
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in [
        "probe",
        "llm",
        "py_weather",
        "js_weather",
        "js_relay",
        "py_forecast",
        "js_forecast",
    ] {
        catalog.register_shared(name);
    }
    greet_as_main(&root);
    let node = TestRuntime::node(&repo().join("node/rutis-runtime/package.json"));
    let python = TestRuntime::python(&py);
    let sessions = Arc::new(SessionResolver::npm(&catalog, "node"));
    sessions.runtime(node.handle());
    sessions.runtime(python.handle());
    let node = root.plugin(node);
    let python = root.plugin(python);
    let options = LoaderOptions {
        catalog,
        ..LoaderOptions::default()
    };
    let plugin = LoaderPlugin::new(Chain::new().with_shared(sessions.clone()), options);
    let loader = plugin.handle();
    sessions.attach(&loader);
    root.plugin(plugin).await.unwrap();
    rows_stage(&root, &sessions, "node").await;
    rows_stage(&root, &sessions, "py").await;
    Fixture {
        root,
        loader,
        node,
        python,
        probe,
        py,
        js,
        _dir: dir,
    }
}

fn py_row(id: &str, module: &str) -> Value {
    json!({ "id": id, "name": format!("py:{module}"), "config": {} })
}

fn js_row(fixture: &Fixture, id: &str, file: &str) -> Value {
    let path = fixture.js.join(format!("{file}.mjs"));
    json!({ "id": id, "name": path.to_string_lossy(), "config": {} })
}

fn all_rows(fixture: &Fixture) -> Vec<Value> {
    vec![
        py_row("pp", "py_provider"),
        py_row("pc", "py_consumer"),
        py_row("pl", "py_local"),
        js_row(fixture, "jp", "js_provider"),
        js_row(fixture, "jc", "js_consumer"),
        js_row(fixture, "jl", "js_local"),
    ]
}

// ── The suite ───────────────────────────────────────────────────

/// Both runtimes start cold with rows that use each other's services: no
/// runtime waits for the other, and every row starts.
/// guarantee: loader/services-by-name, loader/runtimes-do-not-wait-for-each-other
#[tokio::test(flavor = "multi_thread")]
async fn both_runtimes_start_cold_and_use_each_other() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let report = fixture
        .loader
        .reconcile(layer(all_rows(&fixture)), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    let probe = &fixture.probe;
    probe.wait_for("py consumer: js sunny / js later").await;
    probe.wait_for("js consumer: py sunny / py later").await;
    // Within one process, a service is the provider's own object.
    probe.wait_for("py local: native").await;
    probe.wait_for("js local: native").await;
    assert_eq!(fixture.node.state().state, FiberState::Active);
    assert_eq!(fixture.python.state().state, FiberState::Active);
    fixture.root.shutdown().await.unwrap();
}

/// Python awaits a Node service that awaits a Python one: the reply carries a
/// future whose origin holds ids of the other session, tagged with it
/// (`s1/node:2`), which the Node runtime refused, ending its session and
/// with it the process (#225).
/// risk: C4, C6
#[tokio::test(flavor = "multi_thread")]
async fn python_awaits_node_that_awaits_python() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let report = fixture
        .loader
        .reconcile(
            layer(vec![
                py_row("pp", "py_provider"),
                js_row(&fixture, "relay", "js_relay"),
                py_row("rt", "py_round_trip"),
            ]),
            None,
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture
        .probe
        .wait_for("py round trip: js relays py later")
        .await;
    assert_eq!(fixture.node.state().state, FiberState::Active);
    fixture.root.shutdown().await.unwrap();
}

/// A provider that goes stops exactly its users, in either language, before
/// it stops itself; everything else keeps running.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_that_goes_stops_only_its_users() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    for (provider, users, others) in [
        ("pp", ["jc", "pl"], ["pc", "jl"]),
        ("jp", ["pc", "jl"], ["jc", "pl"]),
    ] {
        let fixture = fixture().await;
        let loader = &fixture.loader;
        let all = all_rows(&fixture);
        loader.reconcile(layer(all.clone()), None).await.unwrap();
        COLD.until(
            || {
                ["pp", "pc", "pl", "jp", "jc", "jl"]
                    .iter()
                    .all(|id| row_state(loader, id) == Some(FiberState::Active))
            },
            "every row to run",
        )
        .await;

        let remaining: Vec<Value> = all.into_iter().filter(|r| r["id"] != provider).collect();
        loader.reconcile(layer(remaining), None).await.unwrap();
        let (language, prefix) = if provider == "pp" {
            ("py", "py")
        } else {
            ("js", "js")
        };
        let provider_bye = format!("{prefix} provider: bye");
        fixture.probe.wait_for(&provider_bye).await;
        COLD.until(
            || {
                users
                    .iter()
                    .all(|id| row_state(loader, id) == Some(FiberState::Pending))
            },
            "the users to wait",
        )
        .await;
        for id in others {
            assert_eq!(
                row_state(loader, id),
                Some(FiberState::Active),
                "{id} keeps running"
            );
        }
        // Users stop before the provider does.
        let user_byes: Vec<String> = match language {
            "py" => vec!["js consumer: bye".into(), "py local: bye".into()],
            _ => vec!["py consumer: bye".into(), "js local: bye".into()],
        };
        let provider_at = fixture.probe.position(&provider_bye).unwrap();
        for bye in &user_byes {
            let at = fixture
                .probe
                .position(bye)
                .unwrap_or_else(|| panic!("{bye} missing"));
            assert!(
                at < provider_at,
                "{bye} before {provider_bye}: {:?}",
                fixture.probe.lines()
            );
        }
        fixture.root.shutdown().await.unwrap();
    }
}

/// What a plugin injects gates it in rutis: it waits for the service, and
/// stops when it goes. The same for both languages.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn injected_services_gate_the_plugin() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let transitions = common::Transitions::on(&fixture.root);
    loader
        .reconcile(
            layer(vec![
                py_row("pg", "py_gated"),
                js_row(&fixture, "jg", "js_gated"),
            ]),
            None,
        )
        .await
        .unwrap();
    let fiber = |id: &str| loader.get(id).unwrap().plugin.unwrap();
    // A row resolved before its runtime ran is resolved again once it
    // runs (the fixture waits for the runtimes now, so here the rows
    // resolve with them; the check still allows both), and its new injects
    // replace its fiber: the new resolution shows first, then the old fiber
    // goes and, once it is gone, the new one comes. So wait for the new
    // fiber, not for the injects (#342, as #316). A row resolved with its
    // runtime keeps its first fiber.
    let resolved_first = ["pg", "jg"].map(|id| {
        let entry = loader.get(id).unwrap();
        (entry.plugin.unwrap(), entry.meta["inject"].is_array())
    });
    let first = resolved_first.map(|(plugin, _)| plugin);
    let offline = resolved_first.map(|(plugin, online)| (!online).then_some(plugin));
    COLD.until(
        || {
            ["pg", "jg"].iter().zip(offline).all(|(id, offline)| {
                loader.get(id).is_some_and(|entry| {
                    entry.meta["inject"].is_array()
                        && entry.plugin.is_some_and(|plugin| Some(plugin) != offline)
                })
            })
        },
        "both rows to run with their injects",
    )
    .await;
    // The states now; that they stay so until `llm` comes, `loads` checks below.
    assert_eq!(row_state(loader, "pg"), Some(FiberState::Pending));
    assert_eq!(row_state(loader, "jg"), Some(FiberState::Pending));

    let llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    fixture.probe.wait_for("py gated: 42").await;
    fixture.probe.wait_for("js gated: 42").await;
    // Neither started before `llm`: each row's fibers loaded once, for it.
    // An earlier start's Loading comes before the changes `loads` waits
    // for (one fiber's changes arrive in order).
    for (index, id) in ["pg", "jg"].into_iter().enumerate() {
        let fibers = [first[index], fiber(id)];
        assert_eq!(
            transitions.loads(&fibers).await,
            1,
            "{id}: {:?}",
            fibers.map(|fiber| transitions.of(fiber))
        );
    }
    llm.dispose().await.unwrap();
    fixture.probe.wait_for("py gated: bye").await;
    fixture.probe.wait_for("js gated: bye").await;
    COLD.until(
        || {
            row_state(loader, "pg") == Some(FiberState::Pending)
                && row_state(loader, "jg") == Some(FiberState::Pending)
        },
        "both rows to wait again",
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// The Python process ends on its own: its rows wait, the Node runtime and
/// its rows that do not use Python services keep running.
/// guarantee: loader/runtime-down-stops-its-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_python_process_stops_only_its_rows() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let loader = &fixture.loader;
    std::fs::write(
        fixture.py.join("py_exit.py"),
        "import os, threading\n\ndef apply(ctx, config):\n    threading.Timer(0.2, lambda: os._exit(17)).start()\n",
    )
    .unwrap();
    let mut all = all_rows(&fixture);
    loader.reconcile(layer(all.clone()), None).await.unwrap();
    fixture.probe.wait_for("js local: native").await;
    fixture.probe.wait_for("py local: native").await;
    all.push(py_row("px", "py_exit"));
    loader.reconcile(layer(all), None).await.unwrap();
    COLD.until(
        || {
            ["pp", "pc", "pl", "jc"]
                .iter()
                .all(|id| row_state(loader, id) == Some(FiberState::Pending))
        },
        "the Python rows to wait",
    )
    .await;
    assert_eq!(row_state(loader, "jp"), Some(FiberState::Active));
    assert_eq!(row_state(loader, "jl"), Some(FiberState::Active));
    assert_eq!(fixture.python.state().state, FiberState::Active);
    fixture.root.shutdown().await.unwrap();
}

const PY_FORECAST: &str = r#"
class Forecast:
    def __init__(self, city):
        self.city = city

    def today(self):
        return self.city


provides = {"py_forecast": Forecast}


def apply(ctx, config):
    ctx.provide("py_forecast", Forecast(config["city"]))
"#;

const PY_READER: &str = r#"
inject = ["py_forecast", "probe"]


def apply(ctx, config):
    ctx.use("probe").record(f"{config['tag']}: {ctx.use('py_forecast').today()}")
"#;

const JS_FORECAST: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { js_forecast: { today: 'sync' } },
  apply(ctx, config) {
    ctx.provide('js_forecast', { today: () => config.city })
  },
})
"#;

const JS_READER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['js_forecast', 'probe'],
  async apply(ctx, config) {
    ctx.use('probe').record(`${config.tag}: ${await ctx.use('js_forecast').today()}`)
  },
})
"#;

/// A service a row provides under a name it isolates is that isolate's: a
/// row of the same isolate injects it, a row of another isolate does not
/// see it (it waits), the rows outside get the provider outside, and two
/// providers of the name, one isolated, run side by side. Node and Python
/// alike (it used to be published outside its isolate, and one of the two
/// providers failed with `… is already provided by another plugin`).
/// guarantee: loader/isolate-and-inject
#[tokio::test(flavor = "multi_thread")]
async fn an_isolated_second_provider_runs_beside_the_first() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let plugin = repo().join("node/rutis/src/index.mjs");
    let plugin = plugin.to_string_lossy();
    for (name, text) in [("py_forecast", PY_FORECAST), ("py_reader", PY_READER)] {
        std::fs::write(fixture.py.join(format!("{name}.py")), text).unwrap();
    }
    for (name, text) in [("js_forecast", JS_FORECAST), ("js_reader", JS_READER)] {
        std::fs::write(
            fixture.js.join(format!("{name}.mjs")),
            text.replace("PLUGIN", &plugin),
        )
        .unwrap();
    }
    let with = |mut row: Value, config: Value, isolate: Option<Value>| {
        row["config"] = config;
        if let Some(isolate) = isolate {
            row["isolate"] = isolate;
        }
        row
    };
    let mut rows = Vec::new();
    for lang in ["py", "js"] {
        let service = format!("{lang}_forecast");
        let at = |label: &str| Some(json!({ service.clone(): label }));
        let row = |id: &str, module: &str| match lang {
            "py" => py_row(id, module),
            _ => js_row(&fixture, id, module),
        };
        let provider = format!("{lang}_forecast");
        let reader = format!("{lang}_reader");
        rows.extend([
            with(
                row(&format!("{lang}-oslo"), &provider),
                json!({ "city": "oslo" }),
                None,
            ),
            with(
                row(&format!("{lang}-bergen"), &provider),
                json!({ "city": "bergen" }),
                at("bergen"),
            ),
            with(
                row(&format!("{lang}-in"), &reader),
                json!({ "tag": format!("{lang} in bergen") }),
                at("bergen"),
            ),
            with(
                row(&format!("{lang}-out"), &reader),
                json!({ "tag": format!("{lang} outside") }),
                None,
            ),
            with(
                row(&format!("{lang}-other"), &reader),
                json!({ "tag": format!("{lang} in tromso") }),
                at("tromso"),
            ),
        ]);
    }
    let report = fixture.loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    let probe = &fixture.probe;
    for lang in ["py", "js"] {
        probe.wait_for(&format!("{lang} in bergen: bergen")).await;
        probe.wait_for(&format!("{lang} outside: oslo")).await;
        for id in ["oslo", "bergen", "in", "out"] {
            let id = format!("{lang}-{id}");
            COLD.until(
                || row_state(&fixture.loader, &id) == Some(FiberState::Active),
                &format!("{id} running"),
            )
            .await;
        }
    }
    // Another isolate does not see bergen's: its reader waits.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    for lang in ["py", "js"] {
        assert!(
            !probe.lines().iter().any(|line| line.contains("in tromso")),
            "{:?}",
            probe.lines()
        );
        assert_eq!(
            row_state(&fixture.loader, &format!("{lang}-other")),
            Some(FiberState::Pending),
            "{lang}"
        );
    }
    // Each provider once: the one outside, and bergen's in its isolate.
    let status = fixture.loader.status();
    for lang in ["py", "js"] {
        let service = format!("{lang}_forecast");
        let mut providers: Vec<_> = status
            .services
            .iter()
            .filter(|s| s.name == service)
            .filter_map(|s| s.provider.clone())
            .collect();
        providers.sort();
        assert_eq!(
            providers,
            [format!("{lang}-bergen"), format!("{lang}-oslo")],
            "{status}"
        );
    }
    fixture.root.shutdown().await.unwrap();
}
