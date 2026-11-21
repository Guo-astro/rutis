//! A local runtime's process ending while the host runs: the call in
//! progress fails with how it ended, its rows wait, and the service taken
//! before the end belongs to the old process, never answered by a restart.
//! A runtime the host does not restart stays down past the first backoff;
//! one removed by a reload during its backoff is not started again.
//!
//! Unix only: the runtime is killed with kill(2).
#![cfg(unix)]
// The tests hold the environment until they end, across their awaits: the
// variables they set are for the hosts they start.
#![allow(clippy::await_holding_lock)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::common::{environment, repo};
use rutis::host::FiberState;
use rutis_bridge::runtime::{RuntimeHandle, RuntimeState};
use rutis_bridge::session::{settle, HostDispatch, Value as RpcValue};
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_loader::report::EntryStatus;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::json;

/// Twice HANG_GUARD: the Python runtime starts cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// Provides `store`: `hold` writes a line and waits for the gate file,
/// `ping` answers `pong`; every process writes its start, with its id.
const STORE: &str = r#"
import asyncio
import os

provides = {"store": {"hold": "async", "ping": "sync"}}


class Store:
    def __init__(self, log, gate):
        self.log = log
        self.gate = gate

    async def hold(self):
        with open(self.log, "a") as out:
            out.write("holding\n")
        while not os.path.exists(self.gate):
            await asyncio.sleep(0.01)
        return "released"

    def ping(self):
        return "pong"


def apply(ctx, config):
    with open(config["log"], "a") as out:
        out.write(f"started {os.getpid()}\n")
    ctx.provide("store", Store(config["log"], config["gate"]))
"#;

/// A configuration of the row `store` on a Python runtime in `dir`, its
/// `restart` as given.
fn config(dir: &Path, restart: &str) -> HostConfig {
    let mut py = json!({ "project": dir, "restart": restart });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    serde_json::from_value(json!({
        "id": "runtime-exit",
        "runtimes": { "py": py },
        "rows": [{
            "id": "store",
            "name": "py:store",
            "config": {
                "log": dir.join("store.log"),
                "gate": dir.join("gate"),
            },
        }],
    }))
    .unwrap()
}

/// A host running the row `store`, its runtime ready, and `restart` set.
async fn started(restart: &str) -> (Host, tempfile::TempDir) {
    std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("store.py"), STORE).unwrap();
    let config = config(dir.path(), restart);
    let host = Host::start(&config).await.unwrap();
    COLD.within("the Python runtime", host.runtimes_ready())
        .await
        .unwrap();
    host.load(config.rows()).await.unwrap();
    (host, dir)
}

/// The runtime `py`'s handle.
fn handle(host: &Host) -> &RuntimeHandle {
    &host
        .runtimes
        .iter()
        .find(|(name, _)| name == "py")
        .expect("the Python runtime")
        .1
}

