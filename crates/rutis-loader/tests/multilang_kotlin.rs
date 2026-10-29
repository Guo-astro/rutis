//! The runtime conformance suite with Kotlin (design
//! docs/design-android-runtime-2026-10-12.md §九, the `multilang` Kotlin
//! column): leaf plugins of the Kotlin SDK's jvm target (`Rutis.serve` in
//! the conformance jar, started through the generic launcher), JavaScript
//! and Python start cold together and use each other's services; Node and
//! Python call Kotlin synchronously while Kotlin calls back into them; a
//! provider that goes stops its users first; what a Kotlin plugin injects
//! gates it; a service it does not declare is announced; and a cycle of
//! synchronous calls through Kotlin is refused with `SyncWaitCycle`, the
//! rest completing.
#![cfg(all(feature = "node", feature = "python", feature = "kotlin"))]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use common::runtimes::{
    greet_as_main, kotlin_launcher, layer, repo, row_state, rows_stage, Llm, Probe, TestRuntime,
    COLD,
};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{host_key, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

const PY_PROVIDER: &str = r#"
class Weather:
    def today(self):
        return "py sunny"

    async def later(self):
        return "py later"


provides = {"py_weather": Weather}
inject = ["probe"]


def apply(ctx, config):
    ctx.provide("py_weather", Weather())
"#;

const PY_KT_CONSUMER: &str = r#"
inject = ["kt_weather", "probe"]


async def apply(ctx, config):
    probe = ctx.use("probe")
    weather = ctx.use("kt_weather")
    days = weather.each(["mon", "tue"], lambda day: day.upper())
    probe.record(f"py kt consumer: {weather.today()} / {await weather.later()} / {','.join(days)}")
    return lambda: probe.record("py kt consumer: bye")
"#;

const JS_PROVIDER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { js_weather: { today: 'sync', later: 'async' } },
  apply(ctx) {
    ctx.provide('js_weather', { today: () => 'js sunny', later: async () => 'js later' })
  },
})
"#;

const JS_KT_CONSUMER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['kt_weather', 'probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    const weather = ctx.use('kt_weather')
    // A synchronous call into Kotlin that calls back into this process.
    const days = weather.each(['mon', 'tue'], (day) => day.toUpperCase())
    probe.record(`js kt consumer: ${weather.today()} / ${await weather.later()} / ${days.join(',')}`)
    return () => probe.record('js kt consumer: bye')
  },
})
"#;

/// Uses the service the Kotlin plugin provides without declaring it.
const JS_FORECAST: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['kt_forecast', 'probe'],
  async apply(ctx) {
    ctx.use('probe').record(`js forecast: ${await ctx.use('kt_forecast').tomorrow()}`)
  },
})
"#;

/// Calls Kotlin's `fail`, recording what it throws.
const JS_ERRORS: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['kt_weather', 'probe'],
  async apply(ctx) {
    try {
      await ctx.use('kt_weather').fail()
    } catch (error) {
      ctx.use('probe').record(`kt_weather threw ${error.name} ${error.code} ${error.message} ${typeof error.data.wind} ${error.data.wind}`)
    }
  },
})
"#;

fn plugin_url() -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    url::Url::from_file_path(&sdk).unwrap().to_string()
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text.replace("PLUGIN", &plugin_url())).unwrap();
    path
}

fn node(name: &str) -> TestRuntime {
    TestRuntime::node(&repo().join("node/rutis-runtime/package.json")).named(name)
}

/// The Kotlin runtime `kt` (rows `kt:<plugin>`), in `project`.
fn kotlin(project: &Path) -> TestRuntime {
    std::fs::create_dir_all(project).unwrap();
    TestRuntime::new("kt", kotlin_launcher(project), project)
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    dir: tempfile::TempDir,
}

