//! Entry points of the fuzz targets in `fuzz/` (#179; standard Q6.4.2,
//! risks P2, E7). Built only by cargo-fuzz (`--cfg fuzzing`); each runs
//! the code that reads input from the far end and panics when one of its
//! properties fails, which the fuzzer reports as a crash.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value as Json;

use crate::channel::{ChannelError, Closer};
use crate::pipe::lines;
use crate::session::protocol::{Frame, WireValue};
use crate::session::rpc::{decode_text, deeper_than, parse_frame, MAX_FRAME_DEPTH};
use crate::session::values::{base64_encode, check_keys};
use crate::session::Value;

/// An error graph from the far end, as a host reads and prints it
/// (`crate::thrown`, `Error::site`, `Error::stack`, `Error::code`,
/// [`crate::report`]), and as a Rust method throws it on
/// ([`crate::exports::rethrown`], [`crate::exports::located`]): reading
/// never panics, and what is printed starts
/// with the error's own text, made printable (the diagnostics design §4.6).
///
/// Its `data` is read too, within limits (#631,
/// `bridge/error-code-and-data b`): a graph is followed without
/// memoization, so a node naming itself twice, 64 levels of the depth limit
/// [`MAX_DEPTH`](crate::session::MAX_DEPTH), is 2^64 steps; a graph past the
/// limits below is left unread, and one within them reads in bounded time and
/// size, crosses again unchanged, and leaves out what it cannot hold while the
/// rest of the error still reads.
pub fn error_graph(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if text.len() > GRAPH_BYTES {
        return;
    }
    let Ok(graph) = serde_json::from_str::<Json>(text) else {
        return;
    };
    let error = crate::session::Error::Remote {
        name: "E".into(),
        message: "m".into(),
        graph: Some(graph),
    };
    let _ = (error.code(), error.stack(), error.site());
    error_data(&error);
    // A Rust method throwing it on, or locating it, never panics either.
    let error = crate::exports::rethrown(error, "m", "f.rs", 1);
    let error = crate::exports::located(error, "m", "f.rs", 1);
    let _ = error.rethrown();
    let site = crate::session::Site::new("row", "call");
    let error = error.with_site(&site);
    let kind = rutis::host::ErrorKind::PluginFailed(Box::new(error));
    let printed = crate::report::report(&kind);
    // The first line is the error's text, or a headline ending with it.
    let first = printed.lines().next().unwrap_or_default();
    assert!(first.ends_with("E: m"), "{printed}");
    assert!(
        crate::report::whereabouts(&kind).len() <= crate::report::MAX_LINES,
        "bounded"
    );
}

/// The largest graph one input is read as.
const GRAPH_BYTES: usize = 64 * 1024;
/// The most values reading a graph's data may visit, and the most bytes of
/// its data it may copy: the limits a graph from the far end is read within
/// (see [`error_graph`]).
const DATA_STEPS: usize = 4096;
const DATA_BYTES: usize = 256 * 1024;
/// How long reading such a graph's data may take.
const DATA_TIME: Duration = Duration::from_secs(1);

/// What reading one value of an error graph costs, as
/// [`crate::session::Error::data`] (`session/mod.rs` `graph_value`) reads it:
/// a step for each value visited, the bytes of each data value copied.
#[derive(Clone, Copy)]
struct Cost {
    steps: usize,
    bytes: usize,
}

impl Cost {
    const ONE: Cost = Cost { steps: 1, bytes: 0 };

    fn add(self, other: Cost) -> Cost {
        Cost {
            steps: self.steps.saturating_add(other.steps),
            bytes: self.bytes.saturating_add(other.bytes),
        }
    }

    fn over(self) -> bool {
        self.steps > DATA_STEPS || self.bytes > DATA_BYTES
    }
}

