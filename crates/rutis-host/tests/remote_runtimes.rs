//! A remote runtime reached by a connection row
//! (`docs/design-remote-session-2026-10-28.md` §六, §七), end to end: a
//! `rutis-host/dial` row dials a runtime that listens, the rows
//! `<row>:<plugin>` run on it, and the row's connection has its lines and
//! its part of `rutis-host status`.
//!
//! The runtimes are real ones listening on loopback: Python, Bun and Go.

// The tests hold the environment until they end, across their awaits: the
// variables they set are for the hosts they start.
#![allow(clippy::await_holding_lock)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::{environment, repo, write};
use rutis_bridge::session::settle;
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt;

/// Twice HANG_GUARD: the Python runtimes start cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// The remote runtime's plugin.
const CONTACTS: &str = r#"
class Contacts:
    def owner(self):
        return "gpu"


provides = {"contacts": Contacts}


def apply(ctx, config):
    ctx.provide("contacts", Contacts())
"#;

fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// A Python runtime `id` listening on loopback with the plugins in
/// `project`, for the host `test`, accepting `token`; the address it
/// listens on.
async fn listening_python(
    project: &Path,
    id: &str,
    token: &str,
) -> (tokio::process::Child, String) {
    let mut child = tokio::process::Command::new(python())
        .args(["-m", "rutis", "listen:ws://127.0.0.1:0/rutis"])
        .args(["--id", id, "--peer", "test"])
        .arg(project)
        .env("PYTHONPATH", repo().join("python/rutis"))
        .env("RUTIS_TOKEN", token)
        .current_dir(project)
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = COLD
        .within("the runtime's address", async {
            loop {
                let line = lines.next_line().await.unwrap().expect("its address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("runtime: {line}");
        }
    });
    (child, address)
}

/// `contacts.owner()` on the host's root, once the row provides it.
async fn owner(host: &Host) -> Value {
    let contacts = COLD
        .eventually(|| host.service("contacts"), "the remote row's service")
        .await;
    let owner = contacts.invoke("owner", json!([]).into()).unwrap();
    settle(owner).await.unwrap().json().unwrap()
}

/// A dial row `gpu` to a runtime that listens: the row `gpu:contacts_plugin`
/// runs there, `status` says the row is connected; when the runtime goes,
/// the row's service goes with it and `status` says disconnected.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_dial_row_runs_the_rows_of_a_runtime_that_listens() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("remote/contacts_plugin.py", CONTACTS)]);
    let (mut runtime, address) =
        listening_python(&dir.path().join("remote"), "gpu", "gpu-token").await;
    std::env::set_var("RUTIS_TOKEN_GPU", "gpu-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "rows": [
            { "id": "gpu", "name": "rutis-host/dial", "config": { "address": address } },
            { "id": "contacts", "name": "gpu:contacts_plugin" }
        ]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    assert_eq!(owner(&host).await, json!("gpu"));
    let status = (host.status_report())();
    assert!(
        status.ends_with("connections:\n  gpu: connected\n"),
        "{status}"
    );

    runtime.kill().await.unwrap();
    COLD.eventually(
        || host.service("contacts").is_none().then_some(()),
        "the withdrawal",
    )
    .await;
    COLD.until(
        || (host.status_report())().ends_with("connections:\n  gpu: disconnected\n"),
        "the row disconnected",
    )
    .await;
    host.shut_down(HANG_GUARD).await.ok();
}

