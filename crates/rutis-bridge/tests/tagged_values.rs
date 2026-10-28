//! Tagged values across the Rust side (#308): a call forwarded to a far
//! end that did not declare a value's capability fails, through a host's
//! own forwarding and through relays; generated types carry tagged values
//! through serde (`arg`, `decode_value`), and their integers beyond
//! ±(2^53−1) as bigints. The wire rules are tested in
//! `src/session/rpc/tests.rs`, the formats in `src/session/values.rs`.

use std::sync::{Arc, OnceLock};

use rutis_bridge::channel::PeerId;
use rutis_bridge::session::{
    arg, decode_value, BigInt, Bytes, Connection, Dispatch, Endpoint, Error, Format, Instant,
    Reply, Value, ValueMap, ValueSet,
};
use serde_json::json;

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// `local` without the capabilities of any tagged value: a far end older
/// than #308.
fn without_values(local: &str) -> Endpoint {
    let mut endpoint = Endpoint::rust(id(local));
    endpoint
        .capabilities
        .retain(|capability| !capability.starts_with("value-"));
    endpoint
}

/// A session between `left` and `right` over a memory channel.
fn session(
    left: Endpoint,
    left_dispatch: Arc<dyn Dispatch>,
    right: Endpoint,
    right_dispatch: Arc<dyn Dispatch>,
) -> (Connection, Connection) {
    let (a, b) = rutis_bridge::memory::pair();
    (
        Connection::open_with(a, left_dispatch, Format::Endpoint(left)).unwrap(),
        Connection::open_with(b, right_dispatch, Format::Endpoint(right)).unwrap(),
    )
}

fn instant() -> Instant {
    "2026-10-15T03:22:15.123456789Z".parse().unwrap()
}

/// The host: forwards every call it receives to its downstream session,
/// as a host forwards a call to the runtime serving a service.
struct Forward(OnceLock<Connection>);
impl Dispatch for Forward {
    fn invoke(&self, source: &Connection, target: &str, method: &str, args: Value) -> Reply {
        let down = self.0.get().unwrap().clone();
        let (target, method) = (target.to_owned(), method.to_owned());
        let calling = down.clone();
        let forwarded = down.forward_async(source, async move {
            calling.invoke_async(&target, &method, args).await
        });
        Ok(Value::future(forwarded))
    }
}

/// A downstream far end: `echo` returns its arguments; `call` calls the
/// function it is passed and returns what it returns.
struct Downstream;
impl Dispatch for Downstream {
    fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
        match method {
            "echo" => Ok(args),
            "call" => args
                .list()?
                .remove(0)
                .reference()?
                .call(Value::List(vec![])),
            _ => Err(Error::Value(format!("no method {method}"))),
        }
    }
}

struct Nothing;
impl Dispatch for Nothing {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        Err(Error::Value("nothing here".into()))
    }
}

/// `app` → host → `down`, with `down` greeting as `downstream`.
async fn chain(downstream: Endpoint) -> (Connection, Vec<Connection>) {
    let forward = Arc::new(Forward(OnceLock::new()));
    let (app, host_up) = session(
        Endpoint::rust(id("app")),
        Arc::new(Nothing),
        Endpoint::rust(id("host")),
        forward.clone(),
    );
    let (host_down, down) = session(
        Endpoint::rust(id("host")),
        Arc::new(Nothing),
        downstream,
        Arc::new(Downstream),
    );
    assert!(forward.0.set(host_down.clone()).is_ok());
    for session in [&app, &host_up, &host_down, &down] {
        session.ready().await.unwrap();
    }
    (app, vec![host_up, host_down, down])
}

fn names_the_capability(result: Reply, capability: &str) {
    match result {
        Err(Error::Remote { message, .. }) => assert!(
            message.contains(&format!("did not declare the capability {capability}")),
            "{message}"
        ),
        other => panic!("an error naming {capability} expected, got {other:?}"),
    }
}

/// guarantee: bridge/value-tags
///
/// A host forwards a call from a far end that sends tagged values to one
/// that does not take them: the forwarded call fails, naming the value's
/// capability, and both sessions go on. Forwarded to a far end that takes
/// them, the same values arrive.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_forwarded_to_a_far_end_without_the_capability_fails() {
    let (app, rest) = chain(without_values("old")).await;
    let args = || Value::List(vec![Value::Instant(instant())]);
    names_the_capability(
        rutis_bridge::session::settle(app.invoke_async("svc", "echo", args()).await.unwrap()).await,
        "value-instant",
    );
    let data = Value::List(vec![Value::Data(json!({ "at": "noon" }))]);
    let back =
        rutis_bridge::session::settle(app.invoke_async("svc", "echo", data.clone()).await.unwrap())
            .await
            .unwrap();
    assert_eq!(back, data);
    for session in rest.iter().chain([&app]) {
        assert!(session.close_reason().is_none());
    }

    let (app, _rest) = chain(Endpoint::rust(id("new"))).await;
    let back =
        rutis_bridge::session::settle(app.invoke_async("svc", "echo", args()).await.unwrap())
            .await
            .unwrap();
    assert_eq!(back, args());
}

