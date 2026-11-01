//! The CPU time of an instance's life (#980): creating N instances of eight
//! rows, each row with a `FiberStatusChanged` listener of its own (8N
//! listeners on one name, as a Dim session had on rutis 0.17.1), closing them
//! together, and what an idle host costs — no call, no event queued.
//!
//! Its own test binary: the numbers are the process's, and no other test may
//! run beside the measured one. Print them with
//! `cargo test -p rutis-loader --release --test close_cpu -- --nocapture`.
//!
//! Unix only: the numbers are the process's CPU time, which the tests read
//! with `clock_gettime`; Windows has no such reading here (wall time cannot
//! tell an idle host from a busy one), and the baseline runs on the Linux
//! runners.
#![cfg(unix)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{patches, MemStore};
use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::Ctx;
use rutis_loader::edit::Editable;
use rutis_loader::{Builtins, Layer, Loader, LoaderOptions, LoaderPlugin};
use serde_json::Value;

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

/// A row that listens to every fiber's status: what a host's per-session
/// code does, and what the loader did for each row in 0.17.1.
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

/// Rows of one instance.
const ROWS: usize = 8;

/// The two numbers of instances: ten times apart, so the CPU of ten times the
/// instances can be held against the CPU of one tenth of them. A debug build
/// takes ten times less (as `close_memory`).
const SIZES: [usize; 2] = if cfg!(debug_assertions) {
    [40, 400]
} else {
    [100, 1000]
};

/// How long the host is left alone to see what it costs: nothing is called
/// and no event is queued in this window.
const IDLE: Duration = Duration::from_secs(1);

/// At most this share of the idle window may be CPU: the workers are parked
/// while nothing happens, which measured under 1%.
const IDLE_SHARE: f64 = 0.05;

/// Ten times the instances may take at most this much of the CPU of one tenth
/// of them, creating and closing.
///
/// Ten times is what the work alone would be. These numbers are more, because
/// every status change reaches every listener: a listener per row means the
/// calls of one instance's status changes grow with the instances too, so ten
/// times the instances is about twelve times the CPU creating (measured 11.9
/// in release and 11.8 in debug; before #980, which rebuilt the list of
/// listeners per registration and asked every listener whether it is live for
/// every emit, 48.6 and 33.9). Closing grows more than that, 14 to 34 times
/// whatever the listeners (measured before and after #980; the loader looks
/// the copy up and rebuilds its tables per instance). The thresholds hold
/// those numbers against getting worse.
const CREATE_RATIO: f64 = 30.0;
const CLOSE_RATIO: f64 = 60.0;

/// The CPU time this process used: what its threads spent, not how long it
/// took, so a machine that runs other work beside the tests, or has fewer
/// cores, gives the same number.
fn cpu() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let ok = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    assert_eq!(ok, 0, "clock_gettime");
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

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

/// An instance of the workload, its two numbers and the two idle windows.
struct Sample {
    /// CPU of creating the N instances, a listener per row.
    created: Duration,
    /// CPU of closing them all at once.
    closed: Duration,
    /// CPU of the idle window with N instances running.
    idle_with: Duration,
    /// CPU of the idle window once they closed.
    idle_after: Duration,
    /// The window the idle numbers were measured over.
    idle_window: Duration,
}

/// A root and a loader with the `doc` group of plugin `row`, ready to create
/// instances of it.
async fn host(row: &str) -> (Ctx, Loader) {
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
                Layer::new("base", patches(serde_json::from_str(&doc(row)).unwrap())),
                Layer::new("user", store.patches()),
            ],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    (root, loader)
}

/// The events queued on the bus: the status changes of the fibers created or
/// closed, until every call they start has started.
fn queued(root: &Ctx) -> usize {
    root.diagnostics()
        .event_backlogs
        .iter()
        .map(|backlog| backlog.pending)
        .sum()
}

/// Waits until the bus is quiet, and fails when it is not: an idle window is
/// worth nothing while events are still being dispatched.
async fn drained(root: &Ctx) {
    let start = Instant::now();
    let mut quiet = 0;
    while start.elapsed() < Duration::from_secs(120) {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if queued(root) == 0 {
            quiet += 1;
            if quiet == 5 {
                return;
            }
        } else {
            quiet = 0;
        }
    }
    panic!("the bus did not go quiet: {} events queued", queued(root));
}

/// Nothing is called and no event is queued for [`IDLE`]: how much CPU the
/// process uses anyway, and the window it was measured over.
async fn idle(root: &Ctx) -> (Duration, Duration) {
    drained(root).await;
    let start = cpu();
    let wall = Instant::now();
    tokio::time::sleep(IDLE).await;
    (cpu() - start, wall.elapsed())
}

/// One round of the fixture: the host, N instances with a listener per row,
/// and the two idle windows.
async fn measure(n: usize) -> Sample {
    let (root, loader) = host("watch").await;
    // The listeners of the loader itself: what the instances must not add to.
    let listening = root.events().subscriptions().len();
    let start = cpu();
    let mut instances = Vec::with_capacity(n);
    for _ in 0..n {
        instances.push(loader.create_instance(&root, "doc").await.unwrap().plugin);
    }
    let created = cpu() - start;
    let (idle_with, idle_window) = idle(&root).await;

    let start = cpu();
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
    let closed = cpu() - start;
    let (idle_after, _) = idle(&root).await;
    assert_eq!(
        root.events().subscriptions().len(),
        listening,
        "listeners left behind"
    );
    root.shutdown().await.unwrap();
    Sample {
        created,
        closed,
        idle_with,
        idle_after,
        idle_window,
    }
}

fn report(samples: &[(usize, Sample)]) {
    println!(
        "instances  create CPU  per instance  close CPU  per instance  idle running  idle closed"
    );
    for (n, s) in samples {
        println!(
            "{n:<10} {:>10?} {:>13?} {:>10?} {:>13?} {:>13?} {:>13?}",
            s.created,
            s.created / *n as u32,
            s.closed,
            s.closed / *n as u32,
            s.idle_with,
            s.idle_after,
        );
    }
}

/// Creating and closing N instances costs CPU in proportion to N, and an idle
/// host uses none of it: no thread spins while nothing is called and no event
/// is queued.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instances_fit_the_cpu_baseline() {
    let mut samples = Vec::new();
    for n in SIZES {
        samples.push((n, measure(n).await));
    }
    report(&samples);
    let (small_n, small) = &samples[0];
    let (large_n, large) = &samples[1];
    let times = large_n / small_n;
    for (what, ratio_limit, small, large) in [
        ("creating", CREATE_RATIO, small.created, large.created),
        ("closing", CLOSE_RATIO, small.closed, large.closed),
    ] {
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(
            ratio < ratio_limit,
            "{what} {large_n} instances took {ratio:.1} times the CPU of {small_n} \
             ({large:?} against {small:?}), {times} times the instances"
        );
    }
    for (what, idle) in [
        ("with the instances running", large.idle_with),
        ("once they closed", large.idle_after),
    ] {
        let share = idle.as_secs_f64() / large.idle_window.as_secs_f64();
        assert!(
            share < IDLE_SHARE,
            "an idle host {what} used {share:.1}% of {:?} ({idle:?})",
            large.idle_window
        );
    }
}
