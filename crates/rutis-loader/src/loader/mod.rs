//! The loader: desired state, reconcile, editable layer and persistence.
//!
//! - `api`: the public `Loader` methods;
//! - `desired`: the composed tree as rows the loader can act on;
//! - `plugins`: the kernel glue (entry factory, group and loader plugins);
//! - `reconcile`: driving fibers towards the desired tree;
//! - `commit`: imperative edits, rollback and the persistence queue;
//! - `instances`: instances of `instanced` groups, created on demand.

mod api;
mod check;
mod commit;
mod config_step;
mod desired;
mod instances;
mod plugins;
mod reconcile;
mod service;
pub mod status;
mod waits;

/// The scope and instance indexes `instance_rows` reads (#1076): their
/// entries are exactly the running rows and the loaded instances, and a
/// per-instance query answers what the scan it replaced answered.
#[cfg(test)]
mod index_tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use rutis::host::{FiberView, InstanceId, PluginId, Snapshot, TypeKey};
use rutis::{Ctx, Event};

use serde_json::Value;

use crate::catalog::{Expressions, ServiceCatalog};
use crate::edit::Edit;
use crate::error::Failure;
use crate::patch::{Layer, Owner, PatchWarning};
use crate::persist::{NoPersist, Persist, Version};
use crate::resolver::{Resolved, Resolver, Values};
use crate::LoaderError;

pub use check::{
    CheckReport, Clash, ConfigStatus, CycleLink, Provider, Requirement, RowCheck, RowStatus,
    Supply, Unknown,
};
pub use config_step::ConfigWarnings;
use desired::Desired;
pub use instances::{CreateInstance, Instance, InstanceResult};
pub use plugins::LoaderPlugin;
pub use service::cannot_reload;
pub(crate) use service::LoaderService;

/// A [`Loader`] that does not keep it alive.
#[derive(Clone)]
#[cfg_attr(not(feature = "runtimes"), allow(dead_code))]
pub(crate) struct WeakLoader(std::sync::Weak<Inner>);

impl Loader {
    #[cfg_attr(not(feature = "runtimes"), allow(dead_code))]
    pub(crate) fn downgrade(&self) -> WeakLoader {
        WeakLoader(Arc::downgrade(&self.inner))
    }

    /// The loader's own context while it is mounted: what plugins of its
    /// resolvers are mounted under.
    #[cfg(feature = "runtimes")]
    pub(crate) fn ctx(&self) -> Option<Ctx> {
        let state = self.inner.state.lock().unwrap();
        state.groups.get(&None).map(|group| group.ctx.clone())
    }
}

#[cfg(feature = "runtimes")]
impl WeakLoader {
    pub(crate) fn upgrade(&self) -> Option<Loader> {
        self.0.upgrade().map(|inner| Loader { inner })
    }
}
pub use waits::{Wait, WaitReason};

pub struct LoaderOptions {
    pub persist: Arc<dyn Persist>,
    /// Service names usable in `isolate`, `inject` and expressions.
    pub catalog: ServiceCatalog,
    /// Evaluates `{ "__jsExpr": .. }` nodes; without it such rows are
    /// `Unresolved`.
    pub expressions: Option<Arc<dyn Expressions>>,
}

impl Default for LoaderOptions {
    fn default() -> Self {
        Self {
            persist: Arc::new(NoPersist),
            catalog: ServiceCatalog::default(),
            expressions: None,
        }
    }
}

/// The layer that imperative edits change, and the stored version it was
/// read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editable {
    pub layer: String,
    pub version: Version,
}

impl Editable {
    pub fn new(layer: impl Into<String>, version: Version) -> Self {
        Self {
            layer: layer.into(),
            version,
        }
    }
}

