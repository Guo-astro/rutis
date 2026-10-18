//! 终态释放(0.2.1)单元验证:瞬态子插件退出后,mount 记录与 root 级
//! provide 的清理记录、provided 记账在长寿 root 上回到基线。

use super::*;
use crate::{BoxFuture, CordisError, Effect, Event, Listener, Plugin};
use std::time::Duration;

struct Noop;

impl Plugin for Noop {
    fn name(&self) -> &str {
        "noop"
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

/// mount 记录经子 fiber 退出路径 drain 自摘,root 的 effects 列表
/// 不随插件 churn 累积。
#[tokio::test]
async fn churn_mount_records_release_from_root() {
    let ctx = Ctx::root().expect("runtime in scope");
    let root = ctx.weak_fiber().upgrade().expect("root alive");
    for _ in 0..25 {
        let view = ctx.plugin(Noop);
        view.dispose().await.unwrap();
        drop(view);
    }
    // 释放尾随 TaskDone(dispose 的 join 点),有界等待列表清空
    for _ in 0..5_000 {
        if root.effects.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        root.effects.lock().unwrap().is_empty(),
        "mount records must not accumulate on a long-lived root"
    );
}

/// root 级 provide 的 evict 清理记录与 provided 记账随 Disposer 释放。
#[tokio::test]
async fn churn_root_provides_release_accounting() {
    let ctx = Ctx::root().expect("runtime in scope");
    let root = ctx.weak_fiber().upgrade().expect("root alive");
    for i in 0..25 {
        let key = TypeKey::keyed_dynamic::<u32>(format!("svc/{i}"));
        let disposer = ctx.provide_as::<u32>(key, Arc::new(i)).unwrap();
        disposer.dispose().await.unwrap();
    }
    assert!(
        root.provided.lock().unwrap().is_empty(),
        "provided accounting must shrink per released binding"
    );
    assert!(
        root.effects.lock().unwrap().is_empty(),
        "evict cleanups must self-remove after drain"
    );
}

/// Stage a terminal task and Dispose intent that arrive after Shutdown was
/// enqueued. The public root `dispose()` can no longer do this (#234); the
/// test guards the driver's completion of such a task.
#[tokio::test]
async fn shutdown_completes_dispose_task_queued_behind_it() {
    let ctx = Ctx::root().unwrap();
    let root = ctx.weak_fiber().upgrade().unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    ctx.effect({
        let entered = entered.clone();
        let release = release.clone();
        move || {
            Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            }))
        }
    })
    .unwrap();

    let shutdown = ctx.shutdown();
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    let dispose_task = TransitionTask::new();
    root.transition.lock().unwrap().terminal_task = Some(dispose_task.clone());
    root.post(Intent::Dispose);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), shutdown)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), join_task(&dispose_task))
        .await
        .expect("dispose task stranded behind Shutdown")
        .unwrap();
}

/// #234: shutdown runs to completion between the moment a root `dispose()`
/// starts and the moment it would register. The dispose must join the
/// shutdown's result, not register on the stopped driver and get `Closed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_234_root_dispose_overtaken_by_shutdown_joins_its_result() {
    let ctx = Ctx::root().unwrap();
    let root = ctx.root_view().unwrap();
    let shutdown = Arc::new(Mutex::new(None));
    test_hooks::on_root_dispose({
        let ctx = ctx.clone();
        let shutdown = shutdown.clone();
        move || {
            let pending = ctx.shutdown();
            let handle = tokio::runtime::Handle::current();
            let result = tokio::task::block_in_place(|| handle.block_on(pending));
            *shutdown.lock().unwrap() = Some(result);
        }
    });
    let dispose = root.dispose();
    let shutdown = shutdown.lock().unwrap().take().expect("hook ran");
    assert!(shutdown.is_ok(), "shutdown: {shutdown:?}");
    let dispose = tokio::time::timeout(Duration::from_secs(10), dispose)
        .await
        .expect("dispose stalled");
    assert!(
        dispose.is_ok(),
        "dispose must share shutdown's Ok, got {dispose:?}"
    );
}