/// `Error::data` on a graph from the far end, within the limits above
/// (#631, `bridge/error-code-and-data b`): what it reads crosses again
/// unchanged, and data it cannot read is left out while the rest of the
/// error still reads.
fn error_data(error: &crate::session::Error) {
    let crate::session::Error::Remote {
        graph: Some(graph), ..
    } = error
    else {
        return;
    };
    let Some(data) = graph_data(graph) else {
        return;
    };
    // What reading it would cost, without reading it: the graph is not
    // expanded here, so the caps hold whatever the far end sent.
    if graph_cost(graph, data, 0, &mut HashMap::new()).over() {
        return;
    }
    let started = Instant::now();
    let read = error.data();
    let elapsed = started.elapsed();
    assert!(
        elapsed <= DATA_TIME,
        "reading a graph's data took {elapsed:?}"
    );
    let Some(read) = read else {
        // Data the graph cannot hold is left out; the rest still reads.
        assert_eq!(error.to_string(), "E: m");
        return;
    };
    let size = written(&read);
    assert!(size <= DATA_BYTES, "a graph's data read as {size} bytes");
    // Read again, it crosses as itself, with the rest of the error.
    let crossed = crate::exports::failure("E", "C", "m".into(), Some(read.clone()));
    assert_eq!(crossed.code(), Some("C"));
    assert_eq!(crossed.data(), Some(read), "data changed on its way back");
    assert_eq!(crossed.to_string(), "E: m");
}

/// The root node's `data` of a graph, as `Error::data` finds it.
fn graph_data(graph: &Json) -> Option<&Json> {
    let root = graph.get("root")?;
    if root.get("type")?.as_str()? != "reference" {
        return None;
    }
    graph
        .get("nodes")?
        .get(root.get("value")?.as_u64()? as usize)?
        .get("data")
}

/// What reading `value` at `depth` costs, following references as
/// `graph_value` does, memoized by the node and depth (a graph may name one
/// node many times, and the node may name itself); an upper bound, as a
/// malformed branch stops the real walk early.
fn graph_cost(
    graph: &Json,
    value: &Json,
    depth: usize,
    memo: &mut HashMap<(usize, usize), Cost>,
) -> Cost {
    if depth > crate::session::MAX_DEPTH {
        return Cost::ONE;
    }
    let Some(kind) = value.get("type").and_then(Json::as_str) else {
        return Cost::ONE;
    };
    match kind {
        "data" => Cost {
            bytes: value.get("value").map_or(0, written),
            ..Cost::ONE
        },
        "bigint" | "number" => Cost {
            bytes: value
                .get("value")
                .and_then(Json::as_str)
                .map_or(0, str::len),
            ..Cost::ONE
        },
        "reference" => {
            let Some(index) = value.get("value").and_then(Json::as_u64) else {
                return Cost::ONE;
            };
            let index = index as usize;
            if let Some(cost) = memo.get(&(index, depth)) {
                return *cost;
            }
            let cost = (|| {
                let node = graph.get("nodes")?.get(index)?;
                let mut cost = Cost::ONE;
                match node.get("type")?.as_str()? {
                    "array" => {
                        for item in node.get("values")?.as_array()? {
                            cost = cost.add(graph_cost(graph, item, depth + 1, memo));
                        }
                    }
                    "object" => {
                        for entry in node.get("values")?.as_array()? {
                            cost.bytes = cost.bytes.saturating_add(entry.get(0)?.as_str()?.len());
                            cost = cost.add(graph_cost(graph, entry.get(1)?, depth + 1, memo));
                        }
                    }
                    _ => {}
                }
                Some(cost)
            })()
            .unwrap_or(Cost::ONE);
            memo.insert((index, depth), cost);
            cost
        }
        _ => Cost::ONE,
    }
}

