//! The runtime conformance suite with Go: leaf plugins in Go (one binary,
//! `tests/fixtures/go/cmd/multilang`), JavaScript and Python start cold
//! together and use each other's services; Node and Python call Go
//! synchronously while Go calls back into them; a provider that goes stops
//! its users first; what a Go plugin injects gates it.
#![cfg(all(feature = "node", feature = "python", feature = "go"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{go_binary, layer, repo, row_state, Llm, Probe};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::host_key;
use rutis_loader::runtime::go::{GoBinaries, GoResolver, GoRuntimes};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

const PY_PROVIDER: &str = r#"
class WeatherError(Exception):
    def __init__(self):
        super().__init__("too windy")
        self.code = "Stormy"
        self.data = {"wind": 2**60}


class Weather:
    def today(self):
        return "py sunny"

    async def later(self):
        return "py later"

    async def fail(self):
        raise WeatherError()


provides = {"py_weather": Weather}
inject = ["probe"]


def apply(ctx, config):
    probe = ctx.use("probe")
    ctx.provide("py_weather", Weather())
    return lambda: probe.record("py provider: bye")
"#;

const PY_GO_CONSUMER: &str = r#"
inject = ["go_weather", "probe"]


async def apply(ctx, config):
    probe = ctx.use("probe")
    weather = ctx.use("go_weather")
    days = weather.each(["mon", "tue"], lambda day: day.upper())
    probe.record(f"py go consumer: {weather.today()} / {await weather.later()} / {','.join(days)}")
    return lambda: probe.record("py go consumer: bye")
"#;

const JS_PROVIDER: &str = r#"
import { definePlugin } from 'PLUGIN'
class Weather {
  today() { return 'js sunny' }
  async later() { return 'js later' }
}
export default definePlugin({
  inject: ['probe'],
  provides: { js_weather: { today: 'sync', later: 'async' } },
  apply(ctx) {
    const probe = ctx.use('probe')
    ctx.provide('js_weather', new Weather())
    return () => probe.record('js provider: bye')
  },
})
"#;

const JS_GO_CONSUMER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['go_weather', 'probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    const weather = ctx.use('go_weather')
    // A synchronous call into Go that calls back into this process.
    const days = weather.each(['mon', 'tue'], (day) => day.toUpperCase())
    probe.record(`js go consumer: ${weather.today()} / ${await weather.later()} / ${days.join(',')}`)
    return () => probe.record('js go consumer: bye')
  },
})
"#;

/// Calls the Go and the Python service's `fail`, recording what it throws.
const JS_ERRORS: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['go_weather', 'py_weather', 'probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    for (const name of ['go_weather', 'py_weather']) {
      try {
        await ctx.use(name).fail()
      } catch (error) {
        probe.record(`${name} threw ${error.name} ${error.code} ${error.message} ${typeof error.data.wind} ${error.data.wind}`)
      }
    }
  },
})
"#;

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    go: rutis_loader::runtime::go::GoRuntimesHandle,
    js: PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let py = dir.path().join("py");
    let js = dir.path().join("js");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::create_dir_all(&js).unwrap();
    std::fs::write(py.join("py_provider.py"), PY_PROVIDER).unwrap();
    std::fs::write(py.join("py_go_consumer.py"), PY_GO_CONSUMER).unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    std::fs::write(
        js.join("js_provider.mjs"),
        JS_PROVIDER.replace("PLUGIN", &sdk),
    )
    .unwrap();
    std::fs::write(
        js.join("js_go_consumer.mjs"),
        JS_GO_CONSUMER.replace("PLUGIN", &sdk),
    )
    .unwrap();
    std::fs::write(js.join("js_errors.mjs"), JS_ERRORS.replace("PLUGIN", &sdk)).unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in ["probe", "llm", "py_weather", "js_weather", "go_weather"] {
        catalog.register_shared(name);
    }
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let python = LocalRuntime::python(&py).python_path(repo().join("python/rutis"));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    let go = Arc::new(
        GoResolver::new(GoBinaries::new().file(go_binary()))
            .with_catalog(&catalog)
            .reserve(["node", "py"]),
    );
    root.plugin(node);
    root.plugin(python);
    let plugin = LoaderPlugin::new(
        Chain::new()
            .with_shared(go.clone())
            .with_shared(python_rows.clone())
            .with_shared(node_rows.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(node_rows));
    root.plugin(RuntimeRowsPlugin::new(python_rows));
    let runtimes = GoRuntimes::new(go, dir.path()).idle(None);
    let go = runtimes.handle();
    root.plugin(runtimes).await.unwrap();
    Fixture {
        root,
        loader,
        probe,
        go,
        js,
        _dir: dir,
    }
}

impl Fixture {
    /// Wait for `line`; on a timeout, say where every row and Go runtime is.
    async fn expect(&self, line: &str) {
        let what = format!("{line:?} to be recorded");
        self.wait_until(&what, || self.probe.position(line).is_some())
            .await;
    }

    /// Wait for `done`; on a timeout, say where every row and Go runtime is.
    /// Three runtimes start cold together (#238): three times HANG_GUARD.
    async fn wait_until(&self, what: &str, mut done: impl FnMut() -> bool) {
        Guard(HANG_GUARD * 3)
            .eventually_with(
                || done().then_some(()),
                what,
                || {
                    let rows: Vec<String> = self
                        .loader
                        .entries()
                        .into_iter()
                        .map(|entry| format!("{}: {:?}", entry.id, entry.status))
                        .collect();
                    let runtimes: Vec<String> = self
                        .go
                        .runtimes()
                        .into_iter()
                        .map(|runtime| format!("{}: {:?}", runtime.name, runtime.state))
                        .collect();
                    format!(
                        "recorded: {:?}\nrows: {rows:#?}\ngo runtimes: {runtimes:?}",
                        self.probe.lines()
                    )
                },
            )
            .await
    }
}

