//! guarantee: bridge/grant-rollback (risk P4, #631). A call whose value
//! cannot cross fails on the side that would send it, explicitly, before a
//! byte goes out, and every reference granted while encoding it is given
//! back: both sides' reference tables are where they were, and the
//! reference still works afterwards.
//!
//! The guarantee names Node and Python only; the Rust side had no such test.
//! Here it is checked for a value past the depth limit, a value the far end
//! did not declare a capability for, a message over the channel's limit
//! (granted, then found too large after encoding), and a result that cannot
//! be sent (the far end's own grant). The property test then runs any
//! sequence of grants, releases and such refusals and checks that each
//! side's tables hold exactly the references the other side still keeps, and
//! a concurrent run checks the same while several threads share the
//! callbacks a refused call grants.
//!
//! `PROPTEST_CASES` cases (256 by default); a failure prints the seed, rerun
//! with `RUTIS_PROPTEST_SEED=<seed>` to replay it (Q7.2).

#![cfg(feature = "testing")]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Instant;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use rutis_bridge::channel::{Channel, ChannelError, PeerId, Sender};
use rutis_bridge::memory;
use rutis_bridge::session::testing::Fixture;
use rutis_bridge::session::{
    decode_value, Connection, Dispatch, Endpoint, Error, Reply, Value, MAX_DEPTH,
};
use serde_json::json;

fn id(name: &str) -> PeerId {
    PeerId::new(name).unwrap()
}

// ── A session with what the near side wrote counted ──────────────────

/// The near side's sender: counts the messages written, and tells the
/// session the channel takes no more than `limit` when one is set.
struct Metered {
    inner: Box<dyn Sender>,
    sent: Arc<AtomicUsize>,
    limit: Option<usize>,
}

impl Sender for Metered {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        self.inner.send(message)?;
        self.sent.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn max_message(&self) -> Option<usize> {
        self.limit
    }
}

/// This side serves nothing: the tests only call the far end.
struct Nothing;

impl Dispatch for Nothing {
    fn invoke(&self, _: &Connection, target: &str, method: &str, _: Value) -> Reply {
        Err(Error::value(format!(
            "main serves nothing: {target}.{method}"
        )))
    }
}

/// Two sessions over an in-process channel, `near` a session that calls
/// `far`, with every message `near` writes counted.
struct Link {
    near: Connection,
    far: Connection,
    sent: Arc<AtomicUsize>,
}

impl Link {
    fn open(limit: Option<usize>, far: Arc<dyn Dispatch>, far_endpoint: Endpoint) -> Self {
        let (near_channel, far_channel) = memory::pair();
        let far = Connection::open(far_channel, far, far_endpoint.expect(id("main"))).unwrap();
        let Channel {
            sender,
            receiver,
            closer,
        } = near_channel;
        let sent = Arc::new(AtomicUsize::new(0));
        let near_channel = Channel {
            sender: Box::new(Metered {
                inner: sender,
                sent: sent.clone(),
                limit,
            }),
            receiver,
            closer,
        };
        let near = Connection::open(
            near_channel,
            Arc::new(Nothing),
            Endpoint::rust(id("main")).expect(id("far")),
        )
        .unwrap();
        Self { near, far, sent }
    }

    fn fixture() -> Self {
        Self::open(
            None,
            Arc::new(Fixture::default()),
            Endpoint::rust(id("far")),
        )
    }

    /// Messages `near` wrote since the session opened, its greeting included.
    fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }

    /// (near exports, near imports), (far exports, far imports).
    fn tables(&self) -> ((usize, usize), (usize, usize)) {
        (self.near.references(), self.far.references())
    }

    /// A round trip with no references and no state: after it, each side has
    /// taken in everything the other wrote before the call was made, so the
    /// tables can be compared.
    fn settle(&self) {
        let value = self
            .near
            .invoke(
                "conformance",
                "echo",
                Value::List(vec![Value::Data(json!(0))]),
            )
            .expect("the session goes on");
        assert_eq!(value.json().unwrap(), json!(0));
    }
}

