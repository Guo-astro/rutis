//! Shuttle model of a binary runtime's restart (#627): a restart takes the
//! old runtime out of the slots, stops it and waits for its process, and
//! only then installs the new one — one name, one process (#307).
//!
//! What runs is the production [`Inner::restart_now`], [`stop`],
//! [`Inner::install`] and [`Busy`]. What stands in for the rest is named
//! where it is installed:
//!
//! - the process is a [`Processes`] a test ends by hand (`Processes::held`),
//!   and the binary is a file a test writes, read through the resolver's
//!   test table, so no binary runs;
//! - the fiber's dispose in [`stop`] is written out under
//!   `--cfg rutis_shuttle` (the kernel's dispose is #622's own subject);
//!   the process wait after it is the code this model checks;
//! - the runtime plugin's `apply`, which starts the real process, is not
//!   driven: the model observes the slot installed, not the process it
//!   would start.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-loader --lib \
//!     --features go,swift --target-dir target/shuttle shuttle
//! ```
//!
//! `RUTIS_SHUTTLE_ITERATIONS` sets the iterations of each scheduler: 1000
//! by default, as on pull requests; the nightly run (`stress.yml`) uses a
//! million. A failure prints its schedule; `RUTIS_SHUTTLE_REPLAY=<schedule>`
//! replays it, with the failing test's name as the filter.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::{Ctx, Error};
use rutis_bridge::pipe::Processes;
use rutis_bridge::runtime::{Launcher, RuntimeStates};
use shuttle::future::block_on;
use shuttle::sync::atomic::{AtomicBool, Ordering};
use shuttle::thread;

use super::{
    stamp, Binaries, BinaryLanguage, BinaryManifest, BinaryResolver, BinaryRuntimes, Busy, Inner,
    Slot, Spawn, Spawned,
};

/// Iterations of each scheduler: `RUTIS_SHUTTLE_ITERATIONS`, or 1000.
fn iterations() -> usize {
    std::env::var("RUTIS_SHUTTLE_ITERATIONS")
        .ok()
        .map(|value| value.parse().expect("RUTIS_SHUTTLE_ITERATIONS is a number"))
        .unwrap_or(1000)
}

/// Explore `model` with shuttle's random scheduler and with PCT (which
/// finds bugs that need a few preemptions at the right places with better
/// odds than uniform choices), or replay `RUTIS_SHUTTLE_REPLAY`.
fn explore<F>(model: F)
where
    F: Fn() + Send + Sync + 'static,
{
    if let Ok(schedule) = std::env::var("RUTIS_SHUTTLE_REPLAY") {
        shuttle::replay(model, schedule.trim().trim_matches('"'));
        return;
    }
    let model = Arc::new(model);
    let iterations = iterations();
    let random = model.clone();
    shuttle::check_random(move || random(), iterations);
    shuttle::check_pct(move || model(), iterations, 3);
}

/// A compiled language of the model: its binaries are never run.
struct Recorded;

impl BinaryLanguage for Recorded {
    const PREFIX: &'static str = "rec";
    const TITLE: &'static str = "Record";
    const MARKER: &'static [u8] = b"rutis-recorded-runtime:1";
    const PLUGIN_API: u64 = 1;

    fn runtime_name(path: &Path) -> String {
        super::runtime_name(Self::PREFIX, path)
    }

    fn launch(binary: &Path, _project: &Path) -> Launcher {
        Launcher::new(binary)
    }
}

/// The runtime the model restarts.
const RUNTIME: &str = "rec-record";

/// A plugin with nothing to do: a runtime that ran before.
struct Idle;

impl DynPlugin for Idle {
    fn name(&self) -> &str {
        "old runtime"
    }

