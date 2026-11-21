//! A local runtime's process ending: by its own exit code, or killed by a
//! signal. Its rows stop and wait, a call in progress fails with how the
//! process ended, and a restart brings the rows back on a new process. The
//! service handle taken before the end belongs to the old process: after
//! the restart it still fails, it is not answered by the new one.
#![cfg(feature = "runtimes")]

use std::sync::Arc;

use crate::common::runtimes::{
    greet_as_main, layer, row_state, rows_stage, Probe, TestRuntime, COLD,
};

/// `ExitStatus` reads this way on each family: Windows says "exit code",
/// Unix "exit status". The reason travels through the error text unchanged.
#[cfg(windows)]
const EXIT_STATUS: &str = "exit code";
#[cfg(not(windows))]
const EXIT_STATUS: &str = "exit status";
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::RuntimeHandle;
use rutis_bridge::session::{host_key, settle, HostDispatch, Value as RpcValue};
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin};
use serde_json::{json, Value};

/// Provides `work`: `hold` records that it is running and never answers,
/// `ping` answers `pong`. With `exit`, a thread ends the process by
/// `os._exit(exit)` once the test writes the file `crash`.
const WORKER: &str = r#"
import asyncio
import os
import threading
import time

inject = ["probe"]
provides = {"work": {"hold": "async", "ping": "sync"}}


class Work:
    def __init__(self, probe):
        self.probe = probe

    async def hold(self):
        self.probe.record("worker: holding")
        await asyncio.Event().wait()

    def ping(self):
        return "pong"


def apply(ctx, config):
    probe = ctx.use("probe")
    ctx.provide("work", Work(probe))
    probe.record("worker: started")
    exit_code = config.get("exit")
    if exit_code:
        crash = config["crash"]

        def wait_and_exit():
            while not os.path.exists(crash):
                time.sleep(0.01)
            os._exit(exit_code)

        threading.Thread(target=wait_and_exit, daemon=True).start()
"#;

const BYSTANDER: &str = r#"
inject = ["probe"]


def apply(ctx, config):
    ctx.use("probe").record("bystander: started")
"#;

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    runtime: rutis::host::FiberView,
    handle: RuntimeHandle,
    dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("worker.py"), WORKER).unwrap();
    std::fs::write(dir.path().join("bystander.py"), BYSTANDER).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    greet_as_main(&root);
    let python = TestRuntime::python(dir.path());
    let handle = python.handle();
    let options = LoaderOptions::default();
    let sessions = Arc::new(SessionResolver::new(&options.catalog));
    sessions.runtime(handle.clone());
    let runtime = root.plugin(python);
    (&runtime).await.unwrap();
    let plugin = LoaderPlugin::new(Chain::new().with_shared(sessions.clone()), options);
    let loader = plugin.handle();
    sessions.attach(&loader);
    root.plugin(plugin).await.unwrap();
    rows_stage(&root, &sessions, "py").await;
    Fixture {
        root,
        loader,
        probe,
        runtime,
        handle,
        dir,
    }
}

fn worker(fixture: &Fixture, exit: Option<i32>) -> Value {
    json!({
        "id": "w",
        "name": "py:worker",
        "config": { "exit": exit, "crash": fixture.dir.path().join("crash") },
    })
}

fn bystander() -> Value {
    json!({ "id": "b", "name": "py:bystander" })
}

/// The service `work`, once the row has provided it.
async fn work(root: &Ctx) -> Arc<dyn HostDispatch> {
    COLD.eventually(
        || root.get_at::<dyn HostDispatch>(&host_key("work")),
        "the worker's service",
    )
    .await
}

/// A call to `method` started now; `hold` never answers.
fn call(
    service: &Arc<dyn HostDispatch>,
    method: &str,
) -> tokio::task::JoinHandle<Result<RpcValue, rutis_bridge::session::Error>> {
    let service = service.clone();
    let method = method.to_owned();
    tokio::spawn(async move {
        let reply = service.invoke(&method, RpcValue::List(Vec::new()))?;
        settle(reply).await
    })
}

/// Whether every row of `ids` is in `state`.
fn all_rows(loader: &Loader, ids: &[&str], state: FiberState) -> bool {
    ids.iter().all(|id| row_state(loader, id) == Some(state))
}

/// The runtime's state, `down` with its reason.
fn down(handle: &RuntimeHandle) -> Option<String> {
    match handle.state() {
        rutis_bridge::runtime::RuntimeState::Down(reason) => Some(reason),
        _ => None,
    }
}

/// The rows and entries as the failure message shows them.
fn situation(fixture: &Fixture) -> String {
    let rows: Vec<String> = ["w", "b"]
        .iter()
        .map(|id| match fixture.loader.get(id) {
            Some(entry) => format!("{id}: {:?}", entry.status),
            None => format!("{id}: none"),
        })
        .collect();
    format!(
        "runtime: {:?}; rows: {rows:?}; probe: {:?}",
        fixture.handle.state(),
        fixture.probe.lines()
    )
}

