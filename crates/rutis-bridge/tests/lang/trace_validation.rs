//! Trace validation for the session channel (#624, `docs/formal-trace.md`
//! §2, §3.1–§3.8, §4): an execution is a trace, and a trace is allowed only
//! if every event in it is one the previous events allow.
//!
//! Three sequences are checked here.
//!
//! - The channel trace `channel::trace` writes: direction, length and how a
//!   channel ended, never its bytes. A close ends both directions of this
//!   end; a far end that ended its stream is not followed by another
//!   message; a failed send ends sending; a failed receive ends the message
//!   stream.
//! - The frames themselves, decoded by the fixture that wraps a channel:
//!   a greeting first and once, protocol 5, no frame before it; a call id
//!   carrying the prefix of the session that issued it, and each session
//!   numbering its own calls (a replacement numbers them from 1 again); a
//!   reply only for a call the same session's far end opened, at most once;
//!   a cancel for a call this side does not have is the one dangling name
//!   allowed (it may cross the reply it cancels).
//! - A lease's steps beside the channels, recorded by the fixture: the
//!   newer handshake, the old session's unload, the newer session's install
//!   and a stale command's refusal; the old channel closes as replaced
//!   before its far plugins are unloaded, and they are unloaded before the
//!   newer session mounts.
//!
//! What frames grant and carry is checked with them:
//!
//! - §3.3, references: a `call`/`get`/`await` names a reference the far end
//!   granted and has not seen fully released; a `release` counts against
//!   those grants and no further; a granted identity is one a session
//!   allocates (1 to 2^53−1); a reference sent home names one its session
//!   granted. Each is a session-level fault when broken, not a call's.
//! - §3.5, failures: a `throw` carries a name and a message, and a graph
//!   whose root names a node — the code and data a program decides by. A
//!   graph that does not resolve is taken as `bad graph` and fails only its
//!   call; the frame stays in the trace, the session goes on.
//! - §3.6, limits: a frame is within the channel's message limit, because a
//!   session checks before writing it. So an over-limit value fails its
//!   call and leaves no frame; a message over the limit that reaches the
//!   carrier ends the channel, which the channel trace shows. The two are
//!   different results, and the trace records only the size either way.
//! - §3.8, replacement: a replaced channel closes with `replaced` and its
//!   session's ending is retryable; a rejected identity and an
//!   incompatible version are not. A replacement is a new session, its
//!   calls numbered from 1 again; a reply on it for an id the session it
//!   replaced opened is not a reply in it. What ends a session is
//!   classified by the error's kind, never by its text
//!   (`crates/rutis-host/src/connect/session.rs:56-61`).
//! - §3.8, leases (`specs/remote-lease/RemoteLease.tla`: `OneLease`,
//!   `OldCommandsStayOld`, `CleanBeforeHello`): a newer connection's
//!   handshake completes while the old lease still serves; the old session
//!   then unloads, its far plugins with it, its channel closes as
//!   `replaced`, and only then does the newer session mount. A command of
//!   the old lease after that is refused before it crosses. The host's
//!   listener that makes this order (`crates/rutis-host/src/connect/
//!   listen.rs:4-10`) is in another crate; the fixture mounts the sessions
//!   in that order and the lease events it records are checked.
//! - §3.8, a host gone: the channel ends normally, the call in flight
//!   fails with a transport error (code `peer-gone`), the session ends with
//!   `peer disconnected`, and no frame follows the far end's end. A far end
//!   that is a process of the test's own is killed with `SIGKILL` instead:
//!   the channel's end then says how the process ended (a receive failure,
//!   not `ended`) and the call fails with that same text, which is the
//!   difference a normal close and a kill leave.
//! - §3.4, synchronous waits: a forward into a runtime whose waits would
//!   close a cycle — chain, put aside or stack order (`crates/rutis-bridge/
//!   src/session/rpc/waits.rs:1-23`) — is not sent, and its caller gets
//!   `SyncWaitCycle`. What a channel trace does not carry of the runtime's
//!   stack, the far end's frames do: its greeting declares `sync-stack` and
//!   `reentrant-sync`, and each call of its names the call it runs under.
//! - §3.8, an access point: a greeting names the end it is on — a
//!   connection greeting as another endpoint is not an admitted session; a
//!   revoked identity is closed with `rejected` (4003) after its hello and
//!   ends as an identity refusal, nothing crossing after it. The old
//!   `AccessEvent` log was removed with #695 and today's listener is in
//!   `rutis-host`, so these are the endings this crate sees of an
//!   admission, not the point's own log.
//!
//! Every allowed trace here was produced by a real channel or session, or
//! is the literal sequence of a replacement, whose two channels are
//! checked apart, as are the repeated close and the repeated end no run
//! here makes. A lease's trace is the channels' lines plus the fixture's
//! steps beside them (a handshake, an unload, an install), which is how the
//! host's order becomes visible without a channel event for it. Every
//! forbidden one is that same trace with one event changed, or a literal
//! sequence. The product trace stays size-only: the fixture decodes frames,
//! the trace does not, because messages carry configuration. Each forbidden
//! case is one this file's rule for it rejects — the channel's own where a
//! line is the fault — and where a case names a rule, deleting that rule
//! accepts it.
//!
//! Not checked here, and gaps of #624: the outbound backlog limit
//! (`SessionOptions::outbound_limit`: only a host makes a session with it,
//! `Connection::open_with` is not public), which reference kinds a frame may
//! name (a `get` of a function is the far end's own refusal), the access
//! point's own log (`specs/remote-lease/AccessPoint.tla`; removed with
//! #695), the guard that a call a runtime has started is not put aside (the
//! graph keeps it, no run here needs it), and the host's real listener (its
//! order is the fixture's). 4002 and 4003 are checked through a memory
//! channel, whose `Close` maps them; the WebSocket close codes themselves
//! are `crates/rutis-bridge/tests/websocket.rs` (feature `websocket`, not
//! built here). The retry classes are the ones
//! `crates/rutis-host/src/connect/session.rs:56-61` and the Go dialer
//! (`go/rutis/internal/runtime/dial.go`, `sessionFailure`) decide by; this
//! file checks the endings those classes are made from.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::Ctx;
use rutis_bridge::channel::trace::{trace, Sink};
use rutis_bridge::channel::{Channel, ChannelError, Close, Closer, PeerId, Receiver, Sender};
use rutis_bridge::session::{
    call_service, host_key, Connection, Dispatch, Endpoint, Error, Handshake, HostDispatch, Reply,
    Value, ENDPOINT_PROTOCOL,
};
use rutis_bridge::{LocalId, Session};
use rutis_test_support::{eventually, until, within};
use serde_json::{json, Value as Json};

fn id(name: &str) -> PeerId {
    PeerId::new(name).unwrap()
}

// ---------------------------------------------------------------- the lines

/// What one run recorded, in the order its events happened: two channels on
/// one host write to one log, so their lines interleave and the validator
/// has to keep them apart by label.
///
/// A line is filed where its event was, not where its thread happened to
/// reach the log. `trace` writes a send's or a receive's line after the
/// message crossed, and a close's before the close happens, so the thread
/// that closes can reach the log first: an allowed trace would then read as
/// a message crossing a closed channel (#624).
///
/// `T` is a line of a channel's trace; a run that records steps beside them
/// (`Lease`) keeps the two in one order through the same tickets.
struct Records<T> {
    /// What the run filed, by ticket.
    lines: Arc<Mutex<Lines<T>>>,
    /// Lines that have taken their place and are not written yet: a run read
    /// before one of them is written is read before its own last line.
    writing: Arc<AtomicUsize>,
}

/// The trace lines of a run of channels, as `channel::trace` writes them.
type Log = Records<String>;

struct Lines<T> {
    /// The next ticket to hand out.
    next: u64,
    /// What each ticket holds, in ticket order, which is the order the run's
    /// events happened in.
    filed: BTreeMap<u64, Slot<T>>,
}

/// One ticket's lines: the line of the event that took it, and, for a close,
/// the lines of what it ran into.
///
/// A close is not one instant. This end closes its receiving before its
/// sending (`crates/rutis-bridge/src/memory/mod.rs:145-154`) and `trace`
/// writes the close's line before either, so while it happens a message can
/// still cross, and a receive can already fail. What crossed came before the
/// channel ended and belongs before the close's line; what failed did so
/// because it was closing and belongs after it.
struct Slot<T> {
    /// What crossed or came in while the close happened.
    before: Vec<T>,
    /// The event's line.
    line: Option<T>,
    /// What failed while the close happened.
    after: Vec<T>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            before: Vec::new(),
            line: None,
            after: Vec::new(),
        }
    }
}

impl<T> Default for Lines<T> {
    fn default() -> Self {
        Self {
            next: 0,
            filed: BTreeMap::new(),
        }
    }
}

impl<T> Default for Records<T> {
    fn default() -> Self {
        Self {
            lines: Arc::default(),
            writing: Arc::default(),
        }
    }
}

impl<T> Clone for Records<T> {
    fn clone(&self) -> Self {
        Self {
            lines: self.lines.clone(),
            writing: self.writing.clone(),
        }
    }
}

/// Where the line `trace` is about to write for an operation goes: at the
/// ticket its event took, or beside the line of the close it ran into
/// (`Slot`). `At` is the close's own line.
#[derive(Clone, Copy)]
enum Pending {
    At(u64),
    Crossed(u64),
    Failed(u64),
}

impl Pending {
    fn ticket(self) -> u64 {
        match self {
            Self::At(at) | Self::Crossed(at) | Self::Failed(at) => at,
        }
    }
}

impl<T> Records<T> {
    /// A fresh ticket: where an event that takes one now stands.
    fn ticket(&self) -> u64 {
        let mut lines = self.lines.lock().unwrap();
        let at = lines.next;
        lines.next += 1;
        at
    }

    /// Take the place of the line `trace` writes for this operation next: a
    /// send's or a receive's event, which the sink files where it happened,
    /// or a close's, which is where the close's line goes. The line counts as
    /// being written until the sink has filed it (`entries`).
    fn hold(&self, pending: Pending) {
        self.writing.fetch_add(1, Ordering::SeqCst);
        PLACE.with(|held| held.set(Some(pending)));
    }

    /// The ticket of a close that starts now, and the record of it for the
    /// operations it may run into: the first close is the channel's end, and
    /// a later one (`one_reason` closes as often as it is asked) says the
    /// same thing again.
    fn closing(&self, closed: &Closed) -> u64 {
        let at = self.ticket();
        closed.ran(at);
        at
    }

    /// File the line of `pending` where its event was: at the ticket it took,
    /// or beside the close's line when it ran into one. A message that
    /// crossed before a close goes before the close's line, because the
    /// channel had not ended; a failure goes after it, because it is the
    /// close's. What took its ticket before the close's line stands where it
    /// took it, which is before that line already.
    fn file(&self, pending: Pending, closed: &Closed, entry: T) {
        let mut lines = self.lines.lock().unwrap();
        let at = pending.ticket();
        // The close this operation ran into, if it took its ticket after the
        // close's line: it began while the close was happening.
        let ran_into = closed.ran_into().filter(|close| at > *close);
        let slot = match ran_into {
            Some(close) => lines.filed.entry(close).or_default(),
            None => lines.filed.entry(at).or_default(),
        };
        match (pending, ran_into) {
            (Pending::At(_), _) | (Pending::Crossed(_) | Pending::Failed(_), None) => {
                slot.line = Some(entry)
            }
            (Pending::Crossed(_), Some(_)) => slot.before.push(entry),
            (Pending::Failed(_), Some(_)) => slot.after.push(entry),
        }
    }

    /// A step this run takes between its lines, filed where it was.
    fn note(&self, entry: T) {
        let at = self.ticket();
        let mut lines = self.lines.lock().unwrap();
        lines.filed.entry(at).or_default().line = Some(entry);
    }
}

impl<T: Clone> Records<T> {
    /// What the run recorded, in the order its events happened, once the lines
    /// being written are in it. A run whose channels are done has none; the
    /// wait is bounded so that a reader on a thread that must also run the
    /// writer cannot hang a test, and an operation still in a channel has no
    /// line yet either way.
    fn entries(&self) -> Vec<T> {
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.writing.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let lines = self.lines.lock().unwrap();
        let mut entries = Vec::new();
        for slot in lines.filed.values() {
            entries.extend(slot.before.iter().cloned());
            entries.extend(slot.line.iter().cloned());
            entries.extend(slot.after.iter().cloned());
        }
        entries
    }
}

impl<T: From<String> + Send + 'static> Records<T> {
    /// Where the lines of a channel traced with `trace` go: filed where the
    /// operation that took the place reported its event (`Pending`).
    fn sink(&self, closed: Arc<Closed>) -> Sink {
        let log = self.clone();
        Arc::new(move |line: &str| {
            let pending = PLACE.with(|held| {
                held.take()
                    .expect("a traced channel writes its line after it takes its place")
            });
            log.file(pending, &closed, T::from(line.to_owned()));
            log.writing.fetch_sub(1, Ordering::SeqCst);
        })
    }
}

impl Records<String> {
    /// The trace lines of the run, in the order their events happened.
    fn lines(&self) -> Vec<String> {
        self.entries()
    }
}

thread_local! {
    /// The place of the line `trace` is about to write for this thread's
    /// operation. A channel's sender and receiver are used from one thread
    /// each (`channel/mod.rs`) and its closer from anywhere, so the operation
    /// a thread is in is the one whose line comes next. A run has one log, so
    /// a place taken for it is the one its sink reads next.
    static PLACE: Cell<Option<Pending>> = const { Cell::new(None) };
}

/// One channel's close, as the ticket its line took (`u64::MAX` while it is
/// open): the first one, which is where the channel ended. An operation whose
/// line comes after it has one ran into it (`Records::file`).
struct Closed(AtomicU64);

impl Default for Closed {
    fn default() -> Self {
        Self(AtomicU64::new(u64::MAX))
    }
}

impl Closed {
    fn ran(&self, at: u64) {
        let _ = self
            .0
            .compare_exchange(u64::MAX, at, Ordering::SeqCst, Ordering::SeqCst);
    }

    fn ran_into(&self) -> Option<u64> {
        let at = self.0.load(Ordering::SeqCst);
        (at != u64::MAX).then_some(at)
    }
}

/// One event of `docs/formal-trace.md` §2.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Sent(usize),
    Received(usize),
    Ended,
    SendFailed(String),
    ReceiveFailed(String),
    ClosedDone,
    ClosedReplaced,
    ClosedRejected(String),
}

struct Line {
    label: String,
    event: Event,
}

/// `<label>: <event>`: the label is up to the first `: ` that leaves an
/// event the format has; a reason may itself hold `: `, which is why the
/// label is not split off by the last one.
fn parse_line(line: &str) -> Result<Line, String> {
    for (at, _) in line.match_indices(": ") {
        if at == 0 {
            continue;
        }
        if let Some(event) = parse_event(&line[at + 2..]) {
            return Ok(Line {
                label: line[..at].to_owned(),
                event,
            });
        }
    }
    Err(format!(
        "{line:?} is not a line of the trace format (docs/formal-trace.md §2)"
    ))
}

fn parse_event(rest: &str) -> Option<Event> {
    if let Some(bytes) = rest
        .strip_prefix("sent ")
        .and_then(|rest| rest.strip_suffix(" bytes"))
    {
        return bytes.parse().ok().map(Event::Sent);
    }
    if let Some(bytes) = rest
        .strip_prefix("received ")
        .and_then(|rest| rest.strip_suffix(" bytes"))
    {
        return bytes.parse().ok().map(Event::Received);
    }
    match rest {
        "ended" => Some(Event::Ended),
        "closed: done" => Some(Event::ClosedDone),
        "closed: replaced" => Some(Event::ClosedReplaced),
        _ => {
            if let Some(reason) = rest
                .strip_prefix("send failed: ")
                .filter(|reason| !reason.is_empty())
            {
                return Some(Event::SendFailed(reason.to_owned()));
            }
            if let Some(reason) = rest
                .strip_prefix("receive failed: ")
                .filter(|reason| !reason.is_empty())
            {
                return Some(Event::ReceiveFailed(reason.to_owned()));
            }
            rest.strip_prefix("closed: rejected: ")
                .filter(|reason| !reason.is_empty())
                .map(|reason| Event::ClosedRejected(reason.to_owned()))
        }
    }
}

/// How far one channel's trace has gone; what may follow follows from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Open,
    /// The far end finished sending (`recv` gave `Ok(None)`): no message
    /// follows its end.
    Ended,
    /// A send failed: the channel ended for sending.
    SendFailed,
    /// A receive failed: the message stream ended.
    ReceiveFailed,
    /// This end closed the channel: nothing successful crosses it after.
    Closed,
}

