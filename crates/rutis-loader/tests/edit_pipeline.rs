//! The edit pipeline and the pending queue as state machines (#631, #629).
//!
//! Three groups of tests over one fixture:
//!
//! - random edit sequences against a model of the editable layer: an edit
//!   the model accepts either applies (layer, storage and running configs
//!   move together) or is refused with all three as they were; the composed
//!   tree is always the model's;
//! - the pending queue under several writers: saves in submission order,
//!   conflicts replayed and retried, the queue kept when retries run out;
//! - futures dropped at each await: an edit dropped half-way must leave the
//!   editable layer either stored or queued — never in the loader alone.
//!
//! The last group needs a way to suspend a future between two of its awaits:
//! [`drop_after`] polls it once per step and yields between steps, so the
//! tasks it spawned run as they would under the executor, and drops it where
//! the driver stops.

mod common;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use common::*;
use proptest::prelude::*;
use rutis::host::BoxFuture;
use rutis_loader::edit::{apply_edit, Edit, Editable, NewEntry, Persist, PersistError, Version};
use rutis_loader::patch::apply_patches;
use rutis_loader::report::Isolate;
use rutis_loader::{Layer, Loader, LoaderError, LoaderOptions, Patch};
use rutis_test_support::within;
use serde_json::{json, Value};

/// `a` (echo), and group `g` with `c` (echo) in it.
fn base() -> Vec<Patch> {
    patches(json!([{ "insert": [
        { "id": "a", "name": "echo", "config": { "label": "a1" } },
        { "id": "g", "name": "group", "group": true, "config": [
            { "id": "c", "name": "echo", "config": { "label": "c1" } }
        ] }
    ] }]))
}

async fn setup() -> (Harness, MemStore, rutis::Ctx, Loader) {
    let harness = Harness::new();
    let store = MemStore::default();
    let (root, loader) = mount(harness.resolver(), store.clone()).await;
    reconcile(&loader, base(), &store).await;
    (harness, store, root, loader)
}

fn new_entry(id: &str, name: &str, config: Value) -> NewEntry {
    NewEntry {
        id: Some(id.into()),
        name: name.into(),
        config,
        ..NewEntry::default()
    }
}

fn editable_index(loader: &Loader) -> usize {
    let editable = loader.editable().expect("an editable layer");
    loader
        .layers()
        .iter()
        .position(|layer| layer.name == editable.layer)
        .expect("the editable layer is one of the loader's layers")
}

/// The patches on the editable layer.
fn editable_patches(loader: &Loader) -> Vec<Patch> {
    let index = editable_index(loader);
    loader.layers()[index].patches.clone()
}

/// What the loader shows as its desired tree: every row in tree order, as
/// composed (children stay inside their group).
fn live_rows(loader: &Loader) -> Vec<(String, Value)> {
    loader
        .entries()
        .into_iter()
        .map(|entry| (entry.id, entry.options))
        .collect()
}

/// The tree `layers` compose to, in the shape of [`live_rows`].
fn model_rows(base: &[Patch], editable: &[Patch]) -> Vec<(String, Value)> {
    apply_patches(&[
        Layer::new("base", base.to_vec()),
        Layer::new("user", editable.to_vec()),
    ])
    .flat
    .into_iter()
    .map(|row| (row.id.unwrap_or_default(), row.value))
    .collect()
}

/// The tree `model` composes to, each row's evaluated config (a group's is
/// its child list; a row without one has `null`).
fn model_configs(base: &[Patch], editable: &[Patch]) -> BTreeMap<String, Value> {
    model_rows(base, editable)
        .into_iter()
        // A group's evaluated config is `null` (`Loader::evaluated`): its
        // children are the tree, not a config.
        .filter(|(_, value)| value.get("group").and_then(Value::as_bool) != Some(true))
        .map(|(id, value)| (id, value.get("config").cloned().unwrap_or(Value::Null)))
        .collect()
}

/// The config every row runs with (or would run with). Not the raw options:
/// expressions and overrides are evaluated.
fn running_configs(loader: &Loader) -> BTreeMap<String, Value> {
    loader
        .entries()
        .into_iter()
        .filter(|entry| entry.options.get("group").and_then(Value::as_bool) != Some(true))
        .filter_map(|entry| {
            let config = loader.evaluated(&entry.id)?.ok()?;
            Some((entry.id, config))
        })
        .collect()
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    layers: Vec<Layer>,
    stored: Vec<Patch>,
    pending: Vec<Edit>,
    configs: BTreeMap<String, Value>,
}

