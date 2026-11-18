//! Instanced groups: one configuration, copies created on demand.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use common::{json, patches, schema_of, MemStore, Scoped};
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, Key, PluginFactory, PluginId,
    TypeKey,
};
use rutis::{Ctx, Error};
use rutis_loader::edit::{Editable, Instance, InstanceResult, NewEntry};
use rutis_loader::report::{EntryInfo, EntryStatus, Isolate, LoaderChanged, ReconcileReport};
use rutis_loader::resolve::Build;
use rutis_loader::{
    Builtins, Chain, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, ServiceCatalog,
};
use rutis_test_support::{until, Guard, HANG_GUARD};
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
    ProvideDoc(Key<Doc>),
    ReadDoc(Key<Doc>),
    ProvidePage(Key<Page>),
    ReadPage { doc: Key<Doc>, page: Key<Page> },
    CreatePage,
    ProvideShared,
    ReadShared,
}

struct Probe {
    mode: Mode,
    title: String,
    config: Cfg,
    log: Log,
}

impl DynPlugin for Probe {
    fn name(&self) -> &str {
        "probe"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if self.config.fail_in.as_deref() == Some(self.title.as_str()) {
                return Err(rutis::Error::from(ErrorKind::PluginFailed(
                    format!("failed in {}", self.title).into(),
                )));
            }
            let label = &self.config.label;
            let line = match &self.mode {
                Mode::Echo => format!("apply {label}"),
                Mode::ProvideDoc(key) => {
                    ctx.provide_at(key.clone(), Arc::new(Doc(self.title.clone())))?;
                    format!("doc {}", self.title)
                }
                Mode::ReadDoc(key) => {
                    let doc = ctx.require_at(key)?;
                    format!("read {label} {}", doc.0)
                }
                Mode::ProvidePage(key) => {
                    ctx.provide_at(key.clone(), Arc::new(Page(self.title.clone())))?;
                    format!("page {}", self.title)
                }
                Mode::ReadPage { doc, page } => {
                    let doc = ctx.require_at(doc)?;
                    let page = ctx.require_at(page)?;
                    format!("read page {} in {}", page.0, doc.0)
                }
                Mode::ProvideShared => {
                    ctx.provide(Shared)?;
                    format!("provide shared {}", self.title)
                }
                Mode::ReadShared => {
                    ctx.require::<Shared>()?;
                    format!("read shared {}", self.title)
                }
                Mode::CreatePage => {
                    let loader = ctx.require::<Loader>()?;
                    let page = loader
                        .create_instance(ctx, "page")
                        .with(Title(format!("{}/p", self.title)))
                        .await
                        .map_err(|e| {
                            rutis::Error::from(ErrorKind::PluginFailed(e.to_string().into()))
                        })?;
                    // The nested instance hands its creator the views of what
                    // it spawned: the same rows, in the same order, as its
                    // report (only rows with no fiber are missing).
                    let rows: Vec<String> = page
                        .rows
                        .iter()
                        .map(|(id, view)| format!("{id}={:?}", view.state().state))
                        .collect();
                    self.log
                        .lock()
                        .unwrap()
                        .push(format!("page rows {}", rows.join(" ")));
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

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn validate_config(&self, config: &Cfg) -> Result<(), Error> {
        if config.invalid || config.reject_in.as_deref() == Some(self.title.as_str()) {
            return Err(rutis::Error::from(ErrorKind::Validation {
                issues: vec![format!("rejected in {:?}", self.title)],
            }));
        }
        Ok(())
    }

    fn build(&self, config: &Cfg) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Probe {
            mode: self.mode.clone(),
            title: self.title.clone(),
            config: config.clone(),
            log: self.log.clone(),
        }))
    }
}

