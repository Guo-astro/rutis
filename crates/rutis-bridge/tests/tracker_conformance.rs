//! Wire-level conformance for the session rules the tracking table still
//! marks "部分验证" (`docs/formal-trace/rules/trace-contract.md` §3 and
//! `docs/formal-trace/rules/guarantees-bridge.md`): a session between this
//! side and a far end scripted frame by frame on a memory channel, so what
//! is checked is the frames themselves, not an internal helper.
//!
//! Rules whose gap names an implementation other than Rust (Node, Python,
//! Go, Bun, Swift, Kotlin) stay open: these checks close the Rust column
//! only.
//!
//! rule: TRC-FRAME-1
//! rule: TRC-FRAME-2
//! rule: TRC-FRAME-3
//! rule: TRC-MALFORMED-1
//! rule: TRC-MALFORMED-2
//! rule: TRC-MALFORMED-3
//! rule: TRC-CANCEL-1
//! rule: TRC-CANCEL-2
//! rule: TRC-CANCEL-3
//! rule: TRC-LIMIT-2
//! rule: TRC-ERR-1
//! rule: TRC-ERR-2
//! guarantee: bridge/unknown-fields

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use rutis_bridge::channel::{Channel, ChannelError, PeerId, Sender};
use rutis_bridge::session::{
    Connection, Dispatch, Endpoint, Error, Reply, Value, ENDPOINT_PROTOCOL,
};
use rutis_test_support::{until, within};
use serde_json::{json, Value as Json};

fn id(name: &str) -> PeerId {
    PeerId::new(name).unwrap()
}

/// A far end that serves nothing.
struct Nothing;
impl Dispatch for Nothing {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        Err(Error::value("main serves nothing"))
    }
}

fn session(dispatch: Arc<dyn Dispatch>) -> (Connection, Channel) {
    let (ours, theirs) = rutis_bridge::memory::pair();
    let peer = Connection::open(ours, dispatch, Endpoint::rust(id("main"))).unwrap();
    (peer, theirs)
}

fn read(channel: &mut Channel) -> Json {
    match channel.receiver.recv().unwrap() {
        Some(message) => serde_json::from_slice(&message).unwrap(),
        None => panic!("the far end read the channel's end"),
    }
}

fn send(channel: &mut Channel, frame: Json) {
    channel
        .sender
        .send(&serde_json::to_vec(&frame).unwrap())
        .unwrap();
}

/// Read one frame off the test's thread, which may block.
async fn read_off(channel: Channel) -> (Json, Channel) {
    within(
        "a frame",
        tokio::task::spawn_blocking(move || {
            let mut channel = channel;
            let frame = read(&mut channel);
            (frame, channel)
        }),
    )
    .await
    .unwrap()
}

/// Answer this side's greeting as `endpoint`, so the session is ready.
fn greet(channel: &mut Channel, endpoint: &str) {
    let hello = read(channel);
    assert_eq!(hello["op"], json!("hello"), "{hello}");
    assert_eq!(hello["version"], json!(ENDPOINT_PROTOCOL), "{hello}");
    assert_eq!(hello["endpoint"], json!("main"), "{hello}");
    send(
        channel,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": endpoint }),
    );
}

/// The `id` of a frame the far end reads.
fn frame_id(frame: &Json) -> String {
    frame["id"].as_str().expect("an id").to_owned()
}

/// The error a far end reads in a `throw`: `(name, message, code, data)`.
fn thrown(frame: &Json) -> (String, String, Option<String>, Option<Json>) {
    assert_eq!(frame["op"], json!("throw"), "{frame}");
    let error = &frame["error"];
    let root = &error["graph"]["nodes"][0];
    // A field of the graph that is a value: `{type, value}` on the wire.
    let data = root.get("data").map(|data| match data.get("value") {
        Some(value) => value.clone(),
        None => data.clone(),
    });
    (
        error["name"].as_str().expect("a name").to_owned(),
        error["message"].as_str().expect("a message").to_owned(),
        root["code"].as_str().map(str::to_owned),
        data,
    )
}

/// A call from the far end into this side, as the far end writes it.
fn invoke(id: &str, target: &str, method: &str) -> Json {
    json!({
        "op": "invoke",
        "id": id,
        "path": [],
        "target": target,
        "method": method,
        "args": { "type": "data", "value": [] },
    })
}

