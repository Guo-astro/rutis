//! The resource baseline of instances (#979): what one instance holds while
//! it runs, what closing many of them at once peaks at, and what is left
//! after they are closed — instances, fibers, service bindings, event
//! listeners, tasks, OS threads. The live allocations are counted with a
//! global allocator; `close_memory` was extended from the peak of #977 into
//! this baseline.
//!
//! Its own test binary: the counting allocator is global, and no other test
//! may allocate beside the one measured. Print the numbers with
//! `cargo test -p rutis-loader --release --test close_memory -- --nocapture`.
//!
//! Thresholds: see `assert_baseline` and the constants above it. They come
//! from measurements on a 32-core Linux machine (2026-11-02) and, for the
//! macOS bound, from the CI runner's numbers; the numbers of every run are
//! printed. Waits are scaled by `RUTIS_TEST_TIME_SCALE`; the thresholds are
//! allocation sizes and do not depend on machine speed.
//!
//! What one instance's footprint is made of, what was saved and what was
//! left alone, and what this test does not cover (language runtime processes,
//! service bindings inside instances) are in
//! `docs/performance-instance-footprint-2026-11-02.md`. The OS thread check
//! reads `/proc/self/status` and is Linux only; the other counts still apply
//! on macOS and Windows.

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
use rutis_test_support::time_scale;
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
    /// What is left of the fibers (`plugins`), of the service bindings, of
    /// the event listeners and of the tasks after closing, as `after - base`.
    residue: Counts,
}

/// What a root holds, for the residue check after closing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    plugins: isize,
    bindings: isize,
    status_listeners: isize,
    listeners: isize,
    tasks: isize,
    /// OS threads of the test process; 0 and not checked off Linux (see
    /// `os_threads`).
    threads: isize,
}

impl std::ops::Sub for Counts {
    type Output = Counts;

    fn sub(self, other: Counts) -> Counts {
        Counts {
            plugins: self.plugins - other.plugins,
            bindings: self.bindings - other.bindings,
            status_listeners: self.status_listeners - other.status_listeners,
            listeners: self.listeners - other.listeners,
            tasks: self.tasks - other.tasks,
            threads: self.threads - other.threads,
        }
    }
}

impl Counts {
    fn of(root: &Ctx) -> Counts {
        let diagnostics = root.diagnostics();
        let (status, listeners) = status_listeners(root);
        Counts {
            plugins: diagnostics.plugins.len() as isize,
            bindings: diagnostics.bindings.len() as isize,
            status_listeners: status as isize,
            listeners: listeners as isize,
            tasks: tokio::runtime::Handle::current()
                .metrics()
                .num_alive_tasks() as isize,
            threads: os_threads(),
        }
    }
}