/// The state after `event`, or why the event cannot follow `state`.
fn after(state: State, event: &Event) -> Result<State, String> {
    use State::*;
    match (state, event) {
        (_, Event::ClosedDone | Event::ClosedReplaced | Event::ClosedRejected(_)) => Ok(Closed),
        (Closed, Event::Sent(_) | Event::Received(_) | Event::Ended) => {
            Err("this end closed the channel: nothing crosses it after the close".into())
        }
        // A send or receive already on a closed channel reports the close.
        (Closed, Event::SendFailed(_) | Event::ReceiveFailed(_)) => Ok(Closed),
        (ReceiveFailed, Event::Received(_) | Event::Ended) => {
            Err("a receive failed: the message stream ended there".into())
        }
        // The two directions can end apart: a send may still succeed.
        (ReceiveFailed, _) => Ok(ReceiveFailed),
        (SendFailed, Event::Sent(_)) => Err("a send failed: the channel ended for sending".into()),
        // What arrived before the far end's close can still be drained.
        (SendFailed, _) => Ok(SendFailed),
        (Ended, Event::Received(_)) => {
            Err("the far end ended the stream: no message can follow its end".into())
        }
        (Ended, _) => Ok(Ended),
        (Open, Event::Sent(_) | Event::Received(_)) => Ok(Open),
        (Open, Event::Ended) => Ok(Ended),
        (Open, Event::SendFailed(_)) => Ok(SendFailed),
        (Open, Event::ReceiveFailed(_)) => Ok(ReceiveFailed),
    }
}

/// A trace whose every event follows from the ones before it.
struct Checked {
    lines: Vec<Line>,
    /// Where each channel's trace ended.
    last: BTreeMap<String, State>,
}

impl Checked {
    fn channels(&self) -> usize {
        self.last.len()
    }

    /// The events of one channel, in the order they were recorded.
    fn events(&self, label: &str) -> Vec<Event> {
        self.lines
            .iter()
            .filter(|line| line.label == label)
            .map(|line| line.event.clone())
            .collect()
    }
}

/// The first event that does not follow, and the rule it breaks.
#[derive(Debug)]
struct Violation {
    /// 0-based index of the event that broke the rule.
    at: usize,
    /// What the validator read there.
    text: String,
    why: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (trace line {}): {}",
            self.text,
            self.at + 1,
            self.why
        )
    }
}

fn check_trace<S: AsRef<str>>(lines: &[S]) -> Result<Checked, Violation> {
    let mut parsed = Vec::with_capacity(lines.len());
    let mut states: BTreeMap<String, State> = BTreeMap::new();
    for (at, text) in lines.iter().enumerate() {
        let text = text.as_ref();
        let violation = |why: String| Violation {
            at,
            text: text.to_owned(),
            why,
        };
        let line = parse_line(text).map_err(violation)?;
        let state = states.get(&line.label).copied().unwrap_or(State::Open);
        let next = after(state, &line.event).map_err(violation)?;
        states.insert(line.label.clone(), next);
        parsed.push(line);
    }
    Ok(Checked {
        lines: parsed,
        last: states,
    })
}

fn accepted<S: AsRef<str>>(lines: &[S]) -> Checked {
    check_trace(lines)
        .unwrap_or_else(|violation| panic!("an allowed trace was rejected: {violation}"))
}

fn rejected<S: AsRef<str>>(lines: &[S]) -> Violation {
    match check_trace(lines) {
        Ok(_) => panic!(
            "a forbidden trace was accepted: {:?}",
            lines
                .iter()
                .map(|line| line.as_ref().to_owned())
                .collect::<Vec<_>>()
        ),
        Err(violation) => violation,
    }
}

// --------------------------------------------------------------- the frames

/// One reference a frame's value carries: granted to the far end
/// (`home: false`), or sent back to the session it belongs to
/// (`home: true`, §3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Carried {
    id: u64,
    home: bool,
}

/// One frame the fixture decoded: which end sent it, its `op`, the call or
/// value or reference it names, the endpoint and protocol of its greeting,
/// the references its value carries, the failure it throws, and its length
/// on the wire.
#[derive(Clone, Debug, PartialEq)]
struct Frame {
    from: String,
    op: String,
    id: Option<String>,
    version: Option<u64>,
    endpoint: Option<String>,
    /// The capabilities a greeting declares (`sync-stack`, `reentrant-sync`,
    /// §3.2): what its waits do in the wait graph, and what it takes.
    capabilities: Vec<String>,
    /// The invocation chain a call is made under (`path`, §3.4): the calls
    /// its answer waits for, the forwarded ones tagged with their session.
    path: Vec<String>,
    /// The caller waits for this call synchronously (`sync`).
    sync: bool,
    bytes: usize,
    /// The `reference` a `call`, `get`, `await` or `release` names.
    reference: Option<u64>,
    /// The `count` a `release` says.
    count: Option<u64>,
    /// References the frame's `args` or `value` carries (§3.3).
    carries: Vec<Carried>,
    /// The failure a `throw` carries (§3.5).
    failure: Option<Failure>,
    /// Why the frame carries no value protocol 5 has, when it does not.
    malformed: Option<String>,
}

/// What a failure's `graph` is to the receiving side (`Error::code` and
/// `Error::data`, `session/mod.rs`).
#[derive(Clone, Debug, PartialEq)]
enum Graph {
    /// No graph at all: a `SyncWaitCycle`, a refusal.
    None,
    /// A graph that does not resolve: not `{root: {type: "reference",
    /// value: n}, nodes: [...]}` with a node at `n`. A malformed graph
    /// fails only the call that got it (§3.5), never the session.
    Bad,
    /// A graph whose root names a node, with the code and data it says.
    Node {
        code: Option<String>,
        data: Option<Json>,
    },
}

/// The failure a `throw` carries, read as the receiving side reads it.
#[derive(Clone, Debug, PartialEq)]
struct Failure {
    name: String,
    message: String,
    graph: Graph,
}

impl Failure {
    /// The code a program decides by: the code of the node the graph's root
    /// names, `None` for one without.
    fn code(&self) -> Option<&str> {
        match &self.graph {
            Graph::Node { code, .. } => code.as_deref(),
            Graph::None | Graph::Bad => None,
        }
    }

    /// The data of that node.
    fn data(&self) -> Option<&Json> {
        match &self.graph {
            Graph::Node { data, .. } => data.as_ref(),
            _ => None,
        }
    }
}

/// Frames decoded by the fixture, in the order the readers saw them. The
/// order across the two streams is theirs; each stream keeps its own.
#[derive(Clone, Default)]
struct Decoded(Arc<Mutex<Vec<Frame>>>);

impl Decoded {
    fn push(&self, frame: Frame) {
        self.0.lock().unwrap().push(frame);
    }

    fn all(&self) -> Vec<Frame> {
        self.0.lock().unwrap().clone()
    }

    /// What `end` sent, in its order.
    fn from(&self, end: &str) -> Vec<Frame> {
        self.all()
            .into_iter()
            .filter(|frame| frame.from == end)
            .collect()
    }
}

/// A receiver that decodes what the far end sent before handing it on. Only
/// a fixture does this; `channel::trace` records sizes and never bytes.
struct Decoding {
    inner: Box<dyn Receiver>,
    /// The end that sent what this receiver reads.
    from: &'static str,
    frames: Decoded,
}

impl Receiver for Decoding {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        let message = self.inner.recv()?;
        if let Some(message) = &message {
            self.frames.push(decode(self.from, message));
        }
        Ok(message)
    }
}

fn decode(from: &str, message: &[u8]) -> Frame {
    let json: Json = serde_json::from_slice(message).unwrap_or(Json::Null);
    let mut frame = Frame {
        from: from.to_owned(),
        op: json
            .get("op")
            .and_then(Json::as_str)
            .unwrap_or("<not a frame>")
            .to_owned(),
        id: json.get("id").and_then(Json::as_str).map(str::to_owned),
        version: json.get("version").and_then(Json::as_u64),
        endpoint: json
            .get("endpoint")
            .and_then(Json::as_str)
            .map(str::to_owned),
        capabilities: json
            .get("capabilities")
            .and_then(Json::as_array)
            .map(|capabilities| {
                capabilities
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        path: json
            .get("path")
            .and_then(Json::as_array)
            .map(|path| {
                path.iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        sync: json.get("sync").and_then(Json::as_bool).unwrap_or(false),
        bytes: message.len(),
        reference: json.get("reference").and_then(Json::as_u64),
        count: json.get("count").and_then(Json::as_u64),
        carries: Vec::new(),
        failure: None,
        malformed: None,
    };
    // The references a frame's value carries, and the failure it throws.
    for field in ["args", "value"] {
        let Some(value) = json.get(field) else {
            continue;
        };
        if let Err(why) = carried(value, &mut frame.carries) {
            frame.malformed = Some(why);
            return frame;
        }
    }
    if frame.op == "throw" {
        match json.get("error") {
            Some(error) => match failure_of(error) {
                Some(failure) => frame.failure = Some(failure),
                None => {
                    frame.malformed = Some(format!(
                        "a throw carries a failure: a name and a message, not {error}"
                    ))
                }
            },
            None => frame.malformed = Some("a throw carries no error".into()),
        }
    }
    frame
}

/// The references a wire value carries, into `out`. A value of data holds
/// none, whatever it looks like: a reference-shaped object inside `data` is
/// data.
fn carried(value: &Json, out: &mut Vec<Carried>) -> Result<(), String> {
    let tag = value
        .get("type")
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{value} is not a tagged value"))?;
    match tag {
        "reference" => {
            let reference = value
                .get("value")
                .ok_or_else(|| format!("{value} is a reference without its value"))?;
            let id = reference
                .get("id")
                .and_then(Json::as_u64)
                .ok_or_else(|| format!("{value} names no reference id"))?;
            let home = reference
                .get("home")
                .and_then(Json::as_bool)
                .ok_or_else(|| format!("{value} does not say whether it goes home"))?;
            out.push(Carried { id, home });
            Ok(())
        }
        "list" | "set" => {
            let Some(items) = value.get("value").and_then(Json::as_array) else {
                return Err(format!("{value} is a {tag} without items"));
            };
            for item in items {
                carried(item, out)?;
            }
            Ok(())
        }
        "map" => {
            let Some(entries) = value.get("value").and_then(Json::as_array) else {
                return Err(format!("{value} is a map without entries"));
            };
            for entry in entries {
                let pair = entry
                    .as_array()
                    .filter(|pair| pair.len() == 2)
                    .ok_or_else(|| format!("{entry} is not a map entry"))?;
                carried(&pair[0], out)?;
                carried(&pair[1], out)?;
            }
            Ok(())
        }
        "record" => {
            let Some(fields) = value.get("value").and_then(Json::as_object) else {
                return Err(format!("{value} is a record without fields"));
            };
            for field in fields.values() {
                carried(field, out)?;
            }
            Ok(())
        }
        // Data, undefined, a signal and the tagged values carry no
        // references.
        _ => Ok(()),
    }
}

/// The failure `error` (a `throw`'s `error`) carries, or `None` when it is
/// no failure: a name and a message are what make it one.
fn failure_of(error: &Json) -> Option<Failure> {
    let name = error.get("name")?.as_str()?.to_owned();
    let message = error.get("message")?.as_str()?.to_owned();
    let graph = match error.get("graph") {
        None | Some(Json::Null) => Graph::None,
        Some(graph) => match graph_root(graph) {
            Some(node) => Graph::Node {
                code: node.get("code").and_then(Json::as_str).map(str::to_owned),
                data: node.get("data").and_then(|data| graph_value(graph, data)),
            },
            None => Graph::Bad,
        },
    };
    Some(Failure {
        name,
        message,
        graph,
    })
}

/// The node a graph's root names (`session/mod.rs` `graph_root`).
fn graph_root(graph: &Json) -> Option<&serde_json::Map<String, Json>> {
    let root = graph.get("root")?;
    if root.get("type")?.as_str()? != "reference" {
        return None;
    }
    graph
        .get("nodes")?
        .get(root.get("value")?.as_u64()? as usize)?
        .as_object()
}

/// A value of an error graph as JSON (`session/mod.rs` `graph_value`).
fn graph_value(graph: &Json, value: &Json) -> Option<Json> {
    graph_value_at(graph, value, 0)
}

fn graph_value_at(graph: &Json, value: &Json, depth: usize) -> Option<Json> {
    if depth > 64 {
        return None;
    }
    match value.get("type")?.as_str()? {
        "undefined" => Some(Json::Null),
        "data" => value.get("value").cloned(),
        "bigint" | "number" => {
            let text = value.get("value")?.as_str()?;
            Some(
                text.parse::<serde_json::Number>()
                    .map(Json::Number)
                    .unwrap_or_else(|_| Json::String(text.to_owned())),
            )
        }
        "reference" => {
            let node = graph
                .get("nodes")?
                .get(value.get("value")?.as_u64()? as usize)?;
            match node.get("type")?.as_str()? {
                "array" => node
                    .get("values")?
                    .as_array()?
                    .iter()
                    .map(|item| graph_value_at(graph, item, depth + 1))
                    .collect::<Option<Vec<_>>>()
                    .map(Json::Array),
                "object" => node
                    .get("values")?
                    .as_array()?
                    .iter()
                    .map(|entry| {
                        let key = entry.get(0)?.as_str()?.to_owned();
                        Some((key, graph_value_at(graph, entry.get(1)?, depth + 1)?))
                    })
                    .collect::<Option<serde_json::Map<_, _>>>()
                    .map(Json::Object),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `channel` traced as `label`, each line of it filed where its event
/// happened (`Records`).
fn traced_channel<T: From<String> + Send + 'static>(
    channel: Channel,
    label: &str,
    log: &Records<T>,
) -> Channel {
    let closed = Arc::new(Closed::default());
    // Inside the trace, so that a send's ticket is taken before the message
    // crosses and a receive's after the message comes in: their lines are
    // written after they come back from the channel.
    let traced = trace(
        Channel {
            sender: Box::new(Ticket {
                inner: channel.sender,
                log: log.clone(),
                closed: closed.clone(),
            }),
            receiver: Box::new(Ticket {
                inner: channel.receiver,
                log: log.clone(),
                closed: closed.clone(),
            }),
            closer: channel.closer,
        },
        label,
        log.sink(closed.clone()),
    );
    // A close's line is written before the close happens, so its ticket is
    // taken before the trace runs: outside it.
    Channel {
        sender: traced.sender,
        receiver: traced.receiver,
        closer: Arc::new(Ticket {
            inner: traced.closer,
            log: log.clone(),
            closed,
        }),
    }
}

/// One operation of a traced channel: it takes the ticket its line is filed
/// under, and hands the line to the log where its event was. A close takes
/// its ticket before it runs, which is where its line goes; a send takes one
/// before the message crosses and a receive one as the message comes in, and
/// either goes beside the close's line if a close ran into it (`Records`).
struct Ticket<T, L> {
    inner: T,
    log: Records<L>,
    closed: Arc<Closed>,
}

impl<L: Send + 'static> Sender for Ticket<Box<dyn Sender>, L> {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        // Where the send is, taken before the message crosses: its line has
        // to stand before anything that saw it cross, whatever this thread
        // does when the channel returns it.
        let at = self.log.ticket();
        let result = self.inner.send(message);
        let pending = match result.is_ok() {
            true => Pending::Crossed(at),
            false => Pending::Failed(at),
        };
        self.log.hold(pending);
        result
    }

    fn max_message(&self) -> Option<usize> {
        self.inner.max_message()
    }
}

impl<L: Send + 'static> Receiver for Ticket<Box<dyn Receiver>, L> {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        let result = self.inner.recv();
        // Where the message came in, taken as it comes back: a receive that
        // waits has no event to stand at until it has one.
        let at = self.log.ticket();
        let pending = match result.is_ok() {
            true => Pending::Crossed(at),
            false => Pending::Failed(at),
        };
        self.log.hold(pending);
        result
    }
}

impl<L: Send + 'static> Closer for Ticket<Arc<dyn Closer>, L> {
    fn close(&self, why: Close) {
        let at = self.log.closing(&self.closed);
        self.log.hold(Pending::At(at));
        self.inner.close(why);
    }
}

/// `channel` traced as `label`, its receiver decoding what the far end
/// (`from`) sends.
fn instrumented(
    channel: Channel,
    label: &'static str,
    from: &'static str,
    log: &Log,
    frames: &Decoded,
) -> Channel {
    let mut channel = traced_channel(channel, label, log);
    channel.receiver = Box::new(Decoding {
        inner: channel.receiver,
        from,
        frames: frames.clone(),
    });
    channel
}

/// One frame a scripted far end read.
fn read_frame(channel: &mut Channel) -> Json {
    let message = channel
        .receiver
        .recv()
        .expect("the far end reads")
        .expect("a frame, not the end");
    serde_json::from_slice(&message).expect("a frame")
}

/// One frame a scripted far end writes.
fn write_frame(channel: &mut Channel, frame: Json) {
    channel
        .sender
        .send(&serde_json::to_vec(&frame).unwrap())
        .expect("the far end writes");
}

const CALL_OPS: [&str; 4] = ["invoke", "call", "get", "await"];
const REPLY_OPS: [&str; 2] = ["return", "throw"];
/// The `Frame` vocabulary of protocol 5 (`session/protocol.rs`).
const FRAME_OPS: [&str; 10] = [
    "hello", "invoke", "call", "get", "await", "return", "throw", "release", "cancel", "ack",
];
/// The call ops that name a reference of the far end (§3.3).
const REFERENCE_CALL_OPS: [&str; 3] = ["call", "get", "await"];
/// The largest reference id a session allocates (2^53−1): beyond it, a
/// number cannot cross as one (`session/rpc.rs` `decode`, "invalid
/// reference identity").
const MAX_REFERENCE: u64 = 9_007_199_254_740_991;

/// The two ends of one session, as the fixture labelled them: the labels
/// that greet each other. A session's call ids are its own, so a
/// replacement session numbers its calls from 1 again (§3.8).
type Ends<'a> = (&'a str, &'a str);

/// The other end of the session `end` is in.
fn peer_of<'a>(sessions: &[Ends<'a>], end: &str) -> Option<&'a str> {
    sessions.iter().find_map(|(a, b)| {
        if *a == end {
            Some(*b)
        } else if *b == end {
            Some(*a)
        } else {
            None
        }
    })
}

