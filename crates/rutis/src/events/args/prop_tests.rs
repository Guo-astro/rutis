//! guarantee: core/D2, events/by-name
//! Property test of the event-argument codec (#631): the shape table of
//! protocol spec §12.1 decides how many arguments a value is, and any
//! nested serde value survives the round trip through those arguments.
//!
//! The table, at the top level only:
//!
//! | top-level shape | arguments |
//! | --- | --- |
//! | unit, unit struct, a value that serializes as one (`serde_json::Value::Null`) | none |
//! | tuple or tuple struct of 2 or more fields | one per field |
//! | anything else (scalar, newtype, struct, sequence, map, enum, a tuple of one) | one |
//!
//! Everything below the top level is plain JSON. Each generated case is
//! built around an arbitrary nested `serde_json::Value`; the test checks
//! the argument count and content against that table, then decodes the
//! arguments back through `Codec` and compares the value.
//!
//! `PROPTEST_CASES` cases (256 by default); a failure prints the seed, and
//! `RUTIS_PROPTEST_SEED=<seed>` replays it (Q7.2).

use std::any::Any;
use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use crate::events::Codec;

// ── top-level shapes ────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Unit;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct Single(Json);

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct Pair(Json, Json);

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct Triple(Json, Json, Json);

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct Named {
    first: Json,
    second: Vec<Json>,
    third: BTreeMap<String, Json>,
    count: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
enum Choice {
    Unit,
    Newtype(Json),
    Tuple(Json, Json),
    Struct { a: Json, b: Vec<Json> },
}

/// A recursive value: arbitrary JSON below three layers of structs and enums.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct Nested {
    name: String,
    nodes: Vec<Node>,
    extras: BTreeMap<String, Json>,
    maybe: Option<Box<Nested>>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
enum Node {
    Null,
    Leaf(Json),
    Pair(Json, Json),
    Tag { id: u64, body: Box<Nested> },
}

// ── generating values ───────────────────────────────────────────

/// An arbitrary JSON value, bounded in depth and width.
fn json_value() -> BoxedStrategy<Json> {
    let leaf = prop_oneof![
        Just(Json::Null),
        any::<bool>().prop_map(Json::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<u64>().prop_map(|n| json!(n)),
        "[ -~]{0,8}".prop_map(Json::String),
        prop::num::f64::NORMAL.prop_map(|n| json!(n)),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Json::Array),
            prop::collection::btree_map("[a-z]{1,4}", inner, 0..4)
                .prop_map(|map| Json::Object(map.into_iter().collect())),
        ]
    })
    .boxed()
}

fn named() -> impl Strategy<Value = Named> {
    (
        json_value(),
        prop::collection::vec(json_value(), 0..3),
        prop::collection::btree_map("[a-z]{1,4}", json_value(), 0..3),
        prop::option::of(any::<u64>()),
    )
        .prop_map(|(first, second, third, count)| Named {
            first,
            second,
            third,
            count,
        })
}

fn choice() -> impl Strategy<Value = Choice> {
    let value = json_value();
    prop_oneof![
        Just(Choice::Unit),
        value.clone().prop_map(Choice::Newtype),
        (value.clone(), value.clone()).prop_map(|(a, b)| Choice::Tuple(a, b)),
        (value.clone(), prop::collection::vec(value.clone(), 0..3))
            .prop_map(|(a, b)| Choice::Struct { a, b }),
    ]
}

fn nested() -> impl Strategy<Value = Nested> {
    (
        json_value(),
        json_value(),
        prop::collection::vec(json_value(), 0..3),
    )
        .prop_map(|(head, tail, rest)| Nested {
            name: "node".into(),
            nodes: vec![
                Node::Pair(head.clone(), tail),
                Node::Leaf(head.clone()),
                Node::Tag {
                    id: 7,
                    body: Box::new(Nested {
                        name: "tag".into(),
                        nodes: vec![Node::Null],
                        extras: BTreeMap::from([("x".into(), head.clone())]),
                        maybe: None,
                    }),
                },
            ],
            extras: BTreeMap::from([("rest".into(), Json::Array(rest))]),
            maybe: Some(Box::new(Nested {
                name: "inner".into(),
                nodes: vec![Node::Leaf(head)],
                extras: BTreeMap::new(),
                maybe: None,
            })),
        })
}

/// One top-level shape with its payload.
#[derive(Clone, Debug)]
enum Case {
    Unit,
    UnitStruct,
    Json(Json),
    Newtype(Json),
    Pair(Pair),
    Triple(Triple),
    OneTuple(Json),
    TwoTuple(Json, Json),
    Seq(Vec<Json>),
    Map(BTreeMap<String, Json>),
    Named(Named),
    Choice(Choice),
    Nested(Nested),
}