fn snapshot(loader: &Loader, store: &MemStore) -> Snapshot {
    Snapshot {
        layers: loader.layers(),
        stored: store.patches(),
        pending: loader.pending(),
        configs: running_configs(loader),
    }
}

/// The patches the loader must hold: the stored layer with every queued edit
/// applied in order — the contract [`Persist::save`] states. A loader whose
/// editable layer is neither this nor stored has an edit no restart can
/// restore and no flush will write.
fn assert_accounted(loader: &Loader, store: &MemStore, what: &str) {
    let index = editable_index(loader);
    let editable = loader.editable().unwrap();
    let pending = loader.pending();
    let mut layers = vec![
        Layer::new("base", base()),
        Layer::new(editable.layer.clone(), store.patches()),
    ];
    for edit in &pending {
        layers[1].patches = apply_edit(&layers, 1, edit).unwrap_or_else(|error| {
            panic!("the queued edit {edit:?} does not apply to the stored layer: {error:?}")
        });
    }
    assert_eq!(
        loader.layers()[index].patches,
        layers[1].patches,
        "{what}: the editable layer is neither stored nor queued (pending {pending:?}, stored {:?})",
        store.patches()
    );
    assert_eq!(
        editable.version,
        store.version(),
        "{what}: the loader holds a version the store does not have"
    );
}

// ------------------------------------------------------------ the model

/// The rows edits may name: the two base rows, their group, and one a
/// `create` may add.
const IDS: [&str; 4] = ["a", "c", "g", "n1"];

fn any_config() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({ "label": "ok" })),
        // Refused by `validate_config` in the dry run.
        Just(json!({ "invalid": true })),
        // Accepted by the dry run, fails in `apply`: rolls back.
        Just(json!({ "fail_apply": true })),
    ]
}

fn some_name() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&["echo", "echo2", "plain", "missing"][..])
}

fn some_parent() -> impl Strategy<Value = Option<&'static str>> {
    prop::option::of(prop::sample::select(&["g", "a"][..]))
}

/// One edit, spread over every kind: refuses, no-ops, in-place updates,
/// respawns, creates, removes and moves.
fn any_edit() -> impl Strategy<Value = Edit> {
    let ids = prop::sample::select(&IDS[..]);
    prop_oneof![
        4 => (ids.clone(), any_config())
            .prop_map(|(id, config)| Edit::Update { id: id.to_owned(), config }),
        2 => (ids.clone(), any::<bool>())
            .prop_map(|(id, disabled)| Edit::SetDisabled { id: id.to_owned(), disabled }),
        1 => (
            ids.clone(),
            prop::option::of(prop::sample::select(&["dep", "echo", "nope"][..]))
                .prop_map(|inject| inject.map(|name| vec![name.to_owned()])),
        )
            .prop_map(|(id, inject)| Edit::SetInject { id: id.to_owned(), inject }),
        1 => (ids.clone(), prop::option::of(Just(json!({ "dep": "s" }))))
            .prop_map(|(id, isolate)| Edit::SetIsolate {
                id: id.to_owned(),
                isolate: isolate.and_then(|value| value.as_object().cloned()),
            }),
        2 => (ids.clone(), some_name())
            .prop_map(|(id, name)| Edit::Rename { id: id.to_owned(), name: name.to_owned() }),
        1 => (prop::sample::select(&["n1", "n2"][..]), some_parent())
            .prop_map(|(id, parent)| Edit::Move {
                id: id.to_owned(),
                parent: parent.map(str::to_owned),
                position: None,
            }),
        2 => ids.clone().prop_map(|id| Edit::Remove { id: id.to_owned() }),
        2 => (
            prop::sample::select(&["n1", "n2"][..]),
            some_parent(),
            some_name(),
        )
            .prop_map(|(id, parent, name)| Edit::Create {
                entry: json!({ "id": id, "name": name, "config": { "label": id } }),
                parent: parent.map(str::to_owned),
                position: None,
            }),
    ]
}

/// What `apply_edit` would do to the model's editable layer; the rest of the
/// pipeline either accepts that or refuses, but never changes the layer to
/// something else.
fn model_apply(model: &[Patch], edit: &Edit) -> Result<Vec<Patch>, LoaderError> {
    let layers = vec![
        Layer::new("base", base()),
        Layer::new("user", model.to_vec()),
    ];
    apply_edit(&layers, 1, edit)
}