/// A sender that advertises the channel's limit, as a transport with one
/// does: the session checks what it sends against it.
struct Limited {
    inner: Box<dyn Sender>,
    limit: usize,
}

impl Sender for Limited {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        self.inner.send(message)
    }

    fn max_message(&self) -> Option<usize> {
        Some(self.limit)
    }
}

const LIMIT: usize = 1024;

/// The near end of a channel that takes at most [`LIMIT`] bytes a message.
fn limited(dispatch: Arc<dyn Dispatch>) -> (Connection, Channel) {
    let (ours, theirs) = rutis_bridge::memory::pair();
    let ours = Channel {
        sender: Box::new(Limited {
            inner: ours.sender,
            limit: LIMIT,
        }),
        receiver: ours.receiver,
        closer: ours.closer,
    };
    let peer = Connection::open(ours, dispatch, Endpoint::rust(id("main"))).unwrap();
    (peer, theirs)
}

/// A call is an `invoke` frame — `id`, `path`, `target`, `method`, `args`
/// and `sync` — and its answer is a `return` or a `throw` with the same
/// `id`.
/// rule: TRC-FRAME-1
#[tokio::test(flavor = "multi_thread")]
async fn a_call_is_an_invoke_and_its_answer_carries_the_same_id() {
    let (peer, far) = session(Arc::new(Nothing));
    let mut far = far;
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    let answering = tokio::task::spawn_blocking(move || {
        let mut far = far;
        let call = read(&mut far);
        let mut fields: Vec<&str> = call
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            ["args", "id", "method", "op", "path", "sync", "target"],
            "{call}"
        );
        assert_eq!(call["op"], json!("invoke"), "{call}");
        assert_eq!(call["id"], json!("main:1"), "{call}");
        assert_eq!(call["path"], json!([]), "{call}");
        assert_eq!(call["target"], json!("conformance"), "{call}");
        assert_eq!(call["method"], json!("echo"), "{call}");
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [{ "type": "data", "value": 1 }] }),
            "{call}"
        );
        assert_eq!(call["sync"], json!(true), "{call}");
        send(
            &mut far,
            json!({
                "op": "return",
                "id": call["id"].clone(),
                "value": { "type": "data", "value": "answered" },
            }),
        );
        // A failing call: a `throw` with the same id.
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        send(
            &mut far,
            json!({
                "op": "throw",
                "id": call["id"].clone(),
                "error": { "name": "TypeError", "message": "no" },
            }),
        );
        far
    });
    let value = peer
        .invoke(
            "conformance",
            "echo",
            Value::List(vec![Value::Data(json!(1))]),
        )
        .unwrap();
    assert_eq!(value.json().unwrap(), json!("answered"));
    let failed = peer
        .invoke("conformance", "echo", Value::Data(json!([])))
        .expect_err("the far end threw");
    match failed {
        Error::Remote { name, message, .. } => {
            assert_eq!((name.as_str(), message.as_str()), ("TypeError", "no"));
        }
        other => panic!("a `throw` is the far end's error: {other:?}"),
    }
    within("the far end", answering).await.unwrap();
}

/// Frames are read one at a time, but a call's body does not hold the
/// channel: a second call is answered while the first still runs.
/// rule: TRC-FRAME-1
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_call_does_not_hold_the_channel() {
    struct Held {
        started: Mutex<mpsc::Sender<()>>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Dispatch for Held {
        fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
            match method {
                "hold" => {
                    self.started.lock().unwrap().send(()).unwrap();
                    self.release.lock().unwrap().recv().unwrap();
                    Ok(Value::Data(json!("first")))
                }
                _ => Ok(args),
            }
        }
    }
    let (started, first_started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (peer, mut far) = session(Arc::new(Held {
        started: Mutex::new(started),
        release: Mutex::new(released),
    }));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    send(&mut far, invoke("node:1", "conformance", "hold"));
    send(&mut far, invoke("node:2", "conformance", "echo"));
    within(
        "the first call",
        tokio::task::spawn_blocking(move || first_started.recv().unwrap()),
    )
    .await
    .unwrap();
    // The second is answered while the first still runs.
    let (second, far) = read_off(far).await;
    assert_eq!(frame_id(&second), "node:2", "{second}");
    assert_eq!(second["op"], json!("return"), "{second}");
    release.send(()).unwrap();
    let (first, _far) = read_off(far).await;
    assert_eq!(frame_id(&first), "node:1", "{first}");
    assert_eq!(first["value"]["value"], json!("first"));
}

/// A `return` for a call this side never issued is a framing failure: the
/// session closes, and a call made after it fails.
/// rule: TRC-FRAME-2
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_for_a_call_this_side_never_issued_ends_the_session() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    send(
        &mut far,
        json!({ "op": "return", "id": "node:7", "value": { "type": "undefined" } }),
    );
    within("the session's end", peer.closed()).await;
    let reason = peer.close_reason().expect("a reason");
    assert!(
        matches!(reason, Error::Transport(_)),
        "the session ends as a transport failure: {reason}"
    );
    assert!(
        peer.invoke("conformance", "echo", Value::Data(json!([])))
            .is_err(),
        "a call after the end fails"
    );
}