/// #234: a root dispose registered before shutdown closes admission and the
/// shutdown share one cleanup and the same result `Arc`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_234_root_dispose_before_shutdown_shares_one_cleanup() {
    let ctx = Ctx::root().unwrap();
    let root = ctx.root_view().unwrap();
    let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    ctx.effect({
        let cleanups = cleanups.clone();
        move || {
            Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    cleanups.fetch_add(1, Ordering::SeqCst);
                    Err(CordisError::PluginFailed("root cleanup failed".into()))
                })
            }))
        }
    })
    .unwrap();
    let dispose = root.dispose();
    let shutdown = ctx.shutdown();
    let (dispose, shutdown) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(dispose, shutdown)
    })
    .await
    .expect("dispose/shutdown stalled");
    let (dispose, shutdown) = (dispose.unwrap_err(), shutdown.unwrap_err());
    assert!(
        Arc::ptr_eq(&dispose, &shutdown),
        "{dispose:?} vs {shutdown:?}"
    );
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProbeEvent;
impl Event for ProbeEvent {
    const NAME: &'static str = "rutis-test-instance-churn";
    type Value = ();
}

fn own_key(ctx: &Ctx) -> crate::EventKey<ProbeEvent> {
    crate::EventKey::dynamic(format!("rutis-test-instance-churn/{}", ctx.instance()))
}

struct IdleListener;
impl Listener<ProbeEvent> for IdleListener {
    fn call<'a>(
        &'a self,
        _ctx: &'a Ctx,
        _event: &'a ProbeEvent,
        _: crate::Call<'a, ()>,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        Box::pin(async { Ok(None) })
    }
}