/// The same edit through the public methods.
async fn apply_public(loader: &Loader, edit: &Edit) -> Result<(), LoaderError> {
    match edit.clone() {
        Edit::Update { id, config } => loader.update(&id, config).await,
        Edit::SetDisabled { id, disabled } => loader.set_disabled(&id, disabled).await,
        Edit::SetInject { id, inject } => loader.set_inject(&id, inject.unwrap_or_default()).await,
        Edit::SetIsolate { id, isolate } => {
            let isolate: BTreeMap<String, Isolate> = isolate
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| {
                    let isolate = match value {
                        Value::Bool(true) => Isolate::Private,
                        Value::String(label) => Isolate::Shared(label),
                        other => panic!("the generator wrote an isolate of {other}"),
                    };
                    (name, isolate)
                })
                .collect();
            loader.set_isolate(&id, isolate).await
        }
        Edit::Rename { id, name } => loader.rename_module(&id, &name).await,
        Edit::Move {
            id,
            parent,
            position,
        } => loader.move_to(&id, parent.as_deref(), position).await,
        Edit::Remove { id } => loader.remove(&id).await,
        Edit::Create {
            entry,
            parent,
            position,
        } => {
            let id = entry["id"].as_str().unwrap().to_owned();
            let name = entry["name"].as_str().unwrap().to_owned();
            let config = entry.get("config").cloned().unwrap_or(Value::Null);
            loader
                .create(
                    NewEntry {
                        id: Some(id),
                        name,
                        config,
                        ..NewEntry::default()
                    },
                    parent.as_deref(),
                    position,
                )
                .await
                .map(|_| ())
        }
    }
}

async fn run_sequence(edits: &[Edit]) {
    let harness = Harness::new();
    let store = MemStore::default();
    let (_root, loader) = mount(harness.resolver(), store.clone()).await;
    reconcile(&loader, base(), &store).await;
    let mut model: Vec<Patch> = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        let what = format!("edit {index}: {edit:?}");
        let expected = model_apply(&model, edit);
        let before = snapshot(&loader, &store);
        let result = within(&what, apply_public(&loader, edit)).await;
        match expected {
            Err(_) => {
                // `apply_edit` refused it: the pipeline never gets further,
                // so nothing moved.
                assert!(
                    result.is_err(),
                    "{what}: the model refuses it but the loader accepted it"
                );
                assert_eq!(
                    editable_patches(&loader),
                    model,
                    "{what}: a refused edit changed the editable layer"
                );
                assert_eq!(
                    snapshot(&loader, &store),
                    before,
                    "{what}: a refused edit changed the layers, storage, queue or configs"
                );
            }
            Ok(next) => match result {
                Ok(()) => {
                    model = next;
                    assert_eq!(
                        editable_patches(&loader),
                        model,
                        "{what}: the editable layer is not the model's"
                    );
                    assert_eq!(
                        store.patches(),
                        model,
                        "{what}: the stored layer is not the model's"
                    );
                    assert!(
                        loader.pending().is_empty(),
                        "{what}: a saved edit is still queued"
                    );
                }
                Err(_) => {
                    // Dry run or rollback: the layer, storage and queue are
                    // as they were (the running configs are compared below
                    // through the model, which did not move either).
                    assert_eq!(
                        editable_patches(&loader),
                        model,
                        "{what}: a failed edit left its patches in the layer"
                    );
                    assert_eq!(store.patches(), before.stored, "{what}: storage changed");
                    assert_eq!(
                        loader.pending(),
                        before.pending,
                        "{what}: the queue changed"
                    );
                }
            },
        }
        // The desired tree is the model's, whatever the edit did, and so is
        // the config every row runs with.
        assert_eq!(
            live_rows(&loader),
            model_rows(&base(), &model),
            "{what}: the desired tree is not the model's"
        );
        assert_eq!(
            running_configs(&loader),
            model_configs(&base(), &model),
            "{what}: the running configs are not the model's"
        );
    }
    // A fresh loader on the same storage composes the same tree.
    let harness = Harness::new();
    let (_root2, fresh) = mount(harness.resolver(), store.clone()).await;
    reconcile(&fresh, base(), &store).await;
    assert_eq!(
        live_rows(&fresh),
        live_rows(&loader),
        "the stored layers do not restore the live tree"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// guarantee: loader/edit-dry-run-first, loader/apply-failure-rolls-back,
    /// loader/ownership-and-overrides, loader/saved-layers-restore
    ///
    /// Eight random edits against a fresh loader, compared with the editable
    /// layer as a model: accepted edits move the layer to the model's next
    /// state (and storage with it), refused ones move nothing, and the
    /// composed tree is always the model's. The edits include refuses, apply
    /// failures that roll back, renames, moves and creates.
    #[test]
    fn random_edits_match_the_model(edits in prop::collection::vec(any_edit(), 8..=8)) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(run_sequence(&edits));
    }
}

