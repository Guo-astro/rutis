//! Tagged values between the Rust session and the Python runtime (#308
//! phase 3): every tag Rust sends comes back from Python's `conformance`
//! echo as Python holds it (instants, local datetimes and times truncated
//! to the microsecond, design V7); a map whose keys collide in Python
//! fails only that call, and the session goes on. The other direction
//! (values Python makes) is in `weather.expected.json`, checked by
//! `session_matrix.rs`.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis_bridge::channel::PeerId;
use rutis_bridge::session::{Connection, Endpoint, Error, Format, Value};
use serde_json::json;

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Nothing;
impl rutis_bridge::session::Dispatch for Nothing {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> rutis_bridge::session::Reply {
        Err(Error::Value("main serves nothing".into()))
    }
}

/// The Python session conformance target on a Unix socket, as endpoint
/// `python`.
async fn python() -> (Connection, tokio::process::Child) {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("values.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let child = tokio::process::Command::new("python3")
        .arg(repo().join("python/rutis/tests/conformance_session.py"))
        .arg(&socket)
        .env("PYTHONPATH", repo().join("python/rutis"))
        .kill_on_drop(true)
        .spawn()
        .unwrap();
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
    let format = Format::Endpoint(Endpoint::rust(id("main")).expect(id("python")));
    let connection = Connection::open_with(channel, Arc::new(Nothing), format).unwrap();
    connection.ready().await.unwrap();
    (connection, child)
}

fn echo(far: &Connection, value: Value) -> Result<Value, Error> {
    far.invoke("conformance", "echo", Value::List(vec![value]))
}

/// guarantee: bridge/value-tags (Python). Every tag crosses to Python and
/// back; times come back to the microsecond, truncated.
#[tokio::test(flavor = "multi_thread")]
async fn python_echoes_every_tagged_value() {
    let (far, _child) = python().await;
    tokio::task::spawn_blocking(move || {
        let sent = Value::List(vec![
            Value::Instant("2026-10-15T03:22:15.123456789Z".parse().unwrap()),
            Value::Instant("1969-12-31T23:59:59.9999999Z".parse().unwrap()),
            Value::DateTime("2026-10-15T14:30:05.25".parse().unwrap()),
            Value::Date("2026-10-15".parse().unwrap()),
            Value::Time("23:59:59.000000999".parse().unwrap()),
            Value::Bytes(b"hello".to_vec()),
            Value::BigInt("-123456789012345678901".parse().unwrap()),
            Value::Decimal("-12.3400".parse().unwrap()),
            Value::Uuid("0f8fad5b-d9cb-469f-a165-70867728950e".parse().unwrap()),
            Value::Map(vec![
                (Value::Data(json!("a")), Value::Data(json!(1))),
                (Value::Data(json!(2)), Value::Undefined),
                (
                    Value::Date("2026-01-01".parse().unwrap()),
                    Value::Set(vec![Value::Data(json!("x"))]),
                ),
                (Value::BigInt("10".parse().unwrap()), Value::Set(vec![])),
            ]),
            Value::Record(
                [(
                    "at".to_owned(),
                    Value::Uuid("00000000-0000-0000-0000-000000000001".parse().unwrap()),
                )]
                .into(),
            ),
            Value::Data(json!({ "plain": [1, 2.5, null] })),
        ]);
        let expected = Value::List(vec![
            Value::Instant("2026-10-15T03:22:15.123456Z".parse().unwrap()),
            Value::Instant("1969-12-31T23:59:59.999999Z".parse().unwrap()),
            Value::DateTime("2026-10-15T14:30:05.25".parse().unwrap()),
            Value::Date("2026-10-15".parse().unwrap()),
            Value::Time("23:59:59".parse().unwrap()),
            Value::Bytes(b"hello".to_vec()),
            Value::BigInt("-123456789012345678901".parse().unwrap()),
            Value::Decimal("-12.3400".parse().unwrap()),
            Value::Uuid("0f8fad5b-d9cb-469f-a165-70867728950e".parse().unwrap()),
            Value::Map(vec![
                (Value::Data(json!("a")), Value::Data(json!(1))),
                // Undefined is null in data (design V10).
                (Value::Data(json!(2)), Value::Data(json!(null))),
                (
                    Value::Date("2026-01-01".parse().unwrap()),
                    Value::Set(vec![Value::Data(json!("x"))]),
                ),
                (Value::BigInt("10".parse().unwrap()), Value::Set(vec![])),
            ]),
            Value::Record(
                [(
                    "at".to_owned(),
                    Value::Uuid("00000000-0000-0000-0000-000000000001".parse().unwrap()),
                )]
                .into(),
            ),
            Value::Data(json!({ "plain": [1, 2.5, null] })),
        ]);
        assert_eq!(echo(&far, sent).unwrap(), expected);
    })
    .await
    .unwrap();
}

/// guarantee: bridge/value-tags (Python). Keys distinct on the wire but
/// one key in a Python dict (1 and true, 1 and a bigint 1) fail only that
/// call; the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn keys_that_collide_in_python_fail_only_that_call() {
    let (far, _child) = python().await;
    tokio::task::spawn_blocking(move || {
        for keys in [
            [Value::Data(json!(1)), Value::Data(json!(true))],
            [Value::Data(json!(1)), Value::BigInt("1".parse().unwrap())],
            [
                Value::Decimal("1.0".parse().unwrap()),
                Value::Decimal("1.00".parse().unwrap()),
            ],
        ] {
            let map = Value::Map(
                keys.iter()
                    .cloned()
                    .map(|key| (key, Value::Data(json!(0))))
                    .collect(),
            );
            for value in [map, Value::Set(keys.to_vec())] {
                match echo(&far, value) {
                    Err(Error::Remote { name, message, .. }) => {
                        assert_eq!(name, "ValueError");
                        assert!(message.contains("equal in Python"), "{message}");
                    }
                    other => panic!("expected the call to fail, got {other:?}"),
                }
            }
        }
        assert!(far.close_reason().is_none());
        assert_eq!(
            echo(&far, Value::Data(json!("still here"))).unwrap(),
            Value::Data(json!("still here"))
        );
    })
    .await
    .unwrap();
}