struct ChurnPlugin(Vec<TypeKey>);
impl Plugin for ChurnPlugin {
    fn name(&self) -> &str {
        "churn-plugin"
    }
    fn injects(&self) -> &[TypeKey] {
        &self.0
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            ctx.provide_as(TypeKey::instance::<u64>(ctx.instance()), Arc::new(1u64))?;
            ctx.events().on(ctx, &own_key(ctx), IdleListener)?;
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn thousand_shutdowns_reclaim_all_root_side_records() {
    let ctx = Ctx::root().unwrap();
    let root = ctx.weak_fiber().upgrade().unwrap();
    let service = ctx.provide(1u32).unwrap();
    for _ in 0..1000 {
        let view = ctx.plugin(ChurnPlugin(vec![TypeKey::of::<u32>()]));
        (&view).await.unwrap();
        view.shutdown().await.unwrap();
        assert_eq!(view.state().state, FiberState::Disposed);
        assert!(view.inner.driver.lock().unwrap().is_none());
        assert_eq!(root.children.lock().unwrap().len(), 0);
        assert_eq!(root.effects.lock().unwrap().len(), 1);
        assert_eq!(root.effect_index.lock().unwrap().len(), 1);
        assert!(view.inner.effect_index.lock().unwrap().is_empty());
        assert_eq!(ctx.shared().registry.table_counts(), (1, 0));
        assert_eq!(ctx.events().table_counts(), (0, 0, 0));
        drop(view);
    }
    service.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// [`ChurnPlugin`] that also emits on its instance key, so the bus keeps
/// a dispatch chain for it while the callback runs, and listens on a key
/// every instance shares, so one listener list holds all of them.
struct EmittingChurnPlugin(Vec<TypeKey>);
impl Plugin for EmittingChurnPlugin {
    fn name(&self) -> &str {
        "emitting-churn-plugin"
    }
    fn injects(&self) -> &[TypeKey] {
        &self.0
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            ctx.provide_as(TypeKey::instance::<u64>(ctx.instance()), Arc::new(1u64))?;
            let key = own_key(ctx);
            ctx.events().on(ctx, &key, IdleListener)?;
            ctx.events().on(ctx, &crate::EventKey::of(), IdleListener)?;
            ctx.events().emit(ctx, &key, Arc::new(ProbeEvent))?;
            Ok(Effect::Done)
        })
    }
}

/// Above this many slots a table shrinks once it is less than a quarter
/// full (the sparse-shrink rule of `fiber.rs`, `effect.rs`,
/// `registry.rs` and `bus.rs`).
const SPARSE_LIMIT: usize = 64;

/// Allocated slots of the tables a parent keeps per child, by name.
fn root_side_capacities(ctx: &Ctx, root: &Arc<FiberInner>) -> Vec<(&'static str, usize)> {
    let (bindings, injects) = ctx.shared().registry.table_capacities();
    let (names, entries, tails) = ctx.events().table_capacities();
    vec![
        ("children", root.children.lock().unwrap().capacity()),
        ("effects", root.effects.lock().unwrap().capacity()),
        ("effect_index", root.effect_index.lock().unwrap().capacity()),
        ("bindings", bindings),
        ("inject_index", injects),
        ("event_names", names),
        ("event_entries", entries),
        ("dispatch_tail", tails),
        // One key's list: the consumers of the shared service, the
        // listeners of the shared event key (#291).
        ("inject_list", ctx.shared().registry.max_list_capacity()),
        ("listener_list", ctx.events().max_list_capacity()),
    ]
}

/// The entries a parent keeps per child: children, mount records,
/// registry bindings and declarations, exact listener keys.
fn root_side_counts(ctx: &Ctx, root: &Arc<FiberInner>) -> (usize, usize, (usize, usize), usize) {
    (
        root.children.lock().unwrap().len(),
        root.effects.lock().unwrap().len(),
        ctx.shared().registry.table_counts(),
        ctx.events().table_counts().0,
    )
}

/// Batches of subtrees, each batch alive at once and then shut down, next
/// to a resident sibling: in every round the parent's tables grow past the
/// batch size, and after the batch is gone they are back to the resident's
/// entries and shrink below the sparse limit. Capacity does not keep what
/// earlier rounds needed. This holds for the tables and for the one list a
/// key keeps when every subtree uses that key next to the resident (#291).
///
/// guarantee: instance/subtrees.5
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_tables_shrink_back_after_every_batch_of_subtrees() {
    const BATCH: usize = 4 * SPARSE_LIMIT;
    const ROUNDS: usize = 8;
    let ctx = Ctx::root().unwrap();
    let root = ctx.weak_fiber().upgrade().unwrap();
    let service = ctx.provide(1u32).unwrap();
    let resident = ctx.plugin(EmittingChurnPlugin(vec![TypeKey::of::<u32>()]));
    (&resident).await.unwrap();
    let resident_generation = resident.state().generation;
    let baseline = root_side_counts(&ctx, &root);

    for round in 0..ROUNDS {
        let views: Vec<_> = (0..BATCH)
            .map(|_| ctx.plugin(EmittingChurnPlugin(vec![TypeKey::of::<u32>()])))
            .collect();
        for view in &views {
            view.await.unwrap();
        }
        let peak = root_side_capacities(&ctx, &root);
        for (table, capacity) in &peak {
            if matches!(
                *table,
                "children"
                    | "effects"
                    | "bindings"
                    | "event_names"
                    | "event_entries"
                    | "inject_list"
                    | "listener_list"
            ) {
                assert!(
                    *capacity >= BATCH,
                    "round {round}: {table} holds {capacity} < {BATCH}"
                );
            }
        }
        for view in views {
            view.shutdown().await.unwrap();
        }
        // Mount records leave the parent once each drain has finished.
        rutis_test_support::eventually_with(
            || (root_side_counts(&ctx, &root) == baseline).then_some(()),
            "entries back to the resident's",
            || format!("{:?}, baseline {baseline:?}", root_side_counts(&ctx, &root)),
        )
        .await;
        rutis_test_support::until(
            || ctx.events().table_counts().2 == 0,
            "dispatch chains finished",
        )
        .await;
        for (table, capacity) in root_side_capacities(&ctx, &root) {
            assert!(
                capacity <= SPARSE_LIMIT,
                "round {round}: {table} keeps {capacity} slots (peak {peak:?})"
            );
        }
    }
    let state = resident.state();
    assert_eq!(
        (state.state, state.generation),
        (FiberState::Active, resident_generation)
    );
    service.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}
