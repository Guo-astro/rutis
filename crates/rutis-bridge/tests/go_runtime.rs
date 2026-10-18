#![cfg(feature = "go")]
//! The Go runtime speaks the same protocol and row contract as the Node and
//! Python ones, and carries Go's values across: describe, load with exports,
//! lease hosts, sync and async calls, callbacks, cancellation reaching the
//! method's context, errors by type name, panics, objects by reference.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use rutis::Ctx;
use rutis_bridge::runtime::{row_projection, Launcher, Mount, Process};
use rutis_bridge::session::{host_key, Error, HostDispatch};
use rutis_bridge::session::{settle, Reply, Value as RpcValue};
use rutis_test_support::until;
use serde_json::{json, Value};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The test plugins (`weather`, `probe`) in one binary, built once.
fn binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let binary = directory.join(if cfg!(windows) {
            "plugins.exe"
        } else {
            "plugins"
        });
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./internal/fixtures/runtime")
            .current_dir(repo().join("go/rutis"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build");
        binary
    })
}

struct Clock(Arc<AtomicUsize>);

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "now");
        Ok(json!(self.0.fetch_add(1, Ordering::SeqCst)).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "now": "sync" }))
    }
}

async fn go(project: &Path) -> Arc<Process> {
    let launcher = Launcher::go(binary(), project);
    Process::mount(
        &repo().join("go/rutis"),
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
    config: Value,
    name: &str,
) -> Arc<dyn HostDispatch> {
    let described = process.describe_row(Path::new(entry)).await.unwrap();
    let projection = row_projection(&described.provides);
    projection.attach(ctx, process.clone()).unwrap();
    process
        .load_row_exporting(
            key,
            Path::new(entry),
            config,
            &[],
            &[],
            &described.provides,
            projection,
            &[],
        )
        .await
        .unwrap();
    let key = host_key(name);
    until(
        || ctx.get_as::<dyn HostDispatch>(key.clone()).is_some(),
        name,
    )
    .await;
    ctx.get_as::<dyn HostDispatch>(key).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn go_rows_follow_the_row_contract() {
    let dir = tempfile::tempdir().unwrap();
    let process = go(dir.path()).await;
    for feature in ["rows.v2", "hosts", "leaf", "scopes"] {
        assert!(process.supports(feature), "{feature}");
    }

    let described = process.describe_row(Path::new("weather")).await.unwrap();
    assert_eq!(described.inject, ["clock"]);
    assert_eq!(
        Value::Object(described.provides.clone()),
        json!({ "weather": {
            "today": "sync", "later": "async", "each": "sync", "echo": "sync", "crash": "sync",
            "samples": "sync", "sample": "sync",
        } })
    );
    assert_eq!(
        described.config.unwrap()["properties"]["city"],
        json!({ "type": "string", "description": "the city to report on" })
    );

    // A plugin the binary does not have: the error lists those it has.
    match process.describe_row(Path::new("nowhere")).await {
        Err(Error::Remote { name, message, .. }) => {
            assert_eq!(name, "NotFound");
            assert!(message.contains("probe, weather"), "{message}");
        }
        other => panic!("describing a missing plugin should fail, got {other:?}"),
    }

    let ticks = Arc::new(AtomicUsize::new(0));
    let lease = process
        .lease_host("clock", Arc::new(Clock(ticks.clone())), None)
        .await
        .unwrap();
    let ctx = Ctx::root().unwrap();
    let weather = service(
        &process,
        &ctx,
        "w",
        "weather",
        json!({ "city": "Bergen" }),
        "weather",
    )
    .await;
    let sync = weather.clone();
    let today = tokio::task::spawn_blocking(move || sync.invoke("today", json!([]).into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(today.json().unwrap(), json!("Bergen at 0"));
    let later = settle(weather.invoke("later", json!([]).into()).unwrap())
        .await
        .unwrap();
    assert_eq!(later.json().unwrap(), json!("Bergen later"));
    let callback = RpcValue::callback(|args| {
        let [day]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        Ok(json!(day.to_uppercase()).into())
    });
    let sync = weather.clone();
    let days =
        tokio::task::spawn_blocking(move || sync.invoke("each", RpcValue::List(vec![callback])))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(days.json().unwrap(), json!(["MON", "TUE"]));
    drop(weather);

    // A new config restarts the row with it.
    process
        .update_row("w", json!({ "city": "Oslo" }))
        .await
        .unwrap();
    let key = host_key("weather");
    until(
        || {
            ctx.get_as::<dyn HostDispatch>(key.clone())
                .and_then(|weather| weather.invoke("today", json!([]).into()).ok())
                .and_then(|today| today.json().ok())
                .is_some_and(|today| today.as_str().unwrap_or("").starts_with("Oslo"))
        },
        "the new config",
    )
    .await;

    process.unload_row("w").await.unwrap();
    until(
        || ctx.get_as::<dyn HostDispatch>(key.clone()).is_none(),
        "the withdrawal",
    )
    .await;
    lease.release().await.unwrap();
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_async_call_cancels_the_methods_context() {
    let dir = tempfile::tempdir().unwrap();
    let process = go(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let work = service(&process, &ctx, "p", "probe", json!(null), "work").await;
    let waiting = work.invoke("wait", json!([]).into()).unwrap();
    {
        let settled = settle(waiting);
        let mut settled = std::pin::pin!(settled);
        // Dropped once the method runs (and waits for its context).
        let probe = work.clone();
        tokio::select! {
            // The call first: polled, it sends what is then dropped.
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

#[tokio::test(flavor = "multi_thread")]
async fn errors_cross_with_their_names_and_panics_with_their_stack() {
    let dir = tempfile::tempdir().unwrap();
    let process = go(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let work = service(&process, &ctx, "p", "probe", json!(null), "work").await;
    let failure = |method: &str| match work.invoke(method, json!([]).into()) {
        Err(Error::Remote {
            name,
            message,
            graph,
        }) => (name, message, graph),
        other => panic!("{method} should throw, got {other:?}"),
    };
    let (name, message, _) = failure("fail");
    assert_eq!(
        (name.as_str(), message.as_str()),
        ("Quota", "quota exhausted")
    );
    let (name, message, _) = failure("named");
    assert_eq!(
        (name.as_str(), message.as_str()),
        ("NotFound", "no such city")
    );
    let (name, message, graph) = failure("boom");
    assert_eq!(name, "Panic");
    assert!(message.contains("boom"), "{message}");
    let stack = graph.unwrap()["nodes"][0]["stack"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(stack.contains("probe.(*Work).Boom"), "{stack}");
    // The session goes on.
    assert_eq!(
        work.invoke("ping", json!([]).into())
            .unwrap()
            .json()
            .unwrap(),
        json!("pong")
    );
    drop(work);
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn data_is_copied_and_objects_cross_by_reference() {
    let dir = tempfile::tempdir().unwrap();
    let process = go(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let work = service(&process, &ctx, "p", "probe", json!(null), "work").await;
    assert_eq!(
        work.invoke("record", json!([]).into())
            .unwrap()
            .json()
            .unwrap(),
        json!({ "x": 1, "y": 2 })
    );
    assert_eq!(
        work.invoke("sum", json!([[1, 2, 3]]).into())
            .unwrap()
            .json()
            .unwrap(),
        json!(6)
    );
    // The same counter each time, its state kept.
    let counter = || {
        work.invoke("counter", json!([]).into())
            .unwrap()
            .reference()
            .unwrap()
    };
    let add = |n: i64| {
        counter()
            .call_method("add", RpcValue::List(vec![json!(n).into()]))
            .unwrap()
            .json()
            .unwrap()
    };
    assert_eq!(add(2), json!(2));
    assert_eq!(add(3), json!(5), "the object keeps its state");
    let counter = counter();
    assert_eq!(counter.get("total").unwrap().json().unwrap(), json!(5));
    drop((counter, work));
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}
