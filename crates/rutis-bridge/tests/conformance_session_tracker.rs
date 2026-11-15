//! Wire-level conformance for the session and value rules the tracking
//! table still marks partial or unverified (#639): a far end scripted frame
//! by frame on a memory channel, so what is checked is the frames and the
//! errors themselves, not an internal helper. The runtime-contract rules
//! (`rows.*`, `hosts.*`) use a fake runtime mounted as a row through
//! `common::runtime`, which answers what the test scripts it to.
//!
//! Rules whose gap names an implementation other than Rust (Node, Python,
//! Go, Bun, Swift, Kotlin) stay open: these checks close the Rust column
//! only. `M1-13-3` is reproduced here as far as the wire shows it; what a
//! runtime that does not take object references does with one is tested on
//! that runtime's side.
//!
//! rule: SES-1-1
//! rule: SES-1-2
//! rule: SES-2-2
//! rule: SES-2-7
//! rule: SES-6.1-1
//! rule: SES-6.1-3
//! rule: SES-6.1-4
//! rule: SES-6.1-5
//! rule: SES-6.1-6
//! rule: SES-6.1-7
//! rule: SES-6.1-8
//! rule: SES-6.1-9
//! rule: SES-6.1-10
//! rule: SES-6.1-11
//! rule: SES-6.1-12
//! rule: SES-6.1-13
//! rule: SES-6.1-14
//! rule: SES-6.1-15
//! rule: SES-6.1-16
//! rule: SES-6.1-17
//! rule: SES-6.1-20
//! rule: SES-6.1-22
//! rule: SES-6.1-24
//! rule: SES-6.1-26
//! rule: SES-6.1-27
//! rule: SES-7.1-6
//! rule: SES-7.2-5
//! rule: SES-12-8
//! rule: CHN-0-1
//! rule: CHN-2-3
//! rule: CHN-4-4
//! rule: CHN-4-6
//! rule: CHN-4-8
//! rule: CHN-4-9
//! rule: CHN-4-10
//! rule: CHN-4-11
//! rule: TAG-2-1
//! rule: TAG-5-1
//! rule: M1-4-2
//! rule: M1-5.1-1
//! rule: M1-7.1-1
//! rule: M1-13-3
//! rule: SYN-5-2
//! rule: SYN-5-4
//! rule: REM-4.2-2
//! rule: DIA-4-1
//! rule: INS-4.2-3
//! rule: CHN-2-5
//! rule: VAL-2-8
//! rule: MNT-2-8n
//! rule: MNT-3-6
//! rule: MNT-5-4
//! rule: bridge/error-code-and-data a
//! rule: bridge/exported-services b
//! rule: bridge/exported-services c
//! rule: MNT-4.3-1
//! rule: MNT-2-8l
//! rule: MNT-2-8m
//! rule: MNT-2-12
//! rule: MNT-4.1-1
//! rule: INS-4.2-10
//! rule: REM-1-1
//! rule: REM-3-1
//! rule: REM-4.2-7
//! rule: REM-5.1-2
//! rule: REM-5.1-6
//! rule: REM-5.2-1
//! rule: REM-6.3-2
//! rule: REM-8-3
//! rule: REM-8-6
//! rule: M1-4-1
//! rule: REQ-3-3
//! rule: REQ-5-0
//! rule: SYN-2-13
//! rule: M2-2-1
//! rule: M1-4.1-1
//! rule: M1-5.1-3
//! rule: M1-14-5
//! rule: INS-4.2-9
//! rule: INS-5-5
//! rule: CHN-0-2
//! rule: CHN-3-4
//! rule: CHN-2.2-1
//! rule: M2-2-5
//! rule: REM-4.1-4
//! rule: REM-4.2-10
//! rule: REM-5.2-6
//! rule: REM-6.2-6
//! rule: REM-10-1
//! rule: DIA-3-1
//! rule: DIA-4-8
//! rule: APP-1-2
//! rule: APP-1-5
//! rule: APP-1-6
//! rule: NOD-3-8
//! rule: CHN-10-6
//! rule: CHN-10-7
//! rule: DIA-2-5
//! rule: DIA-6-3
//! rule: DIA-6-5
//! rule: DIA-6-6
//! rule: DIA-6-7
//! rule: REM-2-2
//! rule: CHN-1-5
//! rule: REM-1-3
//! rule: M1-14-1
//! rule: M1-14-9
//! rule: M1-5.4-1
//! rule: INS-4.2-4
//! rule: VAL-1-V2
//! rule: SYN-2-10
//! rule: M1-11.2-1
//! rule: M1-11.2-3
//! rule: APP-3-1
//! rule: NOD-3-6
//! guarantee: bridge/value-tags
//! guarantee: bridge/object-refs
//! guarantee: bridge/unknown-fields

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};

use rutis::events::{Call, Scope};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, InstanceId, PluginFactory};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError, Event};
use rutis_bridge::channel::{Channel, ChannelError, Close, Closer, PeerId, Receiver, Sender};
use rutis_bridge::runtime::{row_projection, row_projection_of, Process};
use rutis_bridge::session::{
    call_service, host_key, host_key_in, settle, BigInt, Connection, Decimal, Dispatch, Endpoint,
    Error, HostDispatch, Instant, Reference, Reply, Uuid, Value, ENDPOINT_PROTOCOL,
};
use rutis_bridge::{
    catalog_key, session_key, Installed, LocalId, PluginCatalog, Session, StaticCatalog,
};
use rutis_test_support::{until, within};
use serde_json::{json, Value as Json};
use tokio::sync::oneshot;

mod common;

/// The capabilities of every tagged value, as a far end declares them when
/// it takes all of them.
const VALUE_CAPABILITIES: [&str; 10] = [
    "value-instant",
    "value-datetime",
    "value-date",
    "value-time",
    "value-bytes",
    "value-map",
    "value-set",
    "value-bigint",
    "value-decimal",
    "value-uuid",
];

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

/// A far end that answers a call with its arguments.
struct Echo;
impl Dispatch for Echo {
    fn invoke(&self, _: &Connection, _: &str, _: &str, args: Value) -> Reply {
        Ok(args)
    }
}

/// A far end that returns an instant from `instant`, and its arguments
/// otherwise.
struct Instanting;
impl Dispatch for Instanting {
    fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
        match method {
            "instant" => Ok(Value::Instant(Instant::from_unix(0, 0).unwrap())),
            _ => Ok(args),
        }
    }
}

/// A far end that answers every call with `ok`: the tagged value itself is
/// what the far end scripted on the channel checks.
struct Ack;
impl Dispatch for Ack {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        Ok(Value::Data(json!("ok")))
    }
}

/// A far end that hands out a function reference from `give`.
struct Giving;
impl Dispatch for Giving {
    fn invoke(&self, _: &Connection, _: &str, method: &str, _: Value) -> Reply {
        match method {
            "give" => Ok(Value::callback(|_| Ok(Value::Undefined))),
            _ => Ok(Value::Undefined),
        }
    }
}

/// A session between this side (`main`) and a far end scripted on the
/// channel, with this side's capabilities.
fn session_with(dispatch: Arc<dyn Dispatch>, endpoint: Endpoint) -> (Connection, Channel) {
    let (ours, theirs) = rutis_bridge::memory::pair();
    let peer = Connection::open(ours, dispatch, endpoint).unwrap();
    (peer, theirs)
}

/// A session between this side (`main`) and a far end scripted on the
/// channel.
fn session(dispatch: Arc<dyn Dispatch>) -> (Connection, Channel) {
    session_with(dispatch, Endpoint::rust(id("main")))
}

fn read(channel: &mut Channel) -> Json {
    match channel.receiver.recv().unwrap() {
        Some(message) => serde_json::from_slice(&message).unwrap(),
        None => panic!("the channel ended"),
    }
}

/// [`read`] for a frame nested deeper than serde_json's own limit: the
/// depth rule (SES-6.1-20) writes frames of about 130 levels.
fn read_deep(channel: &mut Channel) -> Json {
    use serde::Deserialize;
    let message = match channel.receiver.recv().unwrap() {
        Some(message) => message,
        None => panic!("the channel ended"),
    };
    let mut deserializer = serde_json::Deserializer::from_slice(&message);
    deserializer.disable_recursion_limit();
    let frame = Json::deserialize(&mut deserializer).expect("a frame");
    deserializer.end().expect("one frame");
    frame
}

fn send(channel: &mut Channel, frame: Json) {
    channel
        .sender
        .send(&serde_json::to_vec(&frame).unwrap())
        .unwrap();
}

/// Answer this side's greeting as `endpoint`, declaring `capabilities`, so
/// the session is ready.
fn greet(channel: &mut Channel, endpoint: &str, capabilities: &[&str]) {
    let hello = read(channel);
    assert_eq!(hello["op"], json!("hello"), "{hello}");
    assert_eq!(hello["version"], json!(ENDPOINT_PROTOCOL), "{hello}");
    assert_eq!(hello["endpoint"], json!("main"), "{hello}");
    // The session protocol has no `objects` capability: what a far end takes
    // by reference is not declared, so nothing can be checked before sending
    // an object reference (M1-13-3).
    let declared = hello["capabilities"].as_array().expect("capabilities");
    assert!(
        declared.iter().all(|capability| capability != "objects"),
        "{hello}"
    );
    send(
        channel,
        json!({
            "op": "hello",
            "version": ENDPOINT_PROTOCOL,
            "endpoint": endpoint,
            "capabilities": capabilities,
        }),
    );
}

/// The `id` of a frame the far end reads.
fn frame_id(frame: &Json) -> String {
    frame["id"].as_str().expect("an id").to_owned()
}

/// A call from the far end into this side, with `args` as the protocol's
/// value shape.
fn invoke(id: &str, target: &str, method: &str, args: Json) -> Json {
    json!({
        "op": "invoke",
        "id": id,
        "path": [],
        "target": target,
        "method": method,
        "args": args,
    })
}

/// One argument of a call, as the protocol carries an argument list.
fn one(arg: &Json) -> Json {
    json!({ "type": "list", "value": [arg] })
}

/// A call of `echo` with `arg` as its only argument; the reply the far end
/// reads.
fn echo(far: &mut Channel, id: &str, arg: &Json) -> Json {
    send(far, invoke(id, "svc", "echo", one(arg)));
    let reply = read(far);
    assert_eq!(reply["id"], json!(id), "{reply}");
    reply
}

/// `arg` crosses and comes back exactly as it was sent.
fn accepted(far: &mut Channel, id: &str, arg: &Json) {
    let reply = echo(far, id, arg);
    assert_eq!(reply["op"], json!("return"), "{arg}: {reply}");
    assert_eq!(reply["value"], one(arg), "{arg}: {reply}");
}

/// The error a far end reads in a `throw`: `(name, message, code, data)`.
fn thrown(frame: &Json) -> (String, String, Option<String>, Option<Json>) {
    assert_eq!(frame["op"], json!("throw"), "{frame}");
    let error = &frame["error"];
    let root = &error["graph"]["nodes"][0];
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

/// `arg` is refused only for its call: a `BindingError` with code
/// `bad-value`, the empty data of a received value, and `said` in its
/// message. The session goes on.
fn refused(far: &mut Channel, id: &str, arg: &Json, said: &str) {
    let reply = echo(far, id, arg);
    let (name, message, code, data) = thrown(&reply);
    assert_eq!(name, "BindingError", "{arg}: {reply}");
    assert_eq!(code.as_deref(), Some("bad-value"), "{arg}: {reply}");
    assert_eq!(data, Some(json!({})), "{arg}: {reply}");
    assert!(message.contains(said), "{arg}: {message}");
}

/// `arg` is not a value frame at all: the session ends as a transport
/// failure saying `said`, and a call after it fails.
async fn malformed(arg: &Json, said: &str) {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    send(&mut far, invoke("node:1", "svc", "echo", one(arg)));
    within(said, peer.closed()).await;
    match peer.close_reason() {
        Some(Error::Transport(message)) => assert!(message.contains(said), "{arg}: {message}"),
        other => panic!("{arg}: a transport error expected, got {other:?}"),
    }
    assert!(
        peer.invoke("svc", "echo", Value::Undefined).is_err(),
        "{arg}: a call after the end fails"
    );
}

/// `frame` is not a frame this side can read: the session ends as a
/// transport failure saying `said`, without a reply, and a call after it
/// fails.
async fn unreadable(frame: Json, said: &str) {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    send(&mut far, frame.clone());
    within(said, peer.closed()).await;
    match peer.close_reason() {
        Some(Error::Transport(message)) => assert!(message.contains(said), "{frame}: {message}"),
        other => panic!("{frame}: a transport error expected, got {other:?}"),
    }
    assert!(
        matches!(far.receiver.recv(), Ok(None)),
        "{frame}: the channel ends without a reply"
    );
    assert!(
        peer.invoke("svc", "echo", Value::Undefined).is_err(),
        "{frame}: a call after the end fails"
    );
}

/// A value is `{type: tag, value: …}` (SES-6.1-1): data, lists, records and
/// `undefined` cross in that shape, `undefined` kept as a whole argument
/// and result.
/// rule: SES-6.1-1
#[tokio::test(flavor = "multi_thread")]
async fn values_cross_in_the_protocols_tagged_shape() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();

    let samples = [
        json!({ "type": "data", "value": { "a": [1, true, null] } }),
        json!({ "type": "list", "value": [{ "type": "data", "value": 1 }, { "type": "data", "value": "x" }] }),
        json!({ "type": "record", "value": { "a": { "type": "data", "value": 1 } } }),
    ];
    for (n, arg) in samples.iter().enumerate() {
        accepted(&mut far, &format!("node:{}", n + 1), arg);
    }
    // A whole argument `undefined` stays `undefined`; the call's argument
    // list is a `list` value.
    send(
        &mut far,
        invoke("node:9", "svc", "echo", json!({ "type": "undefined" })),
    );
    assert_eq!(
        read(&mut far),
        json!({ "id": "node:9", "op": "return", "value": { "type": "undefined" } })
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// An ordinary JSON field is never read as a type (SES-6.1-3): data whose
/// content has the shape of a marker crosses as data, and the `type` and
/// `value` of a record are fields.
/// rule: SES-6.1-3
#[tokio::test(flavor = "multi_thread")]
async fn data_shaped_like_a_marker_stays_data() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    accepted(
        &mut far,
        "node:1",
        &json!({
            "type": "data",
            "value": { "type": "instant", "value": "2026-10-15T03:22:15Z" },
        }),
    );
    accepted(
        &mut far,
        "node:2",
        &json!({
            "type": "list",
            "value": [
                { "type": "data", "value": { "type": "date", "value": "2024-02-29" } },
                { "type": "data", "value": { "type": "bytes", "value": "AA==" } },
            ],
        }),
    );
    accepted(
        &mut far,
        "node:3",
        &json!({
            "type": "record",
            "value": {
                "type": { "type": "data", "value": "instant" },
                "value": { "type": "data", "value": "2026-10-15T03:22:15Z" },
            },
        }),
    );
    // The same shape from this side goes as data, not as a tag.
    let far_end = std::thread::spawn(move || {
        let call = read(&mut far);
        assert_eq!(
            call["args"],
            json!({ "type": "data", "value": { "type": "instant", "value": "2026-10-15T03:22:15Z" } }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
        );
        far
    });
    let answer = peer.invoke(
        "svc",
        "take",
        Value::Data(json!({ "type": "instant", "value": "2026-10-15T03:22:15Z" })),
    );
    assert_eq!(answer.unwrap().json().unwrap(), json!("ok"));
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `instant` is RFC 3339 in UTC: `YYYY-MM-DDTHH:MM:SS`, an optional 1–9
/// digit fraction, uppercase `T` and `Z`, years 0001–9999, seconds 00–59
/// (SES-6.1-4). Only the canonical text crosses.
/// rule: SES-6.1-4
#[tokio::test(flavor = "multi_thread")]
async fn instants_are_rfc3339_utc_text() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    for (n, text) in [
        "2026-10-15T03:22:15Z",
        "2026-10-15T03:22:15.5Z",
        "2026-10-15T03:22:15.123456789Z",
        "0001-01-01T00:00:00Z",
        "9999-12-31T23:59:59.999999999Z",
    ]
    .into_iter()
    .enumerate()
    {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "instant", "value": text }),
        );
    }
    let refused_texts = [
        "2026-10-15T03:22:15z",
        "2026-10-15T03:22:15+00:00",
        "2026-10-15 03:22:15Z",
        "2026-02-30T00:00:00Z",
        "2026-10-15T03:22:60Z",
        "0000-01-01T00:00:00Z",
        "2026-10-15T03:22:15.1234567890Z",
    ];
    for (n, text) in refused_texts.into_iter().enumerate() {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "instant", "value": text }),
            "invalid instant",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "instant", "value": "2026-10-15T03:22:15.5Z" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `datetime` is a local date and time, `YYYY-MM-DDTHH:MM:SS` with an
/// optional fraction and no zone (SES-6.1-5): it crosses unchanged, never
/// read in a time zone and never as an `instant`.
/// rule: SES-6.1-5
#[tokio::test(flavor = "multi_thread")]
async fn datetimes_are_local_text_without_a_zone() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    for (n, text) in [
        "2026-10-15T03:22:15",
        "2026-10-15T03:22:15.5",
        "2026-10-15T03:22:15.123456789",
        "0001-01-01T00:00:00",
        "9999-12-31T23:59:59.999999999",
        "2026-10-15T00:00:00",
    ]
    .into_iter()
    .enumerate()
    {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "datetime", "value": text }),
        );
    }
    let refused_texts = [
        "2026-10-15T03:22:15Z",
        "2026-10-15T03:22:15+00:00",
        "2026-10-15T03:22:15-05:00",
        "2026-10-15 03:22:15",
        "2026-02-30T00:00:00",
        "2026-10-15T24:00:00",
        "2026-10-15T03:22:60",
        "2026-10-15T03:22:15.1234567890",
        "0000-01-01T00:00:00",
    ];
    for (n, text) in refused_texts.into_iter().enumerate() {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "datetime", "value": text }),
            "invalid datetime",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "datetime", "value": "2026-10-15T03:22:15.5" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `date` is `YYYY-MM-DD`, years 0001–9999, and must be a day that exists
/// (SES-6.1-6).
/// rule: SES-6.1-6
#[tokio::test(flavor = "multi_thread")]
async fn dates_must_be_days_that_exist() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    for (n, text) in ["2024-02-29", "0001-01-01", "9999-12-31", "2026-04-30"]
        .into_iter()
        .enumerate()
    {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "date", "value": text }),
        );
    }
    for (n, text) in [
        "2026-02-29",
        "2026-04-31",
        "2026-13-01",
        "2026-00-10",
        "0000-01-01",
    ]
    .into_iter()
    .enumerate()
    {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "date", "value": text }),
            "invalid date",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "date", "value": "2024-02-29" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `time` is `HH:MM:SS`, 24-hour 00–23, an optional 1–9 digit fraction and
/// no zone (SES-6.1-7).
/// rule: SES-6.1-7
#[tokio::test(flavor = "multi_thread")]
async fn times_are_24_hour_text_without_a_zone() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    for (n, text) in [
        "00:00:00",
        "23:59:59",
        "14:30:05.25",
        "00:00:00.000000001",
        "12:34:56.789",
    ]
    .into_iter()
    .enumerate()
    {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "time", "value": text }),
        );
    }
    let refused_texts = [
        "24:00:00",
        "14:60:00",
        "14:30:60",
        "14:30:05Z",
        "14:30:05+01:00",
        "14:30:05+0100",
        "1:30:05",
        "14:30",
        "14:30:05.",
        "14:30:05.1234567890",
        " 14:30:05",
    ];
    for (n, text) in refused_texts.into_iter().enumerate() {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "time", "value": text }),
            "invalid time",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "time", "value": "14:30:05.25" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `bytes` is RFC 4648 §4 base64, padded, without whitespace, the unused
/// low bits zero: only the canonical text crosses (SES-6.1-8).
/// rule: SES-6.1-8
#[tokio::test(flavor = "multi_thread")]
async fn bytes_are_canonical_padded_base64() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    for (n, text) in ["", "AA==", "AAEC", "aGVsbG8="].into_iter().enumerate() {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "bytes", "value": text }),
        );
    }
    for (n, text) in [
        "AA",     // no padding
        "AA===",  // too much padding
        "A A==",  // whitespace
        "AB==",   // unused low bits not zero
        "-_==",   // URL alphabet
        "AA==\n", // whitespace
    ]
    .into_iter()
    .enumerate()
    {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "bytes", "value": text }),
            "invalid bytes",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "bytes", "value": "AA==" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `map` is `[[key, value], …]` in the sender's order (SES-6.1-9): the
/// order crosses both ways, not sorted or regrouped.
/// rule: SES-6.1-9
#[tokio::test(flavor = "multi_thread")]
async fn maps_keep_the_senders_order() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    accepted(
        &mut far,
        "node:1",
        &json!({ "type": "map", "value": [
            [{ "type": "data", "value": "z" }, { "type": "data", "value": 3 }],
            [{ "type": "data", "value": "a" }, { "type": "data", "value": 1 }],
            [{ "type": "data", "value": "m" }, { "type": "data", "value": 2 }],
            [{ "type": "bigint", "value": "9007199254740993" }, { "type": "data", "value": "big" }],
        ] }),
    );
    let far_end = std::thread::spawn(move || {
        let call = read(&mut far);
        assert_eq!(
            call["args"],
            json!({ "type": "map", "value": [
                [{ "type": "data", "value": "z" }, { "type": "data", "value": 3 }],
                [{ "type": "data", "value": "a" }, { "type": "data", "value": 1 }],
                [{ "type": "data", "value": "m" }, { "type": "data", "value": 2 }],
                [{ "type": "bigint", "value": "9007199254740993" }, { "type": "data", "value": "big" }],
            ] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
        );
        far
    });
    let sent = Value::Map(vec![
        (Value::Data(json!("z")), Value::Data(json!(3))),
        (Value::Data(json!("a")), Value::Data(json!(1))),
        (Value::Data(json!("m")), Value::Data(json!(2))),
        (
            Value::BigInt("9007199254740993".parse::<BigInt>().unwrap()),
            Value::Data(json!("big")),
        ),
    ]);
    assert_eq!(
        peer.invoke("svc", "take", sent).unwrap().json().unwrap(),
        json!("ok")
    );
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `set` is `[element, …]` in the sender's order (SES-6.1-10).
/// rule: SES-6.1-10
#[tokio::test(flavor = "multi_thread")]
async fn sets_keep_the_senders_order() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    accepted(
        &mut far,
        "node:1",
        &json!({ "type": "set", "value": [
            { "type": "data", "value": "z" },
            { "type": "data", "value": "a" },
            { "type": "data", "value": "m" },
        ] }),
    );
    let far_end = std::thread::spawn(move || {
        let call = read(&mut far);
        assert_eq!(
            call["args"],
            json!({ "type": "set", "value": [
                { "type": "data", "value": "z" },
                { "type": "data", "value": "a" },
                { "type": "data", "value": "m" },
            ] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
        );
        far
    });
    let sent = Value::Set(vec![
        Value::Data(json!("z")),
        Value::Data(json!("a")),
        Value::Data(json!("m")),
    ]);
    assert_eq!(
        peer.invoke("svc", "take", sent).unwrap().json().unwrap(),
        json!("ok")
    );
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `bigint` is a decimal integer, an optional `-`, no leading zeros, no
/// `-0`, at most 4300 digits (SES-6.1-11).
/// rule: SES-6.1-11
#[tokio::test(flavor = "multi_thread")]
async fn bigints_have_no_leading_zero_and_at_most_4300_digits() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    let longest = "1".repeat(4300);
    for (n, text) in ["0", "-5", "123456789012345678901234567890", &longest]
        .into_iter()
        .enumerate()
    {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "bigint", "value": text }),
        );
    }
    let too_long = "1".repeat(4301);
    for (n, text) in ["007", "-0", "+5", "", &too_long].into_iter().enumerate() {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "bigint", "value": text }),
            "invalid bigint",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "bigint", "value": "0" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `decimal` is `-?(0|[1-9][0-9]*)(\.[0-9]+)?`: no exponent, the trailing
/// zeros kept, at most 4300 digits (SES-6.1-12).
/// rule: SES-6.1-12
#[tokio::test(flavor = "multi_thread")]
async fn decimals_are_decimal_text_with_their_trailing_zeros() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    for (n, text) in ["0", "-0.0", "1.50", "123.456"].into_iter().enumerate() {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "decimal", "value": text }),
        );
    }
    for (n, text) in ["1e5", "1.", ".5", "01", "+1", "NaN", "1.0e5"]
        .into_iter()
        .enumerate()
    {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "decimal", "value": text }),
            "invalid decimal",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "decimal", "value": "1.50" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// `uuid` is lowercase `8-4-4-4-12` hex (SES-6.1-13): uppercase, braces
