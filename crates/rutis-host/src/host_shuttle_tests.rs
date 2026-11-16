//! Shuttle models of a removed runtime's wait for its process (#627,
//! #622's host-reload item): when a reload removes a runtime, the host stops
//! it and waits for the process it started — its own generation's, not
//! whichever runtime's process ends first (57f536a0).
//!
//! What runs is [`take_processes`] and [`runtime_process_ended`], the pieces
//! [`Host::stop_runtime_process`] is made of, with real [`Processes`]: a
//! test ends a process by hand (`Processes::held`), and under
//! `--cfg rutis_shuttle` the wait yields to shuttle's scheduler instead of
//! sleeping real 20 ms rounds. What is written out is the rest of the
//! reload: the configuration, the rows and the host are #622's own models'
//! subject, not this wait's; the model starts from the process lists the
//! reload leaves (one per runtime).
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-host --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! `RUTIS_SHUTTLE_ITERATIONS` sets the iterations of each scheduler: 1000
//! by default, as on pull requests; the nightly run (`stress.yml`) uses a
//! million. A failure prints its schedule; `RUTIS_SHUTTLE_REPLAY=<schedule>`
//! replays it, with the failing test's name as the filter.
//!
//! guarantee: host/reload-config

use std::sync::Arc;

use rutis_bridge::pipe::Processes;
use shuttle::future::block_on;
use shuttle::sync::atomic::{AtomicBool, Ordering};
use shuttle::thread;

use super::{runtime_process_ended, take_processes};

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

/// A reload removes `py`: it takes the processes `py` started, stops it,
/// and waits for that generation's process. `node`'s process ending does
/// not free `py`; only `py`'s own does, and `bun`'s (never ended here) never
/// does.
///
/// Mutations this model catches: a wait for whichever runtime's process
/// ends first (`running()` over all of them), one that returns at once
/// (the wait is done before any process ended), a wait over every
/// remaining runtime's processes (`bun`'s never ends, so it would not
/// return), and a take that removes another runtime's processes (the
/// other's end frees it, the own one does not).
#[test]
fn shuttle_a_removed_runtime_waits_for_its_own_process() {
    explore(|| {
        let (own, own_process) = Processes::held();
        let (other, other_process) = Processes::held();
        let (never, _never_process) = Processes::held();
        let mut processes = vec![
            ("py".to_owned(), own),
            ("node".to_owned(), other),
            ("bun".to_owned(), never),
        ];

        // The reload takes the runtime's own processes before it stops it.
        let own = take_processes(&mut processes, "py").expect("py's processes");
        assert_eq!(
            processes
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["node", "bun"],
            "taking one runtime's processes changed another's"
        );

        let done = Arc::new(AtomicBool::new(false));
        let waiting = thread::spawn({
            let done = done.clone();
            move || {
                block_on(runtime_process_ended("py", &own)).expect("the wait");
                done.store(true, Ordering::SeqCst);
            }
        });

        // Another runtime's process ends: the wait must not return.
        other_process.end();
        for _ in 0..3 {
            thread::yield_now();
        }
        assert!(
            !done.load(Ordering::SeqCst),
            "another runtime's process freed the removed runtime's wait"
        );

        // Its own process ends: it returns.
        own_process.end();
        waiting.join().unwrap();
        assert!(
            done.load(Ordering::SeqCst),
            "the wait did not return once its own process ended"
        );
    });
}
