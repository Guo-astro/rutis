use std::future::IntoFuture;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::{
    BoxFuture, CordisError, Ctx, DependencyStatus, Effect, FiberState, FiberView, Plugin, TypeKey,
};
use tokio::sync::oneshot;

struct Consumer {
    key: TypeKey,
}

impl Plugin for Consumer {
    fn name(&self) -> &str {
        "diagnostic-consumer"
    }

    fn injects(&self) -> &[TypeKey] {
        std::slice::from_ref(&self.key)
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

struct MetadataProbe {
    key: TypeKey,
    names: Arc<AtomicUsize>,
    injects: Arc<AtomicUsize>,
}

impl Plugin for MetadataProbe {
    fn name(&self) -> &str {
        self.names.fetch_add(1, Ordering::SeqCst);
        "diagnostic-metadata-probe"
    }

    fn injects(&self) -> &[TypeKey] {
        self.injects.fetch_add(1, Ordering::SeqCst);
        std::slice::from_ref(&self.key)
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

struct BlockingConsumer {
    key: TypeKey,
    cleanup: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl Plugin for BlockingConsumer {
    fn name(&self) -> &str {
        "diagnostic-blocking-consumer"
    }

    fn injects(&self) -> &[TypeKey] {
        std::slice::from_ref(&self.key)
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        let (entered, release) = self.cleanup.lock().unwrap().take().unwrap();
        Box::pin(async move {
            Ok(Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    let _ = entered.send(());
                    let _ = release.await;
                    Ok(())
                })
            })))
        })
    }
}

struct Provider {
    key: TypeKey,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl Plugin for Provider {
    fn name(&self) -> &str {
        "diagnostic-provider"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            ctx.provide_as(self.key.clone(), Arc::new(7u64))?;
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                let _ = entered.send(());
                let _ = release.await;
            }
            Ok(Effect::Done)
        })
    }
}

async fn settled(view: &FiberView) {
    tokio::time::timeout(Duration::from_secs(10), view.into_future())
        .await
        .expect("fiber did not settle")
        .expect("fiber failed");
}

async fn state(view: &FiberView, expected: FiberState) {
    let mut watch = view.watch();
    tokio::time::timeout(Duration::from_secs(10), async {
        while watch.borrow().state != expected {
            watch.changed().await.expect("fiber watch closed");
        }
    })
    .await
    .expect("fiber did not reach expected state");
}

fn dependency(root: &Ctx, view: &FiberView) -> rutis::DependencyDiagnostics {
    root.diagnostics()
        .plugins
        .into_iter()
        .find(|plugin| plugin.id == view.id)
        .expect("fiber missing from diagnostics")
        .injects
        .into_iter()
        .next()
        .expect("declared dependency missing")
}