/// A `cancel` for no call of this side's is ignored (it may have overtaken
/// its own reply), and the session goes on.
/// rule: TRC-FRAME-3
/// rule: TRC-MALFORMED-1
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_for_an_unknown_call_is_ignored_and_calls_go_on() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    send(&mut far, json!({ "op": "cancel", "id": "node:99" }));
    let answering = tokio::task::spawn_blocking(move || {
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        send(
            &mut far,
            json!({
                "op": "return",
                "id": frame_id(&call),
                "value": { "type": "data", "value": "after" },
            }),
        );
        far
    });
    let reply = peer.invoke("conformance", "echo", Value::Data(json!([])));
    assert_eq!(reply.unwrap().json().unwrap(), json!("after"));
    within("the far end", answering).await.unwrap();
}

/// Malformed or dangling frames close the session, with a transport error;
/// every case gets a fresh channel, and a call after it fails.
/// rule: TRC-MALFORMED-2
#[tokio::test(flavor = "multi_thread")]
async fn malformed_frames_end_the_session() {
    let cases: &[(&str, &[u8])] = &[
        ("not JSON", b"{not json"),
        ("nothing at all", b""),
        ("a byte order mark", b"\xef\xbb\xbf{}"),
        ("invalid UTF-8", &[b'{', 0xff, b'}']),
        ("not an object", b"[]"),
        ("an unknown op", br#"{"op":"nonsense","id":"node:1"}"#),
        ("a field of the wrong type", br#"{"op":"cancel","id":5}"#),
        (
            "a call frame missing its target",
            br#"{"op":"invoke","id":"node:1","path":[],"method":"m","args":{"type":"undefined"}}"#,
        ),
        (
            "a call on an unknown reference",
            br#"{"op":"call","id":"node:1","path":[],"reference":99,"method":"m","args":{"type":"undefined"}}"#,
        ),
    ];
    for (case, message) in cases {
        let (peer, mut far) = session(Arc::new(Nothing));
        greet(&mut far, "node");
        peer.ready().await.unwrap();
        far.sender.send(message).unwrap();
        within(case, peer.closed()).await;
        let reason = peer.close_reason().expect(case);
        assert!(
            matches!(reason, Error::Transport(_)),
            "{case}: the session ends as a transport failure, not {reason}"
        );
        assert!(
            peer.invoke("conformance", "echo", Value::Data(json!([])))
                .is_err(),
            "{case}: a call after the end fails"
        );
    }
}

/// An unknown field in a call frame fails only that call, with `bad-frame`
/// and `{op, fields}`; in a reply, only the call it answers; `release` and
/// `cancel` ignore extras; in a greeting, the handshake fails.
/// guarantee: bridge/unknown-fields
/// rule: TRC-MALFORMED-3
#[tokio::test(flavor = "multi_thread")]
async fn unknown_fields_fail_only_their_call_and_the_session_goes_on() {
    struct Echo;
    impl Dispatch for Echo {
        fn invoke(&self, _: &Connection, _: &str, _: &str, args: Value) -> Reply {
            Ok(args)
        }
    }
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node");
    peer.ready().await.unwrap();

    // A call frame with an unknown top-level field: a `throw` for it alone.
    let mut with_field = invoke("node:1", "conformance", "echo");
    with_field["trace"] = json!("x");
    send(&mut far, with_field);
    let (throw, far) = read_off(far).await;
    assert_eq!(frame_id(&throw), "node:1");
    let (name, message, code, data) = thrown(&throw);
    assert_eq!(name, "BindingError");
    assert_eq!(message, "unknown field \"trace\" in invoke");
    assert_eq!(code.as_deref(), Some("bad-frame"));
    assert_eq!(data, Some(json!({ "op": "invoke", "fields": ["trace"] })));

    // `cancel` has no call to fail: its extras are ignored, after a failed
    // call as before one, and nothing ends.
    let mut far = far;
    send(
        &mut far,
        json!({ "op": "cancel", "id": "node:1", "extra": 1 }),
    );
    send(&mut far, invoke("node:2", "conformance", "echo"));
    let (reply, far) = read_off(far).await;
    assert_eq!(frame_id(&reply), "node:2");
    assert_eq!(reply["op"], json!("return"), "{reply}");

    // `release` is applied too: a reference this side exported comes back
    // with an extra field, which is ignored, and the release itself counts.
    let answering = tokio::task::spawn_blocking(move || {
        let mut far = far;
        let call = read(&mut far);
        let reference = call["args"]["value"][0]["value"]["id"]
            .as_u64()
            .expect("the exported reference");
        send(
            &mut far,
            json!({ "op": "release", "reference": reference, "count": 1, "extra": 1 }),
        );
        send(
            &mut far,
            json!({
                "op": "return",
                "id": frame_id(&call),
                "value": { "type": "data", "value": "released" },
            }),
        );
        far
    });
    let callback = Value::callback(Ok);
    let value = peer
        .invoke("conformance", "hold", Value::List(vec![callback]))
        .expect("the call with a reference");
    assert_eq!(value.json().unwrap(), json!("released"));
    assert!(peer.close_reason().is_none(), "the session goes on");
    let far = within("the far end", answering).await.unwrap();

    // A reply carrying an unknown field fails only the call waiting for it.
    let answering = tokio::task::spawn_blocking(move || {
        let mut far = far;
        let call = read(&mut far);
        send(
            &mut far,
            json!({
                "op": "return",
                "id": frame_id(&call),
                "value": { "type": "data", "value": [] },
                "extra": 1,
            }),
        );
        far
    });
    let failed = peer
        .invoke("conformance", "echo", Value::Data(json!([])))
        .expect_err("the reply's unknown field fails the call");
    assert_eq!(failed.code(), Some("bad-frame"), "{failed}");
    assert_eq!(
        failed.data(),
        Some(json!({ "op": "return", "fields": ["extra"] }))
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
    within("the far end", answering).await.unwrap();

    // A greeting with an unknown field fails the handshake.
    let (other, mut theirs) = session(Arc::new(Nothing));
    let hello = read(&mut theirs);
    assert_eq!(hello["op"], json!("hello"));
    send(
        &mut theirs,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "node", "extra": 1 }),
    );
    let error = other.ready().await.expect_err("the greeting is refused");
    assert!(
        error
            .to_string()
            .contains("unknown field \"extra\" in hello"),
        "{error}"
    );
    within("the refused session", other.closed()).await;
    // Nothing more comes this side's way: only the channel's end.
    assert!(
        matches!(theirs.receiver.recv(), Ok(None)),
        "the channel ends"
    );
}

/// A caller that gives up sends `cancel` for its call, which the callee
/// reads; the session goes on.
/// rule: TRC-CANCEL-1
#[tokio::test(flavor = "multi_thread")]
async fn giving_up_on_a_call_sends_a_cancel_the_callee_reads() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    let (seen, invoked) = mpsc::channel();
    let far_end = std::thread::spawn(move || {
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        let id = frame_id(&call);
        seen.send(()).unwrap();
        // The caller gave up: a `cancel` for that call, before any reply.
        assert_eq!(read(&mut far), json!({ "op": "cancel", "id": id }));
        // A call after it works.
        let next = read(&mut far);
        assert_eq!(next["op"], json!("invoke"), "{next}");
        send(
            &mut far,
            json!({
                "op": "return",
                "id": frame_id(&next),
                "value": { "type": "data", "value": "still here" },
            }),
        );
    });
    let mut call = Box::pin(peer.invoke_async("conformance", "slow", Value::Data(json!([]))));
    tokio::select! {
        biased;
        answer = &mut call => panic!("the call only ends when given up: {answer:?}"),
        received = tokio::task::spawn_blocking(move || invoked.recv().unwrap()) => {
            received.unwrap();
        }
    }
    drop(call);
    let reply = peer.invoke_async("conformance", "echo", Value::Data(json!([])));
    assert_eq!(
        within("a call after the cancel", reply)
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("still here")
    );
    within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
}

