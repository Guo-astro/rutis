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

/// A row that listens to every fiber's status, as a host's per-session
/// code may: one more `FiberStatusChanged` listener per instance.
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

/// An instanced group of four rows; the last is `last`.
fn doc(last: &str) -> String {
    format!(
        r#"[{{ "insert": [
    {{ "id": "doc", "group": true, "instanced": true, "config": [
        {{ "id": "a", "name": "plain" }},
        {{ "id": "b", "name": "plain" }},
        {{ "id": "c", "name": "plain" }},
        {{ "id": "d", "name": "{last}" }}
    ] }}
] }}]"#
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

/// Until the live bytes stop moving: spawned deliveries have drained.
async fn settle() {
    let mut last = LIVE.load(Ordering::Relaxed);
    let mut still = 0;
    for _ in 0..1000 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let live = LIVE.load(Ordering::Relaxed);
        still = if live.abs_diff(last) < 4096 {
            still + 1
        } else {
            0
        };
        if still == 10 {
            return;
        }
        last = live;
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

/// What closing adds above the running instances grows about ten times
/// for ten times the instances (a hundred times when quadratic; before the
/// fix of #977 it was 35 times with one listener per instance).
fn assert_proportional(samples: &BTreeMap<usize, Sample>) {
    let (small, large) = (&samples[&100], &samples[&1000]);
    let added = |s: &Sample| s.peak.saturating_sub(s.created).max(1) as f64;
    let ratio = added(large) / added(small);
    assert!(
        ratio < 20.0,
        "closing added {ratio:.1} times as much for 10 times the instances \
         ({} B for 100, {} B for 1000)",
        added(small),
        added(large),
    );
}

/// Closing N instances together peaks in proportion to N, whether the
/// instances add `FiberStatusChanged` listeners or not: a dispatch does not
/// copy a listener list that grows with N.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_many_instances_peaks_in_proportion() {
    for (title, last) in [
        ("plain rows (the loader's one listener)", "plain"),
        ("one FiberStatusChanged listener per instance", "watch"),
    ] {
        let mut samples = BTreeMap::new();
        for n in [100, 1000] {
            samples.insert(n, measure(n, last).await);
        }
        report(title, &samples);
        assert_proportional(&samples);
    }
}