/// A loader over `runtimes` (`<name>:<plugin>` rows; the one marked `true`
/// also runs the npm names and paths, Node's way), with the host services `probe` and `extra` and the shared
/// `services`.
async fn fixture(
    dir: tempfile::TempDir,
    runtimes: Vec<(TestRuntime, bool)>,
    services: &[&str],
    extra: Vec<(&str, Arc<dyn HostDispatch>)>,
) -> Fixture {
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    for (name, service) in extra {
        root.provide_at(host_key(name), service).unwrap();
    }
    let mut catalog = ServiceCatalog::new();
    for name in services {
        catalog.register_shared(*name);
    }
    let npm = runtimes
        .iter()
        .find(|(_, npm)| *npm)
        .map(|(runtime, _)| runtime.handle().name().to_owned());
    let sessions = Arc::new(match &npm {
        Some(npm) => SessionResolver::npm(&catalog, npm),
        None => SessionResolver::new(&catalog),
    });
    let mut names = Vec::new();
    for (runtime, _) in runtimes {
        names.push(runtime.handle().name().to_owned());
        sessions.runtime(runtime.handle());
        root.plugin(runtime);
    }
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
    for name in &names {
        rows_stage(&root, &sessions, name).await;
    }
    Fixture {
        root,
        loader,
        probe,
        dir,
    }
}

/// Node, Python and Kotlin, with the plugins of the cases written.
async fn three() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let py = dir.path().join("py");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::write(py.join("py_provider.py"), PY_PROVIDER).unwrap();
    std::fs::write(py.join("py_kt_consumer.py"), PY_KT_CONSUMER).unwrap();
    let js = dir.path().join("js");
    std::fs::create_dir_all(&js).unwrap();
    for (file, text) in [
        ("js_provider.mjs", JS_PROVIDER),
        ("js_kt_consumer.mjs", JS_KT_CONSUMER),
        ("js_forecast.mjs", JS_FORECAST),
        ("js_errors.mjs", JS_ERRORS),
    ] {
        write(&js, file, text);
    }
    let python = TestRuntime::python(&py);
    let kt = kotlin(&dir.path().join("kt"));
    let services = [
        "probe",
        "llm",
        "py_weather",
        "js_weather",
        "kt_weather",
        "kt_forecast",
    ];
    fixture(
        dir,
        vec![(node("node"), true), (python, false), (kt, false)],
        &services,
        Vec::new(),
    )
    .await
}

impl Fixture {
    fn js(&self, id: &str, file: &str) -> Value {
        let path = self.dir.path().join("js").join(format!("{file}.mjs"));
        json!({ "id": id, "name": path.to_string_lossy(), "config": {} })
    }

    /// Wait for `line`; on a timeout, say where every row is. Three
    /// runtimes start cold together (#238): three times HANG_GUARD.
    async fn expect(&self, line: &str) {
        self.wait_until(&format!("{line:?} to be recorded"), || {
            self.probe.position(line).is_some()
        })
        .await;
    }

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
                    format!("recorded: {:?}\nrows: {rows:#?}", self.probe.lines())
                },
            )
            .await
    }
}

fn named(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "config": {} })
}

fn all_rows(fixture: &Fixture) -> Vec<Value> {
    vec![
        named("pp", "py:py_provider"),
        named("pk", "py:py_kt_consumer"),
        fixture.js("jp", "js_provider"),
        fixture.js("jk", "js_kt_consumer"),
        named("kp", "kt:kt_provider"),
        named("kc", "kt:kt_consumer"),
        named("kl", "kt:kt_local"),
    ]
}