/// One call a frame opened.
struct Call {
    /// Whether a reply settled it.
    settled: bool,
    /// The failure the reply carried, when it was a `throw`.
    failure: Option<Failure>,
}

struct FrameChecked {
    /// Calls a call frame opened, by the end that opened it and its id.
    calls: BTreeMap<(String, String), Call>,
    /// Grants outstanding, by the end that granted and the reference id.
    outstanding: BTreeMap<(String, u64), u64>,
}

impl FrameChecked {
    fn unanswered(&self) -> usize {
        self.calls.values().filter(|call| !call.settled).count()
    }

    /// How many of `id` the end `owner` granted and has not seen released.
    fn outstanding(&self, owner: &str, id: u64) -> u64 {
        self.outstanding
            .get(&(owner.to_owned(), id))
            .copied()
            .unwrap_or(0)
    }

    /// The failure that settled the call `id`, when `issuer` opened it and
    /// the reply was a `throw` (§3.5).
    fn failure(&self, issuer: &str, id: &str) -> Option<&Failure> {
        self.calls
            .get(&(issuer.to_owned(), id.to_owned()))?
            .failure
            .as_ref()
    }
}

/// Check a run's frames in the order they were recorded: a greeting first
/// and once, a call id carrying the prefix of the session that issued it,
/// every reply settling a call the same session's far end opened, what
/// frames grant and release (§3.3), the failure a `throw` carries (§3.5),
/// and that no frame is over the channel's `limit` (§3.6).
fn check_frames(
    frames: &[Frame],
    sessions: &[Ends<'_>],
    limit: Option<usize>,
) -> Result<FrameChecked, Violation> {
    let mut greeted: BTreeSet<String> = BTreeSet::new();
    let mut endpoints: BTreeMap<String, String> = BTreeMap::new();
    let mut calls: BTreeMap<(String, String), Call> = BTreeMap::new();
    let mut granted: BTreeMap<(String, u64), u64> = BTreeMap::new();
    for (at, frame) in frames.iter().enumerate() {
        let violation = |why: String| Violation {
            at,
            text: format!("{} sent `{}`", frame.from, frame.op),
            why,
        };
        let peer = peer_of(sessions, &frame.from)
            .ok_or_else(|| violation(format!("no session here has an end {:?}", frame.from)))?
            .to_owned();
        if let Some(why) = &frame.malformed {
            return Err(violation(why.clone()));
        }
        if !FRAME_OPS.contains(&frame.op.as_str()) {
            return Err(violation(format!(
                "`{}` is not an op of protocol {ENDPOINT_PROTOCOL}",
                frame.op
            )));
        }
        // §3.6: a session checks a frame against the channel's limit before
        // writing it, so a frame the channel carried is within it; one over
        // it ends the channel, which is not how a value fails its call.
        if let Some(limit) = limit {
            if frame.bytes > limit {
                return Err(violation(format!(
                    "a frame of {} bytes is over the channel's limit of {limit} bytes: an \
                     over-limit value fails its call before the frame is written, so no frame \
                     over the limit is in an allowed trace (#675)",
                    frame.bytes
                )));
            }
        }
        if frame.op == "hello" {
            if !greeted.insert(frame.from.clone()) {
                return Err(violation("a session greets once".into()));
            }
            if frame.version != Some(ENDPOINT_PROTOCOL.into()) {
                return Err(violation(format!(
                    "a greeting speaks protocol {ENDPOINT_PROTOCOL}, not {:?}",
                    frame.version
                )));
            }
            if let Some(endpoint) = &frame.endpoint {
                endpoints.insert(frame.from.clone(), endpoint.clone());
            }
            continue;
        }
        // §4: no frame before the greeting.
        if !greeted.contains(&frame.from) {
            return Err(violation(
                "a frame reached the session before its greeting".into(),
            ));
        }
        if CALL_OPS.contains(&frame.op.as_str()) {
            let id = frame
                .id
                .as_ref()
                .ok_or_else(|| violation("a call names no id".into()))?;
            // §4 identity: the id carries the prefix of the session that
            // issued the call, so an id of another session cannot name it.
            if let Some(endpoint) = endpoints.get(&frame.from) {
                let prefix = format!("{endpoint}:");
                if !id.starts_with(&prefix) {
                    return Err(violation(format!(
                        "the call id {id:?} does not carry the issuer's prefix {prefix:?}"
                    )));
                }
            }
            if calls
                .insert(
                    (frame.from.clone(), id.clone()),
                    Call {
                        settled: false,
                        failure: None,
                    },
                )
                .is_some()
            {
                return Err(violation(format!(
                    "the call id {id:?} is opened twice in this session"
                )));
            }
        }
        if REPLY_OPS.contains(&frame.op.as_str()) {
            let id = frame
                .id
                .as_ref()
                .ok_or_else(|| violation("a reply names no id".into()))?;
            // §3.8: an id belongs to the session that issued it, so a reply
            // on the replacement session cannot settle the call of the one
            // it replaced.
            match calls.get_mut(&(peer.clone(), id.clone())) {
                None => {
                    return Err(violation(format!(
                        "a reply to the call {id:?}, which no frame of this session opened"
                    )))
                }
                Some(call) if call.settled => {
                    return Err(violation(format!("the call {id:?} is answered twice")))
                }
                Some(call) => {
                    call.settled = true;
                    call.failure = frame.failure.clone();
                }
            }
        }
        // §3.3: what the frame carries, then what it names. `cancel` is the
        // one dangling name the protocol allows (§3.1): it may cross the
        // reply it cancels, so an unknown one is not a fault.
        for carried in &frame.carries {
            if carried.id == 0 || carried.id > MAX_REFERENCE {
                return Err(violation(format!(
                    "the reference {} is no identity a session allocates (1 to {MAX_REFERENCE}): \
                     one outside it ends the session",
                    carried.id
                )));
            }
            if carried.home {
                // A reference sent home names one this side granted: the
                // object belongs to its session, and comes back to it.
                if granted
                    .get(&(peer.clone(), carried.id))
                    .copied()
                    .unwrap_or(0)
                    == 0
                {
                    return Err(violation(format!(
                        "the reference {} goes home to {peer}, which never granted it to {} (or \
                         saw it fully released)",
                        carried.id, frame.from
                    )));
                }
            } else {
                *granted.entry((frame.from.clone(), carried.id)).or_insert(0) += 1;
            }
        }
        if REFERENCE_CALL_OPS.contains(&frame.op.as_str()) {
            let reference = frame
                .reference
                .ok_or_else(|| violation("a call names no reference".into()))?;
            if granted
                .get(&(peer.clone(), reference))
                .copied()
                .unwrap_or(0)
                == 0
            {
                return Err(violation(format!(
                    "the call names the reference {reference}, which {peer} never granted to {} \
                     (or fully released): an unknown reference ends the session (§3.3)",
                    frame.from
                )));
            }
        }
        if frame.op == "release" {
            let reference = frame
                .reference
                .ok_or_else(|| violation("a release names no reference".into()))?;
            let count = frame
                .count
                .ok_or_else(|| violation("a release says no count".into()))?;
            let left = {
                let outstanding = granted.entry((peer.clone(), reference)).or_insert(0);
                if count == 0 || count > *outstanding {
                    return Err(violation(format!(
                        "a release of {count} of the reference {reference}: {peer} has \
                         {outstanding} of it granted (an invalid release count ends the session)"
                    )));
                }
                *outstanding -= count;
                *outstanding
            };
            if left == 0 {
                granted.remove(&(peer, reference));
            }
        }
    }
    Ok(FrameChecked {
        calls,
        outstanding: granted,
    })
}

fn decoded(rows: &[(&str, &str)]) -> Vec<Frame> {
    rows.iter()
        .map(|(from, text)| decode(from, text.as_bytes()))
        .collect()
}

fn checked_frames(frames: &[Frame], sessions: &[Ends<'_>], limit: Option<usize>) -> FrameChecked {
    check_frames(frames, sessions, limit)
        .unwrap_or_else(|violation| panic!("an allowed frame sequence was rejected: {violation}"))
}

fn rejected_frames(frames: &[Frame], sessions: &[Ends<'_>], limit: Option<usize>) -> Violation {
    match check_frames(frames, sessions, limit) {
        Ok(_) => panic!("a forbidden frame sequence was accepted: {frames:?}"),
        Err(violation) => violation,
    }
}

/// §3.8: an admitted session's greeting names the end it is on. `sessions`
/// labels the ends by the endpoint ids they greet with; a greeting that
/// names another end is a connection the access point did not admit under
/// that id (#624, `specs/remote-lease/AccessPoint.tla`). Labels that are
/// not endpoint ids (a replacement's `main-1`, `node-1`) are not checked.
fn check_admission(frames: &[Frame], sessions: &[Ends<'_>]) -> Result<(), Violation> {
    let is_end = |label: &str| {
        sessions
            .iter()
            .any(|(host, far)| *host == label || *far == label)
    };
    for (at, frame) in frames.iter().enumerate() {
        if frame.op != "hello" || !is_end(&frame.from) {
            continue;
        }
        if frame.endpoint.as_deref() != Some(frame.from.as_str()) {
            return Err(Violation {
                at,
                text: format!("{} sent `hello`", frame.from),
                why: format!(
                    "the greeting of {} names the endpoint {:?}: an admitted session's greeting \
                     names the end it is on",
                    frame.from, frame.endpoint
                ),
            });
        }
    }
    Ok(())
}

/// How a far end runs calls while it waits (§3.2 `sync-stack`,
/// `reentrant-sync`), as its greeting says.
#[derive(Clone, Copy, Debug, Default)]
struct Profile {
    /// One thread, one stack: a call that is not started waits for the
    /// calls the runtime has out.
    stacked: bool,
    /// Runs incoming calls while it waits.
    reentrant: bool,
}

/// The session a call id belongs to, by the prefix the issuing side puts on
/// it: `main-a:3` is session `main-a`'s host's call, `node-a:3` its far
/// end's (`session/rpc.rs`, `local_prefix` and the far end's).
fn call_session<'a>(call: &str, sessions: &[Ends<'a>]) -> Option<&'a str> {
    sessions.iter().find_map(|(host, far)| {
        let named = |label: &str| {
            call.strip_prefix(label)
                .is_some_and(|rest| rest.starts_with(':'))
        };
        (named(host) || named(far)).then_some(*host)
    })
}

/// A call the wait graph holds (§3.4, `crates/rutis-bridge/src/session/rpc/
/// waits.rs:1-23`): a forward into a session, or the entry that bounds the
/// chains made under it.
struct Hop {
    /// The id the call was sent with.
    id: String,
    /// The session that sent it: the one whose far end runs it.
    dst: String,
    /// The session that made it, when one did: the chain's last call.
    src: Option<String>,
    /// The call its chain names last: the one it was made under.
    sid: Option<String>,
    /// The ids its `path` names, the call itself not.
    chain: Vec<String>,
    /// Nobody waits for it: an asynchronous call off a marked chain.
    entry: bool,
}

/// The calls of the wait cycle the last of `hops` would close, each by id,
/// when there is one. The edges are the three the session draws
/// (`waits.rs`): chain, put aside and stack order; `profile` is a far
/// end's, by its label.
fn wait_cycle(
    hops: &[Hop],
    sessions: &[Ends<'_>],
    profile: impl Fn(&str) -> Profile,
) -> Option<Vec<String>> {
    let last = hops.len() - 1;
    let index: BTreeMap<&str, usize> = hops
        .iter()
        .enumerate()
        .map(|(at, hop)| (hop.id.as_str(), at))
        .collect();
    let entry_of = |call: &str| index.get(call).is_some_and(|&at| hops[at].entry);
    // Calls a call out of their session named: that runtime has started
    // them (a call made under them names them).
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for hop in hops.iter().filter(|hop| !hop.entry) {
        let Some(src) = hop.src.as_deref() else {
            continue;
        };
        for entry in &hop.chain {
            if call_session(entry, sessions) == Some(src) {
                named.insert(entry);
            }
        }
    }
    let started: Vec<bool> = hops
        .iter()
        .map(|hop| named.contains(hop.id.as_str()))
        .collect();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); hops.len()];
    // Chain: a call waits for the calls made under it, through the nearest
    // one; an entry bounds them (nobody waits for it).
    for (at, hop) in hops.iter().enumerate() {
        if hop.entry {
            continue;
        }
        for entry in hop.chain.iter().rev() {
            if let Some(&caller) = index.get(entry.as_str()) {
                successors[caller].push(at);
            }
            if entry_of(entry) {
                break;
            }
        }
    }
    // By session: the calls into it in order, and the calls out of it.
    let mut into: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut out: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (at, hop) in hops.iter().enumerate() {
        into.entry(hop.dst.as_str()).or_default().push(at);
        if hop.entry {
            continue;
        }
        if let Some(src) = hop.src.as_deref() {
            out.entry(src).or_default().push(at);
        }
    }
    for (session, calls) in &into {
        let Some(far) = sessions
            .iter()
            .find(|(host, _)| host == session)
            .map(|(_, far)| *far)
        else {
            continue;
        };
        let profile = profile(far);
        if !profile.stacked {
            continue;
        }
        // Put aside: a call not started in a non-reentrant runtime with
        // calls out, none of them on its chain, waits for all of them.
        if let Some(outs) = out.get(session).filter(|_| !profile.reentrant) {
            let sids: BTreeSet<&str> = outs
                .iter()
                .filter_map(|&at| hops[at].sid.as_deref())
                .collect();
            for &at in calls {
                let hop = &hops[at];
                let related = hop.chain.iter().any(|entry| {
                    call_session(entry, sessions) == Some(*session) && sids.contains(entry.as_str())
                });
                if !hop.entry && !related && !started[at] {
                    successors[at].extend(outs.iter().copied().filter(|&out| out != at));
                }
            }
        }
        // Stack order: a call waits for every later call into the same
        // runtime that started there, through the next one that did.
        let mut next_started = None;
        for &at in calls.iter().rev() {
            if let Some(later) = next_started {
                successors[at].push(later);
            }
            if started[at] {
                next_started = Some(at);
            }
        }
    }
    // Depth-first from the new call, remembering how each was reached.
    let mut from: Vec<Option<usize>> = vec![None; hops.len()];
    let mut stack = vec![last];
    let mut seen = vec![false; hops.len()];
    while let Some(at) = stack.pop() {
        for &next in &successors[at] {
            if next == last {
                let mut cycle = vec![hops[last].id.clone()];
                let mut back = Vec::new();
                let mut here = at;
                while here != last {
                    back.push(hops[here].id.clone());
                    here = from[here].expect("reached from the new call");
                }
                cycle.extend(back.into_iter().rev());
                return Some(cycle);
            }
            if !seen[next] {
                seen[next] = true;
                from[next] = Some(at);
                stack.push(next);
            }
        }
    }
    None
}

