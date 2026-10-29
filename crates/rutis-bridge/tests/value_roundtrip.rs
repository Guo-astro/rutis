//! Property tests across the session (#178, part 5): data values generated
//! on the Rust side round-trip through each runtime's `conformance` echo
//! (risk P7, C5); references granted and released in any order leave both
//! reference tables at the number not yet released (risk P4).
//!
//! Each runtime starts once per test and runs `PROPTEST_CASES` cases (256
//! by default). A failure prints the shrunk value and the seed; rerun with
//! `RUTIS_PROPTEST_SEED=<seed>` to replay it (Q7.2).
//!
//! The comparison follows the value table in `docs/guide/plugin-api.md`:
//! data crosses unchanged, except that numbers in JavaScript (Node, Bun)
//! are IEEE 754 doubles. There an integer beyond ±(2^53−1) comes back as the
//! nearest double, -0 as 0 and an integral float (`2.0`) as an integer;
//! the table lists these as not guaranteed. Every other float comes back
//! bit for bit (#290), and every object key as sent (#302). An integer
//! beyond ±(2^53−1) crosses as a bigint (design V6): to a far end that did
//! not declare `value-bigint`, a value holding one fails on this side
//! before it is sent, naming the capability. NaN and ±Infinity are not
//! JSON and are not generated.
//!
//! Values nest up to the limit, 64 levels (`MAX_DEPTH`). Past it, the
//! side that would send the value fails that call, whichever side it is,
//! with an error naming the limit, and the session goes on (#279): each
//! runtime is checked as the sender of a value too deep, in an argument
//! and in a result.
//!
//! Tagged values (#308): the ten values beyond JSON (instant, datetime,
//! date, time, bytes, map, set, bigint, decimal, uuid), mixed with data,
//! lists and records, map keys and set elements of the kinds design §5
//! allows and none equal, round-trip through a far end that declared their
//! capabilities, unchanged but for integers beyond ±(2^53−1) in data,
//! which come back as the bigints they crossed as; to one that did not, each such call fails
//! on this side before it is sent, naming the capability, and the session
//! goes on. Node and Bun take them as JavaScript does (design §2): their
//! echo sends back an instant to the millisecond and a datetime, date,
//! time, decimal or uuid as its text, and refuses a map or set whose keys
//! JavaScript cannot hold apart.
#![cfg(all(unix, feature = "testing"))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::testing::Fixture;
use rutis_bridge::session::{Connection, Endpoint, Error, Value, MAX_DEPTH};
use serde_json::{json, Number, Value as Json};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// No calls into `main`: the tests only call the far end.
struct Nothing;
impl rutis_bridge::session::Dispatch for Nothing {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> rutis_bridge::session::Reply {
        Err(rutis_bridge::session::Error::Value(
            "main serves nothing".into(),
        ))
    }
}

fn main_endpoint(far: &str) -> Endpoint {
    Endpoint::rust(id("main")).expect(id(far))
}

// ── Far ends (as in runtime_conformance.rs) ──────────────────────────

/// A Rust far end serving [`Fixture`] over an in-memory channel.
fn rust_far_end() -> (Connection, Connection) {
    rust_far_end_as(Endpoint::rust(id("rust")))
}

/// [`rust_far_end`], greeting as `far` (with its capabilities).
fn rust_far_end_as(far: Endpoint) -> (Connection, Connection) {
    let (a, b) = rutis_bridge::memory::pair();
    let far = Connection::open(b, Arc::new(Fixture::default()), far.expect(id("main"))).unwrap();
    (
        Connection::open(a, Arc::new(Nothing), main_endpoint("rust")).unwrap(),
        far,
    )
}

/// Listen on a Unix socket, start `command` with it, open the session.
async fn far_end(
    command: tokio::process::Command,
    far: &str,
) -> (Connection, tokio::process::Child) {
    far_end_with(command, main_endpoint(far)).await
}

/// `main` as a far end that takes no objects with methods, as every
/// runtime is: what the far end would send it fails (#675).
fn main_without_objects(far: &str) -> Endpoint {
    let mut endpoint = Endpoint::rust(id("main")).expect(id(far));
    endpoint
        .capabilities
        .retain(|capability| capability != "objects");
    endpoint
}

/// [`far_end`], with this side greeting in `format`.
async fn far_end_with(
    mut command: tokio::process::Command,
    format: Endpoint,
) -> (Connection, tokio::process::Child) {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("roundtrip.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let child = command.arg(&socket).kill_on_drop(true).spawn().unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let stream = stream.into_std().unwrap();
    stream.set_nonblocking(false).unwrap();
    let reader = stream.try_clone().unwrap();
    struct Shut(std::os::unix::net::UnixStream);
    impl rutis_bridge::channel::Closer for Shut {
        fn close(&self, _: rutis_bridge::Close) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }
    let closer = Arc::new(Shut(stream.try_clone().unwrap()));
    let channel = rutis_bridge::pipe::framed(reader, stream, closer);
    let connection = Connection::open(channel, Arc::new(Nothing), format).unwrap();
    connection.ready().await.unwrap();
    (connection, child)
}

fn node() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("node");
    command
        .args(["--import", "tsx"])
        .arg(repo().join("node/rutis-runtime/test/fixtures/conformance-session.mjs"))
        .current_dir(repo().join("node/rutis-runtime"));
    command
}

fn python() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("python3");
    command
        .arg(repo().join("python/rutis/tests/conformance_session.py"))
        .env("PYTHONPATH", repo().join("python/rutis"));
    command
}

// ── Running a property with a printed seed (Q7.2) ────────────────────

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

// ── Values ───────────────────────────────────────────────────────────

const SAFE: i64 = (1 << 53) - 1;

fn string() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[a-z\n\r\t\"\\\\ ]{0,8}",
        prop::sample::select(vec![
            "",
            "\n",
            "a\r\nb",
            "\0",
            "\u{7f}",
            "\u{2028}\u{2029}",
            "\u{feff}",
            "é",
            "😀",
            "\u{10ffff}",
            "\u{ffff}",
            "𝄞 clef",
            "\\u0000",
        ])
        .prop_map(String::from),
    ]
}

fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => string(),
        1 => prop::sample::select(vec![
            "__proto__", "constructor", "prototype", "type", "value", "toString", "",
        ])
        .prop_map(String::from),
    ]
}

