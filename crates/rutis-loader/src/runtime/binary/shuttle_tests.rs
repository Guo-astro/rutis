//! Shuttle models of a binary runtime's restart (#627): a restart takes the
//! old runtime out of the slots, stops it and waits for its process, and
//! only then installs the new one — one name, one process (#307); and a row
//! that resolves to the runtime meanwhile is wanted again, not a second
//! process.
//!
//! What runs is the production [`Inner::restart_now`], [`stop`],
//! [`Inner::install`], [`Inner::want`] and [`Busy`]. What stands in for the
//! rest is named where it is installed:
//!
//! - the process is a [`Processes`] a test ends by hand (`Processes::held`);
//! - the binary is a file of [`model`]: the model replaces it with a stamp of
//!   its own, so no file is written and no clock decides when it changed
//!   ([`super::stamp`] asks the model first);
//! - the launcher, the disposal of an old runtime's view and the install of
//!   the new one are notes of the model ([`model::Note`]): the launcher is
//!   the seam a start goes through, and the kernel's dispose is #622's own
//!   subject, so it is written out under `--cfg rutis_shuttle` and recorded;
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
//!
//! guarantee: host/runtime-restart, loader/go-runtimes

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::{Ctx, Error};
use rutis_bridge::pipe::Processes;
use rutis_bridge::runtime::{Launcher, RuntimeStates};
use shuttle::future::block_on;
use shuttle::sync::atomic::{AtomicBool, Ordering};
use shuttle::thread;

