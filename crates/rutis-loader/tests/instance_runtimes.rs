//! Shared service names inside instances, across languages: each instance's
//! TS and Python rows use and provide that instance's services, though
//! every instance's rows share one process per language.
#![cfg(all(feature = "node", feature = "python"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::{BoxFuture, CordisError, Ctx, Effect, FiberState, Plugin, PluginFactory, TypeKey};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{
    host_key, host_key_in, settle, HostDispatch, Reply, Value as RpcValue,
};
use rutis_loader::{
    Build, Builtins, Chain, EntryInfo, EntryStatus, Instance, InstanceResult, Layer, Loader,
    LoaderOptions, LoaderPlugin, Patch, RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

// ── The language rows ───────────────────────────────────────────

/// Uses the instance's `tools`, and provides the instance's `py_timeline`.
const PY_ROW: &str = r#"
inject = ["tools", "probe"]


class Timeline:
    def __init__(self, title):
        self._title = title

    async def title(self):
        return "py " + self._title


provides = {"py_timeline": Timeline}


def apply(ctx, config):
    title = ctx.use("tools").title()
    ctx.use("probe").record(f"py sees {title}")
    ctx.provide("py_timeline", Timeline(title))
"#;

const JS_ROW: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['tools', 'probe'],
  provides: { js_timeline: { title: 'async' } },
  apply(ctx) {
    const title = ctx.use('tools').title()
    ctx.use('probe').record(`js sees ${title}`)
    ctx.provide('js_timeline', { async title() { return `js ${title}` } })
  },
})
"#;

// ── Rust plugins ────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<String>>>);

impl HostDispatch for Probe {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        assert_eq!(method, "record");
        let [line]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "record": "sync" }))
    }
}

impl Probe {
    fn has(&self, line: &str) -> bool {
        self.0.lock().unwrap().iter().any(|l| l == line)
    }

    async fn wait_for(&self, line: &str) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while !self.has(line) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{line:?} not recorded: {:?}", self.0.lock().unwrap()));
    }
}

/// The instance's `tools`: tells its instance's title.
struct Tools(String);

impl HostDispatch for Tools {
    fn invoke(&self, method: &str, _: RpcValue) -> Reply {
        assert_eq!(method, "title");
        Ok(json!(self.0).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "title": "sync" }))
    }
}

/// The instance's title, given with `with`.
struct Title(String);

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
struct NoConfig {}

enum Job {
    /// Provide `Tools` at this key.
    Provide(TypeKey),
    /// Read both timelines at these keys and record them.
    Read { py: TypeKey, js: TypeKey },
}

struct Job1 {
    job: Arc<Job>,
    title: String,
    injects: Vec<TypeKey>,
}

impl Plugin for Job1 {
    fn name(&self) -> &str {
        "job"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            match &*self.job {
                Job::Provide(key) => {
                    ctx.provide_as::<dyn HostDispatch>(
                        key.clone(),
                        Arc::new(Tools(self.title.clone())),
                    )?;
                }
                Job::Read { py, js } => {
                    let mut seen = Vec::new();
                    for key in [py, js] {
                        let timeline = ctx.require_as::<dyn HostDispatch>(key.clone())?;
                        let call = timeline
                            .invoke("title", RpcValue::List(Vec::new()))
                            .map_err(|e| CordisError::PluginFailed(e.to_string().into()))?;
                        let title = settle(call)
                            .await
                            .and_then(RpcValue::json)
                            .map_err(|e| CordisError::PluginFailed(e.to_string().into()))?;
                        seen.push(title.as_str().unwrap_or_default().to_owned());
                    }
                    let probe = ctx.require_as::<dyn HostDispatch>(host_key("probe"))?;
                    let line = format!("rust in {} reads {}", self.title, seen.join(" / "));
                    probe
                        .invoke("record", RpcValue::List(vec![json!(line).into()]))
                        .map_err(|e| CordisError::PluginFailed(e.to_string().into()))?;
                }
            }
            Ok(Effect::Done)
        })
    }
}

struct JobFactory {
    job: Arc<Job>,
    title: String,
    injects: Vec<TypeKey>,
}

impl PluginFactory<NoConfig> for JobFactory {
    fn name(&self) -> &str {
        "job"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn build(&self, _: &NoConfig) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(Job1 {
            job: self.job.clone(),
            title: self.title.clone(),
            injects: self.injects.clone(),
        }))
    }
}

fn title(build: &Build) -> String {
    build
        .value::<Title>()
        .map(|t| t.0.clone())
        .unwrap_or_default()
}

fn builtins() -> Builtins {
    let mut builtins = Builtins::new();
    builtins.register_with::<NoConfig, _, _>("tools", |build| {
        let instance = build.instance("session")?;
        Ok(JobFactory {
            job: Arc::new(Job::Provide(host_key_in("tools", instance))),
            title: title(build),
            injects: Vec::new(),
        })
    });
    builtins.register_with::<NoConfig, _, _>("reader", |build| {
        let instance = build.instance("session")?;
        let py = host_key_in("py_timeline", instance);
        let js = host_key_in("js_timeline", instance);
        Ok(JobFactory {
            injects: vec![py.clone(), js.clone(), host_key("probe")],
            job: Arc::new(Job::Read { py, js }),
            title: title(build),
        })
    });
    builtins
}