fn number() -> impl Strategy<Value = Json> {
    prop_oneof![
        any::<i64>().prop_map(|n| json!(n)),
        any::<u64>().prop_map(|n| json!(n)),
        // Around ±2^53: the last exact integers in a double and the first
        // past them.
        (-3i64..=3, any::<bool>()).prop_map(|(offset, negative)| {
            let n = SAFE + 1 + offset;
            json!(if negative { -n } else { n })
        }),
        prop::sample::select(vec![
            json!(0),
            json!(-1),
            json!(i64::MIN),
            json!(i64::MAX),
            json!(u64::MAX),
        ]),
        any::<f64>().prop_filter_map("finite", |f| Number::from_f64(f).map(Json::Number)),
        prop::sample::select(vec![
            -0.0,
            0.0,
            2.0,
            0.1,
            1e21,
            1e-7,
            5e-324,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
            9007199254740993.0,
        ])
        .prop_map(|f| json!(f)),
    ]
}

fn leaf() -> impl Strategy<Value = Json> {
    prop_oneof![
        Just(Json::Null),
        any::<bool>().prop_map(Json::Bool),
        number(),
        string().prop_map(Json::String),
    ]
}

/// Values nested up to 8 levels, plus single chains up to the limit
/// (`MAX_DEPTH`, 64 levels).
fn value() -> impl Strategy<Value = Json> {
    let nested = leaf().prop_recursive(8, 64, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Json::Array),
            prop::collection::btree_map(key(), inner, 0..4)
                .prop_map(|fields| Json::Object(fields.into_iter().collect())),
        ]
    });
    let deep =
        (leaf(), prop::collection::vec(any::<bool>(), 0..=MAX_DEPTH)).prop_map(|(leaf, levels)| {
            levels.into_iter().fold(leaf, |inner, array| {
                if array {
                    json!([inner])
                } else {
                    json!({ "k": inner })
                }
            })
        });
    prop_oneof![8 => nested, 1 => deep]
}

/// How a runtime carries numbers.
#[derive(Clone, Copy)]
enum Numbers {
    /// As sent: integer or float, the same value, a float bit for bit
    /// (-0 kept).
    Exact,
    /// As IEEE 754 doubles (JavaScript).
    Doubles,
}

/// Whether `got` is what `sent` must come back as, or where it differs.
fn same(sent: &Json, got: &Json, numbers: Numbers, at: &str) -> Result<(), String> {
    let differ = || Err(format!("at {at}: sent {sent}, got {got}"));
    match (sent, got) {
        (Json::Number(a), Json::Number(b)) => {
            let (x, y) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            let equal = match numbers {
                Numbers::Exact => match (a.is_f64(), b.is_f64()) {
                    (true, true) => x.to_bits() == y.to_bits(),
                    (false, false) => a == b,
                    _ => false,
                },
                // The double nearest the sent number; -0 and 0 alike.
                Numbers::Doubles => x == y,
            };
            if equal {
                Ok(())
            } else {
                differ()
            }
        }
        (Json::Array(a), Json::Array(b)) => {
            if a.len() != b.len() {
                return differ();
            }
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                same(a, b, numbers, &format!("{at}[{index}]"))?;
            }
            Ok(())
        }
        (Json::Object(a), Json::Object(b)) => {
            let keys = |map: &serde_json::Map<String, Json>| {
                map.keys().cloned().collect::<BTreeSet<String>>()
            };
            if keys(a) != keys(b) {
                return differ();
            }
            for (key, a) in a {
                same(a, &b[key], numbers, &format!("{at}.{key:?}"))?;
            }
            Ok(())
        }
        _ if sent == got => Ok(()),
        _ => differ(),
    }
}

/// guarantee: bridge/value-roundtrip. Risk P7, C5.
fn round_trip(far: &Connection, name: &str, numbers: Numbers) {
    check(&format!("{name} round trip"), value(), |sent| {
        let echoed = far
            .invoke(
                "conformance",
                "echo",
                Value::List(vec![Value::Data(sent.clone())]),
            )
            .and_then(Value::json);
        // Beyond ±(2^53−1) an integer is a bigint (design V6), sent only to
        // a far end that takes them.
        if has_unsafe_integer(&sent) && !far.supports("value-bigint") {
            return match echoed {
                Err(Error::Value(message))
                    if message.contains("did not declare the capability value-bigint") =>
                {
                    Ok(())
                }
                other => Err(TestCaseError::fail(format!(
                    "an error naming value-bigint expected, got {other:?}"
                ))),
            };
        }
        let got = echoed.map_err(|error| TestCaseError::fail(format!("echo failed: {error}")))?;
        same(&sent, &got, numbers, "$").map_err(TestCaseError::fail)
    });
}

/// Whether `value` holds an integer beyond ±(2^53−1).
fn has_unsafe_integer(value: &Json) -> bool {
    match value {
        Json::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.unsigned_abs() > SAFE as u64,
            (None, Some(u)) => u > SAFE as u64,
            (None, None) => false,
        },
        Json::Array(items) => items.iter().any(has_unsafe_integer),
        Json::Object(fields) => fields.values().any(has_unsafe_integer),
        _ => false,
    }
}

// ── Tagged values (#308) ─────────────────────────────────────────────

use rutis_bridge::session as rpc;

fn date() -> impl Strategy<Value = rpc::Date> {
    // Every day of years 0001–9999.
    let first = rpc::Date::new(1, 1, 1).unwrap().days_since_unix_epoch();
    let last = rpc::Date::new(9999, 12, 31)
        .unwrap()
        .days_since_unix_epoch();
    (first..=last).prop_map(|days| rpc::Date::from_days_since_unix_epoch(days).unwrap())
}

fn time_of_day() -> impl Strategy<Value = rpc::TimeOfDay> {
    (
        0u8..24,
        0u8..60,
        0u8..60,
        prop_oneof![
            Just(0u32),
            0u32..1_000_000_000,
            (0u32..1000).prop_map(|ms| ms * 1_000_000)
        ],
    )
        .prop_map(|(h, m, s, ns)| rpc::TimeOfDay::new(h, m, s, ns).unwrap())
}

