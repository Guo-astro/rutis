//! Service names inside instances: one name, each instance's own key.

mod common;

use std::sync::{Arc, Mutex};

use common::{patches, MemStore};
use rutis::{BoxFuture, CordisError, Ctx, Effect, FiberState, Plugin, PluginFactory, TypeKey};
use rutis_loader::{
    Build, Builtins, Editable, EntryInfo, EntryStatus, ExprScope, Expressions, InstanceResult,
    Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, ServiceCatalog,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

type Log = Arc<Mutex<Vec<String>>>;

/// The service `tools` names, one per `session` instance; expressions read
/// it as its title.
#[derive(Debug, serde::Serialize)]
struct Tools(String);

/// The instance's title, given with `with`.
struct Title(String);

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
struct Cfg {
    #[serde(default)]
    label: Value,
    /// Fails in `apply` in the instance whose title this is.
    #[serde(default)]
    fail_in: Option<String>,
}

enum Mode {
    /// Provides `Tools` at this key.
    Provide(TypeKey),
    /// Logs its config label.
    Echo,
}

struct Probe {
    mode: Arc<Mode>,
    title: String,
    config: Cfg,
    log: Log,
}

impl Plugin for Probe {
    fn name(&self) -> &str {
        "probe"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            if self.config.fail_in.as_deref() == Some(self.title.as_str()) {
                return Err(CordisError::PluginFailed("failed here".into()));
            }
            let line = match &*self.mode {
                Mode::Provide(key) => {
                    ctx.provide_as(key.clone(), Arc::new(Tools(self.title.clone())))?;
                    format!("tools {}", self.title)
                }
                Mode::Echo => format!("echo {} {}", self.title, self.config.label),
            };
            self.log.lock().unwrap().push(line);
            Ok(Effect::Done)
        })
    }
}

struct ProbeFactory {
    mode: Arc<Mode>,
    title: String,
    log: Log,
}

impl PluginFactory<Cfg> for ProbeFactory {
    fn name(&self) -> &str {
        "probe"
    }

    fn build(&self, config: &Cfg) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(Probe {
            mode: self.mode.clone(),
            title: self.title.clone(),
            config: config.clone(),
            log: self.log.clone(),
        }))
    }
}

fn title(build: &Build) -> String {
    build
        .value::<Title>()
        .map(|t| t.0.clone())
        .unwrap_or_default()
}

