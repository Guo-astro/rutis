//! A runtime that dials in (`runtimes.remote` with `listen` and
//! `instances`): its connection is an instance of the group, the group's
//! `<name>:<plugin>` rows run on it, and a local row in the same instance
//! uses its services; when it goes, so does the instance.
//!
//! The runtime is a real one: `python -m rutis dial:…` to the host's
//! listener, with the shared token.

// The test holds the environment until it ends, across its awaits: the
// variables it sets are for the host it starts.
#![allow(clippy::await_holding_lock)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{environment, repo, write};
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::json;
use tokio::io::AsyncBufReadExt;

/// Twice HANG_GUARD: the Python runtimes start cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// The remote runtime's plugin.
const CONTACTS: &str = r#"
class Contacts:
    def owner(self):
        return "pixel"


provides = {"contacts": Contacts}


def apply(ctx, config):
    ctx.provide("contacts", Contacts())
"#;

/// The host's own row in the same instance, using the connection's
/// `contacts`.
const DIGEST: &str = r#"
inject = ["contacts"]


class Digest:
    def __init__(self, contacts):
        self.contacts = contacts

    def summary(self):
        return "contacts of " + self.contacts.owner()


provides = {"digest": Digest}


def apply(ctx, config):
    ctx.provide("digest", Digest(ctx.use("contacts")))
"#;

fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// The remote runtime `pixel`, dialing `address` for `test` with the
/// shared token.
fn remote_python(project: &Path, address: &str) -> tokio::process::Child {
    let mut child = tokio::process::Command::new(python())
        .args(["-m", "rutis"])
        .arg(format!("dial:{address}"))
        .args(["--id", "pixel", "--peer", "test"])
        .arg(project)
        .env("PYTHONPATH", repo().join("python/rutis"))
        .env("RUTIS_TOKEN", "shared")
        .current_dir(project)
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("runtime: {line}");
        }
    });
    child
}

/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_dials_in_is_an_instance() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote");
    write(
        dir.path(),
        &[
            ("remote/contacts_plugin.py", CONTACTS),
            ("digest.py", DIGEST),
        ],
    );

    std::env::set_var("RUTIS_TOKEN_PHONE", "shared");
    std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
    let python = std::env::var_os("RUTIS_PYTHON").map(PathBuf::from);
    let mut config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
        "runtimes": {
            "py": { "project": dir.path(), "python": python },
            "remote": [{ "name": "phone", "language": "python", "listen": "public", "instances": "phone" }]
        },
        "rows": [{ "id": "phone", "group": true, "instanced": true, "config": [
            { "id": "contacts", "name": "phone:contacts_plugin" },
            { "id": "digest", "name": "py:digest" }
        ] }]
    }))
    .unwrap();
    config.rebase(dir.path());
    let host = Host::start(&config).await.unwrap();
    COLD.within("the Python runtime", host.runtimes_ready())
        .await
        .unwrap();
    host.load(config.rows()).await.unwrap();
    let address = format!("ws://{}/rutis", host.listening[0].1);
    let mut runtime = remote_python(&remote, &address);

    // The host's row in the connection's instance uses its `contacts`,
    // which is that instance's own.
    let digest = COLD
        .eventually(|| host.service("digest"), "the digest of the instance")
        .await;
    let summary = digest.invoke("summary", json!([]).into()).unwrap();
    assert_eq!(summary.json().unwrap(), json!("contacts of pixel"));
    drop(digest);
    assert!(host.service("contacts").is_none());
    let copies = |id: &str| {
        host.loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == id && entry.instance.is_some())
            .count()
    };
    assert_eq!(copies("contacts"), 1);

    // The connection goes: its instance closes.
    runtime.kill().await.unwrap();
    COLD.eventually(
        || (host.service("digest").is_none() && copies("contacts") == 0).then_some(()),
        "the instance closed",
    )
    .await;
    host.shut_down(HANG_GUARD).await.ok();
}
