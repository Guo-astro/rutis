//! Apps hosting Swift plugins with `RutisApp` (macOS here, as a stand-in;
//! the iOS simulator in tests/e2e/tests/ios_app.rs): an app that dials in
//! is an instance of the group `runtimes.remote[].instances` names; two
//! apps with the same plugin are two instances, a local row in each using
//! its own app's service; `stop` stops that app's users and `start` brings
//! them back, as the app's observable state says. An app that listens is
//! linked to like any remote runtime, and comes back after `stop`, `start`.
//!
//! The app is `RutisAppFixture` (swift/rutis/Sources/RutisAppFixture),
//! driven on stdin as an app's business layer drives `RutisApp`.
#![cfg(target_os = "macos")]
// The tests hold the environment until they end, across their awaits: the
// variables they set are for the host they start.
#![allow(clippy::await_holding_lock)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use common::{environment, repo, write};
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Runtimes start cold: twice the hang guard.
const COLD: Guard = Guard(Duration::from_secs(2 * HANG_GUARD.as_secs()));

/// The host's row in each app's instance: while it runs, the file named
/// after the app (its `device` service says) is in `dir`.
const USER: &str = r#"
import os

inject = ["device"]


def apply(ctx, config):
    path = os.path.join(config["dir"], ctx.use("device").name())
    open(path, "w").close()
    ctx.effect(lambda: os.remove(path))
"#;

/// `RutisAppFixture`, built once.
fn fixture() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = std::process::Command::new("swift")
            .args(["build", "--product", "RutisAppFixture"])
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build --product RutisAppFixture");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).join("RutisAppFixture")
    })
}

/// An app: its commands in, its lines out.
struct App {
    name: String,
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::sync::mpsc::UnboundedReceiver<String>,
    /// Every line so far.
    seen: Vec<String>,
}

impl App {
    /// The app `id`, `dial`ing or `listen`ing on `address`.
    fn new(mode: &str, address: &str, id: &str, token: &str) -> Self {
        let mut child = tokio::process::Command::new(fixture())
            .args([mode, address, id])
            .env("RUTIS_TOKEN", token)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (send, lines) = tokio::sync::mpsc::unbounded_channel();
        let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let (name, also) = (id.to_owned(), send.clone());
        tokio::spawn(async move {
            while let Ok(Some(line)) = stdout.next_line().await {
                eprintln!("{name}: {line}");
                let _ = send.send(line);
            }
        });
        let name = id.to_owned();
        tokio::spawn(async move {
            while let Ok(Some(line)) = stderr.next_line().await {
                eprintln!("{name}: {line}");
                let _ = also.send(line);
            }
        });
        let stdin = child.stdin.take().unwrap();
        App {
            name: id.to_owned(),
            child,
            stdin,
            lines,
            seen: Vec::new(),
        }
    }

    /// The first line (from now on) that `pick` takes.
    async fn line<T>(&mut self, what: &str, mut pick: impl FnMut(&str) -> Option<T>) -> T {
        let what = format!("{}: {what}", self.name);
        COLD.within(&what, async {
            loop {
                let line = self.lines.recv().await.expect("the app's lines");
                self.seen.push(line.clone());
                if let Some(found) = pick(&line) {
                    break found;
                }
            }
        })
        .await
    }

    /// Runs `command` (`start`, `stop`) to its end.
    async fn command(&mut self, command: &str) {
        self.stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        let done = format!("done {command}");
        self.line(&done.clone(), |line| (line == done).then_some(()))
            .await;
    }

    /// The states it reported so far, in order.
    fn states(&self) -> Vec<&str> {
        self.seen
            .iter()
            .filter_map(|line| line.strip_prefix("state: "))
            .collect()
    }
}

