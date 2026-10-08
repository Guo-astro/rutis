//! Instanced groups: one configuration, copies created on demand.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{patches, MemStore};
use rutis::{
    BoxFuture, CordisError, Ctx, Effect, EventKey, FiberState, Listener, Plugin, PluginFactory,
    TypeKey,
};
use rutis_loader::{
    Builtins, Editable, EntryInfo, EntryStatus, Instance, InstanceResult, Isolate, Layer, Loader,
    LoaderChanged, LoaderError, LoaderOptions, LoaderPlugin, NewEntry, ServiceCatalog,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

type Log = Arc<Mutex<Vec<String>>>;

/// The document an instance of `doc` provides, by its instance key.
#[derive(Debug)]
struct Doc(String);
/// The page an instance of `page` provides.
#[derive(Debug)]
struct Page(String);
/// A business value given with `with`.
#[derive(Debug)]
struct Title(String);
/// A global service for `isolate`.
#[derive(Debug)]
struct Shared;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
struct Cfg {
    #[serde(default)]
    label: String,
    #[serde(default)]
    invalid: bool,
    /// Rejected by validation in the instance whose title this is.
    #[serde(default)]
    reject_in: Option<String>,
    #[serde(default)]
    quit: bool,
    /// Fails in `apply` in the instance whose title this is.
    #[serde(default)]
    fail_in: Option<String>,
}

#[derive(Clone)]
enum Mode {
    Echo,
    ProvideDoc(TypeKey),
    ReadDoc(TypeKey),
    ProvidePage(TypeKey),
    ReadPage { doc: TypeKey, page: TypeKey },
    CreatePage,
}

struct Probe {
    mode: Mode,
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
                return Err(CordisError::PluginFailed(
                    format!("failed in {}", self.title).into(),
                ));
            }
            let label = &self.config.label;
            let line = match &self.mode {
                Mode::Echo => format!("apply {label}"),
                Mode::ProvideDoc(key) => {
                    ctx.provide_as(key.clone(), Arc::new(Doc(self.title.clone())))?;
                    format!("doc {}", self.title)
                }
                Mode::ReadDoc(key) => {
                    let doc = ctx.require_as::<Doc>(key.clone())?;
                    format!("read {label} {}", doc.0)
                }
                Mode::ProvidePage(key) => {
                    ctx.provide_as(key.clone(), Arc::new(Page(self.title.clone())))?;
                    format!("page {}", self.title)
                }
                Mode::ReadPage { doc, page } => {
                    let doc = ctx.require_as::<Doc>(doc.clone())?;
                    let page = ctx.require_as::<Page>(page.clone())?;
                    format!("read page {} in {}", page.0, doc.0)
                }
                Mode::CreatePage => {
                    let loader = ctx.require::<Loader>()?;
                    let page = loader
                        .create_instance(ctx, "page")
                        .with(Title(format!("{}/p", self.title)))
                        .await
                        .map_err(|e| CordisError::PluginFailed(e.to_string().into()))?;
                    format!("created page with {} rows", page.report.len())
                }
            };
            self.log.lock().unwrap().push(line);
            if self.config.quit {
                ctx.dispose_self()?;
            }
            let log = self.log.clone();
            let cleanup = format!("cleanup {label} {}", self.title);
            Ok(Effect::Disposer(Box::new(move || {
                log.lock().unwrap().push(cleanup);
                Ok(())
            })))
        })
    }
}

struct ProbeFactory {
    mode: Mode,
    injects: Vec<TypeKey>,
    title: String,
    log: Log,
}