/// Which row a fiber belongs to, for plugins that act on their row's
/// settings themselves (see `Resolved::foreign_scope`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowInfo {
    pub id: String,
    /// `isolate` as (service name, scope label): rows naming the same label
    /// share a scope; a private scope's label is unique to the row. Inside
    /// an instance the labels are the instance's own: a private scope is
    /// one per copy, a named one is shared within the instance. The row's
    /// own: an `isolate` of a group around it is not here.
    pub isolate: Vec<(String, String)>,
    /// `inject`: extra service names the row waits for. A runtime's row
    /// (`Resolved::foreign_scope`) waits only for those the service catalog
    /// shares, and leases them; the others the runtime resolves itself.
    pub inject: Vec<String>,
}

/// A new row for [`Loader::create`].
#[derive(Debug, Clone, Default)]
pub struct NewEntry {
    /// Generated when absent.
    pub id: Option<String>,
    pub name: String,
    pub config: Value,
    pub group: bool,
    /// An instanced group: created only through [`Loader::create_instance`].
    pub instanced: bool,
    pub disabled: bool,
    /// Catalog names of extra services the row waits for.
    pub inject: Vec<String>,
    /// Catalog name → isolation.
    pub isolate: BTreeMap<String, Isolate>,
}

/// One `isolate` entry: a scope private to the row, or one shared by every
/// row naming the same label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Isolate {
    Private,
    Shared(String),
}

impl Isolate {
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Isolate::Private => Value::Bool(true),
            Isolate::Shared(label) => Value::String(label.clone()),
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum EntryStatus {
    /// The row itself is disabled.
    Disabled,
    /// An enclosing group is not running, or the loader is not mounted.
    Inactive,
    /// The row cannot run: invalid, unknown module, or unsupported content.
    Unresolved(LoaderError),
    Running(Snapshot),
    /// A copy inside an instance disposed itself; only that copy stopped.
    /// [`Loader::restart_instance`] with [`EntryInfo::plugin`] (its last
    /// fiber), a change to the row, or a rebuild of the instance starts it
    /// again.
    Stopped,
}

/// The instance an entry runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstanceInfo {
    /// The instance's group fiber.
    pub plugin: PluginId,
    /// Enclosing instances, innermost first: (group id, the group fiber's
    /// `ctx.instance()`).
    pub chain: Vec<(String, InstanceId)>,
}

#[derive(Clone)]
#[non_exhaustive]
pub struct EntryInfo {
    pub id: String,
    /// The row as composed (raw; expressions are not evaluated).
    pub options: Value,
    pub parent: Option<String>,
    pub owner: Owner,
    /// Field → name of the last layer that replaced it.
    pub overridden: BTreeMap<String, String>,
    pub status: EntryStatus,
    /// The last config the loader tried to apply was refused by the plugin's
    /// validation. A running row keeps its previous config.
    pub rejected: Option<LoaderError>,
    pub plugin: Option<PluginId>,
    pub view: Option<FiberView>,
    pub schema: Option<Value>,
    pub meta: Value,
    /// For a copy of a row inside an instance, that instance. A row inside
    /// instanced groups is listed once without it (the row itself), then
    /// once per instance.
    pub instance: Option<InstanceInfo>,
}

impl std::fmt::Debug for EntryInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntryInfo")
            .field("id", &self.id)
            .field("options", &self.options)
            .field("parent", &self.parent)
            .field("owner", &self.owner)
            .field("overridden", &self.overridden)
            .field("status", &self.status)
            .field("rejected", &self.rejected)
            .field("plugin", &self.plugin)
            .field("instance", &self.instance)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    pub warnings: Vec<PatchWarning>,
    /// Rows skipped while reading the tree (missing or duplicate ids).
    pub issues: Vec<String>,
    /// Rows failing after this reconcile that were not failing before.
    pub new_failures: Vec<Failure>,
    /// Every row failing after this reconcile.
    pub failures: Vec<Failure>,
}

