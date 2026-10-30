//! Closing many instances at once (#977): the live allocations while they
//! close stay in proportion to the instances, not to their square.
//!
//! Its own test binary: the counting allocator is global, and no other test
//! may allocate beside the one measured. Print the numbers with
//! `cargo test -p rutis-loader --release --test close_memory -- --nocapture`.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::{patches, MemStore};
use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::Ctx;
use rutis::Event;
use rutis_loader::edit::Editable;
use rutis_loader::{Builtins, Layer, LoaderOptions, LoaderPlugin};
use serde_json::Value;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
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
                let grow = new_size - layout.size();
                let live = LIVE.fetch_add(grow, Ordering::Relaxed) + grow;
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// A row that does nothing.
struct Plain;

impl rutis::Plugin for Plain {
    type Deps = ();

    fn name(&self) -> &str {
        "plain"
    }

    async fn apply(&self, _ctx: &Ctx, (): ()) -> rutis::Result {
        Ok(())
    }
}

/// A row that listens to every fiber's status, as the loader did for each
/// row in 0.17.1 and a host's per-session code may.
struct Watch;

impl rutis::Plugin for Watch {
    type Deps = ();

    fn name(&self) -> &str {
        "watch"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.on_sync::<rutis::host::FiberStatusChanged>(|_, _| Ok(None))?;
        Ok(())
    }
}

/// Rows of one instance. Like a Dim session on rutis 0.17.1, where the
/// loader followed each row with a `FiberStatusChanged` listener of its
/// own: eight rows, each closing with several status changes.
const ROWS: usize = 8;

/// The two numbers of instances. A debug build takes ten times less: each
/// queued emit checks every listener, which is slow there (about two
/// minutes at 1000).
const SIZES: [usize; 2] = if cfg!(debug_assertions) {
    [40, 400]
} else {
    [100, 1000]
};

/// An instanced group of [`ROWS`] rows of plugin `row`.
fn doc(row: &str) -> String {
    let rows: Vec<String> = (0..ROWS)
        .map(|i| format!(r#"{{ "id": "r{i}", "name": "{row}" }}"#))
        .collect();
    format!(
        r#"[{{ "insert": [
    {{ "id": "doc", "group": true, "instanced": true, "config": [ {} ] }}
] }}]"#,
        rows.join(", ")
    )
}

struct Sample {
    /// Live bytes with the N instances running, above the empty loader.
    created: usize,
    /// The peak while they all close, above the empty loader.
    peak: usize,
    /// Live bytes once closing settled, above the empty loader.
    settled: isize,
    /// `FiberStatusChanged` listeners with the N instances running.
    status_listeners: usize,
    /// Every listener with the N instances running.
    listeners: usize,
}

fn status_listeners(root: &Ctx) -> (usize, usize) {
    let all = root.events().subscriptions();
    let status = all
        .iter()
        .filter(|entry| entry.name == rutis::host::FiberStatusChanged::NAME)
        .count();
    (status, all.len())
}

/// Until the tasks and the live bytes stop moving: queued deliveries have
/// drained (a debug build drains thousands of them for seconds).
async fn settle() {
    let tasks = || {
        tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks()
    };
    let mut last = (tasks(), LIVE.load(Ordering::Relaxed));
    let mut still = 0;
    for _ in 0..30_000 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let now = (tasks(), LIVE.load(Ordering::Relaxed));
        still = if now.0 == last.0 && now.1.abs_diff(last.1) < 4096 {
            still + 1
        } else {
            0
        };
        if still == 10 {
            return;
        }
        last = now;
    }
}

async fn measure(n: usize, last: &str) -> Sample {
    let mut builtins = Builtins::new();
    builtins.register::<Value, _, _>("plain", |_| Ok(Plain));
    builtins.register::<Value, _, _>("watch", |_| Ok(Watch));
    let store = MemStore::default();
    let root = Ctx::root().unwrap();
    let plugin = LoaderPlugin::new(
        builtins,
        LoaderOptions {
            persist: Arc::new(store.clone()),
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let report = loader
        .reconcile(
            vec![
                Layer::new("base", patches(serde_json::from_str(&doc(last)).unwrap())),
                Layer::new("user", store.patches()),
            ],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    settle().await;
    let base = LIVE.load(Ordering::Relaxed);

    let mut instances = Vec::with_capacity(n);
    for _ in 0..n {
        instances.push(loader.create_instance(&root, "doc").await.unwrap().plugin);
    }
    settle().await;
    let (status, listeners) = status_listeners(&root);
    let created = LIVE.load(Ordering::Relaxed).saturating_sub(base);

    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let closing: Vec<_> = instances
        .iter()
        .map(|plugin| {
            let loader = loader.clone();
            let plugin = *plugin;
            tokio::spawn(async move { loader.remove_instance(plugin).await })
        })
        .collect();
    for close in closing {
        close.await.unwrap().unwrap();
    }
    drop(instances);
    settle().await;
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    let settled = LIVE.load(Ordering::Relaxed) as isize - base as isize;
    root.shutdown().await.unwrap();
    Sample {
        created,
        peak,
        settled,
        status_listeners: status,
        listeners,
    }
}

fn report(title: &str, samples: &BTreeMap<usize, Sample>) {
    println!("{title}");
    println!("N     created      close peak   settled    status listeners  listeners");
    for (n, s) in samples {
        println!(
            "{n:<5} {:>10} B {:>12} B {:>9} B {:>17} {:>10}",
            s.created, s.peak, s.settled, s.status_listeners, s.listeners
        );
    }
}

/// Closing adds little above what the running instances hold, and what it
/// adds grows about ten times for ten times the instances. Before the fix
/// of #977, with a listener per row: 819 MB at the peak against 82 MB for
/// 1000 running instances, and 93 times as much added for 10 times the
/// instances; after it, 87 MB and about 8 times (release build).
fn assert_proportional(samples: &BTreeMap<usize, Sample>) {
    let (small, large) = (&samples[&SIZES[0]], &samples[&SIZES[1]]);
    assert!(
        large.peak * 2 < large.created * 3,
        "closing {} instances peaked at {} B, against {} B for them running",
        SIZES[1],
        large.peak,
        large.created
    );
    let added = |s: &Sample| s.peak.saturating_sub(s.created).max(1) as f64;
    let ratio = added(large) / added(small);
    assert!(
        ratio < 20.0,
        "closing added {ratio:.1} times as much for 10 times the instances \
         ({} B for {}, {} B for {})",
        added(small),
        SIZES[0],
        added(large),
        SIZES[1],
    );
}

/// Closing N instances together peaks in proportion to N, also with a
/// `FiberStatusChanged` listener per row (8N listeners, as Dim's sessions
/// on rutis 0.17.1): the queued emits do not each copy a listener list
/// that grows with N.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_many_instances_peaks_in_proportion() {
    for (title, last) in [
        ("plain rows (the loader's one listener)", "plain"),
        ("a FiberStatusChanged listener per row", "watch"),
    ] {
        let mut samples = BTreeMap::new();
        for n in SIZES {
            samples.insert(n, measure(n, last).await);
        }
        report(title, &samples);
        assert_proportional(&samples);
    }
}