/// Three runtimes start cold with rows using each other's services: none
/// waits for another, every row starts, synchronous calls into Kotlin that
/// call back into Node or Python complete, and within the Kotlin process a
/// service is the provider's own object.
/// guarantee: loader/runtimes-do-not-wait-for-each-other
#[tokio::test(flavor = "multi_thread")]
async fn three_runtimes_with_kotlin_start_cold_and_use_each_other() {
    let fixture = three().await;
    let report = fixture
        .loader
        .reconcile(layer(all_rows(&fixture)), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.expect("kt provider: start").await;
    fixture
        .expect("kt consumer: js sunny / js later / py sunny / py later")
        .await;
    fixture
        .expect("js kt consumer: kt sunny / kt later / MON,TUE")
        .await;
    fixture
        .expect("py kt consumer: kt sunny / kt later / MON,TUE")
        .await;
    fixture.expect("kt local: native / kt later").await;
    fixture.root.shutdown().await.unwrap();
}

/// A service a Kotlin plugin provides without declaring it is announced
/// (its methods all async) and used by name from JavaScript; an error with
/// a code and data reaches a JavaScript caller with both, a big integer in
/// the data as a bigint (#440).
/// guarantee: bridge/error-code-and-data
#[tokio::test(flavor = "multi_thread")]
async fn an_undeclared_service_and_an_error_with_code_and_data_cross_from_kotlin() {
    let fixture = three().await;
    let rows = vec![
        named("kp", "kt:kt_provider"),
        named("kf", "kt:kt_forecast"),
        fixture.js("jf", "js_forecast"),
        fixture.js("je", "js_errors"),
    ];
    let report = fixture.loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.expect("js forecast: kt tomorrow").await;
    fixture
        .expect("kt_weather threw WeatherError Stormy too windy bigint 1152921504606846976")
        .await;
    fixture.root.shutdown().await.unwrap();
}

/// A Kotlin provider that goes stops exactly its users, in every language,
/// before it stops itself.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn a_kotlin_provider_that_goes_stops_its_users_first() {
    let fixture = three().await;
    let loader = &fixture.loader;
    let all = all_rows(&fixture);
    loader.reconcile(layer(all.clone()), None).await.unwrap();
    let ids = ["pp", "pk", "jp", "jk", "kp", "kc", "kl"];
    fixture
        .wait_until("every row to run", || {
            ids.iter()
                .all(|id| row_state(loader, id) == Some(FiberState::Active))
        })
        .await;
    let remaining: Vec<Value> = all.into_iter().filter(|row| row["id"] != "kp").collect();
    loader.reconcile(layer(remaining), None).await.unwrap();
    fixture.probe.wait_for("kt provider: bye").await;
    fixture
        .wait_until("the users to wait", || {
            ["pk", "jk", "kl"]
                .iter()
                .all(|id| row_state(loader, id) == Some(FiberState::Pending))
        })
        .await;
    for id in ["pp", "jp", "kc"] {
        assert_eq!(
            row_state(loader, id),
            Some(FiberState::Active),
            "{id} keeps running"
        );
    }
    let provider_at = fixture.probe.position("kt provider: bye").unwrap();
    for bye in [
        "py kt consumer: bye",
        "js kt consumer: bye",
        "kt local: bye",
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

/// What a Kotlin plugin injects gates it: it waits for the service, and
/// stops when it goes.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn injected_services_gate_a_kotlin_plugin() {
    let fixture = three().await;
    let loader = &fixture.loader;
    loader
        .reconcile(layer(vec![named("kg", "kt:kt_gated")]), None)
        .await
        .unwrap();
    assert_eq!(row_state(loader, "kg"), Some(FiberState::Pending));
    assert!(!fixture
        .probe
        .lines()
        .iter()
        .any(|l| l.starts_with("kt gated")));
    let llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    fixture.probe.wait_for("kt gated: 42").await;
    llm.dispose().await.unwrap();
    fixture.probe.wait_for("kt gated: bye").await;
    fixture
        .wait_until("the row to wait again", || {
            row_state(loader, "kg") == Some(FiberState::Pending)
        })
        .await;
    fixture.root.shutdown().await.unwrap();
}

// ── Synchronous calls waiting on each other through Kotlin ───────

/// What each Node runtime does for the other.
const NODE_WORK: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { SELF: { work: 'sync' } },
  apply(ctx) {
    ctx.provide('SELF', { work: () => 'SELF works' })
  },
})
"#;

/// `go` waits at the gate until the other runtime's `go` arrives too, then
/// calls OTHER's `work` synchronously.
const NODE_GO_PEER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['gate', 'OTHER'],
  provides: { SELF: { go: 'sync' } },
  apply(ctx) {
    const gate = ctx.use('gate')
    ctx.provide('SELF', {
      go() {
        gate.arrive('SELF')
        try { return `SELF got ${ctx.use('OTHER').work()}` } catch (error) { return `SELF: ${error.name}` }
      },
    })
  },
})
"#;

/// Blocks each arrival until `parties` arrived: the two calls then cross.
struct Gate {
    state: Mutex<usize>,
    changed: Condvar,
    parties: usize,
}

