//! What the wait table did, recorded for a test to check against the model
//! (`specs/cross-runtime-sync/CrossRuntimeSync.tla`: `RustForward`,
//! `RustAnswered`, `RustReturn`, `End`; feature `testing`).
//!
//! Each of `waits.rs`'s six operations records one [`Event`], and `forward`
//! records the decision it made: a hop, a call back on its own chain, a call
//! nobody waits for, or a refusal naming the cycle. Recording is off until a
//! test [`capture`]s it, so a run that captures nothing pays an atomic read
//! per operation and no allocation.
//!
//! A capture also takes the table's state at that moment: a process's table
//! outlives it, and the tests of a binary share the table, so a capture can
//! begin while another run's calls are in flight. [`check`] replays from that
//! state as the model does: the profiles are the runtimes' constants
//! (`stacked`, `reentrant`), a marked call is one a far end waits for, and a
//! hop is a call this process forwarded. It rebuilds the model's graph
//! (`RustSucc`: chain, put aside, stack order) before each forward and asks
//! whether the decision was one the model allows there.
//!
//! An operation that finds the table empty (`answered` with nothing marked,
//! `returned` with no hop) changes nothing and is not recorded: the replay
//! has no state to change either.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use super::Hop;

mod check;

pub use check::{check, Trace, Violation};

/// One operation of the wait table, in the order it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `profile`: a session's far end greeted, and what it does while it
    /// waits synchronously.
    Profile {
        session: String,
        stacked: bool,
        reentrant: bool,
    },
    /// `marked`: a call received with `sync`; its caller waits for it.
    Marked { session: String, id: String },
    /// `answered`: this side answered the far end's call `id`.
    Answered { session: String, id: String },
    /// `returned`: the reply to this side's call `id` was read, or the call
    /// was cancelled.
    Returned { session: String, id: String },
    /// `ended`: the session ended.
    Ended { session: String },
    /// `forward`: a call this side sends, and what was recorded for it.
    Forward(Forward),
}

/// A call this side sends on `dst` (see `waits::forward`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forward {
    /// The session the call goes to.
    pub dst: String,
    /// The call's id there.
    pub id: String,
    /// Its number on that session.
    pub seq: u64,
    /// The chain as sent, before the far end's rebase (`path`): an untagged
    /// entry is `dst`'s, a tagged one (`s1/node:3`) the tagged session's.
    pub path: Vec<String>,
    /// The caller waits for it synchronously (the frame says `sync`).
    pub synchronous: bool,
    /// What was recorded.
    pub decision: Decision,
}

/// What `waits::forward` did with a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Recorded as a hop: `src` and `sid` are the marked call it was made
    /// under (neither when it is the host's own call, #597), and `entry` when
    /// nobody waits for it.
    Hop {
        src: Option<(String, String)>,
        entry: bool,
    },
    /// A call back into the session it came from: on its caller's chain,
    /// which runs it, so not recorded.
    SameChain { caller: (String, String) },
    /// Not recorded: nothing was marked and the call is asynchronous, or the
    /// thread sends unchained.
    NotWaited { why: &'static str },
    /// Refused: it would have waited on itself. `calls` is the cycle, the
    /// new call first.
    Refused { calls: Vec<String> },
}

/// A run's record: what the wait table did while the [`Capture`] was alive,
/// and the state it began in.
///
/// Events of other runs of the same test binary are recorded too; the
/// process gives each session a tag of its own (`rpc.rs`, `next_tag`), so a
/// replay keeps them apart by session and neither sees the other. A session
/// another run started before this capture began has its profile, marks and
/// hops in the trace's initial state, so the replay still starts from what
/// the table held.
#[derive(Debug)]
pub struct Capture {
    inner: Arc<Recorder>,
}

#[derive(Debug)]
struct Recorder {
    /// The table when recording began: where the replay starts.
    initial: check::Initial,
    events: Mutex<Vec<Event>>,
}

impl Capture {
    /// What was recorded so far, in the order it happened.
    pub fn events(&self) -> Vec<Event> {
        self.inner.events.lock().unwrap().clone()
    }

    /// The run: the table's state when recording began, and what was
    /// recorded after.
    pub fn trace(&self) -> Trace {
        Trace::of(self.inner.initial.clone(), self.events())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        CAPTURES.fetch_sub(1, Ordering::SeqCst);
        let mut recorders = RECORDERS.lock().unwrap();
        recorders.retain(|recorder| recorder.strong_count() > 0);
    }
}