/// Two apps with the plugin `device` dial in: each is an instance, its
/// `device` the service the host's row in that instance uses; one app's
/// `stop` stops its users only, its `start` brings them back.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn apps_that_dial_in_are_instances_their_users_stop_and_start_with_them() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let users = dir.path().join("users");
    std::fs::create_dir_all(&users).unwrap();
    write(dir.path(), &[("user.py", USER)]);
    std::env::set_var("RUTIS_TOKEN_PHONE", "shared");
    std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
    let python = std::env::var_os("RUTIS_PYTHON").map(PathBuf::from);
    let mut config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
        "runtimes": {
            "py": { "project": dir.path(), "python": python },
            "remote": [{ "name": "phone", "language": "swift", "listen": "public", "instances": "phone" }]
        },
        "rows": [{ "id": "phone", "group": true, "instanced": true, "config": [
            { "id": "device", "name": "phone:device" },
            { "id": "user", "name": "py:user", "config": { "dir": users } }
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

    let mut kim = App::new("dial", &address, "kim-iphone", "shared");
    let mut lee = App::new("dial", &address, "lee-iphone", "shared");
    kim.command("start").await;
    lee.command("start").await;
    let running = |user: &str| users.join(user).exists();
    let devices = || {
        host.loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == "device" && entry.instance.is_some())
            .count()
    };
    COLD.until(
        || running("kim-iphone") && running("lee-iphone"),
        "each app's user, with its own device",
    )
    .await;
    assert_eq!(devices(), 2, "an instance per app");
    // Outside the instances, there is no device.
    assert!(host.service("device").is_none());

    // kim stops: its user stops, lee's runs on.
    kim.command("stop").await;
    COLD.until(|| !running("kim-iphone"), "kim's user stopped")
        .await;
    assert!(running("lee-iphone"));
    COLD.until(|| devices() == 1, "kim's instance closed").await;

    // kim starts again: a new connection, the user back.
    kim.command("start").await;
    COLD.until(|| running("kim-iphone"), "kim's user back")
        .await;
    kim.line("connected again", |line| {
        (line == "state: connected").then_some(())
    })
    .await;
    assert_eq!(
        kim.states(),
        [
            "stopped",
            "connecting",
            "connected",
            "stopped",
            "connecting",
            "connected"
        ],
        "kim's states"
    );
    assert!(
        kim.seen.iter().any(|line| line == "observed"),
        "the state is observable"
    );
    kim.command("quit").await;
    lee.command("quit").await;
    COLD.until(
        || !running("kim-iphone") && !running("lee-iphone") && devices() == 0,
        "both instances closed",
    )
    .await;
    let _ = (kim.child.wait().await, lee.child.wait().await);
    host.shut_down(HANG_GUARD).await.ok();
}

/// An app listening (on loopback here): the host links to it as to any
/// remote runtime, `<runtime>:<plugin>` rows run on it; after `stop` the
/// service is gone, after `start` the link connects again and it is back.
/// guarantee: loader/remote-runtime-rows
#[tokio::test(flavor = "multi_thread")]
async fn an_app_that_listens_is_linked_to_and_comes_back_after_stop_and_start() {
    let _environment = environment();
    // A port of its own: the address stays the same across stop and start,
    // as a device's does.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listen = format!("ws://127.0.0.1:{port}/rutis");
    let mut app = App::new("listen", &listen, "pad", "pad-token");
    app.stdin.write_all(b"start\n").await.unwrap();
    let address = app
        .line("its address", |line| {
            line.strip_prefix("rutis: listening on ").map(str::to_owned)
        })
        .await;
    app.line("started", |line| (line == "done start").then_some(()))
        .await;
    std::env::set_var("RUTIS_TOKEN_PAD", "pad-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "runtimes": { "remote": [{ "name": "pad", "language": "swift" }] },
        "rows": [
            { "id": "pad", "name": "rutis-bridge/peer", "config": { "peer": "pad", "dial": address, "runtime": "pad" } },
            { "id": "device", "name": "pad:device" }
        ]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    let name = || {
        let service = host.service("device")?;
        let value =
            tokio::task::block_in_place(|| service.invoke("name", json!([]).into())).ok()?;
        value.json().ok()
    };
    let named = COLD.eventually(name, "the app's device").await;
    assert_eq!(named, json!("pad"));
    app.line("connected", |line| {
        (line == "state: connected").then_some(())
    })
    .await;

    app.command("stop").await;
    COLD.until(|| host.service("device").is_none(), "the device gone")
        .await;
    app.command("start").await;
    let named = COLD.eventually(name, "the device back").await;
    assert_eq!(named, json!("pad"));
    app.command("quit").await;
    host.shut_down(HANG_GUARD).await.ok();
}