/// and any other layout are refused, and the session goes on.
/// rule: SES-6.1-13
#[tokio::test(flavor = "multi_thread")]
async fn uuids_are_lowercase_in_the_canonical_layout() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    for (n, text) in [
        "00000000-0000-0000-0000-000000000000",
        "0f8fad5b-d9cb-469f-a165-70867728950e",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
    ]
    .into_iter()
    .enumerate()
    {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "uuid", "value": text }),
        );
    }
    for (n, text) in [
        "0F8FAD5B-D9CB-469F-A165-70867728950E",
        "0f8fad5b-d9cb-469f-a165-70867728950g",
        "{0f8fad5b-d9cb-469f-a165-70867728950e}",
        "0f8fad5bd9cb469fa16570867728950e",
        "0f8fad5b-d9cb-469f-a165-70867728950",
        "0f8fad5b-d9cb-469f-a165-70867728950e0",
        "0f8fad5b-d9cb-469f-a165-7086772895-0e",
    ]
    .into_iter()
    .enumerate()
    {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "uuid", "value": text }),
            "invalid uuid",
        );
    }
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A map key and a set element may only be a string, a finite number, a
/// boolean, or a bigint, datetime, date, time, decimal or uuid value
/// (SES-6.1-14); anything else fails only that call.
/// rule: SES-6.1-14
#[tokio::test(flavor = "multi_thread")]
async fn map_keys_and_set_elements_are_limited_to_key_kinds() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    let allowed = json!([
        [{ "type": "data", "value": "s" }, { "type": "data", "value": 0 }],
        [{ "type": "data", "value": 1 }, { "type": "data", "value": 1 }],
        [{ "type": "data", "value": true }, { "type": "data", "value": 2 }],
        [{ "type": "bigint", "value": "9007199254740993" }, { "type": "data", "value": 3 }],
        [{ "type": "datetime", "value": "2026-10-15T03:22:15" }, { "type": "data", "value": 4 }],
        [{ "type": "date", "value": "2026-10-15" }, { "type": "data", "value": 5 }],
        [{ "type": "time", "value": "03:22:15" }, { "type": "data", "value": 6 }],
        [{ "type": "decimal", "value": "1.50" }, { "type": "data", "value": 7 }],
        [{ "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" }, { "type": "data", "value": 8 }],
    ]);
    accepted(
        &mut far,
        "node:1",
        &json!({ "type": "map", "value": allowed }),
    );
    accepted(
        &mut far,
        "node:2",
        &json!({ "type": "set", "value": [
            { "type": "data", "value": "s" },
            { "type": "data", "value": 1 },
            { "type": "data", "value": true },
            { "type": "bigint", "value": "9007199254740993" },
            { "type": "datetime", "value": "2026-10-15T03:22:15" },
            { "type": "date", "value": "2026-10-15" },
            { "type": "time", "value": "03:22:15" },
            { "type": "decimal", "value": "1.50" },
            { "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" },
        ] }),
    );
    let refused_keys = [
        json!({ "type": "instant", "value": "2026-10-15T03:22:15Z" }),
        json!({ "type": "bytes", "value": "AA==" }),
        json!({ "type": "data", "value": null }),
        json!({ "type": "undefined" }),
        json!({ "type": "list", "value": [] }),
        json!({ "type": "record", "value": {} }),
        json!({ "type": "map", "value": [] }),
        json!({ "type": "set", "value": [] }),
        json!({ "type": "signal" }),
    ];
    for (n, key) in refused_keys.iter().enumerate() {
        refused(
            &mut far,
            &format!("node:{}", n + 10),
            &json!({ "type": "map", "value": [[key, { "type": "data", "value": 1 }]] }),
            "a map key cannot be ",
        );
    }
    refused(
        &mut far,
        "node:30",
        &json!({ "type": "set", "value": [
            { "type": "instant", "value": "2026-10-15T03:22:15Z" },
        ] }),
        "a set element cannot be an instant",
    );
    accepted(
        &mut far,
        "node:99",
        &json!({ "type": "map", "value": [
            [{ "type": "data", "value": "k" }, { "type": "data", "value": 1 }],
        ] }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// An integer beyond ±(2^53−1) is a `bigint` key, equal to the same
/// `bigint`, and a whole-number float beyond that range cannot be a key
/// (SES-6.1-15).
/// rule: SES-6.1-15
#[tokio::test(flavor = "multi_thread")]
async fn bigint_keys_equal_the_same_value_and_float_keys_are_refused() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    accepted(
        &mut far,
        "node:1",
        &json!({ "type": "map", "value": [
            [{ "type": "bigint", "value": "9007199254740993" }, { "type": "data", "value": "a" }],
            [{ "type": "bigint", "value": "-9007199254740993" }, { "type": "data", "value": "b" }],
        ] }),
    );
    // The same integer as data crosses as a bigint key: the two are one key.
    refused(
        &mut far,
        "node:2",
        &json!({ "type": "map", "value": [
            [{ "type": "bigint", "value": "9007199254740993" }, { "type": "data", "value": "a" }],
            [{ "type": "data", "value": 9007199254740993u64 }, { "type": "data", "value": "b" }],
        ] }),
        "duplicate map key 9007199254740993",
    );
    // A whole-number float beyond the range is not exact in every
    // language, so it cannot be a key at all.
    refused(
        &mut far,
        "node:3",
        &json!({ "type": "map", "value": [
            [{ "type": "data", "value": 9007199254740992.0 }, { "type": "data", "value": "a" }],
        ] }),
        "a map key cannot be the float",
    );
    accepted(
        &mut far,
        "node:4",
        &json!({ "type": "map", "value": [
            [{ "type": "bigint", "value": "5" }, { "type": "data", "value": "small" }],
        ] }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A sender checks the keys of a map and the elements of a set before it
/// sends them (SES-6.1-17): a duplicate or a kind no language takes as a
/// key fails that call before any frame is written, naming the argument.
/// rule: SES-6.1-17
#[tokio::test(flavor = "multi_thread")]
async fn map_keys_are_checked_before_the_frame_is_written() {
    let (peer, mut far) = session(Arc::new(Ack));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    let duplicates = Value::Map(vec![
        (Value::Data(json!(1)), Value::Data(json!("a"))),
        (Value::Data(json!(1.0)), Value::Data(json!("b"))),
    ]);
    let refused_by_key = Value::Map(vec![(
        Value::Instant(Instant::from_unix(0, 0).unwrap()),
        Value::Data(json!("a")),
    )]);
    let refused_by_element = Value::Set(vec![Value::Data(json!("x")), Value::Data(json!("x"))]);
    for (n, (value, said)) in [
        (duplicates, "duplicate map key 1"),
        (refused_by_key, "a map key cannot be an instant"),
        (refused_by_element, "duplicate set element"),
    ]
    .into_iter()
    .enumerate()
    {
        let error = peer
            .invoke("svc", "take", Value::List(vec![value]))
            .expect_err("the value is refused before it is sent");
        assert_eq!(error.code(), Some("bad-value"), "{n}: {error}");
        assert_eq!(
            error.data(),
            Some(json!({ "part": "argument", "argument": 1 })),
            "{n}: {error}"
        );
        assert!(error.to_string().contains(said), "{n}: {error}");
    }
    // Nothing of the refused calls was written: the far end's next frame is
    // the call that is sent.
    let far_end = std::thread::spawn(move || {
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [{ "type": "map", "value": [
                [{ "type": "data", "value": "k" }, { "type": "data", "value": 1 }],
            ] }] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
        );
        far
    });
    let sent = Value::List(vec![Value::Map(vec![(
        Value::Data(json!("k")),
        Value::Data(json!(1)),
    )])]);
    assert_eq!(
        peer.invoke("svc", "take", sent).unwrap().json().unwrap(),
        json!("ok")
    );
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// Two keys of one map, or two elements of one set, may not be equal:
/// numbers by value (`1` and `1.0`), the rest by kind and text
/// (SES-6.1-16).
/// rule: SES-6.1-16
#[tokio::test(flavor = "multi_thread")]
async fn equal_map_keys_and_set_elements_are_refused() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    refused(
        &mut far,
        "node:1",
        &json!({ "type": "map", "value": [
            [{ "type": "data", "value": 1 }, { "type": "data", "value": "a" }],
            [{ "type": "data", "value": 1.0 }, { "type": "data", "value": "b" }],
        ] }),
        "duplicate map key",
    );
    refused(
        &mut far,
        "node:2",
        &json!({ "type": "set", "value": [
            { "type": "data", "value": "x" },
            { "type": "data", "value": "x" },
        ] }),
        "duplicate set element",
    );
    accepted(
        &mut far,
        "node:3",
        &json!({ "type": "map", "value": [
            [{ "type": "data", "value": 1 }, { "type": "data", "value": "a" }],
            [{ "type": "data", "value": 2 }, { "type": "data", "value": "b" }],
        ] }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A `map` and a `set` count one of the 64 levels a value may nest
/// (SES-6.1-20), like a list: a value 64 levels deep crosses, 65 is
/// refused before it is sent, naming the argument and `too-deep`.
/// rule: SES-6.1-20
#[tokio::test(flavor = "multi_thread")]
async fn a_map_or_set_counts_one_of_the_sixty_four_levels() {
    fn lists(levels: usize, inner: Value) -> Value {
        (0..levels).fold(inner, |inner, _| Value::List(vec![inner]))
    }
    fn map() -> Value {
        Value::Map(vec![(Value::Data(json!("k")), Value::Data(json!(1)))])
    }
    fn set() -> Value {
        Value::Set(vec![Value::Data(json!("x"))])
    }
    let (peer, mut far) = session(Arc::new(Ack));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    // One level too many: the map or set is the 65th.
    for (n, value) in [lists(64, map()), lists(64, set())].into_iter().enumerate() {
        let error = peer
            .invoke("svc", "take", Value::List(vec![value]))
            .expect_err("the value nests deeper than 64 levels");
        assert_eq!(error.code(), Some("bad-value"), "{n}: {error}");
        assert_eq!(
            error.data(),
            Some(json!({ "part": "argument", "argument": 1, "reason": "too-deep" })),
            "{n}: {error}"
        );
        assert!(
            error
                .to_string()
                .contains("the value nests deeper than 64 levels"),
            "{n}: {error}"
        );
    }
    let far_end = std::thread::spawn(move || {
        for (n, kind) in ["map", "set"].into_iter().enumerate() {
            let call = read_deep(&mut far);
            // The argument list, then 63 lists: the map or set is the 64th
            // level.
            let mut value = &call["args"];
            for _ in 0..64 {
                assert_eq!(value["type"], json!("list"), "{n}: {call}");
                value = &value["value"][0];
            }
            assert_eq!(value["type"], json!(kind), "{n}: {call}");
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
            );
        }
        far
    });
    for (n, value) in [lists(63, map()), lists(63, set())].into_iter().enumerate() {
        assert_eq!(
            peer.invoke("svc", "take", Value::List(vec![value]))
                .unwrap()
                .json()
                .unwrap(),
            json!("ok"),
            "{n}"
        );
    }
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// Each tag has one capability, `value-<tag>`, and a far end may declare
/// only some of them (SES-6.1-22): a value whose tag it declared crosses,
/// one it did not fails before it is sent, naming the tag and the
/// capability; the rest of the session goes on.
/// rule: SES-6.1-22
#[tokio::test(flavor = "multi_thread")]
async fn a_partial_declaration_takes_only_the_declared_tags() {
    let declared: Vec<&str> = VALUE_CAPABILITIES
        .iter()
        .copied()
        .filter(|capability| !matches!(*capability, "value-instant" | "value-bytes"))
        .collect();
    let (peer, mut far) = session(Arc::new(Ack));
    greet(&mut far, "node", &declared);
    peer.ready().await.unwrap();
    // An undeclared tag fails before the frame is written.
    for (n, (tag, capability, value)) in [
        (
            "instant",
            "value-instant",
            Value::Instant(Instant::from_unix(0, 0).unwrap()),
        ),
        ("bytes", "value-bytes", Value::Bytes(vec![0, 1])),
    ]
    .into_iter()
    .enumerate()
    {
        let error = peer
            .invoke("svc", "take", Value::List(vec![value]))
            .expect_err("the far end did not declare the tag");
        assert_eq!(error.code(), Some("bad-value"), "{n}: {error}");
        assert_eq!(
            error.data(),
            Some(json!({ "part": "argument", "argument": 1 })),
            "{n}: {error}"
        );
        assert!(
            error.to_string().contains(&format!(
                "the far end does not take {tag} values (it did not declare the capability {capability})"
            )),
            "{n}: {error}"
        );
    }
    // Every declared tag crosses, unchanged.
    let far_end = std::thread::spawn(move || {
        for (tag, text) in [
            ("datetime", "2026-10-15T03:22:15"),
            ("date", "2026-10-15"),
            ("time", "03:22:15"),
            (
                "map",
                "[[{\"type\":\"data\",\"value\":\"k\"},{\"type\":\"data\",\"value\":1}]]",
            ),
            ("set", "[{\"type\":\"data\",\"value\":\"x\"}]"),
            ("bigint", "9007199254740993"),
            ("decimal", "1.50"),
            ("uuid", "0f8fad5b-d9cb-469f-a165-70867728950e"),
        ]
        .into_iter()
        {
            let call = read(&mut far);
            let argument = &call["args"]["value"][0];
            assert_eq!(argument["type"], json!(tag), "{call}");
            if tag == "map" || tag == "set" {
                assert_eq!(
                    argument["value"],
                    serde_json::from_str::<Json>(text).unwrap(),
                    "{call}"
                );
            } else {
                assert_eq!(argument["value"], json!(text), "{call}");
            }
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
            );
        }
        let call = read(&mut far);
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [{ "type": "data", "value": "after" }] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
        );
        far
    });
    let values = [
        Value::DateTime("2026-10-15T03:22:15".parse().unwrap()),
        Value::Date("2026-10-15".parse().unwrap()),
        Value::Time("03:22:15".parse().unwrap()),
        Value::Map(vec![(Value::Data(json!("k")), Value::Data(json!(1)))]),
        Value::Set(vec![Value::Data(json!("x"))]),
        Value::BigInt("9007199254740993".parse::<BigInt>().unwrap()),
        Value::Decimal("1.50".parse::<Decimal>().unwrap()),
        Value::Uuid(
            "0f8fad5b-d9cb-469f-a165-70867728950e"
                .parse::<Uuid>()
                .unwrap(),
        ),
    ];
    for (n, value) in values.into_iter().enumerate() {
        assert_eq!(
            peer.invoke("svc", "take", Value::List(vec![value]))
                .unwrap()
                .json()
                .unwrap(),
            json!("ok"),
            "{n}"
        );
    }
    assert_eq!(
        peer.invoke(
            "svc",
            "take",
            Value::List(vec![Value::Data(json!("after"))])
        )
        .unwrap()
        .json()
        .unwrap(),
        json!("ok")
    );
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A tagged value goes only to a far end that declared `value-<tag>`
/// (SES-6.1-24): an argument fails before it is sent, a result answers the
/// far end's call with a `throw`; the error names the value and the
/// capability, and the session goes on.
/// rule: SES-6.1-24
#[tokio::test(flavor = "multi_thread")]
async fn a_tagged_value_to_a_far_end_without_its_capability_fails_only_that_call() {
    let (peer, mut far) = session(Arc::new(Instanting));
    // The far end takes no `instant` values.
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    // A result it cannot take: this side's answer is a `throw` naming the
    // result and the capability.
    send(
        &mut far,
        invoke("node:1", "svc", "instant", json!({ "type": "undefined" })),
    );
    let reply = read(&mut far);
    let (name, message, code, data) = thrown(&reply);
    assert_eq!(name, "BindingError");
    assert_eq!(code.as_deref(), Some("bad-value"));
    assert_eq!(data, Some(json!({ "part": "result" })));
    assert_eq!(
        message,
        "svc.instant: the result cannot be sent: cannot send an instant: \
         the far end does not take instant values (it did not declare the \
         capability value-instant)"
    );
    // An argument it cannot take: this side's call fails before anything is
    // sent, with code and data.
    let error = peer
        .invoke(
            "svc",
            "take",
            Value::List(vec![Value::Instant(Instant::from_unix(0, 0).unwrap())]),
        )
        .expect_err("the far end does not take instants");
    assert_eq!(error.code(), Some("bad-value"));
    assert_eq!(
        error.data(),
        Some(json!({ "part": "argument", "argument": 1 }))
    );
    assert!(
        error
            .to_string()
            .contains("the far end does not take instant values (it did not declare the capability value-instant)"),
        "{error}"
    );
    // Nothing of the refused call was sent: the next frame is the next call,
    // and both calls are answered.
    send(
        &mut far,
        invoke(
            "node:2",
            "svc",
            "echo",
            json!({ "type": "data", "value": "after" }),
        ),
    );
    assert_eq!(
        read(&mut far),
        json!({ "id": "node:2", "op": "return", "value": { "type": "data", "value": "after" } })
    );
    let answering = std::thread::spawn(move || {
        let next = read(&mut far);
        assert_eq!(frame_id(&next), "main:2", "{next}");
        assert_eq!(
            next["args"],
            json!({ "type": "data", "value": 1 }),
            "{next}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": "main:2", "value": { "type": "data", "value": "plain" } }),
        );
        far
    });
    let plain = peer.invoke("svc", "echo", Value::Data(json!(1)));
    assert_eq!(plain.unwrap().json().unwrap(), json!("plain"));
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || answering.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A received value this side does not take, one not in its format, or one
/// whose keys collide fails only its call: the far end reads a `throw` with
/// code `bad-value`; the session goes on (SES-6.1-26).
/// rule: SES-6.1-26
#[tokio::test(flavor = "multi_thread")]
async fn a_received_value_this_side_refuses_fails_only_its_call() {
    // This side takes every tagged value but `uuid`.
    let mut endpoint = Endpoint::rust(id("main"));
    endpoint.capabilities.retain(|c| c != "value-uuid");
    let (peer, mut far) = session_with(Arc::new(Echo), endpoint);
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    // A tag this side did not declare: the far end reads its call's error.
    refused(
        &mut far,
        "node:1",
        &json!({ "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" }),
        "received a uuid value, which this side does not take \
         (it did not declare the capability value-uuid)",
    );
    // A value not in its format, and keys that collide.
    refused(
        &mut far,
        "node:2",
        &json!({ "type": "instant", "value": "2026-10-15T03:22:15+02:00" }),
        "invalid instant",
    );
    refused(
        &mut far,
        "node:3",
        &json!({ "type": "map", "value": [
            [{ "type": "data", "value": 1 }, { "type": "data", "value": "a" }],
            [{ "type": "data", "value": 1.0 }, { "type": "data", "value": "b" }],
        ] }),
        "duplicate map key",
    );
    // The session goes on: a value it takes crosses.
    accepted(
        &mut far,
        "node:4",
        &json!({ "type": "instant", "value": "2026-10-15T03:22:15Z" }),
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A `value` whose JSON shape is wrong (an `instant` that is not a string,
/// a `map` entry that is not a pair) is a malformed frame: the session
/// ends, and a call after it fails (SES-6.1-27).
/// rule: SES-6.1-27
#[tokio::test(flavor = "multi_thread")]
async fn a_value_of_the_wrong_json_shape_ends_the_session() {
    malformed(
        &json!({ "type": "instant", "value": 5 }),
        "invalid type: integer `5`, expected a string",
    )
    .await;
    malformed(
        &json!({ "type": "map", "value": [1] }),
        "invalid type: integer `1`, expected a tuple of size 2",
    )
    .await;
    malformed(
        &json!({ "type": "map", "value": [[{ "type": "data", "value": 1 }]] }),
        "invalid length 1, expected a tuple of size 2",
    )
    .await;
    malformed(
        &json!({ "type": "set", "value": 5 }),
        "invalid type: integer `5`, expected a sequence",
    )
    .await;
    malformed(
        &json!({ "type": "record", "value": [] }),
        "invalid type: sequence, expected a map",
    )
    .await;
    malformed(&json!({ "type": "data" }), "missing field `value`").await;
    malformed(
        &json!({ "type": "bogus", "value": 1 }),
        "unknown variant `bogus`",
    )
    .await;
}

/// A call frame with a field of the wrong type, or a missing one
/// (CHN-4-4): the frame cannot be read, the session ends as a transport
/// failure naming the field, without a reply.
/// rule: CHN-4-4
#[tokio::test(flavor = "multi_thread")]
async fn a_frame_with_a_mistyped_field_ends_the_session() {
    let cases = [
        (
            json!({ "op": "invoke", "id": "node:1", "path": [], "target": 7,
                    "method": "echo", "args": { "type": "undefined" } }),
            "invalid type: integer `7`, expected a string",
        ),
        (
            json!({ "op": "invoke", "id": "node:1", "path": [], "target": "svc",
                    "args": { "type": "undefined" } }),
            "missing field `method`",
        ),
        (
            json!({ "op": "get", "id": "node:1", "path": [], "reference": 7,
                    "property": 5 }),
            "invalid type: integer `5`, expected a string",
        ),
        (
            json!({ "op": "cancel", "id": 5 }),
            "invalid type: integer `5`, expected a string",
        ),
        (
            json!({ "op": "release", "reference": 7, "count": "1" }),
            "invalid type: string \"1\", expected u64",
        ),
        (
            json!({ "op": "throw", "id": "node:1",
                    "error": { "name": 7, "message": "x" } }),
            "invalid type: integer `7`, expected a string",
        ),
        (
            json!({ "op": "throw", "id": "node:1",
                    "error": { "name": "Error", "message": 7 } }),
            "invalid type: integer `7`, expected a string",
        ),
    ];
    for (frame, said) in cases {
        unreadable(frame, said).await;
    }
}

/// A call, a property read or an await naming a reference this side does
/// not have, or already released, and a release of an unknown reference or
/// with a count of 0 or over granted (CHN-4-6): the frame is malformed,
/// the session ends as a transport failure, and a call after it fails.
/// rule: CHN-4-6
#[tokio::test(flavor = "multi_thread")]
async fn unknown_or_released_references_end_the_session() {
    /// A session with a reference this side exported (`give`); the far end
    /// sends `frames(reference)` and the session ends saying `said`.
    async fn ending(said: &str, frames: impl FnOnce(u64) -> Vec<Json>) {
        let (peer, mut far) = session(Arc::new(Giving));
        greet(&mut far, "node", &[]);
        peer.ready().await.unwrap();
        send(
            &mut far,
            invoke("node:1", "svc", "give", json!({ "type": "undefined" })),
        );
        let reply = read(&mut far);
        let reference = reply["value"]["value"]["id"]
            .as_u64()
            .expect("a reference id");
        for frame in frames(reference) {
            send(&mut far, frame);
        }
        within(said, peer.closed()).await;
        match peer.close_reason() {
            Some(Error::Transport(message)) => assert!(message.contains(said), "{message}"),
            other => panic!("a transport error expected, got {other:?}"),
        }
        assert!(
            peer.invoke("svc", "echo", Value::Undefined).is_err(),
            "a call after the end fails"
        );
    }
    ending("unknown or released reference", |_| {
        vec![
            json!({ "op": "call", "id": "node:2", "path": [], "reference": 99,
                      "args": { "type": "undefined" } }),
        ]
    })
    .await;
    ending("unknown or released reference", |_| {
        vec![
            json!({ "op": "get", "id": "node:3", "path": [], "reference": 99,
                      "property": "name" }),
        ]
    })
    .await;
    ending("unknown or released reference", |_| {
        vec![json!({ "op": "await", "id": "node:4", "path": [], "reference": 99 })]
    })
    .await;
    ending("release of unknown reference", |_| {
        vec![json!({ "op": "release", "reference": 99, "count": 1 })]
    })
    .await;
    ending("invalid reference release count", |reference| {
        vec![json!({ "op": "release", "reference": reference, "count": 0 })]
    })
    .await;
    ending("invalid reference release count", |reference| {
        vec![json!({ "op": "release", "reference": reference, "count": 2 })]
    })
    .await;
    // Released once: the next call names a reference that is gone.
    ending("unknown or released reference", |reference| {
        vec![
            json!({ "op": "release", "reference": reference, "count": 1 }),
            json!({ "op": "call", "id": "node:5", "path": [], "reference": reference,
                    "args": { "type": "undefined" } }),
        ]
    })
    .await;
    ending("unknown or released reference", |reference| {
        vec![
            json!({ "op": "release", "reference": reference, "count": 1 }),
            json!({ "op": "get", "id": "node:6", "path": [], "reference": reference,
                    "property": "name" }),
        ]
    })
    .await;
    ending("release of unknown reference", |reference| {
        vec![
            json!({ "op": "release", "reference": reference, "count": 1 }),
            json!({ "op": "release", "reference": reference, "count": 1 }),
        ]
    })
    .await;
}

/// A request before the handshake, and a second `hello`, are protocol
/// failures: the session ends, and no call is answered (CHN-4-8).
/// rule: CHN-4-8
#[tokio::test(flavor = "multi_thread")]
async fn a_request_before_the_handshake_or_a_second_hello_ends_the_session() {
    {
        let (peer, mut far) = session(Arc::new(Echo));
        let hello = read(&mut far);
        assert_eq!(hello["op"], json!("hello"));
        // A call before this side's greeting ends the session.
        send(
            &mut far,
            invoke("node:1", "svc", "echo", json!({ "type": "undefined" })),
        );
        within("the session's end", peer.closed()).await;
        match peer.close_reason() {
            Some(Error::Transport(message)) => {
                assert_eq!(message, "request before protocol handshake")
            }
            other => panic!("a transport error expected, got {other:?}"),
        }
        assert!(
            matches!(far.receiver.recv(), Ok(None)),
            "the channel ends without a reply"
        );
    }
    {
        let (peer, mut far) = session(Arc::new(Echo));
        greet(&mut far, "node", &[]);
        // A second greeting, after the first.
        send(
            &mut far,
            json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "node" }),
        );
        within("the session's end", peer.closed()).await;
        match peer.close_reason() {
            Some(Error::Transport(message)) => {
                assert_eq!(message, "duplicate protocol handshake")
            }
            other => panic!("a transport error expected, got {other:?}"),
        }
    }
}

/// A reply to a call this side gave up on is dropped, counted, and does not
/// end the session (CHN-4-9).
/// rule: CHN-4-9
#[tokio::test(flavor = "multi_thread")]
async fn a_late_reply_to_a_cancelled_call_is_dropped() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let (seen, was_seen) = mpsc::channel();
    let far_end = std::thread::spawn(move || {
        let call = read(&mut far);
        let id = frame_id(&call);
        seen.send(id.clone()).unwrap();
        // The caller gave up: a cancel for that call.
        assert_eq!(read(&mut far), json!({ "op": "cancel", "id": id }));
        // A late reply for the cancelled call: dropped, not a protocol
        // failure.
        send(
            &mut far,
            json!({ "op": "return", "id": id, "value": { "type": "data", "value": "late" } }),
        );
        // A call after it is answered.
        let next = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&next), "value": { "type": "data", "value": "after" } }),
        );
        far
    });
    let mut call = Box::pin(peer.invoke_async("svc", "slow", Value::Undefined));
    tokio::select! {
        biased;
        answer = &mut call => panic!("the call only ends when given up: {answer:?}"),
        received = tokio::task::spawn_blocking(move || was_seen.recv().unwrap()) => {
            received.unwrap();
        }
    }
    drop(call);
    let reply = peer.invoke_async("svc", "echo", Value::Data(json!(1)));
    assert_eq!(
        within("a call after the late reply", reply)
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("after")
    );
    let _far = within(
        "the far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A `cancel` for a call this side has not got is ignored (it may have
/// overtaken its own reply), and the session goes on (CHN-4-10).
/// rule: CHN-4-10
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_for_an_unknown_call_is_ignored() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    send(&mut far, json!({ "op": "cancel", "id": "node:99" }));
    send(
        &mut far,
        invoke(
            "node:1",
            "svc",
            "echo",
            json!({ "type": "data", "value": "after" }),
        ),
    );
    assert_eq!(
        read(&mut far),
        json!({ "id": "node:1", "op": "return", "value": { "type": "data", "value": "after" } })
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A call frame whose `path` already holds its own `id` is an illegal
/// frame (TAG-2-1): the session ends as a transport failure, without a
/// reply; a path holding another call's id is not.
/// rule: TAG-2-1
#[tokio::test(flavor = "multi_thread")]
async fn a_path_holding_the_frames_own_id_ends_the_session() {
    {
        let (peer, mut far) = session(Arc::new(Echo));
        greet(&mut far, "node", &[]);
        peer.ready().await.unwrap();
        // Only the frame's own id in its path is illegal.
        let mut looped = invoke("node:1", "svc", "echo", json!({ "type": "undefined" }));
        looped["path"] = json!(["node:1"]);
        send(&mut far, looped);
        within("the session's end", peer.closed()).await;
        match peer.close_reason() {
            Some(Error::Transport(message)) => {
                assert_eq!(message, "invalid invocation identity/path")
            }
            other => panic!("a transport error expected, got {other:?}"),
        }
        assert!(
            matches!(far.receiver.recv(), Ok(None)),
            "the channel ends without a reply"
        );
        assert!(peer.invoke("svc", "echo", Value::Undefined).is_err());
    }
    {
        // A path with another call's id crosses.
        let (peer, mut far) = session(Arc::new(Echo));
        greet(&mut far, "node", &[]);
        peer.ready().await.unwrap();
        let mut chained = invoke(
            "node:2",
            "svc",
            "echo",
            json!({ "type": "data", "value": "after" }),
        );
        chained["path"] = json!(["node:1"]);
        send(&mut far, chained);
        assert_eq!(
            read(&mut far),
            json!({ "id": "node:2", "op": "return",
                    "value": { "type": "data", "value": "after" } })
        );
        assert!(peer.close_reason().is_none(), "the session goes on");
    }
    {
        // A `call` on an exported reference checks its path the same way.
        let (peer, mut far) = session(Arc::new(Giving));
        greet(&mut far, "node", &[]);
        peer.ready().await.unwrap();
        send(
            &mut far,
            invoke("node:1", "svc", "give", json!({ "type": "undefined" })),
        );
        let reply = read(&mut far);
        let reference = reply["value"]["value"]["id"]
            .as_u64()
            .expect("a reference id");
        send(
            &mut far,
            json!({ "op": "call", "id": "node:2", "path": ["node:2"],
                    "reference": reference, "args": { "type": "undefined" } }),
        );
        within("the session's end", peer.closed()).await;
        match peer.close_reason() {
            Some(Error::Transport(message)) => {
                assert_eq!(message, "invalid invocation identity/path")
            }
            other => panic!("a transport error expected, got {other:?}"),
        }
    }
}

/// A business `close()` and a proxy release are separate (SES-7.1-6):
/// dropping an imported object reference writes a `release` frame and
/// calls no business method on its owner; the session goes on.
/// rule: SES-7.1-6
#[tokio::test(flavor = "multi_thread")]
async fn releasing_a_proxy_calls_no_business_method() {
    let (owner, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "a", &[]);
    owner.ready().await.unwrap();
    let far_end = std::thread::spawn(move || {
        // A hands out an object reference.
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list",
                "value": [{ "type": "reference", "value": {
                    "id": 7, "home": false, "kind": "object", "origin": [],
                } }],
            } }),
        );
        // Releasing the proxy is only a release: no `call`, no `get`, no
        // business `close()`.
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        // The next frame is the next call, answered as usual.
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "alive" } }),
        );
        far
    });
    let object = owner
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    drop(object);
    assert_eq!(
        owner
            .invoke("svc", "still", Value::Undefined)
            .unwrap()
            .json()
            .unwrap(),
        json!("alive")
    );
    let _far = within(
        "the owner's far end",
        tokio::task::spawn_blocking(move || far_end.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(owner.close_reason().is_none(), "the session goes on");
}

/// A reference another session exported is relayed to a third session as a
/// live reference: one reference however often it is sent; its methods and
/// properties are forwarded to its owner with the chain rebased; releasing
/// the relay releases it there (M1-7.1-1).
/// rule: M1-7.1-1
#[tokio::test(flavor = "multi_thread")]
async fn an_object_relayed_to_another_session_reaches_its_owner() {
    let (a, ra) = session(Arc::new(Nothing));
    let (b, rb) = session(Arc::new(Nothing));
    let mut ra = ra;
    let mut rb = rb;
    greet(&mut ra, "a", &[]);
    greet(&mut rb, "b", &[]);
    a.ready().await.unwrap();
    b.ready().await.unwrap();

    let a_far = std::thread::spawn(move || {
        // A owns the object: it hands out reference 7 of its own session.
        let call = read(&mut ra);
        send(
            &mut ra,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list",
                "value": [{ "type": "reference", "value": {
                    "id": 7, "home": false, "kind": "object", "origin": [],
                } }],
            } }),
        );
        // The relayed method call: the owner's own id, and a chain rebased
        // to the session that made the call.
        let call = read(&mut ra);
        assert_eq!(call["op"], json!("call"), "{call}");
        assert_eq!(call["reference"], json!(7), "{call}");
        assert_eq!(call["method"], json!("greet"), "{call}");
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [] }),
            "{call}"
        );
        let path = call["path"].as_array().expect("a path");
        assert_eq!(path.len(), 1, "{call}");
        assert!(
            path[0].as_str().unwrap().ends_with("/b:1"),
            "the chain is rebased to the caller's session: {call}"
        );
        send(
            &mut ra,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "hello" } }),
        );
        // The relayed property read.
        let call = read(&mut ra);
        assert_eq!(call["op"], json!("get"), "{call}");
        assert_eq!(call["reference"], json!(7), "{call}");
        assert_eq!(call["property"], json!("name"), "{call}");
        send(
            &mut ra,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "cache" } }),
        );
        // The relay was released: the owner's own reference is released.
        assert_eq!(
            read(&mut ra),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        ra
    });
    let b_far = std::thread::spawn(move || {
        // The object handed over, as this session's reference.
        let call = read(&mut rb);
        let reference = call["args"]["value"][0]["value"].clone();
        assert_eq!(reference["kind"], json!("object"), "{call}");
        assert_eq!(reference["home"], json!(false), "{call}");
        let reference = reference["id"].as_u64().expect("a reference id");
        send(
            &mut rb,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "taken" } }),
        );
        // Handed over again: the same reference.
        let call = read(&mut rb);
        assert_eq!(
            call["args"]["value"][0]["value"]["id"],
            json!(reference),
            "{call}"
        );
        send(
            &mut rb,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "again" } }),
        );
        // Its method and property reach the owner.
        send(
            &mut rb,
            json!({ "op": "call", "id": "b:1", "path": [], "reference": reference,
                    "method": "greet", "args": { "type": "list", "value": [] }, "sync": false }),
        );
        assert_eq!(
            read(&mut rb),
            json!({ "id": "b:1", "op": "return", "value": { "type": "data", "value": "hello" } })
        );
        send(
            &mut rb,
            json!({ "op": "get", "id": "b:2", "path": [], "reference": reference,
                    "property": "name", "sync": false }),
        );
        assert_eq!(
            read(&mut rb),
            json!({ "id": "b:2", "op": "return", "value": { "type": "data", "value": "cache" } })
        );
        // Both grants released.
        send(
            &mut rb,
            json!({ "op": "release", "reference": reference, "count": 2 }),
        );
        rb
    });

    let object = a
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    let handed = Value::List(vec![Value::Reference(object.clone())]);
    assert_eq!(
        b.invoke("svc", "take", handed.clone())
            .unwrap()
            .json()
            .unwrap(),
        json!("taken")
    );
    assert_eq!(
        b.invoke("svc", "take", handed).unwrap().json().unwrap(),
        json!("again")
    );
    drop(object);
    let _rb = within(
        "b's far end",
        tokio::task::spawn_blocking(move || b_far.join().unwrap()),
    )
    .await
    .unwrap();
    let _ra = within(
        "a's far end",
        tokio::task::spawn_blocking(move || a_far.join().unwrap()),
    )
    .await
    .unwrap();
    assert!(a.close_reason().is_none(), "the owner's session goes on");
    assert!(b.close_reason().is_none(), "the holder's session goes on");
}

