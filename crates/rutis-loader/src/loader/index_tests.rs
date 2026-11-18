//! The scope and instance indexes `instance_rows` reads (#1076): their
//! entries are exactly the running rows and the loaded instances, and the
//! query answers what the scan it replaced answered.

use rutis::host::{CtxHost, FiberView, InstanceId, PluginId};
use rutis::{Ctx, Plugin};
use serde_json::{json, Value};

use super::{Loader, LoaderOptions, LoaderPlugin, Slot, State};
use crate::patch::{Layer, Patch};
use crate::Builtins;

/// A row that only runs, for a copy to exist in each instance.
struct Plain;

impl Plugin for Plain {
    type Deps = ();

    fn name(&self) -> &str {
        "plain"
    }

    async fn apply(&self, _ctx: &Ctx, (): ()) -> rutis::Result {
        Ok(())
    }
}

/// Creates a `page` inside the instance it runs in, as a part of the
/// application would: a nested instance, created while the instance
/// above loads.
struct Creator;

impl Plugin for Creator {
    type Deps = (std::sync::Arc<Loader>,);

    fn name(&self) -> &str {
        "creator"
    }

    async fn apply(&self, ctx: &Ctx, (loader,): (std::sync::Arc<Loader>,)) -> rutis::Result {
        loader.create_instance(ctx, "page").await.map_err(|error| {
            rutis::Error::from(rutis::host::ErrorKind::PluginFailed(
                error.to_string().into(),
            ))
        })?;
        Ok(())
    }
}

/// Disposes itself once it runs: the copy stops (`Stopped`), and the
/// instance it is in stays.
struct Quit;

impl Plugin for Quit {
    type Deps = ();

    fn name(&self) -> &str {
        "quit"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.dispose_self()?;
        Ok(())
    }
}

/// `instance_rows` as it read before the indexes: the instance's number
/// by kernel, its scopes by walking every instance's parents, then every
/// desired row × each of those scopes.
impl State {
    fn instance_rows_scanning(&self, instance: InstanceId) -> Vec<(String, FiberView)> {
        let Some(root) = self
            .instances
            .iter()
            .find_map(|(number, record)| (record.kernel == instance).then_some(*number))
        else {
            return Vec::new();
        };
        let inside = |number: u64| {
            let mut scope = Some(number);
            while let Some(number) = scope {
                if number == root {
                    return true;
                }
                scope = self
                    .instances
                    .get(&number)
                    .and_then(|record| record.parent.as_ref())
                    .and_then(|parent| parent.scope);
            }
            false
        };
        let mut scopes: Vec<u64> = self
            .instances
            .keys()
            .copied()
            .filter(|number| inside(*number))
            .collect();
        scopes.sort_unstable();
        let mut rows = Vec::new();
        for row in &self.desired.rows {
            for scope in &scopes {
                let slot = Slot {
                    row: row.id.clone(),
                    scope: Some(*scope),
                };
                if let Some(running) = self.running.get(&slot) {
                    if running.view.instance() != instance {
                        rows.push((row.id.clone(), running.view.clone()));
                    }
                }
            }
        }
        rows
    }
}

fn patches(value: Value) -> Vec<Patch> {
    serde_json::from_value(value).unwrap()
}

/// Every running copy is under its scope, every instance under its
/// parent, and nothing else is in the indexes: a write that bypassed
/// `State::insert_running`, `State::remove_running` or the instance
/// methods would show here.
fn check_indexes(state: &State) {
    for slot in state.running.keys() {
        assert!(
            state
                .running_rows
                .get(&slot.scope)
                .is_some_and(|rows| rows.contains(&slot.row)),
            "the copy at {slot:?} is missing from the scope index"
        );
    }
    for (scope, rows) in &state.running_rows {
        for row in rows {
            let slot = Slot {
                row: row.clone(),
                scope: *scope,
            };
            assert!(
                state.running.contains_key(&slot),
                "the scope index holds {slot:?}, which does not run"
            );
        }
    }
    for (number, instance) in &state.instances {
        assert_eq!(
            state.instances_by_kernel.get(&instance.kernel),
            Some(number),
            "instance {number} is not indexed by its fiber"
        );
        let parent = instance.parent.as_ref().and_then(|parent| parent.scope);
        assert!(
            state
                .instance_children
                .get(&parent)
                .is_some_and(|children| children.contains(number)),
            "instance {number} is not under its parent"
        );
    }
    for (kernel, number) in &state.instances_by_kernel {
        assert_eq!(
            state.instances.get(number).map(|instance| instance.kernel),
            Some(*kernel),
            "the kernel index holds {number}, which is gone or rebuilt"
        );
    }
    for (parent, children) in &state.instance_children {
        for number in children {
            let instance = &state.instances[number];
            assert_eq!(
                instance.parent.as_ref().and_then(|parent| parent.scope),
                *parent,
                "instance {number} is not a child of its parent"
            );
        }
    }
}

