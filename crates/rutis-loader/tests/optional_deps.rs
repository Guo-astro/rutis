//! Optional services of language plugins (`optional` beside `inject`):
//! never waited for; leased in when there as the row starts and kept until
//! it stops, as Rust's `Option<Arc<T>>` is a snapshot taken at load. A
//! provider that goes leaves the row running: a hand-written `HostDispatch`
//! keeps answering, a `#[rutis::service]` one and another runtime's row say
//! `service <name> is not available`. A name both required and optional is
//! the row's error.
#![cfg(feature = "runtimes")]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{
    greet_as_main, layer, repo, row_state, rows_stage, Llm, Probe, TestRuntime, COLD,
};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{call_service, host_key, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// A plugin using `SERVICE` when there: its service `config.as` answers
/// `of()` with what `SERVICE.METHOD()` returns, the error it throws, or
/// `none` when the service is not there.
const CONSUMER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['probe'],
  optional: ['SERVICE'],
  apply(ctx, config) {
    ctx.use('probe').record(`${config.as}: start`)
    ctx.provide(config.as, {
      async of() {
        const service = ctx.get('SERVICE')
        if (!service) return 'none'
        try { return String(await service.METHOD()) } catch (error) { return `${error.code}: ${error.message}` }
      },
    })
  },
})
"#;

/// Declares `llm` both required and optional.
const BOTH: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['llm'],
  optional: ['llm'],
  apply() {},
})
"#;

const PY_LLM: &str = r#"
class Llm:
    def ask(self):
        return "py 42"


provides = {"py_llm": Llm}


def apply(ctx, config):
    ctx.provide("py_llm", Llm())
"#;

/// Hands two plugin definitions to `defs`: `reader`, using `llm` when
/// there (its service `config.as` answers `of()` as [`CONSUMER`]'s does),
/// and `both`, declaring `llm` both required and optional.
const HANDS: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['defs'],
  async apply(ctx) {
    const defs = ctx.use('defs')
    await defs.register('reader', definePlugin({
      name: 'reader',
      optional: ['llm'],
      apply(ctx, config) {
        ctx.provide(config.as, {
          async of() {
            const llm = ctx.get('llm')
            return llm ? String(await llm.ask()) : 'none'
          },
        })
      },
    }))
    await defs.register('both', definePlugin({ name: 'both', inject: ['llm'], optional: ['llm'], apply() {} }))
  },
})
"#;

/// The plugin definitions rows hand over, by the name they give.
#[derive(Default)]
pub struct Defs {
    handed: std::sync::Mutex<Vec<(String, rutis::Definition)>>,
}

#[rutis::service]
impl Defs {
    pub fn register(&self, name: String, def: rutis::Definition) {
        self.handed.lock().unwrap().push((name, def));
    }
}

impl Defs {
    fn get(&self, name: &str) -> Option<rutis::Definition> {
        let handed = self.handed.lock().unwrap();
        handed
            .iter()
            .find(|(handed, _)| handed == name)
            .map(|(_, def)| def.clone())
    }
}

/// A hand-written service other than [`Llm`]: answers 43.
struct OtherLlm;

impl HostDispatch for OtherLlm {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "ask");
        Ok(json!("43").into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "ask": "sync" }))
    }
}

/// A `#[rutis::service]` type, served to the rows by name (`counter`).
#[derive(Default)]
pub struct Counter;

#[rutis::service]
impl Counter {
    pub fn count(&self) -> u32 {
        7
    }
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    js: PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let js = dir.path().join("js");
    let py = dir.path().join("py");
    std::fs::create_dir_all(&js).unwrap();
    std::fs::create_dir_all(&py).unwrap();
    std::fs::write(py.join("py_llm.py"), PY_LLM).unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    for (file, service, method) in [
        ("uses_llm", "llm", "ask"),
        ("uses_late", "late", "ask"),
        ("uses_counter", "counter", "count"),
        ("uses_py_llm", "py_llm", "ask"),
    ] {
        let text = CONSUMER
            .replace("PLUGIN", &sdk)
            .replace("SERVICE", service)
            .replace("METHOD", method);
        std::fs::write(js.join(format!("{file}.mjs")), text).unwrap();
    }
    std::fs::write(js.join("both.mjs"), BOTH.replace("PLUGIN", &sdk)).unwrap();
    std::fs::write(js.join("hands.mjs"), HANDS.replace("PLUGIN", &sdk)).unwrap();

    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(Probe::default()))
        .unwrap();
    let catalog = ServiceCatalog::new();
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
        js,
        _dir: dir,
    }
}