/// The first synchronous forward in `frames` that would wait on itself
/// (#228, `specs/cross-runtime-sync` policy B), with the calls recorded
/// before it as the wait graph. A forward whose chain names a marked call —
/// its caller waits for it — closes a cycle when the call it becomes waits,
/// through any of the graph's edges, for a call that waits for it; the
/// session does not send it.
///
/// The chain is the call's `path` (§3.4), the profile the far end's
/// greeting (§3.2) and a marked call one the far end sent with `sync` and
/// has not answered. What a session's ends do is all in the frames: there is
/// no event for a session ending, so a trace here ends its sessions after
/// its calls settled. The graph also keeps the guard that a call the
/// runtime has started is not put aside (`waits.rs`, `started`); no trace
/// here needs it — the runs this file records either leave a runtime's
/// calls unstarted or do not put them aside.
fn refused_waits(frames: &[Frame], sessions: &[Ends<'_>]) -> Option<Violation> {
    /// The call id a chain entry names: `s1/node:3` is session `s1`'s call
    /// `node:3`; an untagged entry is the destination session's own id.
    fn entry_id(entry: &str) -> &str {
        entry.rsplit_once('/').map_or(entry, |(_, id)| id)
    }
    let profile = |far: &str| -> Profile {
        frames
            .iter()
            .find(|frame| frame.from == far && frame.op == "hello")
            .map(|hello| Profile {
                stacked: hello.capabilities.iter().any(|name| name == "sync-stack"),
                reentrant: hello
                    .capabilities
                    .iter()
                    .any(|name| name == "reentrant-sync"),
            })
            .unwrap_or_default()
    };
    let is_far = |label: &str| sessions.iter().any(|(_, far)| *far == label);
    let is_host = |label: &str| sessions.iter().any(|(host, _)| *host == label);
    // Calls a far end sent with `sync` and has not answered: its runtime
    // waits for them, and so does everything forwarded under them.
    let mut marked: BTreeSet<String> = BTreeSet::new();
    let mut hops: Vec<Hop> = Vec::new();
    for (at, frame) in frames.iter().enumerate() {
        let Some(id) = frame.id.as_deref() else {
            continue;
        };
        if REPLY_OPS.contains(&frame.op.as_str()) {
            marked.remove(id);
            // The reply to a forward: it waits for nothing any more.
            hops.retain(|hop| hop.id != id);
            continue;
        }
        if !CALL_OPS.contains(&frame.op.as_str()) {
            continue;
        }
        // A call a far end sent into its session: its caller waits for it
        // when it says `sync`.
        if is_far(&frame.from) {
            if frame.sync {
                marked.insert(id.to_owned());
            }
            continue;
        }
        if !is_host(&frame.from) {
            continue;
        }
        // Nobody waits for a call made while nothing is marked: the
        // session records no entry for it either.
        if !frame.sync && marked.is_empty() {
            continue;
        }
        // The call it was made under, when that one's caller waits: the
        // chain's last entry, named on the wire.
        let caller = frame
            .path
            .last()
            .map(|entry| entry_id(entry))
            .filter(|call| marked.contains(*call));
        let entry = caller.is_none() && !frame.sync;
        hops.push(Hop {
            id: id.to_owned(),
            dst: frame.from.clone(),
            src: caller.and_then(|call| call_session(call, sessions).map(str::to_owned)),
            sid: caller.map(str::to_owned),
            chain: frame
                .path
                .iter()
                .map(|entry| entry_id(entry).to_owned())
                .collect(),
            entry,
        });
        if entry {
            continue;
        }
        if let Some(cycle) = wait_cycle(&hops, sessions, profile) {
            return Some(Violation {
                at,
                text: format!("{} sent `{}`", frame.from, frame.op),
                why: format!(
                    "the call {id} would wait on itself: {} → {id} (synchronous calls)",
                    cycle.join(" → ")
                ),
            });
        }
    }
    None
}

fn ops(frames: &[Frame]) -> Vec<(&str, Option<&str>)> {
    frames
        .iter()
        .map(|frame| (frame.op.as_str(), frame.id.as_deref()))
        .collect()
}

// ---------------------------------------------------- allowed channel traces

/// A memory pair, the near end traced under `label`.
fn traced(label: &str) -> (Channel, Channel, Log) {
    let (near, far) = rutis_bridge::memory::pair();
    let log = Log::default();
    (traced_channel(near, label, &log), far, log)
}

/// A normal end: the far end finishes, then this end closes. The four lines
/// are the ones `docs/formal-trace.md` §2 and §5 fix, with the 18-byte
/// secret of `channel/trace.rs`, which the trace must not carry.
#[test]
fn a_normal_end_is_an_allowed_trace() {
    let (mut near, mut far, log) = traced("memory peer mac");
    near.sender.send(b"{\"token\":\"secret\"}").unwrap();
    far.receiver.recv().unwrap().unwrap();
    far.sender.send(b"{\"token\":\"secret\"}").unwrap();
    near.receiver.recv().unwrap().unwrap();
    far.closer.close(Close::Done);
    assert_eq!(near.receiver.recv().unwrap(), None);
    near.closer.close(Close::Done);

    let lines = log.lines();
    assert_eq!(
        lines,
        [
            "memory peer mac: sent 18 bytes",
            "memory peer mac: received 18 bytes",
            "memory peer mac: ended",
            "memory peer mac: closed: done",
        ]
    );
    let checked = accepted(&lines);
    assert_eq!(checked.channels(), 1);
    assert_eq!(checked.last["memory peer mac"], State::Closed);

    // The same run with a message after the close is not a trace.
    let mut changed = lines.clone();
    changed.push("memory peer mac: sent 1 bytes".into());
    assert_eq!(rejected(&changed).at, 4);
}

/// A newer connection of the same far end: this end's receive and send both
/// report the replacement, then this end closes as replaced.
#[test]
fn a_replaced_channel_is_an_allowed_trace() {
    let (mut near, mut far, log) = traced("peer");
    near.sender.send(b"ping").unwrap();
    far.receiver.recv().unwrap().unwrap();
    far.closer.close(Close::Replaced);
    match near.receiver.recv() {
        Err(ChannelError::Closed { reason }) => assert_eq!(reason, "replaced by a new connection"),
        other => panic!("a replaced channel receives an error, got {other:?}"),
    }
    match near.sender.send(b"ping") {
        Err(ChannelError::Closed { reason }) => assert_eq!(reason, "replaced by a new connection"),
        other => panic!("a replaced channel sends an error, got {other:?}"),
    }
    near.closer.close(Close::Replaced);

    let lines = log.lines();
    assert_eq!(
        lines,
        [
            "peer: sent 4 bytes",
            "peer: receive failed: replaced by a new connection",
            "peer: send failed: replaced by a new connection",
            "peer: closed: replaced",
        ]
    );
    let checked = accepted(&lines);
    assert_eq!(checked.last["peer"], State::Closed);

    // A message where the receive failed is not a trace.
    let mut changed = lines.clone();
    changed.insert(2, "peer: received 4 bytes".into());
    assert_eq!(rejected(&changed).at, 2);
}

/// A refused identity: `Close::Rejected(reason)` reaches this end as a
/// rejected channel, with its reason, in the `receive failed` line and in
/// the `closed: rejected` line.
#[test]
fn a_rejected_channel_is_an_allowed_trace() {
    let (mut near, far, log) = traced("peer");
    far.closer.close(Close::Rejected("unknown identity".into()));
    match near.receiver.recv() {
        Err(ChannelError::Rejected { reason }) => assert_eq!(reason, "rejected: unknown identity"),
        other => panic!("a rejected channel receives a rejection, got {other:?}"),
    }
    near.closer
        .close(Close::Rejected("unknown identity".into()));

    let lines = log.lines();
    assert_eq!(
        lines,
        [
            "peer: receive failed: rejected: unknown identity",
            "peer: closed: rejected: unknown identity",
        ]
    );
    let checked = accepted(&lines);
    assert_eq!(checked.last["peer"], State::Closed);

    // The end of the stream cannot come after a failed receive.
    let mut changed = lines.clone();
    changed.insert(1, "peer: ended".into());
    assert_eq!(rejected(&changed).at, 1);
}

/// The far end gone: this end's send fails; what arrived before is drained,
/// then the stream ends.
#[test]
fn a_send_failure_is_an_allowed_trace() {
    let (mut near, far, log) = traced("peer");
    drop(far);
    assert!(near.sender.send(b"ping").is_err());
    assert_eq!(near.receiver.recv().unwrap(), None);
    near.closer.close(Close::Done);

    let lines = log.lines();
    assert_eq!(
        lines,
        [
            "peer: send failed: the channel is closed",
            "peer: ended",
            "peer: closed: done",
        ]
    );
    let checked = accepted(&lines);
    assert_eq!(checked.last["peer"], State::Closed);

    // A send cannot succeed after a failed one.
    let mut changed = lines.clone();
    changed.insert(1, "peer: sent 4 bytes".into());
    assert_eq!(rejected(&changed).at, 1);
}

/// A receive that fails ends the message stream: no message and no end can
/// follow it, only the failures and the close.
#[test]
fn a_receive_failure_is_an_allowed_trace() {
    let (mut near, far, log) = traced("peer");
    far.closer.close(Close::Rejected("bad token".into()));
    assert!(near.receiver.recv().is_err());
    near.closer.close(Close::Done);

    let lines = log.lines();
    assert_eq!(
        lines,
        [
            "peer: receive failed: rejected: bad token",
            "peer: closed: done",
        ]
    );
    accepted(&lines);

    let mut changed = lines.clone();
    changed.push("peer: received 4 bytes".into());
    assert_eq!(rejected(&changed).at, 2);
}

/// The far end finished sending, not receiving: sending after its end is
/// allowed, and so is the end reported twice (the channel contract checks a
/// receive after the end) and a close repeated (closing is idempotent).
#[test]
fn sending_after_the_fars_end_is_allowed() {
    let (mut near, far, log) = traced("peer");
    let Channel {
        sender,
        receiver,
        closer,
    } = far;
    drop(sender);
    assert_eq!(near.receiver.recv().unwrap(), None);
    near.sender.send(b"ping").unwrap();
    drop((receiver, closer));
    near.closer.close(Close::Done);

    let lines = log.lines();
    assert_eq!(
        lines,
        ["peer: ended", "peer: sent 4 bytes", "peer: closed: done",]
    );
    accepted(&lines);

    let checked = accepted(&[
        "peer: ended",
        "peer: ended",
        "peer: closed: done",
        "peer: closed: replaced",
    ]);
    assert_eq!(checked.last["peer"], State::Closed);
}

// ------------------------------------------------- allowed session and frames

struct Echo;

impl Dispatch for Echo {
    fn invoke(&self, _: &Connection, _: &str, _method: &str, args: Value) -> Reply {
        Ok(args)
    }
}

/// Two real sessions over one memory pair, both ends traced into one log,
/// every frame decoded by the fixture: the channel trace, the frames and the
/// bytes they took agree, the payload never reaches the log, and the frames
/// are ones protocol 5 allows.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_session_run_is_an_allowed_trace() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);

    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let callee = Connection::open(
        theirs,
        Arc::new(Echo),
        Endpoint::rust(id("node")).expect(id("main")),
    )
    .unwrap();
    within("the caller's greeting", caller.ready())
        .await
        .unwrap();
    within("the callee's greeting", callee.ready())
        .await
        .unwrap();
    let reply = within(
        "the call",
        caller.invoke_async("svc", "echo", Value::Data(json!({ "secret": "hunter2" }))),
    )
    .await
    .unwrap();
    assert_eq!(reply.json().unwrap(), json!({ "secret": "hunter2" }));

    caller.close(Error::Transport("done".into()));
    within("the callee ending", callee.closed()).await;
    within("the caller ending", caller.closed()).await;
    // The caller's reader reports the close it woke from as a failed
    // receive; wait for that line rather than for the close.
    until(
        || {
            log.lines()
                .iter()
                .any(|line| line == "main: receive failed: closed here")
        },
        "the caller's reader line",
    )
    .await;
    // Each end's line for the close frame it sends comes from the sending
    // thread, while the ending and its closed come from another: wait until
    // both ends have told their whole story, not for the half that arrives
    // first (#1080).
    until(
        || {
            let checked = accepted(&log.lines());
            checked.events("main").len() == 6 && checked.events("node").len() == 6
        },
        "both ends' events",
    )
    .await;

    let lines = log.lines();
    assert!(
        lines.iter().all(|line| !line.contains("hunter2")),
        "the trace carries no payload: {lines:?}"
    );
    let checked = accepted(&lines);
    assert_eq!(checked.channels(), 2, "{lines:?}");
    assert_eq!(checked.last["main"], State::Closed);
    assert_eq!(checked.last["node"], State::Closed);

    let main = frames.from("main");
    let node = frames.from("node");
    assert_eq!(
        ops(&main),
        [("hello", None), ("invoke", Some("main:1"))],
        "the caller greets, then calls"
    );
    assert_eq!(
        ops(&node),
        [("hello", None), ("return", Some("main:1"))],
        "the callee greets, then answers that call"
    );
    // §2: the trace compares direction, label and byte length; the lengths
    // here are the fixture's, frame by frame.
    assert_eq!(
        checked.events("main"),
        [
            Event::Sent(main[0].bytes),
            Event::Received(node[0].bytes),
            Event::Sent(main[1].bytes),
            Event::Received(node[1].bytes),
            Event::ClosedDone,
            Event::ReceiveFailed("closed here".into()),
        ]
    );
    assert_eq!(
        checked.events("node"),
        [
            Event::Sent(node[0].bytes),
            Event::Received(main[0].bytes),
            Event::Received(main[1].bytes),
            Event::Sent(node[1].bytes),
            Event::Ended,
            Event::ClosedDone,
        ]
    );

    let all = frames.all();
    let checked = checked_frames(&all, &[("main", "node")], None);
    assert_eq!(checked.unanswered(), 0, "every call was answered");
}

/// The greeting the real sessions below open with.
const HELLO_MAIN: &str = r#"{"op":"hello","version":5,"endpoint":"main"}"#;
// ------------------------------------------------------ §3.3, the references

/// A reference this side grants, the far end calls it, and the release that
/// counts: one callback granted twice in one call, called once, and released
/// with the count it was granted by, after which the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_reference_is_granted_by_count_called_and_released() {
    struct Callee;
    impl Dispatch for Callee {
        fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
            match method {
                "use" => {
                    let mut args = args.list()?;
                    let first = args.remove(0).reference()?;
                    let second = args.remove(0).reference()?;
                    let answer = first.call(Value::List(vec![Value::Data(json!(41))]))?;
                    // Let go of both references: one release, counting both
                    // grants of the same object (`Drop for Import`).
                    drop(second);
                    Ok(answer)
                }
                _ => Ok(Value::Data(json!("still here"))),
            }
        }
    }
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let callee = Connection::open(
        theirs,
        Arc::new(Callee),
        Endpoint::rust(id("node")).expect(id("main")),
    )
    .unwrap();
    within("the caller's greeting", caller.ready())
        .await
        .unwrap();
    within("the callee's greeting", callee.ready())
        .await
        .unwrap();

    let callback = Value::callback(|args| {
        let n = args.list()?.remove(0).json()?;
        Ok(Value::Data(json!(n.as_i64().unwrap() + 1)))
    });
    let answer = within(
        "the call granting one reference twice",
        caller.invoke_async(
            "svc",
            "use",
            Value::List(vec![callback.clone(), callback.clone()]),
        ),
    )
    .await
    .unwrap();
    assert_eq!(answer.json().unwrap(), json!(42));
    let after = within(
        "the call after the release",
        caller.invoke_async("svc", "after", Value::Undefined),
    )
    .await
    .unwrap();
    assert_eq!(after.json().unwrap(), json!("still here"));
    caller.close(Error::Transport("done".into()));
    within("the callee ending", callee.closed()).await;
    within("the caller ending", caller.closed()).await;

    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    assert_eq!(checked.last["node"], State::Closed);

    let all = frames.all();
    let checked = checked_frames(&all, &[("main", "node")], None);
    assert_eq!(checked.unanswered(), 0, "{all:?}");
    assert_eq!(
        checked.outstanding("main", 1),
        0,
        "the release cleared both grants: {all:?}"
    );

    // The frame that granted the same object twice, the call of it, and one
    // release counting both grants.
    let main = frames.from("main");
    let grant = main
        .iter()
        .find(|frame| frame.op == "invoke")
        .expect("the invoke that granted the reference");
    assert_eq!(
        grant.carries,
        [
            Carried { id: 1, home: false },
            Carried { id: 1, home: false }
        ],
        "the same object sent twice is one reference granted twice"
    );
    let node = frames.from("node");
    let call = node
        .iter()
        .find(|frame| frame.op == "call")
        .expect("the call of the granted reference");
    assert_eq!(call.reference, Some(1));
    let releases: Vec<&Frame> = node.iter().filter(|frame| frame.op == "release").collect();
    assert_eq!(releases.len(), 1, "{node:?}");
    assert_eq!(
        (releases[0].reference, releases[0].count),
        (Some(1), Some(2)),
        "one release, counting both grants"
    );
}

// ---------------------------------------------------------- §3.5, the errors

