//! The session conformance suite (feature `testing`): what any
//! implementation of the session protocol must do, as checks an
//! implementation runs against itself. Each check panics with what was
//! broken.
//!
//! **Session** ([`session`]): the far end serves the target `conformance`:
//!
//! | method | does |
//! | --- | --- |
//! | `echo(value)` | returns `value` |
//! | `apply(fn, value)` | returns `fn(value)`, calling back during the call |
//! | `later(value)` | returns a future of `value` |
//! | `fail(name, message)` | throws an error named `name` |
//! | `hold(fn)` / `fire(value)` / `drop()` | keeps `fn`, calls it, lets it go |
//! | `abortable(signal)` | returns a future that ends once `signal` aborts |
//! | `started()` | whether an `abortable` call has reached it |
//! | `aborted()` | whether such a signal aborted |
//! | `reenter(fn)` | returns `fn()`; `fn` calls `echo` while the far end waits |
//! | `keep(slot, fn, ...)` / `free(slot)` | keeps `fn` in `slot`, releases it |
//! | `make()` | returns a new function |
//! | `tables()` | `{ exports, imports }`: the far end's reference table sizes |
//!
//! `keep` and `free` serve the release-count property test
//! (`tests/value_roundtrip.rs`, risk P4); the Rust, Node and Python fixtures
//! have them, [`session`] does not call them. `make` also serves the
//! malformed matrix: [`MALFORMED_SEQUENCES`] calls it to have the far end
//! grant a reference, so every fixture serves it.
//!
//! [`Fixture`] is this implementation of it, for sessions between two Rust
//! endpoints.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::session::rpc::{settle, Connection, Dispatch, Reference, Reply, Value};
use crate::session::Error;

const TARGET: &str = "conformance";
/// How long a wait lasts before the check fails as hung: only a hang
/// guard (#231).
const GUARD: Duration = Duration::from_secs(10);

/// The `conformance` target, for a Rust far end.
#[derive(Default)]
pub struct Fixture {
    held: Mutex<Option<Reference>>,
    kept: Mutex<std::collections::HashMap<u64, Reference>>,
    started: AtomicBool,
    aborted: Arc<AtomicBool>,
}

impl Dispatch for Fixture {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        if target != TARGET {
            return Err(Error::value(format!("no target {target}")));
        }
        let mut args = args.list()?.into_iter();
        let mut next = || args.next().unwrap_or(Value::Undefined);
        match method {
            "echo" => Ok(next()),
            "apply" => {
                let callback = next().reference()?;
                callback.call(Value::List(vec![next()]))
            }
            "later" => {
                let value = next();
                Ok(Value::future(async move {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    Ok(value)
                }))
            }
            "fail" => {
                let name: String = crate::session::decode(next().json()?)?;
                let message: String = crate::session::decode(next().json()?)?;
                Err(Error::Remote {
                    name,
                    message,
                    graph: None,
                })
            }
            "hold" => {
                *self.held.lock().unwrap() = Some(next().reference()?);
                Ok(Value::Undefined)
            }
            "fire" => {
                let held = self.held.lock().unwrap().clone();
                held.ok_or_else(|| Error::value("nothing held"))?
                    .call(Value::List(vec![next()]))
            }
            "drop" => {
                self.held.lock().unwrap().take();
                Ok(Value::Undefined)
            }
            "abortable" => {
                self.started.store(true, Ordering::SeqCst);
                // The signal argument aborts when the caller drops the call;
                // a Rust callee sees it as the future being dropped.
                let aborted = self.aborted.clone();
                struct Mark(Arc<AtomicBool>);
                impl Drop for Mark {
                    fn drop(&mut self) {
                        self.0.store(true, Ordering::SeqCst);
                    }
                }
                Ok(Value::future(async move {
                    let _mark = Mark(aborted);
                    std::future::pending::<()>().await;
                    Ok(Value::Undefined)
                }))
            }
            "keep" => {
                let slot: u64 = crate::session::decode(next().json()?)?;
                let kept = next().reference()?;
                self.kept.lock().unwrap().insert(slot, kept);
                Ok(Value::Undefined)
            }
            "free" => {
                let slot: u64 = crate::session::decode(next().json()?)?;
                let freed = self.kept.lock().unwrap().remove(&slot);
                // Dropping the import releases it.
                drop(freed);
                Ok(Value::Undefined)
            }
            "make" => Ok(Value::callback(Ok)),
            // The value one level deeper, returned or passed to a callback:
            // past the depth limit, this side's call or reply fails (#279).
            "wrap" => Ok(Value::List(vec![next()])),
            "applyWrapped" => {
                let callback = next().reference()?;
                callback.call(Value::List(vec![Value::List(vec![next()])]))
            }
            "tables" => {
                let (exports, imports) = peer.references();
                Ok(json!({ "exports": exports, "imports": imports }).into())
            }
            "started" => Ok(json!(self.started.load(Ordering::SeqCst)).into()),
            "aborted" => Ok(json!(self.aborted.load(Ordering::SeqCst)).into()),
            "reenter" => {
                let callback = next().reference()?;
                callback.call(Value::List(vec![]))
            }
            _ => Err(Error::value(format!("no method {method}"))),
        }
    }
}