    fn platform(&self) -> bool {
        true
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

/// A resolver that reads `path` as the runtime [`RUNTIME`] without running
/// it.
fn prepared(path: &Path) -> BinaryResolver<Recorded> {
    let resolver = BinaryResolver::<Recorded>::new(Binaries::new());
    resolver.prepared.lock().unwrap().insert(
        RUNTIME.to_owned(),
        (
            path.to_owned(),
            BinaryManifest {
                sdk: "test".into(),
                plugin_api: Recorded::PLUGIN_API,
                plugins: BTreeMap::new(),
                platform: None,
            },
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ),
    );
    resolver
}

/// The runtimes of [`Recorded`] in `dir`, every start a fresh runtime whose
/// process the model does not start. Each prepared start's state is kept in
/// `starts` so its sender outlives the handle.
fn runtimes(dir: &Path, starts: &Arc<StdMutex<Vec<RuntimeStates>>>) -> Arc<Inner<Recorded>> {
    let spawn: Spawn = Arc::new({
        let starts = starts.clone();
        move |name: &str, _launcher, anchor: &Path| {
            let states = RuntimeStates::new(name, anchor);
            states.starting();
            starts.lock().unwrap().push(states.clone());
            Spawned {
                handle: states.handle(),
                processes: Processes::default(),
                plugin: Box::new(Idle),
            }
        }
    });
    let binary = dir.join("record");
    let resolver = Arc::new(prepared(&binary));
    BinaryRuntimes::<Recorded>::new(resolver, dir, spawn).inner
}

/// The slot of a runtime that runs now, its process one the model ends.
fn slot(root: &Ctx, binary: &Path, processes: Processes) -> Slot {
    let states = RuntimeStates::new(RUNTIME, binary.parent().unwrap());
    states.starting();
    Slot {
        view: root.plugin(Idle),
        handle: states.handle(),
        processes,
        stamp: stamp(binary),
        binary: binary.to_owned(),
        last_used: Instant::now(),
        stopped: None,
        generation: 1,
    }
}

/// The old runtime's process has not ended: the restart does not install
/// the new runtime under its name. Its own process ending frees the name;
/// another runtime's does not.
///
/// Mutations this model catches: an install before the old process is
/// waited for (the new slot is there while the old process still runs),
/// a stop that waits for whichever process ends first (another runtime's
/// end frees the name), a wait that returns without a process (the new
/// slot appears before any end), and a restart that leaves the old slot in
/// place (two slots under one name, or a generation that does not change).
#[test]
fn shuttle_a_restart_installs_the_new_runtime_only_after_the_old_process_ended() {
    explore(|| {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("record");
        std::fs::write(&binary, b"a binary").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let root = Ctx::root_with(runtime.handle().clone());
        let starts = Arc::new(StdMutex::new(Vec::new()));
        let inner = runtimes(dir.path(), &starts);
        *inner.ctx.lock().unwrap() = Some(root.clone());

        let (own, own_process) = Processes::held();
        let (_, other_process) = Processes::held();
        {
            let mut slots = inner.slots.lock().unwrap();
            // One start before this restart, as production's first install
            // records it.
            slots.generation = 1;
            slots
                .running
                .insert(RUNTIME.to_owned(), slot(&root, &binary, own));
        }

        let finished = Arc::new(AtomicBool::new(false));
        let restart = thread::spawn({
            let inner = inner.clone();
            let finished = finished.clone();
            move || {
                let busy = Busy::take(&inner, RUNTIME).expect("the runtime is not busy");
                let result = block_on(inner.restart_now(RUNTIME, busy));
                finished.store(true, Ordering::SeqCst);
                result
            }
        });

        // Taken out of the slots, the old runtime is being stopped: its
        // process has not ended yet.
        let still_running = || inner.slots.lock().unwrap().running.contains_key(RUNTIME);
        while still_running() {
            thread::yield_now();
        }
        // Neither does another runtime's process ending free the name.
        other_process.end();
        for _ in 0..3 {
            thread::yield_now();
        }
        assert!(
            !inner.slots.lock().unwrap().running.contains_key(RUNTIME),
            "another runtime's process freed the name"
        );
        assert!(
            !finished.load(Ordering::SeqCst),
            "the restart went on without the old process"
        );

        // Its own process ends: the new runtime is installed.
        own_process.end();
        restart.join().unwrap().unwrap();
        assert!(
            finished.load(Ordering::SeqCst),
            "the restart did not finish once its process ended"
        );
        let slots = inner.slots.lock().unwrap();
        assert_eq!(
            slots.running.len(),
            1,
            "one name, more than one runtime in the slots"
        );
        let slot = slots.running.get(RUNTIME).expect("the new runtime");
        assert_eq!(
            slot.generation, 2,
            "the installed runtime is not a new start"
        );
        drop(slots);
        assert_eq!(
            starts.lock().unwrap().len(),
            1,
            "the restart prepared no process, or more than one"
        );
    });
}