/// The test process's OS threads, from `/proc/self/status` on Linux. The
/// loader starts no threads of its own; this catches a runtime (or a row)
/// leaving worker or blocking threads behind, which the task count cannot.
/// It is 0 elsewhere: the full runs on macOS and Windows skip this one
/// check, the rest of `Counts` still applies.
fn os_threads() -> isize {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .expect("/proc/self/status")
            .lines()
            .find_map(|line| line.strip_prefix("Threads:"))
            .and_then(|value| value.trim().parse().ok())
            .expect("Threads: in /proc/self/status")
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
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
/// drained (a debug build drains thousands of them for seconds). The bound
/// and how long it has to stay still grow with `RUTIS_TEST_TIME_SCALE`: a
/// loaded machine drains slower, and a real leak still fails in bounded
/// time.
async fn settle() {
    let scale = time_scale();
    let quiet = (10.0 * scale).ceil() as usize;
    let rounds = (30_000.0 * scale) as usize;
    let tasks = || {
        tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks()
    };
    let mut last = (tasks(), LIVE.load(Ordering::Relaxed));
    let mut still = 0;
    for _ in 0..rounds {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let now = (tasks(), LIVE.load(Ordering::Relaxed));
        still = if now.0 == last.0 && now.1.abs_diff(last.1) < 4096 {
            still + 1
        } else {
            0
        };
        if still == quiet {
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
    let base_counts = Counts::of(&root);

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
    let residue = Counts::of(&root) - base_counts;
    root.shutdown().await.unwrap();
    Sample {
        created,
        peak,
        settled,
        status_listeners: status,
        listeners,
        residue,
    }
}

/// The numbers of one run, total and per instance, for the PR and the logs.
fn report(title: &str, samples: &BTreeMap<usize, Sample>) {
    println!("{title}");
    println!(
        "N     created (per inst)   close peak   settled (per inst)  \
         status listeners  listeners  residue (fibers, bindings, status, listeners, tasks, threads)"
    );
    for (n, s) in samples {
        println!(
            "{n:<5} {:>10} B ({:>7} B) {:>12} B {:>9} B ({:>7} B) {:>17} {:>10}  {:?}",
            s.created,
            s.created / n,
            s.peak,
            s.settled,
            s.settled / *n as isize,
            s.status_listeners,
            s.listeners,
            s.residue
        );
    }
}

/// The most one instance may hold while it runs: its group fiber and the
/// eight rows. Measured on 2026-11-02: Linux 49.6-60.3 KiB per instance
/// (49.6-52.2 plain, 57.8-60.3 watch); macOS 68,234-70,772 B (66.6-69.1
/// KiB), where the allocator spends more per allocation. Inlining the
/// row-dispose branch again adds about 13.5 KiB per instance, so the bounds
/// sit below the rollback and above the normal worst on both platforms: on
/// Linux the cheapest rollback is 62.0 KiB (debug, N=40) and the bound is
/// 61.5 KiB; on macOS the rollback is about 84 KiB and the bound is 78 KiB.
/// A regression that removes that `Box` fails here as well as in the monitor
/// future's own size test.
#[cfg(target_os = "macos")]
const MAX_CREATED_PER_INSTANCE: usize = 78 * 1024;
#[cfg(not(target_os = "macos"))]
const MAX_CREATED_PER_INSTANCE: usize = 61 * 1024 + 512;

/// What closing may leave for the loader: measured 18-58 KiB after closing
/// 40 to 1000 instances, in debug and release. The bound is a fixed 128 KiB
/// (about twice the worst) plus 128 B per instance for a table that keeps a
/// slot per instance; unlike the running set, this must not grow with N.
/// Before the fix of #979 the loader kept the maps' peak capacity: 3.1-6 KiB
/// per instance (about 1.2 MB for 400, 6 MB for 1000).
fn max_settled(n: usize) -> isize {
    (128 * 1024 + 128 * n) as isize
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

/// One instance holds what the baseline allows, and closing them leaves
/// nothing behind: every fiber, service binding, event listener and task
/// the instances added is gone, and the live bytes do not grow with N.
fn assert_baseline(samples: &BTreeMap<usize, Sample>) {
    for (n, s) in samples {
        let per_instance = s.created / n;
        assert!(
            per_instance <= MAX_CREATED_PER_INSTANCE,
            "{n} instances hold {per_instance} B each, over the {} B baseline",
            MAX_CREATED_PER_INSTANCE
        );
        let allowed = max_settled(*n);
        assert!(
            s.settled <= allowed,
            "closing {n} instances left {} B ({:?}), over the {allowed} B baseline",
            s.settled,
            s.residue
        );
        assert_eq!(
            s.residue,
            Counts::default(),
            "closing {n} instances left fibers, bindings, listeners or tasks behind"
        );
    }
}

/// One instance of eight rows stays within the per-instance baseline of
/// live bytes; closing N at once peaks in proportion to N, also with a
/// `FiberStatusChanged` listener per row (8N listeners, as Dim's sessions on
/// rutis 0.17.1); and closing leaves no fiber, service binding, listener,
/// task or live bytes behind (#979).
///
/// guarantee: loader/instance-close-leaves-nothing
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instances_fit_the_resource_baseline() {
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
        assert_baseline(&samples);
    }
}