/// M1-13-3: an object reference relayed to a far end that did not declare
/// `objects`. The session protocol has no such capability, and the relay
/// sends the reference anyway; what happens next is the far end's decoder's
/// doing:
///
/// - one that refuses the value for that call only reads the reference and
///   answers with a `throw`: the call fails, both sessions go on;
/// - one that ends its session on the reference (as Node's `receive` did
///   before #687) leaves this side's call failing with `peer-gone`; the
///   session that ends is the far end's, and the owner's session goes on.
///
/// The object itself never leaves its owner.
/// rule: M1-13-3
#[tokio::test(flavor = "multi_thread")]
async fn an_object_relayed_to_a_far_end_without_objects_is_still_sent() {
    /// A session whose far end owns object 7.
    fn owner() -> (Connection, std::thread::JoinHandle<Channel>) {
        let (a, mut ra) = session(Arc::new(Nothing));
        greet(&mut ra, "a", &[]);
        let far = std::thread::spawn(move || {
            let call = read(&mut ra);
            send(
                &mut ra,
                json!({ "op": "return", "id": frame_id(&call), "value": {
                    "type": "list",
                    "value": [{ "type": "reference", "value": {
                        "id": 7, "home": false, "kind": "object", "origin": [],
                    } }],
                } }),
            );
            // Calls after the relay: the session is alive. A release may
            // arrive first when the far end's session ended and its relay
            // went with it.
            loop {
                let frame = read(&mut ra);
                if frame["op"] == json!("release") {
                    assert_eq!(frame["reference"], json!(7), "{frame}");
                    continue;
                }
                assert_eq!(frame["op"], json!("invoke"), "{frame}");
                send(
                    &mut ra,
                    json!({ "op": "return", "id": frame_id(&frame), "value": { "type": "data", "value": "alive" } }),
                );
                break;
            }
            ra
        });
        (a, far)
    }
    fn object_of(a: &Connection) -> Value {
        a.invoke("svc", "give", Value::Undefined)
            .unwrap()
            .list()
            .unwrap()
            .remove(0)
    }

    // The far end refuses the value for its call only.
    let (a, a_far) = owner();
    let (b, rb) = session(Arc::new(Nothing));
    let mut rb = rb;
    greet(&mut rb, "b", &[]);
    a.ready().await.unwrap();
    b.ready().await.unwrap();
    let b_far = std::thread::spawn(move || {
        let call = read(&mut rb);
        let reference = call["args"]["value"][0]["value"].clone();
        assert_eq!(reference["kind"], json!("object"), "{call}");
        assert_eq!(reference["home"], json!(false), "{call}");
        send(
            &mut rb,
            json!({ "op": "throw", "id": frame_id(&call),
                    "error": { "name": "TypeError", "message": "invalid reference" } }),
        );
        // The next call reaches the far end, and the session goes on.
        let call = read(&mut rb);
        assert_eq!(
            call["args"],
            json!({ "type": "data", "value": 1 }),
            "{call}"
        );
        send(
            &mut rb,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "after" } }),
        );
        rb
    });
    let object = object_of(&a);
    let failed = b
        .invoke("svc", "take", Value::List(vec![object.clone()]))
        .expect_err("the far end refused the reference");
    match failed {
        Error::Remote { name, message, .. } => {
            assert_eq!(
                (name.as_str(), message.as_str()),
                ("TypeError", "invalid reference")
            )
        }
        other => panic!("the far end's error expected, got {other:?}"),
    }
    drop(object);
    let reply = b.invoke("svc", "plain", Value::Data(json!(1)));
    assert_eq!(reply.unwrap().json().unwrap(), json!("after"));
    assert!(b.close_reason().is_none(), "the far end's session goes on");
    assert!(a.close_reason().is_none(), "the owner's session goes on");
    let alive = a.invoke("svc", "still", Value::Undefined);
    assert_eq!(alive.unwrap().json().unwrap(), json!("alive"));
    let _rb = within(
        "b's far end",
        tokio::task::spawn_blocking(move || b_far.join().unwrap()),
    )
    .await
    .unwrap();
    let _ra = within(
        "a's far end",
        tokio::task::spawn_blocking(move || a_far.join().unwrap()),
    )
    .await
    .unwrap();
    drop(a);

    // The far end's decoder ends its session on the reference.
    let (a, a_far) = owner();
    let (b, rb) = session(Arc::new(Nothing));
    let mut rb = rb;
    greet(&mut rb, "b", &[]);
    a.ready().await.unwrap();
    b.ready().await.unwrap();
    let b_far = std::thread::spawn(move || {
        let call = read(&mut rb);
        let reference = call["args"]["value"][0]["value"].clone();
        assert_eq!(reference["kind"], json!("object"), "{call}");
        // The decoder cannot take it: its session ends here.
        drop(rb);
    });
    let object = object_of(&a);
    let failed = b.invoke("svc", "take", Value::List(vec![object]));
    let failed = failed.expect_err("the far end's session ended");
    assert_eq!(failed.code(), Some("peer-gone"), "{failed}");
    assert!(
        matches!(failed, Error::Transport(_)),
        "the call fails as the far end's session ends: {failed}"
    );
    within("the far end's session's end", b.closed()).await;
    assert!(b.close_reason().is_some(), "the far end's session ended");
    assert!(a.close_reason().is_none(), "the owner's session goes on");
    // A call on the owner's session is answered: the upstream session is
    // unaffected.
    let alive = a.invoke("svc", "still", Value::Undefined);
    assert_eq!(alive.unwrap().json().unwrap(), json!("alive"));
    within(
        "b's far end",
        tokio::task::spawn_blocking(move || b_far.join().unwrap()),
    )
    .await
    .unwrap();
    let _ra = within(
        "a's far end",
        tokio::task::spawn_blocking(move || a_far.join().unwrap()),
    )
    .await
    .unwrap();
}

// ── Third batch: proxies, names, frames (#639) ───────────────────────────

/// A far end that records the `target` and `method` of every call it gets.
struct Recording {
    seen: Arc<Mutex<Vec<(String, String)>>>,
}
impl Dispatch for Recording {
    fn invoke(&self, _: &Connection, target: &str, method: &str, _: Value) -> Reply {
        self.seen
            .lock()
            .unwrap()
            .push((target.to_owned(), method.to_owned()));
        Ok(Value::Data(json!("ok")))
    }
}

/// One reference of `kind`, exported by the far end as its reference `id`.
fn reference(id: u64, kind: &str) -> Json {
    json!({ "type": "reference", "value": {
        "id": id, "home": false, "kind": kind, "origin": [],
    } })
}

/// Wait for the far end's thread, so its asserts fail this test; what it
/// returns stays with the caller, which keeps the channel open.
async fn far_end<T: Send + 'static>(handle: std::thread::JoinHandle<T>) -> T {
    within(
        "the far end",
        tokio::task::spawn_blocking(move || handle.join().unwrap()),
    )
    .await
    .unwrap()
}

/// A live object at the far end stays there (SES-1-1): an object reference
/// arrives as a proxy, its methods and its live properties are asked of the
/// owner in `call` and `get` frames, and the object itself never crosses.
/// rule: SES-1-1
#[tokio::test(flavor = "multi_thread")]
async fn an_object_reference_from_the_far_end_is_a_proxy() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list",
                "value": [reference(7, "object")],
            } }),
        );
        // The method call: the reference's id, not a serialized object.
        let call = read(&mut far);
        assert_eq!(call["op"], json!("call"), "{call}");
        assert_eq!(call["reference"], json!(7), "{call}");
        assert_eq!(call["method"], json!("greet"), "{call}");
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [{ "type": "data", "value": "Oslo" }] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "hello" } }),
        );
        // The property read: every read is live.
        let read_back = read(&mut far);
        assert_eq!(read_back["op"], json!("get"), "{read_back}");
        assert_eq!(read_back["reference"], json!(7), "{read_back}");
        assert_eq!(read_back["property"], json!("name"), "{read_back}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&read_back), "value": { "type": "data", "value": "cache" } }),
        );
        // Holding a proxy is holding the owner's reference.
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        far
    });
    let object = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    assert!(object.is_object(), "an object reference is a proxy");
    assert_eq!(
        object
            .call_method("greet", Value::List(vec![Value::Data(json!("Oslo"))]))
            .unwrap()
            .json()
            .unwrap(),
        json!("hello")
    );
    assert_eq!(object.get("name").unwrap().json().unwrap(), json!("cache"));
    drop(object);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// References in a return value are live (SES-1-2): a returned function is
/// called with a `call` frame and a returned future is awaited with `await`,
/// both back in the session that returned them.
///
/// A reference in an event payload is the events side of the same rule
/// (`events.expected.json`), not this file's.
/// rule: SES-1-2
#[tokio::test(flavor = "multi_thread")]
async fn references_in_a_return_value_are_live() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list",
                "value": [reference(8, "function"), reference(9, "future")],
            } }),
        );
        // The function: a `call` with no method.
        let call = read(&mut far);
        assert_eq!(call["op"], json!("call"), "{call}");
        assert_eq!(call["reference"], json!(8), "{call}");
        assert_eq!(call.get("method"), None, "{call}");
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "called" } }),
        );
        // The future: an `await` of its reference.
        let awaited = read(&mut far);
        assert_eq!(awaited["op"], json!("await"), "{awaited}");
        assert_eq!(awaited["reference"], json!(9), "{awaited}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&awaited), "value": { "type": "data", "value": "settled" } }),
        );
        for expected in [8, 9] {
            assert_eq!(
                read(&mut far),
                json!({ "op": "release", "reference": expected, "count": 1 })
            );
        }
        far
    });
    let values = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap();
    let mut values = values.into_iter();
    let function = values.next().unwrap().reference().unwrap();
    let future = values.next().unwrap().reference().unwrap();
    assert!(function.is_function(), "a function reference");
    assert!(future.is_future(), "a future reference");
    assert_eq!(
        function
            .call(Value::List(Vec::new()))
            .unwrap()
            .json()
            .unwrap(),
        json!("called")
    );
    assert_eq!(future.wait().unwrap().json().unwrap(), json!("settled"));
    drop(function);
    drop(future);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A live object is handed over as a reference that keeps its identity
/// (SES-2-2): twice the same exported object is one proxy, no body of it
/// crosses, and both grants are released with one counted `release`.
/// rule: SES-2-2
#[tokio::test(flavor = "multi_thread")]
async fn the_same_object_twice_is_one_proxy() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        for _ in 0..2 {
            let call = read(&mut far);
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&call), "value": {
                    "type": "list",
                    "value": [reference(7, "object")],
                } }),
            );
        }
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 2 })
        );
        far
    });
    let first = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    let second = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    assert_eq!(first, second, "one proxy for one exported object");
    drop(first);
    drop(second);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A proxy answers nothing itself (SES-2-7): every property name, the ones
/// a prototype chain would answer included, is a live read at the owner, and
/// there is no frame that would assign a field or enumerate a proxy.
/// rule: SES-2-7
#[tokio::test(flavor = "multi_thread")]
async fn a_proxy_answers_no_property_locally() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list",
                "value": [reference(7, "object")],
            } }),
        );
        for property in ["constructor", "__proto__", "hasOwnProperty", "toString"] {
            let read_back = read(&mut far);
            assert_eq!(read_back["op"], json!("get"), "{read_back}");
            assert_eq!(read_back["property"], json!(property), "{read_back}");
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&read_back), "value": {
                    "type": "data", "value": format!("of {property}"),
                } }),
            );
        }
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        far
    });
    let object = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    for property in ["constructor", "__proto__", "hasOwnProperty", "toString"] {
        assert_eq!(
            object.get(property).unwrap().json().unwrap(),
            json!(format!("of {property}")),
            "{property} is the owner's to answer"
        );
    }
    drop(object);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");

    // A field assignment and an enumeration are not frames at all: an `op`
    // the protocol does not have ends the session.
    for op in ["set", "keys"] {
        unreadable(
            json!({ "op": op, "id": "node:1", "reference": 7, "property": "name",
                    "value": { "type": "data", "value": 1 } }),
            &format!("unknown variant `{op}`"),
        )
        .await;
    }
}

/// A method or a property is one name (SES-12-8): a name with dots in it
/// crosses as it is — a path is never split off it, in either direction.
/// rule: SES-12-8
#[tokio::test(flavor = "multi_thread")]
async fn a_method_or_property_name_is_one_name() {
    // This side calls a proxy: the dotted name goes as one `method` and one
    // `property`.
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list",
                "value": [reference(7, "object")],
            } }),
        );
        let call = read(&mut far);
        assert_eq!(call["op"], json!("call"), "{call}");
        assert_eq!(call["method"], json!("inner.method"), "{call}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "called" } }),
        );
        let read_back = read(&mut far);
        assert_eq!(read_back["op"], json!("get"), "{read_back}");
        assert_eq!(
            read_back["property"],
            json!("inner.property"),
            "{read_back}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&read_back), "value": { "type": "data", "value": "read" } }),
        );
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        far
    });
    let object = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    assert_eq!(
        object
            .call_method("inner.method", Value::List(Vec::new()))
            .unwrap()
            .json()
            .unwrap(),
        json!("called")
    );
    assert_eq!(
        object.get("inner.property").unwrap().json().unwrap(),
        json!("read")
    );
    drop(object);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// The far end's dotted target and method reach this side's dispatch as the
/// one name each they are: no path is resolved from them.
/// rule: SES-12-8
#[tokio::test(flavor = "multi_thread")]
async fn a_dotted_target_is_one_name_at_the_far_ends_call() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (peer, mut far) = session_with(
        Arc::new(Recording { seen: seen.clone() }),
        Endpoint::rust(id("main")),
    );
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    send(
        &mut far,
        invoke(
            "node:1",
            "svc.inner",
            "inner.method",
            one(&json!({ "type": "data", "value": 1 })),
        ),
    );
    assert_eq!(
        read(&mut far),
        json!({ "id": "node:1", "op": "return", "value": { "type": "data", "value": "ok" } })
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![("svc.inner".to_owned(), "inner.method".to_owned())],
        "one target, one method"
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A reference outlives its session as an error, not as a gate (SES-7.2-5):
/// an operation on a held reference after the session ended fails with the
/// end of that session.
/// rule: SES-7.2-5
#[tokio::test(flavor = "multi_thread")]
async fn a_reference_after_the_session_ends_fails_alone() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list",
                "value": [reference(7, "object"), reference(8, "function")],
            } }),
        );
        // The far end goes: its session ends.
        drop(far);
    });
    let mut held = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .into_iter();
    let object = held.next().unwrap().reference().unwrap();
    let function = held.next().unwrap().reference().unwrap();
    within("the session's end", peer.closed()).await;
    assert!(peer.close_reason().is_some(), "the far end went");

    // The object's owner is gone: the call says so, by code and by data.
    let object_error = object
        .call_method("greet", Value::List(Vec::new()))
        .expect_err("the owner's session ended");
    assert_eq!(object_error.code(), Some("peer-gone"), "{object_error}");
    assert_eq!(
        object_error.data(),
        Some(json!({ "runtime": "node", "reason": "peer disconnected" })),
        "{object_error}"
    );
    assert_eq!(
        object_error.to_string(),
        "a call on an object from node failed: runtime node disconnected",
        "the error names where the object came from"
    );
    let read_error = object.get("name").expect_err("the owner's session ended");
    assert!(
        matches!(&read_error, Error::Transport(message)
            if message.contains("peer disconnected") || message.contains("connection closed")),
        "{read_error}"
    );
    // A function reference from the same session fails the same way.
    let function_error = function
        .call(Value::List(Vec::new()))
        .expect_err("the session that returned it ended");
    assert!(
        matches!(&function_error, Error::Transport(message)
            if message.contains("peer disconnected") || message.contains("connection closed")),
        "{function_error}"
    );
    far_end(end).await;
}

/// An `origin` entry carries a session tag that is only `s` and digits
/// (TAG-5-1): a tagged entry of another session is accepted as it is — the
/// format needs no protocol version, and a tagged entry is not taken for a
/// call of this session.
///
/// This closes the Rust column; what each runtime's own origin validation
/// does with a new tag (Node, Python, Go, Bun) stays open.
/// rule: TAG-5-1
#[tokio::test(flavor = "multi_thread")]
async fn origin_entries_with_a_new_tag_are_accepted() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        // An object whose chain runs through other sessions: an older
        // counter tag and a newer long one.
        let call = read(&mut far);
        let mut answered = reference(7, "object");
        answered["value"]["origin"] = json!(["s1/node:3", "s0123456789012347/node:4"]);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list", "value": [answered],
            } }),
        );
        let call = read(&mut far);
        assert_eq!(call["op"], json!("call"), "{call}");
        assert_eq!(call["reference"], json!(7), "{call}");
        assert_eq!(call["path"], json!([]), "{call}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "reached" } }),
        );
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        // A call whose chain carries tagged entries of other sessions is
        // dispatched here, not taken for one of this session's calls.
        let mut called = invoke(
            "node:2",
            "svc",
            "echo",
            one(&json!({ "type": "data", "value": "chained" })),
        );
        called["path"] = json!(["s1/node:3", "s0123456789012347/node:4"]);
        send(&mut far, called);
        assert_eq!(
            read(&mut far),
            json!({ "id": "node:2", "op": "return",
                    "value": { "type": "list", "value": [{ "type": "data", "value": "chained" }] } })
        );
        far
    });
    let object = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    assert!(object.is_object(), "the tagged origin does not refuse it");
    assert_eq!(
        object
            .call_method("greet", Value::List(Vec::new()))
            .unwrap()
            .json()
            .unwrap(),
        json!("reached")
    );
    drop(object);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A synchronous call keeps its session able to take calls the other way