/// The row `store`'s fiber state, when it runs.
fn row_state(host: &Host, id: &str) -> Option<FiberState> {
    match host.loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

/// The service `store`, once its row has provided it.
async fn service(host: &Host) -> Arc<dyn HostDispatch> {
    COLD.eventually(|| host.service("store"), "the store service")
        .await
}

/// The lines the plugin wrote.
fn lines(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("store.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The number of processes that ran the plugin.
fn starts(dir: &Path) -> usize {
    lines(dir)
        .iter()
        .filter(|line| line.starts_with("started "))
        .count()
}

/// Wait until the plugin wrote `line`.
async fn written(dir: &Path, line: &str) {
    COLD.eventually(
        || {
            lines(dir)
                .iter()
                .any(|written| written == line)
                .then_some(())
        },
        &format!("{line:?} in {:?}", lines(dir)),
    )
    .await;
}

/// A call to `method` started now; `hold` waits for its gate.
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

/// SIGTERM to the runtime's process.
fn kill(handle: &RuntimeHandle) -> i32 {
    let pid = handle.pid().expect("the runtime's process") as i32;
    // SAFETY: plain kill(2) on the runtime process this host started.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0, "kill");
    pid
}

/// The runtime's reason once it is down.
async fn down(handle: &RuntimeHandle) -> String {
    match COLD
        .within(
            "the runtime going down",
            handle.wait_for(|state| matches!(state, RuntimeState::Down(_))),
        )
        .await
    {
        RuntimeState::Down(reason) => reason,
        other => panic!("the runtime is not down: {other:?}"),
    }
}

/// A call in progress when the runtime's process ends fails with
/// `peer-gone` and how it ended; `on-failure` starts the runtime again,
/// and the service taken before the end is not answered by the new
/// process.
/// guarantee: host/runtime-restart, loader/runtime-down-stops-its-rows,
/// bridge/in-flight-call-fails-once
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_does_not_answer_with_the_old_process() {
    let _environment = environment();
    let (host, dir) = started("on-failure").await;
    let _exits = host.supervise();
    let old = service(&host).await;
    let held = call(&old, "hold");
    written(dir.path(), "holding").await;

    let killed = kill(handle(&host));
    let reason = down(handle(&host)).await;
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
        || row_state(&host, "store") == Some(FiberState::Pending),
        "the row to wait for the runtime",
    )
    .await;

    // `on-failure` starts it again after the first wait (1 s), and the row
    // comes back on the new process.
    COLD.within(
        "the runtime to run again",
        handle(&host).wait_for(|state| matches!(state, RuntimeState::Ready(_))),
    )
    .await;
    let again = handle(&host).pid().expect("the new process");
    assert_ne!(again, killed as u32, "the old process came back");
    COLD.until(
        || row_state(&host, "store") == Some(FiberState::Active),
        "the row again",
    )
    .await;
    assert_eq!(starts(dir.path()), 2, "{:?}", lines(dir.path()));

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
    let now = service(&host).await;
    assert_eq!(
        settle(now.invoke("ping", RpcValue::List(Vec::new())).unwrap())
            .await
            .unwrap()
            .json()
            .unwrap(),
        json!("pong")
    );
    host.shut_down(HANG_GUARD).await.ok();
}

/// `restart: "no"` (the default): the runtime stays down past the first
/// backoff, its row waits, and no new process runs.
/// guarantee: host/runtime-restart
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_the_host_may_not_restart_stays_down() {
    let _environment = environment();
    let (mut host, dir) = started("no").await;
    let old = service(&host).await;
    let held = call(&old, "hold");
    written(dir.path(), "holding").await;

    kill(handle(&host));
    let reason = down(handle(&host)).await;
    assert!(reason.contains("signal: 15"), "{reason}");
    let _ = COLD
        .within("the call in progress to fail", held)
        .await
        .unwrap()
        .expect_err("the call cannot succeed");

    // Longer than the first wait of `on-failure` (1 s): no restart at all.
    // clock: weak-negative — the runtime is not started again
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(matches!(handle(&host).state(), RuntimeState::Down(_)));
    assert_eq!(starts(dir.path()), 1, "{:?}", lines(dir.path()));

    // Removed: the down runtime stops at once, and no process is left.
    let empty: HostConfig = serde_json::from_value(json!({ "id": "runtime-exit" })).unwrap();
    let reloaded = host.reload(empty).await.unwrap();
    assert_eq!(reloaded.stopped, ["py"]);
    assert!(host.runtimes.is_empty());
    assert_eq!(starts(dir.path()), 1, "{:?}", lines(dir.path()));
    host.shut_down(HANG_GUARD).await.ok();
}

/// A reload that removes the runtime while its call is in progress does
/// not wait for that call: it fails with `peer-gone`, and the service
/// taken before the reload stays dead.
/// guarantee: host/reload-config, bridge/in-flight-call-fails-once
#[tokio::test(flavor = "multi_thread")]
async fn a_reload_ends_the_call_in_progress_and_the_old_service() {
    let _environment = environment();
    let (mut host, dir) = started("no").await;
    let old = service(&host).await;
    let held = call(&old, "hold");
    written(dir.path(), "holding").await;

    // The runtime is removed: its row unloads, its process ends, and the
    // call that never answers does not hold the reload up.
    let empty: HostConfig = serde_json::from_value(json!({ "id": "runtime-exit" })).unwrap();
    let reloaded = COLD.within("the reload", host.reload(empty)).await.unwrap();
    assert_eq!(reloaded.stopped, ["py"]);
    assert!(host.runtimes.is_empty());

    let error = COLD
        .within("the call of the row before the reload", held)
        .await
        .unwrap()
        .expect_err("the call cannot succeed");
    assert_eq!(error.code(), Some("peer-gone"), "{error}");
    assert!(
        error.to_string().contains("runtime py") && error.to_string().contains("during the call"),
        "{error}"
    );
    // The old service is not there any more.
    let old = match old.invoke("ping", RpcValue::List(Vec::new())) {
        Err(error) => error,
        Ok(reply) => settle(reply)
            .await
            .expect_err("the old service is not answered by anything"),
    };
    assert!(old.code().is_some(), "{old}");
    host.shut_down(HANG_GUARD).await.ok();
}

/// A reload that removes the runtime while its supervisor waits to start
/// it again stops it for good: once the wait is over, no new process
/// starts.
/// guarantee: host/reload-config, host/runtime-restart
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_removed_during_the_backoff_is_not_started_again() {
    let _environment = environment();
    let (mut host, dir) = started("on-failure").await;
    let _exits = host.supervise();
    let old = service(&host).await;
    let held = call(&old, "hold");
    written(dir.path(), "holding").await;

    kill(handle(&host));
    down(handle(&host)).await;
    let _ = COLD
        .within("the call in progress to fail", held)
        .await
        .unwrap()
        .expect_err("the call cannot succeed");
    // The first wait is 1 s: this reload happens inside it.
    let empty: HostConfig = serde_json::from_value(json!({ "id": "runtime-exit" })).unwrap();
    let reloaded = host.reload(empty).await.unwrap();
    assert_eq!(reloaded.stopped, ["py"]);
    assert!(host.runtimes.is_empty());

    // Past the wait the supervisor had before it: nothing started.
    // clock: weak-negative — the runtime is not started again
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(host.runtimes.is_empty());
    assert_eq!(starts(dir.path()), 1, "{:?}", lines(dir.path()));
    host.shut_down(HANG_GUARD).await.ok();
}