impl HostDispatch for Gate {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "arrive");
        let mut arrived = self.state.lock().unwrap();
        *arrived += 1;
        self.changed.notify_all();
        let (arrived, timeout) = self
            .changed
            .wait_timeout_while(arrived, COLD.limit(), |arrived| *arrived < self.parties)
            .unwrap();
        drop(arrived);
        assert!(!timeout.timed_out(), "the gate never opened");
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "arrive": "sync" }))
    }
}

fn service(root: &Ctx, name: &str) -> Option<Arc<dyn HostDispatch>> {
    root.get_at::<dyn HostDispatch>(&host_key(name))
}

/// Call `method` of `name` synchronously, on a thread of its own.
fn call(root: &Ctx, name: &str, method: &str) -> std::sync::mpsc::Receiver<String> {
    let service = service(root, name).unwrap_or_else(|| panic!("no service {name}"));
    let method = method.to_owned();
    let (done, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reply = service
            .invoke(&method, RpcValue::List(Vec::new()))
            .map_err(|error| error.to_string())
            .and_then(|value| value.json().map_err(|error| error.to_string()));
        let _ = done.send(match reply {
            Ok(value) => value.as_str().unwrap_or_default().to_owned(),
            Err(error) => format!("failed: {error}"),
        });
    });
    result
}

/// Two Node runtimes waiting on each other through Kotlin: `a` calls the
/// Kotlin relay, which calls `b_work` in the other runtime, while `b` calls
/// `a_work` back. Kotlin runs every call on a thread of its own, and its
/// synchronous calls say they wait (`sync-wait`): the host sees the cycle
/// and refuses the call that closes it, and the rest completes.
/// guarantee: bridge/sync-wait-cycle-refused
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_node_runtimes_waiting_on_each_other_through_kotlin() {
    let dir = tempfile::tempdir().unwrap();
    let work = |me: &str| NODE_WORK.replace("SELF", me);
    let go = |me: &str, other: &str| NODE_GO_PEER.replace("SELF", me).replace("OTHER", other);
    let files = [
        ("aw", "node", write(dir.path(), "aw.mjs", &work("a_work"))),
        ("bw", "nb", write(dir.path(), "bw.mjs", &work("b_work"))),
        (
            "a",
            "node",
            write(dir.path(), "a.mjs", &go("a_svc", "g_relay")),
        ),
        (
            "b",
            "nb",
            write(dir.path(), "b.mjs", &go("b_svc", "a_work")),
        ),
    ];
    let kt = kotlin(&dir.path().join("kt"));
    let services = ["gate", "a_work", "b_work", "a_svc", "b_svc", "g_relay"];
    let gate = Arc::new(Gate {
        state: Mutex::new(0),
        changed: Condvar::new(),
        parties: 2,
    });
    let fixture = fixture(
        dir,
        vec![(node("node"), true), (node("nb"), false), (kt, false)],
        &services,
        vec![("gate", gate)],
    )
    .await;
    let mut rows: Vec<Value> = files
        .iter()
        .map(|(id, runtime, file)| match *runtime {
            "node" => json!({ "id": id, "name": file.to_string_lossy() }),
            runtime => json!({ "id": id, "name": format!("{runtime}:{}", file.display()) }),
        })
        .collect();
    rows.push(named("g", "kt:kt_relay"));
    fixture.loader.reconcile(layer(rows), None).await.unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["aw", "bw", "a", "b", "g"]
                .iter()
                .all(|id| row_state(&loader, id) == Some(FiberState::Active))
                && services.iter().all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;
    let a_go = call(&fixture.root, "a_svc", "go");
    let b_go = call(&fixture.root, "b_svc", "go");
    let answer = |result: std::sync::mpsc::Receiver<String>, what: &str| {
        result
            .recv_timeout(COLD.limit())
            .unwrap_or_else(|_| panic!("{what} hangs: the runtimes wait on each other"))
    };
    let answers = [answer(a_go, "a.go"), answer(b_go, "b.go")];
    assert!(
        answers
            == [
                "a_svc got kt_relay relays b_work works",
                "b_svc: SyncWaitCycle"
            ]
            || answers
                == [
                    "a_svc got kt_relay: SyncWaitCycle",
                    "b_svc got a_work works"
                ],
        "exactly one call is refused, and the other completes: {answers:?}"
    );
    fixture.root.shutdown().await.unwrap();
}
