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
//! the table lists these as not guaranteed. A float read on the Rust side
//! may be up to two units in the last place off (#290, not guaranteed). NaN and
//! ±Infinity are not JSON and are not generated.
//!
//! Values nest at most 64 deep, below the receiving parser's limit (about
//! 125 levels of data): a deeper value ends the whole session (#279, not
//! guaranteed).
#![cfg(all(unix, feature = "testing"))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::testing::Fixture;
use rutis_bridge::session::{Connection, Endpoint, Format, Value};
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

fn main_endpoint(far: &str) -> Format {
    Format::Endpoint(Endpoint::rust(id("main")).expect(id(far)))
}

// ── Far ends (as in runtime_conformance.rs) ──────────────────────────

/// A Rust far end serving [`Fixture`] over an in-memory channel.
fn rust_far_end() -> (Connection, Connection) {
    let (a, b) = rutis_bridge::transport::memory::pair();
    let far = Connection::open_with(
        b,
        Arc::new(Fixture::default()),
        Format::Endpoint(Endpoint::rust(id("rust")).expect(id("main"))),
    )
    .unwrap();
    (
        Connection::open_with(a, Arc::new(Nothing), main_endpoint("rust")).unwrap(),
        far,
    )
}

/// Listen on a Unix socket, start `command` with it, open the session.
async fn far_end(
    mut command: tokio::process::Command,
    far: &str,
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
        fn close(&self, _: &str) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }
    let closer = Arc::new(Shut(stream.try_clone().unwrap()));
    let channel = rutis_bridge::transport::local::framed(reader, stream, closer);
    let connection = Connection::open_with(channel, Arc::new(Nothing), main_endpoint(far)).unwrap();
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

/// Values nested up to 8 levels, plus single chains up to 64 deep: below
/// the depth that ends the session (#279).
fn value() -> impl Strategy<Value = Json> {
    let nested = leaf().prop_recursive(8, 64, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Json::Array),
            prop::collection::btree_map(key(), inner, 0..4)
                .prop_map(|fields| Json::Object(fields.into_iter().collect())),
        ]
    });
    let deep = (leaf(), prop::collection::vec(any::<bool>(), 0..64)).prop_map(|(leaf, levels)| {
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
    /// As sent: integer or float, the same value, -0 kept. The number is
    /// how many times a Rust side parses the value on its way (two between
    /// Rust endpoints, one otherwise; see [`near`]).
    Exact(u64),
    /// As IEEE 754 doubles (JavaScript).
    Doubles,
}

/// The same double, or one of the same sign at most two units in the last
/// place away per parse: a Rust side parses JSON floats with serde_json's
/// default (not `float_roundtrip`) precision. Measured over 2·10^7 random
/// doubles, about 30% come back off, never by more than 2 units (#290,
/// listed as not guaranteed in `guide/plugin-api.md`). -0 and 0 differ.
fn near(x: f64, y: f64, parses: u64) -> bool {
    let (a, b) = (x.to_bits(), y.to_bits());
    (a >> 63) == (b >> 63) && a.abs_diff(b) <= 2 * parses
}

/// Whether `got` is what `sent` must come back as, or where it differs.
fn same(sent: &Json, got: &Json, numbers: Numbers, at: &str) -> Result<(), String> {
    let differ = || Err(format!("at {at}: sent {sent}, got {got}"));
    match (sent, got) {
        (Json::Number(a), Json::Number(b)) => {
            let (x, y) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            let equal = match numbers {
                Numbers::Exact(parses) => match (a.is_f64(), b.is_f64()) {
                    (true, true) => near(x, y, parses),
                    (false, false) => a == b,
                    _ => false,
                },
                // The double nearest the sent number; -0 and 0 alike.
                Numbers::Doubles => x == y || near(x, y, 1),
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

/// guarantee: bridge/value-roundtrip (proposed). Risk P7, C5.
fn round_trip(far: &Connection, name: &str, numbers: Numbers) {
    check(&format!("{name} round trip"), value(), |sent| {
        let got = far
            .invoke(
                "conformance",
                "echo",
                Value::List(vec![Value::Data(sent.clone())]),
            )
            .and_then(Value::json)
            .map_err(|error| TestCaseError::fail(format!("echo failed: {error}")))?;
        same(&sent, &got, numbers, "$").map_err(TestCaseError::fail)
    });
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

/// guarantee: bridge/release-count (proposed). Risk P4: after any sequence
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
    tokio::task::spawn_blocking(move || round_trip(&near, "rust", Numbers::Exact(2)))
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
    tokio::task::spawn_blocking(move || round_trip(&near, "python", Numbers::Exact(1)))
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
#[tokio::test(flavor = "multi_thread")]
async fn go_values_round_trip() {
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
    let (near, _child) = far_end(tokio::process::Command::new(binary), "go").await;
    tokio::task::spawn_blocking(move || round_trip(&near, "go", Numbers::Exact(1)))
        .await
        .unwrap();
}

#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn bun_values_round_trip() {
    let mut command = tokio::process::Command::new("bun");
    command
        .args(["--no-install", "--no-env-file"])
        .arg(repo().join("bun/rutis-bun/test/fixtures/conformance-session.ts"))
        .current_dir(repo().join("bun/rutis-bun"));
    let (near, _child) = far_end(command, "bun").await;
    tokio::task::spawn_blocking(move || round_trip(&near, "bun", Numbers::Doubles))
        .await
        .unwrap();
}