fn cases() -> impl Strategy<Value = Case> {
    let value = json_value();
    prop_oneof![
        1 => Just(Case::Unit),
        1 => Just(Case::UnitStruct),
        3 => value.clone().prop_map(Case::Json),
        3 => value.clone().prop_map(Case::Newtype),
        3 => (value.clone(), value.clone()).prop_map(|(a, b)| Case::Pair(Pair(a, b))),
        2 => (value.clone(), value.clone(), value.clone())
            .prop_map(|(a, b, c)| Case::Triple(Triple(a, b, c))),
        1 => value.clone().prop_map(Case::OneTuple),
        2 => (value.clone(), value.clone()).prop_map(|(a, b)| Case::TwoTuple(a, b)),
        2 => prop::collection::vec(value.clone(), 0..4).prop_map(Case::Seq),
        2 => prop::collection::btree_map("[a-z]{1,4}", value.clone(), 0..4).prop_map(Case::Map),
        2 => named().prop_map(Case::Named),
        3 => choice().prop_map(Case::Choice),
        3 => nested().prop_map(Case::Nested),
    ]
}

// ── the table ───────────────────────────────────────────────────

/// The argument count the table gives a case, written out per shape so the
/// test never asks the codec how many it thinks there are. A bare
/// `serde_json::Value::Null` serializes as a unit, so the table's "unit" row
/// takes it even though its Rust type is not `()`.
fn table_count(case: &Case) -> usize {
    match case {
        Case::Unit | Case::UnitStruct => 0,
        Case::Pair(_) | Case::TwoTuple(..) => 2,
        Case::Triple(_) => 3,
        Case::Json(Json::Null) => 0,
        Case::Json(_)
        | Case::Newtype(_)
        | Case::OneTuple(_)
        | Case::Seq(_)
        | Case::Map(_)
        | Case::Named(_)
        | Case::Choice(_)
        | Case::Nested(_) => 1,
    }
}

/// The arguments the table gives a case: the payload itself for a one-argument
/// shape (the array for a one-field tuple), one per field for a tuple of two
/// or more, none for a unit.
fn table_args(case: &Case) -> Vec<Json> {
    match case {
        Case::Unit | Case::UnitStruct => Vec::new(),
        Case::Json(Json::Null) => Vec::new(),
        Case::Json(value) | Case::Newtype(value) => vec![value.clone()],
        Case::OneTuple(value) => vec![Json::Array(vec![value.clone()])],
        Case::Pair(Pair(a, b)) | Case::TwoTuple(a, b) => vec![a.clone(), b.clone()],
        Case::Triple(Triple(a, b, c)) => vec![a.clone(), b.clone(), c.clone()],
        Case::Seq(items) => vec![Json::Array(items.clone())],
        Case::Map(map) => vec![Json::Object(map.clone().into_iter().collect())],
        Case::Named(value) => vec![serde_json::to_value(value).expect("a named struct")],
        Case::Choice(value) => vec![serde_json::to_value(value).expect("an enum")],
        Case::Nested(value) => vec![serde_json::to_value(value).expect("a nested value")],
    }
}

// ── running it against the codec ────────────────────────────────

/// `value` through the codec: the arguments equal `expected`, the value
/// decodes back from them and from JSON, and the bounds are those of `T`.
fn roundtrip<T>(value: T, expected: Vec<Json>) -> Result<(), TestCaseError>
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug + Send + 'static,
{
    let codec = Codec::of::<T>();
    let actual = codec
        .to_args(&value as &dyn Any)
        .map_err(|error| TestCaseError::fail(format!("to_args({value:?}): {error}")))?;
    if actual != expected {
        return Err(TestCaseError::fail(format!(
            "to_args({value:?}) is {actual:?}, the table says {expected:?}"
        )));
    }
    let decoded = codec
        .from_args(expected.clone())
        .map_err(|error| TestCaseError::fail(format!("from_args({expected:?}): {error}")))?;
    let decoded = decoded
        .downcast::<T>()
        .map_err(|_| TestCaseError::fail("from_args returned another type"))?;
    if *decoded != value {
        return Err(TestCaseError::fail(format!(
            "from_args({expected:?}) is {decoded:?}, the value was {value:?}"
        )));
    }
    let json = codec
        .to_json(&value as &dyn Any)
        .map_err(|error| TestCaseError::fail(format!("to_json({value:?}): {error}")))?;
    let from_json = codec
        .from_json(json)
        .map_err(|error| TestCaseError::fail(format!("from_json({value:?}): {error}")))?;
    let from_json = from_json
        .downcast::<T>()
        .map_err(|_| TestCaseError::fail("from_json returned another type"))?;
    if *from_json != value {
        return Err(TestCaseError::fail(format!(
            "from_json of {value:?} is {from_json:?}"
        )));
    }
    Ok(())
}

