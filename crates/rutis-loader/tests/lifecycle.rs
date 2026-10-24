//! P5: a plugin that disposes itself disables its row; volatile config
//! fields are applied in place.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use rutis::host::{BoxFuture, CtxHost, FiberState};
use rutis::Ctx;
use rutis_loader::edit::{volatile_key, Editable, NewEntry, VolatileUpdate};
use rutis_loader::report::{EntryStatus, LoaderChanged};
use rutis_loader::resolve::Resolved;
use rutis_loader::{Builtins, Layer, LoaderOptions, LoaderPlugin, Resolver};
use rutis_test_support::{still_pending, until};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

type Log = Arc<Mutex<Vec<String>>>;

#[derive(Deserialize, JsonSchema)]
struct Tunable {
    #[serde(default)]
    name: String,
    /// Changed in place.
    #[serde(default)]
    #[schemars(extend("x-volatile" = true))]
    level: u32,
    /// Dispose itself during apply.
    #[serde(default)]
    quit: bool,
}

struct TunablePlugin {
    name: String,
    level: u32,
    quit: bool,
    log: Log,
}

impl rutis::Plugin for TunablePlugin {
    type Deps = ();

    fn name(&self) -> &str {
        "tunable"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        self.log
            .lock()
            .unwrap()
            .push(format!("apply {} {}", self.name, self.level));
        if self.quit {
            ctx.dispose_self()?;
        }
        let log = self.log.clone();
        ctx.listen::<VolatileUpdate>()
            .name(volatile_key(ctx))
            .on(move |e, _| {
                log.lock()
                    .unwrap()
                    .push(format!("volatile {:?} {}", e.paths, e.config["level"]));
                std::future::ready(Ok(None))
            })?;
        Ok(())
    }
}

struct Tunables(Builtins);

impl Resolver for Tunables {
    fn resolve<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Arc<Resolved>, rutis_loader::LoaderError>> {
        self.0.resolve(name)
    }
}

async fn setup(
    store: &MemStore,
) -> (
    Ctx,
    rutis_loader::Loader,
    Log,
    Arc<Mutex<Vec<LoaderChanged>>>,
) {
    let log = Log::default();
    let mut builtins = Builtins::new();
    let tunables = log.clone();
    builtins.register("tunable", move |config: &Tunable| {
        Ok(TunablePlugin {
            name: config.name.clone(),
            level: config.level,
            quit: config.quit,
            log: tunables.clone(),
        })
    });
    let root = Ctx::root().unwrap();
    let plugin = LoaderPlugin::new(
        Tunables(builtins),
        LoaderOptions {
            persist: Arc::new(store.clone()),
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
    (root, loader, log, changes)
}

fn self_disposed(changes: &Arc<Mutex<Vec<LoaderChanged>>>) -> Option<(String, Option<String>)> {
    changes.lock().unwrap().iter().find_map(|c| match c {
        LoaderChanged::SelfDisposed { id, error } => Some((id.clone(), error.clone())),
        _ => None,
    })
}

/// guarantee: loader/self-dispose
#[tokio::test]
async fn a_plugin_that_disposes_itself_disables_its_row() {
    let store = MemStore::default();
    let (_root, loader, _log, changes) = setup(&store).await;
    loader
        .reconcile(
            vec![
                Layer::new("base", patches(json!([{ "insert": [{ "id": "lower", "name": "tunable", "config": { "quit": true } }] }]))),
                Layer::new("user", vec![]),
            ],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    until(|| self_disposed(&changes).is_some(), "SelfDisposed").await;
    assert_eq!(self_disposed(&changes), Some(("lower".into(), None)));
    until(
        || matches!(loader.get("lower").unwrap().status, EntryStatus::Disabled),
        "the row disabled",
    )
    .await;
    assert_eq!(
        store.patches(),
        patches(json!([{ "id": "lower", "disabled": true }]))
    );
}

/// guarantee: loader/self-dispose
#[tokio::test]
async fn without_an_editable_layer_the_failure_is_reported() {
    let store = MemStore::default();
    let (_root, loader, _log, changes) = setup(&store).await;
    loader
        .reconcile(
            vec![Layer::new("base", patches(json!([{ "insert": [{ "id": "q", "name": "tunable", "config": { "quit": true } }] }])))],
            None,
        )
        .await
        .unwrap();
    until(|| self_disposed(&changes).is_some(), "SelfDisposed").await;
    let (_, error) = self_disposed(&changes).unwrap();
    assert!(error.unwrap().contains("no editable layer"));
}

// In-process only (the kernel, the loader and its monitors are tasks of
// this runtime), so the paused clock applies.
/// guarantee: loader/self-dispose
#[tokio::test(start_paused = true)]
async fn loader_disposals_are_not_self_disposals() {
    let store = MemStore::default();
    let (root, loader, _log, changes) = setup(&store).await;
    let (sender, mut reports) = tokio::sync::mpsc::unbounded_channel();
    // Sends each row reported as disposing itself.
    root.on::<LoaderChanged>(move |e, _| {
        if let LoaderChanged::SelfDisposed { id, .. } = &*e {
            let _ = sender.send(id.clone());
        }
        std::future::ready(Ok(None))
    })
    .unwrap();
    reconcile(
        &loader,
        patches(json!([{ "insert": [{ "id": "t", "name": "tunable" }] }])),
        &store,
    )
    .await;
    loader.set_disabled("t", true).await.unwrap();
    loader.set_disabled("t", false).await.unwrap();
    loader
        .reconcile(vec![Layer::new("base", vec![])], None)
        .await
        .unwrap();
    // Monitors decide after the fibers are gone and report only a positive
    // finding. On the paused clock, still_pending's hour passes only once
    // every task waits: the monitors have run as far as they can, and had
    // every chance to (wrongly) report one.
    let mut report = Box::pin(reports.recv());
    assert!(
        still_pending(&mut report).await,
        "{:?}",
        changes.lock().unwrap()
    );
    assert!(self_disposed(&changes).is_none());
}

/// guarantee: loader/volatile-in-place
#[tokio::test]
async fn volatile_fields_change_in_place() {
    let store = MemStore::default();
    let (_root, loader, log, _changes) = setup(&store).await;
    reconcile(&loader, vec![], &store).await;
    loader
        .create(
            NewEntry {
                id: Some("t".into()),
                name: "tunable".into(),
                config: json!({ "name": "a", "level": 1 }),
                ..NewEntry::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let view = loader.get("t").unwrap().view.unwrap();
    let generation = view.state().generation;
    log.lock().unwrap().clear();

    loader
        .update("t", json!({ "name": "a", "level": 2 }))
        .await
        .unwrap();
    until(|| !log.lock().unwrap().is_empty(), "the volatile update").await;
    assert_eq!(*log.lock().unwrap(), ["volatile [[\"level\"]] 2"]);
    assert_eq!(view.state().generation, generation, "not restarted");
    assert_eq!(
        loader.evaluated("t").unwrap().unwrap(),
        json!({ "name": "a", "level": 2 })
    );
    assert!(store.patches()[0].insert.as_ref().unwrap()[0]["config"]["level"] == json!(2));

    // A restart builds from the stored config.
    log.lock().unwrap().clear();
    loader.restart("t").await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["apply a 2"]);

    // An ordinary field restarts it.
    log.lock().unwrap().clear();
    loader
        .update("t", json!({ "name": "b", "level": 2 }))
        .await
        .unwrap();
    assert_eq!(*log.lock().unwrap(), ["apply b 2"]);
    assert_eq!(view.state().state, FiberState::Active);
    let _: Value = json!(null);
}
