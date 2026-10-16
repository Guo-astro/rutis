//! Tagged values across the Rust side (#308): a call forwarded to a far
//! end that did not declare a value's capability fails, through a host's
//! own forwarding and through relays; `Process::emit` checks its arguments
//! against the runtime's capabilities; generated types carry tagged values
//! through serde (`arg`, `decode_value`). The wire rules are tested in
//! `src/session/rpc/tests.rs`, the formats in `src/session/values.rs`.

use std::sync::{Arc, Mutex, OnceLock};

use rutis_bridge::channel::PeerId;
use rutis_bridge::runtime::{Mount, Process};
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
    let (a, b) = rutis_bridge::transport::memory::pair();
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

/// A runtime that mounts nothing and records the events emitted into it.
#[derive(Default)]
struct Runtime(Mutex<Vec<Value>>);
impl Dispatch for Runtime {
    fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
        match (target, method) {
            ("", "mount") => Ok(Value::Data(
                json!({ "services": {}, "features": ["events"] }),
            )),
            ("", "emit") => {
                self.0.lock().unwrap().push(args);
                Ok(Value::Undefined)
            }
            _ => Err(Error::Value(format!("no method {method}"))),
        }
    }
}

async fn runtime(endpoint: Endpoint) -> (Arc<Process>, Arc<Runtime>, Connection) {
    let (a, b) = rutis_bridge::transport::memory::pair();
    let recorded = Arc::new(Runtime::default());
    let far = Connection::open_with(b, recorded.clone(), Format::Endpoint(endpoint)).unwrap();
    let process = Process::attach(
        a,
        Mount {
            emits: vec!["tick".into()],
            ..Mount::default()
        },
        Format::Endpoint(Endpoint::rust(id("host"))),
    )
    .await
    .unwrap();
    (process, recorded, far)
}

/// guarantee: bridge/value-tags
///
/// Events the host emits into a runtime (`Process::emit`) carry tagged
/// values only to a runtime that declared them: to an older one the
/// emission fails, naming the capability, and later emissions go on.
#[tokio::test(flavor = "multi_thread")]
async fn process_emit_checks_the_runtimes_capabilities() {
    let (process, recorded, _far) = runtime(without_values("rt")).await;
    match process
        .emit("tick", vec![Value::Bytes(b"payload".to_vec())])
        .await
    {
        Err(Error::Value(message)) => assert!(
            message.contains("did not declare the capability value-bytes"),
            "{message}"
        ),
        other => panic!("an error naming value-bytes expected, got {other:?}"),
    }
    process.emit("tick", vec![json!(1).into()]).await.unwrap();
    assert_eq!(recorded.0.lock().unwrap().len(), 1);

    let (process, recorded, _far) = runtime(Endpoint::rust(id("rt"))).await;
    process
        .emit("tick", vec![Value::Bytes(b"payload".to_vec())])
        .await
        .unwrap();
    assert_eq!(
        recorded.0.lock().unwrap()[0],
        Value::List(vec![
            Value::Data(json!("tick")),
            Value::List(vec![Value::Bytes(b"payload".to_vec())]),
        ])
    );
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