/// The indexed rows of `instance` are the ones a full scan gives, and
/// the indexes are what the state holds.
fn check(loader: &Loader, instance: InstanceId) -> Vec<(String, PluginId)> {
    let ids = |rows: Vec<(String, FiberView)>| -> Vec<(String, PluginId)> {
        rows.into_iter().map(|(id, view)| (id, view.id)).collect()
    };
    let indexed = ids(loader.instance_rows(instance));
    let state = loader.inner.state.lock().unwrap();
    let scanned = ids(state.instance_rows_scanning(instance));
    check_indexes(&state);
    assert_eq!(indexed, scanned, "instance {instance:?}");
    indexed
}

/// Sibling instances with one nested instance each, a copy that stops, a
/// removal, an unknown instance and the loader's own context: every
/// per-instance answer is the scan's.
///
/// guarantee: loader/instances, loader/instance-close-leaves-nothing
#[tokio::test(flavor = "current_thread")]
async fn instance_rows_reads_the_indexes_the_scan_once_did() {
    let mut builtins = Builtins::new();
    builtins.register::<Value, _, _>("plain", |_| Ok(Plain));
    builtins.register::<Value, _, _>("creator", |_| Ok(Creator));
    builtins.register::<Value, _, _>("quit", |_| Ok(Quit));
    let root = Ctx::root().unwrap();
    let plugin = LoaderPlugin::new(builtins, LoaderOptions::default());
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let layers = vec![Layer::new(
        "base",
        patches(json!([{ "insert": [
            { "id": "doc", "group": true, "instanced": true, "config": [
                { "id": "doc-scope", "name": "plain" },
                { "id": "creator", "name": "creator" },
                { "id": "page", "group": true, "instanced": true, "config": [
                    { "id": "page-scope", "name": "plain" }
                ] },
                { "id": "quitter", "name": "quit" }
            ] },
            { "id": "global", "name": "plain" }
        ] }])),
    )];
    loader.reconcile(layers, None).await.unwrap();

    let a = loader.create_instance(&root, "doc").await.unwrap();
    let b = loader.create_instance(&root, "doc").await.unwrap();
    let a_id = a.view.instance();
    let b_id = b.view.instance();
    // Two copies of doc-scope and creator, then the page of each: the
    // nested instance's group and its rows are part of the instance
    // above, in configuration order.
    let ids = |rows: &[(String, PluginId)]| -> Vec<String> {
        rows.iter().map(|(id, _)| id.clone()).collect()
    };
    let plugins = |rows: &[(String, PluginId)]| -> Vec<PluginId> {
        rows.iter().map(|(_, plugin)| *plugin).collect()
    };
    // The copies that disposed themselves stop, alone and in both
    // instances: the state the index is read from after a stop.
    rutis_test_support::until(
        || {
            !ids(&check(&loader, a_id)).iter().any(|id| id == "quitter")
                && !ids(&check(&loader, b_id)).iter().any(|id| id == "quitter")
        },
        "the copies that quit are gone",
    )
    .await;
    // Both stopped: the quitter ran, and it is the running map the index is
    // read against that no longer has it.
    assert_eq!(loader.inner.state.lock().unwrap().stopped.len(), 2);
    let a_rows = check(&loader, a_id);
    assert_eq!(ids(&a_rows), ["doc-scope", "creator", "page", "page-scope"]);
    // The other instance answers the same rows, of its own fibers.
    let b_rows = check(&loader, b_id);
    assert_eq!(ids(&b_rows), ids(&a_rows));
    assert!(plugins(&a_rows)
        .iter()
        .all(|plugin| !plugins(&b_rows).contains(plugin)));
    // The loader's own context is no instance; an unknown fiber has no
    // rows either (checked below, once the instance is gone).
    assert!(check(&loader, root.instance()).is_empty());

    // Removing one leaves the other, index and scan alike.
    loader.remove_instance(a.plugin).await.unwrap();
    assert!(check(&loader, a_id).is_empty());
    assert_eq!(ids(&check(&loader, b_id)), ids(&b_rows));
    check_indexes(&loader.inner.state.lock().unwrap());

    root.shutdown().await.unwrap();
}