// ---------------------------------------------- the pending queue, writers

/// Storage that checks saves never overlap and records their batches, with
/// CAS; a save yields in the middle so an overlap would be seen.
#[derive(Clone, Default)]
struct SerialStore {
    inner: Arc<Mutex<Serial>>,
}

#[derive(Default)]
struct Serial {
    patches: Vec<Patch>,
    version: u64,
    active: usize,
    most_active: usize,
    saves: Vec<Vec<Edit>>,
}

impl SerialStore {
    fn patches(&self) -> Vec<Patch> {
        self.inner.lock().unwrap().patches.clone()
    }

    fn version(&self) -> Version {
        Version(self.inner.lock().unwrap().version.to_string())
    }

    fn most_active(&self) -> usize {
        self.inner.lock().unwrap().most_active
    }

    fn saves(&self) -> Vec<Vec<Edit>> {
        self.inner.lock().unwrap().saves.clone()
    }
}

impl Persist for SerialStore {
    fn load<'a>(
        &'a self,
        _layer: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let state = self.inner.lock().unwrap();
            Ok((state.patches.clone(), Version(state.version.to_string())))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        expected: &'a Version,
        edits: &'a [Edit],
        patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move {
            {
                let mut state = self.inner.lock().unwrap();
                state.active += 1;
                state.most_active = state.most_active.max(state.active);
            }
            // A save takes a while; the loader must not start another.
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            let mut state = self.inner.lock().unwrap();
            state.active -= 1;
            if expected.0 != state.version.to_string() {
                return Err(PersistError::Conflict);
            }
            state.patches = patches.to_vec();
            state.version += 1;
            state.saves.push(edits.to_vec());
            Ok(Version(state.version.to_string()))
        })
    }
}

async fn mount_serial(harness: &Harness, store: &SerialStore) -> (rutis::Ctx, Loader) {
    let (root, loader) = mount_with(
        harness.resolver(),
        LoaderOptions {
            persist: Arc::new(store.clone()),
            ..LoaderOptions::default()
        },
    )
    .await;
    loader
        .reconcile(
            vec![Layer::new("base", base()), Layer::new("user", Vec::new())],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    (root, loader)
}

/// guarantee: loader/persist-queue, loader/edits-serialized
///
/// Two edits started together are serialized by the loader: no save is in
/// flight while another starts, and the saves carry the edits in submission
/// order — the update, then the disable.
#[tokio::test]
async fn saves_are_serial_and_carry_the_queue_in_order() {
    let harness = Harness::new();
    let store = SerialStore::default();
    let (_root, loader) = mount_serial(&harness, &store).await;

    let update = Edit::Update {
        id: "a".into(),
        config: json!({ "label": "a2" }),
    };
    let disable = Edit::SetDisabled {
        id: "c".into(),
        disabled: true,
    };
    let (first, second) = tokio::join!(
        within("the update", loader.update("a", json!({ "label": "a2" }))),
        within("the disable", loader.set_disabled("c", true)),
    );
    first.unwrap();
    second.unwrap();

    assert_eq!(store.most_active(), 1, "two saves overlapped");
    assert_eq!(
        store.saves().concat(),
        vec![update, disable],
        "the saves do not carry the edits in submission order"
    );
    assert!(loader.pending().is_empty());
    assert_eq!(store.patches(), editable_patches(&loader));
}

/// Storage that always refuses with a conflict, and accepts once `accept` is
/// set — the version never moves until then.
#[derive(Clone, Default)]
struct ConflictStore {
    inner: Arc<Mutex<Conflict>>,
}

#[derive(Default)]
struct Conflict {
    accept: bool,
    patches: Vec<Patch>,
    version: u64,
    attempts: usize,
    saves: Vec<Vec<Edit>>,
}

impl ConflictStore {
    fn accept(&self) {
        self.inner.lock().unwrap().accept = true;
    }

    fn saves(&self) -> Vec<Vec<Edit>> {
        self.inner.lock().unwrap().saves.clone()
    }

    fn patches(&self) -> Vec<Patch> {
        self.inner.lock().unwrap().patches.clone()
    }

    fn version(&self) -> Version {
        Version(self.inner.lock().unwrap().version.to_string())
    }

    fn attempts(&self) -> usize {
        self.inner.lock().unwrap().attempts
    }
}

impl Persist for ConflictStore {
    fn load<'a>(
        &'a self,
        _layer: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let state = self.inner.lock().unwrap();
            Ok((state.patches.clone(), Version(state.version.to_string())))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        _expected: &'a Version,
        edits: &'a [Edit],
        patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move {
            let mut state = self.inner.lock().unwrap();
            state.attempts += 1;
            if !state.accept {
                return Err(PersistError::Conflict);
            }
            state.patches = patches.to_vec();
            state.version += 1;
            state.saves.push(edits.to_vec());
            Ok(Version(state.version.to_string()))
        })
    }
}