/// (CHN-2-3): while this side waits for a reply, a reverse call arrives,
/// runs and is answered on the same channel before the reply.
/// rule: CHN-2-3
#[tokio::test(flavor = "multi_thread")]
async fn a_reverse_call_is_answered_while_a_synchronous_call_waits() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        assert_eq!(call["sync"], json!(true), "the caller waits: {call}");
        // Asked before the reply to the waiting call: the answer comes back
        // while that call still waits.
        send(
            &mut far,
            invoke(
                "node:1",
                "svc",
                "echo",
                one(&json!({ "type": "data", "value": "reverse" })),
            ),
        );
        assert_eq!(
            read(&mut far),
            json!({ "id": "node:1", "op": "return",
                    "value": { "type": "list", "value": [{ "type": "data", "value": "reverse" }] } })
        );
        // Only then the first call's reply.
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "slow" } }),
        );
        far
    });
    let answering = peer.clone();
    let call =
        tokio::task::spawn_blocking(move || answering.invoke("svc", "slow", Value::Undefined));
    let far_thread = tokio::task::spawn_blocking(move || end.join().unwrap());
    let (answer, far) = within("the waiting call and the far end", async move {
        tokio::join!(call, far_thread)
    })
    .await;
    let _far = far.unwrap();
    assert_eq!(
        answer.unwrap().unwrap().json().unwrap(),
        json!("slow"),
        "the waiting call's own reply"
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// Unknown fields do not end a session (CHN-4-11): a frame with a call's id
/// fails only that call with code `bad-frame` and its `{op, fields}` data; a
/// greeting with a field this side does not know fails the handshake;
/// `release` and `cancel` ignore the extra fields.
/// rule: CHN-4-11
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_field_fails_only_its_call() {
    // A call frame from the far end: its call fails, and only it.
    {
        let (peer, mut far) = session(Arc::new(Echo));
        greet(&mut far, "node", &[]);
        peer.ready().await.unwrap();
        let mut frame = invoke(
            "node:1",
            "svc",
            "echo",
            one(&json!({ "type": "data", "value": 1 })),
        );
        frame["extra"] = json!(1);
        send(&mut far, frame);
        let (name, message, code, data) = thrown(&read(&mut far));
        assert_eq!(name, "BindingError");
        assert_eq!(message, "unknown field \"extra\" in invoke");
        assert_eq!(code.as_deref(), Some("bad-frame"));
        assert_eq!(data, Some(json!({ "op": "invoke", "fields": ["extra"] })));
        // The session goes on.
        let reply = echo(
            &mut far,
            "node:2",
            &json!({ "type": "data", "value": "after" }),
        );
        assert_eq!(
            reply["value"],
            one(&json!({ "type": "data", "value": "after" }))
        );
        assert!(peer.close_reason().is_none(), "the session goes on");
    }
    // A reply with an extra field fails the call waiting for it.
    {
        let (peer, mut far) = session(Arc::new(Nothing));
        greet(&mut far, "node", &[]);
        peer.ready().await.unwrap();
        let end = std::thread::spawn(move || {
            let call = read(&mut far);
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&call),
                        "value": { "type": "data", "value": "late" }, "extra": 1 }),
            );
            // The next call is answered as usual.
            let call = read(&mut far);
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "after" } }),
            );
            far
        });
        let error = peer
            .invoke("svc", "answer", Value::Undefined)
            .expect_err("the reply named a field this side does not know");
        assert_eq!(error.code(), Some("bad-frame"), "{error}");
        assert_eq!(
            error.data(),
            Some(json!({ "op": "return", "fields": ["extra"] })),
            "{error}"
        );
        assert_eq!(
            peer.invoke("svc", "answer", Value::Undefined)
                .unwrap()
                .json()
                .unwrap(),
            json!("after"),
            "the session goes on"
        );
        let _far = far_end(end).await;
        assert!(peer.close_reason().is_none(), "the session goes on");
    }
    // `release` and `cancel` ignore a field they do not know.
    {
        let (peer, mut far) = session(Arc::new(Giving));
        greet(&mut far, "node", &[]);
        peer.ready().await.unwrap();
        send(
            &mut far,
            invoke("node:1", "svc", "give", json!({ "type": "undefined" })),
        );
        let reply = read(&mut far);
        let exported = reply["value"]["value"]["id"]
            .as_u64()
            .expect("a reference id");
        send(
            &mut far,
            json!({ "op": "release", "reference": exported, "count": 1, "extra": true }),
        );
        send(
            &mut far,
            json!({ "op": "cancel", "id": "node:9", "extra": true }),
        );
        send(
            &mut far,
            invoke("node:2", "svc", "give", json!({ "type": "undefined" })),
        );
        let reply = read(&mut far);
        assert_eq!(reply["op"], json!("return"), "{reply}");
        assert_eq!(
            reply["value"]["value"]["kind"],
            json!("function"),
            "{reply}"
        );
        assert!(peer.close_reason().is_none(), "the session goes on");
    }
    // A greeting with a field this side does not know fails the handshake.
    {
        let (peer, mut far) = session(Arc::new(Echo));
        let hello = read(&mut far);
        assert_eq!(hello["op"], json!("hello"), "{hello}");
        send(
            &mut far,
            json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "node",
                    "capabilities": [], "extra": 1 }),
        );
        match peer.ready().await {
            Err(Error::Handshake(rutis_bridge::session::Handshake::Incompatible(message))) => {
                assert_eq!(message, "unknown field \"extra\" in hello")
            }
            other => panic!("a refused handshake expected, got {other:?}"),
        }
    }
}

/// A session speaks on its own logical channel (CHN-0-1): no frame carries a
/// channel or a session identifier — each session is one channel, and what
/// one session's far end sends is answered there alone; a session's end
/// stays on its own channel.
///
/// The rule's other half, several sessions over one shared physical
/// connection (CHN-5-4), is not shown: this crate has no multiplexing
/// adapter to share one.
/// rule: CHN-0-1
#[tokio::test(flavor = "multi_thread")]
async fn a_session_speaks_on_its_own_channel() {
    let (first_session, mut first) = session(Arc::new(Echo));
    let (second_session, mut second) = session(Arc::new(Echo));
    greet(&mut first, "node", &[]);
    greet(&mut second, "node", &[]);
    first_session.ready().await.unwrap();
    second_session.ready().await.unwrap();

    // The same frame on both channels: each session answers its own.
    let frame = invoke(
        "node:1",
        "svc",
        "echo",
        one(&json!({ "type": "data", "value": "both" })),
    );
    let mut fields: Vec<&str> = frame
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        vec!["args", "id", "method", "op", "path", "target"],
        "no channel or session identifier in a call frame"
    );
    send(&mut first, frame.clone());
    send(&mut second, frame.clone());
    let expected = json!({ "id": "node:1", "op": "return",
                           "value": { "type": "list", "value": [{ "type": "data", "value": "both" }] } });
    assert_eq!(read(&mut first), expected, "the first channel's answer");
    assert_eq!(read(&mut second), expected, "the second channel's answer");

    // Ending one session's channel ends that session alone.
    drop(first);
    within("the first session's end", first_session.closed()).await;
    assert!(
        first_session.close_reason().is_some(),
        "the first session ended"
    );
    send(
        &mut second,
        invoke(
            "node:2",
            "svc",
            "echo",
            one(&json!({ "type": "data", "value": "still" })),
        ),
    );
    assert_eq!(
        read(&mut second),
        json!({ "id": "node:2", "op": "return",
                "value": { "type": "list", "value": [{ "type": "data", "value": "still" }] } })
    );
    assert!(
        second_session.close_reason().is_none(),
        "the second session goes on"
    );
}

// ── Fourth batch: the runtime contract, callbacks and errors (#639) ──────

/// A far end that hands out a function of the host's (`svc.give`) and echoes
/// its arguments (`svc.echo`).
struct Offering;

impl Dispatch for Offering {
    fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
        match (target, method) {
            ("svc", "give") => Ok(Value::callback(|argument| {
                Ok(Value::Data(json!(argument.list()?.len())))
            })),
            ("svc", "echo") => Ok(args),
            _ => Err(Error::value(format!("no method {method}"))),
        }
    }
}

/// A host service that calls the function the far end passed it, and waits
/// for its promise before answering.
struct Applying;

impl Dispatch for Applying {
    fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
        match (target, method) {
            ("svc", "apply") => {
                let mut arguments = args.list()?.into_iter();
                let callback = arguments.next().unwrap_or(Value::Undefined).reference()?;
                let argument = arguments.next().unwrap_or(Value::Undefined);
                // As any host service that calls a plugin back: not on the
                // reader's thread, and the promise, if any, is awaited.
                Ok(Value::future(async move {
                    let called = callback.call_async(Value::List(vec![argument])).await?;
                    settle(called).await
                }))
            }
            _ => Err(Error::value(format!("no method {method}"))),
        }
    }
}

/// A host service that refuses with a code and data of its own.
struct Failing;

impl Dispatch for Failing {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        Err(Error::binding(
            "kaboom",
            "the host failed".into(),
            Some(json!({ "why": "here" })),
        ))
    }
}

/// The next frame of `op`, checking the `release`s met before it: a test
/// that holds references may see one at any point.
fn expect(far: &mut Channel, op: &str, releases: &mut Vec<u64>) -> Json {
    loop {
        let frame = read(far);
        if frame["op"] == json!(op) {
            return frame;
        }
        assert_eq!(frame["op"], json!("release"), "{frame}");
        assert_eq!(frame["count"], json!(1), "{frame}");
        releases.push(frame["reference"].as_u64().expect("a reference id"));
    }
}

/// A session reads and writes on threads of its own (CHN-2-5): a
/// synchronous call made inside a `current_thread` runtime — its only thread
/// blocked until the reply — is still written and answered, and so is the
/// next one.
///
/// The other implementations' sides of the rule (Node's I/O worker while its
/// main thread waits) stay open.
/// rule: CHN-2-5
#[tokio::test(flavor = "multi_thread")]
async fn the_channel_advances_while_the_callers_runtime_is_blocked() {
    within(
        "calls from a blocked current-thread runtime",
        tokio::task::spawn_blocking(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let (peer, mut far) = runtime.block_on(async { session(Arc::new(Echo)) });
            greet(&mut far, "node", &[]);
            runtime.block_on(peer.ready()).unwrap();
            let far = std::thread::spawn(move || {
                for text in ["one", "two"] {
                    let call = read(&mut far);
                    assert_eq!(
                        call["args"],
                        one(&json!({ "type": "data", "value": text })),
                        "{call}"
                    );
                    send(
                        &mut far,
                        json!({ "op": "return", "id": frame_id(&call),
                                "value": { "type": "data", "value": text } }),
                    );
                }
                far
            });
            // The runtime's only thread waits for the reply here: the
            // session's reader and writer are not tasks of its runtime.
            runtime.block_on(async {
                for (n, text) in ["one", "two"].into_iter().enumerate() {
                    let reply = peer
                        .invoke("svc", "echo", Value::List(vec![Value::Data(json!(text))]))
                        .unwrap();
                    assert_eq!(reply.json().unwrap(), json!(text), "call {}", n + 1);
                }
            });
            let _far = far.join().unwrap();
        }),
    )
    .await
    .unwrap();
}

/// A `bigint` stays a `bigint` at every size (VAL-2-8): one within
/// ±(2^53−1) crosses as `{"type":"bigint"}` and comes back a `BigInt`, not
/// a JSON number, so sending it on keeps the tag; a JSON number stays one.
///
/// Python's and Go's own sides of the rule (an `int` subclass, a
/// `*big.Int`) stay open.
/// rule: VAL-2-8
#[tokio::test(flavor = "multi_thread")]
async fn a_small_bigint_keeps_its_tag() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    // What arrives tagged is a BigInt here, and goes on tagged.
    for (n, text) in ["5", "-7", "0", "9007199254740991"].into_iter().enumerate() {
        accepted(
            &mut far,
            &format!("node:{}", n + 1),
            &json!({ "type": "bigint", "value": text }),
        );
    }
    // What goes out as a BigInt crosses tagged, whatever its size; a JSON
    // number crosses as data.
    let end = std::thread::spawn(move || {
        for text in ["5", "-7", "0"] {
            let call = read(&mut far);
            assert_eq!(
                call["args"],
                one(&json!({ "type": "bigint", "value": text })),
                "{call}"
            );
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
            );
        }
        let call = read(&mut far);
        assert_eq!(
            call["args"],
            one(&json!({ "type": "data", "value": 5 })),
            "a JSON number is no bigint: {call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": { "type": "data", "value": "ok" } }),
        );
        far
    });
    for text in ["5", "-7", "0"] {
        let value = Value::BigInt(text.parse::<BigInt>().unwrap());
        assert_eq!(
            peer.invoke("svc", "echo", Value::List(vec![value]))
                .unwrap()
                .json()
                .unwrap(),
            json!("ok"),
            "a small bigint sent"
        );
    }
    assert_eq!(
        peer.invoke("svc", "echo", Value::List(vec![Value::Data(json!(5))]))
            .unwrap()
            .json()
            .unwrap(),
        json!("ok")
    );
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A property of a live object is read live (MNT-3-6): every read is a
/// `get` frame naming the reference and the property, and the value is the
/// owner's at that moment, never a copy kept here.
/// rule: MNT-3-6
#[tokio::test(flavor = "multi_thread")]
async fn a_property_read_is_live() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list", "value": [reference(7, "object")],
            } }),
        );
        for text in ["first", "second"] {
            let read_back = read(&mut far);
            assert_eq!(read_back["op"], json!("get"), "{read_back}");
            assert!(
                read_back["id"].as_str().unwrap().starts_with("main:"),
                "{read_back}"
            );
            assert!(read_back["path"].is_array(), "{read_back}");
            assert_eq!(read_back["reference"], json!(7), "{read_back}");
            assert_eq!(read_back["property"], json!("name"), "{read_back}");
            send(
                &mut far,
                json!({ "op": "return", "id": frame_id(&read_back),
                        "value": { "type": "data", "value": text } }),
            );
        }
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        far
    });
    let object = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    assert_eq!(object.get("name").unwrap().json().unwrap(), json!("first"));
    assert_eq!(
        object.get("name").unwrap().json().unwrap(),
        json!("second"),
        "a second read asks the owner again"
    );
    drop(object);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A host service may answer with `rpc::Value::callback` (MNT-5-4): the far
/// end gets a function reference, calls it with its own arguments, and the
/// closure here runs for each call; the session goes on after its release.
/// rule: MNT-5-4
#[tokio::test(flavor = "multi_thread")]
async fn a_host_returned_callback_is_callable_by_the_far_end() {
    let (peer, mut far) = session(Arc::new(Offering));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    send(
        &mut far,
        invoke("node:1", "svc", "give", json!({ "type": "undefined" })),
    );
    let reply = read(&mut far);
    let value = &reply["value"]["value"];
    assert_eq!(value["kind"], json!("function"), "{reply}");
    assert_eq!(value["home"], json!(false), "{reply}");
    let reference = value["id"].as_u64().expect("a reference id");
    for (id, arguments, answer) in [
        ("node:2", json!([{ "type": "data", "value": 1 }]), json!(1)),
        (
            "node:3",
            json!([
                { "type": "data", "value": 1 },
                { "type": "data", "value": 2 }
            ]),
            json!(2),
        ),
    ] {
        send(
            &mut far,
            json!({ "op": "call", "id": id, "path": [], "reference": reference,
                    "args": { "type": "list", "value": arguments } }),
        );
        assert_eq!(
            read(&mut far),
            json!({ "id": id, "op": "return", "value": { "type": "data", "value": answer } }),
            "the host's callback ran with the far end's arguments"
        );
    }
    send(
        &mut far,
        json!({ "op": "release", "reference": reference, "count": 1 }),
    );
    send(
        &mut far,
        invoke(
            "node:4",
            "svc",
            "echo",
            one(&json!({ "type": "data", "value": "after" })),
        ),
    );
    assert_eq!(
        read(&mut far),
        json!({ "id": "node:4", "op": "return",
                "value": { "type": "list", "value": [{ "type": "data", "value": "after" }] } })
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A function the far end returns is called synchronously by `call` and
/// awaited by `call_async` (MNT-2-8n): the first frame says `sync: true`,
/// the second is asynchronous (no `sync` field) and its promise is awaited,
/// and both reach the far end that returned the reference.
/// rule: MNT-2-8n
#[tokio::test(flavor = "multi_thread")]
async fn a_returned_function_is_called_sync_and_its_promise_awaited() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let mut releases = Vec::new();
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list", "value": [reference(7, "function")],
            } }),
        );
        // `call`: the caller waits for this answer.
        let called = expect(&mut far, "call", &mut releases);
        assert_eq!(called["reference"], json!(7), "{called}");
        assert_eq!(called["sync"], json!(true), "a synchronous call: {called}");
        assert_eq!(
            called["args"],
            json!({ "type": "list", "value": [{ "type": "data", "value": "sync" }] }),
            "{called}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&called), "value": { "type": "data", "value": "done" } }),
        );
        // `call_async`: the answer is a promise, awaited by reference.
        let called = expect(&mut far, "call", &mut releases);
        assert_eq!(
            called.get("sync"),
            None,
            "an asynchronous call waits for a promise, not a frame: {called}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&called), "value": reference(8, "future") }),
        );
        let awaited = expect(&mut far, "await", &mut releases);
        assert_eq!(awaited["reference"], json!(8), "{awaited}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&awaited),
                    "value": { "type": "data", "value": "settled" } }),
        );
        far
    });
    let function = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    assert!(function.is_function(), "a function reference");
    assert_eq!(
        function
            .call(Value::List(vec![Value::Data(json!("sync"))]))
            .unwrap()
            .json()
            .unwrap(),
        json!("done")
    );
    let answer = function
        .call_async(Value::List(vec![Value::Data(json!("async"))]))
        .await
        .unwrap();
    let Value::Reference(promise) = &answer else {
        panic!("the function answered a promise: {answer:?}");
    };
    assert!(promise.is_future(), "a future reference");
    assert_eq!(
        settle(answer).await.unwrap().json().unwrap(),
        json!("settled"),
        "the promise's value"
    );
    drop(function);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A function passed by reference is called by the native side during the
/// call (bridge/exported-services c), which waits for its promise before it
/// answers; a callback that throws fails the call it was called in, and the
/// session goes on.
/// rule: bridge/exported-services c
#[tokio::test(flavor = "multi_thread")]
async fn a_passed_function_is_called_by_the_native_side() {
    let (peer, mut far) = session(Arc::new(Applying));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let mut releases = Vec::new();
        // A promise the host's answer is awaited through.
        send(
            &mut far,
            invoke(
                "node:1",
                "svc",
                "apply",
                json!({ "type": "list", "value": [
                    reference(7, "function"),
                    { "type": "data", "value": "x" },
                ] }),
            ),
        );
        let answer = expect(&mut far, "return", &mut releases);
        let promise = answer["value"]["value"]["id"]
            .as_u64()
            .expect("the host's promise");
        send(
            &mut far,
            json!({ "op": "await", "id": "node:2", "path": [], "reference": promise }),
        );
        // The host calls the function while the call waits.
        let called = expect(&mut far, "call", &mut releases);
        assert_eq!(called["reference"], json!(7), "{called}");
        assert_eq!(
            called["args"],
            json!({ "type": "list", "value": [{ "type": "data", "value": "x" }] }),
            "{called}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&called),
                    "value": { "type": "data", "value": "during" } }),
        );
        let done = expect(&mut far, "return", &mut releases);
        assert_eq!(done["id"], json!("node:2"), "{done}");
        assert_eq!(
            done["value"],
            json!({ "type": "data", "value": "during" }),
            "the host answered after the callback's promise settled: {done}"
        );
        send(
            &mut far,
            json!({ "op": "release", "reference": promise, "count": 1 }),
        );
        // A callback that throws fails the call it was called in.
        send(
            &mut far,
            invoke(
                "node:3",
                "svc",
                "apply",
                json!({ "type": "list", "value": [
                    reference(8, "function"),
                    { "type": "data", "value": "y" },
                ] }),
            ),
        );
        let answer = expect(&mut far, "return", &mut releases);
        let promise = answer["value"]["value"]["id"]
            .as_u64()
            .expect("the host's promise");
        send(
            &mut far,
            json!({ "op": "await", "id": "node:4", "path": [], "reference": promise }),
        );
        let called = expect(&mut far, "call", &mut releases);
        assert_eq!(called["reference"], json!(8), "{called}");
        send(
            &mut far,
            json!({ "op": "throw", "id": frame_id(&called),
                    "error": { "name": "Boom", "message": "no" } }),
        );
        let failed = expect(&mut far, "throw", &mut releases);
        assert_eq!(failed["id"], json!("node:4"), "{failed}");
        assert_eq!(failed["error"]["name"], json!("Boom"), "{failed}");
        assert_eq!(failed["error"]["message"], json!("no"), "{failed}");
        send(
            &mut far,
            json!({ "op": "release", "reference": promise, "count": 1 }),
        );
        // The function references the far end passed: released here.
        for reference in [7, 8] {
            assert!(
                releases.contains(&reference),
                "{reference} was not released: {releases:?}"
            );
        }
        far
    });
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// An error's `code` and `data` cross with it both ways
/// (bridge/error-code-and-data a), and a service's `Err` reaches the far end
/// as a `throw` carrying them (bridge/exported-services b): the far end's
/// error graph is read here, and this side's error is written there.
/// rule: bridge/error-code-and-data a
/// rule: bridge/exported-services b
#[tokio::test(flavor = "multi_thread")]
async fn an_errors_code_and_data_cross_both_ways() {
    // The far end throws: its code and data are readable here.
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "throw", "id": frame_id(&call), "error": {
                "name": "Boom",
                "message": "the far end failed",
                "graph": {
                    "root": { "type": "reference", "value": 0 },
                    "nodes": [{
                        "type": "error",
                        "name": "Boom",
                        "message": "the far end failed",
                        "code": "kaboom",
                        "data": { "type": "data", "value": { "why": "because" } },
                    }],
                },
            } }),
        );
        far
    });
    let error = peer
        .invoke("svc", "echo", Value::Undefined)
        .expect_err("the far end failed");
    assert_eq!(error.code(), Some("kaboom"), "{error}");
    assert_eq!(error.data(), Some(json!({ "why": "because" })), "{error}");
    assert_eq!(
        error.to_string(),
        "Boom: the far end failed",
        "the name and message cross too"
    );
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");

    // This side throws: the far end reads its code and data.
    let (peer, mut far) = session(Arc::new(Failing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    send(
        &mut far,
        invoke("node:1", "svc", "echo", json!({ "type": "undefined" })),
    );
    let (name, message, code, data) = thrown(&read(&mut far));
    assert_eq!(name, "BindingError");
    assert_eq!(message, "the host failed");
    assert_eq!(code.as_deref(), Some("kaboom"));
    assert_eq!(data, Some(json!({ "why": "here" })));
    // Only that call failed: the session takes the next one.
    send(
        &mut far,
        invoke(
            "node:2",
            "svc",
            "echo",
            one(&json!({ "type": "data", "value": "after" })),
        ),
    );
    let (name, _, code, _) = thrown(&read(&mut far));
    assert_eq!(name, "BindingError");
    assert_eq!(code.as_deref(), Some("kaboom"));
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A runtime that answers the row and host contract as the test scripts it.
struct Scripted {
    mount: Json,
    schema: Json,
    schemas: Mutex<Vec<Json>>,
    mounts: Mutex<usize>,
    mount_args: Mutex<Vec<Json>>,
    loads: Mutex<Vec<Json>>,
    updates: Mutex<Vec<Json>>,
    unloads: Mutex<Vec<Json>>,
    hosts: Mutex<Vec<Json>>,
    withdraws: Mutex<Vec<Json>>,
    offers: Mutex<Vec<Json>>,
    calls: Mutex<Vec<(String, String)>>,
    sequence: Mutex<Vec<String>>,
}

impl Scripted {
    fn of(mount: Json, schema: Json) -> Arc<Self> {
        Arc::new(Self {
            mount,
            schema,
            schemas: Mutex::default(),
            loads: Mutex::default(),
            updates: Mutex::default(),
            unloads: Mutex::default(),
            hosts: Mutex::default(),
            withdraws: Mutex::default(),
            offers: Mutex::default(),
            calls: Mutex::default(),
            mounts: Mutex::default(),
            mount_args: Mutex::default(),
            sequence: Mutex::default(),
        })
    }

    fn new(schema: Json) -> Arc<Self> {
        Self::of(
            json!({ "services": {}, "features": ["rows.v2", "hosts"] }),
            schema,
        )
    }

    /// A runtime that reports `features` when it mounts.
    fn with_features(features: &[&str], schema: Json) -> Arc<Self> {
        Self::of(json!({ "services": {}, "features": features }), schema)
    }

    /// A runtime whose `mount` reply lists `plugin` as described by `schema`.
    fn listing(plugin: &str, schema: Json) -> Arc<Self> {
        Self::of(
            json!({ "services": {}, "features": ["rows.v2", "hosts"],
                    "plugins": { plugin: schema.clone() } }),
            schema,
        )
    }
}

impl Dispatch for Scripted {
    fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
        match (target, method) {
            ("", "mount") => {
                *self.mounts.lock().unwrap() += 1;
                self.mount_args.lock().unwrap().push(args.json()?);
                self.sequence.lock().unwrap().push("mount".into());
                Ok(Value::Data(self.mount.clone()))
            }
            ("", "link.offers") => {
                self.offers.lock().unwrap().push(args.json()?);
                Ok(Value::Undefined)
            }
            ("", "rows.schema") => {
                self.schemas.lock().unwrap().push(args.json()?);
                Ok(Value::Data(self.schema.clone()))
            }
            ("", "rows.load") => {
                self.loads.lock().unwrap().push(args.json()?);
                self.sequence.lock().unwrap().push("rows.load".into());
                Ok(Value::Undefined)
            }
            ("", "rows.update") => {
                self.updates.lock().unwrap().push(args.json()?);
                self.sequence.lock().unwrap().push("rows.update".into());
                Ok(Value::Undefined)
            }
            ("", "rows.unload") => {
                self.unloads.lock().unwrap().push(args.json()?);
                self.sequence.lock().unwrap().push("rows.unload".into());
                Ok(Value::Undefined)
            }
            ("", "hosts.provide") => {
                self.hosts.lock().unwrap().push(args.json()?);
                self.sequence.lock().unwrap().push("hosts.provide".into());
                Ok(Value::Undefined)
            }
            ("", "hosts.withdraw") => {
                self.withdraws.lock().unwrap().push(args.json()?);
                self.sequence.lock().unwrap().push("hosts.withdraw".into());
                Ok(Value::Undefined)
            }
            _ if target.is_empty() => Err(Error::value(format!("no method {method}"))),
            // A call into a row's service, at the handle the far end was
            // given: what a runtime does when one of its rows is called.
            _ => {
                self.calls
                    .lock()
                    .unwrap()
                    .push((target.to_owned(), method.to_owned()));
                match method {
                    "later" => Ok(Value::future(async { Ok(Value::Data(json!("later"))) })),
                    _ => Ok(Value::Data(json!("sunny"))),
                }
            }
        }
    }
}

/// The root, the process and the far end of a session where `runtime` is the
/// far end's row `rt`.
async fn runtime_rig(runtime: Arc<Scripted>) -> (Ctx, Arc<Process>, Connection) {
    let root = common::runtime::root();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let mut endpoint = Endpoint::rust(id("rt"));
    endpoint.capabilities.push("runtime".into());
    let far = Connection::open(theirs, runtime, endpoint).unwrap();
    let process = common::runtime::attached(&root, "rt", ours).await.unwrap();
    (root, process, far)
}

/// A host service that answers `today` and says what it was asked.
struct Today(Arc<Mutex<Vec<String>>>);

impl HostDispatch for Today {
    fn invoke(&self, method: &str, _: Value) -> Reply {
        self.0.lock().unwrap().push(method.to_owned());
        match method {
            "today" => Ok(Value::Data(json!("sunny"))),
            other => Err(Error::no_method("weather", other, vec!["today"])),
        }
    }
}

/// A row is loaded with the row contract's arguments (SYN-5-2): its key,
/// the entry the runtime itself interprets, its config, the isolate pairs,
/// the injected names, and the scope and id it is named by.
/// rule: SYN-5-2
#[tokio::test(flavor = "multi_thread")]
async fn a_row_is_loaded_with_the_row_contracts_arguments() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, _far) = runtime_rig(runtime.clone()).await;
    process
        .load_row(
            "chat",
            Path::new("plugin.js"),
            json!({ "room": "lobby" }),
            &[("weather".to_owned(), "tenant".to_owned())],
            &["db".to_owned()],
            &[Scope {
                id: "17".into(),
                group: "tenant".into(),
            }],
        )
        .await
        .unwrap();
    assert_eq!(
        *runtime.loads.lock().unwrap(),
        vec![json!([
            "chat",
            "plugin.js",
            { "room": "lobby" },
            [["weather", "tenant"]],
            ["db"],
            {},
            { "scope": [{ "id": "17", "group": "tenant" }], "id": "chat" }
        ])]
    );
}

