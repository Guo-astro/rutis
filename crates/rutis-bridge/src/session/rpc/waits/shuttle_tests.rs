//! Shuttle model of the wait graph's fast path (#628, SYN-9-29).
//!
//! A reply and a cancellation read the table without its lock while nothing
//! is marked (`waits::answered`) or no hop is recorded (`waits::returned`):
//! two atomics and a return. A call marked on another thread at that moment
//! must not be left in the table, and a hop recorded as one returns must not
//! be left either.
//!
//! The model runs the production `marked`, `answered`, `forward`, `returned`
//! and `ended`; `crate::sync` makes the table's mutex and its two counters
//! shuttle's under `--cfg rutis_shuttle`, so a fast-path read and a mark or
//! a forward on another thread interleave at every access.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```

use std::sync::Arc;

use shuttle::sync::Barrier;
use shuttle::thread;

use super::{answered, ended, forward, hops_of, marked, marks_of, returned};

/// Iterations of each scheduler: `RUTIS_SHUTTLE_ITERATIONS`, or 1000.
fn iterations() -> usize {
    std::env::var("RUTIS_SHUTTLE_ITERATIONS")
        .ok()
        .map(|value| value.parse().expect("RUTIS_SHUTTLE_ITERATIONS is a number"))
        .unwrap_or(1000)
}

/// Explore `model` with shuttle's random scheduler and with PCT.
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

/// A session tag of the model's own: the table is the process's.
fn tag() -> String {
    super::super::next_tag()
}

/// #628, SYN-9-29: a call answered while nothing is marked leaves no mark,
/// and a call returned while no hop is recorded leaves no hop — however the
/// two atomics are read against a mark or a forward on another thread.
#[test]
fn an_answer_and_a_return_are_not_lost_on_the_fast_path() {
    explore(|| {
        let session = tag();
        let peer = tag();
        let (mark_a, mark_b, hop) = ("a1", "b2", "h1");

        // Two calls marked together are answered together; a third thread
        // answers and returns nothing of its own (the fast path) around
        // them, and a fourth records and returns one hop.
        let step = Arc::new(Barrier::new(4));
        let marking = |id: &'static str| {
            let (session, step) = (session.clone(), step.clone());
            thread::spawn(move || {
                marked(&session, id);
                step.wait();
                answered(&session, id);
            })
        };
        let first = marking(mark_a);
        let second = marking(mark_b);
        let fast = {
            let (peer, step) = (peer.clone(), step.clone());
            thread::spawn(move || {
                // Nothing is marked and no hop is recorded: these read the
                // counters and return.
                answered(&peer, "nothing");
                returned(&peer, "nothing");
                step.wait();
                answered(&peer, "nothing either");
                returned(&peer, "nothing either");
            })
        };
        let hopping = {
            let (peer, step) = (peer.clone(), step.clone());
            thread::spawn(move || {
                assert!(
                    forward(&peer, hop, 1, &[], true).expect("a first hop closes no cycle"),
                    "a synchronous call recorded no hop"
                );
                step.wait();
                returned(&peer, hop);
                // Whatever was recorded for this session is gone.
                assert_eq!(hops_of(&peer), 0, "a returned hop stayed in the table");
            })
        };
        first.join().unwrap();
        second.join().unwrap();
        fast.join().unwrap();
        hopping.join().unwrap();

        assert_eq!(
            marks_of(&session),
            0,
            "an answered call stayed marked on the fast path"
        );
        assert_eq!(
            hops_of(&peer),
            0,
            "a returned hop stayed in the table on the fast path"
        );
        ended(&session);
        ended(&peer);
        assert_eq!(marks_of(&session), 0, "an ended session kept a mark");
        assert_eq!(hops_of(&peer), 0, "an ended session kept a hop");
    });
}
