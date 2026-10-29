//! Release of bus records: removing the last listener deletes the empty
//! entry, and a dispatch tail removes itself when its task finishes, so
//! names and tails do not accumulate as instances come and go.

use super::*;
use crate::ctx::Ctx;
use crate::error::Error;
use crate::events::Event;
use crate::host::CtxHost;
use crate::internal::CtxInternal;
use crate::BoxFuture;
use std::time::Duration;

#[derive(serde::Serialize, serde::Deserialize)]
struct Ping;

impl Event for Ping {
    const NAME: &'static str = "test::TransientPing";
    type Value = ();
}

struct Capture(tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<Ctx>>>);
impl crate::DynPlugin for Capture {
    fn name(&self) -> &str {
        "capture-sync-context"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let sender = self.0.lock().await.take().unwrap();
            assert!(sender.send(ctx.clone()).is_ok());
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn thousand_subtrees_release_exact_and_prefix_entries() {
    let root = Ctx::root().unwrap();
    let bus = root.events().clone();
    for i in 0..1000 {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let view = root.plugin(Capture(tokio::sync::Mutex::new(Some(sender))));
        (&view).await.unwrap();
        let ctx = receiver.await.unwrap();
        let name = format!("room/{i}");
        ctx.listen::<Ping>()
            .name(name.clone())
            .on_sync(|_, _| Ok(None))
            .unwrap();
        ctx.listen::<Ping>()
            .prefix("room/")
            .on_sync(|_, _| Ok(None))
            .unwrap();
        ctx.named(name).bail(Ping).unwrap();
        view.shutdown().await.unwrap();
        assert_eq!(bus.table_counts().0, 0);
        assert_eq!(bus.table_counts().1, 0);
    }
    root.shutdown().await.unwrap();
}

/// A listener that does nothing.
fn nop(
    _: Arc<Ping>,
    _: crate::events::Call<Ping>,
) -> std::future::Ready<Result<Option<()>, Error>> {
    std::future::ready(Ok(None))
}

#[tokio::test]
async fn thousand_prefix_entries_release_and_tails_prune() {
    let ctx = Ctx::root().unwrap();
    let bus = ctx.events().clone();
    for i in 0..1000 {
        let listener = ctx.listen::<Ping>().prefix("ch/").on(nop).unwrap();
        ctx.named(format!("ch/{i}")).emit(Ping).unwrap();
        listener.dispose().await.unwrap();
        assert_eq!(bus.table_counts().0, 0);
    }
    rutis_test_support::until(|| bus.table_counts().2 == 0, "emit chains finished").await;
    ctx.shutdown().await.unwrap();
}

#[tokio::test]
async fn named_channels_and_dispatch_tails_prune() {
    let ctx = Ctx::root().expect("runtime in scope");
    let bus = ctx.events().clone();
    for i in 0..25 {
        let name = format!("ch/{i}");
        let listener = ctx.listen::<Ping>().name(name.clone()).on(nop).unwrap();
        ctx.named(name.clone())
            .emit(Ping)
            .expect("default event dispatch");
        listener.dispose().await.unwrap();
    }
    for _ in 0..5_000 {
        if bus.table_counts() == (0, 0, 0) && bus.backlogs().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        bus.table_counts(),
        (0, 0, 0),
        "names and tails must self-remove"
    );
    assert!(bus.backlogs().is_empty());
    // 摘除后再次注册/派发同名仍工作(条目按需重建)
    let listener = ctx.listen::<Ping>().name("ch/0").on(nop).unwrap();
    ctx.named("ch/0")
        .emit(Ping)
        .expect("default event dispatch");
    listener.dispose().await.unwrap();
}

/// guarantee: events/emit
/// An emit on a runtime shutting down: `spawn` drops its task's future at
/// once. What the emit recorded (its backlog entry, its name's chain) goes
/// with it instead of staying forever.
#[test]
fn an_emit_dropped_by_a_runtime_shutting_down_leaves_no_backlog() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let ctx = Ctx::root_with(runtime.handle().clone());
    let bus = ctx.events().clone();
    let listener = runtime.block_on(async { ctx.listen::<Ping>().name("gone/1").on(nop).unwrap() });
    // The root outlives its driver: its listener is still live.
    let root = ctx.root_view().unwrap();
    runtime.shutdown_timeout(Duration::from_secs(1));
    assert_eq!(bus.subscriptions().len(), 1);
    ctx.named("gone/1").emit(Ping).unwrap();
    assert!(bus.backlogs().is_empty(), "{:?}", bus.backlogs());
    std::mem::forget(root);
    assert_eq!(
        bus.table_counts().2,
        0,
        "no chain of a task that never runs"
    );
    std::mem::forget(listener);
    std::mem::forget(ctx);
}
