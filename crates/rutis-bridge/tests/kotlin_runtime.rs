#![cfg(feature = "kotlin")]
//! The Kotlin runtime (the SDK's jvm target, `Rutis.serve` in the
//! conformance jar, started through the generic launcher) beyond the shared
//! suites (design docs/design-android-runtime-2026-10-12.md §九 专项): a host
//! older than 0.13 is refused with the exact line and status 1; errors carry
//! their code and data both ways, and again when thrown on; each row calls a
//! host service by the id its own `isolate` gives it, so the host sees the
//! calling row; a dropped call cancels the method's coroutine.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::{row_projection, Mount, Process};
use rutis_bridge::session::{caller_row, host_key, CallerRow, Error, HostDispatch};
use rutis_bridge::session::{settle, Reply, Value as RpcValue};
use rutis_test_support::until;
use serde_json::{json, Value};

mod kotlin;

fn repo() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn kotlin(project: &Path) -> Arc<Process> {
    let launcher = kotlin::launcher().cwd(project);
    Process::mount(
        &repo().join("node/rutis-runtime"),
        Mount {
            anchor: Some(project),
            launcher: Some(&launcher),
            ..Mount::default()
        },
    )
    .await
    .unwrap()
}

/// Load `entry` as row `key` and wait for its service `name`.
async fn service(
    process: &Arc<Process>,
    ctx: &Ctx,
    key: &str,
    entry: &str,
    name: &str,
) -> Arc<dyn HostDispatch> {
    let described = process.describe_row(Path::new(entry)).await.unwrap();
    let projection = row_projection(&described.provides);
    projection.attach(ctx, process.clone()).unwrap();
    process
        .load_row_exporting(
            key,
            Path::new(entry),
            json!(null),
            &[],
            &[],
            &described.provides,
            projection,
            &[],
        )
        .await
        .unwrap();
    let key = host_key(name);
    until(|| ctx.get_at::<dyn HostDispatch>(&key).is_some(), name).await;
    ctx.get_at::<dyn HostDispatch>(&key).unwrap()
}

/// A host whose `hello` lacks `events-v2` (rutis 0.12 and older) is
/// refused right after the handshake: the runtime writes the state line
/// and exits with status 1, having served nothing.
/// guarantee: events/old-host-refused
#[tokio::test(flavor = "multi_thread")]
async fn a_host_without_events_v2_is_refused() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TOKEN: &str = "old-host-token";
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let launcher = kotlin::launcher();
    let mut child = tokio::process::Command::new(&launcher.program)
        .args(&launcher.args)
        .arg(format!("tcp:{address}"))
        .arg(dir.path())
        .env("RUTIS_CHANNEL_TOKEN", TOKEN)
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (mut stream, _) = tokio::select! {
        accepted = listener.accept() => accepted.unwrap(),
        status = child.wait() => panic!("the runtime ended before connecting: {status:?}"),
    };
    let mut presented = vec![0; TOKEN.len() + 1];
    stream.read_exact(&mut presented).await.unwrap();
    assert_eq!(presented, format!("{TOKEN}\n").as_bytes());
    stream
        .write_all(b"{\"op\":\"hello\",\"version\":2,\"capabilities\":[\"sync-wait\"]}\n")
        .await
        .unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    let status = child.wait().await.unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(
        stderr.lines().any(|line| line
            == "rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping"),
        "the exact state line: {stderr}"
    );
    // It sent nothing but its own greeting.
    let mut sent = String::new();
    let _ = stream.read_to_string(&mut sent).await;
    for line in sent.lines() {
        let frame: Value = serde_json::from_str(line).unwrap();
        assert_eq!(frame["op"], "hello", "{sent}");
    }
}

