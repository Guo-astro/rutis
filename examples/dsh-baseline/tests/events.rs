//! Events of a published plugin forwarded to rutis listeners.
#![cfg(all(unix, dsh_baseline))]

use std::sync::{Arc, Mutex};

use dsh_baseline::credentials::{self, CredentialKey, CredentialRef};
use dsh_baseline::events::{CredentialsRecordUpdated, CredentialsReferenceUpdated};
use rutis::{BoxFuture, Call, CordisError, Ctx, EventKey, Listener};
use rutis_bridge::cordis::serde_json::json;
use rutis_test_support::eventually_with;

#[derive(Default, Clone)]
struct Seen(Arc<Mutex<Vec<String>>>);

impl Listener<CredentialsReferenceUpdated> for Seen {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a CredentialsReferenceUpdated,
        _: Call<'a, ()>,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .push(format!("reference {}", event.0));
            Ok(None)
        })
    }
}

impl Listener<CredentialsRecordUpdated> for Seen {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a CredentialsRecordUpdated,
        _: Call<'a, ()>,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(format!("record {}", event.0));
            Ok(None)
        })
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
    ctx.events()
        .on(
            &ctx,
            &EventKey::<CredentialsReferenceUpdated>::of(),
            seen.clone(),
        )
        .unwrap();
    ctx.events()
        .on(
            &ctx,
            &EventKey::<CredentialsRecordUpdated>::of(),
            seen.clone(),
        )
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
