//! A remote runtime written whole in one `runtimes.remote` entry
//! (`docs/design-remote-runtime-consistency-2026-10-26.md` §四), each way it
//! connects, end to end: rutis dials it (`dial`), it dials in (`listen`), it
//! dials in many times, each connection an instance (`listen` +
//! `instances`), and the link row of before 0.19, which still works as it
//! did. Each way's rows run, and its connection has its lines and its part
//! of `rutis-host status`.
//!
//! The runtimes are real Python ones (`python -m rutis listen:…` or
//! `dial:…`).

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
use rutis_host::status::Shown;
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

/// A Python runtime `id` with the plugins in `project`, for the host
/// `test`, presenting or accepting `token`: `how` is `dial:<address>` or
/// `listen:<address>`. For one that listens, the address it listens on.
async fn remote_python(
    project: &Path,
    how: &str,
    id: &str,
    token: &str,
) -> (tokio::process::Child, Option<String>) {
    let mut child = tokio::process::Command::new(python())
        .args(["-m", "rutis", how])
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
    let address = match how.starts_with("listen:") {
        true => Some(
            COLD.within("the runtime's address", async {
                loop {
                    let line = lines.next_line().await.unwrap().expect("its address");
                    if let Some(address) = line.strip_prefix("rutis: listening on ") {
                        break address.to_owned();
                    }
                }
            })
            .await,
        ),
        false => None,
    };
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("runtime: {line}");
        }
    });
    (child, address)
}

/// What the host prints as it runs, gathered.
struct Printed {
    shown: Shown,
    lines: Vec<String>,
}

impl Printed {
    fn new() -> Self {
        Self {
            shown: Shown::default(),
            lines: Vec::new(),
        }
    }

    /// Wait until the host printed a line starting with `start`.
    async fn wait(&mut self, host: &Host, start: &str) -> String {
        let followed = host.followed();
        COLD.within(start, async {
            loop {
                self.lines.extend(self.shown.changes(&followed));
                if let Some(line) = self.lines.iter().find(|line| line.starts_with(start)) {
                    break line.clone();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
    }
}

/// `contacts.owner()` on the host's root, once the row provides it.
async fn owner(host: &Host) -> Value {
    let contacts = COLD
        .eventually(|| host.service("contacts"), "the remote row's service")
        .await;
    let owner = contacts.invoke("owner", json!([]).into()).unwrap();
    settle(owner).await.unwrap().json().unwrap()
}

/// A host `test` with one listener and the Python runtime `py`, configured
/// with `remote` and `rows`, started and its rows loaded.
async fn host(dir: &Path, remote: Value, rows: Value) -> (Host, HostConfig) {
    std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
    let mut config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
        "runtimes": { "remote": remote },
        "rows": rows
    }))
    .unwrap();
    config.rebase(dir);
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    (host, config)
}

/// `dial`: rutis dials the runtime, which greets as the runtime's name.
/// Its link's lines and `status` part say `runtime gpu`.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_rutis_dials() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("remote/contacts_plugin.py", CONTACTS)]);
    let (mut runtime, address) = remote_python(
        &dir.path().join("remote"),
        "listen:ws://127.0.0.1:0/rutis",
        "gpu",
        "gpu-token",
    )
    .await;
    std::env::set_var("RUTIS_TOKEN_GPU", "gpu-token");
    let (host, _) = host(
        dir.path(),
        json!([{ "name": "gpu", "language": "python", "dial": address.unwrap() }]),
        json!([{ "id": "contacts", "name": "gpu:contacts_plugin" }]),
    )
    .await;
    assert_eq!(owner(&host).await, json!("gpu"));
    let mut printed = Printed::new();
    printed.wait(&host, "runtime gpu: link connected").await;
    let status = (host.status_report())();
    assert!(
        status.ends_with("connections:\n  gpu: link connected\n"),
        "{status}"
    );

    // The runtime goes: its row stops, and the link says so and retries.
    runtime.kill().await.unwrap();
    let down = printed.wait(&host, "runtime gpu: link down (").await;
    assert!(down.contains("; retrying in "), "{down}");
    COLD.eventually(
        || host.service("contacts").is_none().then_some(()),
        "the withdrawal",
    )
    .await;
    host.shut_down(HANG_GUARD).await.ok();
}

/// `listen` alone: one runtime dials in, its `--id` the runtime's name, with
/// the token `RUTIS_TOKEN_<NAME>`.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_dials_in_alone() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("remote/contacts_plugin.py", CONTACTS)]);
    std::env::set_var("RUTIS_TOKEN_MAC", "mac-token");
    let (host, _) = host(
        dir.path(),
        json!([{ "name": "mac", "language": "python", "listen": "public" }]),
        json!([{ "id": "contacts", "name": "mac:contacts_plugin" }]),
    )
    .await;
    let status = (host.status_report())();
    assert!(
        status.ends_with("connections:\n  mac: waiting for it to connect\n"),
        "{status}"
    );
    let address = format!("dial:ws://{}/rutis", host.listening[0].1);
    let (mut runtime, _) =
        remote_python(&dir.path().join("remote"), &address, "mac", "mac-token").await;
    assert_eq!(owner(&host).await, json!("gpu"));
    let mut printed = Printed::new();
    printed.wait(&host, "runtime mac: link connected").await;

    runtime.kill().await.unwrap();
    printed
        .wait(&host, "runtime mac: link down (the connection ended)")
        .await;
    COLD.eventually(
        || host.service("contacts").is_none().then_some(()),
        "the withdrawal",
    )
    .await;
    host.shut_down(HANG_GUARD).await.ok();
}

