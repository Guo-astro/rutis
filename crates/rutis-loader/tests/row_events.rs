//! Rows on the logical bus (events protocol §3, §8): each row's state
//! changes are `rutis/row` events at its scope, and an instance of an
//! `instanced` group is a scope.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::Event;
use rutis_loader::report::RowEventState;
use rutis_loader::RowEvent;
use rutis_test_support::eventually;
use serde_json::json;

fn states(events: &Arc<Mutex<Vec<RowEvent>>>, row: &str) -> Vec<RowEventState> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.row == row)
        .map(|event| event.state)
        .collect()
}

/// guarantee: events/framework (rutis/row)
///
/// A row reports `pending` then `running`, and `stopped` when it is
/// disabled; a row inside an instance reports at the instance's scope (its
/// group named), a root row at the root.
#[tokio::test(flavor = "multi_thread")]
async fn rows_announce_their_states_at_their_scopes() {
    let harness = Harness::new();
    let store = MemStore::default();
    let (root, loader) = mount(harness.resolver(), store.clone()).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on::<RowEvent>(move |event, _| {
        seen.lock().unwrap().push((*event).clone());
        std::future::ready(Ok(None))
    })
    .unwrap();
    reconcile(
        &loader,
        patches(json!([{ "insert": [
            { "id": "a", "name": "echo", "config": { "label": "a" } },
            { "id": "doc", "group": true, "instanced": true, "config": [
                { "id": "inner", "name": "echo", "config": { "label": "i" } }
            ] }
        ] }])),
        &store,
    )
    .await;
    eventually(
        || {
            states(&events, "a")
                .contains(&RowEventState::Running)
                .then_some(())
        },
        "a running",
    )
    .await;
    assert_eq!(states(&events, "a")[0], RowEventState::Pending);
    let a = events
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.row == "a")
        .cloned()
        .unwrap();
    assert_eq!(a.plugin, "echo");
    assert!(a.scope.is_empty(), "{a:?}");

    let _instance = loader.create_instance(&root, "doc").await.unwrap();
    eventually(
        || {
            states(&events, "inner")
                .contains(&RowEventState::Running)
                .then_some(())
        },
        "inner running",
    )
    .await;
    let inner = events
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.row == "inner")
        .cloned()
        .unwrap();
    assert_eq!(inner.scope.len(), 1, "{inner:?}");
    assert_eq!(inner.scope[0].group, "doc");

    loader.set_disabled("a", true).await.unwrap();
    eventually(
        || {
            states(&events, "a")
                .contains(&RowEventState::Stopped)
                .then_some(())
        },
        "a stopped",
    )
    .await;
}

fn status_listeners(root: &rutis::Ctx) -> usize {
    root.events()
        .subscriptions()
        .into_iter()
        .filter(|entry| entry.name == rutis::host::FiberStatusChanged::NAME)
        .count()
}

/// guarantee: events/framework (rutis/row)
///
/// The loader follows every row with one `FiberStatusChanged` listener:
/// loading one row or sixty-five leaves the same listener count, and a row
/// stopping leaves it unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn rows_share_one_status_listener() {
    let harness = Harness::new();
    let store = MemStore::default();
    let (root, loader) = mount(harness.resolver(), store.clone()).await;
    let before = status_listeners(&root);
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on::<RowEvent>(move |event, _| {
        seen.lock().unwrap().push((event.row.clone(), event.state));
        std::future::ready(Ok(None))
    })
    .unwrap();
    let count = |state: RowEventState| {
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, seen)| *seen == state)
            .count()
    };
    let rows = |count: usize| {
        let rows: Vec<_> = (0..count)
            .map(|index| {
                json!({
                    "id": format!("r{index}"),
                    "name": "echo",
                    "config": { "label": format!("r{index}") }
                })
            })
            .collect();
        patches(json!([{ "insert": rows }]))
    };
    reconcile(&loader, rows(1), &store).await;
    eventually(
        || (count(RowEventState::Running) == 1).then_some(()),
        "one row running",
    )
    .await;
    assert_eq!(status_listeners(&root), before);
    reconcile(&loader, rows(65), &store).await;
    eventually(
        || (count(RowEventState::Running) == 65).then_some(()),
        "sixty-five rows running",
    )
    .await;
    assert_eq!(status_listeners(&root), before);
    loader.set_disabled("r10", true).await.unwrap();
    eventually(
        || {
            (events
                .lock()
                .unwrap()
                .iter()
                .any(|(row, state)| row == "r10" && *state == RowEventState::Stopped))
            .then_some(())
        },
        "r10 stopped",
    )
    .await;
    assert_eq!(status_listeners(&root), before);
    // Every row gone: nothing is followed — and the registration is still
    // there, because it is the loader's, not the rows' (#806).
    reconcile(&loader, patches(json!([{ "insert": [] }])), &store).await;
    eventually(
        || (rutis_loader::internal::followed_rows(&loader) == 0).then_some(()),
        "no row followed",
    )
    .await;
    assert_eq!(status_listeners(&root), before);
}

