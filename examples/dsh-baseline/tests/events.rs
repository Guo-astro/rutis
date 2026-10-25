//! Events of a published plugin forwarded to rutis listeners.
#![cfg(all(unix, dsh_baseline))]

use std::sync::{Arc, Mutex};

use dsh_baseline::credentials::{self, CredentialKey, CredentialRef};
use dsh_baseline::events::{CredentialsRecordUpdated, CredentialsReferenceUpdated};
use rutis::host::*;
use rutis::Ctx;
use rutis_bridge::cordis::serde_json::json;
use rutis_test_support::eventually_with;

#[derive(Default, Clone)]
struct Seen(Arc<Mutex<Vec<String>>>);

impl Seen {
    fn push(&self, entry: String) {
        self.0.lock().unwrap().push(entry);
    }
}

/// Until `seen` has `expected`, with what it has in the failure.
async fn saw(seen: &Seen, expected: &str) {
    eventually_with(
        || {
            seen.0
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry == expected)
                .then_some(())
        },
        expected,
        || format!("seen: {:?}", seen.0.lock().unwrap()),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn credential_changes_reach_rutis_listeners() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let reference = seen.clone();
    ctx.on::<CredentialsReferenceUpdated>(move |event, _| {
        reference.push(format!("reference {}", event.0));
        async { Ok(None) }
    })
    .unwrap();
    let record = seen.clone();
    ctx.on::<CredentialsRecordUpdated>(move |event, _| {
        record.push(format!("record {}", event.0));
        async { Ok(None) }
    })
    .unwrap();
    let view = ctx.plugin(credentials::Plugin::new(credentials::Config {
        dsh_home: Some(dir.path().to_str().unwrap().into()),
        watch: Some(false),
        ..Default::default()
    }));
    (&view).await.unwrap();
    let store = ctx.get::<credentials::CredentialProvider>().unwrap();

    store
        .set(&CredentialRef::from("test/api-key"), "s3cret")
        .await
        .unwrap();
    saw(&seen, "reference test/api-key").await;

    store
        .modify_record(&CredentialKey::from("example/token"), |_| {
            Box::pin(async { Ok(Some(json!({ "kind": "api-key", "key": "k" }))) })
        })
        .await
        .unwrap();
    saw(&seen, "record example/token").await;

    drop(store);
    view.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}
