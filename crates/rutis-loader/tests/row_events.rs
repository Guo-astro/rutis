//! Rows on the logical bus (events protocol §3, §8): each row's state
//! changes are `rutis/row` events at its scope, and an instance of an
//! `instanced` group is a scope.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::Event;
use rutis_loader::report::RowState;
use rutis_loader::RowEvent;
use rutis_test_support::eventually;
use serde_json::json;

fn states(events: &Arc<Mutex<Vec<RowEvent>>>, row: &str) -> Vec<RowState> {
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
    let _serial = SERIAL.lock().await;
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
                .contains(&RowState::Running)
                .then_some(())
        },
        "a running",
    )
    .await;
    assert_eq!(states(&events, "a")[0], RowState::Pending);
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
                .contains(&RowState::Running)
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
                .contains(&RowState::Stopped)
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

/// The root is one per process, so a listener count sees the loaders of every
/// test running: these tests take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// guarantee: events/framework (rutis/row)
///
/// The loader follows every row with one `FiberStatusChanged` listener:
/// loading one row or sixty-five leaves the same listener count, and a row
/// stopping leaves it unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn rows_share_one_status_listener() {
    let _serial = SERIAL.lock().await;
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
    let count = |state: RowState| {
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
        || (count(RowState::Running) == 1).then_some(()),
        "one row running",
    )
    .await;
    assert_eq!(status_listeners(&root), before);
    reconcile(&loader, rows(65), &store).await;
    eventually(
        || (count(RowState::Running) == 65).then_some(()),
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
                .any(|(row, state)| row == "r10" && *state == RowState::Stopped))
            .then_some(())
        },
        "r10 stopped",
    )
    .await;
    assert_eq!(status_listeners(&root), before);
}

/// guarantee: events/framework (rutis/row)
///
/// Rows stopping with the loader still say `stopped`: a row of the loader's
/// tree and a row of an instance that the loader's disposer clears. The
/// listener is not owned by the loader's fiber, which is closing while its
/// rows go; it leaves when the last follower does, so the root's listener
/// count returns to what it was before the loader.
#[tokio::test(flavor = "multi_thread")]
async fn rows_stopped_with_the_loader_say_stopped() {
    let _serial = SERIAL.lock().await;
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
    let events: Arc<Mutex<Vec<(String, RowState)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on::<RowEvent>(move |event, _| {
        seen.lock().unwrap().push((event.row.clone(), event.state));
        std::future::ready(Ok(None))
    })
    .unwrap();
    let reached = |row: &str, state: RowState| {
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
            (reached("tree", RowState::Running) && reached("inner", RowState::Running))
                .then_some(())
        },
        "both rows running",
    )
    .await;
    view.dispose().await.unwrap();
    eventually(
        || {
            (reached("tree", RowState::Stopped) && reached("inner", RowState::Stopped))
                .then_some(())
        },
        "both rows stopped",
    )
    .await;
    eventually(
        || (status_listeners(&root) == before).then_some(()),
        "the loader's listener left with its last follower",
    )
    .await;
}

/// guarantee: events/framework (rutis/row)
///
/// A restarted loader holds one listener: the registration of the previous
/// run leaves with its last follower, and the new run's rows say their
/// changes through its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_loader_still_listens_once() {
    let _serial = SERIAL.lock().await;
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
    let events: Arc<Mutex<Vec<(String, RowState)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on::<RowEvent>(move |event, _| {
        seen.lock().unwrap().push((event.row.clone(), event.state));
        std::future::ready(Ok(None))
    })
    .unwrap();
    let count = |row: &str, state: RowState| {
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
        || (count("a", RowState::Running) == 1 && count("b", RowState::Running) == 1).then_some(()),
        "both rows running",
    )
    .await;
    assert_eq!(status_listeners(&root), listening);
    view.restart().await.unwrap();
    // A restart disposes the rows, and the loader disables a row disposed on
    // its own; they run again once enabled, through the new registration.
    assert_eq!(
        status_listeners(&root),
        listening,
        "one listener after the restart"
    );
    let started_a = count("a", RowState::Running);
    let started_b = count("b", RowState::Running);
    loader.set_disabled("a", false).await.unwrap();
    loader.set_disabled("b", false).await.unwrap();
    eventually(
        || {
            (count("a", RowState::Running) > started_a && count("b", RowState::Running) > started_b)
                .then_some(())
        },
        "both rows running again",
    )
    .await;
    assert_eq!(
        status_listeners(&root),
        listening,
        "one listener for the rows of the new run"
    );
    let stopped = count("b", RowState::Stopped);
    loader.set_disabled("b", true).await.unwrap();
    eventually(
        || (count("b", RowState::Stopped) == stopped + 1).then_some(()),
        "b stopped after the restart",
    )
    .await;
    assert_eq!(status_listeners(&root), listening);
    view.dispose().await.unwrap();
    eventually(
        || (status_listeners(&root) == before).then_some(()),
        "the listener left with the last follower",
    )
    .await;
}