/// Emitted on the root bus after a reconcile or an edit completes. As data
/// (an event crosses to other processes as data): `{ "type": ..., ... }`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum LoaderChanged {
    Reconciled,
    Edited(Edit),
    Reloaded(String),
    /// A row's plugin disposed itself; the row was disabled in the editable
    /// layer (`error` when that edit failed, e.g. the host keeps no editable
    /// configuration).
    SelfDisposed {
        id: String,
        error: Option<String>,
    },
    /// The copy `plugin` of row `id` inside the instance `instance`
    /// disposed itself; only that copy stopped and no layer changed.
    Stopped {
        id: String,
        instance: PluginId,
        plugin: PluginId,
    },
    /// An instance of the instanced group `group` was created.
    InstanceCreated {
        group: String,
        plugin: PluginId,
    },
    /// An instance of `group` was removed, closed, or dropped with its row.
    InstanceRemoved {
        group: String,
        plugin: PluginId,
    },
}

impl Event for LoaderChanged {
    const NAME: &'static str = "rutis-loader::changed";
    type Value = ();
}

/// A queued, unsaved edit no longer applied when replayed on newer content.
/// As data, `error` is its message (read back as
/// [`LoaderError::InvalidEntry`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingEditDropped {
    pub edit: Edit,
    #[serde(with = "error_text")]
    pub error: LoaderError,
}

/// A [`LoaderError`] as its message.
mod error_text {
    use super::LoaderError;

    pub(super) fn serialize<S: serde::Serializer>(
        error: &LoaderError,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(error)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<LoaderError, D::Error> {
        let message: String = serde::Deserialize::deserialize(deserializer)?;
        Ok(LoaderError::InvalidEntry(message))
    }
}

impl Event for PendingEditDropped {
    const NAME: &'static str = "rutis-loader::pending-edit-dropped";
    type Value = ();
}

/// Handle to the loader; cheap to clone. Mount it with [`LoaderPlugin`].
#[derive(Clone)]
pub struct Loader {
    inner: Arc<Inner>,
}

/// The rows the loader follows right now: its follower map's size, for the
/// tests of the listener's life (`rutis_loader::followed_rows`).
#[doc(hidden)]
pub fn followed_rows(loader: &Loader) -> usize {
    loader.inner.followers.followed()
}

impl Loader {
    /// The instances enclosing a plugin mounted at `at`, innermost first:
    /// what its keys are computed for, as a row's copy there would be. They
    /// are the fibers marked with [`Ctx::mark_scope`] around `at`, this
    /// loader's instances or a host's own; an instance of this loader also
    /// brings the values it was created with.
    #[cfg(feature = "runtimes")]
    pub(crate) fn build_at(&self, at: &Ctx) -> crate::resolver::Build {
        use rutis::internal::CtxInternal;
        let scope = at.scope_instances();
        let state = self.inner.state.lock().unwrap();
        let chain = scope
            .into_iter()
            .rev()
            .map(|(instance, group)| {
                let values = state
                    .instance_of_kernel(instance)
                    .and_then(|number| state.instances.get(&number))
                    .map(|record| record.values.clone())
                    .unwrap_or_default();
                crate::resolver::BuildLink {
                    group,
                    instance,
                    values,
                }
            })
            .collect();
        crate::resolver::Build { chain, row: None }
    }

