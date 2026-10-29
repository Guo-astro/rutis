//! Entry points of the fuzz targets in `fuzz/` (#179; standard Q6.4.2,
//! risks P2, E7). Built only by cargo-fuzz (`--cfg fuzzing`); each runs
//! the code that reads input from the far end and panics when one of its
//! properties fails, which the fuzzer reports as a crash.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

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
pub fn error_graph(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(graph) = serde_json::from_str::<Json>(text) else {
        return;
    };
    let error = crate::session::Error::Remote {
        name: "E".into(),
        message: "m".into(),
        graph: Some(graph),
    };
    let _ = (error.code(), error.data(), error.stack(), error.site());
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
    let Ok(frame) = parse_frame(data) else {
        return;
    };
    let written = serde_json::to_vec(&frame).expect("a frame read writes");
    let again = parse_frame(&written)
        .unwrap_or_else(|error| panic!("a frame written does not read back: {error}"));
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

/// A stream that hands out at most `piece` bytes per read.
struct Pieces {
    data: Vec<u8>,
    at: usize,
    piece: usize,
}

impl Read for Pieces {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = buffer.len().min(self.piece).min(self.data.len() - self.at);
        buffer[..count].copy_from_slice(&self.data[self.at..self.at + count]);
        self.at += count;
        Ok(count)
    }
}

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
