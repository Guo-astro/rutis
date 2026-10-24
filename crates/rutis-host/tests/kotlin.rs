//! Kotlin runtimes in rutis-host (feature `kotlin`; needs `java`, JDK 17):
//! a remote runtime of `language: "kotlin"` (or `"kt"`) is the Kotlin SDK
//! of an application, here its jvm target (the conformance jar, the same
//! dialing and listening code an Android application runs). Its rows are
//! `<runtime>:<plugin>`, linked to by a peer row when it listens, or an
//! instance per connection when it dials in.
#![cfg(feature = "kotlin")]
// Each test holds the environment until it ends, across its awaits: the
// host reads the tokens it sets.
#![allow(clippy::await_holding_lock)]

mod common;
#[path = "../../rutis-bridge/tests/kotlin/mod.rs"]
mod kotlin;

use rutis_bridge::session::HostDispatch;
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt;

/// Runtimes start cold (a JVM): twice the hang guard.
const COLD: Guard = Guard(std::time::Duration::from_secs(2 * HANG_GUARD.as_secs()));

/// The conformance jar running remotely on `channel` (`listen:` or
/// `dial:`) as the endpoint `id`, for the host `test`, presenting or
/// expecting `token`; its stderr lines come out of the receiver.
fn remote(
    channel: &str,
    id: &str,
    token: &str,
) -> (
    tokio::process::Child,
    tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    let mut child = tokio::process::Command::new("java")
        .args(kotlin::JVM_OPTIONS)
        .arg("-jar")
        .arg(kotlin::jar())
        .arg(channel)
        .args(["--id", id, "--peer", "test"])
        .arg(std::env::temp_dir())
        .env("RUTIS_TOKEN", token)
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let (send, lines) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok(Some(line)) = stderr.next_line().await {
            eprintln!("runtime: {line}");
            let _ = send.send(line);
        }
    });
    (child, lines)
}

/// The next line from `lines` that starts with `prefix`, without it.
async fn said(lines: &mut tokio::sync::mpsc::UnboundedReceiver<String>, prefix: &str) -> String {
    COLD.within(prefix, async {
        loop {
            let line = lines.recv().await.expect(prefix);
            if let Some(rest) = line.strip_prefix(prefix) {
                break rest.to_owned();
            }
        }
    })
    .await
}

async fn call(
    service: std::sync::Arc<dyn HostDispatch>,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let what = format!("{method} answering");
    let method = method.to_owned();
    COLD.within(
        &what,
        tokio::task::spawn_blocking(move || service.invoke(&method, args.into())),
    )
    .await
    .unwrap()
    .map_err(|error| error.to_string())
    .and_then(|value| value.json().map_err(|error| error.to_string()))
}

/// A Kotlin runtime listening on another machine (here, on loopback): a
/// peer row links to it, `language: "kotlin"`, and its rows are
/// `<runtime>:<plugin>`.
/// guarantee: loader/remote-runtime-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_kotlin_runtime_runs_rows_named_after_it() {
    let _environment = common::environment();
    let (_child, mut lines) = remote("listen:ws://127.0.0.1:0/rutis", "edge", "edge-token");
    let address = said(&mut lines, "rutis: listening on ").await;
    std::env::set_var("RUTIS_TOKEN_EDGE", "edge-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "runtimes": { "remote": [{ "name": "edge", "language": "kotlin" }] },
        "rows": [
            { "id": "edge", "name": "rutis-bridge/peer", "config": { "peer": "edge", "dial": address, "runtime": "edge" } },
            { "id": "probe", "name": "edge:probe" }
        ]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    let service = COLD
        .eventually(|| host.service("work"), "the work service")
        .await;
    assert_eq!(
        call(service, "ping", json!([])).await.unwrap(),
        json!("pong")
    );
    host.shut_down(HANG_GUARD).await.ok();
}

/// Kotlin runtimes that dial in to the host's listener (`language: "kt"`):
/// each connection is an instance of the group `runtimes.remote[].instances`
/// names, its rows `<name>:<plugin>` on that connection, two connections
/// with the same plugin each in their own instance; a connection going
/// closes its instance alone.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn kotlin_runtimes_that_dial_in_are_instances() {
    let _environment = common::environment();
    std::env::set_var("RUTIS_TOKEN_PHONE", "shared");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
        "runtimes": {
            "remote": [{ "name": "phone", "language": "kt", "listen": "public", "instances": "phone" }]
        },
        "rows": [{ "id": "phone", "group": true, "instanced": true, "config": [
            { "id": "probe", "name": "phone:probe" }
        ] }]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    let address = format!("dial:ws://{}/rutis", host.listening[0].1);
    let (mut pixel, mut pixel_lines) = remote(&address, "pixel", "shared");
    let (_tablet, mut tablet_lines) = remote(&address, "tablet", "shared");
    said(&mut pixel_lines, "rutis: connected to ").await;
    said(&mut tablet_lines, "rutis: connected to ").await;
    let running = |id: &str| {
        host.loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == id && entry.instance.is_some())
            .count()
    };
    COLD.until(|| running("probe") == 2, "a row in each instance")
        .await;
    pixel.kill().await.unwrap();
    COLD.until(|| running("probe") == 1, "the pixel's instance closed")
        .await;
    host.shut_down(HANG_GUARD).await.ok();
}

