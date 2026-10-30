//! A runtime row's own `inject` (the row's, not its plugin's): rutis gates
//! the row on each name the catalog shares and leases it into the row's
//! runtime as it starts, as it does a name the plugin injects and as a Rust
//! row's `inject` gates that row (`combined_injects`). The row waits for
//! it (`waiting for <name>`), stops when it goes and starts again when it
//! is back; the plugin, which declares nothing, uses it.
#![cfg(feature = "runtimes")]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{
    greet_as_main, layer, repo, row_state, rows_stage, Llm, Probe, TestRuntime, COLD,
};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{call_service, host_key};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// Declares nothing but `probe`: its service `config.as` answers `of()`
/// with what `config.service`'s `ask()` returns, or the error it throws.
const JS_READER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['probe'],
  apply(ctx, config) {
    ctx.use('probe').record(`${config.as}: start`)
    ctx.provide(config.as, {
      async of() {
        try { return String(await ctx.use(config.service).ask()) } catch (error) { return `${error.code}: ${error.message}` }
      },
    })
  },
})
"#;

/// Declares nothing but `probe`: records what `config.service`'s `ask()`
/// returns as it starts.
const PY_READER: &str = r#"
inject = ["probe"]


def apply(ctx, config):
    ctx.use("probe").record(f"{config['as']}: {ctx.use(config['service']).ask()}")
"#;

const PY_LLM: &str = r#"
class Llm:
    def ask(self):
        return "py 42"


provides = {"py_llm": Llm}


def apply(ctx, config):
    ctx.provide("py_llm", Llm())
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
    let js = dir.path().join("js");
    let py = dir.path().join("py");
    std::fs::create_dir_all(&js).unwrap();
    std::fs::create_dir_all(&py).unwrap();
    std::fs::write(py.join("py_reader.py"), PY_READER).unwrap();
    std::fs::write(py.join("py_llm.py"), PY_LLM).unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    std::fs::write(js.join("reader.mjs"), JS_READER.replace("PLUGIN", &sdk)).unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    // Every name is shared by name but `clock`, registered as the
    // application's own.
    let mut catalog = ServiceCatalog::new();
    catalog.register::<String>("clock");
    greet_as_main(&root);
    let node = TestRuntime::node(&repo().join("node/rutis-runtime/package.json"));
    let python = TestRuntime::python(&py);
    let sessions = Arc::new(SessionResolver::npm(&catalog, "node"));
    sessions.runtime(node.handle());
    sessions.runtime(python.handle());
    root.plugin(node);
    root.plugin(python);
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
    rows_stage(&root, &sessions, "node").await;
    rows_stage(&root, &sessions, "py").await;
    Fixture {
        root,
        loader,
        probe,
        js,
        _dir: dir,
    }
}

impl Fixture {
    /// The Node row `id` reading `service`, its row's `extra` (`inject`,
    /// `isolate`) added.
    fn js_reader(&self, id: &str, service: &str, extra: Value) -> Value {
        let path = self.js.join("reader.mjs");
        let mut row = json!({
            "id": id,
            "name": path.to_string_lossy(),
            "config": { "as": format!("of_{id}"), "service": service },
        });
        row.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        row
    }

    /// The Python row `id` reading `service`, its row's `inject` `inject`.
    fn py_reader(&self, id: &str, service: &str, inject: &[&str]) -> Value {
        json!({
            "id": id,
            "name": "py:py_reader",
            "config": { "as": id, "service": service },
            "inject": inject,
        })
    }

    async fn active(&self, ids: &[&str]) {
        COLD.until(
            || {
                ids.iter()
                    .all(|id| row_state(&self.loader, id) == Some(FiberState::Active))
            },
            &format!("{ids:?} to run: {}", self.loader.status()),
        )
        .await;
    }

    /// Wait until the row `id` waits for `service` alone (it may first wait
    /// for its runtime too, while that starts) and is not running.
    async fn waits_for(&self, id: &str, service: &str) {
        COLD.until(
            || {
                let Some(entry) = self.loader.entries().into_iter().find(|e| e.id == id) else {
                    return false;
                };
                let waits = self.loader.waits(&entry);
                !waits.is_empty()
                    && waits.iter().all(|wait| wait.service == service)
                    && row_state(&self.loader, id) != Some(FiberState::Active)
            },
            &format!("{id} to wait for {service}: {}", self.loader.status()),
        )
        .await;
    }

    /// What the Node row `id`'s `of()` answers, once its service is there.
    async fn of(&self, id: &str) -> String {
        let name = format!("of_{id}");
        COLD.until(
            || self.root.get_at(&host_key(&name)).is_some(),
            &format!("{name} to be provided: {}", self.loader.status()),
        )
        .await;
        call_service(&self.root, &name, "of", ()).await.unwrap()
    }

    fn starts(&self, id: &str) -> usize {
        let line = format!("of_{id}: start");
        self.probe.lines().iter().filter(|l| **l == line).count()
    }
}

/// A Node row and a Python row whose plugins declare nothing, their rows
/// `inject: ["llm"]`: both wait for `llm`, start once the host provides it
/// and use it, stop when it goes and start again when it is back.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_rows_inject_gates_it_and_is_leased_to_it() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let rows = vec![
        fixture.js_reader("n", "llm", json!({ "inject": ["llm"] })),
        fixture.py_reader("p", "llm", &["llm"]),
    ];
    let report = loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.waits_for("n", "llm").await;
    fixture.waits_for("p", "llm").await;
    assert_eq!(fixture.starts("n"), 0);

    let llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    fixture.active(&["n", "p"]).await;
    assert_eq!(fixture.of("n").await, "42");
    fixture.probe.wait_for("p: 42").await;

    // It goes: the rows stop and wait again; back, they start again.
    llm.dispose().await.unwrap();
    fixture.waits_for("n", "llm").await;
    fixture.waits_for("p", "llm").await;
    let _llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    fixture.active(&["n", "p"]).await;
    assert_eq!(fixture.of("n").await, "42");
    assert_eq!(fixture.starts("n"), 2);
    fixture.root.shutdown().await.unwrap();
}

