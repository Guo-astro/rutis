//! `Loader::status()` with a Python runtime: the rows, the services, the
//! runtime with its process id, a call in progress; a runtime that exits
//! makes its rows wait for it.
#![cfg(feature = "python")]

mod common;

use std::path::Path;
use std::sync::Arc;

use common::runtimes::{layer, rows_stage, Probe, COLD};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::Value as RpcValue;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_loader::runtime::RuntimeResolver;
use rutis_loader::status::{Call, Row, RowState, RuntimeState, Service};
use rutis_loader::{Chain, LoaderOptions, LoaderPlugin};
use serde_json::json;

/// Provides `slow`, whose `wait` takes as long as the call is not
/// cancelled.
const SLOW: &str = r#"
import asyncio

provides = {"slow": {"wait": "async"}}


class Slow:
    async def wait(self):
        await asyncio.sleep(600)


def apply(ctx, config):
    ctx.provide("slow", Slow())
"#;

/// Waits for `missing`, which no row provides.
const NEEDY: &str = "inject = [\"missing\"]\n\ndef apply(ctx, config):\n    pass\n";

/// The status lists each row with its state and why, each service with
/// the row providing it, the runtime running with its process id, and a
/// call from the host to a row's service that has not returned, gone once
/// the caller gives it up. Once the runtime's process is killed, its rows wait for it, and the runtime is
/// down without a process id.
/// guarantee: loader/status
#[tokio::test(flavor = "multi_thread")]
async fn the_status_names_rows_services_runtimes_and_calls() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("slow.py"), SLOW).unwrap();
    std::fs::write(dir.path().join("needy.py"), NEEDY).unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe)).unwrap();
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/rutis");
    let python = LocalRuntime::python(dir.path()).python_path(sdk);
    let rows = Arc::new(RuntimeResolver::modules(python.handle()));
    root.plugin(python).await.unwrap();
    let plugin_loader = LoaderPlugin::new(
        Chain::new().with_shared(rows.clone()),
        LoaderOptions::default(),
    );
    let loader = plugin_loader.handle();
    root.plugin(plugin_loader).await.unwrap();
    rows_stage(&root, rows).await;
    loader
        .reconcile(
            layer(vec![
                json!({ "id": "slow", "name": "py:slow" }),
                json!({ "id": "needy", "name": "py:needy" }),
            ]),
            None,
        )
        .await
        .unwrap();
    let slow = COLD
        .eventually(
            || root.get_at::<dyn HostDispatch>(&host_key("slow")),
            "the slow service",
        )
        .await;
    let call = tokio::spawn(async move {
        rutis_bridge::session::settle(slow.invoke("wait", RpcValue::List(Vec::new()))?).await
    });
    let status = COLD
        .eventually(
            || {
                let status = loader.status();
                (status.calls.len() == 1
                    && status
                        .rows
                        .iter()
                        .all(|row| row.state != RowState::Starting))
                .then_some(status)
            },
            "the call in the status",
        )
        .await;

    assert_eq!(
        status.rows,
        [
            Row::new("slow", None, RowState::Running, None),
            Row::new(
                "needy",
                None,
                RowState::Pending,
                Some("waiting for missing (no row provides it)")
            ),
        ]
    );
    assert!(status
        .services
        .contains(&Service::new("slow", Some("slow"))));
    assert!(status.services.iter().all(|service| service.name != "py"));
    assert_eq!(status.runtimes.len(), 1);
    assert_eq!(status.runtimes[0].name, "py");
    assert_eq!(status.runtimes[0].state, RuntimeState::Running);
    let pid = status.runtimes[0].pid.expect("the runtime's process id");
    let Call {
        from,
        to,
        service,
        method,
        ..
    } = &status.calls[0];
    assert_eq!(
        (
            from.as_deref(),
            to.as_deref(),
            service.as_str(),
            method.as_str()
        ),
        (None, Some("slow"), "slow", "wait")
    );
    let printed = status.to_string();
    assert!(printed.contains("  host → slow.wait  "), "{printed}");
    assert!(
        printed.contains(&format!("  py: running (pid {pid})")),
        "{printed}"
    );

    // A call given up is gone from the status, the runtime still running.
    call.abort();
    COLD.until(|| loader.status().calls.is_empty(), "the call gone")
        .await;
    assert_eq!(loader.status().runtimes[0].state, RuntimeState::Running);

    kill(pid);
    let status = COLD
        .eventually(
            || {
                let status = loader.status();
                (status.runtimes[0].state == RuntimeState::Down).then_some(status)
            },
            "the runtime down",
        )
        .await;
    assert_eq!(status.runtimes[0].pid, None);
    let slow_row = status.rows.iter().find(|row| row.id == "slow").unwrap();
    assert_eq!(slow_row.state, RowState::Pending);
    assert!(
        slow_row
            .reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("waiting for runtime py (down: ")),
        "{slow_row:?}"
    );
    root.shutdown().await.unwrap();
}

#[cfg(unix)]
fn kill(pid: u32) {
    // SAFETY: plain kill(2) on the runtime's process.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0, "kill");
}

#[cfg(windows)]
fn kill(pid: u32) {
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "taskkill");
}
