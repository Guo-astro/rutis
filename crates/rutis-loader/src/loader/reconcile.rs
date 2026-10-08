//! Driving the running fibers towards the desired tree.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use rutis::{
    CordisError, Ctx, Event, EventKey, FiberState, FiberView, PluginFactory, PluginId, TypeKey,
};
use serde_json::Value;

use crate::error::Failure;
use crate::patch::{apply_patches, Layer};
use crate::resolver::{Build, BuildLink, Resolved};
use crate::volatile::{volatile_change, volatile_paths, VolatileUpdate};
use crate::LoaderError;

use super::desired::{Desired, Eval, Row, RowScope};
use super::plugins::{EntryConfig, EntryFactory, GroupPlugin};
use super::{
    EntryInfo, EntryStatus, Failing, Group, Inner, InstanceInfo, LoaderChanged, ReconcileReport,
    Running, Slot, State,
};

/// The factory's own injects followed by the row's `inject`, deduplicated.
fn combined_injects(factory: &dyn PluginFactory<Value>, scope: &RowScope) -> Vec<TypeKey> {
    let mut keys = factory.injects().to_vec();
    for key in scope.inject_keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    keys
}

/// The catalog scope a row runs with: none for a resolver that handles
/// scope itself, else the row's resolved scope (`None` when it failed).
fn effective_scope(resolved: &Resolved, row: &Row) -> Option<RowScope> {
    if resolved.foreign_scope {
        Some(RowScope::default())
    } else {
        row.scope.as_ref().ok().cloned()
    }
}

