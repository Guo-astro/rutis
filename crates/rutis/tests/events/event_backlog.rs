//! Accepted emits whose calls have not all finished are visible in the
//! runtime diagnostics, per name.

use std::sync::Arc;
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::{Ctx, Event};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

#[derive(Serialize, Deserialize)]
struct Job;
impl Event for Job {
    const NAME: &'static str = "backlog/job";
    type Value = ();
}

/// Handles one event per permit.
fn slow(ctx: &Ctx, name: &str, permits: &Arc<Semaphore>) {
    let permits = permits.clone();
    ctx.listen::<Job>()
        .name(name)
        .on(move |_, _| {
            let permits = permits.clone();
            async move {
                permits.acquire().await.unwrap().forget();
                Ok(None)
            }
        })
        .unwrap();
}

fn pending(ctx: &Ctx, name: &str) -> Option<(usize, Duration)> {
    ctx.diagnostics()
        .event_backlogs
        .into_iter()
        .find(|backlog| backlog.name == name)
        .map(|backlog| (backlog.pending, backlog.oldest))
}

/// Since #464 the calls of one name's emits run concurrently (they only
/// start in emit order); an emit stays in the backlog until every call it
/// started finished.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emits_with_unfinished_calls_are_visible() {
    let ctx = Ctx::root().unwrap();
    let permits = Arc::new(Semaphore::new(0));
    let (busy, idle) = ("busy", "idle");
    slow(&ctx, busy, &permits);
    slow(&ctx, idle, &permits);

    for _ in 0..3 {
        ctx.named(busy).emit(Job).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(20)).await; // only lets the oldest age
    let (count, oldest) = pending(&ctx, busy).expect("busy name has a backlog");
    assert_eq!(count, 3);
    assert!(oldest >= Duration::from_millis(20), "{oldest:?}");
    // A name with no accepted emits has no entry.
    assert_eq!(pending(&ctx, idle), None);

    // Each handled event leaves the backlog; the entry goes once it is empty.
    permits.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        while pending(&ctx, busy).map(|(count, _)| count) != Some(2) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("one event handled");
    permits.add_permits(2);
    tokio::time::timeout(Duration::from_secs(10), async {
        while pending(&ctx, busy).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("backlog drained");
    ctx.shutdown().await.unwrap();
}
