//! Rows on the logical bus (events protocol §3, §8): each row's state
//! changes are `rutis/row` events at its scope, and an instance of an
//! `instanced` group is a scope.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
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
