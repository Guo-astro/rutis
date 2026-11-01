//! Registering listeners while an `emit` of their name holds the list it
//! dispatches over costs one append each: the selection asks no listener
//! whether it is live, and the emit that follows dispatches to all of them
//! (#980).
//!
//! Its own test binary: it reads a counter of this process's selections.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::{Ctx, Event};

#[derive(serde::Serialize, serde::Deserialize)]
struct Tick;
impl Event for Tick {
    const NAME: &'static str = "scale/tick";
    type Value = ();
}

fn selection_checks(root: &Ctx) -> u64 {
    root.events().selection_checks_for_testing()
}

/// Waits until `done` holds; the timeout only guards against a hang.
async fn until(done: impl Fn() -> bool, what: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !done() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"));
}

/// The list an `emit` dispatches over lives as long as that emit: the first
/// call of the listener below waits inside it, so the test registers the
/// others while the list is held.
///
/// The test waits on the barrier from its own thread, which blocks a thread of
/// the runtime, so there are four.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registering_while_an_emit_holds_the_list_asks_no_listener() {
    const ADDED: usize = 4096;
    let root = Ctx::root().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Barrier::new(2));
    let entered = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    root.listen::<Tick>()
        .on({
            let calls = calls.clone();
            let gate = gate.clone();
            let entered = entered.clone();
            let held = held.clone();
            move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                if !held.swap(true, Ordering::SeqCst) {
                    entered.store(true, Ordering::SeqCst);
                    gate.wait();
                }
                std::future::ready(Ok(None))
            }
        })
        .unwrap();
    root.emit(Tick).unwrap();
    until(
        || entered.load(Ordering::SeqCst),
        "the emit is in its listener",
    )
    .await;

    // The emit above still holds the list: every registration below appends to
    // it. A list rebuilt for every registration (or every 32 of them) would
    // ask all the listeners registered so far whether they are live.
    let before = selection_checks(&root);
    for i in 0..ADDED {
        if i % 32 == 0 {
            let asked = selection_checks(&root) - before;
            assert_eq!(
                asked, 0,
                "registering asked {asked} listeners whether they are live after {i} of {ADDED}"
            );
        }
        let calls = calls.clone();
        root.on_sync::<Tick>(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        })
        .unwrap();
    }
    let asked = selection_checks(&root) - before;
    assert_eq!(
        asked, 0,
        "registering {ADDED} listeners asked {asked} listeners whether they are live"
    );

    // The emit that follows dispatches to all of them: its selection is the
    // list the first emit holds plus the appends, so it asks one listener
    // whether it is live (the first), not every listener.
    root.emit(Tick).unwrap();
    let asked = selection_checks(&root) - before;
    assert!(
        asked <= 1,
        "selecting the {ADDED} appends asked {asked} listeners whether they are live"
    );

    gate.wait();
    until(
        || calls.load(Ordering::SeqCst) == ADDED + 2,
        "every call started",
    )
    .await;
    root.shutdown().await.unwrap();
}