/// The runs recording now; a [`Capture`] is one of them.
static RECORDERS: LazyLock<Mutex<Vec<Weak<Recorder>>>> = LazyLock::new(Mutex::default);

/// How many captures are alive, read before taking the lock: a run that
/// records nothing pays this and no more.
static CAPTURES: AtomicUsize = AtomicUsize::new(0);

/// Record what the wait table does until the returned [`Capture`] is
/// dropped.
pub fn capture() -> Capture {
    // The state and the recorder are taken under the table's lock: an
    // operation between the two would be neither in the state nor in the
    // events, and the replay would start from a table it cannot have.
    let waits = super::WAITS.lock().unwrap();
    let inner = Arc::new(Recorder {
        initial: check::Initial::of(&waits),
        events: Mutex::default(),
    });
    RECORDERS.lock().unwrap().push(Arc::downgrade(&inner));
    CAPTURES.fetch_add(1, Ordering::SeqCst);
    drop(waits);
    Capture { inner }
}

fn recording() -> bool {
    CAPTURES.load(Ordering::SeqCst) > 0
}

/// Hand `event` to every live capture.
fn record(event: Event) {
    let mut recorders = RECORDERS.lock().unwrap();
    recorders.retain(|recorder| recorder.strong_count() > 0);
    for recorder in recorders.iter().filter_map(Weak::upgrade) {
        recorder.events.lock().unwrap().push(event.clone());
    }
}

pub(super) fn profile(session: &str, stacked: bool, reentrant: bool) {
    if !recording() {
        return;
    }
    record(Event::Profile {
        session: session.to_owned(),
        stacked,
        reentrant,
    });
}

pub(super) fn marked(session: &str, id: &str) {
    if !recording() {
        return;
    }
    record(Event::Marked {
        session: session.to_owned(),
        id: id.to_owned(),
    });
}

pub(super) fn answered(session: &str, id: &str) {
    if !recording() {
        return;
    }
    record(Event::Answered {
        session: session.to_owned(),
        id: id.to_owned(),
    });
}

pub(super) fn returned(session: &str, id: &str) {
    if !recording() {
        return;
    }
    record(Event::Returned {
        session: session.to_owned(),
        id: id.to_owned(),
    });
}

pub(super) fn ended(session: &str) {
    if !recording() {
        return;
    }
    record(Event::Ended {
        session: session.to_owned(),
    });
}

/// A call this side sends was recorded as a hop.
#[allow(clippy::too_many_arguments)]
pub(super) fn forward_hop(
    dst: &str,
    id: &str,
    seq: u64,
    path: &[String],
    synchronous: bool,
    hop: &Hop,
) {
    if !recording() {
        return;
    }
    record(Event::Forward(Forward {
        dst: dst.to_owned(),
        id: id.to_owned(),
        seq,
        path: path.to_vec(),
        synchronous,
        decision: Decision::Hop {
            src: (!hop.src.is_empty()).then(|| (hop.src.clone(), hop.sid.clone())),
            entry: hop.entry,
        },
    }));
}

/// A call this side sends was not recorded: it is back on its caller's own
/// chain, which runs it.
pub(super) fn forward_same_chain(
    dst: &str,
    id: &str,
    seq: u64,
    path: &[String],
    synchronous: bool,
    caller: (&str, &str),
) {
    if !recording() {
        return;
    }
    record(Event::Forward(Forward {
        dst: dst.to_owned(),
        id: id.to_owned(),
        seq,
        path: path.to_vec(),
        synchronous,
        decision: Decision::SameChain {
            caller: (caller.0.to_owned(), caller.1.to_owned()),
        },
    }));
}

/// A call this side sends was not recorded: nobody waits for it.
pub(super) fn forward_not_waited(
    dst: &str,
    id: &str,
    seq: u64,
    path: &[String],
    synchronous: bool,
    why: &'static str,
) {
    if !recording() {
        return;
    }
    record(Event::Forward(Forward {
        dst: dst.to_owned(),
        id: id.to_owned(),
        seq,
        path: path.to_vec(),
        synchronous,
        decision: Decision::NotWaited { why },
    }));
}

/// A call this side sends was refused: it would have waited on itself.
pub(super) fn forward_refused(
    dst: &str,
    id: &str,
    seq: u64,
    path: &[String],
    synchronous: bool,
    calls: &[String],
) {
    if !recording() {
        return;
    }
    record(Event::Forward(Forward {
        dst: dst.to_owned(),
        id: id.to_owned(),
        seq,
        path: path.to_vec(),
        synchronous,
        decision: Decision::Refused {
            calls: calls.to_vec(),
        },
    }));
}