/// `listen` + `instances`: each connection an instance of the group, its
/// coming and going on the host's lines.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn runtimes_that_dial_in_as_instances() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("remote/contacts_plugin.py", CONTACTS)]);
    std::env::set_var("RUTIS_TOKEN_WORKER", "shared");
    let (host, _) = host(
        dir.path(),
        json!([{ "name": "worker", "language": "python", "listen": "public", "instances": "worker" }]),
        json!([{ "id": "worker", "group": true, "instanced": true, "config": [
            { "id": "contacts", "name": "worker:contacts_plugin" }
        ] }]),
    )
    .await;
    let address = format!("dial:ws://{}/rutis", host.listening[0].1);
    let (mut runtime, _) =
        remote_python(&dir.path().join("remote"), &address, "worker-1", "shared").await;
    let mut printed = Printed::new();
    printed
        .wait(&host, "runtime worker: connected worker-1")
        .await;
    let status = (host.status_report())();
    assert!(
        status.ends_with("connections:\n  worker: worker-1\n"),
        "{status}"
    );

    runtime.kill().await.unwrap();
    printed
        .wait(&host, "runtime worker: disconnected worker-1 (")
        .await;
    let status = (host.status_report())();
    assert!(
        status.ends_with("connections:\n  worker: none connected\n"),
        "{status}"
    );

    host.shut_down(HANG_GUARD).await.ok();
}

/// The link row of before 0.19 (`rutis-bridge/peer` with `runtime`) works
/// as it did: its lines are its row's, and `status` has no connections.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_link_row_still_connects_a_runtime() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("remote/contacts_plugin.py", CONTACTS)]);
    let (_runtime, address) = remote_python(
        &dir.path().join("remote"),
        "listen:ws://127.0.0.1:0/rutis",
        "gpu",
        "gpu-token",
    )
    .await;
    std::env::set_var("RUTIS_TOKEN_GPU", "gpu-token");
    let (host, _) = host(
        dir.path(),
        json!([{ "name": "gpu", "language": "python" }]),
        json!([
            { "id": "gpu-link", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": address.unwrap(), "runtime": "gpu" } },
            { "id": "contacts", "name": "gpu:contacts_plugin" }
        ]),
    )
    .await;
    assert_eq!(owner(&host).await, json!("gpu"));
    let mut printed = Printed::new();
    printed.wait(&host, "gpu-link: link connected").await;
    let status = (host.status_report())();
    assert!(!status.contains("connections:"), "{status}");
    host.shut_down(HANG_GUARD).await.ok();
}

/// Each way's mistakes are refused when the host starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_runtime_connects_one_way() {
    let _environment = environment();
    std::env::set_var("RUTIS_TOKEN_GPU", "token");
    let start = |remote: Value, rows: Value| async move {
        let config: HostConfig = serde_json::from_value(json!({
            "id": "test",
            "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
            "runtimes": { "remote": [remote] },
            "rows": rows
        }))
        .unwrap();
        Host::start(&config).await.err().expect("refused")
    };
    let gpu = |fields: Value| {
        let mut remote = json!({ "name": "gpu", "language": "python" });
        remote
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        remote
    };
    let group = json!([{ "id": "g", "group": true, "instanced": true, "config": [] }]);
    assert_eq!(
        start(
            gpu(json!({ "dial": "ws://a/rutis", "listen": "public" })),
            json!([])
        )
        .await,
        "remote runtime gpu: dial and listen both set: rutis dials the runtime (dial) or the \
         runtime dials in (listen), not both"
    );
    assert_eq!(
        start(
            gpu(json!({ "dial": "ws://a/rutis", "instances": "g" })),
            group.clone()
        )
        .await,
        "remote runtime gpu: dial and instances both set: rutis dials one runtime; for an \
         instance per connection, the runtimes dial in: use listen with instances"
    );
    assert_eq!(
        start(gpu(json!({ "listen": "private" })), json!([])).await,
        "remote runtime gpu: no listener named private: add it to listen"
    );
    assert_eq!(
        start(
            gpu(json!({ "dial": "ws://a/rutis" })),
            json!([{ "id": "old", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "ws://a/rutis", "runtime": "gpu" } }])
        )
        .await,
        "remote runtime gpu: link row old connects it too: remove the link row, as \
         runtimes.remote connects it itself"
    );
}