/// guarantee: loader/persist-queue
///
/// A store that keeps changing under the loader (every save conflicts, every
/// load returns the same content) ends the edit with `Conflict` after the
/// retries; the edit stays queued, and a later `flush` writes it once the
/// version settles.
#[tokio::test]
async fn a_store_that_keeps_conflicting_returns_conflict_and_keeps_the_queue() {
    let harness = Harness::new();
    let store = ConflictStore::default();
    let (_root, loader) = mount_with(
        harness.resolver(),
        LoaderOptions {
            persist: Arc::new(store.clone()),
            ..LoaderOptions::default()
        },
    )
    .await;
    loader
        .reconcile(
            vec![Layer::new("base", base()), Layer::new("user", Vec::new())],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();

    let err = within(
        "the edit against a store that always conflicts",
        loader.update("a", json!({ "label": "a2" })),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, LoaderError::Conflict), "{err:?}");
    let queued = loader.pending();
    assert_eq!(
        queued,
        vec![Edit::Update {
            id: "a".into(),
            config: json!({ "label": "a2" })
        }],
        "the queue was not kept"
    );
    assert_eq!(
        store.saves(),
        Vec::<Vec<Edit>>::new(),
        "a conflicting store wrote something"
    );

    // Every attempt is a save: the first and the three retries.
    assert_eq!(store.attempts(), 1 + 3, "the retries did not run");
    assert_eq!(store.patches(), Vec::<Patch>::new());
    store.accept();
    within("the flush", loader.flush()).await.unwrap();
    assert!(loader.pending().is_empty());
    assert_eq!(
        store.saves(),
        vec![queued],
        "the queue was not written in order after the retries"
    );
    assert_eq!(store.patches(), editable_patches(&loader));
}

/// guarantee: loader/persist-queue
///
/// Two writers on one store: the earlier edit failed to save and stayed
/// queued; the later writer saved; the first writer's next edit conflicts,
/// replays its queue on the latest content and saves once. Every edit is in
/// storage once, in submission order.
#[tokio::test]
async fn two_writers_store_each_edit_once_in_order() {
    let store = MemStore::default();
    let (h1, h2) = (Harness::new(), Harness::new());
    let (_r1, one) = mount(h1.resolver(), store.clone()).await;
    let (_r2, two) = mount(h2.resolver(), store.clone()).await;
    reconcile(&one, base(), &store).await;
    reconcile(&two, base(), &store).await;

    store.set_fail(true);
    let failed = Edit::Update {
        id: "a".into(),
        config: json!({ "label": "A" }),
    };
    let err = one.update("a", json!({ "label": "A" })).await.unwrap_err();
    assert!(matches!(err, LoaderError::PersistFailed(_)), "{err:?}");
    assert_eq!(one.pending(), vec![failed.clone()]);
    store.set_fail(false);

    let other = Edit::SetDisabled {
        id: "c".into(),
        disabled: true,
    };
    two.set_disabled("c", true).await.unwrap();
    one.create(new_entry("b", "echo", json!({ "label": "B" })), None, None)
        .await
        .unwrap();

    let created = Edit::Create {
        entry: json!({ "id": "b", "name": "echo", "config": { "label": "B" } }),
        parent: None,
        position: None,
    };
    assert_eq!(
        store.inner.lock().unwrap().saves,
        vec![vec![other], vec![failed, created]],
        "saves are not in submission order, or an edit was saved twice"
    );
    assert!(one.pending().is_empty());
    assert_eq!(
        store.patches(),
        editable_patches(&one),
        "the loader and the store disagree after the replay"
    );
}

// --------------------------------------------------- futures dropped

/// Polls `future` once with a waker that does nothing when woken.
fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    future.poll(&mut context)
}