fn resolver(log: &Log) -> Chain {
    let mut plain = Builtins::new();
    let mut builtins = Scoped::new();
    let title = |build: &Build| {
        build
            .value::<Title>()
            .map(|t| t.0.clone())
            .unwrap_or_default()
    };
    let l = log.clone();
    plain.register_raw(
        "echo",
        json(ProbeFactory {
            mode: Mode::Echo,
            injects: vec![],
            title: String::new(),
            log: l,
        }),
        schema_of::<Cfg>(),
    );
    let l = log.clone();
    builtins.register::<Cfg, _, _>("doc-scope", move |build| {
        Ok(ProbeFactory {
            mode: Mode::ProvideDoc(Key::of().instance(build.instance("doc")?)),
            injects: vec![],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register::<Cfg, _, _>("scoped-echo", move |build| {
        Ok(ProbeFactory {
            mode: Mode::Echo,
            injects: vec![],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register::<Cfg, _, _>("doc-reader", move |build| {
        let key = Key::<Doc>::of().instance(build.instance("doc")?);
        Ok(ProbeFactory {
            injects: vec![key.erase()],
            mode: Mode::ReadDoc(key),
            title: title(build),
            log: l.clone(),
        })
    });
    // Like `doc-reader`, but in instance "B" it fails the way it does
    // outside any instance: the same error from another copy.
    let l = log.clone();
    builtins.register::<Cfg, _, _>("picky-reader", move |build| {
        if title(build) == "B" {
            return Err(rutis::Error::from(ErrorKind::PluginFailed(
                format!("not inside an instance of {:?}", "doc").into(),
            )));
        }
        let key = Key::<Doc>::of().instance(build.instance("doc")?);
        Ok(ProbeFactory {
            injects: vec![key.erase()],
            mode: Mode::ReadDoc(key),
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register::<Cfg, _, _>("page-scope", move |build| {
        Ok(ProbeFactory {
            mode: Mode::ProvidePage(Key::of().instance(build.instance("page")?)),
            injects: vec![],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register::<Cfg, _, _>("page-reader", move |build| {
        let doc = Key::<Doc>::of().instance(build.instance("doc")?);
        let page = Key::<Page>::of().instance(build.instance("page")?);
        Ok(ProbeFactory {
            injects: vec![doc.erase(), page.erase()],
            mode: Mode::ReadPage { doc, page },
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register::<Cfg, _, _>("creator", move |build| {
        Ok(ProbeFactory {
            mode: Mode::CreatePage,
            injects: vec![TypeKey::of::<Loader>()],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register::<Cfg, _, _>("shared-provider", move |build| {
        Ok(ProbeFactory {
            mode: Mode::ProvideShared,
            injects: vec![],
            title: title(build),
            log: l.clone(),
        })
    });
    let l = log.clone();
    builtins.register::<Cfg, _, _>("shared-reader", move |build| {
        Ok(ProbeFactory {
            mode: Mode::ReadShared,
            injects: vec![TypeKey::of::<Shared>()],
            title: title(build),
            log: l.clone(),
        })
    });
    Chain::new().with(plain).with(builtins)
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

    /// What `instance_rows` lists for `instance`: each row id with its
    /// fiber's plugin.
    fn rows(&self, instance: &Instance) -> Vec<(String, PluginId)> {
        self.loader
            .instance_rows(instance.view.instance())
            .into_iter()
            .map(|(id, view)| (id, view.id))
            .collect()
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
) -> (Setup, ReconcileReport) {
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
    let seen = changes.clone();
    root.on::<LoaderChanged>(move |e, _| {
        seen.lock().unwrap().push((*e).clone());
        std::future::ready(Ok(None))
    })
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

fn running(entry: &EntryInfo) -> Option<FiberState> {
    match &entry.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

/// guarantee: loader/instances, loader/instance-close-leaves-nothing
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
    // Each instance lists only its own live rows, not its group fiber.
    let a_rows = s.rows(&a);
    let b_rows = s.rows(&b);
    let ids =
        |rows: &[(String, PluginId)]| rows.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&a_rows), ["doc-scope", "reader"]);
    assert_eq!(ids(&b_rows), ["doc-scope", "reader"]);
    assert!(a_rows.iter().all(|(_, plugin)| *plugin != a.plugin));
    assert!(a_rows
        .iter()
        .all(|(_, plugin)| b_rows.iter().all(|(_, other)| plugin != other)));
    // The loader's own context is no instance.
    assert!(s.loader.instance_rows(s.root.instance()).is_empty());
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
    assert!(s.rows(&a).is_empty());
    assert_eq!(s.rows(&b), b_rows);

    // Closing the fiber directly removes the instance too.
    b.view.shutdown().await.unwrap();
    until(|| s.copies("reader").is_empty(), "the copies gone").await;
    until(
        || {
            s.changes
                .lock()
                .unwrap()
                .iter()
                .filter(|c| matches!(c, LoaderChanged::InstanceRemoved { .. }))
                .count()
                == 2
        },
        "both instances removed",
    )
    .await;
    assert!(s.rows(&b).is_empty());
    assert!(matches!(
        s.loader.remove_instance(b.plugin).await,
        Err(LoaderError::UnknownEntry(_))
    ));
}

/// The rows an instance spawned come back with it, each with its fiber, so a
/// host waits for them and rolls them back without `instance_rows` or
/// `entries`; the views stay readable once the instance is gone.
///
/// guarantee: loader/instances
#[tokio::test]
async fn an_instance_hands_back_the_views_of_its_rows() {
    let s = setup(DOC).await;
    let a = s.create("A").await;
    // The same rows, in the same order, as the report.
    let reported: Vec<&str> = a.report.iter().map(|(id, _)| id.as_str()).collect();
    let handed: Vec<&str> = a.rows.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(reported, ["doc-scope", "reader"]);
    assert_eq!(handed, reported);
    // They are the copies running in this instance: not the group fiber, not
    // another instance's copies.
    let fibers: Vec<PluginId> = a.rows.iter().map(|(_, view)| view.id).collect();
    assert!(fibers.iter().all(|id| *id != a.plugin));
    let b = s.create("B").await;
    assert!(s
        .rows(&b)
        .iter()
        .all(|(_, plugin)| !fibers.contains(plugin)));
    let listed: Vec<PluginId> = s
        .loader
        .instance_rows(a.view.instance())
        .into_iter()
        .map(|(_, view)| view.id)
        .collect();
    assert_eq!(fibers, listed);
    // Waiting for the rows to run takes the views alone.
    for (id, view) in &a.rows {
        view.running(Guard(HANG_GUARD).limit())
            .await
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        assert_eq!(view.state().state, FiberState::Active, "{id}");
    }
    // Rolling back takes the instance's group fiber alone; the views the
    // caller holds stay readable, and say the rows stopped.
    s.loader.remove_instance(a.plugin).await.unwrap();
    assert_eq!(a.rows.len(), 2);
    for (id, view) in &a.rows {
        assert_eq!(view.state().state, FiberState::Disposed, "{id}");
    }
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

/// guarantee: loader/edits-serialized, loader/instances, loader/instance-close-leaves-nothing
#[tokio::test]
async fn nested_instances_are_created_from_inside_and_read_outer_services() {
    let s = setup(NESTED).await;
    let a = s.create("A").await;
    let b = s.create("B").await;
    // The rows of the page inside A are A's too, after A's own rows.
    let rows = s.rows(&a);
    assert_eq!(
        rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["doc-scope", "creator", "page", "page-scope", "page-reader"]
    );
    // The same fibers `entries` lists in A's instances, without A's group.
    let scoped: Vec<_> = s
        .loader
        .entries()
        .into_iter()
        .filter(|row| {
            row.instance.as_ref().is_some_and(|instance| {
                instance
                    .chain
                    .iter()
                    .any(|(_, id)| *id == a.view.instance())
            }) && row.view.as_ref().is_some_and(|view| view.id != a.plugin)
        })
        .map(|row| (row.id, row.view.unwrap().id))
        .collect();
    assert_eq!(rows, scoped);
    assert!(s.rows(&b).iter().all(|row| !rows.contains(row)));
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
    assert!(s.rows(&a).is_empty());
    assert_eq!(s.rows(&b).len(), 5);
}

/// The nested instance inside `parent` whose group row is `group`, as the
/// copies a level down name it: the chain of `page`'s own copy is
/// `[("page", the page), ("doc", parent)]`.
fn nested_inside(s: &Setup, parent: &Instance, group: &str) -> rutis::host::InstanceId {
    s.loader
        .entries()
        .into_iter()
        .find_map(|entry| {
            if entry.id != group {
                return None;
            }
            let chain = entry.instance?.chain;
            let at = chain.iter().position(|(id, _)| id == group)?;
            let above = chain.get(at + 1)?;
            (above.1 == parent.view.instance()).then_some(chain[at].1)
        })
        .unwrap_or_else(|| panic!("{group} inside {:?} is not listed", parent.plugin))
}

/// A row query stops at the instance it names: the instance's own rows and
/// those of the instances nested in it, never the rows of the instance
/// above it, of its siblings, or of their nested ones.
///
/// guarantee: loader/instances
#[tokio::test]
async fn instance_rows_stop_at_the_instance_asked_for() {
    let s = setup(NESTED).await;
    let a = s.create("A").await;
    let b = s.create("B").await;
    let ids = |instance: rutis::host::InstanceId| {
        s.loader
            .instance_rows(instance)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    };
    let a_page = nested_inside(&s, &a, "page");
    let b_page = nested_inside(&s, &b, "page");
    assert_ne!(a_page, b_page);
    // The page inside A: its own rows, not A's (its group fiber included).
    assert_eq!(ids(a_page), ["page-scope", "page-reader"]);
    assert_eq!(
        ids(a.view.instance()),
        ["doc-scope", "creator", "page", "page-scope", "page-reader"]
    );
    assert_eq!(ids(b.view.instance()), ids(a.view.instance()));
    // Removing A takes the page inside it, and B keeps its own.
    s.loader.remove_instance(a.plugin).await.unwrap();
    assert!(ids(a_page).is_empty());
    assert!(ids(a.view.instance()).is_empty());
    assert_eq!(ids(b_page), ["page-scope", "page-reader"]);
    assert_eq!(ids(b.view.instance()).len(), 5);
}

/// An instance created by a plugin while it loads hands its rows to that
/// plugin: the creator gets the same rows, Active, as its own report lists.
/// The instance the host created does not list them — they are the nested
/// instance's, and its creator's to wait for.
///
/// guarantee: loader/instances
#[tokio::test]
async fn a_nested_instance_hands_its_rows_to_its_creator() {
    let s = setup(NESTED).await;
    let a = s.create("A").await;
    let log = s.take_log();
    assert!(
        log.contains(&"page rows page-scope=Active page-reader=Active".to_owned()),
        "{log:?}"
    );
    // The page and its rows belong to the nested instance: the outer one
    // hands back its own rows, and neither list has the nested instance's.
    assert_eq!(
        a.report
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["doc-scope", "creator"]
    );
    assert_eq!(
        a.rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["doc-scope", "creator"]
    );
    assert!(a.rows.iter().all(|(_, view)| view.id != a.plugin));
    // Only a query of the whole instance reaches the page's rows.
    assert_eq!(s.rows(&a).len(), 5);
}

const WITH_IDLE: &str = r#"[{ "insert": [
    { "id": "doc", "group": true, "instanced": true, "config": [
        { "id": "off", "name": "echo", "disabled": true, "config": { "label": "off" } },
        { "id": "on", "name": "echo", "config": { "label": "on" } }
    ] },
    { "id": "idle", "group": true, "instanced": true, "config": [
        { "id": "idle-off", "name": "echo", "disabled": true, "config": { "label": "idle" } }
    ] }
] }]"#;

/// An instance with no running rows lists none, and is no reason for
/// another instance to list the wrong rows.
///
/// guarantee: loader/instances
#[tokio::test]
async fn an_instance_with_no_running_rows_lists_none() {
    let s = setup(WITH_IDLE).await;
    let doc = s.create("A").await;
    let idle = s.loader.create_instance(&s.root, "idle").await.unwrap();
    let ids = |instance: &Instance| {
        s.rows(instance)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&doc), ["on"]);
    assert!(ids(&idle).is_empty());
    let another = s.create("B").await;
    assert_eq!(ids(&another), ["on"]);
    assert_eq!(ids(&doc), ["on"]);
    assert!(ids(&idle).is_empty());
}

/// Instances created at once each list their own rows: the index a query
/// reads is the state's, not a copy one creation made for itself.
///
/// guarantee: loader/instances
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creations_each_list_their_own_rows() {
    let s = setup(DOC).await;
    let mut tasks = Vec::new();
    for title in ["A", "B", "C", "D"] {
        let loader = s.loader.clone();
        let root = s.root.clone();
        tasks.push(tokio::spawn(async move {
            loader
                .create_instance(&root, "doc")
                .with(Title(title.into()))
                .await
        }));
    }
    let mut instances = Vec::new();
    for task in tasks {
        instances.push(task.await.unwrap().unwrap());
    }
    let mut seen: Vec<PluginId> = Vec::new();
    for instance in &instances {
        let rows = s.rows(instance);
        assert_eq!(
            rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            ["doc-scope", "reader"]
        );
        for (_, plugin) in rows {
            assert!(
                !seen.contains(&plugin),
                "another instance's copy {plugin:?} is listed"
            );
            seen.push(plugin);
        }
    }
    assert_eq!(seen.len(), instances.len() * 2);
}

/// guarantee: loader/instance-edits-all-or-none
#[tokio::test]
async fn edits_reach_every_instance_or_none() {
    let s = setup(DOC).await;
    let a = s.create("A").await;
    let b = s.create("B").await;
    s.take_log();
    let ids = |instance: &Instance| {
        s.rows(instance)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    };

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
    // The next read lists the new row, in configuration order.
    assert_eq!(ids(&a), ["doc-scope", "reader", "extra"]);
    assert_eq!(ids(&b), ["doc-scope", "reader", "extra"]);

    // Disabling and removing reach every copy.
    s.loader.set_disabled("extra", true).await.unwrap();
    assert_eq!(s.take_log(), ["cleanup x ", "cleanup x "]);
    assert!(s
        .copies("extra")
        .iter()
        .all(|e| matches!(e.status, EntryStatus::Disabled)));
    // A disabled copy is not live.
    assert_eq!(ids(&a), ["doc-scope", "reader"]);
    s.loader.remove("extra").await.unwrap();
    assert!(s.copies("extra").is_empty());
    assert_eq!(ids(&a), ["doc-scope", "reader"]);
    assert_eq!(ids(&b), ["doc-scope", "reader"]);

    // Restarting the row restarts every copy.
    s.loader.restart("reader").await.unwrap();
    assert_eq!(
        s.take_log(),
        ["cleanup r2 A", "cleanup r2 B", "read r2 A", "read r2 B"]
    );
}

/// guarantee: loader/instance-copy-dispose-self
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
    until(
        || {
            s.copies("quitter")
                .iter()
                .all(|e| matches!(e.status, EntryStatus::Stopped))
        },
        "the copies stopped",
    )
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

/// guarantee: loader/instance-copy-dispose-self
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
    until(
        || {
            s.copies("quitter")
                .iter()
                .all(|e| matches!(e.status, EntryStatus::Stopped))
        },
        "the copies stopped",
    )
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
    until(
        || s.log.lock().unwrap().iter().any(|l| l == "apply q"),
        "the copy applied again",
    )
    .await;
    // It quits again: stopped under a new fiber.
    until(
        || {
            s.copies("quitter")
                .iter()
                .all(|e| matches!(e.status, EntryStatus::Stopped) && e.plugin != Some(plugin))
        },
        "the copy stopped under a new fiber",
    )
    .await;
    assert!(s.loader.restart_instance(plugin).await.is_err());
}

/// guarantee: loader/instances
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

/// guarantee: loader/instances
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

/// guarantee: loader/instance-edits-all-or-none
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

/// guarantee: loader/saved-layers-restore
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

/// guarantee: loader/instance-edits-all-or-none
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

/// guarantee: loader/instance-edits-all-or-none
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

/// guarantee: loader/instances
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
    // Only the group has a fiber: it waits, the row under it is not spawned
    // yet, so it is in the report alone.
    assert_eq!(
        a.rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["g"]
    );
    let g = &a.rows[0].1;
    assert_eq!(g.state().state, FiberState::Pending);
    // A host waiting on it is told what it waits for, with the view alone.
    let error = g.running(Guard(HANG_GUARD).limit()).await.unwrap_err();
    assert!(error.to_string().contains("waiting"), "{error}");
    // The view is the row's own: once the service is there, the same view
    // runs, and the rows it then spawns are not in the creation's snapshot.
    s.loader
        .create(
            NewEntry {
                id: Some("provider".into()),
                name: "shared-provider".into(),
                config: json!({}),
                ..NewEntry::default()
            },
            Some("doc"),
            None,
        )
        .await
        .unwrap();
    until(
        || g.state().state == FiberState::Active,
        "the group loaded once the service was there",
    )
    .await;
    assert_eq!(a.rows.len(), 1);
}

const WITH_FAILURES: &str = r#"[{ "insert": [
    { "id": "doc", "group": true, "instanced": true, "config": [
        { "id": "ok", "name": "scoped-echo", "config": { "label": "ok" } },
        { "id": "bad", "name": "scoped-echo", "config": { "invalid": true } },
        { "id": "fails", "name": "scoped-echo", "config": { "label": "f", "fail_in": "A" } }
    ] }
] }]"#;

/// A row that failed keeps the view it was spawned with; a row refused
/// before it ran is only in the report. A host sees the failure and rolls
/// the instance back with `plugin` and the views alone.
///
/// guarantee: loader/instances
#[tokio::test]
async fn a_failed_row_hands_back_its_view_and_the_host_rolls_back() {
    let s = setup(WITH_FAILURES).await;
    let a = s.create("A").await;
    assert!(
        matches!(
            a.report.as_slice(),
            [(ok, InstanceResult::Active), (bad, InstanceResult::Failed(_)), (fails, InstanceResult::Failed(_))]
                if ok == "ok" && bad == "bad" && fails == "fails"
        ),
        "{:?}",
        a.report
    );
    // `bad` was refused at validation, before any fiber: only `ok` and
    // `fails` have one.
    assert_eq!(
        a.rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["ok", "fails"]
    );
    assert_eq!(a.rows[0].1.state().state, FiberState::Active);
    assert_eq!(a.rows[1].1.state().state, FiberState::Failed);
    // The failed row's view says why, without another query.
    let error = a.rows[1]
        .1
        .running(Guard(HANG_GUARD).limit())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("failed in A"), "{error}");
    // The host rolls back with the group fiber it was given.
    s.take_log();
    s.loader.remove_instance(a.plugin).await.unwrap();
    assert_eq!(s.take_log(), ["cleanup ok A"]);
    for (id, view) in &a.rows {
        assert_eq!(view.state().state, FiberState::Disposed, "{id}");
    }
    // And can create another instance of the group afterwards.
    let b = s.create("B").await;
    assert_eq!(b.rows.len(), 2, "{:?}", b.report);
    assert_eq!(b.rows[0].1.state().state, FiberState::Active);
}

/// Keeps the context it is applied in.
struct Hold(Arc<Mutex<Option<Ctx>>>);

impl DynPlugin for Hold {
    fn name(&self) -> &str {
        "hold"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.0.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

/// guarantee: loader/instances
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

/// guarantee: loader/instance-edits-all-or-none
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

/// guarantee: loader/instance-edits-all-or-none
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

/// guarantee: loader/instance-edits-all-or-none
#[tokio::test]
async fn the_same_error_in_another_copy_does_not_excuse_a_new_one() {
    let s = setup(DOC).await;
    s.create("A").await;
    for (id, name, group, parent) in [
        ("g", "", true, "doc"),
        ("inner", "picky-reader", false, "g"),
    ] {
        let entry = NewEntry {
            id: Some(id.into()),
            name: name.into(),
            group,
            config: json!({ "label": "i" }),
            ..NewEntry::default()
        };
        s.loader.create(entry, Some(parent), None).await.unwrap();
    }
    // The copy in B fails with the error a copy outside `doc` would give.
    let b = s.create("B").await;
    assert!(
        b.report
            .iter()
            .any(|(id, r)| id == "inner" && matches!(r, InstanceResult::Failed(_))),
        "{:?}",
        b.report
    );
    s.take_log();
    let before = s.store.patches();
    let error = s.loader.move_to("g", None, None).await.unwrap_err();
    assert!(matches!(error, LoaderError::Rejected { .. }), "{error}");
    assert!(s.take_log().is_empty());
    assert_eq!(s.store.patches(), before);
}

/// guarantee: loader/instances
#[tokio::test]
async fn isolate_scopes_are_per_copy_and_per_instance() {
    let mut catalog = ServiceCatalog::new();
    catalog.register::<Shared>("shared");
    let s = setup_with(
        r#"[{ "insert": [
        { "id": "doc", "group": true, "instanced": true, "config": [
            { "id": "private", "name": "shared-provider", "isolate": { "shared": true } },
            { "id": "named", "name": "shared-provider", "isolate": { "shared": "L" } },
            { "id": "reader", "name": "shared-reader", "isolate": { "shared": "L" } }
        ] }
    ] }]"#,
        MemStore::default(),
        catalog,
    )
    .await;
    // Each instance has its own private and named scopes: no collision,
    // and the reader sees the provider of its own instance.
    for title in ["A", "B"] {
        let instance = s.create(title).await;
        assert!(
            instance
                .report
                .iter()
                .all(|(_, r)| matches!(r, InstanceResult::Active)),
            "{title}: {:?}",
            instance.report
        );
    }
    let log = s.take_log();
    assert!(log.contains(&"read shared A".to_owned()), "{log:?}");
    assert!(log.contains(&"read shared B".to_owned()), "{log:?}");
}

/// guarantee: loader/closed-after-shutdown
#[tokio::test]
async fn creating_an_instance_is_closed_as_soon_as_root_shutdown_starts() {
    let s = setup(DOC).await;
    s.create("A").await;
    let shutdown = s.root.shutdown();
    let error = s
        .loader
        .create_instance(&s.root, "doc")
        .with(Title("B".into()))
        .await
        .unwrap_err();
    assert!(matches!(error, LoaderError::Closed), "{error:?}");
    shutdown.await.unwrap();
    let error = s.loader.create_instance(&s.root, "doc").await.unwrap_err();
    assert!(matches!(error, LoaderError::Closed), "{error:?}");
}

/// Local reference measurements, without a timing threshold. Run with:
/// `cargo test -p rutis-loader --release --test instances instance_creation_cost -- --ignored --nocapture`
#[tokio::test]
#[ignore = "manual performance sample"]
async fn instance_creation_cost_with_live_instances() {
    for live in [32, 256, 1000] {
        let s = setup(DOC).await;
        for _ in 0..live {
            s.create("resident").await;
        }
        let mut samples = Vec::new();
        // Keep the resident population fixed; removal and fixture logging
        // are outside the timed create_instance call. Warm up 20 times.
        for round in 0..120 {
            let start = std::time::Instant::now();
            let instance = s.create("sample").await;
            let elapsed = start.elapsed().as_nanos();
            if round >= 20 {
                samples.push(elapsed);
            }
            s.loader.remove_instance(instance.plugin).await.unwrap();
            s.take_log();
            s.changes.lock().unwrap().clear();
        }
        samples.sort_unstable();
        println!(
            "live={live} samples={} median_ns={} p10_ns={} p90_ns={}",
            samples.len(),
            samples[50],
            samples[10],
            samples[90]
        );
        s.root.shutdown().await.unwrap();
    }
}

/// One `instance_rows` call with `live` instances in the process (#1076):
/// what a per-instance query costs, by how many instances are loaded.
///
/// Local reference measurements, without a timing threshold. Run with:
/// `cargo test -p rutis-loader --release --test instances instance_rows_cost -- --ignored --nocapture`
#[tokio::test]
#[ignore = "manual performance sample"]
async fn instance_rows_cost_with_live_instances() {
    for (shape, base) in [("flat", DOC), ("nested", NESTED)] {
        for live in [1usize, 100, 1000] {
            let s = setup(base).await;
            let mut target = None;
            for _ in 0..live {
                let instance = s.create("resident").await;
                target.get_or_insert(instance);
            }
            let instance = target.unwrap();
            let id = instance.view.instance();
            let rows = s.rows(&instance).len();
            // Warm up 20 times, then sample 200 calls.
            for _ in 0..20 {
                std::hint::black_box(s.loader.instance_rows(id));
            }
            let mut samples = Vec::new();
            for _ in 0..200 {
                let start = std::time::Instant::now();
                let rows = s.loader.instance_rows(id);
                samples.push(start.elapsed().as_nanos());
                std::hint::black_box(rows);
            }
            samples.sort_unstable();
            println!(
                "shape={shape} live={live} rows={rows} median_ns={} p10_ns={} p90_ns={}",
                samples[100], samples[20], samples[180]
            );
            s.root.shutdown().await.unwrap();
        }
    }
}
