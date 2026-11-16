//! Shuttle model of a memory channel's directions (#628): a close, a send
//! and a receive of one direction, and a drop, interleave at the state's
//! lock and its condition variable. The model runs the production
//! `MemorySender`, `MemoryReceiver` and `MemoryCloser`; `crate::sync::Mutex`
//! and `Condvar` make a direction's state shuttle's under
//! `--cfg rutis_shuttle`, so every wait and wakeup is a point where the
//! scheduler switches threads.
//!
//! What the model checks: a close wakes a blocked send and a blocked
//! receive, each with the reason that reached its direction; a message
//! wakes a receiver, and a receive wakes a blocked sender; dropping an end
//! wakes the other side instead of leaving it waiting.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```

use std::sync::Arc;

use shuttle::thread;

use super::{pair_watched, Direction, CAPACITY};
use crate::channel::{Channel, ChannelError, Close};

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

/// One end's three parts, taken apart so a model can hand each to a thread.
fn parts(
    channel: Channel,
) -> (
    Box<dyn crate::channel::Sender>,
    Box<dyn crate::channel::Receiver>,
    Arc<dyn crate::channel::Closer>,
) {
    (channel.sender, channel.receiver, channel.closer)
}

/// Wait until `count` threads wait on `direction`. The model's own scheduling
/// point: a yield runs a thread blocked on the direction to its wait.
fn until_waiting(direction: &Direction, count: usize) {
    for _ in 0..10_000 {
        if direction.waiting() >= count {
            return;
        }
        thread::yield_now();
    }
    panic!("{count} threads never waited on the direction");
}

/// #628: a close wakes a blocked send and a blocked receive of the same end,
/// each reports why: the sender the pair's reason, the receiver that it was
/// closed here. A later close does not change the reason.
#[test]
fn a_close_wakes_a_blocked_send_and_receive() {
    explore(|| {
        let (a, b, a_sends_into, a_receives_from) = pair_watched();
        let (mut sender, mut receiver, closer) = parts(a);
        // The send direction is full; the receive direction is empty.
        for _ in 0..CAPACITY {
            sender.send(b"x").unwrap();
        }
        let sending = thread::spawn(move || sender.send(b"blocked"));
        let receiving = thread::spawn(move || receiver.recv());
        until_waiting(&a_sends_into, 1);
        until_waiting(&a_receives_from, 1);

        closer.close(Close::Rejected("revoked".into()));
        // A later close keeps the first reason.
        closer.close(Close::Done);
        assert_eq!(
            sending.join().unwrap(),
            Err(ChannelError::Rejected {
                reason: "rejected: revoked".into()
            })
        );
        assert_eq!(
            receiving.join().unwrap(),
            Err(ChannelError::Closed {
                reason: "closed here".into()
            })
        );

        // The far end drains what was there, then reads the same reason.
        let mut b = b;
        for _ in 0..CAPACITY {
            assert_eq!(b.receiver.recv().unwrap().unwrap(), b"x");
        }
        assert_eq!(
            b.receiver.recv(),
            Err(ChannelError::Rejected {
                reason: "rejected: revoked".into()
            })
        );
    });
}

/// #628: a send wakes a waiting receiver, and a receive wakes a blocked
/// sender once it makes room; the direction's queue keeps its order.
#[test]
fn a_send_and_a_receive_wake_each_other() {
    explore(|| {
        // Nothing was sent yet: the receiver waits until a message comes.
        let (mut a, b, a_sends_into, _) = pair_watched();
        let b_receiver = thread::spawn(move || {
            let mut b = b;
            b.receiver.recv()
        });
        until_waiting(&a_sends_into, 1);
        a.sender.send(b"one").unwrap();
        assert_eq!(b_receiver.join().unwrap().unwrap().unwrap(), b"one");

        // The direction is full: the next send waits until a receive makes
        // room, and what it sent comes after the queue.
        let (mut c, mut d, c_sends_into, _) = pair_watched();
        for _ in 0..CAPACITY {
            c.sender.send(b"x").unwrap();
        }
        let mut sender = c.sender;
        let sending = thread::spawn(move || sender.send(b"last"));
        until_waiting(&c_sends_into, 1);
        assert_eq!(d.receiver.recv().unwrap().unwrap(), b"x");
        sending.join().unwrap().unwrap();
        for _ in 0..CAPACITY - 1 {
            assert_eq!(d.receiver.recv().unwrap().unwrap(), b"x");
        }
        assert_eq!(d.receiver.recv().unwrap().unwrap(), b"last");
        c.closer.close(Close::Done);
        assert_eq!(d.receiver.recv(), Ok(None));
    });
}

/// #628: dropping one end wakes the other instead of leaving it waiting: a
/// sender with no room ends when the receiver goes, and a receiver drains
/// what was sent before it sees the sender gone.
#[test]
fn a_drop_wakes_the_other_end() {
    explore(|| {
        // A blocked send wakes when the receiver is dropped.
        let (mut a, b, a_sends_into, _) = pair_watched();
        for _ in 0..CAPACITY {
            a.sender.send(b"x").unwrap();
        }
        let mut sender = a.sender;
        let sending = thread::spawn(move || sender.send(b"blocked"));
        until_waiting(&a_sends_into, 1);
        drop(b);
        assert_eq!(
            sending.join().unwrap(),
            Err(ChannelError::Closed {
                reason: "the channel is closed".into()
            })
        );

        // A receiver drains what was sent, then sees the end when the
        // sender goes.
        let (mut c, mut d, _, _) = pair_watched();
        c.sender.send(b"before").unwrap();
        drop(c.sender);
        assert_eq!(d.receiver.recv().unwrap().unwrap(), b"before");
        assert_eq!(d.receiver.recv(), Ok(None));
    });
}
