//! The channel contract as checks (feature `testing`). Every implementation
//! runs [`contract`] on connected pairs it makes:
//!
//! ```ignore
//! rutis_bridge::channel::testing::contract(|| my_transport::pair());
//! ```
//!
//! Each check panics with what was broken. A transport with a message size
//! limit also runs [`size_limit`] with its limit. Transport-specific
//! behaviour (framing, heartbeats, what the far end is told of a message
//! over the limit, receiving one) is tested by the transport itself.

mod fault;
pub use fault::{fault, Faults};

use std::sync::mpsc;
use std::time::Duration;

use crate::channel::{Channel, ChannelError};

/// How long a check waits for something that must happen.
/// Only a hang guard (#231).
const PATIENCE: Duration = Duration::from_secs(10);

/// How long a check waits before it fails rather than hangs.
const GUARD: Duration = Duration::from_secs(10);

/// Run every check on fresh pairs from `pair`: two channels joined end to
/// end, what one sends the other receives.
pub fn contract(pair: impl Fn() -> (Channel, Channel)) {
    order_and_boundaries(pair());
    large_messages(pair());
    nothing_lost_while_the_receiver_lags(pair());
    close_is_idempotent_and_wakes_a_blocked_receive(pair());
    close_wakes_a_blocked_send(pair());
    the_far_end_closing_ends_the_channel(pair());
}

fn recv(channel: &mut Channel) -> Option<Vec<u8>> {
    channel
        .receiver
        .recv()
        .unwrap_or_else(|error| panic!("receive failed: {error}"))
}

/// Messages arrive once each, in order, with their boundaries, both ways.
pub fn order_and_boundaries((mut a, mut b): (Channel, Channel)) {
    let messages: [&[u8]; 4] = [b"one", b"{}", b"", b"{\"a\":[1,2]}"];
    for message in messages {
        a.sender.send(message).unwrap();
    }
    for message in messages {
        assert_eq!(recv(&mut b).as_deref(), Some(message), "a -> b");
    }
    b.sender.send(b"back").unwrap();
    assert_eq!(recv(&mut a).as_deref(), Some(&b"back"[..]), "b -> a");
}

/// A message of several megabytes keeps its boundary.
pub fn large_messages((mut a, mut b): (Channel, Channel)) {
    let large: Vec<u8> = (0..4 * 1024 * 1024)
        .map(|i| b'a' + (i % 26) as u8)
        .collect();
    let sending = std::thread::spawn(move || {
        a.sender.send(&large).unwrap();
        a.sender.send(b"after").unwrap();
        (a, large)
    });
    let received = recv(&mut b).expect("the large message");
    let (_a, large) = sending.join().unwrap();
    assert!(received == large, "the large message arrived changed");
    assert_eq!(recv(&mut b).as_deref(), Some(&b"after"[..]));
}

/// A sender faster than its receiver waits (backpressure) instead of
/// dropping or reordering.
pub fn nothing_lost_while_the_receiver_lags((mut a, mut b): (Channel, Channel)) {
    const COUNT: usize = 20_000;
    let sending = std::thread::spawn(move || {
        let filler = [b'x'; 512];
        for n in 0..COUNT {
            let mut message = n.to_string().into_bytes();
            message.push(b':');
            message.extend_from_slice(&filler);
            a.sender.send(&message).unwrap();
        }
        a
    });
    // clock: widen — lets the sender fill the channel before reading, the
    // case this checks; it holds either way.
    std::thread::sleep(Duration::from_millis(100));
    for n in 0..COUNT {
        let message = recv(&mut b).expect("a message");
        let prefix = format!("{n}:");
        assert!(
            message.starts_with(prefix.as_bytes()),
            "message {n} out of order"
        );
    }
    sending.join().unwrap();
}

/// Closing twice is harmless, and a receive blocked on the closed channel
/// returns.
pub fn close_is_idempotent_and_wakes_a_blocked_receive((mut a, _b): (Channel, Channel)) {
    let closer = a.closer.clone();
    let (done, finished) = mpsc::channel();
    let receiving = std::thread::spawn(move || {
        let result = a.receiver.recv();
        done.send(()).unwrap();
        result
    });
    // clock: widen — gives the receive time to block before the close; a
    // close before it is the same contract (the receive still ends).
    std::thread::sleep(Duration::from_millis(50));
    closer.close("first");
    closer.close("second");
    finished
        .recv_timeout(PATIENCE)
        .expect("close must wake a blocked receive");
    assert!(
        !matches!(receiving.join().unwrap(), Ok(Some(_))),
        "nothing was sent"
    );
}