/// The far end cancelling its wait for a result aborts the method behind
/// it (a cancellable future), and the session goes on.
/// rule: TRC-CANCEL-2
#[tokio::test(flavor = "multi_thread")]
async fn the_far_end_cancelling_its_await_drops_the_method() {
    struct Flag(Arc<AtomicBool>);
    impl Drop for Flag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    struct Slow {
        polled: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }
    impl Dispatch for Slow {
        fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
            match method {
                "slow" => {
                    let (polled, dropped) = (self.polled.clone(), self.dropped.clone());
                    Ok(Value::cancellable_future(async move {
                        polled.store(true, Ordering::SeqCst);
                        let _flag = Flag(dropped);
                        std::future::pending::<()>().await;
                        Ok(Value::Undefined)
                    }))
                }
                _ => Ok(args),
            }
        }
    }
    let polled = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let (peer, mut far) = session(Arc::new(Slow {
        polled: polled.clone(),
        dropped: dropped.clone(),
    }));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    // The far end calls a method, gets a future back, awaits it, then
    // cancels its wait.
    send(&mut far, invoke("node:1", "conformance", "slow"));
    let (reply, far) = read_off(far).await;
    assert_eq!(reply["op"], json!("return"), "{reply}");
    assert_eq!(reply["value"]["type"], json!("reference"), "{reply}");
    let reference = reply["value"]["value"]["id"]
        .as_u64()
        .expect("a reference id");
    let mut far = far;
    send(
        &mut far,
        json!({
            "op": "await",
            "id": "node:2",
            "path": [],
            "reference": reference,
            "sync": false,
        }),
    );
    until(
        || polled.load(Ordering::SeqCst),
        "the awaited method polled",
    )
    .await;
    send(&mut far, json!({ "op": "cancel", "id": "node:2" }));
    until(
        || dropped.load(Ordering::SeqCst),
        "the cancelled method dropped",
    )
    .await;
    assert_eq!(
        read(&mut far),
        json!({ "op": "ack", "id": "node:2" }),
        "the cancelled await is acknowledged"
    );
    // A call after it works.
    let answering = tokio::task::spawn_blocking(move || {
        let mut far = far;
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        send(
            &mut far,
            json!({
                "op": "return",
                "id": frame_id(&call),
                "value": { "type": "data", "value": "after" },
            }),
        );
        far
    });
    let value = peer
        .invoke("conformance", "echo", Value::Data(json!([])))
        .expect("a call after the cancel");
    assert_eq!(value.json().unwrap(), json!("after"));
    let far = within("the far end", answering).await.unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
    drop(far);
}