/// An error crosses with the code a program decides by: `no-method` and
/// `bad-value` reach the caller as `BindingError`s whose graph carries the
/// code and data, and each fails only its own call (§3.5).
#[tokio::test(flavor = "multi_thread")]
async fn error_graphs_carry_their_codes_across_a_session() {
    struct Throws;
    impl Dispatch for Throws {
        fn invoke(&self, _: &Connection, _: &str, method: &str, _: Value) -> Reply {
            match method {
                "tomorow" => Err(Error::no_method(
                    "weather",
                    "tomorow",
                    vec!["today", "tomorrow"],
                )),
                "sparse" => Err(Error::value("a sparse array")),
                _ => Ok(Value::Data(json!("ok"))),
            }
        }
    }
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let callee = Connection::open(
        theirs,
        Arc::new(Throws),
        Endpoint::rust(id("node")).expect(id("main")),
    )
    .unwrap();
    within("the caller's greeting", caller.ready())
        .await
        .unwrap();
    within("the callee's greeting", callee.ready())
        .await
        .unwrap();

    let no_method = within(
        "the call of a method that is not there",
        caller.invoke_async("weather", "tomorow", Value::Undefined),
    )
    .await
    .unwrap_err();
    assert_eq!(
        no_method.to_string(),
        "weather has no method tomorow (methods: today, tomorrow)"
    );
    assert_eq!(no_method.code(), Some("no-method"));
    assert_eq!(
        no_method.data(),
        Some(json!({
            "service": "weather",
            "method": "tomorow",
            "methods": ["today", "tomorrow"],
        }))
    );
    let bad_value = within(
        "the call of a value that cannot cross",
        caller.invoke_async("svc", "sparse", Value::Undefined),
    )
    .await
    .unwrap_err();
    assert_eq!(bad_value.code(), Some("bad-value"));
    // The next call is unaffected: an error belongs to one call.
    let after = within(
        "the call after them",
        caller.invoke_async("svc", "fine", Value::Undefined),
    )
    .await
    .unwrap();
    assert_eq!(after.json().unwrap(), json!("ok"));
    caller.close(Error::Transport("done".into()));
    within("the callee ending", callee.closed()).await;
    within("the caller ending", caller.closed()).await;

    let lines = log.lines();
    accepted(&lines);
    let all = frames.all();
    let checked = checked_frames(&all, &[("main", "node")], None);
    assert_eq!(checked.unanswered(), 0, "{all:?}");

    let no_method = checked
        .failure("main", "main:1")
        .expect("the throw that answered the first call");
    assert_eq!(no_method.name, "BindingError");
    assert_eq!(no_method.code(), Some("no-method"));
    assert_eq!(
        no_method.data(),
        Some(&json!({
            "service": "weather",
            "method": "tomorow",
            "methods": ["today", "tomorrow"],
        }))
    );
    let bad_value = checked
        .failure("main", "main:2")
        .expect("the throw that answered the second call");
    assert_eq!(bad_value.name, "BindingError");
    assert_eq!(bad_value.code(), Some("bad-value"));
}

/// A graph that does not resolve fails only its call (§3.5): the receiver
/// keeps the failure as it came, the session goes on, and the next call is
/// answered. No session event follows the malformed graph: the trace is
/// allowed, and the failure is recorded as `bad graph`.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_error_graph_fails_only_its_call() {
    let log = Log::default();
    let frames = Decoded::default();
    let (local, far) = rutis_bridge::memory::pair();
    let local = instrumented(local, "main", "node", &log, &frames);
    // The far end is a fixture too: what it reads is decoded, so the trace
    // holds the frames of both directions.
    let mut far = instrumented(far, "node", "main", &log, &frames);
    let caller = Connection::open(
        local,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let scripted = std::thread::spawn(move || {
        let hello = read_frame(&mut far);
        assert_eq!(hello["op"], "hello");
        write_frame(
            &mut far,
            json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "node" }),
        );
        let first = read_frame(&mut far);
        // The root names node 3 of no nodes: a malformed graph.
        write_frame(
            &mut far,
            json!({
                "op": "throw",
                "id": first["id"],
                "error": {
                    "name": "TypeError",
                    "message": "bad graph",
                    "graph": { "root": { "type": "reference", "value": 3 }, "nodes": [] },
                },
            }),
        );
        let second = read_frame(&mut far);
        write_frame(
            &mut far,
            json!({
                "op": "return",
                "id": second["id"],
                "value": { "type": "data", "value": "after the graph" },
            }),
        );
        far
    });
    within("the greeting", caller.ready()).await.unwrap();

    let error = within(
        "the call a malformed graph answers",
        caller.invoke_async("svc", "m", Value::Undefined),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "TypeError: bad graph");
    assert_eq!(error.code(), None, "{error:?}");
    assert!(
        caller.close_reason().is_none(),
        "a malformed graph fails its call, not the session (§3.5)"
    );
    let after = within(
        "the call after it",
        caller.invoke_async("svc", "m", Value::Undefined),
    )
    .await
    .unwrap();
    assert_eq!(after.json().unwrap(), json!("after the graph"));
    caller.close(Error::Transport("done".into()));
    within("the caller ending", caller.closed()).await;
    let _far = scripted.join().unwrap();

    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed, "{lines:?}");
    let all = frames.all();
    let checked = checked_frames(&all, &[("main", "node")], None);
    assert_eq!(checked.unanswered(), 0, "{all:?}");
    let failure = checked
        .failure("main", "main:1")
        .expect("the throw that answered the first call");
    assert_eq!(failure.name, "TypeError");
    assert_eq!(failure.message, "bad graph");
    assert_eq!(failure.graph, Graph::Bad);
    assert_eq!(failure.code(), None);
}

// ------------------------------------------------------ §3.6, the limits

/// `channel` whose sending side takes at most `limit` bytes and says so,
/// as the local and WebSocket transports do: a session reads `max_message`
/// and checks what it would send before writing it, so a value over the
/// limit fails only its call (#675).
fn limited(channel: Channel, limit: usize) -> Channel {
    struct Limited {
        inner: Box<dyn Sender>,
        limit: usize,
    }
    impl Sender for Limited {
        fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
            if message.len() > self.limit {
                return Err(ChannelError::Closed {
                    reason: format!("the channel takes at most {} bytes", self.limit),
                });
            }
            self.inner.send(message)
        }
        fn max_message(&self) -> Option<usize> {
            Some(self.limit)
        }
    }
    Channel {
        sender: Box::new(Limited {
            inner: channel.sender,
            limit,
        }),
        receiver: channel.receiver,
        closer: channel.closer,
    }
}

/// `channel` whose carrier refuses a message over `limit` although the
/// session was not told a limit (`max_message` is `None`): the message the
/// session wrote reaches the carrier, which refuses it; the failed send
/// ends the channel and the session. That is not how a value over a
/// checked limit fails its call.
fn refusing(channel: Channel, limit: usize) -> Channel {
    struct Refusing {
        inner: Box<dyn Sender>,
        limit: usize,
    }
    impl Sender for Refusing {
        fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
            if message.len() > self.limit {
                return Err(ChannelError::Closed {
                    reason: format!(
                        "the carrier refused {} bytes, over its limit of {} bytes",
                        message.len(),
                        self.limit
                    ),
                });
            }
            self.inner.send(message)
        }
    }
    Channel {
        sender: Box::new(Refusing {
            inner: channel.sender,
            limit,
        }),
        receiver: channel.receiver,
        closer: channel.closer,
    }
}

/// §3.6: the session checks a frame against the channel's limit before it
/// writes it. A value over the limit fails only its call: no frame is
/// written, the session goes on, and the call after it is answered. The
/// trace records the size only; the refused value never reaches it. The
/// same rule rejects a frame over the limit that a trace decodes, and
/// takes one of exactly the limit.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_over_the_channels_limit_fails_only_its_call() {
    const LIMIT: usize = 1024;
    const PAYLOAD: &str = "over-limit-value-";
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(limited(ours, LIMIT), "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    // A far end that takes no chunks: the channel's limit is what refuses the
    // frame, which is what this test traces (#1361 lifts it only between two
    // ends that declared `chunked`).
    let mut callee_endpoint = Endpoint::rust(id("node")).expect(id("main"));
    callee_endpoint
        .capabilities
        .retain(|capability| capability != "chunked");
    let callee = Connection::open(theirs, Arc::new(Echo), callee_endpoint).unwrap();
    within("the caller's greeting", caller.ready())
        .await
        .unwrap();
    within("the callee's greeting", callee.ready())
        .await
        .unwrap();

    let big = json!(PAYLOAD.repeat(64));
    let error = within(
        "the call whose argument is over the limit",
        caller.invoke_async("svc", "echo", Value::List(vec![Value::Data(big)])),
    )
    .await
    .unwrap_err();
    match &error {
        Error::Value { message, reason } => {
            assert_eq!(reason, &Some("limit"), "{message}");
            assert!(
                message.starts_with("svc.echo: argument 1 cannot be sent: the message would be ")
                    && message.ends_with(" bytes, over the limit of 1024 bytes"),
                "{message}"
            );
        }
        other => panic!("a value error expected, got {other:?}"),
    }
    assert_eq!(error.code(), Some("bad-value"));
    assert!(
        caller.close_reason().is_none(),
        "a value over the limit fails its call, not the session"
    );
    let answer = within(
        "the call after the refused one",
        caller.invoke_async("svc", "echo", Value::Data(json!("small"))),
    )
    .await
    .unwrap();
    assert_eq!(answer.json().unwrap(), json!("small"));
    caller.close(Error::Transport("done".into()));
    within("the callee ending", callee.closed()).await;
    within("the caller ending", caller.closed()).await;

    let lines = log.lines();
    assert!(
        lines.iter().all(|line| !line.contains(PAYLOAD)),
        "the trace carries sizes, never the value: {lines:?}"
    );
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    for event in checked
        .events("main")
        .iter()
        .chain(checked.events("node").iter())
    {
        if let Event::Sent(bytes) | Event::Received(bytes) = event {
            assert!(
                *bytes <= LIMIT,
                "a frame over the channel's limit is in the trace: {lines:?}"
            );
        }
    }

    let all = frames.all();
    let checked = check_frames(&all, &[("main", "node")], Some(LIMIT))
        .unwrap_or_else(|violation| panic!("an allowed frame sequence was rejected: {violation}"));
    assert_eq!(checked.unanswered(), 0, "{all:?}");
    // The refused call was numbered and never written; the call after it
    // carries the id after the one no frame has.
    assert_eq!(
        ops(&frames.from("main")),
        [("hello", None), ("invoke", Some("main:2"))],
        "the call over the limit left no frame"
    );
    assert!(
        !all.iter()
            .any(|frame| frame.id.as_deref() == Some("main:1")),
        "the refused call's id is not on the wire: {all:?}"
    );

    // §3.6 rule: a frame over the channel's limit is not in an allowed
    // trace, because the session checks before writing it.
    let over = format!(
        r#"{{"op":"invoke","id":"main:1","path":[],"target":"svc","method":"echo","args":{{"type":"data","value":{}}}}}"#,
        json!(PAYLOAD.repeat(64))
    );
    let violation = rejected_frames(
        &decoded(&[("main", HELLO_MAIN), ("main", &over)]),
        &[("main", "node")],
        Some(LIMIT),
    );
    assert_eq!(violation.at, 1, "{violation}");
    assert!(
        violation
            .why
            .contains("over the channel's limit of 1024 bytes"),
        "{violation}"
    );
    // The boundary: a frame of exactly the limit is within it.
    let mut exact = serde_json::to_string(&json!({
        "op": "invoke",
        "id": "main:1",
        "path": [],
        "target": "svc",
        "method": "echo",
        "args": {"type": "undefined"},
    }))
    .unwrap();
    assert!(exact.len() < LIMIT);
    exact.push_str(&" ".repeat(LIMIT - exact.len()));
    assert_eq!(exact.len(), LIMIT);
    let checked = checked_frames(
        &decoded(&[("main", HELLO_MAIN), ("main", &exact)]),
        &[("main", "node")],
        Some(LIMIT),
    );
    assert_eq!(
        checked.unanswered(),
        1,
        "the call of the frame at the limit is in the trace"
    );
}

/// §3.6: a message over the limit that reaches the carrier ends the
/// channel, and with it the session: the failure is a transport one, not
/// the call's. The session was not told the limit (`max_message` is
/// `None`), so it wrote the frame; the carrier refused it. This is the
/// other result from the value checked before writing.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_the_carrier_refuses_ends_the_session() {
    const LIMIT: usize = 1024;
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(refusing(ours, LIMIT), "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let callee = Connection::open(
        theirs,
        Arc::new(Echo),
        Endpoint::rust(id("node")).expect(id("main")),
    )
    .unwrap();
    within("the caller's greeting", caller.ready())
        .await
        .unwrap();
    within("the callee's greeting", callee.ready())
        .await
        .unwrap();

    let error = within(
        "the call whose argument the carrier refuses",
        caller.invoke_async("svc", "echo", Value::Data(json!("z".repeat(2 * LIMIT)))),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, Error::Transport(_)),
        "a carrier refusal is the session's failure, not a value error: {error:?}"
    );
    assert!(
        error.to_string().contains("over its limit of 1024 bytes"),
        "{error}"
    );
    let reason = caller.close_reason().expect("the session ended");
    assert!(matches!(reason, Error::Transport(_)), "{reason:?}");
    within("the callee ending", callee.closed()).await;
    within("the caller ending", caller.closed()).await;

    let lines = log.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("main: send failed: the carrier refused")),
        "{lines:?}"
    );
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    assert!(
        checked
            .events("main")
            .iter()
            .any(|event| matches!(event, Event::SendFailed(_))),
        "{lines:?}"
    );
    // The refused frame never reached the far end: what it read is the
    // greeting, and no frame over the limit is in the trace.
    let all = frames.all();
    assert_eq!(
        ops(&frames.from("main")),
        [("hello", None)],
        "the frame the carrier refused crossed nothing"
    );
    checked_frames(&all, &[("main", "node")], Some(LIMIT));
}

// ------------------------------------------- §3.8, the endings and retrying

/// How an ended session is classified for retrying: by the kind of the
/// error, never by its text (`crates/rutis-host/src/connect/session.rs:56-61`,
/// `go/rutis/internal/runtime/dial.go` `sessionFailure`). Another protocol
/// version or format cannot be retried until the configuration changes; a
/// refused identity is an authentication failure; anything else, being
/// replaced too, is retryable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retry {
    Retryable,
    AuthRejected,
    Incompatible,
}

fn retry(error: &Error) -> Retry {
    match error {
        Error::Handshake(Handshake::Incompatible(_)) => Retry::Incompatible,
        Error::Handshake(Handshake::IdentityMismatch(_)) => Retry::AuthRejected,
        _ => Retry::Retryable,
    }
}

/// `channel` whose closes all say `why` in place of `Done`, as a host makes
/// a replaced connection close as replaced
/// (`crates/rutis-host/src/connect/listen.rs:204-220`). A session closes
/// its side as `Done` while the host closes the channel as replaced or
/// rejected; the memory channel sets a close's reason on its two directions
/// one after the other (`crates/rutis-bridge/src/memory/mod.rs:145-154`), so
/// the two closes can reach the far end as different reasons — it can read
/// the end of the stream where the trace says replaced. One reason for both
/// closes is what the far end of one close gets on a carrier that closes
/// once, as the WebSocket does.
fn one_reason(channel: Channel, why: Close) -> Channel {
    struct OneReason {
        inner: Arc<dyn Closer>,
        why: Close,
    }
    impl Closer for OneReason {
        fn close(&self, why: Close) {
            let why = match why {
                Close::Done => self.why.clone(),
                why => why,
            };
            self.inner.close(why);
        }
    }
    Channel {
        sender: channel.sender,
        receiver: channel.receiver,
        closer: Arc::new(OneReason {
            inner: channel.closer,
            why,
        }),
    }
}

/// §3.8: the far end is replaced by a newer connection of its id. Its
/// channel closes as replaced (4002 on WebSocket), the session ends with
/// the channel's reason, and that ending is retryable: a dialer connects
/// again by its backoff. Nothing crosses the channel after the close.
#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_connection_is_a_retryable_ending() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let theirs = one_reason(theirs, Close::Replaced);
    let node_closer = theirs.closer.clone();
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let callee = Connection::open(
        theirs,
        Arc::new(Echo),
        Endpoint::rust(id("node")).expect(id("main")),
    )
    .unwrap();
    within("the caller's greeting", caller.ready())
        .await
        .unwrap();
    within("the callee's greeting", callee.ready())
        .await
        .unwrap();

    node_closer.close(Close::Replaced);
    within("the callee ending", callee.closed()).await;
    within("the caller ending", caller.closed()).await;

    let ended = caller.close_reason().expect("the caller's session ended");
    assert_eq!(
        ended.to_string(),
        "replaced by a new connection",
        "callee ended with {:?}; lines {:?}",
        callee.close_reason(),
        log.lines()
    );
    assert_eq!(retry(&ended), Retry::Retryable);
    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    assert_eq!(checked.last["node"], State::Closed);
    assert!(
        checked.events("node").contains(&Event::ClosedReplaced),
        "the far end was told it was replaced: {lines:?}"
    );
    let all = frames.all();
    let checked = checked_frames(&all, &[("main", "node")], None);
    assert_eq!(checked.unanswered(), 0, "{all:?}");
}