fn named(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "config": {} })
}

fn js(fixture: &Fixture, id: &str, file: &str) -> Value {
    named(
        id,
        &fixture.js.join(format!("{file}.mjs")).to_string_lossy(),
    )
}

fn all_rows(fixture: &Fixture) -> Vec<Value> {
    vec![
        named("pp", "py:py_provider"),
        named("pg", "py:py_go_consumer"),
        js(fixture, "jp", "js_provider"),
        js(fixture, "jg", "js_go_consumer"),
        named("gp", "go:go_provider"),
        named("gc", "go:go_consumer"),
        named("gl", "go:go_local"),
    ]
}

/// An error thrown in Go (`Code()`, `Data()`) or in Python (`code`,
/// `data` attributes) reaches a JavaScript caller with its code and data,
/// a big integer in the data as a bigint (#440).
/// guarantee: bridge/error-code-and-data
#[tokio::test(flavor = "multi_thread")]
async fn errors_with_a_code_and_data_cross_from_go_and_python() {
    let fixture = fixture().await;
    let rows = vec![
        named("pp", "py:py_provider"),
        named("gp", "go:go_provider"),
        js(&fixture, "je", "js_errors"),
    ];
    let report = fixture.loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture
        .expect("go_weather threw WeatherError Stormy too windy bigint 1152921504606846976")
        .await;
    fixture
        .expect("py_weather threw WeatherError Stormy too windy bigint 1152921504606846976")
        .await;
    fixture.root.shutdown().await.unwrap();
}

/// Three runtimes start cold with rows using each other's services: none
/// waits for another, every row starts, synchronous calls into Go that call
/// back into Node or Python complete.
/// guarantee: loader/runtimes-do-not-wait-for-each-other
#[tokio::test(flavor = "multi_thread")]
async fn three_runtimes_start_cold_and_use_each_other() {
    let fixture = fixture().await;
    let report = fixture
        .loader
        .reconcile(layer(all_rows(&fixture)), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.expect("go provider: start").await;
    fixture
        .expect("go consumer: js sunny / js later / py sunny / py later")
        .await;
    fixture
        .expect("js go consumer: go sunny / go later / MON,TUE")
        .await;
    fixture
        .expect("py go consumer: go sunny / go later / MON,TUE")
        .await;
    // Within one process, a service is the provider's own object.
    fixture.expect("go local: native").await;
    fixture.root.shutdown().await.unwrap();
}

/// A Go provider that goes stops exactly its users, in every language,
/// before it stops itself.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn a_go_provider_that_goes_stops_its_users_first() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let all = all_rows(&fixture);
    loader.reconcile(layer(all.clone()), None).await.unwrap();
    fixture
        .wait_until("every row to run", || {
            ["pp", "pg", "jp", "jg", "gp", "gc", "gl"]
                .iter()
                .all(|id| row_state(loader, id) == Some(FiberState::Active))
        })
        .await;
    let remaining: Vec<Value> = all.into_iter().filter(|row| row["id"] != "gp").collect();
    loader.reconcile(layer(remaining), None).await.unwrap();
    fixture.probe.wait_for("go provider: bye").await;
    fixture
        .wait_until("the users to wait", || {
            ["pg", "jg", "gl"]
                .iter()
                .all(|id| row_state(loader, id) == Some(FiberState::Pending))
        })
        .await;
    for id in ["pp", "jp", "gc"] {
        assert_eq!(
            row_state(loader, id),
            Some(FiberState::Active),
            "{id} keeps running"
        );
    }
    let provider_at = fixture.probe.position("go provider: bye").unwrap();
    for bye in [
        "py go consumer: bye",
        "js go consumer: bye",
        "go local: bye",
    ] {
        let at = fixture
            .probe
            .position(bye)
            .unwrap_or_else(|| panic!("{bye} missing: {:?}", fixture.probe.lines()));
        assert!(
            at < provider_at,
            "{bye} before the provider's: {:?}",
            fixture.probe.lines()
        );
    }
    fixture.root.shutdown().await.unwrap();
}

/// What a Go plugin injects gates it: it waits for the service, and stops
/// when it goes.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn injected_services_gate_a_go_plugin() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let transitions = common::Transitions::on(&fixture.root);
    loader
        .reconcile(layer(vec![named("gg", "go:go_gated")]), None)
        .await
        .unwrap();
    let first = loader.get("gg").unwrap().plugin.unwrap();
    // The state now; that it stays so until `llm` comes, `loads` checks below.
    assert_eq!(row_state(loader, "gg"), Some(FiberState::Pending));
    let llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    fixture.probe.wait_for("go gated: 42").await;
    // It did not start before `llm`: its fibers loaded once, for it. An
    // earlier start's Loading comes before the changes `loads` waits for
    // (one fiber's changes arrive in order).
    let fibers = [first, loader.get("gg").unwrap().plugin.unwrap()];
    assert_eq!(
        transitions.loads(&fibers).await,
        1,
        "{:?}",
        fibers.map(|fiber| transitions.of(fiber))
    );
    llm.dispose().await.unwrap();
    fixture.probe.wait_for("go gated: bye").await;
    fixture
        .wait_until("the row to wait again", || {
            row_state(loader, "gg") == Some(FiberState::Pending)
        })
        .await;
    fixture.root.shutdown().await.unwrap();
}