    #[cfg(feature = "runtimes")]
    pub(crate) fn catalog(&self) -> &ServiceCatalog {
        &self.inner.catalog
    }
}

struct Inner {
    resolver: Arc<dyn Resolver>,
    persist: Arc<dyn Persist>,
    catalog: ServiceCatalog,
    expressions: Option<Arc<dyn Expressions>>,
    /// Serializes reconcile and edits.
    op: tokio::sync::Mutex<()>,
    /// The transitions (fiber, state sequence) an operation was waiting
    /// for when its caller gave up on it (dropped its future): later
    /// operations do not wait for them ([`Inner::settle`], #934).
    abandoned: Mutex<std::collections::HashSet<(rutis::host::PluginId, u64)>>,
    /// A rekeying reconcile is spawned and has not read the keys yet
    /// ([`Loader::rekeying`]): announcements meanwhile need no other.
    #[cfg(feature = "runtimes")]
    rekey_pending: std::sync::atomic::AtomicBool,
    /// The rows, instances and config values warned about
    /// ([`config_step`]).
    warned: config_step::Warned,
    /// Where extra config keys are written ([`Loader::on_config_warning`]).
    config_warnings: Mutex<ConfigWarnings>,
    /// The rows this loader follows with one `FiberStatusChanged` listener.
    followers: Arc<crate::framework::Followers>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The application's layers, as given to `reconcile`.
    layers: Vec<Layer>,
    editable: Option<usize>,
    version: Version,
    pending: Vec<Edit>,
    desired: Desired,
    resolved: HashMap<String, Result<Arc<Resolved>, LoaderError>>,
    /// Running group contexts; `None` is the loader's own (the root).
    groups: HashMap<Option<Slot>, Group>,
    running: HashMap<Slot, Running>,
    /// The rows running in each scope (`None`: outside instances), by row
    /// id: one instance's rows are `instance_rows`' to read without the
    /// rows of the instances around it (#1076). Written by
    /// [`State::insert_running`] and [`State::remove_running`] only, which
    /// is what keeps `running` and this index in step. A row id left here
    /// whose slot is gone is skipped (the query reads `running` anyway), so
    /// a missed removal costs a lookup, never a wrong answer; a missed
    /// *insert* would lose a row, which is why `running` is written through
    /// those two methods only.
    running_rows: HashMap<Option<u64>, BTreeSet<String>>,
    /// Copies whose config the plugin refused: at spawn (the copy does not
    /// run) or on update (the plugin keeps running its previous config).
    rejected: HashMap<Slot, LoaderError>,
    /// Instances of instanced groups, by number (see [`Slot::scope`]).
    instances: HashMap<u64, InstanceRecord>,
    /// The same instances by their group fiber's `ctx.instance()`, for
    /// [`State::instance_of_kernel`] and [`Loader::instance_rows`]. Written
    /// by [`State::insert_instance`], [`State::remove_instance`],
    /// [`State::rebuild_instance`] and [`State::clear_instances`] only; a
    /// stale entry is checked against the record's `kernel` before it is
    /// believed.
    instances_by_kernel: HashMap<InstanceId, u64>,
    /// Instance number → the instances created inside it (`None`: created at
    /// the root). A record's parent never changes, so this is written on
    /// insert and removal only; it is what makes one instance's subtree
    /// readable without scanning the instances beside it (#1076).
    instance_children: HashMap<Option<u64>, BTreeSet<u64>>,
    /// Copies inside instances that disposed themselves, with the fiber
    /// they had; not respawned until restarted or their row changes.
    stopped: HashMap<Slot, PluginId>,
    /// Source of spawn tokens; see [`Running::token`].
    next_token: u64,
    /// Last root context, kept to notice a host shutdown after unmount.
    last_root: Option<Ctx>,
}

/// The state a replayed queue was built on — the layers, the editable
/// layer, its version and the queue — put back if the operation's future is
/// dropped before the replay finished (#1229). The caller changes the layer
/// and takes the queue in one critical section, then holds this across the
/// awaits of the replay; a future dropped in between leaves the queue as it
/// was instead of losing the edits only it held.
struct Restore<'a> {
    inner: &'a Inner,
    layers: Vec<Layer>,
    editable: Option<usize>,
    version: Version,
    pending: Vec<Edit>,
    armed: bool,
}

impl<'a> Restore<'a> {
    /// Take the state to put back from `state`, before the caller replaces
    /// its layer and queue.
    fn new(inner: &'a Inner, state: &State) -> Self {
        Self {
            inner,
            layers: state.layers.clone(),
            editable: state.editable,
            version: state.version.clone(),
            pending: state.pending.clone(),
            armed: true,
        }
    }