impl PluginFactory<Cfg> for ProbeFactory {
    fn name(&self) -> &str {
        "probe"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn validate_config(&self, config: &Cfg) -> Result<(), CordisError> {
        if config.invalid || config.reject_in.as_deref() == Some(self.title.as_str()) {
            return Err(CordisError::Validation {
                issues: vec![format!("rejected in {:?}", self.title)],
            });
        }
        Ok(())
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

fn resolver(log: &Log) -> Builtins {
    let mut builtins = Builtins::new();
    let title = |build: &rutis_loader::Build| {
        build
            .value::<Title>()
            .map(|t| t.0.clone())
            .unwrap_or_default()
    };
    let l = log.clone();
    builtins.register::<Cfg, _>(
        "echo",
        ProbeFactory {
            mode: Mode::Echo,
            injects: vec![],
            title: String::new(),
            log: l,
        },
    );
    let l = log.clone();
    builtins.register_with::<Cfg, _, _>("doc-scope", move |build| {
        Ok(ProbeFactory {
            mode: Mode::ProvideDoc(TypeKey::instance::<Doc>(build.instance("doc")?)),
            injects: vec![],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register_with::<Cfg, _, _>("doc-reader", move |build| {
        let key = TypeKey::instance::<Doc>(build.instance("doc")?);
        Ok(ProbeFactory {
            mode: Mode::ReadDoc(key.clone()),
            injects: vec![key],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register_with::<Cfg, _, _>("page-scope", move |build| {
        Ok(ProbeFactory {
            mode: Mode::ProvidePage(TypeKey::instance::<Page>(build.instance("page")?)),
            injects: vec![],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register_with::<Cfg, _, _>("page-reader", move |build| {
        let doc = TypeKey::instance::<Doc>(build.instance("doc")?);
        let page = TypeKey::instance::<Page>(build.instance("page")?);
        Ok(ProbeFactory {
            mode: Mode::ReadPage {
                doc: doc.clone(),
                page: page.clone(),
            },
            injects: vec![doc, page],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register_with::<Cfg, _, _>("creator", move |build| {
        Ok(ProbeFactory {
            mode: Mode::CreatePage,
            injects: vec![TypeKey::of::<Loader>()],
            title: title(build),
            log: l.clone(),
        })
    });
    builtins
}

struct Changes(Arc<Mutex<Vec<LoaderChanged>>>);

impl Listener<LoaderChanged> for Changes {
    fn call<'a>(
        &'a self,
        _ctx: &'a Ctx,
        e: &'a LoaderChanged,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        self.0.lock().unwrap().push(e.clone());
        Box::pin(async { Ok(None) })
    }
}

struct Setup {
    root: Ctx,
    loader: Loader,
    log: Log,
    store: MemStore,
    changes: Arc<Mutex<Vec<LoaderChanged>>>,
}

impl Setup {
    fn take_log(&self) -> Vec<String> {
        let mut log = std::mem::take(&mut *self.log.lock().unwrap());
        log.sort();
        log
    }

    async fn create(&self, title: &str) -> Instance {
        self.loader
            .create_instance(&self.root, "doc")
            .with(Title(title.into()))
            .await
            .unwrap()
    }

    /// The copies of `id` in instances, by the instance's plugin.
    fn copies(&self, id: &str) -> Vec<EntryInfo> {
        self.loader
            .entries()
            .into_iter()
            .filter(|e| e.id == id && e.instance.is_some())
            .collect()
    }
}

const DOC: &str = r#"[{ "insert": [
    { "id": "doc", "group": true, "instanced": true, "config": [
        { "id": "doc-scope", "name": "doc-scope" },
        { "id": "reader", "name": "doc-reader", "config": { "label": "r" } }
    ] },
    { "id": "global", "name": "echo", "config": { "label": "g" } }
] }]"#;

async fn setup(base: &str) -> Setup {
    setup_with(base, MemStore::default(), ServiceCatalog::default()).await
}

async fn setup_with(base: &str, store: MemStore, catalog: ServiceCatalog) -> Setup {
    let (setup, report) = setup_report(base, store, catalog).await;
    assert!(report.failures.is_empty(), "{report:?}");
    setup
}

async fn setup_report(
    base: &str,
    store: MemStore,
    catalog: ServiceCatalog,
) -> (Setup, rutis_loader::ReconcileReport) {
    let log = Log::default();
    let root = Ctx::root().unwrap();
    let plugin = LoaderPlugin::new(
        resolver(&log),
        LoaderOptions {
            persist: Arc::new(store.clone()),
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    root.events()
        .on(
            &root,
            &EventKey::<LoaderChanged>::of(),
            Changes(changes.clone()),
        )
        .unwrap();
    let report = loader
        .reconcile(
            vec![
                Layer::new("base", patches(serde_json::from_str(base).unwrap())),
                Layer::new("user", store.patches()),
            ],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    (
        Setup {
            root,
            loader,
            log,
            store,
            changes,
        },
        report,
    )
}

async fn eventually(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition");
}

fn running(entry: &EntryInfo) -> Option<FiberState> {
    match &entry.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

#[tokio::test]
async fn each_instance_runs_its_own_copies() {
    let s = setup(DOC).await;
    assert_eq!(s.take_log(), ["apply g"]);
    assert!(s.copies("reader").is_empty());
    assert!(matches!(
        s.loader.get("doc").unwrap().status,
        EntryStatus::Inactive
    ));

    let a = s.create("A").await;
    let b = s.create("B").await;
    assert!(
        a.report
            .iter()
            .all(|(_, r)| matches!(r, InstanceResult::Active)),
        "{:?}",
        a.report
    );
    assert_eq!(
        a.report
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["doc-scope", "reader"]
    );
    // Each reader reads its own instance's document, by the same key shape.
    assert_eq!(s.take_log(), ["doc A", "doc B", "read r A", "read r B"]);

    let copies = s.copies("reader");
    assert_eq!(copies.len(), 2);
    assert!(copies
        .iter()
        .all(|e| running(e) == Some(FiberState::Active)));
    let plugins: Vec<_> = copies
        .iter()
        .map(|e| e.instance.as_ref().unwrap().plugin)
        .collect();
    assert_eq!(plugins, [a.plugin, b.plugin]);
    assert_eq!(
        copies[0].instance.as_ref().unwrap().chain,
        [("doc".to_owned(), a.view.instance())]
    );
    // The global row runs once.
    assert_eq!(
        s.loader
            .entries()
            .iter()
            .filter(|e| e.id == "global")
            .count(),
        1
    );

    // Removing one instance leaves the other.
    s.loader.remove_instance(a.plugin).await.unwrap();
    assert_eq!(s.take_log(), ["cleanup  A", "cleanup r A"]);
    assert_eq!(s.copies("reader").len(), 1);

    // Closing the fiber directly removes the instance too.
    b.view.shutdown().await.unwrap();
    eventually(|| s.copies("reader").is_empty()).await;
    eventually(|| {
        s.changes
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, LoaderChanged::InstanceRemoved { .. }))
            .count()
            == 2
    })
    .await;
    assert!(matches!(
        s.loader.remove_instance(b.plugin).await,
        Err(LoaderError::UnknownEntry(_))
    ));
}

const NESTED: &str = r#"[{ "insert": [
    { "id": "doc", "group": true, "instanced": true, "config": [
        { "id": "doc-scope", "name": "doc-scope" },
        { "id": "creator", "name": "creator" },
        { "id": "page", "group": true, "instanced": true, "config": [
            { "id": "page-scope", "name": "page-scope" },
            { "id": "page-reader", "name": "page-reader" }
        ] }
    ] }
] }]"#;

#[tokio::test]
async fn nested_instances_are_created_from_inside_and_read_outer_services() {
    let s = setup(NESTED).await;
    let a = s.create("A").await;
    let _b = s.create("B").await;
    // `creator` created a page from its own context, during its apply.
    let log = s.take_log();
    for line in [
        "doc A",
        "doc B",
        "page A/p",
        "page B/p",
        "read page A/p in A",
        "read page B/p in B",
        "created page with 2 rows",
    ] {
        assert!(log.iter().any(|l| l == line), "{line} in {log:?}");
    }
    let copies = s.copies("page-reader");
    assert_eq!(copies.len(), 2);
    let chain = &copies[0].instance.as_ref().unwrap().chain;
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].0, "page");
    assert_eq!(chain[1], ("doc".to_owned(), a.view.instance()));

    // A page instance needs a context inside a doc.
    assert!(matches!(
        s.loader.create_instance(&s.root, "page").await,
        Err(LoaderError::InvalidEntry(_))
    ));

    // Removing a doc removes the page inside it.
    s.loader.remove_instance(a.plugin).await.unwrap();
    assert_eq!(s.copies("page-reader").len(), 1);
    assert_eq!(s.copies("creator").len(), 1);
}

#[tokio::test]
async fn edits_reach_every_instance_or_none() {
    let s = setup(DOC).await;
    s.create("A").await;
    s.create("B").await;
    s.take_log();

    // A config change updates every copy.
    s.loader
        .update("reader", json!({ "label": "r2" }))
        .await
        .unwrap();
    let log = s.take_log();
    assert!(log.contains(&"read r2 A".to_owned()), "{log:?}");
    assert!(log.contains(&"read r2 B".to_owned()), "{log:?}");

    // Rejected in one instance: the edit is rolled back everywhere.
    let before = s.store.patches();
    let error = s
        .loader
        .update("reader", json!({ "label": "r3", "reject_in": "B" }))
        .await
        .unwrap_err();
    assert!(matches!(error, LoaderError::Rejected { .. }), "{error}");
    assert_eq!(s.store.patches(), before);
    assert!(s.take_log().is_empty());

    // A new row in the group runs in every instance.
    s.loader
        .create(
            NewEntry {
                id: Some("extra".into()),
                name: "echo".into(),
                config: json!({ "label": "x" }),
                ..NewEntry::default()
            },
            Some("doc"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(s.take_log(), ["apply x", "apply x"]);
    assert_eq!(s.copies("extra").len(), 2);

    // Disabling and removing reach every copy.
    s.loader.set_disabled("extra", true).await.unwrap();
    assert_eq!(s.take_log(), ["cleanup x ", "cleanup x "]);
    assert!(s
        .copies("extra")
        .iter()
        .all(|e| matches!(e.status, EntryStatus::Disabled)));
    s.loader.remove("extra").await.unwrap();
    assert!(s.copies("extra").is_empty());

    // Restarting the row restarts every copy.
    s.loader.restart("reader").await.unwrap();
    assert_eq!(
        s.take_log(),
        ["cleanup r2 A", "cleanup r2 B", "read r2 A", "read r2 B"]
    );
}

#[tokio::test]
async fn a_copy_that_disposes_itself_stops_alone() {
    let s = setup(
        r#"[{ "insert": [
        { "id": "doc", "group": true, "instanced": true, "config": [
            { "id": "quitter", "name": "echo", "config": { "label": "q", "quit": true } },
            { "id": "stays", "name": "echo", "config": { "label": "s" } }
        ] }
    ] }]"#,
    )
    .await;
    s.create("A").await;
    s.create("B").await;
    eventually(|| {
        s.copies("quitter")
            .iter()
            .all(|e| matches!(e.status, EntryStatus::Stopped))
    })
    .await;
    // The row was not disabled, and the other copies run.
    assert!(s.store.patches().is_empty());
    assert!(matches!(
        s.loader.get("quitter").unwrap().status,
        EntryStatus::Inactive
    ));
    assert!(s
        .copies("stays")
        .iter()
        .all(|e| running(e) == Some(FiberState::Active)));
    let stopped: Vec<_> = s
        .changes
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            LoaderChanged::Stopped { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(stopped, ["quitter", "quitter"]);

    // A change to the row starts the stopped copies again.
    s.take_log();
    s.loader
        .update("quitter", json!({ "label": "q2" }))
        .await
        .unwrap();
    assert_eq!(s.take_log(), ["apply q2", "apply q2"]);
    assert!(s
        .copies("quitter")
        .iter()
        .all(|e| running(e) == Some(FiberState::Active)));
}

#[tokio::test]
async fn a_stopped_copy_restarts_by_its_last_plugin() {
    let s = setup(
        r#"[{ "insert": [
        { "id": "doc", "group": true, "instanced": true, "config": [
            { "id": "quitter", "name": "echo", "config": { "label": "q", "quit": true } }
        ] }
    ] }]"#,
    )
    .await;
    let a = s.create("A").await;
    eventually(|| {
        s.copies("quitter")
            .iter()
            .all(|e| matches!(e.status, EntryStatus::Stopped))
    })
    .await;
    let entry = s.copies("quitter").remove(0);
    let plugin = entry.plugin.expect("the stopped copy's last fiber");
    let event = s.changes.lock().unwrap().iter().find_map(|c| match c {
        LoaderChanged::Stopped {
            id,
            instance,
            plugin,
        } => Some((id.clone(), *instance, *plugin)),
        _ => None,
    });
    assert_eq!(event, Some(("quitter".to_owned(), a.plugin, plugin)));
    s.take_log();
    s.loader.restart_instance(plugin).await.unwrap();
    eventually(|| s.log.lock().unwrap().iter().any(|l| l == "apply q")).await;
    // It quits again: stopped under a new fiber.
    eventually(|| {
        s.copies("quitter")
            .iter()
            .all(|e| matches!(e.status, EntryStatus::Stopped) && e.plugin != Some(plugin))
    })
    .await;
    assert!(s.loader.restart_instance(plugin).await.is_err());
}

#[tokio::test]
async fn invalid_uses_are_refused() {
    let (s, report) = setup_report(
        r#"[{ "insert": [
        { "id": "doc", "group": true, "instanced": true, "config": [] },
        { "id": "plain", "group": true, "config": [] },
        { "id": "leaf", "name": "echo", "instanced": true },
        { "id": "outside", "name": "doc-reader" }
    ] }]"#,
        MemStore::default(),
        ServiceCatalog::default(),
    )
    .await;
    let failed: Vec<_> = report.failures.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(failed, ["leaf", "outside"]);
    assert!(matches!(
        s.loader.create_instance(&s.root, "plain").await,
        Err(LoaderError::InvalidEntry(_))
    ));
    assert!(matches!(
        s.loader.create_instance(&s.root, "nothing").await,
        Err(LoaderError::UnknownEntry(_))
    ));
    // `instanced` on a plugin row makes the row invalid.
    assert!(matches!(
        s.loader.get("leaf").unwrap().status,
        EntryStatus::Unresolved(LoaderError::InvalidEntry(_))
    ));
    assert!(s.loader.create_instance(&s.root, "leaf").await.is_err());
    // A plugin that needs an instance does not run outside one.
    assert!(matches!(
        s.loader.get("outside").unwrap().status,
        EntryStatus::Unresolved(LoaderError::Rejected { .. })
    ));
    // Another root's context is refused.
    let other = Ctx::root().unwrap();
    assert!(s.loader.create_instance(&other, "doc").await.is_err());
}

#[tokio::test]
async fn disabling_an_instanced_group_closes_its_instances() {
    let s = setup(DOC).await;
    s.create("A").await;
    s.take_log();
    s.loader.set_disabled("doc", true).await.unwrap();
    assert_eq!(s.take_log(), ["cleanup  A", "cleanup r A"]);
    assert!(s.copies("reader").is_empty());
    assert!(s.loader.create_instance(&s.root, "doc").await.is_err());
    s.loader.set_disabled("doc", false).await.unwrap();
    // Closed instances stay closed; new ones can be created.
    assert!(s.copies("reader").is_empty());
    s.create("B").await;
    assert_eq!(s.copies("reader").len(), 1);
}

#[tokio::test]
async fn rebuilding_an_instance_keeps_its_values() {
    let mut catalog = ServiceCatalog::new();
    catalog.register::<Shared>("shared");
    let s = setup_with(DOC, MemStore::default(), catalog).await;
    let a = s.create("A").await;
    s.take_log();
    let mut isolate = BTreeMap::new();
    isolate.insert("shared".to_owned(), Isolate::Private);
    s.loader.set_isolate("doc", isolate).await.unwrap();
    let log = s.take_log();
    assert!(log.contains(&"read r A".to_owned()), "{log:?}");
    let copies = s.copies("reader");
    assert_eq!(copies.len(), 1);
    let instance = copies[0].instance.as_ref().unwrap();
    // A new group fiber, the same instance and value.
    assert_ne!(instance.plugin, a.plugin);
    assert_eq!(running(&copies[0]), Some(FiberState::Active));
}

#[tokio::test]
async fn stored_layers_and_new_instances_give_the_same_state() {
    let store = MemStore::default();
    let s = setup_with(DOC, store.clone(), ServiceCatalog::default()).await;
    s.create("A").await;
    s.loader
        .update("reader", json!({ "label": "saved" }))
        .await
        .unwrap();
    s.take_log();

    let t = setup_with(DOC, store, ServiceCatalog::default()).await;
    t.take_log();
    t.create("A").await;
    assert_eq!(t.take_log(), ["doc A", "read saved A"]);
}

#[tokio::test]
async fn rebuilding_an_outer_instance_recreates_the_inner_ones_once() {
    let mut catalog = ServiceCatalog::new();
    catalog.register::<Shared>("shared");
    let s = setup_with(NESTED, MemStore::default(), catalog).await;
    s.create("A").await;
    assert_eq!(s.copies("page-reader").len(), 1);
    let mut isolate = BTreeMap::new();
    isolate.insert("shared".to_owned(), Isolate::Private);
    s.loader.set_isolate("doc", isolate).await.unwrap();
    // The page went with the old doc context; `creator` created a new one.
    let copies = s.copies("page-reader");
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(running(&copies[0]), Some(FiberState::Active));
    let removed = s
        .changes
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c, LoaderChanged::InstanceRemoved { group, .. } if group == "page"))
        .count();
    assert_eq!(removed, 1);
}

#[tokio::test]
async fn a_group_edit_is_checked_on_the_plugins_below_it() {
    let s = setup(DOC).await;
    s.create("A").await;
    for (id, name, group, parent) in [("g", "", true, "doc"), ("inner", "doc-reader", false, "g")] {
        let entry = NewEntry {
            id: Some(id.into()),
            name: name.into(),
            group,
            ..NewEntry::default()
        };
        s.loader.create(entry, Some(parent), None).await.unwrap();
    }
    s.take_log();
    let before = s.store.patches();
    // Out of `doc`, the reader in `g` has no instance to read from:
    // refused before anything is unloaded.
    let error = s.loader.move_to("g", None, None).await.unwrap_err();
    assert!(matches!(error, LoaderError::Rejected { .. }), "{error}");
    assert!(s.take_log().is_empty());
    assert_eq!(s.store.patches(), before);
    let copies = s.copies("inner");
    assert_eq!(copies.len(), 1);
    assert_eq!(running(&copies[0]), Some(FiberState::Active));
}

#[tokio::test]
async fn rows_under_a_waiting_group_are_waiting() {
    let mut catalog = ServiceCatalog::new();
    catalog.register::<Shared>("shared");
    let s = setup_with(
        r#"[{ "insert": [
        { "id": "doc", "group": true, "instanced": true, "config": [
            { "id": "g", "group": true, "inject": ["shared"], "config": [
                { "id": "child", "name": "echo" }
            ] }
        ] }
    ] }]"#,
        MemStore::default(),
        catalog,
    )
    .await;
    let a = s.create("A").await;
    assert!(
        matches!(
            a.report.as_slice(),
            [(g, InstanceResult::Waiting), (child, InstanceResult::Waiting)]
                if g == "g" && child == "child"
        ),
        "{:?}",
        a.report
    );
}

/// Keeps the context it is applied in.
struct Hold(Arc<Mutex<Option<Ctx>>>);

impl Plugin for Hold {
    fn name(&self) -> &str {
        "hold"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            *self.0.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn a_top_level_instance_needs_the_loader_context_or_above() {
    let s = setup(DOC).await;
    let held = Arc::new(Mutex::new(None));
    let hold = s.root.plugin(Hold(held.clone()));
    (&hold).await.unwrap();
    let branch = held.lock().unwrap().clone().unwrap();
    // A branch beside the loader is neither its context nor above it.
    assert!(matches!(
        s.loader.create_instance(&branch, "doc").await,
        Err(LoaderError::InvalidEntry(_))
    ));
    // Nor is it once that plugin ended.
    hold.dispose().await.unwrap();
    assert!(s.loader.create_instance(&branch, "doc").await.is_err());
    // The root, above the loader, still works.
    s.create("B").await;
}

#[tokio::test]
async fn a_failing_copy_does_not_exempt_the_others_from_a_group_check() {
    let s = setup(DOC).await;
    s.create("A").await;
    for (id, name, group, parent, config) in [
        ("g", "", true, "doc", json!(null)),
        (
            "inner",
            "doc-reader",
            false,
            "g",
            json!({ "label": "i", "fail_in": "B" }),
        ),
    ] {
        let entry = NewEntry {
            id: Some(id.into()),
            name: name.into(),
            group,
            config,
            ..NewEntry::default()
        };
        s.loader.create(entry, Some(parent), None).await.unwrap();
    }
    // The copy in B fails in apply; the one in A runs.
    let b = s.create("B").await;
    assert!(
        b.report
            .iter()
            .any(|(id, r)| id == "inner" && matches!(r, InstanceResult::Failed(_))),
        "{:?}",
        b.report
    );
    s.take_log();
    let error = s.loader.move_to("g", None, None).await.unwrap_err();
    assert!(matches!(error, LoaderError::Rejected { .. }), "{error}");
    // The healthy copy in A kept running.
    assert!(s.take_log().is_empty());
}

#[tokio::test]
async fn a_failure_already_there_does_not_block_a_group_edit() {
    let mut catalog = ServiceCatalog::new();
    catalog.register::<Shared>("shared");
    let (s, report) = setup_report(
        r#"[{ "insert": [
        { "id": "doc", "group": true, "instanced": true, "config": [
            { "id": "g", "group": true, "config": [
                { "id": "bad", "name": "echo", "config": { "invalid": true } },
                { "id": "good", "name": "echo", "config": { "label": "x" } }
            ] }
        ] }
    ] }]"#,
        MemStore::default(),
        catalog,
    )
    .await;
    assert!(report.failures.is_empty(), "{report:?}");
    s.create("A").await;
    let mut isolate = BTreeMap::new();
    isolate.insert("shared".to_owned(), Isolate::Private);
    // `bad` is refused the same way before and after.
    s.loader.set_isolate("g", isolate).await.unwrap();
    assert_eq!(running(&s.copies("good")[0]), Some(FiberState::Active));
}
