//! A Failed fiber is retried only when its dependencies change (#327).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, FiberView, PluginFactory, TypeKey,
};
use rutis::{Ctx, Error};
use tokio::sync::Notify;

struct A;
struct B;

fn injects() -> [TypeKey; 2] {
    [TypeKey::of::<A>(), TypeKey::of::<B>()]
}

/// Provides `A` and `B` once released, so consumers register while it loads.
struct Provider(Arc<Notify>);

impl DynPlugin for Provider {
    fn name(&self) -> &str {
        "provider"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.0.notified().await;
            ctx.provide(A)?;
            ctx.provide(B)?;
            Ok(Effect::Done)
        })
    }
}

/// Where a consumer of `A` and `B` fails its load; each attempt is counted.
#[derive(Clone, Copy)]
enum Failure {
    Apply,
    Validate,
    Build,
}

struct Consumer {
    failure: Failure,
    attempts: Arc<AtomicUsize>,
    injects: [TypeKey; 2],
}

impl DynPlugin for Consumer {
    fn name(&self) -> &str {
        "consumer"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn validate(&self) -> Result<(), Error> {
        if let Failure::Validate = self.failure {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            return Err(Error::from(ErrorKind::Validation {
                issues: vec!["consumer rejects".into()],
            }));
        }
        Ok(())
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(Error::from(ErrorKind::PluginFailed(
                "consumer fails".into(),
            )))
        })
    }
}

/// Builds the consumer for each generation; fails to build for `Build`.
struct ConsumerFactory(Failure, Arc<AtomicUsize>, [TypeKey; 2]);

impl PluginFactory<()> for ConsumerFactory {
    fn name(&self) -> &str {
        "consumer"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.2.to_vec()
    }

    fn build(&self, _: &()) -> Result<Box<dyn DynPlugin>, Error> {
        if let Failure::Build = self.0 {
            self.1.fetch_add(1, Ordering::SeqCst);
            return Err(Error::from(ErrorKind::PluginFailed(
                "consumer cannot be built".into(),
            )));
        }
        Ok(Box::new(Consumer {
            failure: self.0,
            attempts: self.1.clone(),
            injects: self.2.clone(),
        }))
    }
}

/// The consumer registers while its provider loads. Activating the provider
/// notifies each of its keys, so the consumer gets one dependency check per
/// key: the first loads it and fails, the rest see the same dependencies and
/// leave it Failed. A new provider generation changes them and retries it.
async fn retried_only_on_change(failure: Failure) {
    let root = Ctx::root().unwrap();
    let release = Arc::new(Notify::new());
    let attempts = Arc::new(AtomicUsize::new(0));
    let provider = root.plugin(Provider(release.clone()));
    let consumer: FiberView =
        root.plugin_with(ConsumerFactory(failure, attempts.clone(), injects()), ());

    release.notify_one();
    (&provider).await.unwrap();
    assert!((&consumer).await.is_err());
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    release.notify_one();
    provider.restart().await.unwrap();
    assert!((&consumer).await.is_err());
    assert_eq!(consumer.state().state, FiberState::Failed);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    root.shutdown().await.unwrap();
}

/// guarantee: core/features.errors.failed-retry
#[tokio::test]
async fn a_failed_apply_is_retried_only_when_its_dependencies_change() {
    retried_only_on_change(Failure::Apply).await;
}

/// guarantee: core/features.errors.failed-retry
#[tokio::test]
async fn a_failed_validate_is_retried_only_when_its_dependencies_change() {
    retried_only_on_change(Failure::Validate).await;
}

/// guarantee: core/features.errors.failed-retry
#[tokio::test]
async fn a_failed_build_is_retried_only_when_its_dependencies_change() {
    retried_only_on_change(Failure::Build).await;
}