/// The host reads all of what `rows.schema` reports a plugin declares
/// (M1-5.1-1, SYN-5-4, REM-4.2-2): its config schema, the services it
/// injects and uses optionally, the services and method shapes it provides,
/// its version, and whether it announces what it did not declare and is a
/// Cordis plugin.
/// rule: M1-5.1-1
/// rule: SYN-5-4
/// rule: REM-4.2-2
#[tokio::test(flavor = "multi_thread")]
async fn a_row_schema_reports_what_the_plugin_declares() {
    let schema = json!({
        "config": { "type": "object", "properties": { "room": { "type": "string" } } },
        "inject": ["db"],
        "optional": ["cache"],
        "provides": { "weather": { "today": "async", "now": "sync" } },
        "version": "1.2.3",
        "announces": true,
        "cordis": true,
    });
    let runtime = Scripted::new(schema.clone());
    let (_root, process, _far) = runtime_rig(runtime.clone()).await;
    let described = process.describe_row(Path::new("plugin.js")).await.unwrap();
    assert_eq!(
        *runtime.schemas.lock().unwrap(),
        vec![json!(["plugin.js"])],
        "the entry is what the runtime is asked about"
    );
    assert_eq!(described.config, Some(schema["config"].clone()));
    assert_eq!(described.inject, ["db"]);
    assert_eq!(described.optional, ["cache"]);
    assert_eq!(
        described.provides.get("weather"),
        Some(&json!({ "today": "async", "now": "sync" }))
    );
    assert_eq!(described.version.as_deref(), Some("1.2.3"));
    assert!(described.announces);
    assert!(described.cordis);
}

/// A runtime's `mount` reply lists the plugins it has (DIA-4-1), each
/// described as `rows.schema` describes it: the host reads the same fields
/// either way, and no device information travels in them.
/// rule: DIA-4-1
#[tokio::test(flavor = "multi_thread")]
async fn a_mount_lists_its_plugins_like_rows_schema() {
    let schema = json!({
        "config": { "type": "object" },
        "inject": ["db"],
        "provides": { "weather": { "today": "async" } },
        "version": "1.0.0",
    });
    let runtime = Scripted::listing("plugin.js", schema.clone());
    let (_root, process, _far) = runtime_rig(runtime).await;
    let listed = process
        .plugins()
        .expect("the mount listed its plugins")
        .get("plugin.js")
        .expect("the listed plugin");
    let described = process.describe_row(Path::new("plugin.js")).await.unwrap();
    assert_eq!(listed.config, described.config);
    assert_eq!(listed.inject, described.inject);
    assert_eq!(listed.optional, described.optional);
    assert_eq!(listed.provides, described.provides);
    assert_eq!(listed.version, described.version);
    assert_eq!(listed.announces, described.announces);
    assert_eq!(listed.cordis, described.cordis);
    assert_eq!(listed.config, Some(schema["config"].clone()));
}

/// A runtime's row calls a host service by name (M1-4-2): `host:<name>`
/// finds the provider leased now and calls it here, and a name with no
/// provider fails that call with `unavailable`; the session goes on. (A
/// provider in another session, forwarded to, stays open.)
/// rule: M1-4-2
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_calls_a_host_service_by_name() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let _lease = process
        .lease_host(
            "weather",
            Arc::new(Today(seen.clone())),
            Some(json!({ "today": "async" })),
        )
        .await
        .unwrap();
    assert_eq!(
        *runtime.hosts.lock().unwrap(),
        vec![json!(["weather", { "today": "async" }])],
        "the lease is announced with its method shapes"
    );
    let answer = far
        .invoke_async("host:weather", "today", Value::List(Vec::new()))
        .await
        .unwrap();
    assert_eq!(answer.json().unwrap(), json!("sunny"));
    assert_eq!(*seen.lock().unwrap(), ["today"]);
    // A name nobody provides fails only that call.
    let refused = far
        .invoke_async("host:missing", "today", Value::List(Vec::new()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("unavailable"), "{refused}");
    assert_eq!(
        refused.data(),
        Some(json!({ "service": "missing" })),
        "{refused}"
    );
    assert_eq!(
        refused.to_string(),
        "service missing is not available",
        "{refused}"
    );
    let answer = far
        .invoke_async("host:weather", "today", Value::List(Vec::new()))
        .await
        .unwrap();
    assert_eq!(
        answer.json().unwrap(),
        json!("sunny"),
        "the session goes on"
    );
}

/// A scoped host service is registered and called by its scoped id
/// (INS-4.2-3): with the label `tenant`, the lease is announced with it and
/// the runtime's call target is `weather\0tenant`, while the same name
/// outside the scope is another service, not there.
/// rule: INS-4.2-3
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_host_service_is_called_by_its_scoped_id() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let _lease = process
        .lease_host_in(
            "weather",
            Some("tenant"),
            Arc::new(Today(seen.clone())),
            Some(json!({ "today": "sync" })),
        )
        .await
        .unwrap();
    assert_eq!(
        *runtime.hosts.lock().unwrap(),
        vec![json!(["weather", { "today": "sync" }, "tenant"])],
        "the lease names its scope label"
    );
    let answer = far
        .invoke_async("host:weather\u{0}tenant", "today", Value::List(Vec::new()))
        .await
        .unwrap();
    assert_eq!(answer.json().unwrap(), json!("sunny"));
    let refused = far
        .invoke_async("host:weather", "today", Value::List(Vec::new()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("unavailable"), "{refused}");
    assert_eq!(
        refused.data(),
        Some(json!({ "service": "weather" })),
        "the name outside the label is another service: {refused}"
    );
    assert!(far.close_reason().is_none(), "the session goes on");
}

// ── Fifth batch: held references and runtime rows (#639) ────────────────

/// A far end that saves the function it is passed and answers with its
/// argument otherwise.
struct Holding(Arc<Mutex<Option<Reference>>>);

impl Dispatch for Holding {
    fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
        match (target, method) {
            ("svc", "hold") => {
                let callback = args
                    .list()?
                    .into_iter()
                    .next()
                    .ok_or_else(|| Error::value("no callback"))?
                    .reference()?;
                *self.0.lock().unwrap() = Some(callback);
                Ok(Value::Data(json!("held")))
            }
            _ => Err(Error::value(format!("no method {method}"))),
        }
    }
}

/// A far end whose `svc.slow` waits until the test lets it answer.
struct Blocking {
    started: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<Option<mpsc::Receiver<()>>>,
}

impl Dispatch for Blocking {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(());
        }
        if let Some(release) = self.release.lock().unwrap().take() {
            let _ = release.recv();
        }
        Ok(Value::Data(json!("finished")))
    }
}

/// A proxy's last clone is the one that releases (MNT-4.3-1): a drop that
/// leaves another clone sends nothing, and the last one sends one counted
/// `release`. What the owner does with the handle (keeping it while it is
/// the slot's current object) is the owner's side.
/// rule: MNT-4.3-1
#[tokio::test(flavor = "multi_thread")]
async fn a_proxys_release_waits_for_its_last_clone() {
    let (peer, mut far) = session(Arc::new(Echo));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list", "value": [reference(7, "object")],
            } }),
        );
        // The clone dropped while another is held: the next frame is the
        // call, not a release.
        let call = read(&mut far);
        assert_eq!(
            call["op"],
            json!("invoke"),
            "no release while another clone is held: {call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call),
                    "value": { "type": "data", "value": "held" } }),
        );
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 }),
            "the last clone releases the proxy"
        );
        far
    });
    let object = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    let clone = object.clone();
    drop(object);
    assert_eq!(
        peer.invoke(
            "svc",
            "echo",
            Value::List(vec![Value::Data(json!("still held"))])
        )
        .unwrap()
        .json()
        .unwrap(),
        json!("held")
    );
    drop(clone);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A proxy passed back to its owner is the owner's object, not a copy
/// (MNT-2-8l): the argument is the same reference id and kind the owner
/// exported, and the owner can tell it apart from a new grant.
/// rule: MNT-2-8l
#[tokio::test(flavor = "multi_thread")]
async fn a_proxy_passed_back_is_the_owners_object() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list", "value": [reference(7, "object")],
            } }),
        );
        // The object handed back: the same reference, no second export.
        let call = read(&mut far);
        let value = &call["args"]["value"][0]["value"];
        assert_eq!(
            call["args"]["value"][0]["type"],
            json!("reference"),
            "{call}"
        );
        assert_eq!(value["id"], json!(7), "{call}");
        assert_eq!(value["kind"], json!("object"), "{call}");
        assert_eq!(value["home"], json!(true), "the owner's own object: {call}");
        assert_eq!(value.get("body"), None, "no copy of the object: {call}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call),
                    "value": { "type": "data", "value": "same" } }),
        );
        assert_eq!(
            read(&mut far),
            json!({ "op": "release", "reference": 7, "count": 1 })
        );
        far
    });
    let object = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap()
        .remove(0)
        .reference()
        .unwrap();
    assert_eq!(
        peer.invoke(
            "svc",
            "take",
            Value::List(vec![Value::Reference(object.clone())])
        )
        .unwrap()
        .json()
        .unwrap(),
        json!("same")
    );
    drop(object);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A callback saved by the receiver is called later, and a promise it
/// answers is awaited (MNT-2-8m): the call reaches the far end that passed
/// the function after the original call returned, and its promise's value
/// is the call's value.
/// rule: MNT-2-8m
#[tokio::test(flavor = "multi_thread")]
async fn a_saved_callback_is_called_after_the_call_returns() {
    let saved: Arc<Mutex<Option<Reference>>> = Arc::default();
    let (peer, mut far) = session(Arc::new(Holding(saved.clone())));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        send(
            &mut far,
            invoke("node:1", "svc", "hold", one(&reference(7, "function"))),
        );
        let reply = read(&mut far);
        assert_eq!(
            reply,
            json!({ "id": "node:1", "op": "return",
                    "value": { "type": "data", "value": "held" } })
        );
        // Called later, with the argument the host chose.
        let call = read(&mut far);
        assert_eq!(call["op"], json!("call"), "{call}");
        assert_eq!(call["reference"], json!(7), "{call}");
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [{ "type": "data", "value": "later" }] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": reference(8, "future") }),
        );
        let awaited = read(&mut far);
        assert_eq!(awaited["op"], json!("await"), "{awaited}");
        assert_eq!(awaited["reference"], json!(8), "{awaited}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&awaited),
                    "value": { "type": "data", "value": "settled" } }),
        );
        let mut released = Vec::new();
        for _ in 0..2 {
            let frame = read(&mut far);
            assert_eq!(frame["op"], json!("release"), "{frame}");
            assert_eq!(frame["count"], json!(1), "{frame}");
            released.push(frame["reference"].as_u64().expect("a reference id"));
        }
        released.sort_unstable();
        assert_eq!(released, [7, 8], "both references released");
        far
    });
    until(
        || saved.lock().unwrap().is_some(),
        "the host to save the callback",
    )
    .await;
    let callback = saved.lock().unwrap().take().unwrap();
    let answer = callback
        .call_async(Value::List(vec![Value::Data(json!("later"))]))
        .await
        .unwrap();
    let Value::Reference(promise) = &answer else {
        panic!("the callback answered a promise: {answer:?}");
    };
    assert!(promise.is_future(), "a future reference");
    assert_eq!(
        settle(answer).await.unwrap().json().unwrap(),
        json!("settled")
    );
    drop(callback);
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A synchronous call cannot be cancelled midway (MNT-2-12): a `cancel`
/// that arrives while its method runs does not abort it, and the call
/// answers as if it were never cancelled. (The rule says this is not
/// guaranteed: the test fixes what the implementations do.)
/// rule: MNT-2-12
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_does_not_abort_a_synchronous_call() {
    let (started, started_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (peer, mut far) = session(Arc::new(Blocking {
        started: Mutex::new(Some(started)),
        release: Mutex::new(Some(release_rx)),
    }));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    send(
        &mut far,
        invoke(
            "node:1",
            "svc",
            "slow",
            one(&json!({ "type": "data", "value": "x" })),
        ),
    );
    within(
        "the method to start",
        tokio::task::spawn_blocking(move || started_rx.recv().unwrap()),
    )
    .await
    .unwrap();
    send(&mut far, json!({ "op": "cancel", "id": "node:1" }));
    release.send(()).unwrap();
    assert_eq!(
        read(&mut far),
        json!({ "op": "return", "id": "node:1",
                "value": { "type": "data", "value": "finished" } }),
        "the cancelled synchronous call still answers"
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A host service answering `today` with its own word, for telling two
/// providers apart.
struct Answers(&'static str);

impl HostDispatch for Answers {
    fn invoke(&self, method: &str, _: Value) -> Reply {
        match method {
            "today" => Ok(Value::Data(json!(self.0))),
            other => Err(Error::no_method("weather", other, vec!["today"])),
        }
    }
}

/// A Rust-provided weather service, to compare with a runtime row's.
struct RustWeather;

impl HostDispatch for RustWeather {
    fn invoke(&self, method: &str, _: Value) -> Reply {
        match method {
            "today" => Ok(Value::Data(json!("rust"))),
            other => Err(Error::no_method("weather", other, vec!["today"])),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "today": "sync" }))
    }
}

/// A runtime this side did not start is managed with the same row
/// operations as one it did (REM-1-1), on the same session protocol as a
/// plain session (SYN-2-13): `rows.load`, `rows.schema`, `rows.update` and
/// `rows.unload` are the frames, one session serves every row, and the
/// handshake is [`ENDPOINT_PROTOCOL`].
/// rule: REM-1-1
/// rule: SYN-2-13
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_runtime_runs_rows_with_the_same_control_operations() {
    let runtime = Scripted::new(json!({ "config": { "type": "object" } }));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    let greeting = process
        .connection()
        .greeting()
        .expect("the runtime greeted");
    assert_eq!(greeting.endpoint.as_str(), "rt");
    assert_eq!(ENDPOINT_PROTOCOL, 4, "the runtime's session protocol");
    assert!(greeting.capabilities.iter().any(|one| one == "runtime"));

    process
        .load_row(
            "a",
            Path::new("a.js"),
            json!({ "room": "lobby" }),
            &[],
            &[],
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.loads.lock().unwrap()[0][0],
        json!("a"),
        "the key names the row"
    );
    let described = process.describe_row(Path::new("a.js")).await.unwrap();
    assert_eq!(described.config, Some(json!({ "type": "object" })));
    process
        .update_row("a", json!({ "room": "hall" }))
        .await
        .unwrap();
    assert_eq!(
        *runtime.updates.lock().unwrap(),
        vec![json!(["a", { "room": "hall" }])]
    );
    process.unload_row("a").await.unwrap();
    assert_eq!(*runtime.unloads.lock().unwrap(), vec![json!(["a"])]);
    assert_eq!(
        *runtime.mounts.lock().unwrap(),
        1,
        "one session, however many rows load and unload"
    );
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A runtime resolves its plugin entries itself (REM-8-3): the control side
/// asks `rows.schema` with the entry it was given, even one this side's file
/// system does not have, and reports what the runtime answers. (Marking the
/// row `Unresolved` when the runtime has no such entry is the loader's.)
/// rule: REM-8-3
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_resolves_an_entry_this_side_cannot_see() {
    let schema = json!({ "config": { "type": "object" }, "version": "9.9.9" });
    let runtime = Scripted::new(schema.clone());
    let (_root, process, _far) = runtime_rig(runtime.clone()).await;
    let entry = "/nowhere/on/this/machine/plugin.js";
    let described = process.describe_row(Path::new(entry)).await.unwrap();
    assert_eq!(
        *runtime.schemas.lock().unwrap(),
        vec![json!([entry])],
        "the runtime is asked about the entry, not this side's file system"
    );
    assert_eq!(described.config, Some(schema["config"].clone()));
    assert_eq!(described.version.as_deref(), Some("9.9.9"));
}

/// A runtime that does not speak the row contract is refused when it mounts
/// (REQ-5-0): the missing features are named, and the session fails instead
/// of downgrading to something less.
/// rule: REQ-5-0
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_the_row_contract_is_refused() {
    let runtime = Scripted::with_features(&[], json!({}));
    let root = common::runtime::root();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let mut endpoint = Endpoint::rust(id("rt"));
    endpoint.capabilities.push("runtime".into());
    let _far = Connection::open(theirs, runtime, endpoint).unwrap();
    let said = match common::runtime::attached(&root, "rt", ours).await {
        Ok(_) => panic!("a runtime without the row contract mounted"),
        Err(said) => said,
    };
    assert!(
        said.contains("lacks rows.v2 and hosts"),
        "the missing features are named: {said}"
    );
    // One that speaks it mounts: the refusal is not every runtime's.
    let runtime = Scripted::new(json!({}));
    let (_root, process, _far) = runtime_rig(runtime).await;
    process.check_rows("node").unwrap();
}

/// A leaf runtime reports `leaf` when it mounts (M2-2-1): what it says of
/// itself is what the control side reports, and its row operations are the
/// session's own (a leaf runtime declares no `node` capability).
/// rule: M2-2-1
#[tokio::test(flavor = "multi_thread")]
async fn a_leaf_runtime_reports_itself_and_needs_no_node_link() {
    let runtime = Scripted::with_features(&["rows.v2", "hosts", "leaf"], json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    assert!(process.supports("leaf"), "the leaf runtime's report");
    assert!(
        !process.connection().supports("node"),
        "a leaf runtime is no node bridge"
    );
    process
        .load_row("a", Path::new("a.js"), json!({}), &[], &[], &[])
        .await
        .unwrap();
    assert_eq!(runtime.loads.lock().unwrap().len(), 1);
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// Loading and unloading rows neither creates nor closes the runtime's
/// session (REM-3-1): every row goes over the one session it mounted, and
/// the session outlives them.
/// rule: REM-3-1
#[tokio::test(flavor = "multi_thread")]
async fn loading_and_unloading_rows_leaves_the_session_open() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    let tag = process.connection().tag().to_owned();
    process
        .load_row("a", Path::new("a.js"), json!({}), &[], &[], &[])
        .await
        .unwrap();
    process.unload_row("a").await.unwrap();
    process
        .load_row("b", Path::new("b.js"), json!({}), &[], &[], &[])
        .await
        .unwrap();
    assert_eq!(runtime.loads.lock().unwrap().len(), 2);
    assert_eq!(*runtime.unloads.lock().unwrap(), vec![json!(["a"])]);
    assert_eq!(
        *runtime.mounts.lock().unwrap(),
        1,
        "the session was mounted once"
    );
    assert_eq!(process.connection().tag(), tag);
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A runtime with no feature plugin installed offers no node bridge
/// (REM-5.1-6), and an operation of a family it does not offer is refused,
/// with the session going on (REM-5.2-1).
/// rule: REM-5.1-6
/// rule: REM-5.2-1
#[tokio::test(flavor = "multi_thread")]
async fn a_leaf_runtime_offers_no_node_bridge() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    until(
        || {
            runtime.offers.lock().unwrap().iter().any(|offers| {
                offers[0]["families"]
                    .as_array()
                    .is_some_and(|families| families.iter().any(|family| family == "rows"))
            })
        },
        "the link's offers",
    )
    .await;
    let offers = runtime.offers.lock().unwrap().last().unwrap().clone();
    let families = offers[0]["families"].as_array().expect("families");
    assert!(
        !families.iter().any(|family| family == "plugins"),
        "no node bridge family is offered: {offers}"
    );
    let refused = far
        .invoke_async("", "plugins.frobnicate", Value::Undefined)
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("plugins.frobnicate is not offered here"),
        "{refused}"
    );
    // The refusal is one call's: the session goes on.
    process
        .describe_row(Path::new("a.js"))
        .await
        .expect("the session still answers");
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// An isolate label belongs to one runtime instance (REM-8-6): two runtimes
/// under one host, each leasing `weather` in `tenant`, are two services,
/// and each call reaches its own provider.
/// rule: REM-8-6
#[tokio::test(flavor = "multi_thread")]
async fn isolate_labels_belong_to_their_runtime_instance() {
    let root = common::runtime::root();
    let first = Scripted::new(json!({}));
    let second = Scripted::new(json!({}));
    let (first_ours, first_theirs) = rutis_bridge::memory::pair();
    let (second_ours, second_theirs) = rutis_bridge::memory::pair();
    let mut first_endpoint = Endpoint::rust(id("rt1"));
    first_endpoint.capabilities.push("runtime".into());
    let mut second_endpoint = Endpoint::rust(id("rt2"));
    second_endpoint.capabilities.push("runtime".into());
    let first_far = Connection::open(first_theirs, first.clone(), first_endpoint).unwrap();
    let second_far = Connection::open(second_theirs, second.clone(), second_endpoint).unwrap();
    let first_process = common::runtime::attached(&root, "rt1", first_ours)
        .await
        .unwrap();
    let second_process = common::runtime::attached(&root, "rt2", second_ours)
        .await
        .unwrap();
    let methods = Some(json!({ "today": "sync" }));
    let _first_lease = first_process
        .lease_host_in(
            "weather",
            Some("tenant"),
            Arc::new(Answers("first")),
            methods.clone(),
        )
        .await
        .unwrap();
    let _second_lease = second_process
        .lease_host_in(
            "weather",
            Some("tenant"),
            Arc::new(Answers("second")),
            methods,
        )
        .await
        .unwrap();
    assert_eq!(
        *first.hosts.lock().unwrap(),
        vec![json!(["weather", { "today": "sync" }, "tenant"])]
    );
    assert_eq!(
        *second.hosts.lock().unwrap(),
        vec![json!(["weather", { "today": "sync" }, "tenant"])]
    );
    for (far, answer) in [(&first_far, "first"), (&second_far, "second")] {
        assert_eq!(
            far.invoke_async("host:weather\u{0}tenant", "today", Value::List(Vec::new()))
                .await
                .unwrap()
                .json()
                .unwrap(),
            json!(answer),
            "each instance's own provider"
        );
    }
    assert!(first_far.close_reason().is_none(), "the session goes on");
    assert!(second_far.close_reason().is_none(), "the session goes on");
}

/// A Rust provider and a runtime row's service are one representation
/// (M1-4-1): both are `dyn HostDispatch` at `host_key(name)`, found by
/// `call_service`. The runtime row's import is a native service of this
/// side (REM-5.1-2), names its own session as origin, so the same service
/// does not go back over it (REM-4.2-7), and calls at the slot's handle
/// (MNT-4.1-1): a new object gets a new handle, while a service already
/// published keeps calling the object it was made for.
/// rule: M1-4-1
/// rule: REM-5.1-2
/// rule: REM-4.2-7
/// rule: MNT-4.1-1
#[tokio::test(flavor = "multi_thread")]
async fn a_row_service_slot_is_one_native_service() {
    // A Rust provider: the same key, called the same way.
    let plain = common::runtime::root();
    plain
        .provide_at(host_key("weather"), Arc::new(RustWeather))
        .unwrap();
    assert_eq!(
        call_service::<String>(&plain, "weather", "today", ())
            .await
            .unwrap(),
        "rust"
    );

    // A runtime row's service: the slot's handle is the service name.
    let runtime = Scripted::new(json!({}));
    let (root, process, _far) = runtime_rig(runtime.clone()).await;
    let provides = json!({ "weather": { "today": "sync", "later": "async" } });
    let exports = provides.as_object().unwrap();
    let projection = row_projection(exports);
    projection.attach(&root, process.clone()).unwrap();
    process
        .load_row_exporting(
            "w",
            Path::new("w.js"),
            json!({}),
            &[],
            &[],
            exports,
            projection.clone(),
            &[],
        )
        .await
        .unwrap();
    _far.invoke_async("", "service", json!(["weather", "weather", 1]).into())
        .await
        .unwrap();
    let native = root
        .get_at::<dyn HostDispatch>(&host_key("weather"))
        .expect("a native service at host_key");
    assert_eq!(
        native.origin(),
        Some(process.connection().tag()),
        "the import is this session's own"
    );
    assert_eq!(
        native.methods(),
        Some(provides["weather"].clone()),
        "the declaration is its shape"
    );
    assert_eq!(
        call_service::<String>(&root, "weather", "today", ())
            .await
            .unwrap(),
        "sunny"
    );
    assert_eq!(
        runtime.calls.lock().unwrap()[0],
        ("weather".to_owned(), "today".to_owned()),
        "the call names the slot's first handle"
    );

    // The slot holds another object: a new handle, another service here.
    _far.invoke_async("", "service", json!(["weather", "weather#2", 2]).into())
        .await
        .unwrap();
    let current = root
        .get_at::<dyn HostDispatch>(&host_key("weather"))
        .expect("the current slot's service");
    assert_eq!(
        call_service::<String>(&root, "weather", "today", ())
            .await
            .unwrap(),
        "sunny"
    );
    assert_eq!(
        runtime.calls.lock().unwrap()[1],
        ("weather#2".to_owned(), "today".to_owned()),
        "a new object, a new handle"
    );
    // The service published before the replacement still calls the object
    // it was made for.
    assert_eq!(
        native
            .invoke("today", Value::List(Vec::new()))
            .unwrap()
            .json()
            .unwrap(),
        json!("sunny")
    );
    assert_eq!(
        runtime.calls.lock().unwrap()[2],
        ("weather".to_owned(), "today".to_owned()),
        "a published service pins its handle"
    );
    assert_eq!(
        current.origin(),
        Some(process.connection().tag()),
        "the replacement is this session's own too"
    );
}

/// A declared method's shape is the interface contract across the session
/// (REQ-3-3) and placement does not change it (REM-6.3-2): a `sync` method
/// answers a value, an `async` method a promise whose value is awaited.
/// rule: REQ-3-3
/// rule: REM-6.3-2
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_method_shape_survives_the_runtime() {
    let runtime = Scripted::new(json!({}));
    let (root, process, _far) = runtime_rig(runtime.clone()).await;
    let provides = json!({ "weather": { "today": "sync", "later": "async" } });
    let exports = provides.as_object().unwrap();
    let projection = row_projection(exports);
    projection.attach(&root, process.clone()).unwrap();
    process
        .load_row_exporting(
            "w",
            Path::new("w.js"),
            json!({}),
            &[],
            &[],
            exports,
            projection.clone(),
            &[],
        )
        .await
        .unwrap();
    _far.invoke_async("", "service", json!(["weather", "weather", 1]).into())
        .await
        .unwrap();
    // Sync: the method's value is the call's value.
    assert_eq!(
        call_service::<String>(&root, "weather", "today", ())
            .await
            .unwrap(),
        "sunny"
    );
    // Async: the method answers a promise, and its value is awaited.
    assert_eq!(
        call_service::<String>(&root, "weather", "later", ())
            .await
            .unwrap(),
        "later"
    );
    assert_eq!(
        *runtime.calls.lock().unwrap(),
        vec![
            ("weather".to_owned(), "today".to_owned()),
            ("weather".to_owned(), "later".to_owned()),
        ]
    );
}

/// Two rows isolating the same name export it in their own scopes
/// (INS-4.2-10): the second is not refused, while a third row in the first
/// one's scope is; the exports are the runtime's own before any of its
/// services are announced. (A row seeing the host proxy leased in its
/// scope is INS-4.2-3's side.)
/// rule: INS-4.2-10
#[tokio::test(flavor = "multi_thread")]
async fn rows_isolating_the_same_name_export_it_in_their_own_scopes() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    let provides = json!({ "weather": { "today": "sync" } });
    let exports = provides.as_object().unwrap();
    for (key, label) in [("a", "tenant-a"), ("b", "tenant-b")] {
        process
            .load_row_exporting(
                key,
                Path::new("w.js"),
                json!({}),
                &[("weather".to_owned(), label.to_owned())],
                &[],
                exports,
                row_projection(exports),
                &[],
            )
            .await
            .unwrap();
    }
    assert_eq!(runtime.loads.lock().unwrap().len(), 2, "one load per scope");
    let refused = process
        .load_row_exporting(
            "c",
            Path::new("w.js"),
            json!({}),
            &[("weather".to_owned(), "tenant-a".to_owned())],
            &[],
            exports,
            row_projection(exports),
            &[],
        )
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("weather\u{0}tenant-a is already exported"),
        "the same scope has one provider: {refused}"
    );
    assert_eq!(
        runtime.loads.lock().unwrap().len(),
        2,
        "the refused row never loaded"
    );
    assert!(far.close_reason().is_none(), "the session goes on");
}

// ── Sixth batch: services, instances and control (#639) ─────────────────

/// A plugin that marks its fiber as an instance of `tenant` and hands its
/// identity and context back, for the instance-scoped tests.
struct Tenant(Mutex<Option<oneshot::Sender<(InstanceId, Ctx)>>>);

impl DynPlugin for Tenant {
    fn name(&self) -> &str {
        "tenant"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            ctx.mark_scope("tenant")?;
            let sender = self.0.lock().unwrap().take();
            if let Some(sender) = sender {
                let _ = sender.send((ctx.instance(), ctx.clone()));
            }
            Ok(Effect::Done)
        })
    }
}