/// A `cancel` that arrives after the reply to its call is ignored: the call
/// keeps its value and the session goes on.
/// rule: TRC-CANCEL-3
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_after_the_reply_is_ignored() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    let answering = tokio::task::spawn_blocking(move || {
        let mut far = far;
        let call = read(&mut far);
        let id = frame_id(&call);
        send(
            &mut far,
            json!({ "op": "return", "id": id, "value": { "type": "data", "value": 7 } }),
        );
        send(&mut far, json!({ "op": "cancel", "id": id }));
        far
    });
    let value = peer
        .invoke("conformance", "echo", Value::Data(json!([])))
        .unwrap();
    assert_eq!(value.json().unwrap(), json!(7));
    let far = within("the far end", answering).await.unwrap();
    // The late cancel closed nothing: a later call from the far end is
    // answered.
    let mut far = far;
    send(&mut far, invoke("node:5", "conformance", "echo"));
    let (reply, _far) = read_off(far).await;
    assert_eq!(frame_id(&reply), "node:5");
    assert!(peer.close_reason().is_none());
}

/// An argument that does not fit the channel's limit fails only its call,
/// saying which argument and the limit; the next call goes through.
/// rule: TRC-LIMIT-2
/// rule: TRC-ERR-2
#[tokio::test(flavor = "multi_thread")]
async fn an_argument_over_the_channel_limit_fails_only_its_call() {
    let (peer, mut far) = limited(Arc::new(Nothing));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    let big = "x".repeat(4096);
    let error = peer
        .invoke(
            "conformance",
            "take",
            Value::List(vec![Value::Data(json!(big))]),
        )
        .expect_err("the argument does not fit");
    assert_eq!(error.code(), Some("bad-value"), "{error}");
    let message = error.to_string();
    assert!(
        message.starts_with("conformance.take: argument 1 cannot be sent: the message would be "),
        "{message}"
    );
    assert!(
        message.ends_with(&format!(", over the limit of {LIMIT} bytes")),
        "{message}"
    );
    let data = error.data().expect("data");
    assert_eq!(
        (
            data["part"].clone(),
            data["argument"].clone(),
            data["limit"].clone()
        ),
        (json!("argument"), json!(1), json!(LIMIT))
    );
    // Nothing of the refused call reached the far end: the next frame it
    // reads is the next call.
    let answering = tokio::task::spawn_blocking(move || {
        let mut far = far;
        let call = read(&mut far);
        assert_eq!(call["method"], json!("small"), "{call}");
        send(
            &mut far,
            json!({
                "op": "return",
                "id": frame_id(&call),
                "value": { "type": "data", "value": "fits" },
            }),
        );
        far
    });
    let value = peer
        .invoke("conformance", "small", Value::Data(json!([])))
        .expect("a small call fits");
    assert_eq!(value.json().unwrap(), json!("fits"));
    let far = within("the far end", answering).await.unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
    drop(far);
}