#[tokio::test]
async fn keyed_isolates_report_missing_then_exact_binding_and_recovery() {
    let root = Ctx::root().unwrap();
    let key = TypeKey::keyed::<u64>("primary");
    let a = root.isolate(key.clone(), "A");
    let b = root.isolate(key.clone(), "B");
    let provider_a = a.plugin(Provider {
        key: key.clone(),
        gate: Mutex::new(None),
    });
    settled(&provider_a).await;
    let consumer_a = a.plugin(Consumer { key: key.clone() });
    let consumer_b = b.plugin(Consumer { key: key.clone() });
    settled(&consumer_a).await;
    settled(&consumer_b).await;

    let ready = dependency(&root, &consumer_a);
    let missing = dependency(&root, &consumer_b);
    assert_eq!(ready.status, DependencyStatus::Ready);
    assert_eq!(ready.scope.as_deref(), Some("A"));
    assert_eq!(missing.status, DependencyStatus::Missing);
    assert_eq!(missing.scope.as_deref(), Some("B"));
    assert!(missing.key.describe().contains("u64#primary"));
    assert_eq!(consumer_b.state().state, FiberState::Pending);
    assert!(root
        .diagnostics()
        .plugins
        .iter()
        .find(|plugin| plugin.id == consumer_b.id)
        .unwrap()
        .error
        .is_none());

    let provider_b = b.plugin(Provider {
        key: key.clone(),
        gate: Mutex::new(None),
    });
    settled(&provider_b).await;
    state(&consumer_b, FiberState::Active).await;
    let diagnostics = root.diagnostics();
    let bound = diagnostics
        .plugins
        .iter()
        .find(|plugin| plugin.id == consumer_b.id)
        .unwrap();
    assert_eq!(bound.injects[0].status, DependencyStatus::Ready);
    assert!(bound.resolved_dependencies.iter().any(|dep| {
        dep.key == key
            && dep.scope.as_deref() == Some("B")
            && dep.provider == provider_b.id
            && diagnostics.bindings.iter().any(|binding| {
                binding.key == dep.key
                    && binding.scope == dep.scope
                    && binding.provider == dep.provider
                    && binding.generation == dep.generation
            })
    }));

    provider_b.dispose().await.unwrap();
    state(&consumer_b, FiberState::Pending).await;
    assert_eq!(
        dependency(&root, &consumer_b).status,
        DependencyStatus::Missing
    );
    assert_eq!(
        dependency(&root, &consumer_a).status,
        DependencyStatus::Ready
    );
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn diagnostics_explain_inactive_and_removing_provider() {
    let root = Ctx::root().unwrap();
    let key = TypeKey::of::<u64>();
    let (provided, provided_rx) = oneshot::channel();
    let (resume, resume_rx) = oneshot::channel();
    let provider = root.plugin(Provider {
        key: key.clone(),
        gate: Mutex::new(Some((provided, resume_rx))),
    });
    tokio::time::timeout(Duration::from_secs(10), provided_rx)
        .await
        .expect("provider did not register its binding")
        .unwrap();
    let (cleanup_started, cleanup_started_rx) = oneshot::channel();
    let (cleanup_resume, cleanup_resume_rx) = oneshot::channel();
    let consumer = root.plugin(BlockingConsumer {
        key: key.clone(),
        cleanup: Mutex::new(Some((cleanup_started, cleanup_resume_rx))),
    });
    settled(&consumer).await;
    assert_eq!(
        dependency(&root, &consumer).status,
        DependencyStatus::ProviderInactive(FiberState::Loading)
    );

    resume.send(()).unwrap();
    settled(&provider).await;
    state(&consumer, FiberState::Active).await;
    assert_eq!(dependency(&root, &consumer).status, DependencyStatus::Ready);

    let disposing = provider.dispose();
    tokio::time::timeout(Duration::from_secs(10), cleanup_started_rx)
        .await
        .expect("consumer cleanup did not start")
        .unwrap();
    assert_eq!(
        dependency(&root, &consumer).status,
        DependencyStatus::Removing
    );
    cleanup_resume.send(()).unwrap();
    disposing.await.unwrap();
    state(&consumer, FiberState::Pending).await;
    assert_eq!(
        dependency(&root, &consumer).status,
        DependencyStatus::Missing
    );
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn cached_check_diagnostics_do_not_call_user_code() {
    let root = Ctx::root().unwrap();
    let mode = Arc::new(AtomicU8::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let names = Arc::new(AtomicUsize::new(0));
    let injects = Arc::new(AtomicUsize::new(0));
    let check_mode = mode.clone();
    let check_calls = calls.clone();
    root.provide_as_with_check(TypeKey::of::<u64>(), Arc::new(1u64), move || {
        check_calls.fetch_add(1, Ordering::SeqCst);
        match check_mode.load(Ordering::SeqCst) {
            0 => false,
            1 => panic!("diagnostic check panic"),
            _ => true,
        }
    })
    .unwrap();
    let consumer = root.plugin(MetadataProbe {
        key: TypeKey::of::<u64>(),
        names: names.clone(),
        injects: injects.clone(),
    });
    settled(&consumer).await;
    assert_eq!(
        dependency(&root, &consumer).status,
        DependencyStatus::CheckRejected
    );
    let before = calls.load(Ordering::SeqCst);
    let name_before = names.load(Ordering::SeqCst);
    let inject_before = injects.load(Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(
            dependency(&root, &consumer).status,
            DependencyStatus::CheckRejected
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), before);
    assert_eq!(names.load(Ordering::SeqCst), name_before);
    assert_eq!(injects.load(Ordering::SeqCst), inject_before);
    assert_eq!(consumer.state().state, FiberState::Pending);

    mode.store(1, Ordering::SeqCst);
    root.refresh();
    settled(&consumer).await;
    assert_eq!(
        dependency(&root, &consumer).status,
        DependencyStatus::CheckPanicked
    );
    let before = calls.load(Ordering::SeqCst);
    root.diagnostics();
    assert_eq!(calls.load(Ordering::SeqCst), before);

    mode.store(2, Ordering::SeqCst);
    root.refresh();
    state(&consumer, FiberState::Active).await;
    assert_eq!(dependency(&root, &consumer).status, DependencyStatus::Ready);
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_fiber_drops_dynamic_dependency_key() {
    let root = Ctx::root().unwrap();
    let qualifier: Arc<str> = Arc::from("transient");
    let key = TypeKey::keyed_dynamic::<u64>(qualifier.clone());
    let consumer = root.plugin(Consumer { key: key.clone() });
    settled(&consumer).await;
    assert_eq!(
        dependency(&root, &consumer).status,
        DependencyStatus::Missing
    );
    consumer.shutdown().await.unwrap();
    assert!(root
        .diagnostics()
        .plugins
        .iter()
        .all(|plugin| plugin.id != consumer.id));
    drop(consumer);
    drop(key);
    assert_eq!(Arc::strong_count(&qualifier), 1);
    root.shutdown().await.unwrap();
}

struct ShutdownProbe {
    context: Arc<Mutex<Option<Ctx>>>,
    cleanup: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl Plugin for ShutdownProbe {
    fn name(&self) -> &str {
        "shutdown-probe"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        *self.context.lock().unwrap() = Some(ctx.clone());
        let cleanup = self.cleanup.lock().unwrap().take();
        Box::pin(async move {
            Ok(match cleanup {
                Some((entered, release)) => Effect::AsyncDisposer(Box::new(move || {
                    Box::pin(async move {
                        let _ = entered.send(());
                        let _ = release.await;
                        Ok(())
                    })
                })),
                None => Effect::Done,
            })
        })
    }
}

fn assert_shutdown_state(ctx: &Ctx, expected: bool) {
    assert_eq!(ctx.is_shutting_down(), expected);
    assert_eq!(ctx.diagnostics().shutting_down, expected);
}

/// guarantee: core/features.shutdown-state
#[tokio::test]
async fn shutdown_state_is_shared_before_cleanup_and_after_completion() {
    let root = Ctx::root().unwrap();
    let isolated = root.isolate(TypeKey::of::<u64>(), "isolated");
    let context = Arc::new(Mutex::new(None));
    let (entered, cleanup_started) = oneshot::channel();
    let (resume, release) = oneshot::channel();
    let view = root.plugin(ShutdownProbe {
        context: context.clone(),
        cleanup: Mutex::new(Some((entered, release))),
    });
    settled(&view).await;
    let child = context.lock().unwrap().clone().unwrap();
    for ctx in [&root, &isolated, &child] {
        assert_shutdown_state(ctx, false);
    }

    let shutdown = root.shutdown();
    // shutdown closes admission when called, before its future is polled.
    for ctx in [&root, &isolated, &child] {
        assert_shutdown_state(ctx, true);
    }
    tokio::time::timeout(Duration::from_secs(10), cleanup_started)
        .await
        .expect("cleanup did not start")
        .unwrap();
    assert_shutdown_state(&child, true);
    resume.send(()).unwrap();
    shutdown.await.unwrap();
    drop(view);
    for ctx in [&root, &isolated, &child] {
        assert_shutdown_state(ctx, true);
    }
}

/// guarantee: core/features.shutdown-state
#[tokio::test]
async fn closing_a_child_does_not_report_root_shutdown() {
    let root = Ctx::root().unwrap();
    let mut children = Vec::new();
    for final_shutdown in [false, true] {
        let context = Arc::new(Mutex::new(None));
        let view = root.plugin(ShutdownProbe {
            context: context.clone(),
            cleanup: Mutex::new(None),
        });
        settled(&view).await;
        let child = context.lock().unwrap().clone().unwrap();
        if final_shutdown {
            view.shutdown().await.unwrap();
        } else {
            view.dispose().await.unwrap();
        }
        assert_shutdown_state(&child, false);
        assert_shutdown_state(&root, false);
        children.push(child);
    }
    root.shutdown().await.unwrap();
    for child in children {
        assert_shutdown_state(&child, true);
    }
    assert_shutdown_state(&root, true);
}