/// A child context marked as an instance of `tenant`.
async fn tenant(root: &Ctx) -> (InstanceId, Ctx) {
    let (sender, received) = oneshot::channel();
    root.plugin(Tenant(Mutex::new(Some(sender)))).await.unwrap();
    received.await.unwrap()
}

/// The far end of a runtime session on `ends`, mounted as the row `row`.
async fn runtime_pair(
    ctx: &Ctx,
    row: &str,
    ends: (Channel, Channel),
    runtime: Arc<Scripted>,
) -> (Arc<Process>, Connection) {
    let (ours, theirs) = ends;
    let mut endpoint = Endpoint::rust(id(row));
    endpoint.capabilities.push("runtime".into());
    let far = Connection::open(theirs, runtime, endpoint).unwrap();
    let process = common::runtime::attached(ctx, row, ours).await.unwrap();
    (process, far)
}

/// [`runtime_pair`] on a memory channel.
async fn runtime_on(ctx: &Ctx, row: &str, runtime: Arc<Scripted>) -> (Arc<Process>, Connection) {
    runtime_pair(ctx, row, rutis_bridge::memory::pair(), runtime).await
}

/// A channel that is not the in-memory one: two queues and a condition
/// variable, so the session runs over a different carrier without changing
/// anything it sends (REM-10-1).
struct Wire {
    state: Mutex<WireState>,
    arrived: Condvar,
}

#[derive(Default)]
struct WireState {
    messages: VecDeque<Vec<u8>>,
    ended: bool,
}

impl Wire {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(WireState::default()),
            arrived: Condvar::new(),
        })
    }
}

struct WireSender(Arc<Wire>);

impl Sender for WireSender {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        let mut state = self.0.state.lock().unwrap();
        if state.ended {
            return Err(ChannelError::Closed {
                reason: "the wire ended".into(),
            });
        }
        state.messages.push_back(message.to_vec());
        self.0.arrived.notify_all();
        Ok(())
    }
}

struct WireReceiver(Arc<Wire>);

impl Receiver for WireReceiver {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if let Some(message) = state.messages.pop_front() {
                return Ok(Some(message));
            }
            if state.ended {
                return Ok(None);
            }
            state = self.0.arrived.wait(state).unwrap();
        }
    }
}

struct WireCloser {
    queues: [Arc<Wire>; 2],
}

impl Closer for WireCloser {
    fn close(&self, _: Close) {
        for wire in &self.queues {
            let mut state = wire.state.lock().unwrap();
            state.ended = true;
            wire.arrived.notify_all();
        }
    }
}

/// Two ends of a [`Wire`], a carrier of the test's own.
fn wire_pair() -> (Channel, Channel) {
    let a_to_b = Wire::new();
    let b_to_a = Wire::new();
    let a = Channel {
        sender: Box::new(WireSender(a_to_b.clone())),
        receiver: Box::new(WireReceiver(b_to_a.clone())),
        closer: Arc::new(WireCloser {
            queues: [a_to_b.clone(), b_to_a.clone()],
        }),
    };
    let b = Channel {
        sender: Box::new(WireSender(b_to_a.clone())),
        receiver: Box::new(WireReceiver(a_to_b.clone())),
        closer: Arc::new(WireCloser {
            queues: [a_to_b, b_to_a],
        }),
    };
    (a, b)
}

/// A far end whose channel takes messages only up to `limit` bytes, as a
/// transport with a size limit does. It does not answer `max_message`, so
/// the session cannot check a frame before writing it (CHN-3-4).
struct Limited {
    inner: Box<dyn Sender>,
    limit: usize,
}

impl Sender for Limited {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        if message.len() <= self.limit {
            return self.inner.send(message);
        }
        Err(ChannelError::Closed {
            reason: format!(
                "a frame of {} bytes is over the {}-byte limit",
                message.len(),
                self.limit
            ),
        })
    }
}

/// A far end that records the `link.offers` this side announces, for a
/// session without a runtime behind it (CHN-0-2).
struct Offers(Arc<Mutex<Vec<Json>>>);

impl Dispatch for Offers {
    fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
        if target.is_empty() && method == "link.offers" {
            self.0.lock().unwrap().push(args.json()?);
            return Ok(Value::Undefined);
        }
        Ok(args)
    }
}

/// The families of one `link.offers` frame.
fn families(offers: &Json) -> Vec<String> {
    offers[0]["families"]
        .as_array()
        .expect("families")
        .iter()
        .map(|family| family.as_str().expect("a family").to_owned())
        .collect()
}

/// A lease's method shape comes from the dispatch when the caller gives none
/// (M1-4.1-1): `hosts.provide` carries `{ method: "sync" }` from
/// [`HostDispatch::methods`], a dispatch that says nothing is announced with
/// `{}`, and the withdrawal names the service. (What a runtime's own `host`
/// declaration adds is the loader's side.)
/// rule: M1-4.1-1
#[tokio::test(flavor = "multi_thread")]
async fn host_method_shapes_fill_the_lease() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, _far) = runtime_rig(runtime.clone()).await;
    let lease = process
        .lease_host("weather", Arc::new(RustWeather), None)
        .await
        .unwrap();
    assert_eq!(
        *runtime.hosts.lock().unwrap(),
        vec![json!(["weather", { "today": "sync" }])],
        "the dispatch's own methods are the announcement"
    );
    lease.release().await.unwrap();
    assert_eq!(
        *runtime.withdraws.lock().unwrap(),
        vec![json!(["weather"])],
        "the last lease released withdraws the service"
    );
    let lease = process
        .lease_host("plain", Arc::new(Answers("plain")), None)
        .await
        .unwrap();
    assert_eq!(
        runtime.hosts.lock().unwrap().last(),
        Some(&json!(["plain", {}])),
        "a dispatch with no shape is announced without one"
    );
    lease.release().await.unwrap();
}

/// Only the names a runtime declares are projected, with the shapes it
/// declares (M1-5.1-3): the `service` frame for a declared name publishes a
/// service whose methods are the declared ones; one for a name outside the
/// declaration publishes nothing.
/// rule: M1-5.1-3
#[tokio::test(flavor = "multi_thread")]
async fn declared_provides_shapes_are_what_projects() {
    let runtime = Scripted::new(json!({}));
    let (root, process, far) = runtime_rig(runtime.clone()).await;
    let declared = json!({ "weather": { "today": "sync" } });
    let exports = declared.as_object().unwrap();
    let projection = row_projection(exports);
    projection.attach(&root, process.clone()).unwrap();
    process
        .load_row_exporting(
            "w",
            Path::new("w.js"),
            json!({}),
            &[],
            &[],
            exports,
            projection.clone(),
            &[],
        )
        .await
        .unwrap();
    far.invoke_async("", "service", json!(["weather", "weather#1", 1]).into())
        .await
        .unwrap();
    let service = root
        .get_at::<dyn HostDispatch>(&host_key("weather"))
        .expect("the declared service");
    assert_eq!(
        service.methods(),
        Some(declared["weather"].clone()),
        "the declaration is its shape"
    );
    assert_eq!(
        call_service::<String>(&root, "weather", "today", ())
            .await
            .unwrap(),
        "sunny"
    );
    far.invoke_async("", "service", json!(["sun", "sun#1", 1]).into())
        .await
        .unwrap();
    assert!(
        root.get_at::<dyn HostDispatch>(&host_key("sun")).is_none(),
        "a name outside the declaration is not projected"
    );
    assert_eq!(
        *runtime.calls.lock().unwrap(),
        vec![("weather#1".to_owned(), "today".to_owned())]
    );
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A row's `isolate` names stay in its own scope (M1-14-5): `weather`
/// isolated in `tenant` is not a service of this host, while the row's
/// declared `sunny` is; inside an instance the projection publishes under
/// `host_key_in`, declared and announced services alike (DIA-4-8), and the
/// isolate name still is not one.
/// rule: M1-14-5
/// rule: DIA-4-8
#[tokio::test(flavor = "multi_thread")]
async fn isolate_names_stay_in_the_rows_scope() {
    let runtime = Scripted::new(json!({}));
    let (root, process, far) = runtime_rig(runtime.clone()).await;
    let declared = json!({ "sunny": { "today": "sync" } });
    let exports = declared.as_object().unwrap();
    let projection = row_projection(exports);
    projection.attach(&root, process.clone()).unwrap();
    process
        .load_row_exporting(
            "w",
            Path::new("w.js"),
            json!({}),
            &[("weather".to_owned(), "tenant".to_owned())],
            &[],
            exports,
            projection.clone(),
            &[],
        )
        .await
        .unwrap();
    far.invoke_async("", "service", json!(["sunny", "sunny#1", 1]).into())
        .await
        .unwrap();
    far.invoke_async("", "service", json!(["weather", "weather#1", 1]).into())
        .await
        .unwrap();
    assert!(
        root.get_at::<dyn HostDispatch>(&host_key("sunny"))
            .is_some(),
        "the declared service is projected"
    );
    assert!(
        root.get_at::<dyn HostDispatch>(&host_key("weather"))
            .is_none(),
        "the isolate name is not a host service: {declared}"
    );

    let (instance, ctx) = tenant(&root).await;
    let runtime = Scripted::new(json!({}));
    let (process, far) = runtime_on(&ctx, "rt2", runtime.clone()).await;
    let scope = ctx.scope();
    let declared = json!({ "moon": { "today": "sync" } });
    let exports = declared.as_object().unwrap();
    let projection = row_projection_of("w", exports, move |name| host_key_in(name, instance));
    projection.attach(&ctx, process.clone()).unwrap();
    process
        .load_row_exporting(
            "w",
            Path::new("w.js"),
            json!({}),
            &[("weather".to_owned(), "tenant".to_owned())],
            &[],
            exports,
            projection.clone(),
            &scope,
        )
        .await
        .unwrap();
    far.invoke_async("", "service", json!(["moon", "moon#1", 1]).into())
        .await
        .unwrap();
    far.invoke_async(
        "",
        "service",
        json!(["store", "store#1", 1, { "row": "w", "methods": { "get": "async" } }]).into(),
    )
    .await
    .unwrap();
    assert!(
        ctx.get_at::<dyn HostDispatch>(&host_key_in("moon", instance))
            .is_some(),
        "a declared service is the instance's"
    );
    assert!(
        ctx.get_at::<dyn HostDispatch>(&host_key_in("store", instance))
            .is_some(),
        "an announced service is the instance's"
    );
    assert!(
        root.get_at::<dyn HostDispatch>(&host_key("moon")).is_none(),
        "the instance's services are not the root's"
    );
    assert!(
        root.get_at::<dyn HostDispatch>(&host_key("store"))
            .is_none(),
        "the announced service is not the root's either"
    );
    assert!(
        ctx.get_at::<dyn HostDispatch>(&host_key_in("weather", instance))
            .is_none(),
        "the isolate name is not projected inside the instance either"
    );
    let store = ctx
        .get_at::<dyn HostDispatch>(&host_key_in("store", instance))
        .unwrap();
    let answer = settle(store.invoke("get", Value::List(Vec::new())).unwrap())
        .await
        .unwrap();
    assert_eq!(answer.json().unwrap(), json!("sunny"));
    assert_eq!(
        runtime.calls.lock().unwrap().last(),
        Some(&("store#1".to_owned(), "get".to_owned())),
        "the call reaches the announced object"
    );
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// Rows loaded in an instance carry its scope chain and their isolate table
/// (INS-4.2-9), on the one session the runtime mounted outside the instance
/// (INS-5-5): the same connection and the same `mount` serve a row loaded at
/// the root, whose frame has an empty chain.
/// rule: INS-4.2-9
/// rule: INS-5-5
#[tokio::test(flavor = "multi_thread")]
async fn instance_rows_keep_the_scope_chain_on_one_session() {
    let root = common::runtime::root();
    let (instance, ctx) = tenant(&root).await;
    let runtime = Scripted::new(json!({}));
    let (process, far) = runtime_on(&ctx, "rt", runtime.clone()).await;
    let scope = ctx.scope();
    assert_eq!(
        scope,
        [Scope {
            id: instance.to_string(),
            group: "tenant".into(),
        }],
        "the instance's chain"
    );
    let tag = process.connection().tag().to_owned();
    process
        .load_row(
            "in-instance",
            Path::new("a.js"),
            json!({ "room": "lobby" }),
            &[("weather".to_owned(), "tenant".to_owned())],
            &["db".to_owned()],
            &scope,
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.loads.lock().unwrap()[0],
        json!([
            "in-instance",
            "a.js",
            { "room": "lobby" },
            [["weather", "tenant"]],
            ["db"],
            {},
            { "scope": [{ "id": instance.to_string(), "group": "tenant" }], "id": "in-instance" }
        ]),
        "the instance's chain and the isolate pairs are the row's"
    );
    process
        .load_row("at-root", Path::new("b.js"), json!({}), &[], &[], &[])
        .await
        .unwrap();
    assert_eq!(
        runtime.loads.lock().unwrap()[1][6]["scope"],
        json!([]),
        "a row loaded at the root has no chain"
    );
    assert_eq!(
        *runtime.mounts.lock().unwrap(),
        1,
        "one session outside the instance serves both rows"
    );
    assert_eq!(process.connection().tag(), tag);
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A frame over the channel's size limit is refused and the channel ends
/// (CHN-3-4): the call before it crossed, the oversized return never does,
/// and the session ends as a whole rather than answering that call.
/// rule: CHN-3-4
#[tokio::test(flavor = "multi_thread")]
async fn an_over_limit_frame_ends_the_session() {
    const LIMIT: usize = 4096;
    let (ours, mut far) = rutis_bridge::memory::pair();
    let Channel {
        sender,
        receiver,
        closer,
    } = ours;
    let ours = Channel {
        sender: Box::new(Limited {
            inner: sender,
            limit: LIMIT,
        }),
        receiver,
        closer,
    };
    let peer = Connection::open(ours, Arc::new(Echo), Endpoint::rust(id("main"))).unwrap();
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    send(
        &mut far,
        invoke(
            "node:1",
            "svc",
            "echo",
            one(&json!({ "type": "data", "value": "small" })),
        ),
    );
    assert_eq!(
        read(&mut far)["op"],
        json!("return"),
        "a small frame crosses"
    );
    let huge = "x".repeat(2 * LIMIT);
    send(
        &mut far,
        invoke(
            "node:2",
            "svc",
            "echo",
            one(&json!({ "type": "data", "value": huge })),
        ),
    );
    within("the session's end", peer.closed()).await;
    match peer.close_reason() {
        Some(Error::Transport(reason)) => {
            assert!(reason.contains("over the 4096-byte limit"), "{reason}")
        }
        other => panic!("a transport error expected, got {other:?}"),
    }
    assert!(
        matches!(far.receiver.recv(), Ok(None)),
        "the far end sees the channel end, not the return"
    );
    assert!(
        peer.invoke("svc", "echo", Value::Undefined).is_err(),
        "a call after the end fails"
    );
}

/// A configuration change is one `rows.update` frame (M2-2-5): a leaf
/// runtime restarts the row itself, and the control side sends no
/// `rows.unload` and no `rows.load` for it.
/// rule: M2-2-5
#[tokio::test(flavor = "multi_thread")]
async fn a_config_change_is_one_update() {
    let runtime = Scripted::with_features(&["rows.v2", "hosts", "leaf"], json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    process
        .load_row(
            "w",
            Path::new("w.js"),
            json!({ "room": "lobby" }),
            &[],
            &[],
            &[],
        )
        .await
        .unwrap();
    process
        .update_row("w", json!({ "room": "hall" }))
        .await
        .unwrap();
    process
        .update_row("w", json!({ "room": "garden" }))
        .await
        .unwrap();
    assert_eq!(
        *runtime.updates.lock().unwrap(),
        vec![
            json!(["w", { "room": "hall" }]),
            json!(["w", { "room": "garden" }])
        ],
        "each change is its own update"
    );
    assert_eq!(
        runtime.loads.lock().unwrap().len(),
        1,
        "the row was loaded once"
    );
    assert!(
        runtime.unloads.lock().unwrap().is_empty(),
        "nothing unloaded"
    );
    assert_eq!(*runtime.mounts.lock().unwrap(), 1);
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// Two runtime instances isolating the same name under the same label are
/// two environments (REM-5.2-6): each row's `rows.load` carries its own
/// isolate pair, each instance leases its own `weather`, and each call
/// reaches its own provider.
/// rule: REM-5.2-6
#[tokio::test(flavor = "multi_thread")]
async fn two_peers_isolate_the_same_label_apart() {
    let root = common::runtime::root();
    let first = Scripted::new(json!({}));
    let second = Scripted::new(json!({}));
    let (first_process, first_far) = runtime_on(&root, "rt1", first.clone()).await;
    let (second_process, second_far) = runtime_on(&root, "rt2", second.clone()).await;
    for process in [&first_process, &second_process] {
        process
            .load_row(
                "w",
                Path::new("w.js"),
                json!({}),
                &[("weather".to_owned(), "tenant".to_owned())],
                &[],
                &[],
            )
            .await
            .unwrap();
    }
    for runtime in [&first, &second] {
        assert_eq!(
            runtime.loads.lock().unwrap()[0][3],
            json!([["weather", "tenant"]]),
            "the row's own isolate pair"
        );
    }
    let methods = Some(json!({ "today": "sync" }));
    let _first_lease = first_process
        .lease_host_in(
            "weather",
            Some("tenant"),
            Arc::new(Answers("first")),
            methods.clone(),
        )
        .await
        .unwrap();
    let _second_lease = second_process
        .lease_host_in(
            "weather",
            Some("tenant"),
            Arc::new(Answers("second")),
            methods,
        )
        .await
        .unwrap();
    for runtime in [&first, &second] {
        assert_eq!(
            runtime.hosts.lock().unwrap()[0],
            json!(["weather", { "today": "sync" }, "tenant"]),
            "the label travels with the lease"
        );
    }
    for (far, answer) in [(&first_far, "first"), (&second_far, "second")] {
        assert_eq!(
            far.invoke_async("host:weather\u{0}tenant", "today", Value::List(Vec::new()))
                .await
                .unwrap()
                .json()
                .unwrap(),
            json!(answer),
            "each instance's own provider"
        );
    }
    assert!(first_far.close_reason().is_none(), "the session goes on");
    assert!(second_far.close_reason().is_none(), "the session goes on");
}

/// The same management frames run over another carrier (REM-10-1): a
/// runtime mounted on the in-memory channel and one on the test's own queue
/// channel get the same `mount`, `rows.load`, `rows.update` and
/// `rows.unload`, no more and no fewer.
/// rule: REM-10-1
#[tokio::test(flavor = "multi_thread")]
async fn the_management_frames_do_not_depend_on_the_carrier() {
    let run = |ends| async move {
        let root = common::runtime::root();
        let runtime = Scripted::new(json!({}));
        let (process, far) = runtime_pair(&root, "rt", ends, runtime.clone()).await;
        process
            .load_row(
                "w",
                Path::new("w.js"),
                json!({ "room": "lobby" }),
                &[("weather".to_owned(), "tenant".to_owned())],
                &["db".to_owned()],
                &[],
            )
            .await
            .unwrap();
        process
            .update_row("w", json!({ "room": "hall" }))
            .await
            .unwrap();
        process.unload_row("w").await.unwrap();
        let frames = (
            runtime.loads.lock().unwrap().clone(),
            runtime.updates.lock().unwrap().clone(),
            runtime.unloads.lock().unwrap().clone(),
        );
        let mounts = *runtime.mounts.lock().unwrap();
        assert!(far.close_reason().is_none(), "the session goes on");
        (frames, mounts)
    };
    let memory = run(rutis_bridge::memory::pair()).await;
    let wire = run(wire_pair()).await;
    assert_eq!(memory.0, wire.0, "the same frames on both carriers");
    assert_eq!(memory.1, 1, "one mount on the in-memory carrier");
    assert_eq!(wire.1, 1, "one mount on the queue carrier");
}

/// A runtime this side did not start is driven by the control side alone
/// (DIA-3-1): the lease is announced before the row loads, the configuration
/// change and the unload are the caller's, and the service is withdrawn
/// after the row is gone.
/// rule: DIA-3-1
#[tokio::test(flavor = "multi_thread")]
async fn a_dialed_in_runtime_is_driven_by_control() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    let lease = process
        .lease_host(
            "weather",
            Arc::new(Answers("sunny")),
            Some(json!({ "today": "sync" })),
        )
        .await
        .unwrap();
    process
        .load_row(
            "w",
            Path::new("w.js"),
            json!({ "room": "lobby" }),
            &[],
            &["weather".to_owned()],
            &[],
        )
        .await
        .unwrap();
    process
        .update_row("w", json!({ "room": "hall" }))
        .await
        .unwrap();
    process.unload_row("w").await.unwrap();
    lease.release().await.unwrap();
    let sequence_guard = runtime.sequence.lock().unwrap();
    let sequence: Vec<&str> = sequence_guard
        .iter()
        .map(String::as_str)
        .filter(|step| {
            matches!(
                *step,
                "hosts.provide" | "rows.load" | "rows.update" | "rows.unload" | "hosts.withdraw"
            )
        })
        .collect();
    assert_eq!(
        sequence,
        [
            "hosts.provide",
            "rows.load",
            "rows.update",
            "rows.unload",
            "hosts.withdraw"
        ],
        "the control side gives the order"
    );
    assert_eq!(*runtime.mounts.lock().unwrap(), 1);
    assert_eq!(runtime.hosts.lock().unwrap().len(), 1);
    assert_eq!(runtime.loads.lock().unwrap().len(), 1);
    assert_eq!(runtime.unloads.lock().unwrap().len(), 1);
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A plugin the application carries is reported, not installed (APP-1-5,
/// APP-1-6): the mount's `plugins` list is what this side reports, and a
/// plugin with no row the control side loads is never described or loaded.
/// Loading, configuring and unloading follow the control side's calls
/// (APP-1-2), and only its `rows.load` loads a row (REM-4.1-4).
/// rule: APP-1-2
/// rule: APP-1-5
/// rule: APP-1-6
/// rule: REM-4.1-4
#[tokio::test(flavor = "multi_thread")]
async fn a_mount_listing_is_not_an_install() {
    let schema = json!({
        "config": { "type": "object" },
        "inject": ["db"],
        "provides": { "weather": { "today": "async" } },
        "version": "1.0.0",
    });
    let runtime = Scripted::listing("carried.js", schema.clone());
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    let listed = process.plugins().expect("the mount listed its plugins");
    assert_eq!(listed.len(), 1, "the application's report has one plugin");
    assert_eq!(
        listed["carried.js"].provides.get("weather"),
        Some(&json!({ "today": "async" }))
    );
    assert!(
        runtime.loads.lock().unwrap().is_empty(),
        "a listed plugin is not loaded by the mount"
    );
    assert!(
        runtime.schemas.lock().unwrap().is_empty(),
        "a listed plugin is not described by the mount"
    );
    process
        .describe_row(Path::new("configured.js"))
        .await
        .unwrap();
    assert!(
        runtime.loads.lock().unwrap().is_empty(),
        "describing a plugin loads nothing"
    );
    assert_eq!(
        *runtime.schemas.lock().unwrap(),
        vec![json!(["configured.js"])],
        "only the plugin the control side asks about is described"
    );
    process
        .load_row(
            "configured",
            Path::new("configured.js"),
            json!({ "room": "lobby" }),
            &[],
            &[],
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.loads.lock().unwrap()[0][0],
        json!("configured"),
        "the only load is the one the control side asked for"
    );
    assert!(
        !runtime
            .loads
            .lock()
            .unwrap()
            .iter()
            .any(|load| load[1] == json!("carried.js")),
        "the application's plugin was never loaded"
    );
    process
        .update_row("configured", json!({ "room": "hall" }))
        .await
        .unwrap();
    process.unload_row("configured").await.unwrap();
    assert_eq!(runtime.updates.lock().unwrap().len(), 1);
    assert_eq!(runtime.unloads.lock().unwrap().len(), 1);
    assert_eq!(*runtime.mounts.lock().unwrap(), 1);
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A leaf runtime's injects are names it asks for (REM-4.2-10): the row is
/// loaded with `weather` in its inject list, but until this side leases it
/// the name is not a service and a call to `host:weather` fails with
/// `unavailable`; the lease announces it and the same call answers.
/// rule: REM-4.2-10
#[tokio::test(flavor = "multi_thread")]
async fn a_leaf_rows_injects_are_gated_by_leases() {
    let runtime = Scripted::with_features(&["rows.v2", "hosts", "leaf"], json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    process
        .load_row(
            "w",
            Path::new("w.js"),
            json!({}),
            &[],
            &["weather".to_owned()],
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.loads.lock().unwrap()[0][4],
        json!(["weather"]),
        "the inject name is the row's"
    );
    assert!(
        runtime.hosts.lock().unwrap().is_empty(),
        "an inject name is not a lease"
    );
    let refused = far
        .invoke_async("host:weather", "today", Value::List(Vec::new()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("unavailable"), "{refused}");
    assert_eq!(
        refused.data(),
        Some(json!({ "service": "weather" })),
        "{refused}"
    );
    let _lease = process
        .lease_host(
            "weather",
            Arc::new(Answers("sunny")),
            Some(json!({ "today": "sync" })),
        )
        .await
        .unwrap();
    assert_eq!(
        *runtime.hosts.lock().unwrap(),
        vec![json!(["weather", { "today": "sync" }])]
    );
    assert_eq!(
        far.invoke_async("host:weather", "today", Value::List(Vec::new()))
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("sunny")
    );
    assert!(process.supports("leaf"));
    assert!(!process.connection().supports("node"));
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A failed handshake is a category of its own, not a closed channel's
/// reason (CHN-2.2-1): greeting as another endpoint than the expected one,
/// as this endpoint, and speaking another protocol version each fail with
/// `IdentityMismatch` or `Incompatible` and a sentence naming what was
/// wrong.
/// rule: CHN-2.2-1
#[tokio::test(flavor = "multi_thread")]
async fn a_handshake_failure_has_its_own_category() {
    // The expected endpoint, greeted as another.
    let (peer, mut far) = session_with(
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("node")),
    );
    assert_eq!(read(&mut far)["op"], json!("hello"));
    send(
        &mut far,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "other",
                "capabilities": [] }),
    );
    match peer.ready().await {
        Err(Error::Handshake(rutis_bridge::session::Handshake::IdentityMismatch(reason))) => {
            assert!(reason.contains("other"), "{reason}");
            assert!(reason.contains("node"), "{reason}");
        }
        other => panic!("an identity mismatch expected, got {other:?}"),
    }
    assert!(
        matches!(far.receiver.recv(), Ok(None)),
        "the refused handshake ends the channel"
    );

    // This endpoint's own id.
    let (peer, mut far) = session(Arc::new(Echo));
    assert_eq!(read(&mut far)["op"], json!("hello"));
    send(
        &mut far,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "main",
                "capabilities": [] }),
    );
    match peer.ready().await {
        Err(Error::Handshake(rutis_bridge::session::Handshake::IdentityMismatch(reason))) => {
            assert!(reason.contains("this endpoint"), "{reason}")
        }
        other => panic!("an identity mismatch expected, got {other:?}"),
    }

    // Another protocol version.
    let (peer, mut far) = session(Arc::new(Echo));
    assert_eq!(read(&mut far)["op"], json!("hello"));
    send(
        &mut far,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL + 1, "endpoint": "node",
                "capabilities": [] }),
    );
    match peer.ready().await {
        Err(Error::Handshake(rutis_bridge::session::Handshake::Incompatible(reason))) => {
            assert!(reason.contains("the far end speaks protocol"), "{reason}");
            assert!(reason.contains("upgrade"), "{reason}");
        }
        other => panic!("an incompatible handshake expected, got {other:?}"),
    }
}

/// Two endpoints share the carrier and the session (CHN-0-2): a far end
/// without a runtime behind it and a leaf runtime both use one `Session`
/// and one `link`, and the runtime's rows need no node bridge (NOD-3-8):
/// its offers have no `plugins` family and its session has no `node`
/// capability.
/// rule: CHN-0-2
/// rule: NOD-3-8
#[tokio::test(flavor = "multi_thread")]
async fn a_connected_runtime_is_no_node_connection() {
    // A peer without a runtime: the session and its link exist, and the
    // offers name only the service family.
    let root = common::runtime::root();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let far = Connection::open(
        theirs,
        Arc::new(Offers(heard.clone())),
        Endpoint::rust(id("peer")),
    )
    .unwrap();
    let _ = common::runtime::attached(&root, "peer-row", ours).await;
    until(
        || {
            heard
                .lock()
                .unwrap()
                .iter()
                .any(|offers| families(offers).iter().any(|family| family == "services"))
        },
        "the service offer",
    )
    .await;
    assert_eq!(
        far.greeting()
            .expect("the session greeted")
            .endpoint
            .as_str(),
        "main",
        "the carrier names this endpoint"
    );
    let plain = families(
        heard
            .lock()
            .unwrap()
            .iter()
            .find(|offers| families(offers).iter().any(|family| family == "services"))
            .unwrap(),
    );
    assert_eq!(plain, ["services"], "a plain peer gets the service family");

    // A leaf runtime: the same session and link, plus the runtime's
    // families; no node bridge.
    let runtime = Scripted::with_features(&["rows.v2", "hosts", "leaf"], json!({}));
    let (process, far) = runtime_on(&root, "rt", runtime.clone()).await;
    assert!(process.supports("leaf"));
    assert!(!process.connection().supports("node"));
    until(
        || {
            runtime
                .offers
                .lock()
                .unwrap()
                .iter()
                .any(|offers| families(offers).iter().any(|family| family == "rows"))
        },
        "the runtime's offers",
    )
    .await;
    let runtime_families = {
        let offers = runtime.offers.lock().unwrap();
        families(
            offers
                .iter()
                .find(|offers| families(offers).iter().any(|family| family == "rows"))
                .unwrap(),
        )
    };
    for family in ["services", "host", "rows"] {
        assert!(
            runtime_families.iter().any(|one| one == family),
            "{family} is offered: {runtime_families:?}"
        );
    }
    assert!(
        !runtime_families.iter().any(|one| one == "plugins"),
        "no node bridge is offered: {runtime_families:?}"
    );
    process
        .load_row("w", Path::new("w.js"), json!({}), &[], &[], &[])
        .await
        .unwrap();
    assert_eq!(*runtime.mounts.lock().unwrap(), 1);
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// Functions, promises and object references cross by their kind
/// (REM-6.2-6): a far end that declares no `objects` capability still
/// returns all three, the function is called, the promise awaited and the
/// object read, and this side declares no `objects` either.
/// rule: REM-6.2-6
#[tokio::test(flavor = "multi_thread")]
async fn capabilities_say_which_kinds_cross() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &[]);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list", "value": [reference(7, "function"), reference(8, "future"),
                                          reference(9, "object")],
            } }),
        );
        let call = read(&mut far);
        assert_eq!(call["op"], json!("call"), "{call}");
        assert_eq!(call["reference"], json!(7), "{call}");
        assert_eq!(
            call["args"],
            json!({ "type": "list", "value": [{ "type": "data", "value": "x" }] }),
            "{call}"
        );
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "data", "value": "called" } }),
        );
        let call = read(&mut far);
        assert_eq!(call["op"], json!("await"), "{call}");
        assert_eq!(call["reference"], json!(8), "{call}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "data", "value": "settled" } }),
        );
        let call = read(&mut far);
        assert_eq!(call["op"], json!("get"), "{call}");
        assert_eq!(call["reference"], json!(9), "{call}");
        assert_eq!(call["property"], json!("today"), "{call}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "data", "value": "sunny" } }),
        );
        let mut released = Vec::new();
        for _ in 0..3 {
            let frame = read(&mut far);
            assert_eq!(frame["op"], json!("release"), "{frame}");
            assert_eq!(frame["count"], json!(1), "{frame}");
            released.push(frame["reference"].as_u64().expect("a reference id"));
        }
        released.sort_unstable();
        assert_eq!(released, [7, 8, 9], "all three references released");
        far
    });
    let values = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap();
    let function = values[0].clone().reference().unwrap();
    let future = values[1].clone().reference().unwrap();
    let object = values[2].clone().reference().unwrap();
    assert!(function.is_function(), "a function reference");
    assert!(future.is_future(), "a future reference");
    assert!(object.is_object(), "an object reference");
    let answer = function
        .call_async(Value::List(vec![Value::Data(json!("x"))]))
        .await
        .unwrap();
    assert_eq!(
        settle(answer).await.unwrap().json().unwrap(),
        json!("called")
    );
    assert_eq!(
        settle(Value::Reference(future.clone()))
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("settled")
    );
    let answer = object.get("today").unwrap();
    assert_eq!(
        settle(answer).await.unwrap().json().unwrap(),
        json!("sunny")
    );
    drop(values);
    drop(function);
    drop(future);
    drop(object);
    let _far = far_end(end).await;
    assert!(
        !Endpoint::rust(id("main"))
            .capabilities
            .iter()
            .any(|capability| capability == "objects"),
        "this side declares no objects capability"
    );
    assert!(peer.close_reason().is_none(), "the session goes on");
}