/// §3.8: a replacement is a new session on a new channel. Its calls are
/// its own, numbered from 1 again; an id the replaced session opened is
/// not one the replacement opened, so a reply on the replacement for it
/// settles nothing there. The old channel closes as `replaced`, and the
/// two channels' frames are checked apart. The host's install after the
/// old session unloads is its own order (`crates/rutis-host/src/connect/
/// listen.rs:4-10`, tested there as
/// `a_newer_connection_replaces_the_older_after_the_older_fully_unloaded`);
/// what it leaves in a channel trace is the old channel's close, which is
/// what this case carries.
/// §3.8: an identity refused after the channel was established
/// (`Close::Rejected`, 4003 on WebSocket) ends the session as an endpoint
/// mismatch: an authentication failure, not a retryable one. The text is
/// not what decides it: a reason reading `incompatible` or `retry` is
/// still an identity refusal.
#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_identity_is_an_authentication_failure() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let theirs = one_reason(
        theirs,
        Close::Rejected("incompatible: temporary, retry".into()),
    );
    let node_closer = theirs.closer.clone();
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let callee = Connection::open(
        theirs,
        Arc::new(Echo),
        Endpoint::rust(id("node")).expect(id("main")),
    )
    .unwrap();
    within("the caller's greeting", caller.ready())
        .await
        .unwrap();
    within("the callee's greeting", callee.ready())
        .await
        .unwrap();

    node_closer.close(Close::Rejected("incompatible: temporary, retry".into()));
    within("the callee ending", callee.closed()).await;
    within("the caller ending", caller.closed()).await;

    let ended = caller.close_reason().expect("the caller's session ended");
    assert!(
        matches!(ended, Error::Handshake(Handshake::IdentityMismatch(_))),
        "{ended:?}"
    );
    assert_eq!(
        ended.to_string(),
        "endpoint mismatch: rejected: incompatible: temporary, retry"
    );
    assert_eq!(
        retry(&ended),
        Retry::AuthRejected,
        "classified by type, never by text"
    );
    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    assert!(
        checked.events("main").iter().any(
            |event| matches!(event, Event::ReceiveFailed(reason) if reason.starts_with("rejected: "))
        ),
        "{lines:?}"
    );
    let all = frames.all();
    checked_frames(&all, &[("main", "node")], None);
}

/// §3.8 and §3.2: a far end speaking protocol 3 is incompatible — a dialer
/// stops retrying it until the configuration changes — and its greeting is
/// no greeting of protocol 5, so the frame validator rejects the trace the
/// channel carried. The channel trace itself is allowed: its end is a
/// normal close.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_three_greeting_fails_as_incompatible() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let mut theirs = instrumented(theirs, "node", "main", &log, &frames);
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let far = std::thread::spawn(move || {
        let hello = read_frame(&mut theirs);
        assert_eq!(hello["op"], "hello");
        write_frame(
            &mut theirs,
            json!({ "op": "hello", "version": 3, "endpoint": "node" }),
        );
        theirs.receiver.recv()
    });

    let error = within("the handshake", caller.ready()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "incompatible session: the far end speaks protocol 3, this side 5: upgrade the far end"
    );
    assert_eq!(retry(&error), Retry::Incompatible);
    within("the caller ending", caller.closed()).await;
    let ended = within(
        "the far end's channel",
        tokio::task::spawn_blocking(move || far.join().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(ended, Ok(None), "the channel closed with Done");

    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    let all = frames.all();
    let violation = rejected_frames(&all, &[("main", "node")], None);
    assert_eq!(violation.at, 1, "{violation}");
    assert!(
        violation
            .why
            .contains("a greeting speaks protocol 5, not Some(3)"),
        "{violation}"
    );
}

/// §3.8 and §3.2: a far end that never greets gives up after 10 s with
/// `the far end did not greet in time`; a transport ending, retryable like
/// a replacement, not an incompatible session. The channel's own trace is
/// the greeting then its close; no frame of a session was exchanged.
#[tokio::test(start_paused = true)]
async fn a_far_end_that_never_greets_times_out_as_retryable() {
    let root = Ctx::root().unwrap();
    root.provide(LocalId(id("main"))).unwrap();
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let started = tokio::time::Instant::now();
    let error = match Session::handshake(&root, ours).await {
        Err(error) => error,
        Ok(_) => panic!("a far end that never greets is not a session"),
    };
    assert_eq!(error.to_string(), "the far end did not greet in time");
    assert_eq!(retry(&error), Retry::Retryable);
    assert_eq!(started.elapsed(), Duration::from_secs(10));

    let (first, second) = within(
        "the far end",
        tokio::task::spawn_blocking(move || {
            let mut theirs = theirs;
            (read_frame(&mut theirs), theirs.receiver.recv())
        }),
    )
    .await
    .unwrap();
    assert_eq!(first["op"], "hello");
    assert_eq!(second, Ok(None), "the channel closed with Done");

    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    let main = frames.from("main");
    assert_eq!(ops(&main), [("hello", None)]);
    assert_eq!(
        checked.events("main").first(),
        Some(&Event::Sent(main[0].bytes)),
        "the trace records the greeting's size"
    );
    checked_frames(&frames.all(), &[("main", "node")], None);
}

// --------------------------------------------- §3.4, synchronous waits

/// A far end's greeting on `channel`: it reads this side's greeting, then
/// answers as `endpoint`, declaring `capabilities` (§3.2). A runtime with
/// one thread and one stack declares `sync-stack` and not `reentrant-sync`.
fn greet_as(channel: &mut Channel, endpoint: &str, capabilities: &[&str]) {
    let hello = read_frame(channel);
    assert_eq!(hello["op"], "hello", "{hello}");
    write_frame(
        channel,
        json!({
            "op": "hello",
            "version": ENDPOINT_PROTOCOL,
            "endpoint": endpoint,
            "capabilities": capabilities,
        }),
    );
}

/// A call frame from a scripted far end (`sync` and a chain when given).
fn scripted_call(endpoint: &str, n: u64, method: &str, sync: bool, path: Vec<String>) -> Json {
    json!({
        "op": "invoke",
        "id": format!("{endpoint}:{n}"),
        "path": path,
        "target": "t",
        "method": method,
        "args": { "type": "undefined" },
        "sync": sync,
    })
}

/// Run a far end's script on a thread of its own and wait for it, so a
/// script that never gets its frame fails the test after `guard` instead of
/// holding the test process open (Q7.1). A test starts it with
/// `tokio::spawn`, before it waits for what the script does.
async fn far_end<T: Send + 'static>(
    what: &'static str,
    guard: Duration,
    script: impl FnOnce() -> T + Send + 'static,
) -> T {
    tokio::task::spawn_blocking(move || {
        let (done, finished) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let value = script();
            let _ = done.send(());
            value
        });
        match finished.recv_timeout(guard) {
            Ok(()) => thread.join().unwrap(),
            Err(error) => panic!("{what}: the far end did not finish: {error}"),
        }
    })
    .await
    .unwrap()
}

/// One session's host side in the cycle case: `start` waits for the test,
/// then forwards into the other session; the forward that would close the
/// cycle must not be sent (#228, `specs/cross-runtime-sync`, policy B).
struct Start {
    other: std::sync::OnceLock<Connection>,
    events: Mutex<std::sync::mpsc::Sender<&'static str>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Dispatch for Start {
    fn invoke(&self, peer: &Connection, _: &str, method: &str, args: Value) -> Reply {
        if method != "start" {
            return Ok(args);
        }
        self.events.lock().unwrap().send("start").unwrap();
        self.release.lock().unwrap().recv().unwrap();
        let other = self.other.get().expect("the other session");
        other.forward(peer, || other.invoke("t", "work", Value::List(Vec::new())))
    }
}

/// The other session's host side: `kick` forwards into `Start`'s session
/// and waits there; the far end is one stack, so it answers no other call
/// while it waits.
struct Kick {
    other: std::sync::OnceLock<Connection>,
    events: Mutex<std::sync::mpsc::Sender<&'static str>>,
}

impl Dispatch for Kick {
    fn invoke(&self, peer: &Connection, _: &str, method: &str, args: Value) -> Reply {
        if method != "kick" {
            return Ok(args);
        }
        self.events.lock().unwrap().send("kick").unwrap();
        let other = self.other.get().expect("the other session");
        other.forward(peer, || other.invoke("t", "hold", Value::List(Vec::new())))
    }
}

/// guarantee: bridge/sync-wait-cycle-refused; REM-6.2-10, REM-6.2-11,
/// REM-6.2-15, M1-7.2-5.
///
/// §3.4: a synchronous call forwarded across two one-stack runtimes that
/// would wait on itself is not sent. The cycle here: A's far end calls
/// `start`; A forwards to B (which waits); B's far end calls `kick`, which
/// B forwards to A (A's far end is waiting on `start`, so it holds it);
/// A's `start` then forwards to B, which has that held call out — the
/// forward would wait on itself. A's caller gets `SyncWaitCycle`; no frame
/// names the refused call; everything else completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sync_forward_that_would_close_a_cycle_is_not_sent() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours_a, theirs_a) = rutis_bridge::memory::pair();
    let (ours_b, theirs_b) = rutis_bridge::memory::pair();
    let ours_a = instrumented(ours_a, "main-a", "node-a", &log, &frames);
    let theirs_a = instrumented(theirs_a, "node-a", "main-a", &log, &frames);
    let ours_b = instrumented(ours_b, "main-b", "node-b", &log, &frames);
    let theirs_b = instrumented(theirs_b, "node-b", "main-b", &log, &frames);

    let (events, arrived) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let start = Arc::new(Start {
        other: std::sync::OnceLock::new(),
        events: Mutex::new(events.clone()),
        release: Mutex::new(released),
    });
    let kick = Arc::new(Kick {
        other: std::sync::OnceLock::new(),
        events: Mutex::new(events),
    });
    let a = Connection::open(
        ours_a,
        start.clone(),
        Endpoint::rust(id("main-a")).expect(id("node-a")),
    )
    .unwrap();
    let b = Connection::open(
        ours_b,
        kick.clone(),
        Endpoint::rust(id("main-b")).expect(id("node-b")),
    )
    .unwrap();
    assert!(start.other.set(b.clone()).is_ok());
    assert!(kick.other.set(a.clone()).is_ok());
    let (a_tag, b_tag) = (a.tag().to_owned(), b.tag().to_owned());

    let (go, begin) = std::sync::mpsc::channel();
    let step = Duration::from_secs(20);
    let far = tokio::spawn(far_end("the cycle", step, move || {
        let (mut far_a, mut far_b) = (theirs_a, theirs_b);
        greet_as(&mut far_a, "node-a", &["sync-stack"]);
        greet_as(&mut far_b, "node-b", &["sync-stack"]);
        begin.recv().unwrap();
        // A's far end asks for `start` and then holds its stack, waiting.
        write_frame(
            &mut far_a,
            scripted_call("node-a", 1, "start", true, vec![]),
        );
        assert_eq!(arrived.recv_timeout(step).unwrap(), "start");
        // B's far end asks for `kick`; B forwards it to A and waits for it.
        write_frame(&mut far_b, scripted_call("node-b", 1, "kick", true, vec![]));
        assert_eq!(arrived.recv_timeout(step).unwrap(), "kick");
        // The first hop: B's `kick` forwarded into A, on B's marked chain.
        let first = read_frame(&mut far_a);
        assert_eq!(
            (
                first["op"].as_str(),
                first["id"].as_str(),
                first["sync"].as_bool()
            ),
            (Some("invoke"), Some("main-a:1"), Some(true)),
            "{first}"
        );
        assert_eq!(
            first["path"],
            json!([format!("{b_tag}/node-b:1")]),
            "the chain names B's waiting call: {first}"
        );
        release.send(()).unwrap();
        // The forward that would close the cycle was refused: A's caller
        // gets the failure, and no frame of the refused call crossed.
        let refused = read_frame(&mut far_a);
        assert_eq!(
            (refused["op"].as_str(), refused["id"].as_str()),
            (Some("throw"), Some("node-a:1")),
            "{refused}"
        );
        assert_eq!(refused["error"]["name"], "SyncWaitCycle", "{refused}");
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .ends_with(" would wait on itself (synchronous calls)"),
            "{refused}"
        );
        // A's far end now answers the held call, which B's `kick` waits for.
        write_frame(
            &mut far_a,
            json!({
                "op": "return",
                "id": "main-a:1",
                "value": { "type": "data", "value": "held" },
            }),
        );
        let kicked = read_frame(&mut far_b);
        assert_eq!(
            (kicked["op"].as_str(), kicked["id"].as_str()),
            (Some("return"), Some("node-b:1")),
            "{kicked}"
        );
    }));
    within("both greetings", async {
        a.ready().await.unwrap();
        b.ready().await.unwrap();
    })
    .await;
    go.send(()).unwrap();
    within("the far ends", far).await.unwrap();

    a.close(Error::Transport("done".into()));
    b.close(Error::Transport("done".into()));
    within("the first session ending", a.closed()).await;
    within("the second session ending", b.closed()).await;

    let sessions = [("main-a", "node-a"), ("main-b", "node-b")];
    let all = frames.all();
    let checked = checked_frames(&all, &sessions, None);
    assert_eq!(checked.unanswered(), 0, "{all:?}");
    // The refused forward's id is not on the wire: the session numbers the
    // call it refused, and nothing of it crossed.
    assert!(
        !all.iter()
            .any(|frame| frame.id.as_deref() == Some("main-b:1")),
        "the refused forward left no frame: {all:?}"
    );
    let lines = log.lines();
    accepted(&lines);

    // The trace rule: a synchronous forward into a one-stack runtime that
    // has a call out, not on the forward's chain, would wait on itself.
    let violation = refused_waits(&all, &sessions);
    assert!(
        violation.is_none(),
        "the real trace is allowed: {violation:?}"
    );

    // The same trace with the refused frame put back is not: the cycle
    // check is what keeps it out of the trace.
    let at = all
        .iter()
        .position(|frame| frame.op == "throw" && frame.id.as_deref() == Some("node-a:1"))
        .expect("the throw");
    let mut with_refused = all.clone();
    with_refused.insert(
        at,
        decode(
            "main-b",
            serde_json::to_string(&scripted_call(
                "main-b",
                1,
                "work",
                true,
                vec![format!("{a_tag}/node-a:1")],
            ))
            .unwrap()
            .as_bytes(),
        ),
    );
    checked_frames(&with_refused, &sessions, None);
    let violation = refused_waits(&with_refused, &sessions).expect("the cycle is caught");
    assert_eq!(violation.at, at, "{violation}");
    assert!(
        violation.why.contains("would wait on itself"),
        "{violation}"
    );
}

/// One session's host side in the stack-order case: for each method it
/// forwards, the method called and the session it goes to. The forward
/// keeps the caller's chain (`Connection::forward`), which is what makes
/// the forwarded call one the caller waits for.
struct Forwards(Vec<(&'static str, &'static str, std::sync::OnceLock<Connection>)>);

impl Dispatch for Forwards {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        let Some((_, call, other)) = self.0.iter().find(|(name, _, _)| *name == method) else {
            return Ok(args);
        };
        let other = other.get().expect("the session it forwards to");
        other.forward(peer, || other.invoke(target, call, Value::List(Vec::new())))
    }
}

/// A far end's failure for the call `id` here (`throw`, §3.5).
fn scripted_throw(id: &str, message: &str) -> Json {
    json!({
        "op": "throw",
        "id": id,
        "error": { "name": "Error", "message": message },
    })
}

