use super::*;
use crate::host::CtxHost;

/// A plugin with one listener: the shape a session's row has.
struct Listening;

impl crate::Plugin for Listening {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, (): ()) -> crate::Result {
        ctx.on_sync::<crate::host::FiberStatusChanged>(|_, _| Ok(None))?;
        Ok(())
    }
}

/// The event the tests below dispatch.
#[derive(serde::Serialize, serde::Deserialize)]
struct Ping;

impl crate::Event for Ping {
    const NAME: &'static str = "t/markers";
    type Value = u32;
}

/// The state word a listener call reads is a copy of the four markers the
/// rest of the kernel reads (#1125): a load, a restart, a dispose and the
/// root's close write both.
#[tokio::test]
async fn the_word_matches_the_markers() {
    let root = Ctx::root().unwrap();
    // Held here: the root's fiber is gone once its close finished.
    let root_inner = root.root_view().expect("the root's view").inner;
    let view = root.plugin(Listening);
    view.running(Duration::from_secs(30)).await.unwrap();
    check(&view.inner);
    check(&root_inner);

    view.restart().await.unwrap();
    check(&view.inner);

    view.dispose().await.unwrap();
    check(&view.inner);

    root.shutdown().await.unwrap();
    check(&root_inner);
}

#[cfg(debug_assertions)]
fn check(fiber: &Arc<FiberInner>) {
    fiber.markers_agree().unwrap();
}

#[cfg(not(debug_assertions))]
fn check(_: &Arc<FiberInner>) {}

/// A listener registered by an earlier generation is skipped once the
/// next one is published, and the count its owner keeps is released with
/// the dispatch (#1125).
#[tokio::test]
async fn an_old_generation_listener_is_not_called() {
    let root = Ctx::root().unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let heard = calls.clone();
    root.on_sync::<Ping>(move |_, _| {
        heard.fetch_add(1, Ordering::SeqCst);
        Ok(Some(1))
    })
    .unwrap();
    assert_eq!(root.bail(Ping).unwrap(), Some(1));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // What a reload publishes: the next generation is live, so listeners
    // of the old one are skipped. The effects of that generation are
    // withdrawn as well; this pins the word's own check.
    let inner = root.root_view().unwrap().inner;
    inner.live_generation.store(1, Ordering::SeqCst);
    inner.state.publish_live(1);
    check(&inner);
    assert!(root.bail(Ping).is_ok());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a listener of the old generation was called"
    );
    assert_eq!(inner.flights.count(), 0, "a flight outlived its dispatch");
}

/// One dispatch counts its owner once, however many of its listeners it
/// calls (#1125).
#[tokio::test]
async fn one_dispatch_counts_an_owner_once() {
    let root = Ctx::root().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    for _ in 0..2 {
        let seen = seen.clone();
        root.on_sync::<Ping>(move |_, call: crate::events::SyncCall<'_, Ping>| {
            let fiber = call.ctx().weak_fiber().upgrade().expect("the owner");
            seen.lock().unwrap().push(fiber.flights.count());
            Ok(None)
        })
        .unwrap();
    }
    root.bail(Ping).unwrap();
    assert_eq!(*seen.lock().unwrap(), [1, 1], "one count per dispatch");
    let fiber = root.root_view().unwrap().inner;
    assert_eq!(fiber.flights.count(), 0, "the dispatch released its count");
}