// ── Fifth batch: carriers, dialing, node links and local services (#639) ──

/// A kernel greeting as `name`: a root with the id its sessions greet with.
fn kernel(name: &str) -> Ctx {
    let root = Ctx::root().unwrap();
    root.provide(LocalId(id(name))).unwrap();
    root
}

/// A plugin providing one service that answers `get` with `answer`.
struct Provides {
    service: &'static str,
    answer: &'static str,
}

impl DynPlugin for Provides {
    fn name(&self) -> &str {
        self.service
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            ctx.provide_at(host_key(self.service), Arc::new(Says(self.answer)))?;
            Ok(Effect::Done)
        })
    }
}

/// A host service that answers `get` with its word, asynchronously.
struct Says(&'static str);

impl HostDispatch for Says {
    fn invoke(&self, method: &str, _: Value) -> Reply {
        match method {
            "get" => Ok(Value::Data(json!(self.0))),
            other => Err(Error::no_method(self.0, other, vec!["get"])),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "get": "async" }))
    }
}

/// The catalog plugin `contacts`, providing the service `contacts`.
struct Contacts;

impl PluginFactory<Json> for Contacts {
    fn name(&self) -> &str {
        "contacts"
    }

    fn build(&self, _: &Json) -> Result<Box<dyn DynPlugin>, RutisError> {
        Ok(Box::new(Provides {
            service: "contacts",
            answer: "office",
        }))
    }
}

/// The plugin `user`, whose service calls the kernel's own `clock` plugin
/// `n` times when the far end asks it to.
struct User;

impl PluginFactory<Json> for User {
    fn name(&self) -> &str {
        "user"
    }

    fn build(&self, _: &Json) -> Result<Box<dyn DynPlugin>, RutisError> {
        Ok(Box::new(UserPlugin))
    }
}

struct UserPlugin;

impl DynPlugin for UserPlugin {
    fn name(&self) -> &str {
        "user"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            ctx.provide_at(host_key("user"), Arc::new(Asks(ctx.clone())))?;
            Ok(Effect::Done)
        })
    }
}

/// The service `user`: `use_now(n)` calls `clock` n times, natively.
struct Asks(Ctx);

impl HostDispatch for Asks {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        match method {
            "use_now" => {
                let n = args
                    .list()?
                    .into_iter()
                    .next()
                    .unwrap_or(Value::Undefined)
                    .json()?
                    .as_u64()
                    .unwrap_or(0);
                let ctx = self.0.clone();
                Ok(Value::future(async move {
                    let mut answers = Vec::new();
                    for _ in 0..n {
                        answers.push(call_service::<String>(&ctx, "clock", "get", ()).await?);
                    }
                    Ok(Value::Data(json!(answers)))
                }))
            }
            other => Err(Error::no_method("user", other, vec!["use_now"])),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "use_now": "async" }))
    }
}

/// The connection row `row`: mounts a session on its channel.
struct RowSession {
    row: &'static str,
    channel: Mutex<Option<Channel>>,
}

impl DynPlugin for RowSession {
    fn name(&self) -> &str {
        self.row
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            ctx.mark_row(self.row)?;
            let channel = self.channel.lock().unwrap().take().expect("one mount");
            ctx.plugin(Session::new(channel));
            Ok(Effect::Done)
        })
    }
}

/// A channel pair counting every message sent either way.
struct Counting {
    inner: Box<dyn Sender>,
    count: Arc<AtomicUsize>,
}

impl Sender for Counting {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.inner.send(message)
    }

    fn max_message(&self) -> Option<usize> {
        self.inner.max_message()
    }
}

fn counted(channel: Channel, count: Arc<AtomicUsize>) -> Channel {
    Channel {
        sender: Box::new(Counting {
            inner: channel.sender,
            count,
        }),
        receiver: channel.receiver,
        closer: channel.closer,
    }
}

/// Two ends of a counting memory channel, and the counter.
fn counted_pair() -> (Channel, Channel, Arc<AtomicUsize>) {
    let (ours, theirs) = rutis_bridge::memory::pair();
    let count = Arc::new(AtomicUsize::new(0));
    (
        counted(ours, count.clone()),
        counted(theirs, count.clone()),
        count,
    )
}

/// A Cordis row's services all reach rutis (M1-14-1), and a leaf plugin's
/// `ctx.provide` has the same wire shape (M1-14-9): every name the runtime
/// announces without declaring it is projected with the methods the
/// `service` frame carries, all of them async; a name in the row's
/// `provides` keeps the declared shape, sync methods included. (The
/// runtime's own method extraction from a Cordis object or an SDK plugin
/// is not visible here.)
/// rule: M1-14-1
/// rule: M1-14-9
#[tokio::test(flavor = "multi_thread")]
async fn announced_services_are_all_projected_with_their_methods() {
    for (cordis, row, names) in [
        (true, "cordis-row", ["store", "cache"]),
        (false, "leaf-row", ["leaf-a", "leaf-b"]),
    ] {
        let runtime = Scripted::new(json!({ "cordis": cordis, "announces": true }));
        let (root, process, far) = runtime_rig(runtime.clone()).await;
        let declared = json!({ "weather": { "today": "sync" } });
        let exports = declared.as_object().unwrap();
        let projection = row_projection_of(row, exports, host_key);
        projection.attach(&root, process.clone()).unwrap();
        process
            .load_row_exporting(
                row,
                Path::new("row.js"),
                json!({}),
                &[],
                &[],
                exports,
                projection.clone(),
                &[],
            )
            .await
            .unwrap();
        // The declared service gets its object first.
        far.invoke_async("", "service", json!(["weather", "weather#1", 1]).into())
            .await
            .unwrap();
        // Every announced name, with the methods the runtime gives it.
        for (index, name) in names.iter().enumerate() {
            far.invoke_async(
                "",
                "service",
                json!([name, format!("{name}#1"), index as u64 + 1,
                       { "row": row, "methods": { "get": "async" } }])
                .into(),
            )
            .await
            .unwrap();
        }
        // The declared name announced with another shape: the declaration
        // stays what is projected.
        far.invoke_async(
            "",
            "service",
            json!(["weather", "weather#2", 2,
                   { "row": row, "methods": { "get": "async" } }])
            .into(),
        )
        .await
        .unwrap();
        for name in names {
            let service = root
                .get_at::<dyn HostDispatch>(&host_key(name))
                .unwrap_or_else(|| panic!("{name} is projected"));
            assert_eq!(service.methods(), Some(json!({ "get": "async" })), "{name}");
            let answer = service.invoke("get", Value::List(Vec::new())).unwrap();
            assert!(
                matches!(&answer, Value::Reference(reference) if reference.is_future()),
                "{name}: an announced method is async"
            );
            assert_eq!(
                settle(answer).await.unwrap().json().unwrap(),
                json!("sunny"),
                "{name}"
            );
        }
        let weather = root
            .get_at::<dyn HostDispatch>(&host_key("weather"))
            .unwrap();
        assert_eq!(
            weather.methods(),
            Some(declared["weather"].clone()),
            "the declaration is the shape"
        );
        assert_eq!(
            call_service::<String>(&root, "weather", "today", ())
                .await
                .unwrap(),
            "sunny"
        );
        assert!(
            call_service::<String>(&root, "weather", "get", ())
                .await
                .is_err(),
            "the announced shape does not replace the declaration"
        );
        assert!(far.close_reason().is_none(), "the session goes on");
    }
}

/// A runtime mounts with no host service to wait for (M1-5.4-1): the
/// `mount` call carries no `provided` service and no scope, a later lease
/// is announced into the running process without mounting it again, and
/// the wait for an inject lands on the row: `host:weather` is
/// `unavailable` until it is leased. (The runtime plugin's own `injects`
/// are on that plugin's side.)
/// rule: M1-5.4-1
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_mounts_without_waiting_for_host_services() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    assert_eq!(
        *runtime.mount_args.lock().unwrap(),
        vec![json!({
            "plugins": [],
            "services": null,
            "provided": {},
            "announce": true,
            "scope": [],
            "runtime": "rt",
        })],
        "the mount has no host service and no scope"
    );
    assert!(
        runtime.hosts.lock().unwrap().is_empty(),
        "nothing is leased at the mount"
    );
    process
        .load_row(
            "w",
            Path::new("w.js"),
            json!({}),
            &[],
            &["weather".to_owned()],
            &[],
        )
        .await
        .unwrap();
    let refused = far
        .invoke_async("host:weather", "today", Value::List(Vec::new()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("unavailable"), "{refused}");
    let _lease = process
        .lease_host(
            "weather",
            Arc::new(Answers("sunny")),
            Some(json!({ "today": "sync" })),
        )
        .await
        .unwrap();
    assert_eq!(
        *runtime.mounts.lock().unwrap(),
        1,
        "the lease does not mount the runtime again"
    );
    assert_eq!(
        far.invoke_async("host:weather", "today", Value::List(Vec::new()))
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("sunny")
    );
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// A row sees the service name, and the isolate label is separate metadata
/// (INS-4.2-4): `rows.load` carries `weather` in the inject list and
/// `[weather, tenant]` in the isolate table; the lease is announced with
/// the label, the bare name is no service, and only the scoped id reaches
/// the provider. (The Node isolate symbol and the leaf runtimes' table
/// lookup are on that runtime's side.)
/// rule: INS-4.2-4
#[tokio::test(flavor = "multi_thread")]
async fn rows_see_service_names_and_keep_isolate_labels_apart() {
    let runtime = Scripted::new(json!({}));
    let (_root, process, far) = runtime_rig(runtime.clone()).await;
    process
        .load_row(
            "w",
            Path::new("w.js"),
            json!({}),
            &[("weather".to_owned(), "tenant".to_owned())],
            &["weather".to_owned()],
            &[],
        )
        .await
        .unwrap();
    let load = runtime.loads.lock().unwrap()[0].clone();
    assert_eq!(
        load[3],
        json!([["weather", "tenant"]]),
        "the isolate pair is its own"
    );
    assert_eq!(
        load[4],
        json!(["weather"]),
        "the plugin sees the name, not the scoped id"
    );
    let _lease = process
        .lease_host_in(
            "weather",
            Some("tenant"),
            Arc::new(Answers("sunny")),
            Some(json!({ "today": "sync" })),
        )
        .await
        .unwrap();
    assert_eq!(
        *runtime.hosts.lock().unwrap(),
        vec![json!(["weather", { "today": "sync" }, "tenant"])],
        "the label travels with the lease"
    );
    let bare = far
        .invoke_async("host:weather", "today", Value::List(Vec::new()))
        .await
        .unwrap_err();
    assert_eq!(bare.code(), Some("unavailable"), "{bare}");
    assert_eq!(
        far.invoke_async("host:weather\u{0}tenant", "today", Value::List(Vec::new()))
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("sunny")
    );
    assert!(far.close_reason().is_none(), "the session goes on");
}

/// The default conversion to JSON loses nothing (VAL-1-V2): data, arrays
/// and small bigints convert exactly; a value that would lose information
/// fails that conversion alone, saying which value it was, inside a record
/// too. (The languages' own conversions are on their side.)
/// rule: VAL-1-V2
#[tokio::test(flavor = "multi_thread")]
async fn json_conversion_is_lossless_or_refused_with_its_reason() {
    let (peer, mut far) = session(Arc::new(Nothing));
    greet(&mut far, "node", &VALUE_CAPABILITIES);
    peer.ready().await.unwrap();
    let end = std::thread::spawn(move || {
        let call = read(&mut far);
        assert_eq!(call["op"], json!("invoke"), "{call}");
        send(
            &mut far,
            json!({ "op": "return", "id": frame_id(&call), "value": {
                "type": "list", "value": [
                    { "type": "data", "value": { "a": 1 } },
                    { "type": "bigint", "value": "9007199254740991" },
                    { "type": "bigint", "value": "18446744073709551616" },
                    { "type": "instant", "value": "1970-01-01T00:00:00.123456789Z" },
                    { "type": "record", "value": {
                        "n": { "type": "bigint", "value": "18446744073709551616" },
                        "d": { "type": "data", "value": 1 } } },
                    { "type": "list", "value": [{ "type": "data", "value": 0.1 }] }
                ] } }),
        );
        far
    });
    let values = peer
        .invoke("svc", "give", Value::Undefined)
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(
        values[0].clone().json().unwrap(),
        json!({ "a": 1 }),
        "data is itself"
    );
    assert_eq!(
        values[1].clone().json().unwrap(),
        json!(9_007_199_254_740_991u64),
        "a bigint that fits a JSON number keeps its value"
    );
    let error = values[2].clone().json().unwrap_err().to_string();
    assert!(
        error.contains("bigint") && error.contains("beyond 64 bits"),
        "{error}"
    );
    let error = values[3].clone().json().unwrap_err().to_string();
    assert!(error.contains("instant"), "{error}");
    let error = values[4].clone().json().unwrap_err().to_string();
    assert!(
        error.contains("bigint") && error.contains("beyond 64 bits"),
        "a record fails as a whole: {error}"
    );
    let float = values[5].clone().json().unwrap()[0].as_f64().unwrap();
    assert_eq!(float.to_bits(), 0.1f64.to_bits(), "a float is exact");
    let _far = far_end(end).await;
    assert!(peer.close_reason().is_none(), "the session goes on");
}

/// A node link lets each kernel manage its own plugins and the other only
/// install through the host (REM-1-3): `office` keeps its own `clock`
/// plugin while `main` loads and unloads a catalog plugin there, a plugin
/// office does not have fails its load, and dropping the row stops it on
/// both sides.
/// rule: REM-1-3
#[tokio::test(flavor = "multi_thread")]
async fn a_node_link_keeps_each_kernels_own_plugins() {
    let main = kernel("main");
    let office = kernel("office");
    let catalog: Arc<dyn PluginCatalog> =
        Arc::new(StaticCatalog::new().with("contacts", Installed::new(Contacts)));
    let _catalog = office.provide_at(catalog_key(), catalog).unwrap();
    let own = office.plugin(Provides {
        service: "clock",
        answer: "noon",
    });
    within("office's clock", async { (&own).await.unwrap() }).await;
    let (ours, theirs) = rutis_bridge::memory::pair();
    let _at_office = office.plugin(Session::new(theirs));
    let _row = main.plugin(RowSession {
        row: "office",
        channel: Mutex::new(Some(ours)),
    });
    let mut found = None;
    until(
        || {
            found = main.get_at(&session_key("office"));
            found.is_some()
        },
        "the session of the row office",
    )
    .await;
    let handle = found.expect("the session handle");
    until(
        || handle.process().is_some(),
        "office's process on the session",
    )
    .await;

    let row = handle.run("contacts", json!({})).unwrap();
    within("the load", row.loaded()).await.unwrap();
    until(
        || main.get_at(&host_key("contacts")).is_some(),
        "the contacts service at main",
    )
    .await;
    assert_eq!(
        call_service::<String>(&main, "contacts", "get", ())
            .await
            .unwrap(),
        "office"
    );
    assert_eq!(
        call_service::<String>(&office, "clock", "get", ())
            .await
            .unwrap(),
        "noon",
        "office's own plugin is untouched"
    );
    let missing = handle.run("missing", json!({})).unwrap();
    let error = within("the missing load", missing.loaded())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not contain plugin missing"), "{error}");

    drop(row);
    until(
        || main.get_at(&host_key("contacts")).is_none(),
        "contacts gone at main",
    )
    .await;
    until(
        || office.get_at(&host_key("contacts")).is_none(),
        "contacts gone at office",
    )
    .await;
    assert_eq!(
        call_service::<String>(&office, "clock", "get", ())
            .await
            .unwrap(),
        "noon",
        "the peer's load and unload leave office's own plugin alone"
    );
}

/// Two rows of one runtime use services natively (SYN-2-10, M1-11.2-1,
/// M1-11.2-3): a row the far end loaded into `office` calls office's own
/// `clock` plugin, and the number of session frames for that call does not
/// depend on how many times it calls it. The far end's by-name call to the
/// row itself is the only thing on the wire (APP-3-1): the same app's
/// plugins exchange the object itself, across a connection the name goes
/// through rutis. (The Node runner's packaging and the SDKs' "cannot tell"
/// are not visible; the checked part is the frame count.)
/// rule: SYN-2-10
/// rule: M1-11.2-1
/// rule: M1-11.2-3
/// rule: APP-3-1
#[tokio::test(flavor = "multi_thread")]
async fn rows_of_one_runtime_use_each_others_services_without_frames() {
    let main = kernel("main");
    let office = kernel("office");
    let catalog: Arc<dyn PluginCatalog> =
        Arc::new(StaticCatalog::new().with("user", Installed::new(User)));
    let _catalog = office.provide_at(catalog_key(), catalog).unwrap();
    let own = office.plugin(Provides {
        service: "clock",
        answer: "noon",
    });
    within("office's clock", async { (&own).await.unwrap() }).await;
    let (ours, theirs, count) = counted_pair();
    let _at_office = office.plugin(Session::new(theirs));
    let _row = main.plugin(RowSession {
        row: "office",
        channel: Mutex::new(Some(ours)),
    });
    let mut found = None;
    until(
        || {
            found = main.get_at(&session_key("office"));
            found.is_some()
        },
        "the session of the row office",
    )
    .await;
    let handle = found.expect("the session handle");
    until(
        || handle.process().is_some(),
        "office's process on the session",
    )
    .await;
    let row = handle.run("user", json!({})).unwrap();
    within("the load", row.loaded()).await.unwrap();
    until(
        || main.get_at(&host_key("user")).is_some(),
        "the user service at main",
    )
    .await;

    // One call, then fifty: the frames on the session are the same.
    count.store(0, Ordering::SeqCst);
    let one = call_service::<Json>(&main, "user", "use_now", vec![1u64])
        .await
        .unwrap();
    let frames_one = count.load(Ordering::SeqCst);
    assert_eq!(one, json!(["noon"]), "the nested call answered");
    count.store(0, Ordering::SeqCst);
    let fifty = call_service::<Json>(&main, "user", "use_now", vec![50u64])
        .await
        .unwrap();
    let frames_fifty = count.load(Ordering::SeqCst);
    assert_eq!(fifty.as_array().unwrap().len(), 50);
    assert!(
        fifty
            .as_array()
            .unwrap()
            .iter()
            .all(|answer| answer == &json!("noon")),
        "{fifty}"
    );
    assert!(
        frames_fifty <= frames_one + 2 && frames_fifty < 20,
        "one nested call: {frames_one} frames, fifty: {frames_fifty}"
    );
    assert!(
        main.get_at(&host_key("user")).is_some(),
        "the far end still calls the row by name"
    );
}

/// A peer's listener is described with the node row's scope (NOD-3-6):
/// the kernel that hears a listener of its node link sees the instance
/// chain the link was mounted in, and `[]` outside an instance; the scope
/// is a descriptor for filters, not a delivery rule.
/// rule: NOD-3-6
#[tokio::test(flavor = "multi_thread")]
async fn a_node_links_listeners_carry_the_links_scope() {
    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    struct Scoped(String);

    impl Event for Scoped {
        const NAME: &'static str = "t/scope";
        type Value = ();
    }

    for instance in [false, true] {
        let a = kernel("a");
        let b = kernel("b");
        let (at_a, at_b, expected) = match instance {
            false => {
                let (ours, theirs) = rutis_bridge::memory::pair();
                let at_b = b.plugin(Session::new(theirs));
                let at_a = a.plugin(Session::new(ours));
                (at_a, at_b, Vec::new())
            }
            true => {
                let (id, ctx) = tenant(&a).await;
                let (ours, theirs) = rutis_bridge::memory::pair();
                let at_b = b.plugin(Session::new(theirs));
                let at_a = ctx.plugin(Session::new(ours));
                (
                    at_a,
                    at_b,
                    vec![Scope {
                        id: id.to_string(),
                        group: "tenant".into(),
                    }],
                )
            }
        };
        let _ = b.on::<Scoped>(|_: Arc<Scoped>, _: Call<Scoped>| async { Ok(Some(())) });
        within("both sessions", async {
            (&at_a).await.unwrap();
            (&at_b).await.unwrap();
        })
        .await;
        let seen: Arc<Mutex<Vec<Vec<Scope>>>> = Arc::default();
        let record = seen.clone();
        until(
            || {
                let record = record.clone();
                let _ = a
                    .filter(move |info| {
                        if info.node.as_deref() == Some("b") && info.name == "t/scope" {
                            record.lock().unwrap().push(info.scope.clone());
                        }
                        true
                    })
                    .emit(Scoped("x".into()));
                !seen.lock().unwrap().is_empty()
            },
            "the peer's listener description",
        )
        .await;
        let scope = seen.lock().unwrap()[0].clone();
        assert_eq!(
            scope, expected,
            "the listener's description carries the link's scope"
        );
    }
}

/// A peer greeting as another id than the one the dialer expects is an
/// identity mismatch (DIA-6-6), not a transport failure: the connection is
/// opened expecting `peer` (the dialer's `--peer`), and the sentence names
/// both ids.
/// rule: DIA-6-6
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_greeting_as_another_id_is_an_identity_mismatch() {
    let (peer, mut far) = session_with(
        Arc::new(Echo),
        Endpoint::rust(id("main")).expect(id("peer")),
    );
    assert_eq!(read(&mut far)["op"], json!("hello"));
    send(
        &mut far,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "other",
                "capabilities": [] }),
    );
    match peer.ready().await {
        Err(Error::Handshake(rutis_bridge::session::Handshake::IdentityMismatch(reason))) => {
            assert!(reason.contains("other"), "{reason}");
            assert!(reason.contains("peer"), "{reason}");
            assert!(reason.contains("expected endpoint"), "{reason}");
        }
        other => panic!("an identity mismatch expected, got {other:?}"),
    }
}