impl Fixture {
    /// The row `id` of the consumer `file`, providing `of_<id>`; `round`
    /// in its config, so a new round restarts it.
    fn consumer(&self, id: &str, file: &str, round: u32) -> Value {
        let path = self.js.join(format!("{file}.mjs"));
        json!({
            "id": id,
            "name": path.to_string_lossy(),
            "config": { "as": format!("of_{id}"), "round": round },
        })
    }

    fn row(&self, id: &str, file: &str, extra: Value) -> Value {
        let path = self.js.join(format!("{file}.mjs"));
        let mut row = json!({ "id": id, "name": path.to_string_lossy(), "config": {} });
        row.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        row
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

    /// What the row `id`'s `of()` answers, once its service is there.
    async fn of(&self, id: &str) -> String {
        let name = format!("of_{id}");
        COLD.until(
            || self.root.get_at(&host_key(&name)).is_some(),
            &format!("{name} to be provided"),
        )
        .await;
        call_service(&self.root, &name, "of", ()).await.unwrap()
    }
}

/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn optional_services_are_leased_at_start_and_never_waited_for() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    let rows = |round| {
        vec![
            fixture.consumer("with", "uses_llm", 1),
            fixture.consumer("without", "uses_late", round),
        ]
    };
    let report = loader.reconcile(layer(rows(1)), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    // Not waited for: both run, there or not.
    fixture.active(&["with", "without"]).await;
    let status = loader.status().to_string();
    assert!(!status.contains("waiting"), "{status}");
    assert_eq!(fixture.of("with").await, "42");
    assert_eq!(fixture.of("without").await, "none");

    // One that comes later is not leased: sure to be seen once the row restarts.
    let late = fixture
        .root
        .provide_at(host_key("late"), Arc::new(Llm))
        .unwrap();
    assert_eq!(fixture.of("without").await, "none");
    loader.reconcile(layer(rows(2)), None).await.unwrap();
    poll(
        || async { fixture.of("without").await == "42" },
        "the restarted row to see late",
    )
    .await;

    // A hand-written service that goes: the row keeps running, and keeps
    // the object it leased.
    llm.dispose().await.unwrap();
    assert_eq!(row_state(loader, "with"), Some(FiberState::Active));
    assert_eq!(fixture.of("with").await, "42");
    // Replaced: still the old one, until the row restarts.
    let _other = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(OtherLlm))
        .unwrap();
    assert_eq!(fixture.of("with").await, "42");

    late.dispose().await.unwrap();
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn a_marked_service_that_goes_or_is_replaced_is_not_available() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let counter = fixture.root.provide(Counter).unwrap();
    loader
        .reconcile(layer(vec![fixture.consumer("c", "uses_counter", 1)]), None)
        .await
        .unwrap();
    fixture.active(&["c"]).await;
    assert_eq!(fixture.of("c").await, "7");

    counter.dispose().await.unwrap();
    assert_eq!(row_state(loader, "c"), Some(FiberState::Active));
    assert_eq!(
        fixture.of("c").await,
        "unavailable: service counter is not available"
    );
    // Another provider since: the row still holds the one it leased.
    let _again = fixture.root.provide(Counter).unwrap();
    assert_eq!(
        fixture.of("c").await,
        "unavailable: service counter is not available"
    );
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn another_runtimes_row_that_stops_is_not_available() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let provider = json!({ "id": "p", "name": "py:py_llm", "config": {} });
    loader
        .reconcile(layer(vec![provider.clone()]), None)
        .await
        .unwrap();
    fixture.active(&["p"]).await;
    let consumer = fixture.consumer("c", "uses_py_llm", 1);
    loader
        .reconcile(layer(vec![provider, consumer.clone()]), None)
        .await
        .unwrap();
    fixture.active(&["c"]).await;
    assert_eq!(fixture.of("c").await, "py 42");

