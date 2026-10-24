//! Shared service names inside instances, across languages: each instance's
//! TS and Python rows use and provide that instance's services, though
//! every instance's rows share one process per language.
#![cfg(all(feature = "node", feature = "python"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{active, repo, title, tool_callers, NoConfig, Probe, Title, Tools, COLD};
use common::Scoped;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, Key, PluginFactory, TypeKey};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, host_key_in, settle, HostDispatch, Value as RpcValue};
use rutis_loader::edit::{Instance, InstanceResult};
use rutis_loader::report::{EntryInfo, EntryStatus};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
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

enum Job {
    /// Provide `Tools` at this key.
    Provide(Key<dyn HostDispatch>),
    /// Read both timelines at these keys and record them.
    Read {
        py: Key<dyn HostDispatch>,
        js: Key<dyn HostDispatch>,
    },
}

struct Job1 {
    job: Arc<Job>,
    title: String,
    injects: Vec<TypeKey>,
}

impl DynPlugin for Job1 {
    fn name(&self) -> &str {
        "job"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            match &*self.job {
                Job::Provide(key) => {
                    ctx.provide_at(key.clone(), Arc::new(Tools(self.title.clone())))?;
                }
                Job::Read { py, js } => {
                    let mut seen = Vec::new();
                    for key in [py, js] {
                        let timeline = ctx.require_at(key)?;
                        let call = timeline
                            .invoke("title", RpcValue::List(Vec::new()))
                            .map_err(|e| {
                                rutis::Error::from(ErrorKind::PluginFailed(e.to_string().into()))
                            })?;
                        let title = settle(call).await.and_then(RpcValue::json).map_err(|e| {
                            rutis::Error::from(ErrorKind::PluginFailed(e.to_string().into()))
                        })?;
                        seen.push(title.as_str().unwrap_or_default().to_owned());
                    }
                    let probe = ctx.require_at::<dyn HostDispatch>(&host_key("probe"))?;
                    let line = format!("rust in {} reads {}", self.title, seen.join(" / "));
                    probe
                        .invoke("record", RpcValue::List(vec![json!(line).into()]))
                        .map_err(|e| {
                            rutis::Error::from(ErrorKind::PluginFailed(e.to_string().into()))
                        })?;
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

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn build(&self, _: &NoConfig) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Job1 {
            job: self.job.clone(),
            title: self.title.clone(),
            injects: self.injects.clone(),
        }))
    }
}

fn builtins() -> Scoped {
    let mut builtins = Scoped::new();
    builtins.register::<NoConfig, _, _>("tools", |build| {
        let instance = build.instance("session")?;
        Ok(JobFactory {
            job: Arc::new(Job::Provide(host_key_in("tools", instance))),
            title: title(build),
            injects: Vec::new(),
        })
    });
    builtins.register::<NoConfig, _, _>("reader", |build| {
        let instance = build.instance("session")?;
        let py = host_key_in("py_timeline", instance);
        let js = host_key_in("js_timeline", instance);
        Ok(JobFactory {
            injects: vec![py.erase(), js.erase(), host_key("probe").erase()],
            job: Arc::new(Job::Read { py, js }),
            title: title(build),
        })
    });
    builtins
}

// ── Harness ─────────────────────────────────────────────────────

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
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
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

// ── The suite ───────────────────────────────────────────────────

/// Two instances, the same names: each instance's TS and Python rows see
/// that instance's Rust `tools`, and its Rust reader reads that instance's
/// rows' timelines, through one process per language.
/// guarantee: loader/instance-services
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
    // `tools` sees which row asks, in which instance's scope (#441). Other
    // tests of this file open instances with the same titles: each title's
    // scopes are compared as sets.
    let scopes = |title: &str, id: &str| -> std::collections::BTreeSet<String> {
        let callers = tool_callers(title);
        let scopes: std::collections::BTreeSet<String> = callers
            .iter()
            .filter(|(caller, _)| caller == id)
            .map(|(_, scope)| scope.clone().expect("a scope"))
            .collect();
        assert!(
            !scopes.is_empty(),
            "{id} in {title} asked as a row: {callers:?}"
        );
        scopes
    };
    for title in ["A", "B"] {
        for id in ["py", "js"] {
            let all = scopes(title, id);
            assert!(all.iter().all(|s| s.starts_with("rutis-loader/instance/")));
        }
        let (py, js) = (scopes(title, "py"), scopes(title, "js"));
        assert!(py.intersection(&js).next().is_some(), "one instance");
    }
    let (a, b) = (scopes("A", "js"), scopes("B", "js"));
    assert!(a.is_disjoint(&b), "two instances");
    fixture.root.shutdown().await.unwrap();
}

/// Removing an instance withdraws its services everywhere: nothing of it is
/// left in rutis or in the runtimes, the other instance keeps running, and
/// a new instance registers the same names again.
/// guarantee: loader/instance-close-leaves-nothing
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

/// Instances created and removed over and over leave no task behind in
/// the application. Each language row's services are projected from a
/// projection that watched its runtime's session in a task of its own,
/// which lasted as long as the session: one task per row ever loaded
/// (#191).
/// guarantee: loader/instance-close-leaves-nothing
#[tokio::test(flavor = "multi_thread")]
async fn removed_instances_leave_no_tasks_behind() {
    let fixture = fixture().await;
    let metrics = tokio::runtime::Handle::current().metrics();
    // What the first instance starts for good (a runtime's lazily started
    // parts) is in the baseline.
    let first = fixture.create("first").await;
    fixture.loader.remove_instance(first.plugin).await.unwrap();
    let baseline = metrics.num_alive_tasks();
    for round in 0..20 {
        let instance = fixture.create(&format!("round {round}")).await;
        fixture
            .loader
            .remove_instance(instance.plugin)
            .await
            .unwrap();
    }
    COLD.eventually_with(
        || (metrics.num_alive_tasks() <= baseline).then_some(()),
        "live tasks back to the baseline",
        || format!("{} live, baseline {baseline}", metrics.num_alive_tasks()),
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// A language row outside the group of an instance name it uses is
/// unresolved, saying where it belongs, though the loader leaves its scope
/// to the runtime.
/// guarantee: loader/instance-services
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
