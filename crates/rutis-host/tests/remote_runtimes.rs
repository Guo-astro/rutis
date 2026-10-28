//! A remote runtime reached by a connection row
//! (`docs/design-remote-session-2026-10-28.md` §六, §七), end to end: a
//! `rutis-host/dial` row dials a runtime that listens, the rows
//! `<row>:<plugin>` run on it, and the row's connection has its lines and
//! its part of `rutis-host status`.
//!
//! The runtimes are real Python ones (`python -m rutis listen:…`).

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
