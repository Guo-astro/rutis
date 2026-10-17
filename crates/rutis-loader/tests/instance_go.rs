//! Shared service names inside instances, for Go: each instance's Go row
//! uses and provides that instance's services, though every instance's rows
//! share one Go process.
#![cfg(feature = "go")]

mod common;

use std::sync::Arc;

use common::runtimes::{go_binary, title, NoConfig, Probe, Title, Tools};
use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin, PluginFactory, TypeKey};
use rutis_bridge::session::{host_key, host_key_in, settle, HostDispatch, Value as RpcValue};
use rutis_loader::{
    Builtins, Chain, GoBinaries, GoResolver, GoRuntimes, Layer, Loader, LoaderOptions,
    LoaderPlugin, Patch, ServiceCatalog,
};
use serde_json::json;

// ── Rust plugins ────────────────────────────────────────────────

enum Job {
    /// Provide `Tools` at this key.
    Provide(TypeKey),
    /// Read the Go timeline at this key and record it.
    Read(TypeKey),
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
                Job::Read(go) => {
                    let mut seen = Vec::new();
                    for key in [go] {
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
        let go = host_key_in("go_timeline", instance);
        Ok(JobFactory {
            injects: vec![go.clone(), host_key("probe")],
            job: Arc::new(Job::Read(go)),
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
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe");
    for name in ["tools", "go_timeline"] {
        catalog.register_shared_instance(name, "session");
    }
    let go = Arc::new(GoResolver::new(GoBinaries::new().file(go_binary())).with_catalog(&catalog));
    let plugin = LoaderPlugin::new(
        Chain::new().with(builtins()).with_shared(go.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(GoRuntimes::new(go, dir.path()).idle(None))
        .await
        .unwrap();
    let rows = json!([{ "insert": [
        { "id": "session", "group": true, "instanced": true, "config": [
            { "id": "tools", "name": "tools" },
            { "id": "go", "name": "go:go_row" },
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

/// Two instances, the same names: each instance's Go row sees that
/// instance's Rust `tools`, and each instance's Rust reader reads that
/// instance's Go timeline, through one Go process.
/// guarantee: loader/instance-services
#[tokio::test(flavor = "multi_thread")]
async fn each_instance_sees_its_own_services_in_go() {
    let fixture = fixture().await;
    for title in ["A", "B"] {
        fixture
            .loader
            .create_instance(&fixture.root, "session")
            .with(Title(title.into()))
            .await
            .unwrap();
        fixture.probe.wait_for(&format!("go sees {title}")).await;
        fixture
            .probe
            .wait_for(&format!("rust in {title} reads go {title}"))
            .await;
    }
    assert!(!fixture.probe.has("rust in A reads go B"));
    assert!(!fixture.probe.has("rust in B reads go A"));
    fixture.root.shutdown().await.unwrap();
}