/// guarantee: events/framework (rutis/row)
///
/// Rows stopping with the loader still say `stopped`: a row of the loader's
/// tree and a row of an instance that the loader's disposer clears. The
/// listener is not owned by the loader's fiber, which is closing while its
/// rows go; it belongs to the loader and stays until the loader's `Inner` is
/// dropped (see `the_listener_leaves_when_the_loader_is_dropped`).
#[tokio::test(flavor = "multi_thread")]
async fn rows_stopped_with_the_loader_say_stopped() {
    let root = rutis::Ctx::root().unwrap();
    let harness = Harness::new();
    let store = MemStore::default();
    let before = status_listeners(&root);
    let plugin = rutis_loader::LoaderPlugin::new(
        harness.resolver(),
        rutis_loader::LoaderOptions {
            persist: Arc::new(store.clone()),
            ..Default::default()
        },
    );
    let loader = plugin.handle();
    let view = root.plugin(plugin);
    (&view).await.unwrap();
    let events: Arc<Mutex<Vec<(String, RowEventState)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on::<RowEvent>(move |event, _| {
        seen.lock().unwrap().push((event.row.clone(), event.state));
        std::future::ready(Ok(None))
    })
    .unwrap();
    let reached = |row: &str, state: RowEventState| {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|(seen, said)| seen == row && *said == state)
    };
    reconcile(
        &loader,
        patches(json!([{ "insert": [
            { "id": "tree", "name": "echo", "config": { "label": "tree" } },
            { "id": "doc", "group": true, "instanced": true, "config": [
                { "id": "inner", "name": "echo", "config": { "label": "inner" } }
            ] }
        ] }])),
        &store,
    )
    .await;
    loader.create_instance(&root, "doc").await.unwrap();
    eventually(
        || {
            (reached("tree", RowEventState::Running) && reached("inner", RowEventState::Running))
                .then_some(())
        },
        "both rows running",
    )
    .await;
    view.dispose().await.unwrap();
    eventually(
        || {
            (reached("tree", RowEventState::Stopped) && reached("inner", RowEventState::Stopped))
                .then_some(())
        },
        "both rows stopped",
    )
    .await;
    // The rows leaving does not withdraw it: it is the loader's, and leaves
    // with the loader's `Inner`.
    assert_eq!(
        status_listeners(&root),
        before + 1,
        "the registration stays with the loader"
    );
}

/// guarantee: events/framework (rutis/row)
///
/// The listener belongs to the loader: rows stopping with it still say
/// `stopped` through it, and once every `Loader` handle and the plugin are
/// gone the root's listener count returns to what it was before (#806).
#[tokio::test(flavor = "multi_thread")]
async fn the_listener_leaves_when_the_loader_is_dropped() {
    let root = rutis::Ctx::root().unwrap();
    let harness = Harness::new();
    let store = MemStore::default();
    let before = status_listeners(&root);
    let plugin = rutis_loader::LoaderPlugin::new(
        harness.resolver(),
        rutis_loader::LoaderOptions {
            persist: Arc::new(store.clone()),
            ..Default::default()
        },
    );
    let loader = plugin.handle();
    let view = root.plugin(plugin);
    (&view).await.unwrap();
    assert_eq!(
        status_listeners(&root),
        before + 1,
        "the loader listens once"
    );
    let events: Arc<Mutex<Vec<(String, RowEventState)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on::<RowEvent>(move |event, _| {
        seen.lock().unwrap().push((event.row.clone(), event.state));
        std::future::ready(Ok(None))
    })
    .unwrap();
    let reached = |row: &str, state: RowEventState| {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|(seen, said)| seen == row && *said == state)
    };
    reconcile(
        &loader,
        patches(json!([{ "insert": [
            { "id": "tree", "name": "echo", "config": { "label": "tree" } }
        ] }])),
        &store,
    )
    .await;
    eventually(
        || reached("tree", RowEventState::Running).then_some(()),
        "the row runs",
    )
    .await;
    view.dispose().await.unwrap();
    eventually(
        || reached("tree", RowEventState::Stopped).then_some(()),
        "the row stopped with the loader",
    )
    .await;
    // Still there: the loader has not been dropped.
    assert_eq!(status_listeners(&root), before + 1);
    drop(loader);
    drop(view);
    eventually(
        || (status_listeners(&root) == before).then_some(()),
        "the registration left with the loader",
    )
    .await;
}