// ── Values that cannot cross ─────────────────────────────────────────

/// `depth` lists deep around `inner`.
fn nested(depth: usize, inner: Value) -> Value {
    (0..depth).fold(inner, |inner, _| Value::List(vec![inner]))
}

/// A value past the depth limit (`MAX_DEPTH` levels, #279).
fn too_deep() -> Value {
    nested(MAX_DEPTH + 2, Value::Data(json!("bottom")))
}

/// A callback doubling the number it is given, as real Rust code hands one
/// over: it proves the reference still runs after a rollback.
fn doubling() -> Value {
    Value::callback(|args| {
        let [number]: [i64; 1] = decode_value(args)?;
        Ok(json!(number * 2).into())
    })
}

/// A call carrying `callbacks[slot]` and a value that cannot cross, in
/// `shape`:
///
/// - 0: the callback first, then a value past the depth limit;
/// - 1: both in a record, the callback first;
/// - 2: the value that cannot cross first, the callback after it;
/// - 3: the callback granted twice, then a value past the depth limit.
fn refused(slot: u8, shape: u8, callbacks: &[Value]) -> Value {
    let callback = callbacks[slot as usize].clone();
    match shape % 4 {
        0 => Value::List(vec![callback, too_deep()]),
        1 => {
            let mut fields = BTreeMap::new();
            fields.insert("a".to_owned(), callback);
            fields.insert("z".to_owned(), too_deep());
            Value::Record(fields)
        }
        2 => Value::List(vec![too_deep(), callback]),
        _ => Value::List(vec![
            callback.clone(),
            Value::List(vec![callback, too_deep()]),
        ]),
    }
}

/// The call failed on this side, before anything was written: a value error
/// with `reason`, saying `text`, its code `bad-value`.
fn unsendable(error: Error, reason: Option<&str>, text: &str, what: &str) {
    assert_eq!(
        error.code(),
        Some(rutis_bridge::session::code::BAD_VALUE),
        "{what}: {error}"
    );
    match error {
        Error::Value {
            message,
            reason: why,
        } => {
            assert_eq!(why, reason, "{what}: {message}");
            assert!(
                message.contains(text),
                "{what}: {message} does not say {text:?}"
            );
        }
        other => panic!("{what}: a value that cannot cross expected, got {other:?}"),
    }
}