/// What `run` prints of a dial row: `gpu: connected` once the far end
/// greeted, `gpu: disconnected (<why>); retrying in <s>s` when it went.
/// guarantee: host/remote-runtimes
#[cfg(unix)]
#[test]
fn run_says_when_a_dial_row_connects_and_disconnects() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("remote/contacts_plugin.py", CONTACTS)]);
    let tasks = tokio::runtime::Runtime::new().unwrap();
    let (mut runtime, address) = tasks.block_on(listening_python(
        &dir.path().join("remote"),
        "gpu",
        "gpu-token",
    ));
    let config = json!({
        "id": "test",
        "rows": [{ "id": "gpu", "name": "rutis-host/dial", "config": { "address": address } }]
    });
    std::fs::write(dir.path().join("rutis.json"), config.to_string()).unwrap();
    let mut host = common::process::Host::start_in_env(
        &["run", "rutis.json"],
        dir.path(),
        &[("RUTIS_TOKEN_GPU", "gpu-token")],
    );
    host.wait_line(|line| (line == "gpu: connected").then_some(()));
    tasks.block_on(runtime.kill()).unwrap();
    let line = host.wait_line(|line| {
        line.starts_with("gpu: disconnected (")
            .then(|| line.to_owned())
    });
    assert!(line.contains("); retrying in "), "{line}");
    common::process::signal(&host, libc::SIGTERM, false);
    let (status, _, _) = host.exit();
    assert!(status.success(), "{status:?}");
}

/// A Bun runtime on another machine (here, listening on loopback): a
/// dial row connects it, and its rows are `<row>:<module>`. Bun on
/// Windows is not tested yet.
/// guarantee: loader/remote-runtime-rows
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_dial_row_runs_bun_rows_named_after_it() {
    let _environment = environment();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("package.json"), "{}").unwrap();
    std::fs::write(
        project.path().join("greeter.ts"),
        "export const provides = { greeter: { hello: 'sync' } }\n\
         export function apply(ctx) { ctx.provide('greeter', { hello: (name: string) => 'Bun, ' + name }) }\n",
    )
    .unwrap();
    let mut child = tokio::process::Command::new("bun")
        .args(["--no-install", "--no-env-file"])
        .arg(repo().join("bun/rutis-bun/src/main.ts"))
        .args([
            "listen:ws://127.0.0.1:0/rutis",
            "--id",
            "edge",
            "--peer",
            "remote-bun",
        ])
        .arg(project.path())
        .current_dir(project.path())
        .env("RUTIS_TOKEN", "edge-token")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = COLD
        .within("the runtime's address", async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

    std::env::set_var("RUTIS_TOKEN_EDGE", "edge-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "remote-bun",
        "rows": [
            { "id": "edge", "name": "rutis-host/dial", "config": { "address": address } },
            { "id": "greeter", "name": "edge:./greeter.ts" }
        ]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    let greeter = COLD
        .eventually(|| host.service("greeter"), "the greeter")
        .await;
    let reply = greeter.invoke("hello", json!(["Ada"]).into()).unwrap();
    assert_eq!(
        settle(reply).await.unwrap().json().unwrap(),
        json!("Bun, Ada")
    );
    child.start_kill().unwrap();
}

/// A Go binary on another machine (here, listening on loopback): a dial
/// row connects it, and its rows are `<row>:<plugin>`.
/// guarantee: loader/remote-runtime-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_dial_row_runs_go_rows_named_after_it() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join(if cfg!(windows) {
        "netkit.exe"
    } else {
        "netkit"
    });
    let status = std::process::Command::new("go")
        .args(["build", "-o"])
        .arg(&binary)
        .arg("./cmd/netkit")
        .current_dir(repo().join("crates/rutis-loader/tests/fixtures/go"))
        .status()
        .expect("go is on PATH");
    assert!(status.success());
    let mut child = tokio::process::Command::new(&binary)
        .args([
            "listen:ws://127.0.0.1:0/rutis",
            "--id",
            "edge",
            "--peer",
            "remote-go",
        ])
        .arg(dir.path())
        .env("RUTIS_TOKEN", "edge-token")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = COLD
        .within("the runtime's address", async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    std::env::set_var("RUTIS_TOKEN_EDGE", "edge-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "remote-go",
        "rows": [
            { "id": "edge", "name": "rutis-host/dial", "config": { "address": address } },
            { "id": "ping", "name": "edge:ping" }
        ]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    let service = COLD
        .eventually(|| host.service("ping"), "the ping service")
        .await;
    let reply = tokio::task::spawn_blocking(move || service.invoke("echo", json!(["far"]).into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.json().unwrap(), json!("netkit pong far"));
    child.start_kill().unwrap();
}
