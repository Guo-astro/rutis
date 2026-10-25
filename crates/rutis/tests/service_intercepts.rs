mod common;

use std::future::IntoFuture;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, Key, ServiceReadFailure, TypeKey,
};
use rutis::internal::{CtxInternal, ServiceIntercept, ServiceWriteFailure, ServiceWriter};
use rutis::{Ctx, Error};
use tokio::sync::oneshot;

use common::{on_thread, still_pending};

struct Capture {
    injects: Vec<TypeKey>,
    slot: Arc<Mutex<Option<Ctx>>>,
}

impl DynPlugin for Capture {
    fn name(&self) -> &str {
        "service-intercept-capture"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.slot.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

async fn child(parent: &Ctx, injects: Vec<TypeKey>) -> (rutis::host::FiberView, Ctx) {
    let slot = Arc::new(Mutex::new(None));
    let view = parent.plugin(Capture {
        injects,
        slot: slot.clone(),
    });
    (&view).into_future().await.unwrap();
    let ctx = slot.lock().unwrap().take().unwrap();
    (view, ctx)
}

#[tokio::test]
async fn strict_reads_chain_and_locator_bypass_with_recorded_denial() {
    let root = Ctx::root().unwrap();
    let key = Key::<u64>::named("read");
    root.provide_at(key.clone(), Arc::new(10u64)).unwrap();
    let (view, reader) = child(&root, vec![key.erase()]).await;
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_hook = seen.clone();
    let replace = root
        .intercept_require_as::<u64>(key.clone(), move |value| {
            seen_hook.fetch_add(1, Ordering::SeqCst);
            ServiceIntercept::Replace(Arc::new(*value + 1))
        })
        .unwrap();
    assert_eq!(*reader.get_at(&key).unwrap(), 10);
    assert_eq!(seen.load(Ordering::SeqCst), 0);
    assert_eq!(*reader.require_at(&key).unwrap(), 11);

    let deny = root
        .intercept_require_as::<u64>(key.clone(), |_| ServiceIntercept::Deny)
        .unwrap();
    assert_eq!(
        reader.require_at(&key).unwrap_err().reason,
        ServiceReadFailure::InterceptDenied
    );
    let accesses = root
        .diagnostics()
        .plugins
        .into_iter()
        .find(|plugin| plugin.id == view.id)
        .unwrap()
        .accesses;
    assert!(accesses.iter().any(|access| {
        access.key == key.erase()
            && access.provider.is_some()
            && access.failure == Some(ServiceReadFailure::InterceptDenied)
    }));
    deny.dispose().await.unwrap();

    let panic_hook = root
        .intercept_require_as::<u64>(key.clone(), |_| panic!("read hook"))
        .unwrap();
    assert_eq!(
        reader.require_at(&key).unwrap_err().reason,
        ServiceReadFailure::InterceptPanicked
    );
    panic_hook.dispose().await.unwrap();

    let nested_reader = reader.clone();
    let nested_key = key.clone();
    let reentrant = root
        .intercept_require_as::<u64>(key.clone(), move |_| {
            assert_eq!(
                nested_reader.require_at(&nested_key).unwrap_err().reason,
                ServiceReadFailure::InterceptReentrant
            );
            ServiceIntercept::Continue
        })
        .unwrap();
    assert_eq!(*reader.require_at(&key).unwrap(), 11);
    reentrant.dispose().await.unwrap();
    replace.dispose().await.unwrap();
    assert_eq!(*reader.require_at(&key).unwrap(), 10);
}

#[tokio::test]
async fn strict_read_checks_reject_before_interceptor_runs() {
    let root = Ctx::root().unwrap();
    let key = Key::<u64>::named("precheck");
    let service = root.provide_at(key.clone(), Arc::new(1u64)).unwrap();
    let (_view, undeclared) = child(&root, vec![]).await;
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_hook = hits.clone();
    root.intercept_require_as::<u64>(key.clone(), move |_| {
        hits_hook.fetch_add(1, Ordering::SeqCst);
        ServiceIntercept::Continue
    })
    .unwrap();
    assert_eq!(
        undeclared.require_at(&key).unwrap_err().reason,
        ServiceReadFailure::Undeclared
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    let (_view, declared) = child(&root, vec![key.erase()]).await;
    service.dispose().await.unwrap();
    assert!(matches!(
        declared.require_at(&key).unwrap_err().reason,
        ServiceReadFailure::Unavailable(_)
    ));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn hooks_match_full_key_scope_and_reader_ancestry() {
    let root = Ctx::root().unwrap();
    let key = Key::<u64>::named("shared");
    root.provide_at(key.clone(), Arc::new(1u64)).unwrap();
    let (_a_view, a) = child(&root, vec![key.erase()]).await;
    let (_b_view, b) = child(&root, vec![key.erase()]).await;
    a.intercept_require_as::<u64>(key.clone(), |_| ServiceIntercept::Replace(Arc::new(2)))
        .unwrap();
    assert_eq!(*a.require_at(&key).unwrap(), 2);
    assert_eq!(*b.require_at(&key).unwrap(), 1);

    let write_key = Key::<u32>::named("write-ancestry");
    let (_binding, root_writer) = root
        .provide_mut_at(write_key.clone(), Arc::new(1u32))
        .unwrap();
    let write_hits = Arc::new(AtomicUsize::new(0));
    let write_hits_hook = write_hits.clone();
    a.intercept_set_as::<u32>(write_key, move |_| {
        write_hits_hook.fetch_add(1, Ordering::SeqCst);
        ServiceIntercept::Continue
    })
    .unwrap();
    root_writer.set(&root, Arc::new(2)).unwrap();
    assert_eq!(write_hits.load(Ordering::SeqCst), 0);

    let instance_key = Key::<u64>::of().instance(a.instance());
    a.provide_at(instance_key.clone(), Arc::new(5u64)).unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_hook = hits.clone();
    a.intercept_require_as::<u64>(instance_key.clone(), move |_| {
        hits_hook.fetch_add(1, Ordering::SeqCst);
        ServiceIntercept::Continue
    })
    .unwrap();
    assert_eq!(*a.require_at(&instance_key).unwrap(), 5);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        b.require_at(&instance_key).unwrap_err().reason,
        ServiceReadFailure::OutOfScope
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(matches!(
        root.intercept_require_as::<u64>(instance_key, |_| ServiceIntercept::Continue)
            .map_err(Error::into_kind),
        Err(ErrorKind::InstanceOutOfScope { .. })
    ));

    let mutable_instance = Key::<u32>::of().instance(a.instance());
    let (_binding, instance_writer) = a.provide_mut_at(mutable_instance, Arc::new(1u32)).unwrap();
    assert_eq!(
        instance_writer.set(&b, Arc::new(2)).unwrap_err().reason,
        ServiceWriteFailure::WrongOwner
    );
    instance_writer.set(&a, Arc::new(3)).unwrap();

    let isolated_key = Key::<u32>::named("isolated");
    let left = root.isolate(isolated_key.clone(), "left");
    let right = root.isolate(isolated_key.clone(), "right");
    left.provide_at(isolated_key.clone(), Arc::new(10u32))
        .unwrap();
    right
        .provide_at(isolated_key.clone(), Arc::new(20u32))
        .unwrap();
    left.intercept_require_as::<u32>(isolated_key.clone(), |_| {
        ServiceIntercept::Replace(Arc::new(11))
    })
    .unwrap();
    assert_eq!(*left.require_at(&isolated_key).unwrap(), 11);
    assert_eq!(*right.require_at(&isolated_key).unwrap(), 20);
}

#[tokio::test]
async fn mutable_writer_respects_owner_generation_and_write_hooks() {
    let root = Ctx::root().unwrap();
    let key = Key::<u64>::named("mutable");
    let (binding, writer) = root.provide_mut_at(key.clone(), Arc::new(1u64)).unwrap();
    let (view, reader) = child(&root, vec![key.erase()]).await;
    let old = reader.require_at(&key).unwrap();
    assert_eq!(
        writer.set(&reader, Arc::new(2)).unwrap_err().reason,
        ServiceWriteFailure::WrongOwner
    );
    writer.set(&root, Arc::new(2)).unwrap();
    assert_eq!(*old, 1);
    assert_eq!(*reader.require_at(&key).unwrap(), 2);
    assert_eq!(view.state().state, FiberState::Active);

    let replace = root
        .intercept_set_as::<u64>(key.clone(), |value| {
            ServiceIntercept::Replace(Arc::new(*value + 10))
        })
        .unwrap();
    writer.set(&root, Arc::new(3)).unwrap();
    assert_eq!(*reader.require_at(&key).unwrap(), 13);
    let deny = root
        .intercept_set_as::<u64>(key.clone(), |_| ServiceIntercept::Deny)
        .unwrap();
    assert_eq!(
        writer.set(&root, Arc::new(4)).unwrap_err().reason,
        ServiceWriteFailure::InterceptDenied
    );
    assert_eq!(*reader.require_at(&key).unwrap(), 13);
    deny.dispose().await.unwrap();

    let panic_hook = root
        .intercept_set_as::<u64>(key.clone(), |_| panic!("write hook"))
        .unwrap();
    assert_eq!(
        writer.set(&root, Arc::new(4)).unwrap_err().reason,
        ServiceWriteFailure::InterceptPanicked
    );
    panic_hook.dispose().await.unwrap();
    replace.dispose().await.unwrap();

    let writer = Arc::new(writer);
    let nested_writer = writer.clone();
    let nested_root = root.clone();
    let reentrant = root
        .intercept_set_as::<u64>(key.clone(), move |_| {
            assert_eq!(
                nested_writer
                    .set(&nested_root, Arc::new(99))
                    .unwrap_err()
                    .reason,
                ServiceWriteFailure::InterceptReentrant
            );
            ServiceIntercept::Continue
        })
        .unwrap();
    writer.set(&root, Arc::new(14)).unwrap();
    assert_eq!(*reader.require_at(&key).unwrap(), 14);
    reentrant.dispose().await.unwrap();

    binding.dispose().await.unwrap();
    let (_new_binding, current) = root.provide_mut_at(key.clone(), Arc::new(50u64)).unwrap();
    assert_eq!(
        writer.set(&root, Arc::new(99)).unwrap_err().reason,
        ServiceWriteFailure::Stale
    );
    current.set(&root, Arc::new(51)).unwrap();
    assert_eq!(*reader.require_at(&key).unwrap(), 51);
}

struct DropProbe {
    ctx: Ctx,
    key: Key<DropProbe>,
    check: bool,
    saw_new_value: Arc<AtomicBool>,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        if self.check {
            let replacement = self.ctx.get_at(&self.key).unwrap();
            self.saw_new_value
                .store(!replacement.check, Ordering::SeqCst);
        }
    }
}

#[tokio::test]
async fn replaced_value_drop_can_read_registry_after_commit() {
    let root = Ctx::root().unwrap();
    let key = Key::<DropProbe>::named("drop-reentry");
    let saw_new_value = Arc::new(AtomicBool::new(false));
    let old = Arc::new(DropProbe {
        ctx: root.clone(),
        key: key.clone(),
        check: true,
        saw_new_value: saw_new_value.clone(),
    });
    let (binding, writer) = root.provide_mut_at(key.clone(), old).unwrap();
    writer
        .set(
            &root,
            Arc::new(DropProbe {
                ctx: root.clone(),
                key,
                check: false,
                saw_new_value: saw_new_value.clone(),
            }),
        )
        .unwrap();
    assert!(saw_new_value.load(Ordering::SeqCst));
    drop(writer);
    binding.dispose().await.unwrap();
}

struct PanicOnDrop(bool);

impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        if self.0 {
            panic!("old service value dropped");
        }
    }
}

#[tokio::test]
async fn replaced_value_drop_panic_reports_to_sink_after_commit() {
    let failures = Arc::new(AtomicUsize::new(0));
    let failures_sink = failures.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |error| {
            assert!(error.to_string().contains("old service value dropped"));
            failures_sink.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let key = Key::<PanicOnDrop>::named("drop-panic");
    let (binding, writer) = root
        .provide_mut_at(key.clone(), Arc::new(PanicOnDrop(true)))
        .unwrap();
    writer.set(&root, Arc::new(PanicOnDrop(false))).unwrap();
    assert_eq!(failures.load(Ordering::SeqCst), 1);
    assert!(!root.get_at(&key).unwrap().0);
    drop(writer);
    binding.dispose().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_writer_cannot_commit_after_binding_is_replaced_during_hook() {
    let root = Ctx::root().unwrap();
    let key = Key::<u64>::named("race");
    let (old_binding, old_writer) = root.provide_mut_at(key.clone(), Arc::new(1u64)).unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release = Arc::new(Mutex::new(release_rx));
    let entered_hook = entered.clone();
    let release_hook = release.clone();
    let _hook = root
        .intercept_set_as::<u64>(key.clone(), move |_| {
            if let Some(tx) = entered_hook.lock().unwrap().take() {
                tx.send(()).unwrap();
            }
            release_hook.lock().unwrap().recv().unwrap();
            ServiceIntercept::Continue
        })
        .unwrap();
    let root_writer = root.clone();
    let write = tokio::task::spawn_blocking(move || old_writer.set(&root_writer, Arc::new(2)));
    entered_rx.await.unwrap();
    old_binding.dispose().await.unwrap();
    let (_new_binding, current) = root.provide_mut_at(key.clone(), Arc::new(10u64)).unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(
        write.await.unwrap().unwrap_err().reason,
        ServiceWriteFailure::Stale
    );
    assert_eq!(*root.get_at(&key).unwrap(), 10);
    drop(current);
}

#[tokio::test(start_paused = true)]
async fn subtree_shutdown_waits_for_selected_read_hook() {
    let root = Ctx::root().unwrap();
    let key = Key::<u64>::named("shutdown");
    root.provide_at(key.clone(), Arc::new(1u64)).unwrap();
    let (view, reader) = child(&root, vec![key.erase()]).await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release = Arc::new(Mutex::new(release_rx));
    let entered_hook = entered.clone();
    let release_hook = release.clone();
    reader
        .intercept_require_as::<u64>(key.clone(), move |_| {
            if let Some(tx) = entered_hook.lock().unwrap().take() {
                tx.send(()).unwrap();
            }
            release_hook.lock().unwrap().recv().unwrap();
            ServiceIntercept::Continue
        })
        .unwrap();
    let reader_task = reader.clone();
    let read = on_thread(move || reader_task.require_at(&key));
    entered_rx.await.unwrap();
    let shutdown = view.shutdown();
    tokio::pin!(shutdown);
    assert!(
        still_pending(&mut shutdown).await,
        "shutdown waits for the read hook already running"
    );
    release_tx.send(()).unwrap();
    assert_eq!(*read.await.unwrap().unwrap(), 1);
    shutdown.await.unwrap();
}

#[tokio::test]
async fn different_key_reentry_allowed_for_read_and_write() {
    let root = Ctx::root().unwrap();
    let key_a = Key::<u64>::named("reentry-a");
    let key_b = Key::<u64>::named("reentry-b");
    root.provide_at(key_a.clone(), Arc::new(1u64)).unwrap();
    root.provide_at(key_b.clone(), Arc::new(2u64)).unwrap();

    // B has its own read hook that replaces the value
    let b_read_hits = Arc::new(AtomicUsize::new(0));
    let b_read_hits_hook = b_read_hits.clone();
    let _b_hook = root
        .intercept_require_as::<u64>(key_b.clone(), move |v| {
            b_read_hits_hook.fetch_add(1, Ordering::SeqCst);
            ServiceIntercept::Replace(Arc::new(*v + 10))
        })
        .unwrap();

    // A's read hook does require B — must succeed, not InterceptReentrant
    let root_hook = root.clone();
    let key_b_hook = key_b.clone();
    let b_value_seen = Arc::new(Mutex::new(None));
    let b_value_seen_copy = b_value_seen.clone();
    root.intercept_require_as::<u64>(key_a.clone(), move |_| {
        let b_val = root_hook.require_at(&key_b_hook).unwrap();
        b_value_seen_copy.lock().unwrap().replace(*b_val);
        ServiceIntercept::Continue
    })
    .unwrap();

    let result = root.require_at(&key_a).unwrap();
    assert_eq!(*result, 1);
    assert_eq!(b_read_hits.load(Ordering::SeqCst), 1);
    assert_eq!(*b_value_seen.lock().unwrap(), Some(12)); // 2 + 10 from B's hook

    // Write: hook on C's set writes D, D has its own set hook
    let key_c = Key::<u64>::named("reentry-c");
    let key_d = Key::<u64>::named("reentry-d");
    let (_b_c, writer_c) = root.provide_mut_at(key_c.clone(), Arc::new(10u64)).unwrap();
    let (_b_d, writer_d) = root.provide_mut_at(key_d.clone(), Arc::new(20u64)).unwrap();

    let d_write_hits = Arc::new(AtomicUsize::new(0));
    let d_write_hits_hook = d_write_hits.clone();
    root.intercept_set_as::<u64>(key_d.clone(), move |v| {
        d_write_hits_hook.fetch_add(1, Ordering::SeqCst);
        ServiceIntercept::Replace(Arc::new(*v + 100))
    })
    .unwrap();

    let writer_d = Arc::new(writer_d);
    let writer_d_hook = writer_d.clone();
    let root_copy = root.clone();
    root.intercept_set_as::<u64>(key_c.clone(), move |_| {
        writer_d_hook.set(&root_copy, Arc::new(30)).unwrap();
        ServiceIntercept::Continue
    })
    .unwrap();

    writer_c.set(&root, Arc::new(11)).unwrap();
    assert_eq!(d_write_hits.load(Ordering::SeqCst), 1);
    assert_eq!(*root.get_at(&key_d).unwrap(), 130); // 30 + 100 from D's hook
}

#[tokio::test]
async fn writer_set_fails_stale_during_binding_removal() {
    let root = Ctx::root().unwrap();
    let key = TypeKey::keyed::<u64>("evict");
    let (binding_disposer, writer) = root.provide_mut_at(key.clone(), Arc::new(1u64)).unwrap();

    // Drive disposal in background; evict_and_finalize runs mark_removing_if
    // synchronously before any await, so the removing flag is set quickly.
    let dispose_task = tokio::spawn(binding_disposer.dispose());

    // Spin-wait until the binding is marked removing or fully evicted.
    // If eviction completes before we observe removing=true, the write
    // will still fail with Stale because the binding is no longer current.
    for _ in 0..1000 {
        let diags = root.diagnostics();
        if diags.bindings.iter().any(|b| b.key == key && b.removing) {
            break;
        }
        if !diags.bindings.iter().any(|b| b.key == key) {
            break;
        }
        tokio::task::yield_now().await;
    }

    let err = writer.set(&root, Arc::new(2)).unwrap_err();
    assert_eq!(err.reason, ServiceWriteFailure::Stale);

    dispose_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Regression tests for candidate-value Drop reentry deadlock (PR #55 P1)
// ---------------------------------------------------------------------------

/// A value whose Drop calls `Ctx::effect()`. If dropped inside a framework
/// lock, the reentrant effect triggers a self-deadlock.
struct DropEffect {
    ctx: Ctx,
    label: &'static str,
}

impl Drop for DropEffect {
    fn drop(&mut self) {
        self.ctx.effect_named(self.label, || Effect::Done).unwrap();
    }
}

/// Test 1: When the old binding has been removed and the writer is stale,
/// writing a candidate whose Drop reenters the framework must not deadlock.
#[tokio::test]
async fn writer_set_stale_candidate_drop_no_deadlock_replaced_binding() {
    let root = Ctx::root().unwrap();
    let key = TypeKey::keyed::<DropEffect>("drop-replaced");
    let (binding_disposer, writer) = root
        .provide_mut_at(
            key.clone(),
            Arc::new(DropEffect {
                ctx: root.clone(),
                label: "drop-replaced-initial",
            }),
        )
        .unwrap();

    // Drive disposal in background to make the writer stale.
    let dispose_task = tokio::spawn(binding_disposer.dispose());

    // Spin-wait until the binding is gone or marked removing.
    for _ in 0..1000 {
        let diags = root.diagnostics();
        if diags.bindings.iter().any(|b| b.key == key && b.removing) {
            break;
        }
        if !diags.bindings.iter().any(|b| b.key == key) {
            break;
        }
        tokio::task::yield_now().await;
    }

    // Write a candidate whose Drop calls root.effect(). Without the fix this
    // deadlocks because the candidate is dropped inside framework locks.
    // Use a dedicated OS thread + channel so the Mutex deadlock is observable
    // via recv_timeout (tokio::time::timeout cannot cancel a synchronously
    // blocked thread).
    let root2 = root.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let candidate = DropEffect {
        ctx: root.clone(),
        label: "drop-replaced-candidate",
    };
    std::thread::spawn(move || {
        let res = writer.set(&root2, Arc::new(candidate));
        let _ = tx.send(res);
    });

    let result = rx.recv_timeout(Duration::from_secs(10));
    assert!(
        result.is_ok(),
        "set should not deadlock (recv_timeout means Mutex deadlock)"
    );
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.reason, ServiceWriteFailure::Stale);

    dispose_task.await.unwrap().unwrap();
}

/// Test 2: When the binding is marked removing (but not yet evicted from the
/// registry), writing a DropEffect candidate must not deadlock.
#[tokio::test]
async fn writer_set_stale_candidate_drop_no_deadlock_removing_flag() {
    let root = Ctx::root().unwrap();
    let key = TypeKey::keyed::<DropEffect>("drop-removing");
    let (binding_disposer, writer) = root
        .provide_mut_at(
            key.clone(),
            Arc::new(DropEffect {
                ctx: root.clone(),
                label: "drop-removing-initial",
            }),
        )
        .unwrap();

    let dispose_task = tokio::spawn(binding_disposer.dispose());

    // Spin until we observe removing=true (not just binding absent).
    for _ in 0..1000 {
        let diags = root.diagnostics();
        if diags.bindings.iter().any(|b| b.key == key && b.removing) {
            break;
        }
        tokio::task::yield_now().await;
    }

    let root2 = root.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let candidate = DropEffect {
        ctx: root.clone(),
        label: "drop-removing-candidate",
    };
    std::thread::spawn(move || {
        let res = writer.set(&root2, Arc::new(candidate));
        let _ = tx.send(res);
    });

    let result = rx.recv_timeout(Duration::from_secs(10));
    assert!(
        result.is_ok(),
        "set should not deadlock (recv_timeout means Mutex deadlock)"
    );
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.reason, ServiceWriteFailure::Stale);

    dispose_task.await.unwrap().unwrap();
}

/// Plugin that provides a mutable `DropEffect` service and stores the writer
/// for the test harness.
struct MutableDropProvider {
    key: Key<DropEffect>,
    writer: Arc<Mutex<Option<ServiceWriter<DropEffect>>>>,
    ctx_slot: Arc<Mutex<Option<Ctx>>>,
    drop_ctx: Ctx,
}

impl DynPlugin for MutableDropProvider {
    fn name(&self) -> &str {
        "mutable-drop-provider"
    }
    fn injects(&self) -> Vec<TypeKey> {
        Vec::new()
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let (disposer, writer) = ctx.provide_mut_at(
                self.key.clone(),
                Arc::new(DropEffect {
                    ctx: self.drop_ctx.clone(),
                    label: "drop-gen-initial",
                }),
            )?;
            *self.writer.lock().unwrap() = Some(writer);
            *self.ctx_slot.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Disposer(Box::new(move || {
                drop(disposer);
                Ok(())
            })))
        })
    }
}

