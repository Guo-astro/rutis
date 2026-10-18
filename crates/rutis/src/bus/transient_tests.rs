//! 终态释放:摘除监听后删除空条目,派发尾链任务完成后自摘——随实例 churn
//! 的名字与尾链不残留。

use super::*;
use crate::ctx::Ctx;
use crate::error::CordisError;
use crate::events::{Call, Event, Listener, SyncCall};
use crate::BoxFuture;
use std::time::Duration;

#[derive(serde::Serialize, serde::Deserialize)]
struct Ping;

impl Event for Ping {
    const NAME: &'static str = "test::TransientPing";
    type Value = ();
}

struct Capture(tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<Ctx>>>);
impl crate::Plugin for Capture {
    fn name(&self) -> &str {
        "capture-sync-context"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
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
        let key = crate::EventKey::<Ping>::dynamic(format!("room/{i}"));
        bus.on_sync(&ctx, &key, |_: &Ctx, _: &Ping, _: SyncCall<'_, ()>| {
            Ok(None)
        })
        .unwrap();
        bus.on_sync_pattern(
            &ctx,
            &crate::EventPattern::prefix("room/"),
            |_: &Ctx, _: crate::EventKey<Ping>, _: &Ping, call: SyncCall<'_, ()>| {
                if call.mode().is_waterfall() {
                    call.next()
                } else {
                    Ok(None)
                }
            },
        )
        .unwrap();
        bus.bail_sync(&ctx, &key, &Ping).unwrap();
        bus.waterfall_sync(&ctx, &key, &Ping, |_, _| Ok(None))
            .unwrap();
        view.shutdown().await.unwrap();
        assert_eq!(bus.table_counts().0, 0);
        assert_eq!(bus.table_counts().1, 0);
    }
    root.shutdown().await.unwrap();
}

struct Nop;

impl Listener<Ping> for Nop {
    fn call<'a>(
        &'a self,
        _ctx: &'a Ctx,
        _e: &'a Ping,
        _: Call<'a, ()>,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        Box::pin(async { Ok(None) })
    }
}

impl crate::PatternListener<Ping> for Nop {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: crate::EventKey<Ping>,
        _: &'a Ping,
        _: Call<'a, ()>,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test]
async fn thousand_prefix_entries_release_and_tails_prune() {
    let ctx = Ctx::root().unwrap();
    let bus = ctx.events().clone();
    for i in 0..1000 {
        let listener = bus
            .on_pattern(&ctx, &crate::EventPattern::prefix("ch/"), Nop)
            .unwrap();
        bus.emit(
            &ctx,
            &crate::EventKey::dynamic(format!("ch/{i}")),
            Arc::new(Ping),
        )
        .unwrap();
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
        let listener = bus
            .on::<Ping>(&ctx, &crate::EventKey::dynamic(name.clone()), Nop)
            .unwrap();
        bus.emit::<Ping>(
            &ctx,
            &crate::EventKey::dynamic(name.clone()),
            Arc::new(Ping),
        )
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
    let listener = bus
        .on::<Ping>(&ctx, &crate::EventKey::dynamic("ch/0"), Nop)
        .unwrap();
    bus.emit::<Ping>(&ctx, &crate::EventKey::dynamic("ch/0"), Arc::new(Ping))
        .expect("default event dispatch");
    listener.dispose().await.unwrap();
}

#[tokio::test]
async fn dispatch_observers_prune_after_repeated_registration() {
    let ctx = Ctx::root().unwrap();
    let bus = ctx.events().clone();
    for _ in 0..1000 {
        let observer = bus.observe_dispatch(&ctx, |_| {}).unwrap();
        assert_eq!(bus.observer_count(), 1);
        observer.dispose().await.unwrap();
        assert_eq!(bus.observer_count(), 0);
    }
}