/// Messages no implementation may take as a frame, with what is wrong with
/// each (Q6.2.3), in the endpoint format, sent by `main` once the session
/// is ready.
pub const MALFORMED: [(&str, &[u8]); 13] = [
    ("malformed JSON", br#"{"op":"invoke","id":"main:1""#),
    ("empty", b""),
    ("a leading BOM", b"\xef\xbb\xbf{\"op\":\"cancel\",\"id\":\"main:1\"}"),
    (
        "invalid UTF-8 inside a string",
        b"{\"op\":\"invoke\",\"id\":\"main:1\",\"path\":[],\"target\":\"conformance\",\"method\":\"echo\",\"args\":{\"type\":\"data\",\"value\":\"\xff\"}}",
    ),
    ("not an object", b"42"),
    ("unknown op", br#"{"op":"frobnicate","id":"main:1"}"#),
    (
        "wrong field type",
        br#"{"op":"invoke","id":"main:1","path":[],"target":7,"method":"echo","args":{"type":"undefined"}}"#,
    ),
    (
        "missing field",
        br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","args":{"type":"undefined"}}"#,
    ),
    (
        "unknown wire value",
        br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","method":"echo","args":{"type":"bogus"}}"#,
    ),
    (
        "call of an unknown reference",
        br#"{"op":"call","id":"main:1","path":[],"reference":99,"args":{"type":"undefined"}}"#,
    ),
    (
        "await of an unknown reference",
        br#"{"op":"await","id":"main:1","path":[],"reference":99}"#,
    ),
    (
        "release of an unknown reference",
        br#"{"op":"release","reference":99,"count":1}"#,
    ),
    (
        "reply to an unknown call",
        br#"{"op":"return","id":"far:99","value":{"type":"undefined"}}"#,
    ),
];

/// One frame of a [`MalformedSequence`].
#[derive(Clone, Copy)]
pub struct MalformedFrame {
    /// The frame as `main` writes it. `{hello}` is replaced by the greeting
    /// `main` builds at run time; `{reference}` by the reference id the far
    /// end granted in the reply to an earlier frame of the same scenario.
    pub frame: &'static [u8],
    /// Whether this is a valid call whose `return` is read before the next
    /// frame goes out: it grants the `{reference}` the frames after it name.
    pub replies: bool,
}

/// A case of the malformed matrix that one frame cannot state: a request
/// before the greeting, or a frame naming a reference the far end granted
/// first.
pub struct MalformedSequence {
    /// What is wrong with the frames, for a failure's message.
    pub case: &'static str,
    /// Whether `main` greets before the frames; a request before the
    /// greeting does not.
    pub greeting: bool,
    /// The frames, in order.
    pub frames: &'static [MalformedFrame],
}

/// Cases that end the session, which [`MALFORMED`] cannot carry: a request
/// before the greeting, a second greeting, a frame naming a reference the
/// far end granted first, and frames whose fields cannot be read (SESS-21,
/// SESS-22, SESS-32, SESS-33, SESS-41, SESS-47, SESS-48; §8.1's field-type
/// rule is SESS-6).
///
/// Each ends the session, as the frames of [`MALFORMED`] do: the far end
/// replies to the valid calls a case starts with (the `make` call, which
/// grants the reference the frames after it name) and to nothing else. What
/// ends the session is the channel's end; the far end's reason is not on the
/// wire, so this checks the scope (§8.1: the session, not one call) and not
/// the words, which differ between implementations (G6).
///
/// A `throw` whose `error` is missing `name`/`message` ends the session too;
/// one whose `error` is not an object has the same effect and is the case
/// here.
pub const MALFORMED_SEQUENCES: [MalformedSequence; 13] = [
    MalformedSequence {
        case: "request before the greeting",
        greeting: false,
        frames: &[MalformedFrame {
            frame: br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","method":"echo","args":{"type":"list","value":[]}}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "a second hello",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{hello}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "an invocation with no id",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{"op":"invoke","path":[],"target":"conformance","method":"echo","args":{"type":"list","value":[]}}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "get of an unknown reference",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{"op":"get","id":"main:1","path":[],"reference":99,"property":"x"}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "call of a released reference",
        greeting: true,
        frames: &[
            MalformedFrame {
                frame: br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","method":"make","args":{"type":"list","value":[]}}"#,
                replies: true,
            },
            MalformedFrame {
                frame: br#"{"op":"release","reference":{reference},"count":1}"#,
                replies: false,
            },
            MalformedFrame {
                frame: br#"{"op":"call","id":"main:2","path":[],"reference":{reference},"method":"call","args":{"type":"list","value":[]}}"#,
                replies: false,
            },
        ],
    },
    MalformedSequence {
        case: "release count 0",
        greeting: true,
        frames: &[
            MalformedFrame {
                frame: br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","method":"make","args":{"type":"list","value":[]}}"#,
                replies: true,
            },
            MalformedFrame {
                frame: br#"{"op":"release","reference":{reference},"count":0}"#,
                replies: false,
            },
        ],
    },
    MalformedSequence {
        case: "release count over the grant",
        greeting: true,
        frames: &[
            MalformedFrame {
                frame: br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","method":"make","args":{"type":"list","value":[]}}"#,
                replies: true,
            },
            MalformedFrame {
                frame: br#"{"op":"release","reference":{reference},"count":2}"#,
                replies: false,
            },
        ],
    },
    MalformedSequence {
        case: "cancel of a wrong field type",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{"op":"cancel","id":7}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "ack of a wrong field type",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{"op":"ack","id":7}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "throw of a wrong field type",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{"op":"throw","id":"far:1","error":7}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "throw for an unknown call",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{"op":"throw","id":"far:99","error":{"name":"Error","message":"unknown"}}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "path contains its own id",
        greeting: true,
        frames: &[MalformedFrame {
            frame: br#"{"op":"invoke","id":"main:1","path":["main:1"],"target":"conformance","method":"echo","args":{"type":"list","value":[]}}"#,
            replies: false,
        }],
    },
    MalformedSequence {
        case: "repeated invocation identity",
        greeting: true,
        frames: &[
            MalformedFrame {
                frame: br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","method":"later","args":{"type":"list","value":[{"type":"data","value":"soon"}]}}"#,
                replies: true,
            },
            MalformedFrame {
                frame: br#"{"op":"invoke","id":"main:1","path":[],"target":"conformance","method":"later","args":{"type":"list","value":[{"type":"data","value":"soon"}]}}"#,
                replies: false,
            },
        ],
    },
];

/// What the far end wrote, in the order it wrote it.
enum Incoming {
    Frame(Vec<u8>),
    Ended,
}

/// Check that the far end ends the session, and the channel with it, on
/// each of [`MALFORMED`] and [`MALFORMED_SEQUENCES`], replying nothing but
/// its greeting and the replies a scenario's valid calls ask for.
/// `open` makes a fresh channel to a far end that expects endpoint `main`;
/// this side speaks raw frames on it. Blocks: run it from a thread that may
/// block.
pub fn malformed(open: impl Fn() -> crate::channel::Channel) {
    let hello = serde_json::to_vec(&json!({
        "op": "hello",
        "version": crate::session::ENDPOINT_PROTOCOL,
        "endpoint": "main",
        "implementation": { "name": "conformance", "version": "0" },
    }))
    .unwrap();
    for (case, message) in MALFORMED {
        let frame = MalformedFrame {
            frame: message,
            replies: false,
        };
        check(open(), case, &hello, true, &[frame]);
    }
    for case in &MALFORMED_SEQUENCES {
        check(open(), case.case, &hello, case.greeting, case.frames);
    }
}

/// One scenario: send `frames` (after the greeting when `greeting`), read
/// the replies the valid calls ask for, and check that the session ends
/// without any other reply.
fn check(
    channel: crate::channel::Channel,
    case: &str,
    hello: &[u8],
    greeting: bool,
    frames: &[MalformedFrame],
) {
    let crate::channel::Channel {
        mut sender,
        mut receiver,
        closer,
        ..
    } = channel;
    let (incoming, inbox) = mpsc::channel();
    let reading = std::thread::spawn(move || loop {
        match receiver.recv() {
            Ok(Some(message)) => {
                if incoming.send(Incoming::Frame(message)).is_err() {
                    return;
                }
            }
            _ => {
                let _ = incoming.send(Incoming::Ended);
                return;
            }
        }
    });
    if greeting {
        sender.send(hello).expect("the far end takes the greeting");
    }
    let mut received = Vec::new();
    let mut reference = None;
    let mut waited = 0;
    let mut returned = 0;
    let mut ended = false;
    for frame in frames {
        assert!(
            reference.is_some() || find(frame.frame, b"{reference}").is_none(),
            "{case}: the far end granted no reference for this frame"
        );
        let bytes = fill(frame.frame, hello, reference);
        // The far end may already have gone: the reader then says when.
        if sender.send(&bytes).is_err() {
            ended = true;
            break;
        }
        if frame.replies {
            waited += 1;
            match read_reply(&inbox, &mut received, case) {
                Some(id) => {
                    reference = id;
                    returned += 1;
                }
                None => {
                    ended = true;
                    break;
                }
            }
        }
    }
    // Collect what the far end wrote until the session ends, or it hangs.
    let deadline = Instant::now() + GUARD;
    let mut hung = false;
    while !ended {
        match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Incoming::Frame(message)) => received.push(message),
            Ok(Incoming::Ended) => ended = true,
            Err(_) => {
                hung = true;
                break;
            }
        }
    }
    // Unblock the reader either way, so a failure is reported, not hung.
    closer.close(crate::channel::Close::Done);
    let _ = reading.join();
    assert!(!hung, "{case}: the far end kept the session open");
    assert_eq!(
        returned, waited,
        "{case}: the far end ended the session instead of replying to a valid call"
    );
    let ops: Vec<String> = received
        .iter()
        .map(|message| {
            serde_json::from_slice::<serde_json::Value>(message).unwrap()["op"].to_string()
        })
        .collect();
    // It may end before its own greeting went out, but never replies to the
    // frames a case is about; the only replies are the ones it owes the
    // valid calls before them.
    assert!(
        ops.iter().all(|op| op == "\"hello\"" || op == "\"return\""),
        "{case}: the far end replied {ops:?} before it ended"
    );
    assert_eq!(
        ops.iter().filter(|op| *op == "\"return\"").count(),
        waited,
        "{case}: the far end replied {ops:?} before it ended"
    );
}

/// Read frames until the far end's `return`: the reference its value grants,
/// when it grants one, for the frames after it. `None` means the session
/// ended first.
fn read_reply(
    inbox: &mpsc::Receiver<Incoming>,
    received: &mut Vec<Vec<u8>>,
    case: &str,
) -> Option<Option<u64>> {
    loop {
        match inbox.recv_timeout(GUARD) {
            Ok(Incoming::Frame(message)) => {
                let frame: serde_json::Value =
                    serde_json::from_slice(&message).expect("the far end writes JSON");
                let op = frame["op"].as_str().unwrap_or_default().to_owned();
                let reference = frame["value"]["value"]["id"].as_u64();
                received.push(message);
                if op == "throw" {
                    panic!("{case}: the far end failed a valid call: {frame}");
                }
                if op == "return" {
                    return Some(reference);
                }
            }
            Ok(Incoming::Ended) => return None,
            Err(_) => panic!("{case}: the far end did not reply to a valid call"),
        }
    }
}

/// Replace the `{hello}` and `{reference}` placeholders of a scenario: the
/// greeting, and the reference id an earlier reply granted.
fn fill(frame: &[u8], hello: &[u8], reference: Option<u64>) -> Vec<u8> {
    let mut out = frame.to_vec();
    if let Some(at) = find(&out, b"{hello}") {
        out.splice(at..at + b"{hello}".len(), hello.iter().copied());
    }
    if let Some(reference) = reference {
        let text = reference.to_string();
        if let Some(at) = find(&out, b"{reference}") {
            out.splice(at..at + b"{reference}".len(), text.bytes());
        }
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn data(value: serde_json::Value) -> Value {
    Value::Data(value)
}

/// Run the session checks against `far`, a ready session whose far end
/// serves `conformance`. Synchronous calls block: run it from a thread that
/// may block (`spawn_blocking`).
pub fn session(far: &Connection) {
    let call = |method: &str, args: Vec<Value>| far.invoke(TARGET, method, Value::List(args));
    let runtime = tokio::runtime::Handle::current();

    // Data crosses unchanged.
    let sample = json!({ "text": "a\nb ✓", "n": [1, 2.5, -3, 9007199254740991_u64], "none": null, "yes": true });
    let echoed = call("echo", vec![data(sample.clone())])
        .expect("echo")
        .json()
        .unwrap();
    assert_eq!(echoed, sample, "echo returns data unchanged");

    // A callback runs during the synchronous call that passed it.
    let doubled = call(
        "apply",
        vec![
            Value::callback(|args| {
                let [n]: [i64; 1] = crate::session::decode_value(args)?;
                Ok(json!(n * 2).into())
            }),
            data(json!(21)),
        ],
    )
    .expect("apply")
    .json()
    .unwrap();
    assert_eq!(doubled, json!(42), "a callback answers during the call");

    // Async results arrive.
    let later = runtime
        .block_on(async { settle(call("later", vec![data(json!("soon"))])?).await })
        .expect("later");
    assert_eq!(
        later.json().unwrap(),
        json!("soon"),
        "a future resolves to its value"
    );

    // Errors cross with their name.
    match call(
        "fail",
        vec![data(json!("TypeError")), data(json!("bad input"))],
    ) {
        Err(Error::Remote { name, message, .. }) => {
            assert_eq!(name, "TypeError", "an error keeps its name");
            assert!(
                message.contains("bad input"),
                "an error keeps its message: {message}"
            );
        }
        other => panic!("fail must throw, got {other:?}"),
    }

    // A reference kept by the far end stays callable until let go.
    call(
        "hold",
        vec![Value::callback(|args| {
            let [n]: [i64; 1] = crate::session::decode_value(args)?;
            Ok(json!(n + 1).into())
        })],
    )
    .expect("hold");
    assert_eq!(
        call("fire", vec![data(json!(1))])
            .expect("fire")
            .json()
            .unwrap(),
        json!(2)
    );
    assert_eq!(
        call("fire", vec![data(json!(41))])
            .expect("fire")
            .json()
            .unwrap(),
        json!(42)
    );
    call("drop", vec![]).expect("drop");
    assert!(
        call("fire", vec![data(json!(0))]).is_err(),
        "a dropped reference is gone"
    );

    // Dropping an async call aborts the far end's signal.
    let is_true = |method: &str| {
        call(method, vec![])
            .ok()
            .and_then(|value| value.json().ok())
            == Some(json!(true))
    };
    runtime.block_on(async {
        let pending = far.invoke_async(TARGET, "abortable", Value::List(vec![Value::Signal]));
        let pending = async {
            let reply = pending.await?;
            settle(reply).await
        };
        let mut pending = std::pin::pin!(pending);
        // Dropped once the far end runs it: a cancel ahead of the call would
        // find nothing to abort (#231).
        let deadline = std::time::Instant::now() + GUARD;
        loop {
            tokio::select! {
                ended = &mut pending => panic!("abortable only ends when cancelled: {ended:?}"),
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            if is_true("started") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "abortable never started"
            );
        }
    });
    let deadline = std::time::Instant::now() + GUARD;
    while !is_true("aborted") {
        assert!(
            std::time::Instant::now() < deadline,
            "cancelling a call aborts its signal"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // The callee resolves the cancel: its `ack` was read before this call's
    // reply, so the caller's cancelled table is empty again (protocol 5,
    // #1099).
    assert_eq!(
        far.cancelled(),
        0,
        "the callee's acknowledgement drains the cancelled table"
    );

    // While the far end waits on a synchronous callback, the callback may
    // call the far end again: the call belongs to the chain it waits for.
    let reentrant = far.clone();
    let nested = call(
        "reenter",
        vec![Value::callback(move |_| {
            reentrant.invoke(TARGET, "echo", Value::List(vec![data(json!("nested"))]))
        })],
    )
    .expect("reenter")
    .json()
    .unwrap();
    assert_eq!(
        nested,
        json!("nested"),
        "a nested call reaches the waiting far end"
    );
}

#[cfg(test)]
mod tests {
    use super::fill;

    /// The placeholders of a scenario are what [`fill`] replaces; a silent
    /// miss would make a case end the session for another reason.
    #[test]
    fn a_frame_gets_its_greeting_and_its_reference() {
        assert_eq!(
            fill(br#"{hello}"#, br#"{"op":"hello"}"#, None).as_slice(),
            br#"{"op":"hello"}"#
        );
        assert_eq!(
            fill(
                br#"{"op":"release","reference":{reference},"count":1}"#,
                b"",
                Some(7)
            )
            .as_slice(),
            br#"{"op":"release","reference":7,"count":1}"#
        );
        assert_eq!(
            fill(br#"{"op":"cancel","id":7}"#, b"", Some(7)).as_slice(),
            br#"{"op":"cancel","id":7}"#
        );
    }
}