/// guarantee: events/framework (rutis/row)
///
/// A restarted loader holds one listener: the registration is made once and
/// a later `apply` finds it, and the new run's rows say their changes
/// through it.
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_loader_still_listens_once() {
    let root = rutis::Ctx::root().unwrap();
    let harness = Harness::new();
    let store = MemStore::default();
    let before = status_listeners(&root);
    let plugin = rutis_loader::LoaderPlugin::new(
        harness.resolver(),
        rutis_loader::LoaderOptions {
            persist: Arc::new(store.clone()),
            ..Default::default()
        },
    );
    let loader = plugin.handle();
    let view = root.plugin(plugin);
    (&view).await.unwrap();
    let listening = status_listeners(&root);
    assert_eq!(listening, before + 1, "the loader listens once");
    let events: Arc<Mutex<Vec<(String, RowEventState)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on::<RowEvent>(move |event, _| {
        seen.lock().unwrap().push((event.row.clone(), event.state));
        std::future::ready(Ok(None))
    })
    .unwrap();
    let count = |row: &str, state: RowEventState| {
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, said)| seen == row && *said == state)
            .count()
    };
    reconcile(
        &loader,
        patches(json!([{ "insert": [
            { "id": "a", "name": "echo", "config": { "label": "a" } },
            { "id": "b", "name": "echo", "config": { "label": "b" } }
        ] }])),
        &store,
    )
    .await;
    eventually(
        || {
            (count("a", RowEventState::Running) == 1 && count("b", RowEventState::Running) == 1)
                .then_some(())
        },
        "both rows running",
    )
    .await;
    assert_eq!(status_listeners(&root), listening);
    let ran_a = count("a", RowEventState::Running);
    let ran_b = count("b", RowEventState::Running);
    view.restart().await.unwrap();
    // The withdrawal of a registration is spawned, so the count settles
    // rather than changing on the spot; a second registration would stay 2.
    eventually(
        || (status_listeners(&root) == listening).then_some(()),
        "one listener after the restart",
    )
    .await;
    // A restart disposes the rows; the loader disables a row disposed on its
    // own, and a row it respawned may already run again. Enable whichever is
    // off, so both run through the new registration.
    for id in ["a", "b"] {
        let off = loader.status().rows.iter().any(|row| {
            row.id == id && matches!(row.state, rutis_loader::report::EntryStatus::Disabled)
        });
        if off {
            loader.set_disabled(id, false).await.unwrap();
        }
    }
    eventually(
        || {
            (count("a", RowEventState::Running) > ran_a
                && count("b", RowEventState::Running) > ran_b)
                .then_some(())
        },
        "both rows say running again",
    )
    .await;
    assert_eq!(
        status_listeners(&root),
        listening,
        "one listener for the rows of the new run"
    );
    let stopped = count("b", RowEventState::Stopped);
    loader.set_disabled("b", true).await.unwrap();
    eventually(
        || (count("b", RowEventState::Stopped) == stopped + 1).then_some(()),
        "b stopped after the restart",
    )
    .await;
    assert_eq!(status_listeners(&root), listening);
    view.dispose().await.unwrap();
    // Still there after the rows go: it belongs to the loader.
    eventually(
        || (rutis_loader::internal::followed_rows(&loader) == 0).then_some(()),
        "no row followed",
    )
    .await;
    assert_eq!(status_listeners(&root), listening);
    drop(view);
    drop(loader);
    eventually(
        || (status_listeners(&root) == before).then_some(()),
        "the listener left with the loader",
    )
    .await;
}