// ── This side's grants ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_value_that_cannot_cross_grants_nothing() {
    let link = Link::fixture();
    link.near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || {
        let (near_before, far_before) = link.tables();
        let sent_before = link.sent();
        let callback = doubling();
        let callbacks = vec![callback.clone()];
        for shape in 0..4 {
            let what = format!("shape {shape}");
            let error = link
                .near
                .invoke("conformance", "echo", refused(0, shape, &callbacks))
                .expect_err(&what);
            unsendable(error, Some("too-deep"), "64 levels", &what);
            assert_eq!(
                link.tables(),
                (near_before, far_before),
                "{what}: a grant was left behind"
            );
            assert_eq!(link.sent(), sent_before, "{what}: bytes went out");
            assert!(
                link.near.close_reason().is_none(),
                "{what}: the session ended"
            );
        }
        // The rolled-back reference still reaches the far end and runs, and
        // letting it go leaves the tables where they were.
        let doubled = link
            .near
            .invoke(
                "conformance",
                "apply",
                Value::List(vec![callback, Value::Data(json!(21))]),
            )
            .unwrap();
        assert_eq!(doubled.json().unwrap(), json!(42));
        link.settle();
        assert_eq!(link.tables(), (near_before, far_before));
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_without_its_capability_grants_nothing() {
    // A far end older than the value it is sent (#308): it declared no
    // `value-*` capability.
    let mut far = Endpoint::rust(id("far"));
    far.capabilities
        .retain(|capability| !capability.starts_with("value-"));
    let link = Link::open(None, Arc::new(Fixture::default()), far);
    link.near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || {
        let (near_before, far_before) = link.tables();
        let sent_before = link.sent();
        let value = Value::List(vec![doubling(), Value::Bytes(vec![1, 2, 3])]);
        let error = link
            .near
            .invoke("conformance", "echo", value)
            .expect_err("a bytes value to a far end that declared no value-bytes");
        // `sendable` refuses a tagged value without a reason of its own
        // (design §4.2): the message names the value and the capability.
        unsendable(
            error,
            None,
            "did not declare the capability value-bytes",
            "bytes",
        );
        assert_eq!(
            link.tables(),
            (near_before, far_before),
            "the grant made before the refusal was left behind"
        );
        assert_eq!(link.sent(), sent_before, "bytes went out");
        // Data still crosses; the session goes on.
        link.settle();
        assert_eq!(link.tables(), (near_before, far_before));
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_over_the_channel_limit_grants_nothing() {
    const LIMIT: usize = 2048;
    // A far end that takes no chunks: the channel's per-message limit is what
    // refuses the call, and the grants made while encoding go back (#1361
    // lifts that limit only between two ends that declared `chunked`).
    let mut far = Endpoint::rust(id("far"));
    far.capabilities
        .retain(|capability| capability != "chunked");
    let link = Link::open(Some(LIMIT), Arc::new(Fixture::default()), far);
    link.near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || {
        let (near_before, far_before) = link.tables();
        let sent_before = link.sent();
        // Encoded in full, grants made, then found over the channel's limit
        // (#675): the grants are given back too, not only on encode errors.
        let value = Value::List(vec![doubling(), Value::Data(json!("x".repeat(LIMIT * 4)))]);
        let error = link
            .near
            .invoke("conformance", "echo", value)
            .expect_err("a message over the channel's limit");
        unsendable(
            error,
            Some("limit"),
            &format!("over the limit of {LIMIT} bytes"),
            "over the limit",
        );
        assert_eq!(
            link.tables(),
            (near_before, far_before),
            "the grant made while encoding was left behind"
        );
        assert_eq!(link.sent(), sent_before, "bytes went out");
        // What fits still crosses on the same channel.
        link.near
            .invoke(
                "conformance",
                "keep",
                Value::List(vec![Value::Data(json!(0)), doubling()]),
            )
            .unwrap();
        link.near
            .invoke(
                "conformance",
                "free",
                Value::List(vec![Value::Data(json!(0))]),
            )
            .unwrap();
        link.settle();
        assert_eq!(link.tables(), (near_before, far_before));
        assert!(link.near.close_reason().is_none());
    })
    .await
    .unwrap();
}

// ── The far end's grant in a result ──────────────────────────────────

/// A far end that answers with a reference and, in the same result, a value
/// that cannot cross.
struct Results;

impl Dispatch for Results {
    fn invoke(&self, _: &Connection, target: &str, method: &str, _: Value) -> Reply {
        if target != "results" {
            return Err(Error::value(format!("no target {target}")));
        }
        match method {
            // The reference is granted while the result is encoded; the
            // value past the limit fails the encode after it.
            "unsendable" => Ok(Value::List(vec![
                Value::callback(|_| Ok(Value::Undefined)),
                too_deep(),
            ])),
            "function" => Ok(Value::callback(|_| Ok(Value::Undefined))),
            "ping" => Ok(json!("pong").into()),
            other => Err(Error::value(format!("no method {other}"))),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_that_cannot_cross_grants_nothing() {
    let link = Link::open(None, Arc::new(Results), Endpoint::rust(id("far")));
    link.near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || {
        let (near_before, far_before) = link.tables();
        let sent_before = link.sent();
        // The far end granted a reference and then could not send its
        // result: it says so, its grant given back before the error.
        match link
            .near
            .invoke("results", "unsendable", Value::List(vec![]))
        {
            Err(Error::Remote { name, message, .. }) => {
                assert_eq!(name, "BindingError", "{message}");
                assert!(
                    message.contains("the result cannot be sent"),
                    "{message} does not say the result could not be sent"
                );
                assert!(message.contains("64 levels"), "{message}");
            }
            other => panic!("a result that cannot cross expected an error, got {other:?}"),
        }
        assert_eq!(
            link.tables(),
            (near_before, far_before),
            "the far end's grant was left behind"
        );
        assert_eq!(link.sent(), sent_before + 1, "the call itself went out");
        assert!(link.near.close_reason().is_none(), "the session ended");
        // A function still crosses, and letting it go releases it.
        let function = link
            .near
            .invoke("results", "function", Value::List(vec![]))
            .unwrap();
        assert_eq!(link.far.references(), (1, 0), "the far end exports it");
        assert_eq!(link.near.references(), (0, 1), "this side imports it");
        drop(function);
        // A round trip after the drop: the release is taken in first.
        link.near
            .invoke("results", "ping", Value::List(vec![]))
            .unwrap();
        assert_eq!(link.tables(), (near_before, far_before));
    })
    .await
    .unwrap();
}

// ── Any sequence of grants, releases and refusals ────────────────────

const SLOTS: u8 = 4;

#[derive(Clone, Debug)]
enum Op {
    /// Pass callback `slot` to the far end, which keeps it in `slot`.
    Keep(u8),
    /// The far end lets go of what it keeps in `slot`.
    Free(u8),
    /// A call carrying callback `slot` and a value that cannot cross.
    Refuse { slot: u8, shape: u8 },
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let slot = 0..SLOTS;
    let op = prop_oneof![
        3 => slot.clone().prop_map(Op::Keep),
        3 => slot.clone().prop_map(Op::Free),
        4 => (slot, 0u8..4).prop_map(|(slot, shape)| Op::Refuse { slot, shape }),
    ];
    prop::collection::vec(op, 0..16)
}

/// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED` or a
/// fresh one; on failure, panic with the seed and the shrunk input.
fn check<S: Strategy>(name: &str, strategy: S, test: impl Fn(S::Value) -> Result<(), TestCaseError>)
where
    S::Value: std::fmt::Debug,
{
    let seed: u64 = match std::env::var("RUTIS_PROPTEST_SEED") {
        Ok(seed) => seed.parse().expect("RUTIS_PROPTEST_SEED is a u64"),
        Err(_) => {
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            );
            hasher.finish()
        }
    };
    let mut bytes = [0u8; 32];
    for chunk in bytes.chunks_mut(8) {
        chunk.copy_from_slice(&seed.to_le_bytes());
    }
    // `Config::default()` reads PROPTEST_CASES (256 when unset).
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let cases = config.cases;
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    let started = Instant::now();
    if let Err(error) = runner.run(&strategy, test) {
        panic!("{name}: {error}\nreplay with RUTIS_PROPTEST_SEED={seed}");
    }
    eprintln!(
        "{name}: {cases} cases in {:?} (seed {seed})",
        started.elapsed()
    );
}

/// The tables after `what` hold exactly what the far end keeps (`kept`):
/// this side exports it, the far end imports it, and nothing is imported
/// here or exported there. The far end serves no references of its own.
fn exact(link: &Link, kept: &BTreeSet<u8>, what: &str) -> Result<(), TestCaseError> {
    link.settle();
    let expected = ((kept.len(), 0), (0, kept.len()));
    let actual = link.tables();
    if actual != expected {
        return Err(TestCaseError::fail(format!(
            "after {what}: tables {actual:?}, expected {expected:?} (far keeps {kept:?})"
        )));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn grants_releases_and_refusals_leave_the_tables_exact() {
    tokio::task::spawn_blocking(move || {
        // A link of its own for every case: proptest runs a case, then the
        // failing one smaller and smaller, and a link the failing case left
        // mid-way would make the shrink fail at its first check (`the
        // start`) with the references of the case before (#1080).
        check("grants and refusals", ops(), |ops| {
            let link = Link::fixture();
            tokio::runtime::Handle::current()
                .block_on(link.near.ready())
                .expect("the link's handshake");
            let callbacks: Vec<Value> = (0..SLOTS).map(|_| doubling()).collect();
            let call = |method: &str, args: Vec<Value>, what: &str| {
                link.near
                    .invoke("conformance", method, Value::List(args))
                    .map_err(|error| TestCaseError::fail(format!("{what}: {error}")))
            };
            let mut kept = BTreeSet::new();
            exact(&link, &kept, "the start")?;
            for (index, op) in ops.iter().enumerate() {
                let what = format!("op {index} {op:?}");
                let sent = link.sent();
                match op {
                    Op::Keep(slot) => {
                        call(
                            "keep",
                            vec![Value::Data(json!(slot)), callbacks[*slot as usize].clone()],
                            &what,
                        )?;
                        kept.insert(*slot);
                        if link.sent() != sent + 1 {
                            return Err(TestCaseError::fail(format!(
                                "{what}: {} messages went out, expected one",
                                link.sent() - sent
                            )));
                        }
                    }
                    Op::Free(slot) => {
                        call("free", vec![Value::Data(json!(slot))], &what)?;
                        kept.remove(slot);
                        if link.sent() != sent + 1 {
                            return Err(TestCaseError::fail(format!(
                                "{what}: {} messages went out, expected one",
                                link.sent() - sent
                            )));
                        }
                    }
                    Op::Refuse { slot, shape } => {
                        match link.near.invoke(
                            "conformance",
                            "echo",
                            refused(*slot, *shape, &callbacks),
                        ) {
                            Err(Error::Value { message, .. }) if message.contains("64 levels") => {}
                            other => {
                                return Err(TestCaseError::fail(format!(
                                    "{what}: a value past the limit expected, got {other:?}"
                                )))
                            }
                        }
                        // Refused before anything was written: the grant
                        // never reached the far end.
                        if link.sent() != sent {
                            return Err(TestCaseError::fail(format!(
                                "{what}: {} messages went out, expected none",
                                link.sent() - sent
                            )));
                        }
                    }
                }
                exact(&link, &kept, &what)?;
            }
            // Let everything go: both sides are where they started.
            for slot in std::mem::take(&mut kept) {
                call(
                    "free",
                    vec![Value::Data(json!(slot))],
                    "releasing everything",
                )?;
            }
            exact(&link, &kept, "releasing everything")
        });
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_refusals_grant_nothing() {
    const THREADS: usize = 8;
    const CALLS: usize = 32;
    let link = Link::fixture();
    link.near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || {
        let callbacks: Vec<Value> = (0..SLOTS).map(|_| doubling()).collect();
        let (near_before, far_before) = link.tables();
        let sent_before = link.sent();
        let start = Arc::new(Barrier::new(THREADS));
        let threads: Vec<_> = (0..THREADS)
            .map(|thread| {
                let near = link.near.clone();
                let callbacks = callbacks.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    for call in 0..CALLS {
                        let shape = ((thread + call) % 4) as u8;
                        let slot = (call % SLOTS as usize) as u8;
                        match near.invoke("conformance", "echo", refused(slot, shape, &callbacks)) {
                            Err(Error::Value { message, .. }) if message.contains("64 levels") => {}
                            other => panic!(
                                "thread {thread} call {call}: a value past the limit expected, \
                                 got {other:?}"
                            ),
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            link.sent(),
            sent_before,
            "bytes went out while refusals shared callbacks"
        );
        assert_eq!(
            link.tables(),
            (near_before, far_before),
            "a grant leaked or was counted twice"
        );
        assert!(link.near.close_reason().is_none(), "the session ended");
        // The same callbacks still cross once, and let go.
        link.near
            .invoke(
                "conformance",
                "keep",
                Value::List(vec![Value::Data(json!(0)), callbacks[0].clone()]),
            )
            .unwrap();
        link.near
            .invoke(
                "conformance",
                "free",
                Value::List(vec![Value::Data(json!(0))]),
            )
            .unwrap();
        link.settle();
        assert_eq!(link.tables(), (near_before, far_before));
    })
    .await
    .unwrap();
}
