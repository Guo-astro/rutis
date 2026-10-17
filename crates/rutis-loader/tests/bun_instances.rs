//! Shared service names inside instances, for Bun rows: each instance's Bun
//! row uses and provides that instance's services, though every instance's
//! rows share one Bun process; a removed instance leaves nothing in it.
#![cfg(all(unix, feature = "bun"))]

mod common;

use std::sync::Arc;

use common::runtimes::{active, repo, title, NoConfig, Probe, Title, Tools};
use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin, PluginFactory, TypeKey};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, host_key_in, settle, HostDispatch, Value as RpcValue};
use rutis_loader::{
    Builtins, Chain, EntryInfo, Instance, InstanceResult, Layer, Loader, LoaderOptions,
    LoaderPlugin, Patch, RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};
use serde_json::json;

// ── The language row ────────────────────────────────────────────

/// Uses the instance's `tools`, and provides the instance's `bun_timeline`.
const BUN_ROW: &str = r#"
export const inject = ['tools', 'probe']
export const provides = { bun_timeline: { title: 'async' } }
export function apply(ctx) {
  const title = ctx.use('tools').title()
  ctx.use('probe').record(`bun sees ${title}`)
  ctx.provide('bun_timeline', { async title() { return `bun ${title}` } })
}
"#;

// ── Rust plugins ────────────────────────────────────────────────

enum Job {
    /// Provide `Tools` at this key.
    Provide(TypeKey),
    /// Read the timeline at this key and record it.
    Read(TypeKey),
}

struct JobPlugin {
    job: Arc<Job>,
    title: String,
    injects: Vec<TypeKey>,
}

impl Plugin for JobPlugin {
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
                Job::Read(key) => {
                    let mut seen = Vec::new();
                    {
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
        Ok(Box::new(JobPlugin {
            job: self.job.clone(),
            title: self.title.clone(),
            injects: self.injects.clone(),
        }))
    }
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
        let key = host_key_in("bun_timeline", instance);
        Ok(JobFactory {
            injects: vec![key.clone(), host_key("probe")],
            job: Arc::new(Job::Read(key)),
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
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    std::fs::write(dir.path().join("bun_row.ts"), BUN_ROW).unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe");
    for name in ["tools", "bun_timeline"] {
        catalog.register_shared_instance(name, "session");
    }
    let bun = LocalRuntime::bun(repo().join("bun/rutis-bun"), dir.path());
    let bun_rows = Arc::new(RuntimeResolver::modules(bun.handle()).with_catalog(&catalog));
    root.plugin(bun);
    let chain = Chain::new().with(builtins()).with_shared(bun_rows.clone());
    let plugin = LoaderPlugin::new(
        chain,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(bun_rows));

    let rows = json!([{ "insert": [
        { "id": "session", "group": true, "instanced": true, "config": [
            { "id": "tools", "name": "tools" },
            { "id": "bun", "name": "bun:./bun_row.ts" },
            { "id": "reader", "name": "reader" }
        ] }
    ] }]);
    let patches: Vec<Patch> = serde_json::from_value(rows).unwrap();
    let report = loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    Fixture {
        root,
        loader,
        probe,
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
            .wait_for(&format!("rust in {title} reads bun {title}"))
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

/// Two instances, the same names: each instance's Bun row sees that
/// instance's Rust `tools`, and its Rust reader reads that instance's row's
/// timeline, through one Bun process.
/// guarantee: loader/instance-services
#[tokio::test(flavor = "multi_thread")]
async fn each_instance_sees_its_own_services() {
    let fixture = fixture().await;
    let a = fixture.create("A").await;
    let b = fixture.create("B").await;
    for line in ["bun sees A", "bun sees B"] {
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
    assert!(!fixture.probe.has("rust in A reads bun B"));
    fixture.root.shutdown().await.unwrap();
}

/// Removing an instance withdraws its services everywhere, the other
/// instance keeps running, and a new instance registers the same names in
/// the Bun process again.
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
    for id in ["tools", "bun", "reader"] {
        let copies = fixture.copies(id);
        assert_eq!(copies.len(), 1, "{id}");
        assert!(active(&copies[0]), "{id}: {:?}", copies[0].status);
    }
    fixture.create("C").await;
    fixture.root.shutdown().await.unwrap();
}