/// Decimal digits, `0` or without a leading zero, with an optional `-`.
fn integer_text(digits: usize) -> impl Strategy<Value = String> {
    prop_oneof![
        Just("0".to_owned()),
        (
            any::<bool>(),
            proptest::string::string_regex(&format!("[1-9][0-9]{{0,{digits}}}")).unwrap()
        )
            .prop_map(|(negative, digits)| {
                if negative {
                    format!("-{digits}")
                } else {
                    digits
                }
            }),
    ]
}

fn bigint() -> impl Strategy<Value = rpc::BigInt> {
    prop_oneof![
        any::<i128>().prop_map(rpc::BigInt::from),
        integer_text(60).prop_map(|text| text.parse().unwrap()),
        // The most digits allowed.
        Just("9".repeat(rpc::values::MAX_DIGITS).parse().unwrap()),
    ]
}

fn decimal() -> impl Strategy<Value = rpc::Decimal> {
    (
        any::<bool>(),
        integer_text(30),
        prop::option::of("[0-9]{1,30}"),
    )
        .prop_map(|(negative, integer, fraction)| {
            let sign = if negative && !integer.starts_with('-') {
                "-"
            } else {
                ""
            };
            let fraction = fraction.map(|f| format!(".{f}")).unwrap_or_default();
            format!("{sign}{integer}{fraction}").parse().unwrap()
        })
}

/// Whether `number` is a float whose whole value is beyond ±(2^53−1): not
/// exact in every language, so no key (design §5).
fn inexact_whole(number: &Json) -> bool {
    number
        .as_f64()
        .is_some_and(|x| x.fract() == 0.0 && x.abs() > SAFE as f64)
        && number.is_f64()
}

/// A value every language takes as a map key or set element (design §5).
fn key_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        3 => string().prop_map(|text| Value::Data(Json::String(text))),
        // An integer beyond ±(2^53−1) is a bigint key (design V6).
        2 => number()
            .prop_filter("an exact number", |n| !inexact_whole(n))
            .prop_map(Value::Data),
        1 => any::<bool>().prop_map(|flag| Value::Data(Json::Bool(flag))),
        1 => bigint().prop_map(Value::BigInt),
        1 => (date(), time_of_day()).prop_map(|(d, t)| Value::DateTime(rpc::DateTime::new(d, t))),
        1 => date().prop_map(Value::Date),
        1 => time_of_day().prop_map(Value::Time),
        1 => decimal().prop_map(Value::Decimal),
        1 => any::<u128>().prop_map(|n| Value::Uuid(rpc::Uuid::from_u128(n))),
    ]
}

/// What makes two keys equal, as the session compares them: numbers by
/// value (a coarse, conservative stand-in), an integer beyond ±(2^53−1) as
/// the bigint it crosses as, the rest as written.
fn key_identity(key: &Value) -> String {
    match key {
        Value::Data(number @ Json::Number(n)) if has_unsafe_integer(number) => {
            format!("{:?}", Value::BigInt(n.to_string().parse().unwrap()))
        }
        Value::Data(Json::Number(n)) => {
            let x = n.as_f64().unwrap();
            format!("n{}", if x == 0.0 { 0.0 } else { x })
        }
        other => format!("{other:?}"),
    }
}

/// `entries` without the later ones whose key repeats an earlier one's.
fn distinct<T>(entries: Vec<T>, key: impl Fn(&T) -> &Value) -> Vec<T> {
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(key_identity(key(entry))))
        .collect()
}

fn tagged_leaf() -> impl Strategy<Value = Value> {
    let instant = (-62_135_596_800i64..=253_402_300_799, 0u32..1_000_000_000)
        .prop_map(|(s, ns)| Value::Instant(rpc::Instant::from_unix(s, ns).unwrap()));
    prop_oneof![
        3 => leaf().prop_map(Value::Data),
        2 => key_value(),
        1 => instant,
        1 => prop::collection::vec(any::<u8>(), 0..48).prop_map(Value::Bytes),
    ]
}

/// Data, tagged values, lists, records, maps and sets, nested up to 6
/// levels.
fn tagged_value() -> impl Strategy<Value = Value> {
    tagged_leaf().prop_recursive(6, 48, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::List),
            prop::collection::btree_map(key(), inner.clone(), 0..4).prop_map(Value::Record),
            prop::collection::vec((key_value(), inner), 0..4)
                .prop_map(|entries| Value::Map(distinct(entries, |(key, _)| key))),
            prop::collection::vec(key_value(), 0..4)
                .prop_map(|elements| Value::Set(distinct(elements, |element| element))),
        ]
    })
}

/// guarantee: bridge/value-tags. Tagged values come back from the far
/// end's echo exactly as sent.
fn tagged_round_trip(far: &Connection, name: &str) {
    check(
        &format!("{name} tagged round trip"),
        tagged_value(),
        |sent| {
            let args = Value::List(vec![sent.clone()]);
            let echoed = far
                .invoke("conformance", "echo", args)
                .map_err(|error| TestCaseError::fail(format!("echo failed: {error}")))?;
            prop_assert_eq!(echoed, as_received(sent));
            Ok(())
        },
    );
}

/// What a JavaScript far end's echo (Node, Bun) sends back for `sent`
/// (design §2): an instant truncated to the millisecond (V7), a datetime,
/// date, time, decimal or uuid as its text; `None` when it refuses the
/// call, for a map or set whose keys it cannot hold apart (§5).
fn javascript_echo(sent: &Value) -> Option<Value> {
    let text = |text: String| Value::Data(Json::String(text));
    Some(match sent {
        Value::Instant(instant) => Value::Instant(
            rpc::Instant::from_unix(
                instant.unix_seconds(),
                instant.subsec_nanos() / 1_000_000 * 1_000_000,
            )
            .unwrap(),
        ),
        Value::DateTime(value) => text(value.to_string()),
        Value::Date(value) => text(value.to_string()),
        Value::Time(value) => text(value.to_string()),
        Value::Decimal(value) => text(value.to_string()),
        Value::Uuid(value) => text(value.to_string()),
        Value::List(items) => {
            Value::List(items.iter().map(javascript_echo).collect::<Option<_>>()?)
        }
        Value::Record(fields) => Value::Record(
            fields
                .iter()
                .map(|(key, value)| Some((key.clone(), javascript_echo(value)?)))
                .collect::<Option<_>>()?,
        ),
        Value::Map(entries) => {
            let entries = entries
                .iter()
                .map(|(key, value)| Some((javascript_echo(key)?, javascript_echo(value)?)))
                .collect::<Option<Vec<_>>>()?;
            javascript_keys(entries.iter().map(|(key, _)| key))?;
            Value::Map(entries)
        }
        Value::Set(elements) => {
            let elements = elements
                .iter()
                .map(javascript_echo)
                .collect::<Option<Vec<_>>>()?;
            javascript_keys(&elements)?;
            Value::Set(elements)
        }
        other => other.clone(),
    })
}