    // The provider's row stops; its process stays.
    loader.reconcile(layer(vec![consumer]), None).await.unwrap();
    COLD.until(|| loader.get("p").is_none(), "the provider's row to go")
        .await;
    assert_eq!(row_state(loader, "c"), Some(FiberState::Active));
    poll(
        || async { fixture.of("c").await == "unavailable: service py_llm is not available" },
        "the call to say py_llm is gone",
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// Another runtime's row replaced by a new one in the same process (its
/// config changed): the row that leased the old one at start, not
/// restarted, is told it is not available, and does not reach the new one.
/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn another_runtimes_row_that_is_replaced_is_not_available() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let provider =
        |round: u32| json!({ "id": "p", "name": "py:py_llm", "config": { "round": round } });
    loader
        .reconcile(layer(vec![provider(1)]), None)
        .await
        .unwrap();
    fixture.active(&["p"]).await;
    let consumer = fixture.consumer("c", "uses_py_llm", 1);
    loader
        .reconcile(layer(vec![provider(1), consumer.clone()]), None)
        .await
        .unwrap();
    fixture.active(&["c"]).await;
    assert_eq!(fixture.of("c").await, "py 42");
    let published = || fixture.root.get_at(&host_key("py_llm"));
    let old = published().unwrap();

    // The provider's row restarts with another config; its process stays.
    loader
        .reconcile(layer(vec![provider(2), consumer]), None)
        .await
        .unwrap();
    COLD.until(
        || {
            published()
                .is_some_and(|new| Arc::as_ptr(&new) as *const () != Arc::as_ptr(&old) as *const ())
        },
        "the new provider to be published",
    )
    .await;
    assert_eq!(
        call_service::<String>(&fixture.root, "py_llm", "ask", ())
            .await
            .unwrap(),
        "py 42"
    );
    assert_eq!(row_state(loader, "c"), Some(FiberState::Active));
    assert_eq!(
        fixture.of("c").await,
        "unavailable: service py_llm is not available"
    );
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn a_name_both_required_and_optional_fails_the_row() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    loader
        .reconcile(
            layer(vec![
                fixture.row("both", "both", json!({})),
                fixture.row(
                    "row",
                    "uses_llm",
                    json!({ "inject": ["llm"], "config": { "as": "of_row" } }),
                ),
            ]),
            None,
        )
        .await
        .unwrap();
    let both = fixture.js.join("both.mjs");
    COLD.until(
        || {
            let status = loader.status().to_string();
            status.contains(&format!(
                "both: unresolved: cannot load {:?}: llm is in both inject and optional; keep it in one",
                both.to_string_lossy()
            )) && status.contains(
                "row: unresolved: entry \"row\" rejected: llm is in the row's inject and the plugin's optional; keep it in one",
            )
        },
        &format!("both rows to fail: {}", loader.status()),
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// A copy of a plugin definition a row handed over (`DefinitionMounter`)
/// leases its optional services as a row does, never waiting for them; a
/// name in both its lists fails the mount.
/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_of_a_definition_leases_its_optional_services() {
    let fixture = fixture().await;
    let root = &fixture.root;
    root.provide(Defs::default()).unwrap();
    let defs = root.get::<Defs>().unwrap();
    fixture
        .loader
        .reconcile(layer(vec![fixture.row("hands", "hands", json!({}))]), None)
        .await
        .unwrap();
    let handed = |name: &'static str| {
        let defs = defs.clone();
        move || defs.get(name)
    };
    let reader = COLD
        .eventually(handed("reader"), "reader handed over")
        .await;
    let both = COLD.eventually(handed("both"), "both handed over").await;

    // Not there as the copy starts: it runs, and reads nothing.
    let _without = root.plugin(reader.at(root, json!({ "as": "of_without" })).unwrap());
    assert_eq!(fixture.of("without").await, "none");
    // There: leased in.
    let _llm = root.provide_at(host_key("llm"), Arc::new(Llm)).unwrap();
    let _with = root.plugin(reader.at(root, json!({ "as": "of_with" })).unwrap());
    assert_eq!(fixture.of("with").await, "42");

    let error = match both.at(root, json!({})) {
        Ok(_) => panic!("a copy declaring llm both required and optional mounted"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("llm is in both inject and optional; keep it in one"),
        "{error}"
    );
    root.shutdown().await.unwrap();
}

/// Wait until `check` answers true.
async fn poll<F: std::future::Future<Output = bool>>(mut check: impl FnMut() -> F, what: &str) {
    let deadline = tokio::time::Instant::now() + COLD.limit();
    while !check().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