/// How many bytes `value` writes as JSON.
fn written(value: &Json) -> usize {
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

/// A far end's frames, in sequence, into a live session (#631): the greeting
/// this side expects, then each line of `data` as one frame, with a call
/// after it whose reply says the session handled that frame up to there. The
/// session serves the conformance fixture (`crate::session::testing::Fixture`),
/// so frames may invoke it, name what it granted (`make` and `later` grant
/// references), call a function it was handed (`apply`, `reenter`), keep one
/// (`hold`, `fire`), release them, cancel calls, and answer the calls the
/// fixture makes; whatever a far end sends, the session neither panics nor
/// hangs.
///
/// One frame at a time: the call after each frame takes the next id, so the
/// session takes them in that order, and its reply means the frame before it
/// was handled. The frames' own call ids are renumbered to leave room for
/// those calls (`far:1`, the call after it `far:2`, and so on), a `path` or a
/// `cancel` still naming the call it named. (A far end that sends frames at
/// once may close the session while a call that returns a future is still
/// answered; the future holds the session and the export it lands in after
/// the close: #1216.)
pub fn session_frames(data: &[u8]) {
    use crate::channel::{Channel, Close, PeerId};
    use crate::session::rpc::{Connection, Dispatch, Endpoint};
    use crate::session::testing::Fixture;

    /// The most frames one input sends.
    const FRAMES: usize = 32;
    /// How long one input may take.
    const WAIT: Duration = Duration::from_secs(5);

    let Ok(local) = PeerId::new("fuzz") else {
        return;
    };
    let Ok(far_end) = PeerId::new("far") else {
        return;
    };
    let frames: Vec<Vec<u8>> = data
        .split(|byte| *byte == b'\n')
        .filter(|frame| !frame.is_empty())
        .take(FRAMES)
        .map(<[u8]>::to_vec)
        .collect();
    let (this, far) = crate::memory::pair();
    let peer = {
        let _entered = session_runtime().enter();
        let dispatch: Arc<dyn Dispatch> = Arc::new(Fixture::default());
        match Connection::open(this, dispatch, Endpoint::rust(local)) {
            Ok(peer) => peer,
            Err(_) => return,
        }
    };
    let Channel {
        sender,
        receiver,
        closer,
    } = far;
    let (done, finished) = mpsc::channel();
    let script = std::thread::spawn(move || {
        let mut sender = sender;
        let mut receiver = receiver;
        let hello = serde_json::to_vec(&serde_json::json!({
            "op": "hello",
            "version": crate::session::ENDPOINT_PROTOCOL,
            "endpoint": far_end.as_str(),
            "implementation": { "name": "fuzz", "version": "0" },
        }))
        .expect("a greeting writes");
        if sender.send(&hello).is_err() {
            let _ = done.send(());
            return;
        }
        // The next far-end call id: one past every frame's own, which is
        // renumbered as it is sent.
        let mut next = 1;
        let mut renamed = HashMap::new();
        for frame in frames {
            let frame = rename(&frame, far_end.as_str(), &mut renamed, &mut next);
            if sender.send(&frame).is_err() {
                break;
            }
            let id = format!("{far_end}:{next}");
            next += 1;
            let barrier = serde_json::to_vec(&serde_json::json!({
                "op": "invoke", "id": id, "path": [],
                "target": "conformance", "method": "echo",
                "args": { "type": "list", "value": [] },
            }))
            .expect("a call writes");
            if sender.send(&barrier).is_err() {
                break;
            }
            // What the session wrote until that call's reply: calls the
            // fixture makes need an answer, or it waits for them.
            loop {
                let Ok(Some(message)) = receiver.recv() else {
                    let _ = done.send(());
                    return;
                };
                let Ok(frame) = serde_json::from_slice::<Json>(&message) else {
                    continue;
                };
                match frame.get("op").and_then(Json::as_str) {
                    Some("return" | "throw")
                        if frame.get("id").and_then(Json::as_str) == Some(id.as_str()) =>
                    {
                        break;
                    }
                    Some("invoke" | "call" | "get" | "await") => {
                        let Some(call) = frame.get("id") else {
                            continue;
                        };
                        let answer = serde_json::json!({
                            "op": "return", "id": call, "value": { "type": "undefined" },
                        });
                        let Ok(bytes) = serde_json::to_vec(&answer) else {
                            continue;
                        };
                        if sender.send(&bytes).is_err() {
                            let _ = done.send(());
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }
        let _ = done.send(());
    });
    // The session handles every frame or ends; one that does neither is a
    // hang, failed here rather than left for the fuzzer's own timeout.
    if finished.recv_timeout(WAIT).is_err() {
        panic!("the session handled no frame for {WAIT:?}");
    }
    closer.close(Close::Done);
    let _ = script.join();
    drop(peer);
}

/// The runtime a live session's dispatches run on, made once and kept: a
/// session's future may hold its session for as long as it runs, and a
/// runtime dropped under the two of them leaves them waiting on each other.
fn session_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .thread_name("rutis-fuzz-session")
            .build()
            .expect("a fuzz runtime")
    })
}

/// One frame, its call id and the ones its `path` names renumbered from
/// `renamed` (`far:1`, `far:2`...), taking `next` for a new one: the call
/// after the frame then takes one past it.
fn rename(frame: &[u8], far: &str, renamed: &mut HashMap<String, u64>, next: &mut u64) -> Vec<u8> {
    let Ok(mut parsed) = serde_json::from_slice::<Json>(frame) else {
        return frame.to_vec();
    };
    match parsed.get("op").and_then(Json::as_str) {
        Some("invoke" | "call" | "get" | "await") => {
            let Some(own) = parsed
                .get("id")
                .and_then(Json::as_str)
                .filter(|id| far_call(id, far))
                .map(str::to_owned)
            else {
                return frame.to_vec();
            };
            let new = match renamed.get(&own) {
                Some(new) => *new,
                None => {
                    let new = *next;
                    *next += 1;
                    renamed.insert(own, new);
                    new
                }
            };
            parsed["id"] = Json::String(format!("{far}:{new}"));
            if let Some(path) = parsed.get_mut("path").and_then(Json::as_array_mut) {
                for entry in path {
                    if let Some(text) = entry.as_str() {
                        if let Some(new) = renamed.get(text) {
                            *entry = Json::String(format!("{far}:{new}"));
                        }
                    }
                }
            }
        }
        // A call the far end gave up names one of its own: renamed with it.
        Some("cancel") => {
            let Some(id) = parsed.get("id").and_then(Json::as_str) else {
                return frame.to_vec();
            };
            let Some(new) = renamed.get(id) else {
                return frame.to_vec();
            };
            parsed["id"] = Json::String(format!("{far}:{new}"));
        }
        _ => return frame.to_vec(),
    }
    serde_json::to_vec(&parsed).unwrap_or_else(|_| frame.to_vec())
}

/// Whether `id` is a call of the far end's: its endpoint, then a number.
fn far_call(id: &str, far: &str) -> bool {
    id.strip_prefix(far)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|number| number.bytes().all(|byte| byte.is_ascii_digit()))
}

/// A frame as the session's reader takes it: the depth check, then the
/// parse, then each value in it decoded as `Peer::decode` decodes it
/// (references aside: they need a session). The decoder must never panic;
/// what it reads back must write and read again unchanged:
///
/// - a frame: written again, read again and written once more, the two
///   writes are the same text;
/// - each tagged value (instant, datetime, date, time, bytes, map, set,
///   bigint, decimal, uuid) whose text decodes: its text written from the
///   decoded value decodes to the same value, and bytes, which take only
///   canonical base64, write back the very text.
pub fn session_frame(data: &[u8]) {
    if deeper_than(data, MAX_FRAME_DEPTH) {
        // The reader fails the frame's call without parsing the frame.
        return;
    }
    let Ok(parsed) = parse_frame(data) else {
        return;
    };
    let frame = parsed.frame;
    let written = serde_json::to_vec(&frame).expect("a frame read writes");
    let again = parse_frame(&written)
        .unwrap_or_else(|error| panic!("a frame written does not read back: {error}"))
        .frame;
    let rewritten = serde_json::to_vec(&again).expect("a frame read writes");
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(&rewritten),
        "a frame changed on its way back"
    );
    for value in values(frame) {
        let _ = decode(value);
    }
}