/// guarantee: bridge/sync-wait-cycle-refused; REM-6.2-10, REM-6.2-11,
/// REM-6.2-15, M1-7.2-5.
///
/// §3.4, the two edges of the wait graph a channel's own trace does not
/// show, on a real run: a call waits for what was made under it (chain) and
/// for what later started on a one-stack runtime's stack (stack order).
///
/// n's far end calls `go`; n forwards to p2 (p2's call `main-p2:1`). p2's far end is
/// one stack and reentrant, so it runs a call that arrives while it waits:
/// it calls `mid`, which p2 forwards to p1 under `main-p2:1`. p1's far end
/// calls `kick`, whose forward to p2 (`main-p2:2`) starts on top of
/// `main-p2:1`. p2's far end then calls `deep` — on top of `main-p2:2` —
/// and p2 forwards to n: n's far end is one stack and waiting, so the call
/// would be put aside behind `main-p2:1`, which is buried under
/// `main-p2:2`, which waits for it. p2's forward is refused, and the calls
/// unwound with it.
///
/// The paths are the chain as it crosses sessions (§4, identity): each
/// forward names the call it was made under, tagged with the session that
/// made it (`Connection::tag`), and an entry of a third session passes
/// through as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn a_stack_order_cycle_is_not_sent() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours_n, theirs_n) = rutis_bridge::memory::pair();
    let (ours_p1, theirs_p1) = rutis_bridge::memory::pair();
    let (ours_p2, theirs_p2) = rutis_bridge::memory::pair();
    let ours_n = instrumented(ours_n, "main-n", "node-n", &log, &frames);
    let theirs_n = instrumented(theirs_n, "node-n", "main-n", &log, &frames);
    let ours_p1 = instrumented(ours_p1, "main-p1", "node-p1", &log, &frames);
    let theirs_p1 = instrumented(theirs_p1, "node-p1", "main-p1", &log, &frames);
    let ours_p2 = instrumented(ours_p2, "main-p2", "node-p2", &log, &frames);
    let theirs_p2 = instrumented(theirs_p2, "node-p2", "main-p2", &log, &frames);

    let forwards = |routes: Vec<(&'static str, &'static str)>| {
        Arc::new(Forwards(
            routes
                .into_iter()
                .map(|(method, call)| (method, call, std::sync::OnceLock::new()))
                .collect(),
        ))
    };
    let n = forwards(vec![("go", "mid")]);
    let p1 = forwards(vec![("kick", "hold")]);
    let p2 = forwards(vec![("mid", "hold"), ("deep", "again")]);
    let conn_n = Connection::open(
        ours_n,
        n.clone(),
        Endpoint::rust(id("main-n")).expect(id("node-n")),
    )
    .unwrap();
    let conn_p1 = Connection::open(
        ours_p1,
        p1.clone(),
        Endpoint::rust(id("main-p1")).expect(id("node-p1")),
    )
    .unwrap();
    let conn_p2 = Connection::open(
        ours_p2,
        p2.clone(),
        Endpoint::rust(id("main-p2")).expect(id("node-p2")),
    )
    .unwrap();
    // Where each session forwards: n's `go` and p1's `kick` into p2, p2's
    // `mid` into p1, and p2's `deep` back into n (the forward that would
    // close the cycle).
    assert!(n.0[0].2.set(conn_p2.clone()).is_ok());
    assert!(p1.0[0].2.set(conn_p2.clone()).is_ok());
    assert!(p2.0[0].2.set(conn_p1.clone()).is_ok());
    assert!(p2.0[1].2.set(conn_n.clone()).is_ok());
    let (p1_tag, p2_tag) = (conn_p1.tag().to_owned(), conn_p2.tag().to_owned());
    let (n_in, p1_in, p2_in) = (conn_n.tag().to_owned(), p1_tag.clone(), p2_tag.clone());

    let (go, begin) = std::sync::mpsc::channel();
    let step = Duration::from_secs(20);
    let far = tokio::spawn(far_end("the stack", step, move || {
        let (mut far_n, mut far_p1, mut far_p2) = (theirs_n, theirs_p1, theirs_p2);
        greet_as(&mut far_n, "node-n", &["sync-stack"]);
        greet_as(&mut far_p1, "node-p1", &["sync-stack", "reentrant-sync"]);
        greet_as(&mut far_p2, "node-p2", &["sync-stack", "reentrant-sync"]);
        begin.recv().unwrap();
        // n's far end calls the host: the root of the chain.
        write_frame(&mut far_n, scripted_call("node-n", 1, "go", true, vec![]));
        // n forwards to p2; the call is made under n's waiting call.
        let first = read_frame(&mut far_p2);
        assert_eq!(
            (
                first["op"].as_str(),
                first["id"].as_str(),
                first["sync"].as_bool()
            ),
            (Some("invoke"), Some("main-p2:1"), Some(true)),
            "{first}"
        );
        assert_eq!(
            first["path"],
            json!([format!("{n_in}/node-n:1")]),
            "n's waiting call, tagged with n's session: {first}"
        );
        // p2's far end calls back, naming the call into it, and p2 forwards
        // to p1 under both calls: p2's own call is tagged, and the entry of
        // the third session (p1's `hold`) passes through as it was.
        write_frame(
            &mut far_p2,
            scripted_call("node-p2", 1, "mid", true, vec!["main-p2:1".into()]),
        );
        let second = read_frame(&mut far_p1);
        assert_eq!(
            (
                second["op"].as_str(),
                second["id"].as_str(),
                second["sync"].as_bool()
            ),
            (Some("invoke"), Some("main-p1:1"), Some(true)),
            "{second}"
        );
        assert_eq!(
            second["path"],
            json!([format!("{p2_in}/main-p2:1"), format!("{p2_in}/node-p2:1"),]),
            "p2's stack, tagged with p2's session: {second}"
        );
        // p1's far end calls its host on its own, and p1 forwards to p2: a
        // later call into p2, which its one stack runs on top of the one
        // waiting there.
        write_frame(
            &mut far_p1,
            scripted_call("node-p1", 1, "kick", true, vec![]),
        );
        let third = read_frame(&mut far_p2);
        assert_eq!(
            (
                third["op"].as_str(),
                third["id"].as_str(),
                third["sync"].as_bool()
            ),
            (Some("invoke"), Some("main-p2:2"), Some(true)),
            "{third}"
        );
        assert_eq!(
            third["path"],
            json!([format!("{p1_in}/node-p1:1")]),
            "p1's waiting call, tagged with p1's session: {third}"
        );
        // p2's far end calls back on top of `main-p2:2`, which starts it
        // there; the forward p2 makes to n would wait on its own stack.
        write_frame(
            &mut far_p2,
            scripted_call(
                "node-p2",
                2,
                "deep",
                true,
                vec![format!("{p1_in}/node-p1:1"), "main-p2:2".to_owned()],
            ),
        );
        let refused = read_frame(&mut far_p2);
        assert_eq!(
            (refused["op"].as_str(), refused["id"].as_str()),
            (Some("throw"), Some("node-p2:2")),
            "{refused}"
        );
        assert_eq!(refused["error"]["name"], "SyncWaitCycle", "{refused}");
        assert_eq!(
            refused["error"]["message"], "t.again would wait on itself (synchronous calls)",
            "{refused}"
        );
        // The far end fails its own call, and the calls unwind one by one:
        // each session throws its caller what the refused forward was.
        write_frame(
            &mut far_p2,
            scripted_throw("main-p2:2", "the stack order refused the forward"),
        );
        let kick = read_frame(&mut far_p1);
        assert_eq!(
            (kick["op"].as_str(), kick["id"].as_str()),
            (Some("throw"), Some("node-p1:1")),
            "{kick}"
        );
        write_frame(
            &mut far_p1,
            scripted_throw("main-p1:1", "the forward into p2 failed"),
        );
        let mid = read_frame(&mut far_p2);
        assert_eq!(
            (mid["op"].as_str(), mid["id"].as_str()),
            (Some("throw"), Some("node-p2:1")),
            "{mid}"
        );
        write_frame(
            &mut far_p2,
            scripted_throw("main-p2:1", "the forward into p1 failed"),
        );
        let root = read_frame(&mut far_n);
        assert_eq!(
            (root["op"].as_str(), root["id"].as_str()),
            (Some("throw"), Some("node-n:1")),
            "{root}"
        );
    }));
    within("the greetings", async {
        conn_n.ready().await.unwrap();
        conn_p1.ready().await.unwrap();
        conn_p2.ready().await.unwrap();
    })
    .await;
    go.send(()).unwrap();
    within("the far ends", far).await.unwrap();

    conn_n.close(Error::Transport("done".into()));
    conn_p1.close(Error::Transport("done".into()));
    conn_p2.close(Error::Transport("done".into()));
    within("the first session ending", conn_n.closed()).await;
    within("the second session ending", conn_p1.closed()).await;
    within("the third session ending", conn_p2.closed()).await;

    let sessions = [
        ("main-n", "node-n"),
        ("main-p1", "node-p1"),
        ("main-p2", "node-p2"),
    ];
    let all = frames.all();
    let checked = checked_frames(&all, &sessions, None);
    assert_eq!(checked.unanswered(), 0, "{all:?}");
    accepted(&log.lines());
    // The refused forward's id is not on the wire: n numbered it and
    // nothing of it crossed.
    assert!(
        !all.iter()
            .any(|frame| frame.id.as_deref() == Some("main-n:1")),
        "the refused forward left no frame: {all:?}"
    );
    // The real trace is allowed by all three edges.
    assert_eq!(
        refused_waits(&all, &sessions).map(|violation| violation.to_string()),
        None
    );

    // The same trace with the refused forward put back closes a cycle the
    // chain and stack-order edges make: the call into n waits for the call
    // n has out, that one waits for what started on top of it, and that one
    // was made under the refused call.
    let after = all
        .iter()
        .position(|frame| frame.id.as_deref() == Some("node-p2:2"))
        .expect("the far end's second call")
        + 1;
    let refused = decode(
        "main-n",
        serde_json::to_string(&scripted_call(
            "main-n",
            1,
            "again",
            true,
            vec![
                format!("{p1_tag}/node-p1:1"),
                format!("{p2_tag}/main-p2:2"),
                format!("{p2_tag}/node-p2:2"),
            ],
        ))
        .unwrap()
        .as_bytes(),
    );
    let mut with_refused = all.clone();
    with_refused.insert(after, refused);
    checked_frames(&with_refused, &sessions, None);
    let violation =
        refused_waits(&with_refused, &sessions).expect("the stack order closes a cycle");
    assert_eq!(violation.at, after, "{violation}");
    assert_eq!(violation.text, "main-n sent `invoke`", "{violation}");
    assert_eq!(
        violation.why,
        "the call main-n:1 would wait on itself: main-n:1 → main-p2:1 → main-p2:2 → main-n:1 \
         (synchronous calls)",
        "{violation}"
    );

    // Without the chain entry that made `main-p2:2` started, the later call
    // into p2 is one the runtime has not begun, and nothing waits for it:
    // no stack order, no cycle.
    let mut without_chain = with_refused.clone();
    without_chain[after].path = vec![format!("{p1_tag}/node-p1:1")];
    assert_eq!(
        refused_waits(&without_chain, &sessions).map(|violation| violation.to_string()),
        None,
        "the chain entry is what starts the later call"
    );
    // And without the one stack, the two calls into p2 are not ordered at
    // all: the same trace is allowed.
    let mut without_stack = with_refused.clone();
    let hello = without_stack
        .iter()
        .position(|frame| frame.from == "node-p2" && frame.op == "hello")
        .expect("p2's greeting");
    without_stack[hello].capabilities = vec!["reentrant-sync".into()];
    assert_eq!(
        refused_waits(&without_stack, &sessions).map(|violation| violation.to_string()),
        None,
        "the stack order edge is what closes the cycle"
    );
}

// --------------------------------------------------- §3.8, the host gone

/// guarantee: bridge/in-flight-call-fails-once, bridge/no-call-on-a-dead-session;
/// CHN-2-9, CHN-5-5, MNT-6-6.
///
/// §3.8: the host of a runtime goes away — its channel ends normally, as a
/// killed process's does. The call in flight fails with a transport error
/// (code `peer-gone`, `session/mod.rs` `Error::code`), the session ends
/// with `peer disconnected`, and nothing follows the far end's end: a frame
/// the gone host would have sent is not in an allowed trace, and a call
/// made after the end is refused here without a frame.
#[tokio::test(flavor = "multi_thread")]
async fn a_gone_host_fails_the_call_with_peer_gone() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let host = tokio::spawn(far_end("the host", Duration::from_secs(20), move || {
        let mut theirs = theirs;
        greet_as(&mut theirs, "node", &[]);
        // The runtime's call, then the host is gone: its channel ends the
        // way a killed process's does, with no code of its own.
        let call = read_frame(&mut theirs);
        assert_eq!(
            (call["op"].as_str(), call["id"].as_str()),
            (Some("invoke"), Some("main:1")),
            "{call}"
        );
        theirs.closer.close(Close::Done);
    }));
    within("the greeting", caller.ready()).await.unwrap();
    let call = caller
        .invoke_started("svc", "echo", Value::Data(json!("in flight")))
        .unwrap();
    within("the host going away", host).await.unwrap();

    let error = within("the call", call).await.unwrap_err();
    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert_eq!(
        error.code(),
        Some("peer-gone"),
        "a gone peer is the transport code, not a call's"
    );
    assert_eq!(error.to_string(), "peer disconnected");
    within("the caller ending", caller.closed()).await;
    assert!(
        caller.close_reason().is_some(),
        "the caller's session ended"
    );

    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed);
    assert_eq!(checked.last["node"], State::Closed);
    assert_eq!(
        checked.events("main").last(),
        Some(&Event::ClosedDone),
        "{lines:?}"
    );
    assert_eq!(
        checked.events("node").last(),
        Some(&Event::ClosedDone),
        "{lines:?}"
    );
    let main = checked.events("main");
    let ended = main
        .iter()
        .position(|event| *event == Event::Ended)
        .expect("the far end's end");
    // The host's end is the last message event: nothing is received after.
    assert!(
        main[ended + 1..]
            .iter()
            .all(|event| matches!(event, Event::SendFailed(_) | Event::ClosedDone)),
        "{lines:?}"
    );
    let all = frames.all();
    let checked = checked_frames(&all, &[("main", "node")], None);
    assert_eq!(
        checked.unanswered(),
        1,
        "the call in flight never answered: {all:?}"
    );
    assert_eq!(
        ops(&frames.from("main")),
        [("hello", None), ("invoke", Some("main:1"))]
    );

    // A call after the session ended is refused here, with no frame written.
    let after = caller
        .invoke_async("svc", "echo", Value::Data(json!("after")))
        .await
        .unwrap_err();
    assert_eq!(after.code(), Some("peer-gone"), "{after:?}");
    assert_eq!(
        ops(&frames.from("main")),
        [("hello", None), ("invoke", Some("main:1"))],
        "the refused call left no frame"
    );

    // The trace rule: a frame after the far end's end is not one the
    // channel allows, whatever it carries.
    let ended = lines
        .iter()
        .position(|line| line == "main: ended")
        .expect("the far end's end in the log");
    let mut late = lines.clone();
    late.insert(ended + 1, "main: received 40 bytes".into());
    let violation = rejected(&late);
    assert_eq!(violation.at, ended + 1, "{violation}");
    // And a close ends both directions: a stale command after it is not a
    // sent frame either.
    let mut stale = lines.clone();
    stale.push("main: sent 40 bytes".into());
    let violation = rejected(&stale);
    assert_eq!(violation.at, lines.len(), "{violation}");
}

/// The far end of [`a_killed_far_end_ends_the_session_as_peer_gone`]: a
/// shell that greets as `node`, reads one call, writes `marker`, then
/// waits as `sleep` (replacing itself, so the kill reaches the process
/// that holds the socket). `reader` is its two lines of shell.
#[cfg(unix)]
fn killed_far_end(marker: &std::path::Path) -> String {
    let hello = json!({
        "op": "hello",
        "version": ENDPOINT_PROTOCOL,
        "endpoint": "node",
        "capabilities": [],
    });
    format!(
        "read hello <&3 || exit 1\n\
         printf '%s\\n' '{hello}' >&3\n\
         read call <&3 || exit 1\n\
         : > '{}'\n\
         exec sleep 60\n",
        marker.display()
    )
}

/// guarantee: bridge/in-flight-call-fails-once, bridge/no-call-on-a-dead-session;
/// CHN-2-9, CHN-5-5, MNT-6-6.
///
/// §3.8, for real: the far end is a process of this test's own
/// (`pipe::spawn`), so it is killed with `SIGKILL` instead of closing its
/// channel. What the trace records is the channel's end with how the
/// process ended — a receive failure naming the signal, not `ended` — and
/// the call in flight fails with that same text: the session ends for the
/// transport's reason, `peer-gone`. A process that closed its channel
/// would say `peer disconnected` instead
/// ([`a_gone_host_fails_the_call_with_peer_gone`]).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_far_end_ends_the_session_as_peer_gone() {
    use rutis_bridge::pipe::{self, Processes, Spawn};

    let log = Log::default();
    let frames = Decoded::default();
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("call-read");
    let mut far = Spawn::new("sh");
    far.args = vec!["-c".into(), killed_far_end(&marker).into()];
    let processes = Processes::default();
    let channel = pipe::spawn(&far, &processes).await.unwrap();
    let channel = instrumented(channel, "main", "node", &log, &frames);
    let caller = Connection::open(
        channel,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    within("the greeting", caller.ready()).await.unwrap();

    let call = caller
        .invoke_started("svc", "echo", Value::Data(json!("in flight")))
        .unwrap();
    // The call is out: the far end read it and the session waits for the
    // reply that will never come.
    until(|| marker.exists(), "the far end to read the call").await;
    let pid = processes.pid().expect("the far end still runs");
    // SAFETY: SIGKILL to a process this test started and has not waited
    // for; its id cannot have been reused while it runs.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }

    let error = within("the call", call).await.unwrap_err();
    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert_eq!(
        error.code(),
        Some("peer-gone"),
        "a killed peer is transport"
    );
    // The channel's end says how the process ended, and the session ends
    // with that: not `peer disconnected`, which is a channel that ended of
    // its own accord.
    assert_eq!(
        error.to_string(),
        "the process exited with signal: 9 (SIGKILL)",
        "the ending names the kill"
    );
    within("the caller ending", caller.closed()).await;
    let status = eventually(|| processes.status(), "the far end's end").await;
    assert_eq!(
        status, "exited with signal: 9 (SIGKILL)",
        "a killed process, not one that closed its channel"
    );

    let lines = log.lines();
    let checked = accepted(&lines);
    assert_eq!(checked.last["main"], State::Closed, "{lines:?}");
    let receive_failed = lines
        .iter()
        .position(|line| {
            line == "main: receive failed: the process exited with signal: 9 (SIGKILL)"
        })
        .expect("the channel's end in the log");
    assert_eq!(
        checked.events("main")[receive_failed + 1],
        Event::ClosedDone,
        "the session closes the channel after its end: {lines:?}"
    );
    assert_eq!(receive_failed + 2, lines.len(), "{lines:?}");

    let all = frames.all();
    checked_frames(&all, &[("main", "node")], None);
    // The far end sent its greeting and nothing else: the call it read was
    // never answered.
    assert_eq!(
        ops(&frames.from("node")),
        [("hello", None)],
        "the far end only greets: {all:?}"
    );
    // Two frames left this side: the greeting and the call in flight.
    let sent = |checked: &Checked| {
        checked
            .events("main")
            .iter()
            .filter(|event| matches!(event, Event::Sent(_)))
            .count()
    };
    assert_eq!(
        sent(&checked),
        2,
        "the greeting and the one call: {lines:?}"
    );

    // A call after the end is refused here, with no frame written.
    let after = caller
        .invoke_async("svc", "echo", Value::Data(json!("after")))
        .await
        .unwrap_err();
    assert_eq!(after.code(), Some("peer-gone"), "{after:?}");
    assert_eq!(
        sent(&checked),
        2,
        "the refused call left no frame: {lines:?}"
    );

    // The trace rule: nothing is received after a receive failed, and
    // nothing is sent after the close.
    let mut late = lines.clone();
    late.insert(receive_failed + 1, "main: received 40 bytes".into());
    let violation = rejected(&late);
    assert_eq!(violation.at, receive_failed + 1, "{violation}");
    assert!(violation.why.contains("a receive failed"), "{violation}");
    let mut stale = lines.clone();
    stale.push("main: sent 40 bytes".into());
    let violation = rejected(&stale);
    assert_eq!(violation.at, lines.len(), "{violation}");
}

