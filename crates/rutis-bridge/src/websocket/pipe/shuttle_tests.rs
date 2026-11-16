//! Shuttle model of a WebSocket pipe's blocking side (#628): the sender and
//! receiver of a connection block on the pipe, the connection task closes
//! it, and each wakes the other as messages move. The model runs the
//! production `Pipe::push_blocking`, `pop_blocking` and `close`;
//! `crate::sync::Mutex` and `Condvar` make the queue shuttle's under
//! `--cfg rutis_shuttle`, so every wait and wakeup is a point where the
//! scheduler switches threads.
//!
//! What the model does not cover: the async side (`push`/`pop`, which the
//! connection task awaits on tokio's `Notify`) needs a tokio task to poll,
//! and this model has none. Its wakeups are the same state under the same
//! lock; the deterministic tests of the websocket suite cover the task.
//!
//! Run (CONTRIBUTING.md), with the feature:
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --features \
//!     websocket --lib --target-dir target/shuttle shuttle
//! ```

use std::sync::Arc;

use shuttle::thread;

use super::{Ending, Pipe};
use crate::channel::ChannelError;

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

/// Wait until `count` threads wait on `pipe`.
fn until_waiting(pipe: &Pipe, count: usize) {
    for _ in 0..10_000 {
        if pipe.waiting() >= count {
            return;
        }
        thread::yield_now();
    }
    panic!("{count} threads never waited on the pipe");
}

/// #628: a close wakes a blocked push and a blocked pop, each with the reason
/// its pipe ended (the first close's); what was queued before the close is
/// still read, and nothing is admitted after it.
#[test]
fn a_close_wakes_a_blocked_push_and_pop() {
    explore(|| {
        // A push with no room, and a pop with nothing to read.
        let full = Arc::new(Pipe::new(1));
        full.push_blocking(b"one").unwrap();
        let pushed = {
            let pipe = full.clone();
            thread::spawn(move || pipe.push_blocking(b"two"))
        };
        let empty = Arc::new(Pipe::new(1));
        let popped = {
            let pipe = empty.clone();
            thread::spawn(move || pipe.pop_blocking())
        };
        until_waiting(&full, 1);
        until_waiting(&empty, 1);
        full.close(Ending::Rejected("revoked".into()));
        // A later close keeps the first ending.
        full.close(Ending::Finished);
        empty.close(Ending::Failed("connection lost".into()));
        assert_eq!(
            pushed.join().unwrap(),
            Err(ChannelError::Rejected {
                reason: "revoked".into()
            })
        );
        assert_eq!(
            popped.join().unwrap(),
            Err(ChannelError::Closed {
                reason: "connection lost".into()
            })
        );

        // What was queued before the close is still read; after it, the end,
        // and nothing more is admitted.
        assert_eq!(full.pop_blocking().unwrap().unwrap(), b"one");
        assert_eq!(
            full.pop_blocking(),
            Err(ChannelError::Rejected {
                reason: "revoked".into()
            })
        );
        assert!(full.push_blocking(b"more").is_err());
    });
}

/// #628: a pop makes room for a blocked push, and a push wakes a blocked pop;
/// a normal end reads as `Ok(None)` once the queue is drained.
#[test]
fn a_push_and_a_pop_wake_each_other() {
    explore(|| {
        // A blocked push admits once a pop makes room.
        let pipe = Arc::new(Pipe::new(1));
        pipe.push_blocking(b"one").unwrap();
        let pushing = {
            let pipe = pipe.clone();
            thread::spawn(move || pipe.push_blocking(b"two"))
        };
        until_waiting(&pipe, 1);
        assert_eq!(pipe.pop_blocking().unwrap().unwrap(), b"one");
        pushing.join().unwrap().unwrap();
        assert_eq!(pipe.pop_blocking().unwrap().unwrap(), b"two");

        // A blocked pop wakes when a message arrives.
        let pipe = Arc::new(Pipe::new(1));
        let popping = {
            let pipe = pipe.clone();
            thread::spawn(move || pipe.pop_blocking())
        };
        until_waiting(&pipe, 1);
        pipe.push_blocking(b"three").unwrap();
        assert_eq!(popping.join().unwrap().unwrap().unwrap(), b"three");
        pipe.close(Ending::Finished);
        assert_eq!(pipe.pop_blocking(), Ok(None));
    });
}