/// The values a frame carries.
fn values(frame: Frame) -> Vec<WireValue> {
    match frame {
        Frame::Invoke { args, .. } | Frame::Call { args, .. } => vec![args],
        Frame::Return { value, .. } => vec![value],
        _ => Vec::new(),
    }
}

/// `Peer::decode` (session/rpc.rs) without a session: every tagged value's
/// text is decoded by the session's own code (`decode_text`), then written
/// back and checked; a reference, which needs a session, decodes to a
/// value no key may be, as a reference.
fn decode(value: WireValue) -> Result<Value, crate::session::Error> {
    let bytes_text = match &value {
        WireValue::Bytes(text) => Some(text.clone()),
        _ => None,
    };
    let value = match decode_text(value) {
        Ok(decoded) => {
            let decoded = decoded?;
            written_back(&decoded, bytes_text.as_deref());
            return Ok(decoded);
        }
        Err(value) => value,
    };
    Ok(match value {
        WireValue::Instant(_)
        | WireValue::DateTime(_)
        | WireValue::Date(_)
        | WireValue::Time(_)
        | WireValue::Bytes(_)
        | WireValue::BigInt(_)
        | WireValue::Decimal(_)
        | WireValue::Uuid(_) => unreachable!("decode_text decodes the tagged texts"),
        WireValue::Map(entries) => {
            let mut decoded = Vec::new();
            for (key, value) in entries {
                let value = match decode(value)? {
                    Value::Undefined => Value::Data(Json::Null),
                    value => value,
                };
                decoded.push((decode(key)?, value));
            }
            check_keys(decoded.iter().map(|(key, _)| key), "map key")?;
            Value::Map(decoded)
        }
        WireValue::Set(elements) => {
            let elements = elements
                .into_iter()
                .map(decode)
                .collect::<Result<Vec<_>, _>>()?;
            check_keys(&elements, "set element")?;
            Value::Set(elements)
        }
        WireValue::Undefined => Value::Undefined,
        WireValue::Data(value) => Value::Data(value),
        // Undefined inside a value is null, as the session decodes it
        // (#687 §七).
        WireValue::List(values) => Value::List(
            values
                .into_iter()
                .map(|value| decode(value).map(inside))
                .collect::<Result<_, _>>()?,
        ),
        WireValue::Record(fields) => Value::Record(
            fields
                .into_iter()
                .map(|(key, value)| Ok((key, inside(decode(value)?))))
                .collect::<Result<_, crate::session::Error>>()?,
        ),
        WireValue::Signal | WireValue::Reference { .. } => Value::Signal,
    })
}