/// Whether JavaScript holds `keys` apart, as received: no integral number
/// beyond ±(2^53−1) (a double is not that integer), and no two equal once
/// a datetime, date, ... is its text.
fn javascript_keys<'a>(keys: impl IntoIterator<Item = &'a Value>) -> Option<()> {
    let mut seen = BTreeSet::new();
    for key in keys {
        let identity = match key {
            Value::Data(Json::Number(n)) => {
                let x = n.as_f64().unwrap();
                if x.fract() == 0.0 && x.abs() > SAFE as f64 {
                    return None;
                }
                format!("n{}", if x == 0.0 { 0.0 } else { x })
            }
            other => format!("{other:?}"),
        };
        seen.insert(identity).then_some(())?;
    }
    Some(())
}

/// A value as JSON, a tagged value as `{"$<tag>": ...}` (as the expectations
/// in `weather.expected.json` write it), to compare with `same`.
fn written(value: &Value) -> Json {
    let tagged = |tag: &str, value: Json| json!({ format!("${tag}"): value });
    match value {
        Value::Data(json) => json.clone(),
        Value::List(items) => Json::Array(items.iter().map(written).collect()),
        Value::Record(fields) => Json::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), written(value)))
                .collect(),
        ),
        Value::Map(entries) => tagged(
            "map",
            entries
                .iter()
                .map(|(key, value)| json!([written(key), written(value)]))
                .collect(),
        ),
        Value::Set(elements) => tagged("set", elements.iter().map(written).collect()),
        Value::Bytes(bytes) => tagged("bytes", json!(bytes)),
        other => match other.tag() {
            Some(tag) => tagged(tag, json!(format!("{other:?}"))),
            None => json!(format!("{other:?}")),
        },
    }
}

/// guarantee: bridge/value-tags. Tagged values come back from a JavaScript
/// far end's echo as JavaScript holds them ([`javascript_echo`]); a value
/// it cannot hold fails that call only.
fn javascript_tagged_round_trip(far: &Connection, name: &str) {
    check(
        &format!("{name} tagged round trip"),
        tagged_value(),
        |sent| {
            let echoed = far.invoke("conformance", "echo", Value::List(vec![sent.clone()]));
            // Integers beyond ±(2^53−1) cross as bigints (V6).
            match (javascript_echo(&as_received(sent.clone())), echoed) {
                (Some(expected), Ok(echoed)) => {
                    same(
                        &written(&expected),
                        &written(&echoed),
                        Numbers::Doubles,
                        "$",
                    )
                    .map_err(TestCaseError::fail)?;
                }
                // Refused by the far end, or here before sending (a whole
                // float beyond the range as a key).
                (None, Err(error)) => {
                    let message = error.to_string();
                    prop_assert!(
                        message.contains("map key") || message.contains("set element"),
                        "{}",
                        message
                    );
                }
                (expected, echoed) => {
                    return Err(TestCaseError::fail(format!(
                        "expected {expected:?}, got {echoed:?}"
                    )))
                }
            }
            prop_assert!(far.close_reason().is_none());
            Ok(())
        },
    );
}

/// `value` as it comes back: an integer beyond ±(2^53−1) in data as the
/// bigint it crossed as, the arrays and objects holding one as lists and
/// records (design V6).
fn as_received(value: Value) -> Value {
    match value {
        Value::Data(json) if has_unsafe_integer(&json) => match json {
            Json::Array(items) => Value::List(
                items
                    .into_iter()
                    .map(Value::Data)
                    .map(as_received)
                    .collect(),
            ),
            Json::Object(fields) => Value::Record(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, as_received(Value::Data(value))))
                    .collect(),
            ),
            number => Value::BigInt(number.to_string().parse().unwrap()),
        },
        Value::List(items) => Value::List(items.into_iter().map(as_received).collect()),
        Value::Record(fields) => Value::Record(
            fields
                .into_iter()
                .map(|(key, value)| (key, as_received(value)))
                .collect(),
        ),
        Value::Map(entries) => Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (as_received(key), as_received(value)))
                .collect(),
        ),
        Value::Set(elements) => Value::Set(elements.into_iter().map(as_received).collect()),
        other => other,
    }
}

/// Whether `value` holds a tagged value, an integer beyond ±(2^53−1) in
/// data included.
fn holds_tagged(value: &Value) -> bool {
    match value {
        Value::Data(json) => has_unsafe_integer(json),
        Value::List(items) => items.iter().any(holds_tagged),
        Value::Record(fields) => fields.values().any(holds_tagged),
        other => other.tag().is_some(),
    }
}

/// guarantee: bridge/value-tags. To a far end that declared none of the
/// tagged values, a call carrying one fails on this side, naming the
/// capability; data still crosses; the session goes on.
fn tagged_refused(far: &Connection, name: &str) {
    check(&format!("{name} tagged refused"), tagged_value(), |sent| {
        let tagged = holds_tagged(&sent);
        let echoed = far.invoke("conformance", "echo", Value::List(vec![sent.clone()]));
        match (tagged, echoed) {
            (true, Err(Error::Value(message))) => {
                prop_assert!(
                    message.contains("did not declare the capability value-"),
                    "{}",
                    message
                );
            }
            (false, Ok(echoed)) => prop_assert_eq!(echoed, sent),
            (_, other) => {
                return Err(TestCaseError::fail(format!(
                    "tagged {tagged}: unexpected {other:?}"
                )))
            }
        }
        prop_assert!(far.close_reason().is_none());
        Ok(())
    });
}

// ── The depth limit ──────────────────────────────────────────────────

/// Data `depth` arrays deep.
fn chain(depth: usize) -> Json {
    (0..depth).fold(json!("bottom"), |inner, _| json!([inner]))
}