/// Drives `future` one suspension point per step, yielding in between so the
/// tasks it spawned run, and drops it at step `steps` — or returns its output
/// if it finished first. A fresh future per step because a dropped one cannot
/// be resumed.
async fn drop_after<F: Future>(future: F, steps: usize) -> Option<F::Output> {
    // Boxed, so dropping the handle drops the future (and the locks and
    // saves it holds) at the end of this function.
    let mut future = Box::pin(future);
    for _ in 0..steps {
        if let Poll::Ready(output) = poll_once(future.as_mut()) {
            return Some(output);
        }
        tokio::task::yield_now().await;
    }
    None
}

/// Storage whose saves wait for [`GatedStore::release`]: a dropped future can
/// be caught with a save in flight, after its edit reached the queue.
#[derive(Clone)]
struct GatedStore {
    inner: Arc<Mutex<Gated>>,
    gate: Arc<tokio::sync::Semaphore>,
}

impl Default for GatedStore {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }
}

#[derive(Default)]
struct Gated {
    patches: Vec<Patch>,
    version: u64,
    writes: usize,
    saves: Vec<Vec<Edit>>,
}

impl GatedStore {
    fn patches(&self) -> Vec<Patch> {
        self.inner.lock().unwrap().patches.clone()
    }

    fn version(&self) -> Version {
        Version(self.inner.lock().unwrap().version.to_string())
    }

    fn writes(&self) -> usize {
        self.inner.lock().unwrap().writes
    }

    fn release(&self, count: usize) {
        self.gate.add_permits(count);
    }
}

impl Persist for GatedStore {
    fn load<'a>(
        &'a self,
        _layer: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let state = self.inner.lock().unwrap();
            Ok((state.patches.clone(), Version(state.version.to_string())))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        expected: &'a Version,
        edits: &'a [Edit],
        patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move {
            let permit = self.gate.clone().acquire_owned().await.unwrap();
            drop(permit);
            let mut state = self.inner.lock().unwrap();
            if expected.0 != state.version.to_string() {
                return Err(PersistError::Conflict);
            }
            state.patches = patches.to_vec();
            state.version += 1;
            state.writes += 1;
            state.saves.push(edits.to_vec());
            Ok(Version(state.version.to_string()))
        })
    }
}