/// A process that ends by its own exit code: its rows wait, the call in
/// progress fails with `peer-gone` and the exit code, no row fails, and a
/// restart runs the rows on a new process. A service handle taken before
/// the end still fails after the restart, and the new one answers.
/// guarantee: loader/runtime-down-stops-its-rows, bridge/in-flight-call-fails-once
#[tokio::test(flavor = "multi_thread")]
async fn an_exit_code_ends_the_rows_and_the_call_in_progress() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    let fixture = fixture().await;
    let crash = fixture.dir.path().join("crash");
    fixture
        .loader
        .reconcile(layer(vec![worker(&fixture, Some(7)), bystander()]), None)
        .await
        .unwrap();
    fixture.probe.wait_for("worker: started").await;
    fixture.probe.wait_for("bystander: started").await;

    let old = work(&fixture.root).await;
    let held = call(&old, "hold");
    fixture.probe.wait_for("worker: holding").await;
    std::fs::write(&crash, b"").unwrap();

    let reason = COLD
        .eventually_with(
            || down(&fixture.handle),
            "the runtime to be down",
            || situation(&fixture),
        )
        .await;
    assert_eq!(
        reason,
        format!("runtime ended: the process exited with {EXIT_STATUS}: 7")
    );

    let error = COLD
        .within("the call in progress to fail", held)
        .await
        .unwrap()
        .expect_err("the call cannot succeed");
    assert_eq!(error.code(), Some("peer-gone"), "{error}");
    assert_eq!(
        error.to_string(),
        "runtime py exited during the call",
        "{error}"
    );
    let data = error
        .data()
        .map(|data| data.to_string())
        .unwrap_or_default();
    assert!(data.contains(&format!("{EXIT_STATUS}: 7")), "{data}");

    // Its rows wait for the runtime; none failed.
    COLD.until(
        || all_rows(&fixture.loader, &["w", "b"], FiberState::Pending),
        "every row to wait for the runtime",
    )
    .await;
    assert!(
        fixture
            .loader
            .entries()
            .iter()
            .all(|entry| row_state(&fixture.loader, &entry.id) != Some(FiberState::Failed)),
        "{}",
        situation(&fixture)
    );

    // The trigger is gone: the restarted process runs.
    std::fs::remove_file(&crash).unwrap();
    fixture.runtime.restart().await.unwrap();
    COLD.eventually_with(
        || all_rows(&fixture.loader, &["w", "b"], FiberState::Active).then_some(()),
        "the rows again",
        || situation(&fixture),
    )
    .await;
    // The old process's service is not the new one's: its call fails.
    let error = match old.invoke("ping", RpcValue::List(Vec::new())) {
        Err(error) => error,
        Ok(reply) => settle(reply)
            .await
            .expect_err("the old service is not answered by the new process"),
    };
    assert_eq!(error.code(), Some("peer-gone"), "{error}");
    assert!(
        error
            .to_string()
            .contains("runtime py exited during the call"),
        "{error}"
    );
    // The one provided now answers.
    let new = work(&fixture.root).await;
    assert_eq!(
        settle(new.invoke("ping", RpcValue::List(Vec::new())).unwrap())
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("pong")
    );
    fixture.root.shutdown().await.unwrap();
}

/// A process killed by a signal: its rows wait, and the runtime's reason
/// names the signal (`SIGTERM`), not a broken pipe or a disconnect.
/// guarantee: loader/runtime-down-stops-its-rows, bridge/in-flight-call-fails-once
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_signal_ends_the_rows_and_names_the_signal() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    let fixture = fixture().await;
    fixture
        .loader
        .reconcile(layer(vec![worker(&fixture, None), bystander()]), None)
        .await
        .unwrap();
    fixture.probe.wait_for("worker: started").await;
    fixture.probe.wait_for("bystander: started").await;

    let held = call(&work(&fixture.root).await, "hold");
    fixture.probe.wait_for("worker: holding").await;
    let pid = fixture.handle.pid().expect("the runtime's process");
    // SAFETY: plain kill(2) on the runtime process this test started.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGTERM) }, 0, "kill");

    let reason = COLD
        .eventually_with(
            || down(&fixture.handle),
            "the runtime to be down",
            || situation(&fixture),
        )
        .await;
    assert_eq!(
        reason,
        "runtime ended: the process exited with signal: 15 (SIGTERM)"
    );

    let error = COLD
        .within("the call in progress to fail", held)
        .await
        .unwrap()
        .expect_err("the call cannot succeed");
    assert_eq!(error.code(), Some("peer-gone"), "{error}");
    assert_eq!(
        error.to_string(),
        "runtime py exited during the call",
        "{error}"
    );
    let data = error
        .data()
        .map(|data| data.to_string())
        .unwrap_or_default();
    assert!(data.contains("signal: 15"), "{data}");

    COLD.until(
        || all_rows(&fixture.loader, &["w", "b"], FiberState::Pending),
        "every row to wait for the runtime",
    )
    .await;

    // A restart runs them again on a new process.
    fixture.runtime.restart().await.unwrap();
    COLD.eventually_with(
        || all_rows(&fixture.loader, &["w", "b"], FiberState::Active).then_some(()),
        "the rows again",
        || situation(&fixture),
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}