/// Fibers whose disposal the caller awaits, and events to emit afterwards.
pub(super) type Dropped = (Vec<rutis::BoxFuture<'static, ()>>, Vec<LoaderChanged>);

impl Inner {
    pub(super) fn eval(&self) -> Eval<'_> {
        Eval {
            expressions: self.expressions.as_deref(),
            catalog: &self.catalog,
        }
    }

    /// Compose `layers` and read the rows, evaluating `disabled` with the
    /// root context.
    pub(super) fn build_desired(&self, layers: &[Layer], root: Option<&Ctx>) -> Desired {
        Desired::from_composed(apply_patches(layers), &self.eval(), root)
    }

    pub(super) fn next_token(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.next_token += 1;
        state.next_token
    }

    /// The instances enclosing a copy in `scope`, innermost first.
    pub(super) fn build_for(state: &State, scope: Option<u64>) -> Build {
        let mut chain = Vec::new();
        let mut current = scope;
        while let Some(number) = current {
            let Some(instance) = state.instances.get(&number) else {
                break;
            };
            chain.push(BuildLink {
                group: instance.group.clone(),
                instance: instance.kernel,
                values: instance.values.clone(),
            });
            current = instance.parent.as_ref().and_then(|p| p.scope);
        }
        Build { chain }
    }

    /// The factory `resolved` gives a copy in `scope`.
    pub(super) fn slot_factory(
        state: &State,
        resolved: &Resolved,
        scope: Option<u64>,
    ) -> Result<Arc<dyn PluginFactory<Value>>, CordisError> {
        match &resolved.scoped {
            None => Ok(resolved.factory.clone()),
            Some(scoped) => {
                catch_unwind(AssertUnwindSafe(|| scoped(&Self::build_for(state, scope))))
                    .unwrap_or_else(|_| Err(CordisError::PluginFailed("factory panicked".into())))
            }
        }
    }

    /// Whether the record at `slot` is an instance's own group fiber.
    pub(super) fn is_instance(state: &State, slot: &Slot) -> bool {
        slot.scope
            .and_then(|n| state.instances.get(&n))
            .is_some_and(|instance| instance.group == slot.row)
    }

    /// Register a running group's context and spawn its wanted children.
    /// A group instance that is no longer the current record of its row
    /// (an older spawn still loading) registers nothing.
    pub(super) fn attach(self: &Arc<Self>, group: Option<Slot>, token: u64, ctx: &Ctx) {
        let mut state = self.state.lock().unwrap();
        match &group {
            None => state.last_root = Some(ctx.clone()),
            Some(slot) => {
                if state.running.get(slot).map(|r| r.token) != Some(token) {
                    return;
                }
            }
        }
        state.groups.insert(
            group.clone(),
            Group {
                ctx: ctx.clone(),
                token,
            },
        );
        self.spawn_children(&mut state, &group);
    }

    /// Forget a group's context and the records spawned in it; the kernel
    /// unloads the fibers themselves. Only the instance that registered
    /// the context (same token) can remove it, so a late cleanup of an old
    /// instance leaves a newer one alone.
    pub(super) fn detach(self: &Arc<Self>, group: Option<Slot>, token: u64) {
        let (disposals, events) = {
            let mut state = self.state.lock().unwrap();
            if state.groups.get(&group).map(|g| g.token) != Some(token) {
                return;
            }
            state.groups.remove(&group);
            // A rebuilt instance starts its stopped copies again.
            if let Some(slot) = group.as_ref().filter(|s| Self::is_instance(&state, s)) {
                let scope = slot.scope;
                state.stopped.retain(|s, _| s.scope != scope);
            }
            Self::forget(&mut state, group, token);
            // Instances created in the context go with it.
            Self::prune_instances(&mut state)
        };
        if disposals.is_empty() && events.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let inner = self.clone();
        runtime.spawn(async move {
            for disposal in disposals {
                disposal.await;
            }
            for event in events {
                inner.emit(event);
            }
        });
    }

    /// Drop the records spawned in the group context `group` registered
    /// under `token`, and theirs.
    pub(super) fn forget(state: &mut State, group: Option<Slot>, token: u64) {
        let mut gone: Vec<(Option<Slot>, u64)> = vec![(group, token)];
        while let Some((parent, parent_token)) = gone.pop() {
            let children: Vec<Slot> = state
                .running
                .iter()
                .filter(|(_, r)| r.parent == parent && r.parent_token == parent_token)
                .map(|(slot, _)| slot.clone())
                .collect();
            for slot in children {
                let child = state.running.remove(&slot).unwrap();
                let key = Some(slot);
                if state.groups.get(&key).map(|g| g.token) == Some(child.token) {
                    state.groups.remove(&key);
                }
                gone.push((key, child.token));
            }
        }
    }

    /// Spawn the wanted children of a running group context. Instanced
    /// groups are only spawned by `create_instance`.
    pub(super) fn spawn_children(self: &Arc<Self>, state: &mut State, group: &Option<Slot>) {
        let Some((ctx, parent_token)) = state.groups.get(group).map(|g| (g.ctx.clone(), g.token))
        else {
            return;
        };
        let parent_row = group.as_ref().map(|s| s.row.clone());
        let scope = group.as_ref().and_then(|s| s.scope);
        let candidates: Vec<usize> = state
            .desired
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.parent == parent_row && !row.instanced)
            .map(|(i, _)| i)
            .collect();
        for index in candidates {
            let slot = Slot {
                row: state.desired.rows[index].id.clone(),
                scope,
            };
            self.spawn_row(state, index, group, parent_token, &ctx, slot);
        }
    }

    /// Spawn instances whose group fiber is gone but which still stand (an
    /// instance rebuilt after its group's scope changed), in the context
    /// they were created in.
    pub(super) fn respawn_instances(self: &Arc<Self>, state: &mut State) {
        let mut missing: Vec<u64> = state
            .instances
            .iter()
            .filter(|(n, instance)| {
                !state.running.contains_key(&Slot {
                    row: instance.group.clone(),
                    scope: Some(**n),
                })
            })
            .map(|(n, _)| *n)
            .collect();
        missing.sort_unstable();
        for number in missing {
            let instance = &state.instances[&number];
            let parent = instance.parent.clone();
            let slot = Slot {
                row: instance.group.clone(),
                scope: Some(number),
            };
            let Some((ctx, parent_token)) =
                state.groups.get(&parent).map(|g| (g.ctx.clone(), g.token))
            else {
                continue;
            };
            if parent_token != instance.parent_token {
                continue;
            }
            let Some(&index) = state.desired.by_id.get(&slot.row) else {
                continue;
            };
            if let Some(view) = self.spawn_row(state, index, &parent, parent_token, &ctx, slot) {
                let instance = state.instances.get_mut(&number).unwrap();
                instance.plugin = view.id;
                instance.kernel = view.instance();
            }
        }
    }

    /// Spawn one copy of the row at `index` as `slot` in the group context
    /// `ctx`. Returns its fiber, or `None` when it does not run.
    pub(super) fn spawn_row(
        self: &Arc<Self>,
        state: &mut State,
        index: usize,
        parent: &Option<Slot>,
        parent_token: u64,
        ctx: &Ctx,
        slot: Slot,
    ) -> Option<FiberView> {
        let row = &state.desired.rows[index];
        if state.running.contains_key(&slot)
            || state.stopped.contains_key(&slot)
            || !state.desired.wanted(row)
        {
            return None;
        }
        let name = row.name.clone().unwrap_or_default();
        let scope = row.raw_scope.signature();
        // A resolver that handles isolate/inject itself (foreign scope)
        // gets the parent context as is; others need the catalog's keys.
        let resolved = if row.group {
            None
        } else {
            match state.resolved.get(&name).cloned() {
                Some(Ok(resolved)) => Some(resolved),
                _ => return None,
            }
        };
        let foreign = resolved.as_ref().is_some_and(|r| r.foreign_scope);
        let rust_scope = if foreign {
            RowScope::default()
        } else {
            match &row.scope {
                Ok(scope) => scope.clone(),
                Err(error) => {
                    state.rejected.insert(slot, error.clone());
                    return None;
                }
            }
        };
        let row_ctx = rust_scope.context(ctx);
        let extra: Vec<TypeKey> = rust_scope.inject_keys().cloned().collect();
        state.next_token += 1;
        let token = state.next_token;
        if row.group {
            let view = row_ctx.plugin(GroupPlugin {
                inner: Arc::downgrade(self),
                slot: slot.clone(),
                injects: extra,
                token,
            });
            self.monitor(slot.clone(), view.clone());
            state.running.insert(
                slot,
                Running {
                    parent: parent.clone(),
                    token,
                    parent_token,
                    view: view.clone(),
                    group: true,
                    name,
                    injects: Vec::new(),
                    factory_name: String::new(),
                    resolved: None,
                    config: Value::Null,
                    scope,
                    ctx: row_ctx,
                },
            );
            return Some(view);
        }
        let resolved = resolved.expect("a leaf row resolved above");
        let id = row.id.clone();
        let rejected = |error: CordisError| LoaderError::Rejected {
            id: id.clone(),
            error: Arc::new(error),
        };
        let factory = match Self::slot_factory(state, &resolved, slot.scope) {
            Ok(factory) => factory,
            Err(error) => {
                state.rejected.insert(slot, rejected(error));
                return None;
            }
        };
        let config = match self.eval().value(&row.config, Some(&row_ctx)) {
            Ok(config) => config,
            Err(error) => {
                state.rejected.insert(slot, error);
                return None;
            }
        };
        // The kernel's first load only builds and validates the instance;
        // check the config here as `update` and the dry run do.
        let checked = catch_unwind(AssertUnwindSafe(|| factory.validate_config(&config)))
            .unwrap_or_else(|_| Err(CordisError::PluginFailed("validate_config panicked".into())));
        if let Err(error) = checked {
            state.rejected.insert(slot, rejected(error));
            return None;
        }
        state.rejected.remove(&slot);
        let injects = combined_injects(factory.as_ref(), &rust_scope);
        let view = row_ctx.plugin_with(
            EntryFactory {
                name: name.clone(),
                injects: injects.clone(),
            },
            EntryConfig {
                resolved: resolved.clone(),
                factory: factory.clone(),
                value: config.clone(),
            },
        );
        self.monitor(slot.clone(), view.clone());
        state.running.insert(
            slot,
            Running {
                parent: parent.clone(),
                token,
                parent_token,
                view: view.clone(),
                group: false,
                name,
                injects,
                factory_name: factory.name().to_owned(),
                resolved: Some(resolved),
                config,
                scope,
                ctx: row_ctx,
            },
        );
        Some(view)
    }

    /// Remove instances that no longer stand: their group row is gone, no
    /// longer instanced or wanted, moved, the instance enclosing them was
    /// removed, or the group context they were created in is gone (rebuilt
    /// or unloaded; the plugins that created them create them again).
    /// Their records go too.
    pub(super) fn prune_instances(state: &mut State) -> Dropped {
        let mut disposals: Vec<rutis::BoxFuture<'static, ()>> = Vec::new();
        let mut events = Vec::new();
        loop {
            let mut gone: Vec<u64> = state
                .instances
                .iter()
                .filter(|(_, instance)| {
                    let Some(row) = state.desired.row(&instance.group) else {
                        return true;
                    };
                    !row.instanced
                        || !state.desired.wanted(row)
                        || row.parent.as_deref() != instance.parent.as_ref().map(|p| p.row.as_str())
                        || instance
                            .parent
                            .as_ref()
                            .and_then(|p| p.scope)
                            .is_some_and(|outer| !state.instances.contains_key(&outer))
                        || state.groups.get(&instance.parent).map(|g| g.token)
                            != Some(instance.parent_token)
                })
                .map(|(n, _)| *n)
                .collect();
            if gone.is_empty() {
                break;
            }
            gone.sort_unstable();
            for number in gone {
                let instance = state.instances.remove(&number).unwrap();
                let slot = Slot {
                    row: instance.group.clone(),
                    scope: Some(number),
                };
                if let Some(running) = state.running.remove(&slot) {
                    let key = Some(slot);
                    if state.groups.get(&key).map(|g| g.token) == Some(running.token) {
                        state.groups.remove(&key);
                    }
                    Self::forget(state, key, running.token);
                    let dispose = running.view.dispose();
                    disposals.push(Box::pin(async move {
                        let _ = dispose.await;
                    }));
                }
                state.stopped.retain(|s, _| s.scope != Some(number));
                state.rejected.retain(|s, _| s.scope != Some(number));
                events.push(LoaderChanged::InstanceRemoved {
                    group: instance.group,
                    plugin: instance.plugin,
                });
            }
        }
        (disposals, events)
    }

    /// The record at `slot` goes because a group above it is going (being
    /// disposed, or already forgotten), not because it disposed itself.
    fn cascading(state: &State, slot: &Slot) -> bool {
        if state
            .groups
            .get(&None)
            .is_none_or(|root| root.ctx.diagnostics().shutting_down)
        {
            return true;
        }
        let mut parent = state.running.get(slot).and_then(|r| r.parent.clone());
        while let Some(slot) = parent {
            let Some(running) = state.running.get(&slot) else {
                return true;
            };
            if matches!(
                running.view.state().state,
                FiberState::Disposed | FiberState::Unloading
            ) || !state.groups.contains_key(&Some(slot.clone()))
            {
                return true;
            }
            parent = running.parent.clone();
        }
        false
    }

    /// Watch a spawned fiber; if it ends while still this slot's record, the
    /// plugin disposed itself (or, for an instance, the application closed
    /// it). Disposals the loader starts (or a group's cascade) remove the
    /// record first, so they are not mistaken for it.
    ///
    /// - An ordinary row: drop the record and disable the row, as cordis's
    ///   loader does.
    /// - A copy inside an instance: only that copy stops.
    /// - An instance's group fiber: the instance is removed.
    fn monitor(self: &Arc<Self>, slot: Slot, view: FiberView) {
        let weak = Arc::downgrade(self);
        let mut watch = view.watch();
        tokio::spawn(async move {
            loop {
                if watch.borrow().state == FiberState::Disposed {
                    break;
                }
                if watch.changed().await.is_err() {
                    return;
                }
            }
            let Some(inner) = weak.upgrade() else {
                return;
            };
            enum Outcome {
                Disable,
                Events(Dropped),
            }
            let outcome = {
                let mut state = inner.state.lock().unwrap();
                let state = &mut *state;
                match state.running.get(&slot) {
                    Some(running) if running.view.id == view.id => {}
                    _ => return,
                }
                let cascading = Self::cascading(state, &slot);
                let instance = Self::is_instance(state, &slot);
                let running = state.running.remove(&slot).unwrap();
                let key = Some(slot.clone());
                if state.groups.get(&key).map(|g| g.token) == Some(running.token) {
                    state.groups.remove(&key);
                }
                Self::forget(state, key, running.token);
                if cascading {
                    return;
                }
                if instance {
                    let number = slot.scope.unwrap();
                    let removed = state.instances.remove(&number).unwrap();
                    state.stopped.retain(|s, _| s.scope != Some(number));
                    state.rejected.retain(|s, _| s.scope != Some(number));
                    let (disposals, mut events) = Self::prune_instances(state);
                    events.insert(
                        0,
                        LoaderChanged::InstanceRemoved {
                            group: removed.group,
                            plugin: removed.plugin,
                        },
                    );
                    Outcome::Events((disposals, events))
                } else if let Some(number) = slot.scope {
                    state.stopped.insert(slot.clone(), view.id);
                    let plugin = state.instances.get(&number).map(|i| i.plugin);
                    let events = plugin
                        .map(|instance| LoaderChanged::Stopped {
                            id: slot.row.clone(),
                            instance,
                            plugin: view.id,
                        })
                        .into_iter()
                        .collect();
                    Outcome::Events((Vec::new(), events))
                } else {
                    Outcome::Disable
                }
            };
            match outcome {
                Outcome::Events((disposals, events)) => {
                    for disposal in disposals {
                        disposal.await;
                    }
                    for event in events {
                        inner.emit(event);
                    }
                }
                Outcome::Disable => {
                    let id = slot.row;
                    let loader = super::Loader {
                        inner: inner.clone(),
                    };
                    let error = loader
                        .set_disabled(&id, true)
                        .await
                        .err()
                        .map(|e| e.to_string());
                    inner.emit(LoaderChanged::SelfDisposed { id, error });
                }
            }
        });
    }

    pub(super) fn root(&self) -> Option<Ctx> {
        let state = self.state.lock().unwrap();
        state
            .groups
            .get(&None)
            .map(|g| g.ctx.clone())
            .or(state.last_root.clone())
    }

    pub(super) fn check_open(&self) -> Result<(), LoaderError> {
        match self.root() {
            Some(root) if root.diagnostics().shutting_down => Err(LoaderError::Closed),
            _ => Ok(()),
        }
    }

    pub(super) fn emit<E: Event>(&self, event: E) {
        let root = self
            .state
            .lock()
            .unwrap()
            .groups
            .get(&None)
            .map(|g| g.ctx.clone());
        if let Some(root) = root {
            let _ = root
                .events()
                .emit(&root, &EventKey::<E>::of(), Arc::new(event));
        }
    }

    /// The scopes a row's copies run in: `None` for a row outside instanced
    /// groups, else each instance of its instanced group, in order.
    pub(super) fn scopes(state: &State, row: &Row) -> Vec<Option<u64>> {
        match state.desired.instanced_group(row) {
            None => vec![None],
            Some(group) => {
                let mut scopes: Vec<u64> = state
                    .instances
                    .iter()
                    .filter(|(_, instance)| instance.group == group.id)
                    .map(|(n, _)| *n)
                    .collect();
                scopes.sort_unstable();
                scopes.into_iter().map(Some).collect()
            }
        }
    }

    /// Every failing copy. A failure is new only if the same copy did not
    /// fail the same way with the same row before.
    pub(super) fn failures(state: &State) -> Vec<Failing> {
        let mut out: Vec<Failing> = Vec::new();
        let mut push = |id: &str, scope: Option<u64>, error: String, value: &Value| {
            let failing = Failing {
                failure: Failure {
                    id: id.to_owned(),
                    error,
                },
                scope,
                value: value.to_string(),
            };
            if !out.contains(&failing) {
                out.push(failing);
            }
        };
        for row in &state.desired.rows {
            let parent_wanted = match &row.parent {
                None => true,
                Some(p) => state
                    .desired
                    .row(p)
                    .is_some_and(|p| state.desired.wanted(p)),
            };
            if !parent_wanted {
                continue;
            }
            if let Some(invalid) = &row.invalid {
                push(&row.id, None, invalid.to_string(), &row.value);
                continue;
            }
            if let Err(e) = &row.disabled {
                push(&row.id, None, e.to_string(), &row.value);
                continue;
            }
            if matches!(row.disabled, Ok(true)) {
                continue;
            }
            for scope in Self::scopes(state, row) {
                let slot = Slot {
                    row: row.id.clone(),
                    scope,
                };
                let error = if let Some(rejected) = state.rejected.get(&slot) {
                    Some(rejected.to_string())
                } else if let Some(running) = state.running.get(&slot) {
                    let snapshot = running.view.state();
                    (snapshot.state == FiberState::Failed).then(|| {
                        snapshot
                            .error
                            .map_or_else(|| "failed".to_owned(), |e| e.to_string())
                    })
                } else if row.group || state.stopped.contains_key(&slot) {
                    None
                } else {
                    match row.name.as_ref().and_then(|n| state.resolved.get(n)) {
                        Some(Err(e)) => Some(e.to_string()),
                        _ => None,
                    }
                };
                if let Some(error) = error {
                    push(&row.id, scope, error, &row.value);
                }
            }
        }
        out
    }

    /// Bring the running tree to the current layers and wait until settled.
    /// The caller holds the operation lock.
    pub(super) async fn reconcile_inner(self: &Arc<Self>) -> ReconcileReport {
        let (before, names) = {
            let mut state = self.state.lock().unwrap();
            let before = Self::failures(&state);
            let root = state.groups.get(&None).map(|g| g.ctx.clone());
            state.desired = self.build_desired(&state.composed_layers(), root.as_ref());
            let names: Vec<String> = state
                .desired
                .rows
                .iter()
                .filter(|row| !row.group && state.desired.wanted(row))
                .filter_map(|row| row.name.clone())
                .filter(|name| !state.resolved.contains_key(name))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            (before, names)
        };
        for name in names {
            let resolved = self.resolver.resolve(&name).await;
            self.state.lock().unwrap().resolved.insert(name, resolved);
        }

        // Old instances go first: a respawned provider must not meet its
        // predecessor's service, and a group's cleanup must not race the
        // registration of its successor.
        let (disposals, events) = {
            let mut state = self.state.lock().unwrap();
            let state = &mut *state;
            // Records to keep as they are, or to update in place.
            let mut keep: HashSet<Slot> = HashSet::new();
            for (slot, running) in &state.running {
                let Some(row) = state.desired.row(&slot.row) else {
                    continue;
                };
                if row.parent.as_deref() != running.parent.as_ref().map(|p| p.row.as_str())
                    || row.group != running.group
                    || row.instanced != Self::is_instance(state, slot)
                    || state.desired.instanced_group(row).is_some() != slot.scope.is_some()
                    || !state.desired.wanted(row)
                    || row.raw_scope.signature() != running.scope
                {
                    continue;
                }
                if !row.group {
                    let name = row.name.clone().unwrap_or_default();
                    match state.resolved.get(&name) {
                        Some(Ok(resolved)) => {
                            let Ok(factory) = Self::slot_factory(state, resolved, slot.scope)
                            else {
                                continue;
                            };
                            let same = effective_scope(resolved, row).is_some_and(|s| {
                                combined_injects(factory.as_ref(), &s) == running.injects
                            }) && factory.name() == running.factory_name;
                            if !same {
                                continue;
                            }
                        }
                        _ => continue,
                    }
                }
                keep.insert(slot.clone());
            }
            // A record whose group goes away goes with it.
            loop {
                let orphans: Vec<Slot> = keep
                    .iter()
                    .filter(|slot| {
                        state.running[*slot]
                            .parent
                            .as_ref()
                            .is_some_and(|p| !keep.contains(p))
                    })
                    .cloned()
                    .collect();
                if orphans.is_empty() {
                    break;
                }
                for slot in orphans {
                    keep.remove(&slot);
                }
            }
            let dropped: Vec<Slot> = state
                .running
                .keys()
                .filter(|slot| !keep.contains(*slot))
                .cloned()
                .collect();
            let mut disposals: Vec<rutis::BoxFuture<'static, ()>> = Vec::new();
            for slot in dropped {
                let running = state.running.remove(&slot).unwrap();
                let key = Some(slot);
                if state.groups.get(&key).map(|g| g.token) == Some(running.token) {
                    state.groups.remove(&key);
                }
                let dispose = running.view.dispose();
                disposals.push(Box::pin(async move {
                    let _ = dispose.await;
                }));
            }
            let (pruned, events) = Self::prune_instances(state);
            disposals.extend(pruned);
            let desired = &state.desired;
            let instances = &state.instances;
            let stands = |slot: &Slot| {
                desired
                    .row(&slot.row)
                    .is_some_and(|row| desired.wanted(row))
                    && slot.scope.is_none_or(|n| instances.contains_key(&n))
            };
            state.rejected.retain(|slot, _| stands(slot));
            state.stopped.retain(|slot, _| stands(slot));
            (disposals, events)
        };
        for disposal in disposals {
            disposal.await;
        }
        for event in events {
            self.emit(event);
        }

        let mut updates = Vec::new();
        let mut notifications = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            let state = &mut *state;
            let mut settled = Vec::new();
            let mut unevaluable = Vec::new();
            let mut volatile = Vec::new();
            for (slot, running) in state.running.iter() {
                if running.group {
                    continue;
                }
                let row = state.desired.row(&slot.row).unwrap();
                let name = row.name.clone().unwrap_or_default();
                let Some(Ok(resolved)) = state.resolved.get(&name) else {
                    continue;
                };
                let Ok(factory) = Self::slot_factory(state, resolved, slot.scope) else {
                    continue;
                };
                // Expressions are evaluated where the plugin runs.
                let desired = match self.eval().value(&row.config, Some(&running.ctx)) {
                    Ok(value) => value,
                    Err(error) => {
                        unevaluable.push((slot.clone(), error));
                        continue;
                    }
                };
                let same_module = running
                    .resolved
                    .as_ref()
                    .is_some_and(|r| Arc::ptr_eq(r, resolved));
                if same_module && running.config == desired {
                    // Already running what is wanted: an earlier rejection
                    // no longer applies.
                    settled.push(slot.clone());
                    continue;
                }
                if same_module && running.view.state().state == FiberState::Active {
                    let paths = resolved
                        .schema
                        .as_ref()
                        .map(volatile_paths)
                        .unwrap_or_default();
                    if let Some(changed) = volatile_change(&running.config, &desired, &paths) {
                        volatile.push((slot.clone(), resolved.clone(), factory, desired, changed));
                        continue;
                    }
                }
                let config = EntryConfig {
                    resolved: resolved.clone(),
                    factory,
                    value: desired,
                };
                updates.push((
                    slot.clone(),
                    running.view.clone(),
                    running.view.update(config),
                    name,
                ));
            }
            for slot in settled {
                state.rejected.remove(&slot);
            }
            // The plugin keeps its previous config.
            for (slot, error) in unevaluable {
                state.rejected.insert(slot, error);
            }
            // Volatile-only changes: store without restarting, then tell the
            // plugin. A refused store falls back to an ordinary update.
            for (slot, resolved, factory, desired, paths) in volatile {
                let Some(running) = state.running.get_mut(&slot) else {
                    continue;
                };
                let config = EntryConfig {
                    resolved: resolved.clone(),
                    factory,
                    value: desired.clone(),
                };
                if running.view.set_config(config.clone()).is_ok() {
                    running.config = desired.clone();
                    state.rejected.remove(&slot);
                    let running = &state.running[&slot];
                    notifications.push((
                        running.ctx.clone(),
                        crate::volatile::key_for(running.view.instance()),
                        VolatileUpdate {
                            paths,
                            config: desired,
                        },
                    ));
                } else {
                    let name = running.name.clone();
                    updates.push((
                        slot,
                        running.view.clone(),
                        running.view.update(config),
                        name,
                    ));
                }
            }
            let groups: Vec<Option<Slot>> = state.groups.keys().cloned().collect();
            for group in groups {
                self.spawn_children(state, &group);
            }
            self.respawn_instances(state);
        }
        for (ctx, key, update) in notifications {
            let _ = ctx.events().emit(&ctx, &key, Arc::new(update));
        }
        for (slot, view, update, name) in updates {
            let result = update.await;
            // Record what the kernel actually holds: a rejected update keeps
            // the previous config; one that failed in apply stored the new.
            let current = view.current_config::<EntryConfig>();
            let mut state = self.state.lock().unwrap();
            let Some(running) = state.running.get_mut(&slot) else {
                continue;
            };
            if running.view.id != view.id {
                continue;
            }
            if let Some(current) = current {
                running.resolved = Some(current.resolved.clone());
                running.config = current.value.clone();
                running.name = name;
            }
            match result {
                Err(error) if view.state().state != FiberState::Failed => {
                    let id = slot.row.clone();
                    state
                        .rejected
                        .insert(slot, LoaderError::Rejected { id, error });
                }
                _ => {
                    state.rejected.remove(&slot);
                }
            }
        }
        self.settle(None).await;

        let state = self.state.lock().unwrap();
        let after = Self::failures(&state);
        let new_failures = Failing::public(after.iter().filter(|f| !before.contains(f)));
        ReconcileReport {
            warnings: state.desired.warnings.clone(),
            issues: state.desired.issues.clone(),
            new_failures,
            failures: Failing::public(after.iter()),
        }
    }

    /// Wait until no running record is in transition: every record, or with
    /// `scope`, those of one instance. Groups spawn children while loading,
    /// so repeat until the set of records stops changing.
    pub(super) async fn settle(&self, scope: Option<u64>) {
        loop {
            let views: Vec<FiberView> = {
                let state = self.state.lock().unwrap();
                state
                    .running
                    .iter()
                    .filter(|(slot, _)| scope.is_none() || slot.scope == scope)
                    .map(|(_, r)| r.view.clone())
                    .collect()
            };
            let before: HashSet<PluginId> = views.iter().map(|v| v.id).collect();
            for view in &views {
                let _ = view.await;
            }
            let after: HashSet<PluginId> = {
                let state = self.state.lock().unwrap();
                state
                    .running
                    .iter()
                    .filter(|(slot, _)| scope.is_none() || slot.scope == scope)
                    .map(|(_, r)| r.view.id)
                    .collect()
            };
            if before == after {
                return;
            }
        }
    }

    /// The entry of `row`: the row itself (`scope` `None`), or its copy in
    /// the instance `scope`.
    pub(super) fn info(state: &State, row: &Row, scope: Option<u64>) -> EntryInfo {
        let slot = Slot {
            row: row.id.clone(),
            scope,
        };
        let in_instances = state.desired.instanced_group(row).is_some();
        let running = state.running.get(&slot);
        let resolved = row
            .name
            .as_ref()
            .and_then(|n| state.resolved.get(n))
            .and_then(|r| r.as_ref().ok());
        let status = if let Some(invalid) = &row.invalid {
            EntryStatus::Unresolved(invalid.clone())
        } else if let Err(e) = &row.disabled {
            EntryStatus::Unresolved(e.clone())
        } else if matches!(row.disabled, Ok(true)) {
            EntryStatus::Disabled
        } else if in_instances && scope.is_none() {
            // The row itself: its copies run in instances.
            EntryStatus::Inactive
        } else if let Some(running) = running {
            EntryStatus::Running(running.view.state())
        } else if state.stopped.contains_key(&slot) {
            EntryStatus::Stopped
        } else if let Some(rejected) = state.rejected.get(&slot) {
            EntryStatus::Unresolved(rejected.clone())
        } else if let Some(Err(e)) = row.name.as_ref().and_then(|n| state.resolved.get(n)) {
            if !row.group && state.desired.wanted(row) {
                EntryStatus::Unresolved(e.clone())
            } else {
                EntryStatus::Inactive
            }
        } else {
            EntryStatus::Inactive
        };
        let instance = scope.and_then(|number| {
            let record = state.instances.get(&number)?;
            Some(InstanceInfo {
                plugin: record.plugin,
                chain: Self::build_for(state, Some(number))
                    .chain
                    .into_iter()
                    .map(|link| (link.group, link.instance))
                    .collect(),
            })
        });
        EntryInfo {
            id: row.id.clone(),
            options: row.value.clone(),
            parent: row.parent.clone(),
            owner: row.owner.clone(),
            overridden: row
                .overridden
                .iter()
                .map(|(field, &layer)| {
                    let name = state.layer_name(layer).unwrap_or_default().to_owned();
                    (field.clone(), name)
                })
                .collect(),
            status,
            rejected: state.rejected.get(&slot).cloned(),
            plugin: running
                .map(|r| r.view.id)
                .or_else(|| state.stopped.get(&slot).copied()),
            view: running.map(|r| r.view.clone()),
            schema: resolved.and_then(|r| r.schema.clone()),
            meta: resolved.map(|r| r.meta.clone()).unwrap_or(Value::Null),
            instance,
        }
    }
}