/// A Node row's `inject` names a service a Python row provides: leased into
/// the Node process for it, so its Cordis gate there passes.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_rows_inject_reaches_another_runtimes_service() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let rows = vec![
        json!({ "id": "pl", "name": "py:py_llm", "config": {} }),
        fixture.js_reader("n", "py_llm", json!({ "inject": ["py_llm"] })),
    ];
    let report = fixture.loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.active(&["pl", "n"]).await;
    assert_eq!(fixture.of("n").await, "py 42");
    fixture.root.shutdown().await.unwrap();
}

/// A name the row both isolates and injects is the isolate's: a row of that
/// isolate gets it from a provider there; a row outside waits for it.
/// guarantee: loader/isolate-and-inject
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_rows_inject_is_leased_in_its_isolate() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let team = json!({ "py_llm": "team" });
    let rows = vec![
        json!({ "id": "pl", "name": "py:py_llm", "config": {}, "isolate": team }),
        fixture.js_reader(
            "inside",
            "py_llm",
            json!({ "inject": ["py_llm"], "isolate": team }),
        ),
        fixture.js_reader("outside", "py_llm", json!({ "inject": ["py_llm"] })),
    ];
    let report = fixture.loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.active(&["pl", "inside"]).await;
    assert_eq!(fixture.of("inside").await, "py 42");
    fixture.waits_for("outside", "py_llm").await;
    fixture.root.shutdown().await.unwrap();
}

/// A row given an `inject` it did not have stops, waits for the name, and
/// starts once when it is there.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn adding_to_a_runtime_rows_inject_restarts_it_once() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let transitions = common::Transitions::on(&fixture.root);
    loader
        .reconcile(layer(vec![fixture.js_reader("n", "llm", json!({}))]), None)
        .await
        .unwrap();
    fixture.active(&["n"]).await;
    assert_eq!(fixture.starts("n"), 1);
    let first = loader.get("n").unwrap().plugin.unwrap();
    let before = transitions.plugins();
    loader
        .reconcile(
            layer(vec![fixture.js_reader(
                "n",
                "llm",
                json!({ "inject": ["llm"] }),
            )]),
            None,
        )
        .await
        .unwrap();
    fixture.waits_for("n", "llm").await;
    let _llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    fixture.active(&["n"]).await;
    assert_eq!(fixture.of("n").await, "42");
    assert_eq!(fixture.starts("n"), 2);
    // The row came back once: of the fibers since (its new one and those
    // its start brings along), none went away, so none came and went while
    // it waited (the gate keeps a waiting fiber from loading, so starts
    // alone would not tell); and its two fibers loaded once each.
    let second = loader.get("n").unwrap().plugin.unwrap();
    let new: Vec<_> = transitions
        .plugins()
        .into_iter()
        .filter(|plugin| !before.contains(plugin))
        .map(|plugin| (plugin, transitions.of(plugin)))
        .collect();
    assert!(new.iter().any(|(plugin, _)| *plugin == second), "{new:?}");
    assert!(
        new.iter()
            .all(|(_, states)| !states.contains(&FiberState::Disposed)),
        "{new:?}"
    );
    assert_eq!(transitions.loads(&[first, second]).await, 2);
    fixture.root.shutdown().await.unwrap();
}

/// A name of a runtime row's own `inject` the catalog does not share (one
/// the application registered for a Rust type) neither gates the row in
/// rutis nor is leased to it, and is left to the runtime: the Python row
/// starts and its `use` says the service is not available; the Node row
/// runs in rutis while the Cordis gate in its process waits for the name,
/// so its plugin does not apply.
/// guarantee: loader/isolate-and-inject
#[tokio::test(flavor = "multi_thread")]
async fn a_name_the_catalog_does_not_share_is_left_to_the_runtime() {
    if !rutis_test_support::runtimes::all_enabled(&["node", "python"]) {
        return;
    }
    let fixture = fixture().await;
    let loader = &fixture.loader;
    loader
        .reconcile(
            layer(vec![
                fixture.py_reader("p", "clock", &["clock"]),
                fixture.js_reader("n", "clock", json!({ "inject": ["clock"] })),
            ]),
            None,
        )
        .await
        .unwrap();
    let failure = COLD
        .eventually_with(
            || match loader.get("p")?.status {
                EntryStatus::Running(snapshot) if snapshot.state == FiberState::Failed => {
                    Some(snapshot.error.map(|e| e.to_string()).unwrap_or_default())
                }
                _ => None,
            },
            "p to fail",
            || loader.status().to_string(),
        )
        .await;
    assert!(
        failure.contains("service clock is not available"),
        "{failure}"
    );
    fixture.active(&["n"]).await;
    for id in ["p", "n"] {
        let entry = loader.entries().into_iter().find(|e| e.id == id).unwrap();
        assert!(loader.waits(&entry).is_empty(), "{id}: {}", loader.status());
    }
    assert_eq!(fixture.starts("n"), 0);
    fixture.root.shutdown().await.unwrap();
}
