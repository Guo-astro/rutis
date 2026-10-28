use super::*;
use rutis_test_support::{eventually_blocking, recv_within};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn meets_the_channel_contract() {
    crate::channel::testing::contract(pair);
}

#[test]
fn a_fault_free_fault_wrapper_meets_the_contract_too() {
    use crate::channel::testing::fault;
    crate::channel::testing::contract(|| {
        let (a, b) = pair();
        (fault(a).0, fault(b).0)
    });
}

#[test]
fn faults_delay_go_half_open_and_drop() {
    use crate::channel::testing::fault;
    let (a, b) = pair();
    let (mut a, faults) = fault(a);
    let mut b = b;

    faults.delay(Duration::from_millis(50));
    let started = std::time::Instant::now();
    a.sender.send(b"slow").unwrap();
    // A lower bound only: a slow machine cannot make it fail (#231).
    assert!(started.elapsed() >= Duration::from_millis(50));
    assert_eq!(b.receiver.recv().unwrap().unwrap(), b"slow");

    // Half-open: sends vanish, nothing is delivered, nothing closes.
    faults.half_open();
    a.sender.send(b"lost").unwrap();
    b.sender.send(b"swallowed").unwrap();
    let (got, receiving) = mpsc::channel();
    let receiver = std::thread::spawn(move || {
        let message = a.receiver.recv();
        got.send(()).unwrap();
        (a, message)
    });
    // "swallowed" reached the half-open end and was dropped there.
    eventually_blocking(
        || (faults.swallowed() == 1).then_some(()),
        "the swallowed message",
    );
    faults.heal();
    b.sender.send(b"after").unwrap();
    recv_within("the message after healing", &receiving);
    let (mut a, message) = receiver.join().unwrap();
    // The first message to arrive is the one after healing: neither
    // "swallowed" nor anything else got through while half-open.
    assert_eq!(message.unwrap().unwrap(), b"after");

    faults.drop_and_close();
    assert!(a.sender.send(b"x").is_err());
    // "lost", sent while half-open, never arrives: the end does.
    assert!(matches!(b.receiver.recv(), Ok(None)));
}

#[test]
fn delivers_in_order_with_boundaries_and_ends_normally() {
    let (mut a, mut b) = pair();
    for message in [&b"one"[..], b"", b"three\nlines"] {
        a.sender.send(message).unwrap();
    }
    drop(a.sender);
    assert_eq!(b.receiver.recv().unwrap().unwrap(), b"one");
    assert_eq!(b.receiver.recv().unwrap().unwrap(), b"");
    assert_eq!(b.receiver.recv().unwrap().unwrap(), b"three\nlines");
    assert_eq!(b.receiver.recv().unwrap(), None);
}

#[test]
fn a_full_buffer_blocks_the_sender_until_the_receiver_reads() {
    let (mut a, mut b, sent_into, _) = pair_watched();
    for _ in 0..CAPACITY {
        a.sender.send(b"x").unwrap();
    }
    let (sent, done) = mpsc::channel();
    let sender = std::thread::spawn(move || {
        a.sender.send(b"last").unwrap();
        sent.send(()).unwrap();
    });
    // Blocked: waiting on the full direction, and not done.
    eventually_blocking(
        || (sent_into.waiting() == 1).then_some(()),
        "the blocked sender",
    );
    assert!(done.try_recv().is_err());
    b.receiver.recv().unwrap();
    recv_within("the send once there is room", &done);
    sender.join().unwrap();
}

#[test]
fn close_is_idempotent_and_wakes_blocked_send_and_receive() {
    let (mut a, b, a_sends_into, _) = pair_watched();
    for _ in 0..CAPACITY {
        a.sender.send(b"x").unwrap();
    }
    let closer = a.closer.clone();
    let blocked_send = std::thread::spawn(move || a.sender.send(b"blocked"));
    let (mut c, _d, _, c_receives_from) = pair_watched();
    let closer_c = c.closer.clone();
    let blocked_recv = std::thread::spawn(move || c.receiver.recv());
    // Both block before the close.
    eventually_blocking(
        || (a_sends_into.waiting() == 1 && c_receives_from.waiting() == 1).then_some(()),
        "the blocked send and receive",
    );
    closer.close(Close::Done);
    closer.close(Close::Replaced);
    closer_c.close(Close::Done);
    assert_eq!(
        blocked_send.join().unwrap(),
        Err(ChannelError::Closed {
            reason: "the channel is closed".into()
        })
    );
    assert_eq!(
        blocked_recv.join().unwrap(),
        Err(ChannelError::Closed {
            reason: "closed here".into()
        })
    );
    drop(b);
}

#[test]
fn the_far_end_sees_a_close_as_the_end_after_draining() {
    let (mut a, mut b) = pair();
    a.sender.send(b"before").unwrap();
    a.closer.close(Close::Done);
    assert_eq!(b.receiver.recv().unwrap().unwrap(), b"before");
    assert_eq!(b.receiver.recv().unwrap(), None);
    assert!(b.sender.send(b"after").is_err());
}

#[test]
fn the_far_end_learns_why_a_channel_closed() {
    let (a, mut b) = pair();
    a.closer.close(Close::Done);
    assert_eq!(b.receiver.recv(), Ok(None));

    let (a, mut b) = pair();
    a.closer.close(Close::Replaced);
    assert_eq!(
        b.receiver.recv(),
        Err(ChannelError::Closed {
            reason: "replaced by a new connection".into()
        })
    );

    let (a, mut b) = pair();
    a.closer.close(Close::Rejected("not accepted here".into()));
    assert_eq!(
        b.receiver.recv(),
        Err(ChannelError::Rejected {
            reason: "rejected: not accepted here".into()
        })
    );
}