// ---------------------------------------------- §3.8, leases and takeover

/// A kernel greeting as `name` (`LocalId`, `session_plugin.rs`).
fn kernel(name: &str) -> Ctx {
    let root = Ctx::root().unwrap();
    root.provide(LocalId(id(name))).unwrap();
    root
}

/// A far end's service: `get` answers `answer` (`session_plugin.rs`).
struct Answer(&'static str);

impl HostDispatch for Answer {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        match method {
            "get" => Ok(Value::Data(json!(self.0))),
            _ => Ok(args),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "get": "async" }))
    }
}

/// The plugin `name` providing `service`, whose `get` answers `answer`: what
/// a lease serves while it is mounted.
struct Provides {
    name: &'static str,
    service: &'static str,
    answer: &'static str,
}

impl DynPlugin for Provides {
    fn name(&self) -> &str {
        self.name
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, rutis::Error>> {
        Box::pin(async move {
            ctx.provide_at(host_key(self.service), Arc::new(Answer(self.answer)))?;
            Ok(Effect::Done)
        })
    }
}

/// One event of a lease's life, in the order it happened: a channel's trace
/// line, and what the fixture saw of the takeover.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LeaseEvent {
    /// A line of `channel::trace`, as §2 writes it.
    Channel(String),
    /// The newer connection's handshake completed (`Session::handshake`).
    Greeted,
    /// The old session's fiber ended: its far plugins are unloaded.
    Unloaded,
    /// The newer session mounted: the new lease serves.
    Installed,
    /// A frame sent on the old lease's channel after the takeover: refused
    /// before it crossed.
    RefusedStale,
}

/// A lease's events as they happened, shared by the channel sinks and the
/// test: a channel's line and the steps the test takes between them, in one
/// order, which is what the tickets of `Records` keep.
type Lease = Records<LeaseEvent>;

impl From<String> for LeaseEvent {
    fn from(line: String) -> Self {
        Self::Channel(line)
    }
}

impl Records<LeaseEvent> {
    /// A step the test saw between the channels' lines, filed where it was.
    fn mark(&self, event: LeaseEvent) {
        self.note(event);
    }

    fn events(&self) -> Vec<LeaseEvent> {
        self.entries()
    }
}

/// A sender this test keeps after the session took the channel: what a
/// session of a replaced lease would try to send with.
struct Held(Arc<Mutex<Box<dyn Sender>>>);

impl Sender for Held {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        self.0.lock().unwrap().send(message)
    }

    fn max_message(&self) -> Option<usize> {
        self.0.lock().unwrap().max_message()
    }
}

/// What a lease trace allows (#624, `specs/remote-lease/RemoteLease.tla`:
/// `OneLease`, `OldCommandsStayOld`, `CleanBeforeHello`; the host's
/// install order, `crates/rutis-host/src/connect/listen.rs:4-10`): the
/// channels' lines are §2 traces; the newer connection greets before the
/// old lease is unloaded; the old lease's channel closes as replaced before
/// its fiber ends; its far plugins are unloaded before the newer session
/// mounts; and a command of the old lease after that is refused, never
/// sent.
fn check_lease(events: &[LeaseEvent]) -> Result<(), Violation> {
    // The channels' lines first, as §2 allows them.
    let lines: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            LeaseEvent::Channel(line) => Some(line.as_str()),
            _ => None,
        })
        .collect();
    if let Err(violation) = check_trace(&lines) {
        let at = events
            .iter()
            .enumerate()
            .filter(|(_, event)| matches!(event, LeaseEvent::Channel(_)))
            .nth(violation.at)
            .map(|(at, _)| at)
            .unwrap_or(violation.at);
        return Err(Violation { at, ..violation });
    }
    let at = |what: fn(&LeaseEvent) -> bool| -> Result<usize, Violation> {
        events.iter().position(what).ok_or_else(|| Violation {
            at: events.len(),
            text: "the lease trace".into(),
            why: "the lease's life is missing a step".into(),
        })
    };
    let greeted = at(|event| *event == LeaseEvent::Greeted)?;
    let unloaded = at(|event| *event == LeaseEvent::Unloaded)?;
    let installed = at(|event| *event == LeaseEvent::Installed)?;
    if !(greeted < unloaded && unloaded < installed) {
        let (at, why) = if installed < unloaded {
            (
                installed,
                "the newer session mounts before the old lease is cleaned: one lease at a time",
            )
        } else if unloaded < greeted {
            (
                unloaded,
                "the old lease is cleaned before the newer connection greeted: nothing asked for \
                 the takeover",
            )
        } else {
            (installed, "the newer session mounts before its handshake")
        };
        let text = format!("{:?}", events[at]);
        return Err(Violation {
            at,
            text,
            why: why.into(),
        });
    }
    // The old lease's channel closes as replaced before its fiber ends.
    let replaced = events.iter().position(
        |event| matches!(event, LeaseEvent::Channel(line) if line == "lease-1: closed: replaced"),
    );
    match replaced {
        Some(at) if at < unloaded => {}
        _ => {
            return Err(Violation {
                at: unloaded,
                text: format!("{:?}", events[unloaded]),
                why: "the old lease's far plugins are cleaned before its channel closed as \
                      replaced: the far end reads the end before it is replaced"
                    .into(),
            })
        }
    }
    // A command of the old lease after the takeover is refused, and no
    // frame of it is sent.
    if let Some(refused) = events
        .iter()
        .position(|event| *event == LeaseEvent::RefusedStale)
    {
        if refused < installed {
            return Err(Violation {
                at: refused,
                text: format!("{:?}", events[refused]),
                why: "a stale command is refused before the new lease is installed: the old \
                      lease still serves"
                    .into(),
            });
        }
        if events[refused..]
            .iter()
            .any(|event| matches!(event, LeaseEvent::Channel(line) if line.starts_with("lease-1: sent ")))
        {
            let at = events[refused..]
                .iter()
                .position(|event| {
                    matches!(event, LeaseEvent::Channel(line) if line.starts_with("lease-1: sent "))
                })
                .unwrap()
                + refused;
            return Err(Violation {
                at,
                text: format!("{:?}", events[at]),
                why: "a stale command of the old lease crossed after the takeover: it must be \
                      refused"
                    .into(),
            });
        }
    }
    Ok(())
}

/// guarantee: loader/remote-runtime-lease; REM-4.4-4, REM-4.4-5, REM-4.4-7,
/// REM-4.4-9; `specs/remote-lease/RemoteLease.tla` `OneLease`,
/// `OldCommandsStayOld`, `CleanBeforeHello`.
///
/// §3.8 and `specs/remote-lease`: a newer connection of the same id takes
/// the lease over. Its handshake completes while the old lease still
/// serves; the old session then unloads (its far plugins too), its channel
/// closes as replaced, and only then does the newer session mount. A
/// command of the old lease afterwards is refused, and no frame of it
/// reaches the far end.
#[tokio::test(flavor = "multi_thread")]
async fn a_takeover_cleans_the_old_lease_before_it_installs_the_new() {
    let lease = Lease::default();
    let (main, node) = (kernel("main"), kernel("node"));
    let newer = kernel("node");
    (&node.plugin(Provides {
        name: "clock",
        service: "clock",
        answer: "noon",
    }))
        .await
        .unwrap();
    (&newer.plugin(Provides {
        name: "clock",
        service: "clock",
        answer: "later",
    }))
        .await
        .unwrap();

    // The first lease: both ends of one channel.
    let (ours, theirs) = rutis_bridge::memory::pair();
    let held = Arc::new(Mutex::new(ours.sender));
    let ours = Channel {
        sender: Box::new(Held(held.clone())),
        receiver: ours.receiver,
        closer: ours.closer,
    };
    let ours = traced_channel(ours, "lease-1", &lease);
    // The host's `Replaceable` closes the old connection as replaced
    // (`crates/rutis-host/src/connect/listen.rs:204-220`).
    let ours = one_reason(ours, Close::Replaced);
    let theirs = traced_channel(theirs, "lease-1-far", &lease);
    let old = main.plugin(Session::new(ours));
    let old_far = node.plugin(Session::new(theirs));
    within("the first lease", async {
        (&old).await.unwrap();
        (&old_far).await.unwrap();
    })
    .await;
    eventually(
        || main.get_at(&host_key("clock")).map(|_| ()),
        "the old lease's service at the host",
    )
    .await;
    let answer: String = within(
        "the old lease's service",
        call_service(&main, "clock", "get", ()),
    )
    .await
    .unwrap();
    assert_eq!(answer, "noon");

    // A newer connection of the same id: its handshake completes, nothing
    // is mounted yet and the old lease still serves.
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = traced_channel(ours, "lease-2", &lease);
    let theirs = traced_channel(theirs, "lease-2-far", &lease);
    let next = newer.plugin(Session::new(theirs));
    let greeted = within("the new handshake", Session::handshake(&main, ours))
        .await
        .unwrap();
    within("the new far end", async { (&next).await.unwrap() }).await;
    lease.mark(LeaseEvent::Greeted);
    let answer: String = within(
        "the old lease still serving",
        call_service(&main, "clock", "get", ()),
    )
    .await
    .unwrap();
    assert_eq!(answer, "noon", "the old lease serves until it is cleaned");

    // The old lease is cleaned: its far plugins are unloaded before the
    // newer session may mount.
    within("the old lease unloading", old.dispose())
        .await
        .unwrap();
    lease.mark(LeaseEvent::Unloaded);
    eventually(
        || main.get_at(&host_key("clock")).is_none().then_some(()),
        "the old lease's far plugins unloaded",
    )
    .await;

    let installed = main.plugin(greeted);
    within("the new lease", async { (&installed).await.unwrap() }).await;
    lease.mark(LeaseEvent::Installed);
    eventually(
        || main.get_at(&host_key("clock")).map(|_| ()),
        "the new lease's service at the host",
    )
    .await;
    let answer: String = within(
        "the new lease's service",
        call_service(&main, "clock", "get", ()),
    )
    .await
    .unwrap();
    assert_eq!(answer, "later", "the new lease serves");

    // A command of the old lease after the takeover is refused here: the
    // channel was closed as replaced, and the far end is gone.
    held.lock()
        .unwrap()
        .send(br#"{"op":"invoke","id":"main:1","path":[],"target":"t","method":"m","args":{"type":"undefined"}}"#)
        .unwrap_err();
    lease.mark(LeaseEvent::RefusedStale);
    let answer: String = within(
        "the new lease after the stale command",
        call_service(&main, "clock", "get", ()),
    )
    .await
    .unwrap();
    assert_eq!(
        answer, "later",
        "a command of the old lease does not act on the new one"
    );

    let events = lease.events();
    check_lease(&events)
        .unwrap_or_else(|violation| panic!("the lease trace was rejected: {violation}"));

    // Counterexamples: the same life with one step moved.
    let mut moved = events.clone();
    let installed_at = moved
        .iter()
        .position(|event| *event == LeaseEvent::Installed)
        .unwrap();
    let unloaded_at = moved
        .iter()
        .position(|event| *event == LeaseEvent::Unloaded)
        .unwrap();
    moved.swap(unloaded_at, installed_at);
    let violation = match check_lease(&moved) {
        Ok(()) => panic!("an install before the cleanup was accepted"),
        Err(violation) => violation,
    };
    assert!(
        violation.why.contains("one lease at a time"),
        "the newer session mounted before the old lease was cleaned: {violation}"
    );

    // A stale command that crossed after the takeover is not a lease trace.
    let mut stale = events.clone();
    stale.insert(
        events.len(),
        LeaseEvent::Channel("lease-1: sent 80 bytes".into()),
    );
    let violation = match check_lease(&stale) {
        Ok(()) => panic!("a stale command crossing after the takeover was accepted"),
        Err(violation) => violation,
    };
    assert!(
        violation.why.contains("nothing crosses it after the close"),
        "{violation}"
    );

    // Nor is a command of the old lease refused while it still serves: the
    // takeover is what makes it stale.
    let mut early_refusal = events.clone();
    let greeted_at = early_refusal
        .iter()
        .position(|event| *event == LeaseEvent::Greeted)
        .unwrap();
    let refused_at = early_refusal
        .iter()
        .position(|event| *event == LeaseEvent::RefusedStale)
        .unwrap();
    let refused = early_refusal.remove(refused_at);
    early_refusal.insert(greeted_at + 1, refused);
    let violation = match check_lease(&early_refusal) {
        Ok(()) => panic!("a refusal before the install was accepted"),
        Err(violation) => violation,
    };
    assert!(
        violation.why.contains("before the new lease is installed"),
        "{violation}"
    );

    // The cleanup comes after the old channel's close as replaced; a trace
    // that unloads the session before closing its channel is not one.
    let mut early = events.clone();
    let replaced = early
        .iter()
        .position(|event| {
            matches!(event, LeaseEvent::Channel(line) if line == "lease-1: closed: replaced")
        })
        .unwrap();
    let unloaded_at = early
        .iter()
        .position(|event| *event == LeaseEvent::Unloaded)
        .unwrap();
    early.swap(replaced, unloaded_at);
    let violation = match check_lease(&early) {
        Ok(()) => panic!("a cleanup before the replaced close was accepted"),
        Err(violation) => violation,
    };
    assert!(violation.why.contains("closed as replaced"), "{violation}");
}

// ------------------------------------------- §3.8, an access point's ends

/// rule: DIA-5-7, DIA-5-11, CHN-5-8.
///
/// §3.8 and `specs/remote-lease/AccessPoint.tla`: an identity the access
/// point admitted is revoked — the listen side closes its connection with
/// `Close::Rejected` (4003, "the listen side"). The call in flight and the
/// session end as an identity refusal (an authentication failure, never a
/// retryable one, by the error's kind), nothing crosses the channel after
/// the rejection, and a call made after it is refused here.
///
/// The `AccessEvent` log of the old access point was removed with #695; the
/// listener that admits today (`crates/rutis-host/src/connect/listen.rs`)
/// is in another crate. What this crate sees of an admission is its
/// session's ending: admitted (a greeting and calls), refused after the
/// hello (4003, above), or refused before it (403,
/// `a_rejected_channel_is_an_allowed_trace`).
/// rule: DIA-5-7, DIA-5-11, CHN-5-8.
///
/// §3.8 and §3.2: a connection whose greeting names another endpoint is
/// not an admitted session, whatever it asks for afterwards: the session
/// fails as an endpoint mismatch and the frame trace rejects the greeting.
#[tokio::test(flavor = "multi_thread")]
async fn a_greeting_that_names_another_endpoint_is_not_admitted() {
    let log = Log::default();
    let frames = Decoded::default();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = instrumented(ours, "main", "node", &log, &frames);
    let theirs = instrumented(theirs, "node", "main", &log, &frames);
    let caller = Connection::open(
        ours,
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    )
    .unwrap();
    let far = tokio::spawn(far_end("the far end", Duration::from_secs(20), move || {
        let mut theirs = theirs;
        let hello = read_frame(&mut theirs);
        assert_eq!(hello["op"], "hello", "{hello}");
        write_frame(
            &mut theirs,
            json!({
                "op": "hello",
                "version": ENDPOINT_PROTOCOL,
                "endpoint": "impostor",
            }),
        );
        // What it reads next, and how its channel ends.
        theirs.receiver.recv().map(|message| message.is_none())
    }));
    let error = within("the greeting", caller.ready()).await.unwrap_err();
    assert!(
        matches!(error, Error::Handshake(Handshake::IdentityMismatch(_))),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "endpoint mismatch: the far end greeted as impostor, but node is the expected endpoint"
    );
    within("the caller ending", caller.closed()).await;
    let ended = within("the far end's channel", far).await.unwrap();
    assert_eq!(ended, Ok(true), "the channel closed with Done");

    let lines = log.lines();
    accepted(&lines);
    let all = frames.all();
    let violation = match check_admission(&all, &[("main", "node")]) {
        Ok(()) => panic!("a greeting naming another endpoint was admitted: {all:?}"),
        Err(violation) => violation,
    };
    assert_eq!(violation.at, 1, "{violation}");
    assert!(violation.why.contains("names the endpoint"), "{violation}");
}