/// A result that does not fit the channel's limit is answered with a
/// `throw` naming the result: that call fails, the session goes on.
/// rule: TRC-ERR-2
#[tokio::test(flavor = "multi_thread")]
async fn a_result_over_the_channel_limit_is_answered_with_a_throw() {
    struct Big;
    impl Dispatch for Big {
        fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
            match method {
                "big" => Ok(Value::Data(json!("y".repeat(4096)))),
                _ => Ok(args),
            }
        }
    }
    let (peer, mut far) = limited(Arc::new(Big));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    send(&mut far, invoke("node:1", "conformance", "big"));
    let (throw, far) = read_off(far).await;
    assert_eq!(frame_id(&throw), "node:1");
    let (_, message, code, data) = thrown(&throw);
    assert!(
        message.starts_with("conformance.big: the result cannot be sent: the message would be "),
        "{message}"
    );
    assert_eq!(code.as_deref(), Some("bad-value"));
    assert_eq!(
        data,
        Some(json!({ "part": "result", "limit": LIMIT, "reason": "limit" }))
    );
    // The session goes on: the next call is answered.
    let mut far = far;
    send(&mut far, invoke("node:2", "conformance", "small"));
    let (reply, _far) = read_off(far).await;
    assert_eq!(reply["op"], json!("return"), "{reply}");
    assert!(peer.close_reason().is_none());
}

/// `no-method` crosses with its code, its data and the exact message.
/// rule: TRC-ERR-1
#[tokio::test(flavor = "multi_thread")]
async fn a_no_method_error_crosses_with_its_code() {
    struct Weather;
    impl Dispatch for Weather {
        fn invoke(&self, _: &Connection, target: &str, method: &str, _: Value) -> Reply {
            assert_eq!(target, "weather");
            Err(Error::no_method(
                "weather",
                method,
                vec!["tomorrow", "today"],
            ))
        }
    }
    let (peer, mut far) = session(Arc::new(Weather));
    greet(&mut far, "node");
    peer.ready().await.unwrap();
    send(&mut far, invoke("node:1", "weather", "tomorow"));
    let (throw, far) = read_off(far).await;
    let (name, message, code, data) = thrown(&throw);
    assert_eq!(name, "BindingError");
    assert_eq!(
        message,
        "weather has no method tomorow (methods: today, tomorrow)"
    );
    assert_eq!(code.as_deref(), Some("no-method"));
    assert_eq!(
        data,
        Some(
            json!({ "service": "weather", "method": "tomorow", "methods": ["today", "tomorrow"] })
        )
    );
    // The session goes on.
    let mut far = far;
    send(&mut far, invoke("node:2", "weather", "today"));
    let (second, far) = read_off(far).await;
    assert_eq!(second["op"], json!("throw"), "{second}");
    assert!(peer.close_reason().is_none(), "the session goes on");
    drop(far);
}
