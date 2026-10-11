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
//!
//! [`Fixture`] is this implementation of it, for sessions between two Rust
//! endpoints.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    started: AtomicBool,
    aborted: Arc<AtomicBool>,
}

impl Dispatch for Fixture {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        if target != TARGET {
            return Err(Error::Value(format!("no target {target}")));
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
                held.ok_or_else(|| Error::Value("nothing held".into()))?
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
            "started" => Ok(json!(self.started.load(Ordering::SeqCst)).into()),
            "aborted" => Ok(json!(self.aborted.load(Ordering::SeqCst)).into()),
            "reenter" => {
                let callback = next().reference()?;
                let _ = peer;
                callback.call(Value::List(vec![]))
            }
            _ => Err(Error::Value(format!("no method {method}"))),
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

/// Check that the far end ends the session, and the channel with it, on
/// each of [`MALFORMED`], replying nothing but its greeting. `open` makes a fresh channel to
/// a far end that expects endpoint `main`; this side speaks raw frames on
/// it. Blocks: run it from a thread that may block.
pub fn malformed(open: impl Fn() -> crate::channel::Channel) {
    let hello = serde_json::to_vec(&json!({
        "op": "hello",
        "version": crate::session::ENDPOINT_PROTOCOL,
        "endpoint": "main",
        "implementation": { "name": "conformance", "version": "0" },
        "capabilities": [],
    }))
    .unwrap();
    for (case, message) in MALFORMED {
        let crate::channel::Channel {
            mut sender,
            mut receiver,
            closer,
            ..
        } = open();
        let (done, ended) = std::sync::mpsc::channel();
        let reading = std::thread::spawn(move || {
            let mut received = Vec::new();
            let end = loop {
                match receiver.recv() {
                    Ok(Some(message)) => received.push(message),
                    end => break end,
                }
            };
            let _ = done.send(());
            (received, end)
        });
        sender.send(&hello).expect("the far end takes the greeting");
        // The far end may already have gone after the greeting failed; then
        // the reader says why.
        let _ = sender.send(message);
        let ended = ended.recv_timeout(Duration::from_secs(10));
        // Unblock the reader either way, so a failure is reported, not hung.
        closer.close("checked");
        let (received, end) = reading.join().unwrap();
        assert!(ended.is_ok(), "{case}: the far end kept the session open");
        let ops: Vec<String> = received
            .iter()
            .map(|message| {
                serde_json::from_slice::<serde_json::Value>(message).unwrap()["op"].to_string()
            })
            .collect();
        // It may end before its own greeting went out, but never replies.
        assert!(
            ops.iter().all(|op| op == "\"hello\""),
            "{case}: the far end replied {ops:?} before it ended ({end:?})"
        );
    }
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
