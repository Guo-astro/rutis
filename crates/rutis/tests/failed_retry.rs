//! A Failed fiber is retried only when its dependencies change (#327).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rutis::{BoxFuture, CordisError, Ctx, Effect, FiberState, Plugin, TypeKey};
use tokio::sync::Notify;

struct A;
struct B;

/// Provides `A` and `B` once released, so consumers register while it loads.
struct Provider(Arc<Notify>);

impl Plugin for Provider {
    fn name(&self) -> &str {
        "provider"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            self.0.notified().await;
            ctx.provide(A)?;
            ctx.provide(B)?;
            Ok(Effect::Done)
        })
    }
}

/// Injects `A` and `B` and fails every load, counting them.
struct Consumer(Arc<AtomicUsize>, [TypeKey; 2]);

impl Plugin for Consumer {
    fn name(&self) -> &str {
        "consumer"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.1
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(CordisError::PluginFailed("consumer fails".into()))
        })
    }
}

#[tokio::test]
async fn a_failed_load_is_retried_only_when_its_dependencies_change() {
    let root = Ctx::root().unwrap();
    let release = Arc::new(Notify::new());
    let loads = Arc::new(AtomicUsize::new(0));
    let provider = root.plugin(Provider(release.clone()));
    let consumer = root.plugin(Consumer(
        loads.clone(),
        [TypeKey::of::<A>(), TypeKey::of::<B>()],
    ));

    // Activating the provider notifies each of its keys: the consumer gets
    // one dependency check per key. The first loads it and fails; the rest
    // see the same dependencies and leave it Failed.
    release.notify_one();
    (&provider).await.unwrap();
    assert!((&consumer).await.is_err());
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(loads.load(Ordering::SeqCst), 1);

    // A new provider generation is a change of dependencies: it retries.
    release.notify_one();
    provider.restart().await.unwrap();
    assert!((&consumer).await.is_err());
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(loads.load(Ordering::SeqCst), 2);

    root.shutdown().await.unwrap();
}