    /// The operation finished: keep what it wrote.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = self.inner.state.lock().unwrap();
        state.layers = std::mem::take(&mut self.layers);
        state.editable = self.editable;
        state.version = std::mem::take(&mut self.version);
        state.pending = std::mem::take(&mut self.pending);
    }
}

/// Where a fiber runs: its row, and the instance it belongs to (`None`
/// outside instanced groups). An instanced group's own fiber has its own
/// instance as `scope`; the rows inside it share that scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Slot {
    row: String,
    scope: Option<u64>,
}

impl Slot {
    fn global(row: impl Into<String>) -> Self {
        Self {
            row: row.into(),
            scope: None,
        }
    }
}

/// One failing copy, as reconcile compares them before and after a change:
/// the failure, the copy (its instance number, `None` outside instances),
/// and the row as composed.
#[derive(Clone, PartialEq, Eq)]
struct Failing {
    failure: Failure,
    scope: Option<u64>,
    value: String,
    /// The copy is [`Unavailable`](crate::Unavailable) in its instance: it
    /// fails there alone, and never counts as a new failure of a change.
    unavailable: bool,
    /// A plugin of the copy is running, or ran and failed: its failure is
    /// the kernel's to report, and a change refused while it keeps its
    /// previous config is no failed start.
    running: bool,
}

impl Failing {
    /// The failures as reported: one per row and error.
    fn public<'a>(failing: impl Iterator<Item = &'a Failing>) -> Vec<Failure> {
        let mut out: Vec<Failure> = Vec::new();
        for f in failing {
            if !out.contains(&f.failure) {
                out.push(f.failure.clone());
            }
        }
        out
    }
}

/// One instance of an instanced group.
struct InstanceRecord {
    group: String,
    /// The group context it is created in.
    parent: Option<Slot>,
    /// The token that context was registered under: the instance goes
    /// with that context, and is not carried over to a rebuilt one.
    parent_token: u64,
    /// Values given with `with`, for factories.
    values: Values,
    /// The group fiber and its `ctx.instance()`; both change when the
    /// instance is rebuilt.
    plugin: PluginId,
    kernel: InstanceId,
}

/// A running group's context, tagged with the token of the spawn that owns
/// it so a late cleanup of an older instance cannot unregister a newer one.
struct Group {
    ctx: Ctx,
    token: u64,
}

struct Running {
    parent: Option<Slot>,
    /// Unique per spawn. A group registers its context under this token.
    token: u64,
    /// Token of the group context this row was spawned in.
    parent_token: u64,
    view: FiberView,
    group: bool,
    name: String,
    injects: Vec<TypeKey>,
    /// The module factory's name: its identity. A new one means respawning.
    factory_name: String,
    resolved: Option<Arc<Resolved>>,
    /// The evaluated config the kernel holds.
    config: Value,
    /// `isolate` names and labels and `inject` names it was spawned with.
    scope: (Vec<(String, String)>, Vec<String>),
    /// The row's context (parent with isolates), for re-evaluating config.
    ctx: Ctx,
}

impl State {
    fn layer_name(&self, index: usize) -> Option<&str> {
        self.layers.get(index).map(|l| l.name.as_str())
    }

    /// Record a running copy at `slot`, and its row under the slot's scope.
    fn insert_running(&mut self, slot: Slot, running: Running) {
        self.running_rows
            .entry(slot.scope)
            .or_default()
            .insert(slot.row.clone());
        self.running.insert(slot, running);
    }

    /// Drop the record at `slot`, and its row from the slot's scope.
    fn remove_running(&mut self, slot: &Slot) -> Option<Running> {
        let removed = self.running.remove(slot);
        if removed.is_some() {
            if let Some(rows) = self.running_rows.get_mut(&slot.scope) {
                rows.remove(&slot.row);
                if rows.is_empty() {
                    self.running_rows.remove(&slot.scope);
                }
            }
        }
        removed
    }