/// A tagged value `decode_text` gave, written back: it decodes to the same
/// value and writes the same text again; bytes, which take only canonical
/// base64, write back the very text they came as (`bytes_text`).
fn written_back(value: &Value, bytes_text: Option<&str>) {
    match value {
        Value::Instant(value) => stable(value),
        Value::DateTime(value) => stable(value),
        Value::Date(value) => stable(value),
        Value::Time(value) => stable(value),
        Value::BigInt(value) => stable(value),
        Value::Decimal(value) => stable(value),
        Value::Uuid(value) => stable(value),
        Value::Bytes(bytes) => assert_eq!(
            Some(base64_encode(bytes).as_str()),
            bytes_text,
            "bytes changed on their way back"
        ),
        other => unreachable!("decode_text gave {other:?}"),
    }
}

/// The text written from `value` decodes to the same value, and writes the
/// same text again.
fn stable<T>(value: &T)
where
    T: std::str::FromStr<Err = crate::session::Error>
        + std::fmt::Display
        + PartialEq
        + std::fmt::Debug,
{
    let written = value.to_string();
    let again: T = written.parse().unwrap_or_else(|error| {
        panic!("{value:?} writes {written:?}, which does not decode: {error}")
    });
    assert_eq!(
        *value, again,
        "{value:?} changed on its way back through {written:?}"
    );
    assert_eq!(again.to_string(), written, "{value:?} writes two ways");
}

/// The local line framing (transport/local/lines.rs) reading a stream that
/// arrives in pieces: the first byte picks the message limit (0–255 bytes,
/// so the limit is reached), the second the size of each piece (1–16
/// bytes), the rest is the stream. What the receiver yields must be what a
/// plain split at newlines says, ending as it says; each message must send
/// back as the same line.
pub fn local_lines(data: &[u8]) {
    let [limit, piece, stream @ ..] = data else {
        return;
    };
    let (max, piece) = (usize::from(*limit), usize::from(piece % 16) + 1);
    let written = Arc::new(Mutex::new(Vec::new()));
    let mut channel = lines::limited(
        Pieces {
            data: stream.to_vec(),
            at: 0,
            piece,
        },
        Shared(written.clone()),
        Arc::new(Nothing),
        max,
    );
    let mut expected = expected_lines(stream, max).into_iter();
    loop {
        let got = channel.receiver.recv();
        let want = expected.next().expect("the receiver went on past the end");
        match (got, want) {
            (Ok(Some(line)), Expected::Line(want)) => {
                assert_eq!(line, want, "a message is not the line sent");
                channel
                    .sender
                    .send(&line)
                    .expect("a message received sends back");
            }
            (Ok(None), Expected::End) => break,
            (Err(ChannelError::Closed { reason }), Expected::TooBig) => {
                assert!(reason.contains("over the limit"), "{reason}");
                break;
            }
            (Err(ChannelError::Closed { reason }), Expected::Truncated) => {
                assert!(reason.contains("inside a message"), "{reason}");
                break;
            }
            (got, want) => panic!("received {got:?}, expected {want:?}"),
        }
    }
    // What was sent back is the stream up to the last complete line read.
    let sent = written.lock().unwrap().clone();
    assert!(
        stream.starts_with(&sent),
        "the lines sent back are not the stream"
    );
}

