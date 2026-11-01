//! The emits queued on one name share the listeners they dispatch over: N of
//! them queued up hold one snapshot, not N copies of a list that grows with the
//! registrations between them (#980).
//!
//! Its own test binary: the counting allocator is global, and no other test
//! may allocate beside the one measured. Print the numbers with
//! `cargo test -p rutis-loader --release --test queued_emits_memory -- --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis::Event;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            if new_size >= layout.size() {
                LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(serde::Serialize, serde::Deserialize)]
struct Tick;
impl Event for Tick {
    const NAME: &'static str = "queued/tick";
    type Value = ();
}

/// How many emits the test queues up beside the one its listener holds.
const QUEUED: [usize; 2] = [512, 1024];

/// What one queued emit may hold: its snapshot is the list the first emit
/// built plus the head of the chain of what was registered since, whether that
/// chain is one entry or a thousand.
const PER_EMIT: usize = 4096;

/// Waits until `done` holds; the timeout only guards against a hang.
async fn until(done: impl Fn() -> bool, what: &str) {
    tokio::time::timeout(Duration::from_secs(120), async {
        while !done() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"));
}

/// One round: the listeners held by the first emit, then `queued` more emits,
/// each after one more registration. Returns the bytes held while they wait.
///
/// The test waits on the barrier from its own thread, which blocks a thread of
/// the runtime, so there are four.
async fn round(queued: usize) -> usize {
    let root = Ctx::root().unwrap();
    let gate = Arc::new(Barrier::new(2));
    let entered = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    root.listen::<Tick>()
        .on({
            let gate = gate.clone();
            let entered = entered.clone();
            let held = held.clone();
            move |_, _| {
                if !held.swap(true, Ordering::SeqCst) {
                    entered.store(true, Ordering::SeqCst);
                    gate.wait();
                }
                std::future::ready(Ok(None))
            }
        })
        .unwrap();
    let base = LIVE.load(Ordering::Relaxed);
    root.emit(Tick).unwrap();
    until(
        || entered.load(Ordering::SeqCst),
        "the emit is in its listener",
    )
    .await;
    for _ in 0..queued {
        root.on_sync::<Tick>(|_, _| Ok(None)).unwrap();
        root.emit(Tick).unwrap();
    }
    let held_bytes = LIVE.load(Ordering::Relaxed).saturating_sub(base);
    gate.wait();
    until(
        || root.diagnostics().event_backlogs.is_empty(),
        "every call started",
    )
    .await;
    root.shutdown().await.unwrap();
    held_bytes
}

/// Twice the queued emits hold about twice as much: each keeps the head of the
/// chain, not a list of the listeners registered so far. A copy made per emit
/// holds one entry more than the one before, so twice the emits hold four times
/// as much (measured with such a copy: 6.2 MB for 1024 against 1.9 MB for the
/// head).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_emits_share_the_list_of_listeners() {
    let mut held = Vec::new();
    for queued in QUEUED {
        let bytes = round(queued).await;
        println!(
            "{queued} queued emits hold {bytes} B ({} B each)",
            bytes / queued
        );
        assert!(
            bytes < queued * PER_EMIT,
            "{queued} queued emits hold {bytes} B, more than {} B",
            queued * PER_EMIT
        );
        held.push(bytes);
    }
    let ratio = held[1] as f64 / held[0].max(1) as f64;
    assert!(
        ratio < 3.0,
        "twice the queued emits held {ratio:.1} times as much ({} B against {} B)",
        held[1],
        held[0]
    );
}
