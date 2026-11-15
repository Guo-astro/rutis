//! What the replay allows and refuses, on traces built by hand: the model's
//! counterexample 1 (two one-stack runtimes calling each other, `current-2node`)
//! and the operations of `waits.rs` around it.

use super::*;

fn profile(session: &str) -> Event {
    Event::Profile {
        session: session.into(),
        stacked: true,
        reentrant: false,
    }
}

fn marked(session: &str, id: &str) -> Event {
    Event::Marked {
        session: session.into(),
        id: id.into(),
    }
}

fn forward(
    dst: &str,
    id: &str,
    seq: u64,
    path: &[&str],
    synchronous: bool,
    decision: Decision,
) -> Event {
    Event::Forward(Forward {
        dst: dst.into(),
        id: id.into(),
        seq,
        path: path.iter().map(|entry| (*entry).to_owned()).collect(),
        synchronous,
        decision,
    })
}

/// n1 calls n2 while neither waits on the other's chain: the call into n2,
/// put aside there, waits for what n2 has out; n2's call into n1, put aside
/// too, waits for the first — the model's counterexample 1, which the table
/// refuses.
fn two() -> Vec<Event> {
    vec![
        profile("n1"),
        profile("n2"),
        marked("n1", "node:1"),
        forward(
            "n2",
            "rust:1",
            1,
            &["n1/node:1"],
            true,
            Decision::Hop {
                src: Some(("n1".into(), "node:1".into())),
                entry: false,
            },
        ),
        marked("n2", "node:1"),
        forward(
            "n1",
            "rust:2",
            2,
            &["n2/node:1"],
            true,
            Decision::Refused {
                calls: vec!["n1/rust:2".into(), "n2/rust:1".into()],
            },
        ),
        Event::Answered {
            session: "n2".into(),
            id: "node:1".into(),
        },
        Event::Returned {
            session: "n2".into(),
            id: "rust:1".into(),
        },
        Event::Answered {
            session: "n1".into(),
            id: "node:1".into(),
        },
        Event::Ended {
            session: "n1".into(),
        },
        Event::Ended {
            session: "n2".into(),
        },
    ]
}

/// The refused forward's index in [`two`].
const REFUSAL: usize = 5;

/// The trace the model allows: profiles, a marked chain, a hop, a refusal
/// and the operations that end them.
#[test]
fn the_counterexample_is_refused() {
    let notes = check(&two()).expect("the refusal the model makes");
    assert_eq!(notes, Vec::<String>::new());
}

/// The same call forwarded instead of refused: the guard is what keeps the
/// hang out, and the replay reports a table that skipped it.
#[test]
fn a_cycle_forwarded_is_reported() {
    let mut events = two();
    events[REFUSAL] = forward(
        "n1",
        "rust:2",
        2,
        &["n2/node:1"],
        true,
        Decision::Hop {
            src: Some(("n2".into(), "node:1".into())),
            entry: false,
        },
    );
    let broken = check(&events).expect_err("a call that waits on itself");
    assert!(broken.why.contains("waits on itself"), "{broken}");
    assert_eq!(broken.at, REFUSAL);
}

/// A refusal naming calls that are not a cycle: the names are checked
/// against the graph, not taken on trust.
#[test]
fn a_refusal_must_name_a_cycle() {
    let mut events = two();
    events[REFUSAL] = forward(
        "n1",
        "rust:2",
        2,
        &["n2/node:1"],
        true,
        Decision::Refused {
            calls: vec!["n1/rust:2".into(), "n2/node:1".into()],
        },
    );
    let broken = check(&events).expect_err("a cycle it is not");
    assert!(broken.why.contains("not the cycle"), "{broken}");
}

/// A call the model has nobody waiting for, recorded as a hop: an entry is
/// what bounds the chains made under it.
#[test]
fn an_entry_is_not_a_hop() {
    let mut events = two();
    let at = events.len() - 2;
    events.insert(
        at,
        forward(
            "n2",
            "rust:3",
            2,
            &[],
            false,
            Decision::Hop {
                src: None,
                entry: false,
            },
        ),
    );
    let broken = check(&events).expect_err("a hop nobody waits for");
    assert!(broken.why.contains("hop nobody waits for"), "{broken}");
}

/// The entry the model records while a forwarded call is in progress, with
/// no mark left: `waits.rs` asks a count, so this one is allowed.
#[test]
fn an_entry_with_nothing_marked_is_allowed() {
    let mut events = two();
    let at = events.len() - 2;
    events.insert(
        at,
        forward(
            "n2",
            "rust:3",
            2,
            &[],
            false,
            Decision::Hop {
                src: None,
                entry: true,
            },
        ),
    );
    check(&events).expect("an entry nobody waits for");
}

/// What a runtime does while it waits is a constant of the model: a second
/// greeting that says otherwise is reported.
#[test]
fn a_profile_does_not_change() {
    let mut events = two();
    events.insert(
        2,
        Event::Profile {
            session: "n1".into(),
            stacked: true,
            reentrant: true,
        },
    );
    let broken = check(&events).expect_err("a profile that changes");
    assert!(broken.why.contains("profile"), "{broken}");
}

/// A session ends only when no call of it waits (`End`): a table that ends
/// one with its call out is reported.
#[test]
fn a_session_does_not_end_with_a_call_out() {
    let mut events = two();
    events.insert(
        4,
        Event::Ended {
            session: "n1".into(),
        },
    );
    let broken = check(&events).expect_err("a session ending while it waits");
    assert!(broken.why.contains("ends while"), "{broken}");
}

/// The call numbers of one session grow (`rustCtr`): a table that reuses one
/// is reported.
#[test]
fn the_call_numbers_grow() {
    let mut events = two();
    events.insert(
        5,
        forward(
            "n2",
            "rust:2",
            1,
            &[],
            true,
            Decision::Hop {
                src: None,
                entry: false,
            },
        ),
    );
    let broken = check(&events).expect_err("a call number that does not grow");
    assert!(broken.why.contains("do not grow"), "{broken}");
}