/// guarantee: bridge/value-tags
///
/// Through a relay: the downstream far end calls a function the upstream
/// one passed; its result, an instant, is relayed back to a far end that
/// does not take instants, so that call fails there, and the failure
/// reaches the upstream caller. The sessions go on.
#[tokio::test(flavor = "multi_thread")]
async fn a_result_relayed_to_a_far_end_without_the_capability_fails() {
    let (app, rest) = chain(without_values("old")).await;
    let callback = Value::callback(|_| Ok(Value::Instant(instant())));
    let result = app
        .invoke_async("svc", "call", Value::List(vec![callback.clone()]))
        .await;
    names_the_capability(
        match result {
            Ok(value) => rutis_bridge::session::settle(value).await,
            error => error,
        },
        "value-instant",
    );
    for session in rest.iter().chain([&app]) {
        assert!(session.close_reason().is_none());
    }

    let (app, _rest) = chain(Endpoint::rust(id("new"))).await;
    let back = rutis_bridge::session::settle(
        app.invoke_async("svc", "call", Value::List(vec![callback]))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(back, Value::Instant(instant()));
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Reading {
    at: Instant,
    raw: Bytes,
    tags: ValueSet<String>,
    counts: ValueMap<String, f64>,
    big: Option<BigInt>,
}

fn reading() -> Reading {
    Reading {
        at: instant(),
        raw: Bytes(b"hello".to_vec()),
        tags: ValueSet(vec!["a".into(), "b".into()]),
        counts: ValueMap(vec![("x".into(), 1.5)]),
        big: Some("123456789012345678901234567890".parse().unwrap()),
    }
}

/// Generated types (serde) carry tagged values under their tags: `arg`
/// makes them tagged values in a record, `decode_value` reads them back. A
/// tagged value that would land in dynamic JSON fails the decode. Outside
/// a call they serialize as text and arrays.
#[test]
fn generated_types_carry_tagged_values_through_serde() {
    let value = arg(&reading()).unwrap();
    let Value::Record(fields) = &value else {
        panic!("a record expected, got {value:?}")
    };
    assert_eq!(fields["at"], Value::Instant(instant()));
    assert_eq!(fields["raw"], Value::Bytes(b"hello".to_vec()));
    assert_eq!(
        fields["tags"],
        Value::Set(vec![Value::Data(json!("a")), Value::Data(json!("b"))])
    );
    assert_eq!(
        fields["counts"],
        Value::Map(vec![(Value::Data(json!("x")), Value::Data(json!(1.5)))])
    );
    assert_eq!(
        fields["big"],
        Value::BigInt("123456789012345678901234567890".parse().unwrap())
    );
    let back: Reading = decode_value(value.clone()).unwrap();
    assert_eq!(back, reading());

    // Not JSON: it does not land in dynamic JSON, nor in `json()`.
    let error = decode_value::<serde_json::Value>(value.clone()).unwrap_err();
    assert!(error.to_string().contains("tagged value"), "{error}");
    let error = value.json().unwrap_err();
    assert!(error.to_string().contains("received an instant"), "{error}");
    // Nor into another tagged type.
    let error = decode_value::<Bytes>(Value::Instant(instant())).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("expected bytes, received an instant"),
        "{error}"
    );

    assert_eq!(
        serde_json::to_value(reading()).unwrap(),
        json!({
            "at": "2026-10-15T03:22:15.123456789Z",
            "raw": "aGVsbG8=",
            "tags": ["a", "b"],
            "counts": [["x", 1.5]],
            "big": "123456789012345678901234567890",
        })
    );
    let read: Reading = serde_json::from_value(serde_json::to_value(reading()).unwrap()).unwrap();
    assert_eq!(read, reading());
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Counters {
    id: u64,
    delta: i64,
    safe: i64,
    wide: i128,
    huge: u128,
    ids: ValueMap<u64, String>,
    levels: Vec<i64>,
    ratio: f64,
}

const SAFE: i64 = (1 << 53) - 1;

fn counters() -> Counters {
    Counters {
        id: u64::MAX,
        delta: -SAFE - 1,
        safe: SAFE,
        wide: i128::MIN,
        huge: u128::MAX,
        ids: ValueMap(vec![(1 << 60, "far".into()), (7, "near".into())]),
        levels: vec![SAFE, SAFE + 1],
        ratio: 9007199254740993.0,
    }
}

/// guarantee: bridge/value-tags
///
/// Rust integers in generated types (serde) need no wrapper (design V6):
/// one beyond ±(2^53−1) crosses as a bigint, `i128` and `u128` beyond 64
/// bits too, and a received bigint decodes into an integer type that holds
/// it. To a far end without `value-bigint`, the call fails, naming the
/// capability; integers within the range still cross as data.
#[tokio::test(flavor = "multi_thread")]
async fn rust_integers_beyond_2_53_cross_as_bigints() {
    let value = arg(&counters()).unwrap();
    let Value::Record(fields) = &value else {
        panic!("a record expected, got {value:?}")
    };
    // Beyond 64 bits JSON data cannot hold them: bigints already.
    assert_eq!(fields["wide"], Value::BigInt(i128::MIN.into()));
    assert_eq!(fields["huge"], Value::BigInt(u128::MAX.into()));

    let (near, far) = session(
        Endpoint::rust(id("near")),
        Arc::new(Nothing),
        Endpoint::rust(id("far")),
        Arc::new(Downstream),
    );
    near.ready().await.unwrap();
    far.ready().await.unwrap();
    let echoed = near
        .invoke_async("t", "echo", Value::List(vec![value]))
        .await
        .unwrap();
    let Value::List(mut echoed) = echoed else {
        panic!("a list expected")
    };
    let Value::Record(mut fields) = echoed.remove(0) else {
        panic!("a record expected")
    };
    assert_eq!(fields["id"], Value::BigInt(u64::MAX.into()));
    assert_eq!(fields["delta"], Value::BigInt((-SAFE - 1).into()));
    assert_eq!(fields["safe"], Value::Data(json!(SAFE)));
    assert_eq!(
        fields["levels"],
        Value::List(vec![
            Value::Data(json!(SAFE)),
            Value::BigInt((SAFE + 1).into())
        ])
    );
    assert_eq!(fields["ratio"], Value::Data(json!(9007199254740993.0)));
    assert_eq!(
        fields["ids"],
        Value::Map(vec![
            (
                Value::BigInt((1u64 << 60).into()),
                Value::Data(json!("far"))
            ),
            (Value::Data(json!(7)), Value::Data(json!("near"))),
        ])
    );
    // Within 64 bits the bigints decode into the integer fields. Past 64
    // bits serde has no path from data into an `i128`: such a bigint
    // decodes into a `BigInt` only.
    let wide = fields.remove("wide").unwrap();
    let huge = fields.remove("huge").unwrap();
    assert_eq!(decode_value::<BigInt>(wide).unwrap(), i128::MIN.into());
    assert!(decode_value::<u128>(huge).is_err());
    fields.insert("wide".into(), Value::Data(json!(-5)));
    fields.insert("huge".into(), Value::Data(json!(5)));
    let back: Counters = decode_value(Value::Record(fields)).unwrap();
    assert_eq!(
        back,
        Counters {
            wide: -5,
            huge: 5,
            ..counters()
        }
    );

    // A received bigint decodes into an integer type that holds it, and
    // into a `BigInt`; into one that does not hold it, it fails.
    assert_eq!(
        decode_value::<u64>(Value::BigInt(u64::MAX.into())).unwrap(),
        u64::MAX
    );
    assert_eq!(decode_value::<i64>(Value::BigInt(5.into())).unwrap(), 5);
    assert_eq!(
        decode_value::<BigInt>(Value::BigInt(5.into())).unwrap(),
        5.into()
    );
    assert!(decode_value::<i64>(Value::BigInt(u64::MAX.into())).is_err());
    assert!(decode_value::<u32>(Value::BigInt((1i64 << 40).into())).is_err());
    // As data, a bigint within 64 bits is its number.
    assert_eq!(
        decode_value::<serde_json::Value>(Value::BigInt(u64::MAX.into())).unwrap(),
        json!(u64::MAX)
    );
    assert_eq!(
        Value::BigInt(i64::MIN.into()).json().unwrap(),
        json!(i64::MIN)
    );
    assert!(Value::BigInt(u128::MAX.into()).json().is_err());

    let (old, _old_far) = session(
        Endpoint::rust(id("near")),
        Arc::new(Nothing),
        without_values("old"),
        Arc::new(Downstream),
    );
    old.ready().await.unwrap();
    let args = Value::List(vec![arg(&(1u64 << 60)).unwrap()]);
    match old.invoke_async("t", "echo", args).await {
        Err(Error::Value(message)) => assert!(
            message.contains("did not declare the capability value-bigint"),
            "{message}"
        ),
        other => panic!("a value error expected, got {other:?}"),
    }
    let safe = Value::List(vec![arg(&SAFE).unwrap()]);
    assert_eq!(
        old.invoke_async("t", "echo", safe.clone()).await.unwrap(),
        safe
    );
    assert!(old.close_reason().is_none());
}