async fn setup_gated() -> (Harness, GatedStore, rutis::Ctx, Loader) {
    let harness = Harness::new();
    let store = GatedStore::default();
    let (root, loader) = mount_with(
        harness.resolver(),
        LoaderOptions {
            persist: Arc::new(store.clone()),
            ..LoaderOptions::default()
        },
    )
    .await;
    loader
        .reconcile(
            vec![Layer::new("base", base()), Layer::new("user", Vec::new())],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    (harness, store, root, loader)
}

fn gated_accounted(loader: &Loader, store: &GatedStore) {
    let index = editable_index(loader);
    let pending = loader.pending();
    let mut layers = vec![
        Layer::new("base", base()),
        Layer::new("user", store.patches()),
    ];
    for edit in &pending {
        layers[1].patches = apply_edit(&layers, 1, edit).expect("the queue applies");
    }
    assert_eq!(
        loader.layers()[index].patches,
        layers[1].patches,
        "the editable layer is neither stored nor queued: pending {pending:?}"
    );
}

/// guarantee: loader/persist-queue
///
/// Dropping an edit while its save is in flight (the store holds it before
/// writing) must leave it queued; a later `flush` writes it once, not twice.
#[tokio::test]
async fn an_edit_dropped_with_its_save_in_flight_stays_queued() {
    let (_harness, store, _root, loader) = setup_gated().await;
    let queued = Edit::Update {
        id: "a".into(),
        config: json!({ "label": "a2" }),
    };
    // Poll until the edit is in the queue, then drop at the next suspension
    // point: the save is waiting for the gate.
    let mut future = Box::pin(loader.update("a", json!({ "label": "a2" })));
    let mut steps = 0;
    loop {
        match poll_once(future.as_mut()) {
            Poll::Ready(result) => panic!("the edit completed before its save waited: {result:?}"),
            Poll::Pending if !loader.pending().is_empty() => break,
            Poll::Pending => {}
        }
        tokio::task::yield_now().await;
        steps += 1;
        assert!(steps < 100, "the edit never reached its save");
    }
    drop(future);

    assert_eq!(
        loader.pending(),
        vec![queued.clone()],
        "the queued edit was lost"
    );
    assert_eq!(
        store.writes(),
        0,
        "the store wrote while its save was in flight"
    );
    gated_accounted(&loader, &store);

    store.release(1);
    within("the flush after the dropped save", loader.flush())
        .await
        .unwrap();
    assert!(loader.pending().is_empty());
    assert_eq!(store.writes(), 1, "the edit was written more than once");
    assert_eq!(store.patches(), editable_patches(&loader));
    assert_eq!(store.inner.lock().unwrap().saves, vec![vec![queued]]);
}

/// guarantee: loader/persist-queue
///
/// An edit's future dropped at any suspension point: the editable layer must
/// be the stored layer with the queue applied — never changed in the loader
/// alone. The `commit` window: the edit joins the queue in the same critical
/// section as the layer change, before the reconcile is awaited (#1229).
#[tokio::test]
async fn an_edit_dropped_at_any_await_is_never_lost() {
    let mut completed = false;
    for steps in 1..=8 {
        let (_harness, store, _root, loader) = setup().await;
        let outcome = drop_after(loader.update("a", json!({ "label": "dropped" })), steps).await;
        if let Some(result) = outcome {
            result.unwrap();
            completed = true;
        }
        assert_accounted(&loader, &store, &format!("step {steps}"));
        if completed {
            break;
        }
        loader.settled().await;
        within("the flush after the dropped edit", loader.flush())
            .await
            .unwrap();
        assert!(loader.pending().is_empty());
        assert_accounted(&loader, &store, &format!("step {steps}, after a flush"));
    }
    assert!(completed, "the edit never completed within the sweep");
}

fn row_config(patches: &[Patch], id: &str) -> Option<Value> {
    apply_patches(&[
        Layer::new("base", base()),
        Layer::new("user", patches.to_vec()),
    ])
    .flat
    .into_iter()
    .find(|row| row.id.as_deref() == Some(id))
    .map(|row| row.value["config"].clone())
}

/// guarantee: loader/persist-queue
///
/// A queued edit from an earlier operation is saved only when a flush really
/// finishes. Dropping the flush at any suspension point must leave the edit
/// queued or stored, never neither. The `persist_queue` window: the replay
/// works on a copy of the queue, and a dropped future puts the old layer and
/// queue back (#1229).
#[tokio::test]
async fn a_flush_dropped_at_any_await_keeps_the_queue() {
    let mut completed = false;
    for steps in 1..=8 {
        let store = MemStore::default();
        let (h1, h2) = (Harness::new(), Harness::new());
        let (_r1, one) = mount(h1.resolver(), store.clone()).await;
        let (_r2, two) = mount(h2.resolver(), store.clone()).await;
        reconcile(&one, base(), &store).await;
        reconcile(&two, base(), &store).await;

        store.set_fail(true);
        let queued = Edit::Update {
            id: "a".into(),
            config: json!({ "label": "A" }),
        };
        assert!(matches!(
            one.update("a", json!({ "label": "A" })).await,
            Err(LoaderError::PersistFailed(_))
        ));
        store.set_fail(false);
        two.set_disabled("c", true).await.unwrap();
        assert_eq!(one.pending(), vec![queued.clone()]);

        let outcome = drop_after(one.flush(), steps).await;
        if let Some(result) = outcome {
            result.unwrap();
            completed = true;
        } else {
            let kept = one.pending() == vec![queued.clone()]
                || row_config(&store.patches(), "a") == Some(json!({ "label": "A" }));
            assert!(kept, "step {steps}: a dropped flush lost the queued edit");
        }
        // A later flush writes the queue, whatever the dropped one did.
        within("the flush after the dropped one", one.flush())
            .await
            .unwrap();
        assert!(one.pending().is_empty());
        assert_eq!(
            row_config(&store.patches(), "a"),
            Some(json!({ "label": "A" })),
            "the edit is not in the store after a later flush"
        );
        assert_eq!(
            apply_patches(&[
                Layer::new("base", base()),
                Layer::new("user", store.patches())
            ])
            .flat
            .iter()
            .find(|row| row.id.as_deref() == Some("c"))
            .map(|row| row.value["disabled"].clone()),
            Some(json!(true)),
            "the other writer's edit was lost"
        );
        if completed {
            break;
        }
    }
    assert!(completed, "the flush never completed within the sweep");
}

/// guarantee: loader/persist-queue
///
/// A conflict replays the whole queue on the latest content. Dropping the
/// flush at any suspension point of that replay must keep every queued edit
/// (or have stored all of them), and a later flush writes them all, in order,
/// once (#1229).
#[tokio::test]
async fn a_replay_dropped_at_any_await_keeps_every_queued_edit() {
    let mut completed = false;
    for steps in 1..=32 {
        let store = MemStore::default();
        let (h1, h2) = (Harness::new(), Harness::new());
        let (_r1, one) = mount(h1.resolver(), store.clone()).await;
        let (_r2, two) = mount(h2.resolver(), store.clone()).await;
        reconcile(&one, base(), &store).await;
        reconcile(&two, base(), &store).await;

        store.set_fail(true);
        let first = Edit::Update {
            id: "a".into(),
            config: json!({ "label": "A" }),
        };
        let second = Edit::Update {
            id: "c".into(),
            config: json!({ "label": "C" }),
        };
        for (id, label) in [("a", "A"), ("c", "C")] {
            assert!(matches!(
                one.update(id, json!({ "label": label })).await,
                Err(LoaderError::PersistFailed(_))
            ));
        }
        store.set_fail(false);
        assert_eq!(one.pending(), vec![first.clone(), second.clone()]);
        two.set_disabled("c", true).await.unwrap();

        let outcome = drop_after(one.flush(), steps).await;
        if let Some(result) = outcome {
            result.unwrap();
            completed = true;
        } else {
            let queued = one.pending() == vec![first.clone(), second.clone()];
            let stored = [("a", "A"), ("c", "C")].iter().all(|(id, label)| {
                row_config(&store.patches(), id) == Some(json!({ "label": label }))
            });
            assert!(
                queued || stored,
                "step {steps}: a dropped replay lost a queued edit: pending {:?}",
                one.pending()
            );
        }
        within("the flush after the dropped one", one.flush())
            .await
            .unwrap();
        assert!(one.pending().is_empty());
        assert_eq!(
            row_config(&store.patches(), "a"),
            Some(json!({ "label": "A" }))
        );
        assert_eq!(
            row_config(&store.patches(), "c"),
            Some(json!({ "label": "C" })),
            "the second queued edit is not in the store"
        );
        assert_eq!(
            editable_patches(&one),
            store.patches(),
            "the loader and the store disagree after the replay"
        );
        let saves = store.inner.lock().unwrap().saves.clone();
        assert_eq!(
            saves.last(),
            Some(&vec![first.clone(), second.clone()]),
            "the queue was not written in order, or an edit was dropped"
        );
        assert_eq!(
            saves.iter().filter(|batch| batch.len() == 2).count(),
            1,
            "the replay wrote the queue more than once"
        );
        if completed {
            break;
        }
    }
    assert!(completed, "the flush never completed within the sweep");
}

/// guarantee: loader/persist-queue
///
/// [`Loader::reconcile`] replaces the layers and replays the queue on them.
/// Dropping it while that is awaited must not lose edits an earlier operation
/// left queued (#1229).
#[tokio::test]
async fn a_reconcile_dropped_at_any_await_keeps_the_queue() {
    let mut completed = false;
    // The replay after the replacement layers adds suspension points, so this
    // window needs a wider sweep than the two above.
    for steps in 1..=16 {
        let (_harness, store, _root, loader) = setup().await;
        store.set_fail(true);
        let queued = Edit::Update {
            id: "a".into(),
            config: json!({ "label": "A" }),
        };
        assert!(matches!(
            loader.update("a", json!({ "label": "A" })).await,
            Err(LoaderError::PersistFailed(_))
        ));
        store.set_fail(false);
        assert_eq!(loader.pending(), vec![queued.clone()]);

        let layers = vec![
            Layer::new("base", base()),
            Layer::new("user", store.patches()),
        ];
        let editable = Editable::new("user", store.version());
        let outcome = drop_after(loader.reconcile(layers, Some(editable)), steps).await;
        if let Some(result) = outcome {
            result.unwrap();
            completed = true;
        } else {
            let kept = loader.pending() == vec![queued.clone()]
                || row_config(&store.patches(), "a") == Some(json!({ "label": "A" }));
            assert!(
                kept,
                "step {steps}: a dropped reconcile lost the queued edit"
            );
        }
        within("the flush after the dropped reconcile", loader.flush())
            .await
            .unwrap();
        assert!(loader.pending().is_empty());
        assert_eq!(
            row_config(&store.patches(), "a"),
            Some(json!({ "label": "A" })),
            "the edit is not in the store after a later flush"
        );
        if completed {
            break;
        }
    }
    assert!(completed, "the reconcile never completed within the sweep");
}
