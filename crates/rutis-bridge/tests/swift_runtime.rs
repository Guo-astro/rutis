//! The Swift runtime speaks the same protocol and row contract as the
//! others, and carries Swift's values across (docs/design-swift-sdk-2026-10-12.md
//! §14, "Swift 专项"): describe, load, lease hosts; cancellation reaching
//! the method's Task; errors by type name with their code and data, thrown
//! on with their graph; objects by reference; and a host older than rutis
//! 0.13 refused right after its handshake.
#![cfg(all(feature = "swift", target_os = "macos"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::{row_projection, Launcher, Mount, Process};
use rutis_bridge::session::{host_key, settle, Error, HostDispatch, Value as RpcValue};
use rutis_test_support::{until, within, Guard, HANG_GUARD};
use serde_json::{json, Value};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A product of the Swift package (the root `Package.swift`), built once.
fn binary(product: &str) -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    let bin = BIN.get_or_init(|| {
        let status = std::process::Command::new("swift")
            .arg("build")
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    });
    bin.join(product)
}

async fn swift(project: &Path, product: &str) -> Arc<Process> {
    let launcher = Launcher::swift(&binary(product), project);
    let repo = repo();
    // A cold start: twice the hang guard.
    let mount = Process::mount(
        &repo,
        Mount {
            anchor: Some(project),
            launcher: Some(&launcher),
            ..Mount::default()
        },
    );
    Guard(HANG_GUARD * 2)
        .within("the Swift runtime's start", mount)
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

fn json_of(service: &Arc<dyn HostDispatch>, method: &str, args: Value) -> Value {
    service.invoke(method, args.into()).unwrap().json().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn swift_rows_follow_the_row_contract() {
    let dir = tempfile::tempdir().unwrap();
    let process = swift(dir.path(), "RutisMultilang").await;
    for feature in ["rows.v2", "hosts", "leaf", "scopes", "rows.announce"] {
        assert!(process.supports(feature), "{feature}");
    }
    let described = process.describe_row(Path::new("probe")).await.unwrap();
    assert_eq!(described.inject, Vec::<String>::new());
    assert_eq!(
        described.provides["work"]["isStarted"],
        json!("sync"),
        "{:?}",
        described.provides
    );
    // A plugin the binary does not have: the error lists those it has.
    match process.describe_row(Path::new("nowhere")).await {
        Err(Error::Remote { name, message, .. }) => {
            assert_eq!(name, "NotFound");
            assert!(message.contains("caller, probe"), "{message}");
        }
        other => panic!("describing a missing plugin should fail, got {other:?}"),
    }
    within("the Swift runtime's exit", process.dispose())
        .await
        .unwrap();
}

/// guarantee: bridge/cancellation
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_async_call_cancels_the_methods_task() {
    let dir = tempfile::tempdir().unwrap();
    let process = swift(dir.path(), "RutisMultilang").await;
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
                move || probe.invoke("isStarted", json!([]).into()).ok().and_then(|value| value.json().ok()) == Some(json!(true)),
                "the method started",
            ) => {}
        }
    }
    let probe = work.clone();
    until(
        move || {
            probe
                .invoke("isAborted", json!([]).into())
                .ok()
                .and_then(|value| value.json().ok())
                == Some(json!(true))
        },
        "the cancellation",
    )
    .await;
    assert_eq!(
        json_of(&work, "ping", json!([])),
        json!("pong"),
        "the session goes on"
    );
    drop(work);
    within("the Swift runtime's exit", process.dispose())
        .await
        .unwrap();
    within("the shutdown", ctx.shutdown()).await.unwrap();
}

/// guarantee: bridge/error-code-and-data
#[tokio::test(flavor = "multi_thread")]
async fn errors_cross_with_their_names_codes_and_data_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let process = swift(dir.path(), "RutisMultilang").await;
    let ctx = Ctx::root().unwrap();
    let work = service(&process, &ctx, "p", "probe", "work").await;
    match work.invoke("fail", json!([]).into()) {
        Err(error @ Error::Remote { .. }) => {
            let Error::Remote { name, message, .. } = &error else {
                unreachable!()
            };
            assert_eq!(
                (name.as_str(), message.as_str()),
                ("Quota", "quota exhausted")
            );
            assert_eq!(error.code(), Some("QuotaExceeded"));
            assert_eq!(error.data(), Some(json!({ "left": 0 })));
        }
        other => panic!("fail should throw, got {other:?}"),
    }
    // An error of this side's callback, thrown on by Swift: its name, code
    // and data come back.
    let callback = RpcValue::callback(|_| {
        Err(Error::Remote {
            name: "Teapot".into(),
            message: "short and stout".into(),
            graph: Some(json!({
                "root": { "type": "reference", "value": 0 },
                "nodes": [{ "type": "error", "name": "Teapot", "message": "short and stout", "code": "E418",
                            "data": { "type": "data", "value": 418 } }],
            })),
        })
    });
    match work.invoke("rethrow", RpcValue::List(vec![callback])) {
        Err(error @ Error::Remote { .. }) => {
            let Error::Remote { name, message, .. } = &error else {
                unreachable!()
            };
            assert_eq!(
                (name.as_str(), message.as_str()),
                ("Teapot", "short and stout")
            );
            assert_eq!(error.code(), Some("E418"));
            assert_eq!(error.data(), Some(json!(418)));
        }
        other => panic!("rethrow should throw, got {other:?}"),
    }
    assert_eq!(json_of(&work, "ping", json!([])), json!("pong"));
    drop(work);
    within("the Swift runtime's exit", process.dispose())
        .await
        .unwrap();
    within("the shutdown", ctx.shutdown()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn data_is_copied_and_service_objects_cross_by_reference() {
    let dir = tempfile::tempdir().unwrap();
    let process = swift(dir.path(), "RutisMultilang").await;
    let ctx = Ctx::root().unwrap();
    let work = service(&process, &ctx, "p", "probe", "work").await;
    assert_eq!(
        json_of(&work, "record", json!([])),
        json!({ "x": 1, "y": 2 })
    );
    assert_eq!(json_of(&work, "sum", json!([[1, 2, 3]])), json!(6));
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
    within("the Swift runtime's exit", process.dispose())
        .await
        .unwrap();
    within("the shutdown", ctx.shutdown()).await.unwrap();
}