fn check(case: Case) -> Result<(), TestCaseError> {
    let expected = table_args(&case);
    if expected.len() != table_count(&case) {
        return Err(TestCaseError::fail(format!(
            "the table disagrees with itself for {case:?}: {} arguments, {expected:?}",
            table_count(&case)
        )));
    }
    match case {
        Case::Unit => roundtrip((), expected),
        Case::UnitStruct => roundtrip(Unit, expected),
        Case::Json(value) => roundtrip(value, expected),
        Case::Newtype(value) => roundtrip(Single(value), expected),
        Case::Pair(value) => roundtrip(value, expected),
        Case::Triple(value) => roundtrip(value, expected),
        Case::OneTuple(value) => roundtrip((value,), expected),
        Case::TwoTuple(a, b) => roundtrip((a, b), expected),
        Case::Seq(items) => roundtrip(items, expected),
        Case::Map(map) => roundtrip(map, expected),
        Case::Named(value) => roundtrip(value, expected),
        Case::Choice(value) => roundtrip(value, expected),
        Case::Nested(value) => roundtrip(value, expected),
    }
}

/// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED` or a
/// fresh one; on failure, panic with the seed and the shrunk input.
fn run<S: Strategy>(name: &str, strategy: S, test: impl Fn(S::Value) -> Result<(), TestCaseError>)
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
    let started = std::time::Instant::now();
    if let Err(error) = runner.run(&strategy, test) {
        panic!("{name}: {error}\nreplay with RUTIS_PROPTEST_SEED={seed}");
    }
    eprintln!(
        "{name}: {cases} cases in {:?} (seed {seed})",
        started.elapsed()
    );
}

#[test]
fn event_arguments_match_the_shape_table() {
    run(
        "random nested values in every top-level shape",
        cases(),
        check,
    );
}

/// The count is the table's, not the decoder's: a value of another count is
/// refused (except the documented "one from itself": a tuple of two fields
/// may also arrive as one array).
#[test]
fn the_argument_count_decides_how_a_value_decodes() {
    let pair = Codec::of::<Pair>();
    assert!(pair.from_args(vec![json!(1), json!(2)]).is_ok());
    assert!(pair
        .from_args(vec![json!([1, 2])])
        .expect("one argument that is the array")
        .downcast::<Pair>()
        .is_ok());
    assert!(pair.from_args(vec![json!(1)]).is_err());
    assert!(pair.from_args(vec![json!(1), json!(2), json!(3)]).is_err());
    assert!(pair.from_args(Vec::new()).is_err());

    let named = Codec::of::<Named>();
    let one = json!({"first": 1, "second": [], "third": {}, "count": null});
    assert!(named.from_args(vec![one.clone()]).is_ok());
    assert!(named.from_args(vec![one.clone(), json!(0)]).is_err());
    assert!(named.from_args(Vec::new()).is_err());

    let unit = Codec::of::<Unit>();
    assert!(unit.from_args(Vec::new()).is_ok());
    assert!(unit.from_args(vec![Json::Null]).is_ok());
    assert!(unit.from_args(vec![json!(1)]).is_err());

    let boolean = Codec::of::<bool>();
    assert!(boolean.from_args(vec![json!(true)]).is_ok());
    assert!(boolean.from_args(Vec::new()).is_err());
    assert!(boolean.from_args(vec![json!(true), json!(false)]).is_err());

    // A JSON null is a unit-shaped value: no arguments, and none decode to
    // it. `None` is not unit-shaped: it serializes through `serialize_none`
    // and is one argument, null.
    let null = Codec::of::<Json>();
    assert_eq!(
        null.to_args(&Json::Null as &dyn Any).expect("to_args"),
        Vec::<Json>::new()
    );
    assert_eq!(
        *null
            .from_args(Vec::new())
            .expect("from_args")
            .downcast::<Json>()
            .expect("a json value"),
        Json::Null
    );
    let none = Codec::of::<Option<u64>>();
    assert_eq!(
        none.to_args(&None::<u64> as &dyn Any).expect("to_args"),
        vec![Json::Null]
    );
    assert_eq!(
        *none
            .from_args(vec![Json::Null])
            .expect("from_args")
            .downcast::<Option<u64>>()
            .expect("an option"),
        None
    );
}