fn resolver(log: &Log) -> Builtins {
    let mut builtins = Builtins::new();
    let l = log.clone();
    builtins.register_with::<Cfg, _, _>("tools", move |build| {
        Ok(ProbeFactory {
            mode: Arc::new(Mode::Provide(TypeKey::instance::<Tools>(
                build.instance("session")?,
            ))),
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register_with::<Cfg, _, _>("echo", move |build| {
        Ok(ProbeFactory {
            mode: Arc::new(Mode::Echo),
            title: title(build),
            log: l.clone(),
        })
    });
    builtins
}

/// `{ "__jsExpr": "has tools" }` → whether `tools` is there; `read tools`
/// → its value.
struct HasExpr;

impl Expressions for HasExpr {
    fn evaluate(&self, expr: &str, scope: &ExprScope<'_>) -> Result<Value, LoaderError> {
        if let Some(name) = expr.strip_prefix("has ") {
            return Ok(Value::Bool(scope.has(name)?));
        }
        if let Some(name) = expr.strip_prefix("read ") {
            return Ok(scope.read(name)?.unwrap_or(Value::Null));
        }
        Err(LoaderError::Expression(format!(
            "unknown expression {expr}"
        )))
    }
}

struct Setup {
    root: Ctx,
    loader: Loader,
    log: Log,
}

impl Setup {
    async fn create(&self, title: &str) -> rutis_loader::Instance {
        self.loader
            .create_instance(&self.root, "session")
            .with(Title(title.into()))
            .await
            .unwrap()
    }

    fn copies(&self, id: &str) -> Vec<EntryInfo> {
        self.loader
            .entries()
            .into_iter()
            .filter(|e| e.id == id && e.instance.is_some())
            .collect()
    }

    fn take_log(&self) -> Vec<String> {
        let mut log = std::mem::take(&mut *self.log.lock().unwrap());
        log.sort();
        log
    }
}

async fn setup(base: Value) -> (Setup, rutis_loader::ReconcileReport) {
    let log = Log::default();
    let mut catalog = ServiceCatalog::new();
    catalog.readable_instance::<Tools>("tools", "session");
    let store = MemStore::default();
    let root = Ctx::root().unwrap();
    let plugin = LoaderPlugin::new(
        resolver(&log),
        LoaderOptions {
            persist: Arc::new(store.clone()),
            catalog,
            expressions: Some(Arc::new(HasExpr)),
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let report = loader
        .reconcile(
            vec![
                Layer::new("base", patches(json!([{ "insert": base }]))),
                Layer::new("user", store.patches()),
            ],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    (Setup { root, loader, log }, report)
}

fn active(entry: &EntryInfo) -> bool {
    matches!(&entry.status, EntryStatus::Running(s) if s.state == FiberState::Active)
}

#[tokio::test]
async fn an_instance_name_gates_on_its_own_instance() {
    let (s, report) = setup(json!([
        { "id": "session", "group": true, "instanced": true, "config": [
            { "id": "tools", "name": "tools", "config": { "fail_in": "B" } },
            { "id": "user", "name": "echo", "inject": ["tools"] }
        ] }
    ]))
    .await;
    assert!(report.failures.is_empty(), "{report:?}");
    let a = s.create("A").await;
    let b = s.create("B").await;
    // A has its tools; B's provider failed, so B's user waits for B's own
    // tools instead of using A's.
    assert!(
        matches!(
            a.report.as_slice(),
            [(_, InstanceResult::Active), (_, InstanceResult::Active)]
        ),
        "{:?}",
        a.report
    );
    assert!(
        matches!(
            b.report.as_slice(),
            [(_, InstanceResult::Failed(_)), (_, InstanceResult::Waiting)]
        ),
        "{:?}",
        b.report
    );
    assert_eq!(s.take_log(), ["echo A null", "tools A"]);
    let users = s.copies("user");
    assert_eq!(users.iter().filter(|e| active(e)).count(), 1);
}

#[tokio::test]
async fn an_instance_name_outside_its_instances_is_unresolved() {
    let (s, report) = setup(json!([
        { "id": "session", "group": true, "instanced": true, "config": [] },
        { "id": "outside", "name": "echo", "inject": ["tools"] }
    ]))
    .await;
    let failed: Vec<_> = report.failures.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(failed, ["outside"]);
    match s.loader.get("outside").unwrap().status {
        EntryStatus::Unresolved(LoaderError::OutsideInstance { name, group }) => {
            assert_eq!((name.as_str(), group.as_str()), ("tools", "session"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn expressions_see_the_copy_s_instance() {
    let (s, report) = setup(json!([
        { "id": "session", "group": true, "instanced": true, "config": [
            { "id": "tools", "name": "tools", "config": { "fail_in": "B" } },
            { "id": "user", "name": "echo" }
        ] }
    ]))
    .await;
    assert!(report.failures.is_empty(), "{report:?}");
    s.create("A").await;
    s.create("B").await;
    s.take_log();
    // Once the providers settled: each copy evaluates `tools` in its own
    // instance.
    s.loader
        .update("user", json!({ "label": { "__jsExpr": "has tools" } }))
        .await
        .unwrap();
    let log = s.take_log();
    assert!(log.contains(&"echo A true".to_owned()), "{log:?}");
    assert!(log.contains(&"echo B false".to_owned()), "{log:?}");
    // And read the value of their own instance's service.
    s.loader
        .update("user", json!({ "label": { "__jsExpr": "read tools" } }))
        .await
        .unwrap();
    let log = s.take_log();
    assert!(log.contains(&"echo A \"A\"".to_owned()), "{log:?}");
    assert!(log.contains(&"echo B null".to_owned()), "{log:?}");
}