use super::model::{self, Note};
use super::{
    stamp, Binaries, BinaryLanguage, BinaryManifest, BinaryResolver, BinaryRuntimes, Busy, Inner,
    Slot, Spawn, Spawned, Stamp,
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

/// The runtimes of a model whose binary is only a [`model`] file: its
/// directory, and the runtime it restarts.
const DIR: &str = "model";

/// Bytes of the file before it is replaced, and of the replacement: the two
/// stamps differ by their length alone.
const BEFORE: u64 = 1;
const AFTER: u64 = 2;

/// A model's world: a root with a tokio handle for the fibers a mounted
/// runtime would use, what its starts prepared, its runtimes, and the file
/// they start from as the model has it.
struct Arranged {
    root: Ctx,
    starts: Arc<StdMutex<Vec<RuntimeStates>>>,
    inner: Arc<Inner<Recorded>>,
    binary: PathBuf,
}

/// The world of a model whose file is `len` bytes, the binary it starts from
/// put there by the model (no file system, no writes).
fn arranged(len: u64) -> Arranged {
    model::begin();
    let dir = Path::new(DIR);
    let binary = dir.join("record");
    let _ = model::file(&binary, len);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let root = Ctx::root_with(runtime.handle().clone());
    let starts = Arc::new(StdMutex::new(Vec::new()));
    let inner = runtimes(dir, &starts);
    *inner.ctx.lock().unwrap() = Some(root.clone());
    Arranged {
        root,
        starts,
        inner,
        binary,
    }
}

impl Arranged {
    /// The runtime runs now, from the file as it is, its process one the
    /// model ends.
    fn running(&self) -> rutis_bridge::pipe::HeldProcess {
        let (own, own_process) = Processes::held();
        let mut slots = self.inner.slots.lock().unwrap();
        // One start before this restart, as production's first install
        // records it.
        slots.generation = 1;
        slots
            .running
            .insert(RUNTIME.to_owned(), slot(&self.root, &self.binary, own));
        own_process
    }

    /// The runtime's stamp and generation, as the slots have them; `None`
    /// when no runtime holds the name.
    fn installed(&self) -> Option<(Option<Stamp>, u64)> {
        self.inner
            .slots
            .lock()
            .unwrap()
            .running
            .get(RUNTIME)
            .map(|slot| (slot.stamp.clone(), slot.generation))
    }

    /// Whether a runtime holds the name now.
    fn holds(&self) -> bool {
        self.inner
            .slots
            .lock()
            .unwrap()
            .running
            .contains_key(RUNTIME)
    }

    /// Starts prepared so far.
    fn prepared(&self) -> usize {
        self.starts.lock().unwrap().len()
    }

    /// A restart of the runtime, on a thread of the model: `Busy` held, then
    /// the production `restart_now`.
    fn restart(
        &self,
        finished: &Arc<AtomicBool>,
    ) -> shuttle::thread::JoinHandle<Result<(), String>> {
        let inner = self.inner.clone();
        let finished = finished.clone();
        thread::spawn(move || {
            let busy = Busy::take(&inner, RUNTIME).expect("the runtime is not busy");
            let result = block_on(inner.restart_now(RUNTIME, busy));
            finished.store(true, Ordering::SeqCst);
            result
        })
    }

    /// Wait until the stop of the runtime taken out of the slots waits for its
    /// process.
    fn wait_for_stop(&self) -> Note {
        let waited = Note::Waited {
            runtime: RUNTIME.to_owned(),
        };
        while !model::notes().contains(&waited) {
            thread::yield_now();
        }
        waited
    }
}

/// A reload whose binary was replaced: the old runtime's view is disposed and
/// its process ended before the new configuration — the replacement's
/// launcher, recorded where a start goes through it — is installed.
///
/// Mutations this model catches: an install before the old runtime's view is
/// disposed (the note order), an install before its process ended (the
/// install is there while the stop waits), a start built from the old file
/// (the launcher's stamp is the one before the replacement), and a new
/// runtime that keeps the old file's stamp (the installed slot's stamp).
#[test]
fn shuttle_a_replaced_binary_is_installed_only_after_the_old_runtime_stopped() {
    explore(|| {
        let world = arranged(BEFORE);
        let before = stamp(&world.binary).expect("the file before the replacement");
        let own_process = world.running();
        assert_eq!(
            world.installed().map(|(stamp, _)| stamp),
            Some(Some(before)),
            "the runtime did not start from the file the model arranged"
        );

        // The reload's new configuration: the file replaced, and what starts
        // from it another binary.
        let after = model::file(&world.binary, AFTER);

        let finished = Arc::new(AtomicBool::new(false));
        let restart = world.restart(&finished);

        // The old runtime is out of the slots, its view disposed, and the
        // stop waits for its process: the new configuration is built (its
        // launcher, from the replacement) and not installed.
        let waited = world.wait_for_stop();
        assert_eq!(
            model::notes(),
            vec![
                Note::Launched {
                    runtime: RUNTIME.to_owned(),
                    program: world.binary.as_os_str().to_owned(),
                    stamp: Some(after.clone()),
                },
                Note::Disposed {
                    runtime: RUNTIME.to_owned(),
                },
                waited.clone(),
            ],
            "the replacement was built or installed in another order"
        );
        assert!(
            !finished.load(Ordering::SeqCst),
            "the restart went on without the old process"
        );
        assert!(
            !world.holds(),
            "the new runtime was installed while the old one was stopping"
        );

        // Its own process ends: the new configuration is installed.
        own_process.end();
        restart.join().unwrap().unwrap();
        assert_eq!(
            model::notes(),
            vec![
                Note::Launched {
                    runtime: RUNTIME.to_owned(),
                    program: world.binary.as_os_str().to_owned(),
                    stamp: Some(after.clone()),
                },
                Note::Disposed {
                    runtime: RUNTIME.to_owned(),
                },
                waited,
                Note::Ended {
                    runtime: RUNTIME.to_owned(),
                },
                Note::Installed {
                    runtime: RUNTIME.to_owned(),
                    stamp: Some(after.clone()),
                },
            ],
            "the new configuration was not installed after the old runtime stopped"
        );
        assert_eq!(
            world.installed(),
            Some((Some(after), 2)),
            "the installed runtime is not a new start of the replacement"
        );
        assert_eq!(
            world.prepared(),
            1,
            "the restart prepared no process, or more than one"
        );
    });
}

/// A row of the reload's new configuration resolves to the runtime while the
/// restart is stopping the old one: production marks it wanted again instead
/// of starting a second process under the name, and the runtime the restart
/// installs — from the replacement — serves it.
///
/// Mutations this model catches: a `want` that starts the runtime while it is
/// busy (a second process, and a launcher of its own), one that drops the row
/// (nothing is wanted again), and a restart that installs the file it read
/// before the replacement.
#[test]
fn shuttle_a_row_wanted_during_the_restart_waits_and_the_new_runtime_serves_it() {
    explore(|| {
        let world = arranged(BEFORE);
        let own_process = world.running();
        let after = model::file(&world.binary, AFTER);

        let finished = Arc::new(AtomicBool::new(false));
        let restart = world.restart(&finished);
        world.wait_for_stop();

        // A row arrives meanwhile.
        world.inner.want(RUNTIME);
        assert!(
            world.inner.slots.lock().unwrap().rewanted.contains(RUNTIME),
            "the row was dropped, not wanted again"
        );
        assert!(
            !world.holds(),
            "the row installed a runtime while the old one was stopping"
        );
        assert_eq!(
            world.prepared(),
            1,
            "the row started a second process under the name"
        );

        // Its process ends, and the restart installs the new configuration;
        // the row wanted again once it is over (production's `Busy` drop
        // starts that, the model asks itself) finds it running.
        own_process.end();
        restart.join().unwrap().unwrap();
        world.inner.want(RUNTIME);
        assert_eq!(
            world.prepared(),
            1,
            "the row started a second process under the name"
        );
        assert_eq!(
            world.inner.slots.lock().unwrap().running.len(),
            1,
            "one name, more than one runtime in the slots"
        );
        assert_eq!(
            world.installed(),
            Some((Some(after), 2)),
            "the runtime the row got is of another file"
        );
        let installed = model::notes()
            .iter()
            .filter(|note| matches!(note, Note::Installed { .. }))
            .count();
        assert_eq!(installed, 1, "one name, more than one runtime installed");
    });
}