/// Errors cross with their name, message, code and data both ways: a
/// Kotlin exception's `code` and `data` properties reach Rust, a Rust
/// error's reach the Kotlin plugin as a RutisException, and one thrown on
/// crosses back whole.
/// guarantee: bridge/error-code-and-data
#[tokio::test(flavor = "multi_thread")]
async fn errors_carry_their_code_and_data_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let process = kotlin(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let work = service(&process, &ctx, "p", "probe", "work").await;

    let error = work.invoke("fail", json!([]).into()).unwrap_err();
    let Error::Remote { name, message, .. } = &error else {
        panic!("an error from the plugin: {error:?}")
    };
    assert_eq!(
        (name.as_str(), message.as_str()),
        ("WeatherError", "too windy")
    );
    assert_eq!(error.code(), Some("Stormy"));
    assert_eq!(
        error.data(),
        Some(json!({ "wind": 1152921504606846976_u64 }))
    );

    let quota = || {
        RpcValue::callback(|_| {
            Err(Error::Remote {
                name: "Quota".into(),
                message: "over".into(),
                graph: Some(json!({
                    "root": { "type": "reference", "value": 0 },
                    "nodes": [{
                        "type": "error", "name": "Quota", "message": "over", "stack": "Quota: over",
                        "code": "E42", "data": { "type": "data", "value": { "left": 3 } },
                    }],
                })),
            })
        })
    };
    let seen = work
        .invoke("inspect", RpcValue::List(vec![quota()]))
        .unwrap();
    assert_eq!(seen.json().unwrap(), json!("Quota E42 {left=3}"));
    let again = work
        .invoke("relay", RpcValue::List(vec![quota()]))
        .unwrap_err();
    assert_eq!(again.code(), Some("E42"), "thrown on, whole: {again:?}");
    assert_eq!(again.data(), Some(json!({ "left": 3 })));
    drop(work);
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// Each name registered, with the key of the calling row, if any.
type Heard = Vec<(String, Option<String>)>;

/// `register(name)` records `name` with the row that calls, if any.
#[derive(Clone, Default)]
struct Ledger(Arc<Mutex<Heard>>);

impl HostDispatch for Ledger {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        assert_eq!(method, "register");
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        let row = caller_row().map(|row| row.key().to_owned());
        self.0.lock().unwrap().push((name, row));
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "register": "sync" }))
    }
}

/// Each row reaches a host service by the id its own `isolate` gives the
/// name (the label opaque), so the host sees which row calls; a row of the
/// shared lease names none.
/// guarantee: bridge/caller-row
#[tokio::test(flavor = "multi_thread")]
async fn each_row_calls_by_its_own_service_id() {
    let dir = tempfile::tempdir().unwrap();
    let process = kotlin(dir.path()).await;
    assert!(process.supports("scopes"));
    let ledger = Ledger::default();
    let dispatch: Arc<dyn HostDispatch> = Arc::new(ledger.clone());
    let root = Ctx::root().unwrap();
    let mut leases = Vec::new();
    for key in ["a.1", "b.1"] {
        let id = key.split('.').next().unwrap();
        let row = CallerRow::new(id, key, root.clone());
        let lease = process
            .lease_host_for("ledger", None, &row, dispatch.clone(), None)
            .await
            .unwrap();
        let isolate = [("ledger".to_owned(), lease.label().unwrap().to_owned())];
        process
            .load_row(
                key,
                Path::new("registrant"),
                json!({ "name": id }),
                &isolate,
                &[],
                &[],
            )
            .await
            .unwrap();
        leases.push(lease);
    }
    let shared = process
        .lease_host("ledger", dispatch.clone(), None)
        .await
        .unwrap();
    process
        .load_row(
            "c.1",
            Path::new("registrant"),
            json!({ "name": "c" }),
            &[],
            &[],
            &[],
        )
        .await
        .unwrap();
    let heard = ledger.0.lock().unwrap().clone();
    let of = |name: &str| {
        heard
            .iter()
            .find(|(registered, _)| registered == name)
            .map(|(_, row)| row.clone())
            .unwrap_or_else(|| panic!("{name} registered: {heard:?}"))
    };
    assert_eq!(of("a"), Some("a.1".into()));
    assert_eq!(of("b"), Some("b.1".into()));
    assert_eq!(of("c"), None);
    for key in ["a.1", "b.1", "c.1"] {
        process.unload_row(key).await.unwrap();
    }
    for lease in leases {
        lease.release().await.unwrap();
    }
    shared.release().await.unwrap();
    process.dispose().await.unwrap();
}

/// A dropped async call cancels the method's coroutine (the `cancel` frame
/// cancels the call's scope and interrupts its thread); the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_async_call_cancels_the_coroutine() {
    let dir = tempfile::tempdir().unwrap();
    let process = kotlin(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let work = service(&process, &ctx, "p", "probe", "work").await;
    let waiting = work.invoke("wait", json!([]).into()).unwrap();
    {
        let settled = settle(waiting);
        let mut settled = std::pin::pin!(settled);
        let probe = work.clone();
        tokio::select! {
            biased;
            ended = &mut settled => panic!("the call only ends when cancelled: {ended:?}"),
            () = until(
                move || {
                    probe
                        .invoke("started", json!([]).into())
                        .ok()
                        .and_then(|value| value.json().ok())
                        == Some(json!(true))
                },
                "the method started",
            ) => {}
        }
    }
    let probe = work.clone();
    until(
        move || {
            probe
                .invoke("aborted", json!([]).into())
                .ok()
                .and_then(|value| value.json().ok())
                == Some(json!(true))
        },
        "the cancellation",
    )
    .await;
    let ping = work.invoke("ping", json!([]).into()).unwrap();
    assert_eq!(ping.json().unwrap(), json!("pong"), "the session goes on");
    drop(work);
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}