// ── Harness ─────────────────────────────────────────────────────

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    /// The rows as reconciled.
    rows: Value,
    /// The TS row's file.
    js: PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let py = dir.path().join("py");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::write(py.join("py_row.py"), PY_ROW).unwrap();
    let plugin = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let js = dir.path().join("js_row.mjs");
    std::fs::write(
        &js,
        JS_ROW.replace(
            "PLUGIN",
            url::Url::from_file_path(&plugin).unwrap().as_str(),
        ),
    )
    .unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe");
    for name in ["tools", "py_timeline", "js_timeline"] {
        catalog.register_shared_instance(name, "session");
    }
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let python = LocalRuntime::python(&py).python_path(repo().join("python/rutis"));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    root.plugin(node);
    root.plugin(python);
    let chain = Chain::new()
        .with(builtins())
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
    root.plugin(RuntimeRowsPlugin::new(node_rows));
    root.plugin(RuntimeRowsPlugin::new(python_rows));

    let rows = json!([{ "insert": [
        { "id": "session", "group": true, "instanced": true, "config": [
            { "id": "tools", "name": "tools" },
            { "id": "py", "name": "py:py_row" },
            { "id": "js", "name": js.to_string_lossy() },
            { "id": "reader", "name": "reader" }
        ] }
    ] }]);
    let patches: Vec<Patch> = serde_json::from_value(rows.clone()).unwrap();
    let report = loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    Fixture {
        root,
        loader,
        probe,
        rows,
        js,
        _dir: dir,
    }
}

impl Fixture {
    async fn create(&self, title: &str) -> Instance {
        let instance = self
            .loader
            .create_instance(&self.root, "session")
            .with(Title(title.into()))
            .await
            .unwrap();
        // The reader waits for both timelines, which the rows publish once
        // they started.
        self.probe
            .wait_for(&format!("rust in {title} reads py {title} / js {title}"))
            .await;
        instance
    }

    fn copies(&self, id: &str) -> Vec<EntryInfo> {
        self.loader
            .entries()
            .into_iter()
            .filter(|e| e.id == id && e.instance.is_some())
            .collect()
    }
}

fn active(entry: &EntryInfo) -> bool {
    matches!(&entry.status, EntryStatus::Running(s) if s.state == FiberState::Active)
}

// ── The suite ───────────────────────────────────────────────────

/// Two instances, the same names: each instance's TS and Python rows see
/// that instance's Rust `tools`, and its Rust reader reads that instance's
/// rows' timelines, through one process per language.
#[tokio::test(flavor = "multi_thread")]
async fn each_instance_sees_its_own_services_across_languages() {
    let fixture = fixture().await;
    let a = fixture.create("A").await;
    let b = fixture.create("B").await;
    for line in ["py sees A", "js sees A", "py sees B", "js sees B"] {
        fixture.probe.wait_for(line).await;
    }
    for instance in [&a, &b] {
        assert!(
            instance
                .report
                .iter()
                .all(|(_, r)| matches!(r, InstanceResult::Active | InstanceResult::Waiting)),
            "{:?}",
            instance.report
        );
    }
    assert!(!fixture.probe.has("rust in A reads py B / js B"));
    fixture.root.shutdown().await.unwrap();
}

/// Removing an instance withdraws its services everywhere: nothing of it is
/// left in rutis or in the runtimes, the other instance keeps running, and
/// a new instance registers the same names again.
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_instance_leaves_nothing_behind() {
    let fixture = fixture().await;
    let a = fixture.create("A").await;
    fixture.create("B").await;
    let a_instance = a.view.instance();
    fixture.loader.remove_instance(a.plugin).await.unwrap();
    let left: Vec<_> = fixture
        .root
        .diagnostics()
        .bindings
        .into_iter()
        .filter(|b| b.key.instance_id() == Some(a_instance))
        .map(|b| b.key.describe())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    for id in ["tools", "py", "js", "reader"] {
        let copies = fixture.copies(id);
        assert_eq!(copies.len(), 1, "{id}");
        assert!(active(&copies[0]), "{id}: {:?}", copies[0].status);
    }
    // The runtimes forgot A's registrations: a new instance takes the same
    // names in them again.
    fixture.create("C").await;
    fixture.root.shutdown().await.unwrap();
}

/// A language row outside the group of an instance name it uses is
/// unresolved, saying where it belongs, though the loader leaves its scope
/// to the runtime.
#[tokio::test(flavor = "multi_thread")]
async fn a_language_row_outside_its_instances_is_unresolved() {
    let fixture = fixture().await;
    let mut rows = fixture.rows.clone();
    rows[0]["insert"].as_array_mut().unwrap().push(json!({
        "id": "outside", "name": fixture.js.to_string_lossy(), "inject": ["tools"]
    }));
    let patches: Vec<Patch> = serde_json::from_value(rows).unwrap();
    let report = fixture
        .loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    let failed: Vec<_> = report.failures.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(failed, ["outside"], "{report:?}");
    match fixture.loader.get("outside").unwrap().status {
        EntryStatus::Unresolved(rutis_loader::LoaderError::OutsideInstance { name, group }) => {
            assert_eq!((name.as_str(), group.as_str()), ("tools", "session"))
        }
        other => panic!("{other:?}"),
    }
    fixture.root.shutdown().await.unwrap();
}