/// A send blocked by a receiver that never reads returns once the channel
/// is closed.
pub fn close_wakes_a_blocked_send((mut a, _b): (Channel, Channel)) {
    let closer = a.closer.clone();
    let (done, finished) = mpsc::channel();
    let sending = std::thread::spawn(move || {
        let chunk = vec![b'x'; 64 * 1024];
        let result: Result<(), ChannelError> = loop {
            if let Err(error) = a.sender.send(&chunk) {
                break Err(error);
            }
        };
        done.send(()).unwrap();
        result
    });
    // clock: widen — gives the send time to block on a full channel before
    // the close; the assertion holds whether it did.
    std::thread::sleep(Duration::from_millis(100));
    closer.close("closed while sending");
    finished
        .recv_timeout(PATIENCE)
        .expect("close must wake a blocked send");
    assert!(sending.join().unwrap().is_err());
}

/// When one end closes, the other sees the end after what was sent before
/// it, and can no longer send for long.
pub fn the_far_end_closing_ends_the_channel((mut a, mut b): (Channel, Channel)) {
    a.sender.send(b"last words").unwrap();
    // clock: weak-negative — lets the message leave before the close. The
    // channels do not all promise that a close after a send delivers it, so
    // this checks the ones that do in practice; a slow machine can only
    // make it pass for the wrong reason, not fail.
    std::thread::sleep(Duration::from_millis(20));
    a.closer.close("bye");
    assert_eq!(recv_or_end(&mut b).as_deref(), Some(&b"last words"[..]));
    assert!(
        recv_or_end(&mut b).is_none(),
        "the end follows the last message"
    );
    let failed = (0..1000).any(|_| b.sender.send(&[b'x'; 1024]).is_err());
    assert!(failed, "sending to a closed far end must fail");
}

/// For a transport with a message size limit (Q6.3.2), on pairs whose ends
/// both have the limit `limit`: a message of exactly `limit` bytes arrives;
/// one byte more is refused, ends the channel, and the far end sees the end
/// rather than the message.
///
/// Not part of [`contract`]: the in-memory transport has no limit (it
/// crosses no process boundary, so nothing untrusted sends on it), and a
/// receiver over the limit can only be reached by bypassing the sender's
/// own check, which each transport does in its own tests.
pub fn size_limit((mut a, b): (Channel, Channel), limit: usize) {
    // `b` receives on a thread of its own, so a far end that never sees
    // anything fails the check instead of hanging it.
    let Channel {
        mut receiver,
        closer: far_closer,
        ..
    } = b;
    let (deliver, delivered) = mpsc::channel();
    std::thread::spawn(move || loop {
        let received = receiver.recv();
        let more = matches!(received, Ok(Some(_)));
        if deliver.send(received).is_err() || !more {
            return;
        }
    });
    let next = |what: &str| {
        delivered.recv_timeout(GUARD).unwrap_or_else(|_| {
            far_closer.close("the check gave up");
            panic!("{what}: nothing arrived within {GUARD:?}")
        })
    };

    let at_limit = vec![b'x'; limit];
    let sending = std::thread::spawn(move || {
        a.sender.send(&at_limit).unwrap();
        a
    });
    let received = next("the message at the limit")
        .unwrap_or_else(|error| panic!("receive failed: {error}"))
        .expect("the message at the limit");
    assert_eq!(
        received.len(),
        limit,
        "a message at the limit arrives whole"
    );
    let mut a = sending.join().unwrap();

    let over = vec![b'x'; limit + 1];
    assert!(
        a.sender.send(&over).is_err(),
        "a message over the limit is refused"
    );
    assert!(
        !matches!(next("the end"), Ok(Some(_))),
        "the far end sees the end, not the message"
    );
    assert!(
        a.sender.send(b"after").is_err(),
        "the channel ended with the refused message"
    );
}

fn recv_or_end(channel: &mut Channel) -> Option<Vec<u8>> {
    channel.receiver.recv().ok().flatten()
}