/// Lists `depth` deep around a callback: each level is a list holding a
/// reference, which takes two levels on the wire.
fn lists(depth: usize) -> Value {
    (0..depth).fold(Value::callback(|_| Ok(Value::Undefined)), |inner, _| {
        Value::List(vec![inner])
    })
}

fn list_depth(value: &Value) -> usize {
    match value {
        Value::List(items) => 1 + items.iter().map(list_depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// The call failed, explicitly, saying the limit; `remote`: the far end
/// was the side sending the value.
fn too_deep(result: Result<Value, Error>, remote: bool, what: &str) {
    match (&result, remote) {
        (Err(Error::Value(message)), false) | (Err(Error::Remote { message, .. }), true)
            if message.contains("64 levels") => {}
        _ => panic!("{what}: an error naming the limit expected, got {result:?}"),
    }
}

/// guarantee: bridge/value-limits. Risk E1, P7.
///
/// Up to the limit (`MAX_DEPTH`, 64 levels), data and lists holding
/// references cross both ways, also as the argument the far end passes to
/// a callback. One level more fails only that call, on the side that would
/// send it (this side, or the far end, in a result and in an argument),
/// with an error naming the limit; the session goes on (#279).
fn depth_limit(far: &Connection, name: &str) {
    assert_eq!(MAX_DEPTH, 64);
    let call =
        |method: &str, args: Vec<Value>| far.invoke("conformance", method, Value::List(args));
    let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first = {
        let called = called.clone();
        Value::callback(move |args| {
            called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(args.list()?.into_iter().next().unwrap_or(Value::Undefined))
        })
    };
    let calls = || called.load(std::sync::atomic::Ordering::SeqCst);

    // At the limit: both ways, as data and as lists.
    let back = call("echo", vec![Value::Data(chain(MAX_DEPTH))]).and_then(Value::json);
    assert_eq!(back.unwrap(), chain(MAX_DEPTH), "{name}: data at the limit");
    let back = call("wrap", vec![Value::Data(chain(MAX_DEPTH - 1))]).and_then(Value::json);
    assert_eq!(
        back.unwrap(),
        chain(MAX_DEPTH),
        "{name}: data wrapped to the limit"
    );
    let back = call("echo", vec![lists(MAX_DEPTH)]).unwrap();
    assert_eq!(list_depth(&back), MAX_DEPTH, "{name}: lists at the limit");
    let back = call("wrap", vec![lists(MAX_DEPTH - 1)]).unwrap();
    assert_eq!(
        list_depth(&back),
        MAX_DEPTH,
        "{name}: lists wrapped to the limit"
    );
    let back = call(
        "applyWrapped",
        vec![first.clone(), Value::Data(chain(MAX_DEPTH - 1))],
    )
    .and_then(Value::json);
    assert_eq!(
        back.unwrap(),
        chain(MAX_DEPTH),
        "{name}: a callback's argument at the limit"
    );
    assert_eq!(calls(), 1);

    // One level more: this side sends it.
    too_deep(
        call("echo", vec![Value::Data(chain(MAX_DEPTH + 1))]),
        false,
        &format!("{name}: data sent past the limit"),
    );
    too_deep(
        call("echo", vec![lists(MAX_DEPTH + 1)]),
        false,
        &format!("{name}: lists sent past the limit"),
    );
    // The far end sends it: in its result, and as a callback's argument
    // (the callback never runs).
    too_deep(
        call("wrap", vec![Value::Data(chain(MAX_DEPTH))]),
        true,
        &format!("{name}: data returned past the limit"),
    );
    too_deep(
        call("wrap", vec![lists(MAX_DEPTH)]),
        true,
        &format!("{name}: lists returned past the limit"),
    );
    too_deep(
        call("applyWrapped", vec![first, Value::Data(chain(MAX_DEPTH))]),
        true,
        &format!("{name}: a callback's argument past the limit"),
    );
    assert_eq!(calls(), 1, "{name}: the callback ran with a value too deep");

    // The session goes on.
    let back = call("echo", vec![Value::Data(json!("after"))]).and_then(Value::json);
    assert_eq!(back.unwrap(), json!("after"), "{name}: a call after them");
    assert!(far.close_reason().is_none(), "{name}: the session ended");
}

// ── What a call cannot send (#675) ───────────────────────────────────

/// The local transport's message limit: 16 MiB.
const LIMIT: usize = 16 * 1024 * 1024;

fn over_the_limit(message: &str) -> bool {
    message.contains("the message would be ")
        && message.ends_with("over the limit of 16777216 bytes (16 MiB)")
}

/// guarantee: bridge/bad-values. Risk E1.
///
/// What cannot be sent fails only that call, on the side that would send
/// it, naming the call, the argument or the result and why; the session
/// goes on. A message over the limit (16 MiB): this side's argument, the
/// far end's result, and the far end's argument to a callback (which never
/// runs). And a thrown value that is not an error is relayed by the far end
/// as it came ([`relayed_throw`]).
fn bad_values(far: &Connection, name: &str) {
    let call =
        |method: &str, args: Vec<Value>| far.invoke("conformance", method, Value::List(args));
    match call("echo", vec![Value::Data(json!("x".repeat(LIMIT)))]) {
        Err(Error::Value(message))
            if message.starts_with("conformance.echo: argument 1 cannot be sent: ")
                && over_the_limit(&message) => {}
        other => panic!("{name}: this side's argument over the limit: {other:?}"),
    }
    match call("big", vec![Value::Data(json!(LIMIT + 10))]) {
        Err(Error::Remote { message, .. })
            if message.contains("conformance.big: the result cannot be sent: ")
                && over_the_limit(&message) => {}
        other => panic!("{name}: the far end's result over the limit: {other:?}"),
    }
    let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback = {
        let called = called.clone();
        Value::callback(move |_| {
            called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Value::Undefined)
        })
    };
    match call("applyBig", vec![callback, Value::Data(json!(LIMIT + 10))]) {
        Err(Error::Remote { message, .. })
            if message.contains("a function: argument 1 cannot be sent: ")
                && over_the_limit(&message) => {}
        other => panic!("{name}: the far end's argument over the limit: {other:?}"),
    }
    assert_eq!(
        called.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "{name}: the callback ran with an argument over the limit"
    );
    relayed_throw(far, name);
    let back = call("echo", vec![Value::Data(json!("after"))]).and_then(Value::json);
    assert_eq!(back.unwrap(), json!("after"), "{name}: a call after them");
    assert!(far.close_reason().is_none(), "{name}: the session ended");
}

/// guarantee: bridge/bad-values.
///
/// A thrown value that is not an error (JavaScript's `throw 'a string'`:
/// `ThrownValue`, its graph's root the string, no nodes), thrown by a
/// callback the far end calls and thrown on by the far end, arrives as it
/// was thrown: the name, the message, the root, and the nodes an array (Go
/// sent `null`, which ended Node's session: `invalid error graph`).
fn relayed_throw(far: &Connection, name: &str) {
    let root = json!({ "type": "data", "value": "a string" });
    let thrown = {
        let root = root.clone();
        Value::callback(move |_| {
            Err(Error::Remote {
                name: "ThrownValue".into(),
                message: "a string".into(),
                graph: Some(json!({ "root": root, "nodes": [] })),
            })
        })
    };
    let args = Value::List(vec![thrown, Value::Data(json!(1))]);
    match far.invoke("conformance", "apply", args) {
        Err(Error::Remote {
            name: thrown,
            message,
            graph: Some(graph),
        }) => {
            // Node and Bun throw the string itself on, and a thrown value's
            // message is the value as written (#677): `"a string"`.
            let expected = match name {
                "node" | "bun" => "\"a string\"",
                _ => "a string",
            };
            assert_eq!(
                (thrown.as_str(), message.as_str()),
                ("ThrownValue", expected),
                "{name}"
            );
            assert!(graph["nodes"].is_array(), "{name}: {graph}");
            assert_eq!(graph["root"], root, "{name}: {graph}");
        }
        other => panic!("{name}: the thrown value relayed expected, got {other:?}"),
    }
}

/// guarantee: bridge/bad-values.
///
/// To a far end that takes no objects with methods (`main` without
/// `objects`, as every runtime is), the far end's result holding one fails
/// that call before it is sent, naming the call and the far end; the
/// session goes on.
fn objects_refused(far: &Connection, name: &str) {
    match far.invoke("conformance", "object", Value::List(vec![])) {
        Err(Error::Remote { message, .. })
            if message.contains(
                "conformance.object: the result cannot be sent: an object with methods",
            ) && message
                .contains("cannot cross to main; pass plain data or a function instead") => {}
        other => panic!("{name}: an object to a far end that takes none: {other:?}"),
    }
    let back = far
        .invoke(
            "conformance",
            "echo",
            Value::List(vec![Value::Data(json!("after"))]),
        )
        .and_then(Value::json);
    assert_eq!(back.unwrap(), json!("after"), "{name}: a call after it");
    assert!(far.close_reason().is_none(), "{name}: the session ended");
}

// ── Release counts ───────────────────────────────────────────────────

const SLOTS: u8 = 4;

#[derive(Clone, Debug)]
enum Op {
    /// Pass this side's callback `slot` to the far end `copies` times in
    /// one call; the far end keeps it.
    Grant { slot: u8, copies: u8 },
    /// The far end releases what it keeps in `slot`.
    Release(u8),
    /// The far end makes a function; this side keeps it in `slot`
    /// (dropping what was there).
    Make(u8),
    /// This side drops the function in `slot`.
    Drop(u8),
    /// This side passes the function in `slot` back; it is echoed, granted
    /// again.
    Bounce(u8),
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let slot = 0..SLOTS;
    let op = prop_oneof![
        3 => (slot.clone(), 1u8..=3).prop_map(|(slot, copies)| Op::Grant { slot, copies }),
        2 => slot.clone().prop_map(Op::Release),
        3 => slot.clone().prop_map(Op::Make),
        2 => slot.clone().prop_map(Op::Drop),
        2 => slot.prop_map(Op::Bounce),
    ];
    prop::collection::vec(op, 0..24)
}

/// (exports, imports) of each side.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Tables {
    near: (usize, usize),
    far: (usize, usize),
}

fn tables(near: &Connection) -> Result<Tables, String> {
    let far = near
        .invoke("conformance", "tables", Value::List(vec![]))
        .and_then(Value::json)
        .map_err(|error| format!("tables: {error}"))?;
    let count = |field: &str| far[field].as_u64().map(|n| n as usize);
    Ok(Tables {
        near: near.references(),
        far: (
            count("exports").ok_or("far exports")?,
            count("imports").ok_or("far imports")?,
        ),
    })
}

/// guarantee: bridge/release-count. Risk P4: after any sequence
/// of grants and releases, each side's reference tables hold exactly the
/// references not yet released.
fn releases_balance(near: &Connection, name: &str) {
    check(&format!("{name} release counts"), ops(), |ops| {
        // Counted from where this case starts, so that a case shrunk after
        // an earlier one leaked still shows its own leak.
        let base = tables(near).map_err(TestCaseError::fail)?;
        let call = |method: &str, args: Vec<Value>| {
            near.invoke("conformance", method, Value::List(args))
                .map_err(|error| TestCaseError::fail(format!("{method}: {error}")))
        };
        let slot = |slot: u8| Value::Data(json!(slot));
        let callbacks: Vec<Value> = (0..SLOTS)
            .map(|_| Value::callback(|_| Ok(Value::Undefined)))
            .collect();
        // The model: what the far end keeps, what this side keeps.
        let mut kept = BTreeSet::new();
        let mut made: BTreeMap<u8, Value> = BTreeMap::new();
        let expect = |kept: &BTreeSet<u8>, made: &BTreeMap<u8, Value>, after: &str| {
            let expected = Tables {
                near: (base.near.0 + kept.len(), base.near.1 + made.len()),
                far: (base.far.0 + made.len(), base.far.1 + kept.len()),
            };
            let actual = tables(near).map_err(TestCaseError::fail)?;
            if actual == expected {
                Ok(())
            } else {
                Err(TestCaseError::fail(format!(
                    "after {after}: tables {actual:?}, expected {expected:?} \
                     (far keeps {kept:?}, near keeps {:?})",
                    made.keys().collect::<Vec<_>>()
                )))
            }
        };
        for (index, op) in ops.iter().enumerate() {
            match op {
                Op::Grant { slot: s, copies } => {
                    let mut args = vec![slot(*s)];
                    args.extend((0..*copies).map(|_| callbacks[*s as usize].clone()));
                    call("keep", args)?;
                    kept.insert(*s);
                }
                Op::Release(s) => {
                    call("free", vec![slot(*s)])?;
                    kept.remove(s);
                }
                Op::Make(s) => {
                    let function = call("make", vec![])?;
                    made.insert(*s, function);
                }
                Op::Drop(s) => {
                    made.remove(s);
                }
                Op::Bounce(s) => {
                    if let Some(function) = made.get(s) {
                        call("echo", vec![function.clone()])?;
                    }
                }
            }
            expect(&kept, &made, &format!("op {index} {op:?}"))?;
        }
        // Let everything go: both sides are back where they started.
        for s in std::mem::take(&mut kept) {
            call("free", vec![slot(s)])?;
        }
        made.clear();
        expect(&kept, &made, "releasing everything")
    });
}

// ── Rust ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn rust_values_round_trip() {
    let (near, _far) = rust_far_end();
    near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || round_trip(&near, "rust", Numbers::Exact))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_tagged_values_round_trip() {
    let (near, _far) = rust_far_end();
    near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || tagged_round_trip(&near, "rust"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_tagged_values_refused_without_capabilities() {
    let mut older = Endpoint::rust(id("rust"));
    older
        .capabilities
        .retain(|capability| !capability.starts_with("value-"));
    let (near, _far) = rust_far_end_as(older);
    near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || tagged_refused(&near, "rust"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_depth_limit() {
    let (near, _far) = rust_far_end();
    near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || depth_limit(&near, "rust"))
        .await
        .unwrap();
}

/// guarantee: bridge/bad-values (#675). A Rust far end relays a thrown value
/// that is not an error as it came; its memory channel has no size limit.
#[tokio::test(flavor = "multi_thread")]
async fn rust_relays_a_thrown_value() {
    let (near, _far) = rust_far_end();
    near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || relayed_throw(&near, "rust"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_releases_balance() {
    let (near, _far) = rust_far_end();
    near.ready().await.unwrap();
    tokio::task::spawn_blocking(move || releases_balance(&near, "rust"))
        .await
        .unwrap();
}

// ── Node ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn node_values_round_trip() {
    let (near, _child) = far_end(node(), "node").await;
    tokio::task::spawn_blocking(move || round_trip(&near, "node", Numbers::Doubles))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn node_tagged_values_round_trip() {
    let (near, _child) = far_end(node(), "node").await;
    tokio::task::spawn_blocking(move || javascript_tagged_round_trip(&near, "node"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn node_depth_limit() {
    let (near, _child) = far_end(node(), "node").await;
    tokio::task::spawn_blocking(move || depth_limit(&near, "node"))
        .await
        .unwrap();
}

/// guarantee: bridge/bad-values (#675).
#[tokio::test(flavor = "multi_thread")]
async fn node_bad_values() {
    let (near, _child) = far_end(node(), "node").await;
    tokio::task::spawn_blocking(move || bad_values(&near, "node"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn node_releases_balance() {
    let (near, _child) = far_end(node(), "node").await;
    tokio::task::spawn_blocking(move || releases_balance(&near, "node"))
        .await
        .unwrap();
}

// ── Python ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn python_values_round_trip() {
    let (near, _child) = far_end(python(), "python").await;
    tokio::task::spawn_blocking(move || round_trip(&near, "python", Numbers::Exact))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn python_depth_limit() {
    let (near, _child) = far_end(python(), "python").await;
    tokio::task::spawn_blocking(move || depth_limit(&near, "python"))
        .await
        .unwrap();
}

/// guarantee: bridge/bad-values (#675).
#[tokio::test(flavor = "multi_thread")]
async fn python_bad_values() {
    let (near, _child) = far_end(python(), "python").await;
    tokio::task::spawn_blocking(move || bad_values(&near, "python"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn python_releases_balance() {
    let (near, _child) = far_end(python(), "python").await;
    tokio::task::spawn_blocking(move || releases_balance(&near, "python"))
        .await
        .unwrap();
}

// ── Go, Bun: values only (their fixtures have no reference tables) ───

/// The Go fixture echoes the raw JSON (`json.RawMessage`): this checks the
/// Go session, not how `encoding/json` decodes into Go types (the table's
/// Go row).
#[cfg(feature = "go")]
async fn go_far_end() -> (Connection, tokio::process::Child) {
    far_end(go(), "go").await
}

/// The Go session fixture, built once per test binary.
#[cfg(feature = "go")]
fn go() -> tokio::process::Command {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let binary = BINARY.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let binary = directory.join("conformance");
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./internal/fixtures/session")
            .current_dir(repo().join("go/rutis"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build ./internal/fixtures/session");
        binary
    });
    tokio::process::Command::new(binary)
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_values_round_trip() {
    let (near, _child) = go_far_end().await;
    tokio::task::spawn_blocking(move || round_trip(&near, "go", Numbers::Exact))
        .await
        .unwrap();
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_depth_limit() {
    let (near, _child) = go_far_end().await;
    tokio::task::spawn_blocking(move || depth_limit(&near, "go"))
        .await
        .unwrap();
}

/// guarantee: bridge/bad-values (#675).
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_bad_values() {
    let (near, _child) = far_end(go(), "go").await;
    tokio::task::spawn_blocking(move || bad_values(&near, "go"))
        .await
        .unwrap();
}

/// `value` as a far end that holds lists and records of data as data
/// sends it back: a list or record holding no tagged value is data (on the
/// wire, the two are the same value).
#[cfg(any(feature = "go", feature = "kotlin"))]
fn as_data(value: Value) -> Value {
    match value {
        Value::List(items) => {
            let items: Vec<Value> = items.into_iter().map(as_data).collect();
            if items.iter().any(holds_tagged) {
                Value::List(items)
            } else {
                Value::Data(Json::Array(
                    items.into_iter().map(|item| item.json().unwrap()).collect(),
                ))
            }
        }
        Value::Record(fields) => {
            let fields: BTreeMap<String, Value> = fields
                .into_iter()
                .map(|(key, value)| (key, as_data(value)))
                .collect();
            if fields.values().any(holds_tagged) {
                Value::Record(fields)
            } else {
                Value::Data(Json::Object(
                    fields
                        .into_iter()
                        .map(|(key, value)| (key, value.json().unwrap()))
                        .collect(),
                ))
            }
        }
        Value::Map(entries) => Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (as_data(key), as_data(value)))
                .collect(),
        ),
        Value::Set(elements) => Value::Set(elements.into_iter().map(as_data).collect()),
        other => other,
    }
}

/// guarantee: bridge/value-tags. Tagged values come back from the Go
/// session's echo as sent: lists and records holding none come back as
/// the same data.
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_tagged_values_round_trip() {
    let (near, _child) = go_far_end().await;
    tokio::task::spawn_blocking(move || {
        check("go tagged round trip", tagged_value(), |sent| {
            let echoed = near
                .invoke("conformance", "echo", Value::List(vec![sent.clone()]))
                .map_err(|error| TestCaseError::fail(format!("echo failed: {error}")))?;
            // Integers beyond ±(2^53−1) cross as bigints (V6).
            prop_assert_eq!(as_data(echoed), as_data(as_received(sent)));
            Ok(())
        })
    })
    .await
    .unwrap();
}

// ── Swift: values, tagged values, depth and release counts ───────────

/// The Swift session fixture (`swift/rutis/Sources/RutisSessionFixture`),
/// built once per test binary with the `swift` on PATH. Its echo returns
/// the value as the session received it, so tagged values come back
/// exactly as sent.
#[cfg(all(feature = "swift", target_os = "macos"))]
async fn swift_far_end() -> (Connection, tokio::process::Child) {
    far_end(swift(), "swift").await
}

/// The Swift session fixture, built once per test binary.
#[cfg(all(feature = "swift", target_os = "macos"))]
fn swift() -> tokio::process::Command {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let binary = BINARY.get_or_init(|| {
        let product = "RutisSessionFixture";
        let status = std::process::Command::new("swift")
            .args(["build", "--product", product])
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build --product {product}");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).join(product)
    });
    tokio::process::Command::new(binary)
}

#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_values_round_trip() {
    let (near, _child) = swift_far_end().await;
    tokio::task::spawn_blocking(move || round_trip(&near, "swift", Numbers::Exact))
        .await
        .unwrap();
}

#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_tagged_values_round_trip() {
    let (near, _child) = swift_far_end().await;
    tokio::task::spawn_blocking(move || tagged_round_trip(&near, "swift"))
        .await
        .unwrap();
}

#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_depth_limit() {
    let (near, _child) = swift_far_end().await;
    tokio::task::spawn_blocking(move || depth_limit(&near, "swift"))
        .await
        .unwrap();
}

/// guarantee: bridge/bad-values (#675).
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_bad_values() {
    let (near, _child) = far_end(swift(), "swift").await;
    tokio::task::spawn_blocking(move || bad_values(&near, "swift"))
        .await
        .unwrap();
    let (near, _second) = far_end_with(swift(), main_without_objects("swift")).await;
    tokio::task::spawn_blocking(move || objects_refused(&near, "swift"))
        .await
        .unwrap();
}

#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_releases_balance() {
    let (near, _child) = swift_far_end().await;
    tokio::task::spawn_blocking(move || releases_balance(&near, "swift"))
        .await
        .unwrap();
}

#[cfg(feature = "bun")]
async fn bun_far_end() -> (Connection, tokio::process::Child) {
    far_end(bun(), "bun").await
}

#[cfg(feature = "bun")]
fn bun() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("bun");
    command
        .args(["--no-install", "--no-env-file"])
        .arg(repo().join("bun/rutis-bun/test/fixtures/conformance-session.ts"))
        .current_dir(repo().join("bun/rutis-bun"));
    command
}

#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn bun_values_round_trip() {
    let (near, _child) = bun_far_end().await;
    tokio::task::spawn_blocking(move || round_trip(&near, "bun", Numbers::Doubles))
        .await
        .unwrap();
}

#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn bun_tagged_values_round_trip() {
    let (near, _child) = bun_far_end().await;
    tokio::task::spawn_blocking(move || javascript_tagged_round_trip(&near, "bun"))
        .await
        .unwrap();
}

#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn bun_depth_limit() {
    let (near, _child) = bun_far_end().await;
    tokio::task::spawn_blocking(move || depth_limit(&near, "bun"))
        .await
        .unwrap();
}

/// guarantee: bridge/bad-values (#675).
#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn bun_bad_values() {
    let (near, _child) = far_end(bun(), "bun").await;
    tokio::task::spawn_blocking(move || bad_values(&near, "bun"))
        .await
        .unwrap();
}

// ── Kotlin: values only (its fixture has no reference tables) ────────

#[cfg(feature = "kotlin")]
mod kotlin;

/// The Kotlin session fixture (the SDK's jvm target), which echoes the
/// session values it decodes: data as its JSON, tagged values as the JDK
/// values they decode to.
#[cfg(feature = "kotlin")]
async fn kotlin_far_end() -> (Connection, tokio::process::Child) {
    far_end(kotlin::session(), "kotlin").await
}

#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn kotlin_values_round_trip() {
    let (near, _child) = kotlin_far_end().await;
    tokio::task::spawn_blocking(move || round_trip(&near, "kotlin", Numbers::Exact))
        .await
        .unwrap();
}

#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn kotlin_depth_limit() {
    let (near, _child) = kotlin_far_end().await;
    tokio::task::spawn_blocking(move || depth_limit(&near, "kotlin"))
        .await
        .unwrap();
}

/// guarantee: bridge/bad-values (#675).
#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn kotlin_bad_values() {
    let (near, _child) = far_end(kotlin::session(), "kotlin").await;
    tokio::task::spawn_blocking(move || bad_values(&near, "kotlin"))
        .await
        .unwrap();
}

/// guarantee: bridge/value-tags. Tagged values come back from the Kotlin
/// session's echo as sent: lists and records holding none come back as the
/// same data.
#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn kotlin_tagged_values_round_trip() {
    let (near, _child) = kotlin_far_end().await;
    tokio::task::spawn_blocking(move || {
        check("kotlin tagged round trip", tagged_value(), |sent| {
            let echoed = near
                .invoke("conformance", "echo", Value::List(vec![sent.clone()]))
                .map_err(|error| TestCaseError::fail(format!("echo failed: {error}")))?;
            prop_assert_eq!(as_data(echoed), as_data(as_received(sent)));
            Ok(())
        })
    })
    .await
    .unwrap();
}