#[derive(Debug)]
enum Expected {
    Line(Vec<u8>),
    End,
    TooBig,
    Truncated,
}

/// What reading `stream` with messages of at most `max` bytes yields.
fn expected_lines(stream: &[u8], max: usize) -> Vec<Expected> {
    let mut out = Vec::new();
    let mut rest = stream;
    loop {
        match rest.iter().position(|&byte| byte == b'\n') {
            Some(end) if end > max => {
                out.push(Expected::TooBig);
                return out;
            }
            Some(end) => {
                out.push(Expected::Line(rest[..end].to_vec()));
                rest = &rest[end + 1..];
            }
            None if rest.is_empty() => {
                out.push(Expected::End);
                return out;
            }
            None if rest.len() > max => {
                out.push(Expected::TooBig);
                return out;
            }
            None => {
                out.push(Expected::Truncated);
                return out;
            }
        }
    }
}

/// The bytes a channel reads from, a piece at a time.
struct Pieces {
    data: Vec<u8>,
    at: usize,
    piece: usize,
}

impl Read for Pieces {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let rest = &self.data[self.at.min(self.data.len())..];
        let take = rest.len().min(buffer.len()).min(self.piece);
        buffer[..take].copy_from_slice(&rest[..take]);
        self.at += take;
        Ok(take)
    }
}

/// What a channel wrote, shared with the test.
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Nothing;

impl Closer for Nothing {
    fn close(&self, _: crate::channel::Close) {}
}

/// A WebSocket upgrade request as the listener reads it
/// (transport/websocket/listen.rs): tungstenite's server handshake with
/// the listener's limits, and in its callback the listener's own reading of
/// the path, the bearer token and the offered session protocols, then its
/// choice of protocol. Credentials stand in as accepted: checking them is
/// the connection plugin's job. Must not panic, whatever the bytes.
#[cfg(feature = "websocket")]
pub fn websocket_upgrade(data: &[u8]) {
    use crate::websocket::listen::{agree_protocol, read_upgrade};
    use crate::websocket::{ws_config, Limits};
    use tokio_tungstenite::tungstenite;
    use tungstenite::handshake::server::{Request, Response};

    let ours = format!("rutis.{}", crate::session::ENDPOINT_PROTOCOL);
    #[allow(clippy::result_large_err)] // tungstenite fixes the signature
    let check = |request: &Request, mut response: Response| {
        let upgrade = read_upgrade(request, "/rutis")?;
        // The connection plugin would compare the token; it is a part of the
        // header, as received.
        if let Some(token) = upgrade.bearer {
            let header = request.headers()["Authorization"].as_bytes();
            assert!(header.ends_with(token.as_bytes()));
        }
        let protocol = agree_protocol(&ours, &upgrade.offered)?;
        assert_eq!(protocol, ours.as_str());
        response
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", protocol);
        Ok(response)
    };
    let stream = Duplex {
        input: std::io::Cursor::new(data),
        output: Vec::new(),
    };
    let _ = tungstenite::accept_hdr_with_config(stream, check, Some(ws_config(&Limits::default())));
}

/// The two directions of a connection: the dialer's bytes in, the
/// listener's reply out.
#[cfg(feature = "websocket")]
struct Duplex<'a> {
    input: std::io::Cursor<&'a [u8]>,
    output: Vec<u8>,
}

#[cfg(feature = "websocket")]
impl Read for Duplex<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(buffer)
    }
}

#[cfg(feature = "websocket")]
impl Write for Duplex<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A value inside another: undefined is null there.
fn inside(value: Value) -> Value {
    match value {
        Value::Undefined => Value::Data(serde_json::Value::Null),
        value => value,
    }
}