/// The host's row in each instance, using its connection's `work`: it
/// loads only once a call through it answered.
const WATCHER: &str = r#"
inject = ["work"]


def apply(ctx, config):
    if ctx.use("work").ping() != "pong":
        raise RuntimeError("the work service did not answer")
"#;

/// An application hosting the test plugins through `RutisApp`
/// (`arcships.rutis.conformance.AppKt`), told `start` and `stop` on its
/// stdin; the states it goes through come out of the receiver.
struct App {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    states: tokio::sync::mpsc::UnboundedReceiver<String>,
}

impl App {
    fn start(address: &str, id: &str, token: &str) -> Self {
        let mut child = tokio::process::Command::new("java")
            .args(kotlin::JVM_OPTIONS)
            .arg("-cp")
            .arg(kotlin::jar())
            .arg("arcships.rutis.conformance.AppKt")
            .args(["connect", address, id])
            .env("RUTIS_TOKEN", token)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let (send, states) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(Some(line)) = stdout.next_line().await {
                eprintln!("app: {line}");
                if let Some(state) = line.strip_prefix("state: ") {
                    let _ = send.send(state.to_owned());
                }
            }
        });
        Self {
            child,
            stdin,
            states,
        }
    }

    async fn tell(&mut self, command: &str) {
        use tokio::io::AsyncWriteExt;
        self.stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
    }

    /// Waits for a state starting with `prefix`.
    async fn reaches(&mut self, prefix: &str) {
        COLD.within(prefix, async {
            loop {
                let state = self.states.recv().await.expect(prefix);
                if state.starts_with(prefix) {
                    break;
                }
            }
        })
        .await
    }
}

/// `RutisApp` (design §五) on the desktop JVM: `start` dials in and rutis
/// loads the rows of its instance, a Python row there using the
/// application's service; `stop` unloads them and the instance closes, its
/// user stopping too; `start` again brings both back. The state follows:
/// connecting, connected, stopped.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn an_application_stops_and_starts_again() {
    let _environment = common::environment();
    let dir = tempfile::tempdir().unwrap();
    common::write(dir.path(), &[("watcher.py", WATCHER)]);
    std::env::set_var("RUTIS_TOKEN_PHONE", "shared");
    std::env::set_var("RUTIS_PYTHON_PATH", common::repo().join("python/rutis"));
    let python = std::env::var_os("RUTIS_PYTHON").map(std::path::PathBuf::from);
    let mut config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
        "runtimes": {
            "py": { "project": dir.path(), "python": python },
            "remote": [{ "name": "phone", "language": "kotlin", "listen": "public", "instances": "phone" }]
        },
        "rows": [{ "id": "phone", "group": true, "instanced": true, "config": [
            { "id": "probe", "name": "phone:probe" },
            { "id": "watcher", "name": "py:watcher" }
        ] }]
    }))
    .unwrap();
    config.rebase(dir.path());
    let host = Host::start(&config).await.unwrap();
    COLD.within("the Python runtime", host.runtimes_ready())
        .await
        .unwrap();
    host.load(config.rows()).await.unwrap();
    let active = |id: &str| {
        host.loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == id && entry.instance.is_some())
            .filter(|entry| {
                matches!(&entry.status, rutis_loader::report::EntryStatus::Running(snapshot)
                    if snapshot.state == rutis::host::FiberState::Active)
            })
            .count()
    };

    let mut app = App::start(
        &format!("ws://{}/rutis", host.listening[0].1),
        "pixel",
        "shared",
    );
    app.reaches("Stopped(").await;
    app.tell("start").await;
    app.reaches("Connected(").await;
    COLD.until(|| active("watcher") == 1, "the watcher using the app")
        .await;

    app.tell("stop").await;
    app.reaches("Stopped(reason=null)").await;
    COLD.until(
        || active("watcher") == 0 && active("probe") == 0,
        "the instance closed",
    )
    .await;

    app.tell("start").await;
    app.reaches("Connected(").await;
    COLD.until(|| active("watcher") == 1, "the watcher back")
        .await;
    app.child.kill().await.unwrap();
    host.shut_down(HANG_GUARD).await.ok();
}