    /// Record an instance under `number`, indexed by its group fiber and
    /// under the instance it was created in.
    fn insert_instance(&mut self, number: u64, instance: InstanceRecord) {
        let parent = instance.parent.as_ref().and_then(|parent| parent.scope);
        self.instances_by_kernel.insert(instance.kernel, number);
        self.instance_children
            .entry(parent)
            .or_default()
            .insert(number);
        self.instances.insert(number, instance);
    }

    /// Drop the instance `number` and its entries in the indexes.
    fn remove_instance(&mut self, number: u64) -> Option<InstanceRecord> {
        let removed = self.instances.remove(&number)?;
        self.forget_instance_indexes(number, &removed);
        Some(removed)
    }

    /// The instance `number` was spawned again: its group fiber changed.
    fn rebuild_instance(&mut self, number: u64, plugin: PluginId, kernel: InstanceId) {
        let Some(instance) = self.instances.get_mut(&number) else {
            return;
        };
        if self.instances_by_kernel.get(&instance.kernel) == Some(&number) {
            self.instances_by_kernel.remove(&instance.kernel);
        }
        instance.plugin = plugin;
        instance.kernel = kernel;
        self.instances_by_kernel.insert(kernel, number);
    }

    /// Drop every instance record and its index entries.
    fn clear_instances(&mut self) {
        self.instances.clear();
        self.instances_by_kernel.clear();
        self.instance_children.clear();
    }

    /// The number of the instance whose group fiber is `kernel`.
    fn instance_of_kernel(&self, kernel: InstanceId) -> Option<u64> {
        let number = *self.instances_by_kernel.get(&kernel)?;
        let instance = self.instances.get(&number)?;
        (instance.kernel == kernel).then_some(number)
    }

    /// `removed` is out of `instances`: take its index entries too.
    fn forget_instance_indexes(&mut self, number: u64, removed: &InstanceRecord) {
        if self.instances_by_kernel.get(&removed.kernel) == Some(&number) {
            self.instances_by_kernel.remove(&removed.kernel);
        }
        let parent = removed.parent.as_ref().and_then(|parent| parent.scope);
        if let Some(children) = self.instance_children.get_mut(&parent) {
            children.remove(&number);
            if children.is_empty() {
                self.instance_children.remove(&parent);
            }
        }
    }

    /// The numbers of `number` and the instances created inside it, in
    /// creation order.
    fn instance_tree(&self, number: u64) -> Vec<u64> {
        let mut tree = Vec::new();
        let mut pending = vec![number];
        while let Some(number) = pending.pop() {
            tree.push(number);
            if let Some(children) = self.instance_children.get(&Some(number)) {
                pending.extend(children.iter().copied());
            }
        }
        tree.sort_unstable();
        tree
    }

    /// Drops the peak capacity of the tables rows and instances live in once
    /// they are mostly empty: a loader that created and closed thousands of
    /// instances would otherwise keep their peak tables for the life of the
    /// process (#979). The rule is the kernel's for its registry and the
    /// event table (capacity over 64 slots, under a quarter full), so the
    /// cost is spread over the removals: each shrink moves the entries that
    /// are left, and the table has to grow fourfold to be checked again.
    fn shrink_sparse(&mut self) {
        shrink_if_sparse(&mut self.groups);
        shrink_if_sparse(&mut self.running);
        shrink_if_sparse(&mut self.running_rows);
        shrink_if_sparse(&mut self.instances);
        shrink_if_sparse(&mut self.instances_by_kernel);
        shrink_if_sparse(&mut self.instance_children);
        shrink_if_sparse(&mut self.stopped);
        shrink_if_sparse(&mut self.rejected);
    }
}

/// The sparse rule of [`State::shrink_sparse`].
fn shrink_if_sparse<K: Eq + std::hash::Hash, V>(map: &mut HashMap<K, V>) {
    if map.capacity() > 64 && map.len() * 4 < map.capacity() {
        map.shrink_to_fit();
    }
}