/// The WebSocket carrier and the dial-time categories (DIA-2-5, DIA-6-3,
/// DIA-6-5, DIA-6-7), its framing and size limit (CHN-10-6, CHN-10-7), a
/// listener that admits only a verified controller (REM-2-2), and the node
/// bridge over a real dial and accept in both directions (CHN-1-5).
///
/// The binary-message half of CHN-10-6 has no entry point through the
/// public channel API (the sender writes text only); it stays open. The
/// certificate case covers loopback TLS; a non-loopback `wss://` deployment
/// is the same classifier.
#[cfg(feature = "websocket")]
mod carrier {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener as StdListener};
    use std::sync::mpsc::{channel as std_channel, Sender as StdSender};
    use std::time::Duration;

    use rutis_bridge::session::Handshake;
    use rutis_bridge::websocket::{
        self, Admit, Admitted, Credential, Limits, Listener, ListenerConfig, Refused, ServerTls,
        Trust, Upgrade,
    };
    use rutis_bridge::ConnectError;

    use super::*;

    /// The session protocol the WebSocket binding carries.
    const PROTOCOL: &str = "rutis.4";

    /// An admission that lets token `good` through and turns the rest away.
    fn admitting() -> Admit {
        Arc::new(|upgrade: Upgrade<'_>| match upgrade.bearer {
            Some("good") => Ok(Admitted {
                protocol: PROTOCOL.into(),
                deliver: Box::new(|_| {}),
            }),
            Some(_) => Err(Refused::Forbidden("not this token".into())),
            None => Err(Refused::Unavailable("credentials required".into())),
        })
    }

    /// A listener letting `good` through, handing each channel over.
    fn listening_with(
        protocol: &'static str,
        limits: Limits,
    ) -> (
        Listener,
        String,
        tokio::sync::mpsc::UnboundedReceiver<Channel>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let admit: Admit = Arc::new(move |upgrade: Upgrade<'_>| match upgrade.bearer {
            Some("good") => Ok(Admitted {
                protocol: protocol.into(),
                deliver: Box::new({
                    let sender = sender.clone();
                    move |channel| {
                        let _ = sender.send(channel);
                    }
                }),
            }),
            Some(_) => Err(Refused::Forbidden("not this token".into())),
            None => Err(Refused::Unavailable("credentials required".into())),
        });
        let config = ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap());
        let listener = websocket::listen(&config, &limits, admit).unwrap();
        let address = format!("ws://{}/rutis", listener.local_addr());
        (listener, address, receiver)
    }

    fn listening() -> (
        Listener,
        String,
        tokio::sync::mpsc::UnboundedReceiver<Channel>,
    ) {
        listening_with(PROTOCOL, Limits::default())
    }

    async fn dial_with(
        address: &str,
        token: Option<&str>,
        trust: &Trust,
        limits: &Limits,
    ) -> Result<Channel, ConnectError> {
        let credential = token.map(|token| Credential::Bearer(token.into()));
        websocket::dial(address, PROTOCOL, credential.as_ref(), trust, limits).await
    }

    async fn dial(
        address: &str,
        token: Option<&str>,
        trust: &Trust,
    ) -> Result<Channel, ConnectError> {
        dial_with(address, token, trust, &Limits::default()).await
    }

    /// A raw HTTP listener answering one upgrade with `status`, then
    /// closing: enough to check the dialer's status classification.
    fn http_status(status: &str) -> String {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let address = format!("ws://{}/rutis", listener.local_addr().unwrap());
        let status = status.to_owned();
        std::thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let _ = tcp.read(&mut buffer);
            let _ = write!(
                tcp,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = tcp.shutdown(Shutdown::Both);
        });
        address
    }

    /// An address whose listener accepts the TCP connection and never
    /// answers, and the sender that lets its thread go.
    fn silent() -> (String, StdSender<()>) {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let address = format!("ws://{}/rutis", listener.local_addr().unwrap());
        let (release, held) = std_channel();
        std::thread::spawn(move || {
            let (_tcp, _) = listener.accept().unwrap();
            let _ = held.recv();
        });
        (address, release)
    }

    /// A CA and a certificate for localhost signed by it, as PEM.
    fn pki() -> (String, String, String) {
        use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "rutis test CA");
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);
        let key = KeyPair::generate().unwrap();
        let mut params =
            CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "localhost");
        let certificate = params.signed_by(&key, &issuer).unwrap();
        (ca.pem(), certificate.pem(), key.serialize_pem())
    }

    /// Each frame is one text message (CHN-10-6): the bytes the listener
    /// receives are exactly the frame's UTF-8 JSON, with no newline, and a
    /// second frame is a second message, not appended to the first. (The
    /// binary message's close has no public entry point; it stays open.)
    /// rule: CHN-10-6
    #[tokio::test(flavor = "multi_thread")]
    async fn a_frame_is_one_text_message_without_a_newline() {
        let (_listener, address, mut delivered) = listening();
        let mut dialed = dial(&address, Some("good"), &Trust::default())
            .await
            .unwrap();
        let accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        let first = json!({ "op": "hello", "version": ENDPOINT_PROTOCOL,
                            "endpoint": "main", "capabilities": [] });
        let second = json!({ "op": "call", "id": "a:1", "reference": 1,
                             "method": "get", "args": { "type": "list", "value": [] } });
        dialed
            .sender
            .send(&serde_json::to_vec(&first).unwrap())
            .unwrap();
        dialed
            .sender
            .send(&serde_json::to_vec(&second).unwrap())
            .unwrap();
        let (one, two) = within(
            "the two messages",
            tokio::task::spawn_blocking(move || {
                let mut accepted = accepted;
                (accepted.receiver.recv(), accepted.receiver.recv())
            }),
        )
        .await
        .unwrap();
        let one = one.unwrap().unwrap();
        let two = two.unwrap().unwrap();
        let text = std::str::from_utf8(&one).unwrap();
        assert!(!text.contains('\n'), "{text:?}");
        assert_eq!(serde_json::from_str::<Json>(text).unwrap(), first);
        assert_eq!(serde_json::from_slice::<Json>(&two).unwrap(), second);
    }

    /// A message over the limit closes the channel with 1009 (CHN-10-7):
    /// the default is 16 MiB, a configured limit refuses the send, and the
    /// far end reads the close; a session over the same channel sees one
    /// transport end.
    /// rule: CHN-10-7
    #[tokio::test(flavor = "multi_thread")]
    async fn an_over_limit_message_closes_with_1009() {
        assert_eq!(Limits::default().max_message, 16 * 1024 * 1024);
        let limits = Limits {
            max_message: 256,
            ..Default::default()
        };
        let (_listener, address, mut delivered) = listening_with(PROTOCOL, limits.clone());
        let mut dialed = dial_with(&address, Some("good"), &Trust::default(), &limits)
            .await
            .unwrap();
        let accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        match dialed.sender.send(&vec![b'x'; 512]) {
            Err(ChannelError::Closed { reason }) => {
                assert!(reason.contains("512"), "{reason}");
                assert!(reason.contains("256"), "{reason}");
            }
            other => panic!("a closed channel expected, got {other:?}"),
        }
        let far = within(
            "the far end's close",
            tokio::task::spawn_blocking(move || {
                let mut accepted = accepted;
                accepted.receiver.recv()
            }),
        )
        .await
        .unwrap();
        match far {
            Err(ChannelError::Closed { reason }) => {
                assert!(reason.contains("1009"), "{reason}");
                assert!(reason.contains("512 bytes"), "{reason}");
            }
            other => panic!("the far end sees the close, got {other:?}"),
        }
    }

    /// A session that knows the channel's limit refuses the oversized frame
    /// before the transport (#675): only that call fails, the far end reads
    /// no frame and no close, and the session goes on. (The raw channel's
    /// own limit still closes with 1009, checked above; the rule's "session
    /// sees one Closed" is the path without a reported limit, #675's
    /// deliberate exception.)
    /// rule: CHN-10-7
    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_refuses_an_oversized_frame_before_the_transport() {
        // The greeting is 289 bytes: the limit must let it through and
        // stop the call.
        let limits = Limits {
            max_message: 512,
            ..Default::default()
        };
        let (_listener, address, mut delivered) = listening_with(PROTOCOL, limits.clone());
        let dialed = dial_with(&address, Some("good"), &Trust::default(), &limits)
            .await
            .unwrap();
        let mut accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        let peer = Connection::open(dialed, Arc::new(Echo), Endpoint::rust(id("main"))).unwrap();
        greet(&mut accepted, "runtime", &[]);
        within("the greeting", peer.ready()).await.unwrap();
        let error = within("the oversized call", async {
            peer.invoke("svc", "echo", Value::Data(json!("x".repeat(1024))))
        })
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("exceeds the limit")
                || error.to_string().contains("over the limit"),
            "{error}"
        );
        assert!(
            peer.close_reason().is_none(),
            "the session goes on after the refused call"
        );
        accepted.closer.close(Close::Done);
    }

    /// A non-loopback `ws://` address is refused before anything is dialed
    /// (DIA-2-5): only `wss://` carries a non-loopback connection, and the
    /// error is the incompatible category the dialer stops on.
    /// rule: DIA-2-5
    #[tokio::test(flavor = "multi_thread")]
    async fn a_non_loopback_ws_address_is_refused_before_connecting() {
        match dial("ws://192.0.2.1:80/rutis", None, &Trust::default()).await {
            Err(ConnectError::Incompatible { reason }) => {
                assert!(reason.contains("non-loopback"), "{reason}");
                assert!(reason.contains("wss://"), "{reason}");
            }
            other => panic!("an incompatible address expected, got {other:?}"),
        }
    }

    /// Credential refusals are their own category (DIA-6-3): 401, 403, a
    /// server certificate this side does not trust, and a 4003 after the
    /// hello are authentication failures, while the same listener with its
    /// CA is reached.
    /// rule: DIA-6-3
    #[tokio::test(flavor = "multi_thread")]
    async fn credential_refusals_have_their_own_category() {
        let (_listener, address, _delivered) = listening();
        assert!(
            matches!(dial(&address, None, &Trust::default()).await,
                     Err(ConnectError::AuthRejected { reason }) if reason.contains("401")),
            "no credentials: 401"
        );
        assert!(
            matches!(dial(&address, Some("bad"), &Trust::default()).await,
                     Err(ConnectError::AuthRejected { reason }) if reason.contains("403")),
            "refused credentials: 403"
        );
        // A certificate this side does not trust.
        let (ca, certificate, key) = pki();
        let listener = websocket::listen(
            &ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap()).tls(ServerTls {
                certificate_pem: certificate.into_bytes(),
                key_pem: key.into_bytes(),
                client_ca_pem: None,
            }),
            &Limits::default(),
            admitting(),
        )
        .unwrap();
        let address = format!("wss://127.0.0.1:{}/rutis", listener.local_addr().port());
        match dial(&address, Some("good"), &Trust::default()).await {
            Err(ConnectError::AuthRejected { reason }) => {
                assert!(
                    reason.contains("certificate") || reason.contains("TLS"),
                    "{reason}"
                );
            }
            other => panic!("an untrusted certificate is authentication, got {other:?}"),
        }
        let _trusted = dial(&address, Some("good"), &Trust::only(ca.into_bytes()))
            .await
            .unwrap();
        // 4003 after the hello: the access point refuses this id.
        let (_listener, address, mut delivered) = listening();
        let dialed = dial(&address, Some("good"), &Trust::default())
            .await
            .unwrap();
        let accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        let closing = std::thread::spawn(move || {
            let mut accepted = accepted;
            let hello = read(&mut accepted);
            assert_eq!(hello["op"], json!("hello"), "{hello}");
            accepted
                .closer
                .close(Close::Rejected("this id is taken".into()));
        });
        let root = common::runtime::root();
        let error = Session::handshake(&root, dialed)
            .await
            .err()
            .expect("the refused handshake fails");
        match error {
            Error::Handshake(Handshake::IdentityMismatch(reason)) => {
                assert!(reason.contains("rejected"), "{reason}");
                assert!(reason.contains("this id is taken"), "{reason}");
            }
            other => panic!("a refused id is an identity mismatch, got {other:?}"),
        }
        closing.join().unwrap();
    }

    /// Protocol mismatches are the incompatible category (DIA-6-5): 404,
    /// 405 and 426 at the upgrade, another session protocol, and a far end
    /// greeting with a version this side does not speak.
    /// rule: DIA-6-5
    #[tokio::test(flavor = "multi_thread")]
    async fn protocol_mismatches_are_incompatible() {
        for status in [
            "404 Not Found",
            "405 Method Not Allowed",
            "426 Upgrade Required",
        ] {
            let address = http_status(status);
            match dial(&address, Some("good"), &Trust::default()).await {
                Err(ConnectError::Incompatible { reason }) => {
                    assert!(
                        reason.contains(status.split(' ').next().unwrap()),
                        "{status}: {reason}"
                    );
                }
                other => panic!("{status}: an incompatible upgrade expected, got {other:?}"),
            }
        }
        // A listener speaking another session protocol.
        let (_listener, address, _delivered) = listening_with("rutis.3", Limits::default());
        match dial(&address, Some("good"), &Trust::default()).await {
            Err(ConnectError::Incompatible { reason }) => {
                assert!(reason.contains("400"), "{reason}");
                assert!(reason.contains("rutis.3"), "{reason}");
            }
            other => panic!("a protocol refusal is incompatible, got {other:?}"),
        }
        // The far end's hello names a protocol this side does not speak.
        let root = common::runtime::root();
        let (ours, theirs) = rutis_bridge::memory::pair();
        let end = std::thread::spawn(move || {
            let mut far = theirs;
            let hello = read(&mut far);
            assert_eq!(hello["op"], json!("hello"), "{hello}");
            send(
                &mut far,
                json!({ "op": "hello", "version": ENDPOINT_PROTOCOL + 1,
                        "endpoint": "node", "capabilities": [] }),
            );
            far
        });
        let error = Session::handshake(&root, ours)
            .await
            .err()
            .expect("the mismatched handshake fails");
        match error {
            Error::Handshake(Handshake::Incompatible(reason)) => {
                assert!(reason.contains("the far end speaks protocol"), "{reason}");
                assert!(reason.contains("upgrade"), "{reason}");
            }
            other => panic!("a version mismatch is incompatible, got {other:?}"),
        }
        let _far = far_end(end).await;
    }

    /// The rest of the failures are retryable (DIA-6-7): a connection
    /// nobody answers, a listener that never answers within the handshake
    /// limit, a normal dropout, and 4002 from a newer connection.
    /// rule: DIA-6-7
    #[tokio::test(flavor = "multi_thread")]
    async fn retryable_failures_are_the_rest() {
        let probe = StdListener::bind("127.0.0.1:0").unwrap();
        let address = format!("ws://{}/rutis", probe.local_addr().unwrap());
        drop(probe);
        match dial(&address, Some("good"), &Trust::default()).await {
            Err(ConnectError::Retryable { reason }) => {
                assert!(
                    reason.contains("refused") || reason.contains("refusing"),
                    "{reason}"
                );
            }
            other => panic!("a refused connection is retryable, got {other:?}"),
        }
        let (address, release) = silent();
        let limits = Limits {
            handshake: Duration::from_millis(100),
            ..Default::default()
        };
        match dial_with(&address, Some("good"), &Trust::default(), &limits).await {
            Err(ConnectError::Retryable { reason }) => {
                assert!(reason.contains("timed out"), "{reason}")
            }
            other => panic!("a silent listener is a timeout, got {other:?}"),
        }
        let _ = release.send(());
        let (_listener, address, mut delivered) = listening();
        let dialed = dial(&address, Some("good"), &Trust::default())
            .await
            .unwrap();
        let accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        accepted.closer.close(Close::Done);
        let end = within(
            "the far end's end",
            tokio::task::spawn_blocking(move || {
                let mut dialed = dialed;
                dialed.receiver.recv()
            }),
        )
        .await
        .unwrap();
        assert_eq!(end, Ok(None), "a normal close is the channel's end");
        let dialed = dial(&address, Some("good"), &Trust::default())
            .await
            .unwrap();
        let accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        accepted.closer.close(Close::Replaced);
        let end = within(
            "the far end's end",
            tokio::task::spawn_blocking(move || {
                let mut dialed = dialed;
                dialed.receiver.recv()
            }),
        )
        .await
        .unwrap();
        match end {
            Err(ChannelError::Closed { reason }) => {
                assert!(reason.contains("replaced"), "{reason}")
            }
            other => panic!("4002 is a closed channel, got {other:?}"),
        }
    }

    /// A listener admits only a controller it can verify (REM-2-2):
    /// without a token the upgrade is refused with 401, a token it does
    /// not accept with 403, and the accepted one starts a session.
    /// rule: REM-2-2
    #[tokio::test(flavor = "multi_thread")]
    async fn a_listener_admits_only_a_verified_controller() {
        let (_listener, address, mut delivered) = listening();
        assert!(
            matches!(dial(&address, None, &Trust::default()).await,
                     Err(ConnectError::AuthRejected { reason }) if reason.contains("401")),
            "no credentials: 401"
        );
        assert!(
            matches!(dial(&address, Some("bad"), &Trust::default()).await,
                     Err(ConnectError::AuthRejected { reason }) if reason.contains("403")),
            "a token it does not accept: 403"
        );
        let root = common::runtime::root();
        let dialed = dial(&address, Some("good"), &Trust::default())
            .await
            .unwrap();
        let accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        let end = std::thread::spawn(move || {
            let mut accepted = accepted;
            greet(&mut accepted, "runtime", &[]);
            accepted
        });
        let greeted = within("the handshake", Session::handshake(&root, dialed))
            .await
            .unwrap();
        assert_eq!(greeted.id(), "runtime");
        let _far = far_end(end).await;
    }

    /// Node features do not depend on who dialed (CHN-1-5): over one
    /// WebSocket connection the listener runs a plugin on the dialer and
    /// the dialer runs one on the listener, each from its own catalog. (The
    /// old feature plugins are the session's families now; the checked part
    /// is that both ends offer the node bridge, whichever accepted.)
    /// rule: CHN-1-5
    #[tokio::test(flavor = "multi_thread")]
    async fn node_features_do_not_depend_on_who_dialed() {
        let (listener, address, mut delivered) = listening();
        let a = kernel("a");
        let b = kernel("b");
        let catalog: Arc<dyn PluginCatalog> =
            Arc::new(StaticCatalog::new().with("contacts", Installed::new(Contacts)));
        let _ca = a.provide_at(catalog_key(), catalog).unwrap();
        let catalog: Arc<dyn PluginCatalog> =
            Arc::new(StaticCatalog::new().with("contacts", Installed::new(Contacts)));
        let _cb = b.provide_at(catalog_key(), catalog).unwrap();
        let dialed = dial(&address, Some("good"), &Trust::default())
            .await
            .unwrap();
        let accepted = within("the accepted channel", delivered.recv())
            .await
            .unwrap();
        let (greeted_a, greeted_b) = tokio::join!(
            Session::handshake(&a, accepted),
            Session::handshake(&b, dialed)
        );
        let greeted_a = greeted_a.unwrap();
        let greeted_b = greeted_b.unwrap();
        assert_eq!(greeted_a.id(), "b", "the listener sees the dialer");
        assert_eq!(greeted_b.id(), "a", "the dialer sees the listener");
        let handle_a = greeted_a.handle();
        let handle_b = greeted_b.handle();
        let _view_a = a.plugin(greeted_a);
        let _view_b = b.plugin(greeted_b);
        until(|| handle_a.process().is_some(), "a's process on b").await;
        until(|| handle_b.process().is_some(), "b's process on a").await;
        let at_b = handle_a.run("contacts", json!({})).unwrap();
        let at_a = handle_b.run("contacts", json!({})).unwrap();
        within("the loads", async {
            at_b.loaded().await.unwrap();
            at_a.loaded().await.unwrap();
        })
        .await;
        until(
            || a.get_at(&host_key("contacts")).is_some(),
            "the contacts service at a",
        )
        .await;
        until(
            || b.get_at(&host_key("contacts")).is_some(),
            "the contacts service at b",
        )
        .await;
        assert_eq!(
            call_service::<String>(&a, "contacts", "get", ())
                .await
                .unwrap(),
            "office",
            "the listener runs the dialer's plugin"
        );
        assert_eq!(
            call_service::<String>(&b, "contacts", "get", ())
                .await
                .unwrap(),
            "office",
            "the dialer runs the listener's plugin"
        );
        drop(listener);
    }
}
