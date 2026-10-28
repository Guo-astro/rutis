//! `ServiceWriter`: the provider's update of a binding made with
//! `provide_mut_at` (bridge projections use it).

use std::future::IntoFuture;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, Key, TypeKey};
use rutis::internal::{CtxInternal, ServiceWriteFailure, ServiceWriter};
use rutis::{Ctx, Error};

struct Capture {
    injects: Vec<TypeKey>,
    slot: Arc<Mutex<Option<Ctx>>>,
}

impl DynPlugin for Capture {
    fn name(&self) -> &str {
        "service-writer-capture"
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
async fn mutable_writer_respects_owner_and_generation() {
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

    writer.set(&root, Arc::new(3)).unwrap();
    assert_eq!(*reader.require_at(&key).unwrap(), 3);

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