/// Test 3: After the provider fiber restarts, `transition.generation`
/// differs from the binding's `provider_gen`. This hits the generation
/// check inside the locked section (path 2 of the three write-failure
/// paths). The candidate `DropEffect` must be dropped without deadlock.
///
/// The stale writer still reaches the binding's generation check. Drop's
/// reentrant effect uses root because an old apply context cannot register
/// anything after the provider restarts.
///
/// On the old (pre-IIFE) code, path 2 does not deadlock either: Rust drops
/// locals in reverse declaration order, so the `value` local is dropped
/// *after* the `transition` and `_admission` guards release their locks.
/// Path 2 is accidentally safe; the IIFE fix makes that boundary explicit.
#[tokio::test]
async fn writer_set_stale_candidate_drop_no_deadlock_generation_stale() {
    let root = Ctx::root().unwrap();
    let key = Key::<DropEffect>::named("drop-generation");
    let writer_slot = Arc::new(Mutex::new(None));
    let ctx_slot = Arc::new(Mutex::new(None));
    let provider = MutableDropProvider {
        key: key.clone(),
        writer: writer_slot.clone(),
        ctx_slot: ctx_slot.clone(),
        drop_ctx: root.clone(),
    };
    let view = root.plugin(provider);
    (&view).into_future().await.unwrap();
    let writer = writer_slot.lock().unwrap().take().unwrap();
    let provider_ctx = ctx_slot.lock().unwrap().take().unwrap();

    // Restart the provider. restart() awaits the full unload→load cycle,
    // so after this the fiber is Active with an incremented generation.
    view.restart().await.unwrap();

    // Verify the error reports the old binding's provider_gen (= 1),
    // confirming the generation check inside the locked section was hit
    // rather than a preflight failure.
    let err = writer
        .set(
            &provider_ctx,
            Arc::new(DropEffect {
                ctx: root.clone(),
                label: "check-gen",
            }),
        )
        .unwrap_err();
    assert_eq!(err.reason, ServiceWriteFailure::Stale);
    assert_eq!(err.generation, 1, "should report old binding gen");

    let (tx, rx) = std::sync::mpsc::channel();
    let candidate = DropEffect {
        ctx: root.clone(),
        label: "drop-generation-candidate",
    };
    std::thread::spawn(move || {
        let res = writer.set(&provider_ctx, Arc::new(candidate));
        let _ = tx.send(res);
    });

    let result = rx.recv_timeout(Duration::from_secs(10));
    assert!(
        result.is_ok(),
        "set should not deadlock (recv_timeout means Mutex deadlock)"
    );
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.reason, ServiceWriteFailure::Stale);

    drop(view);
}
