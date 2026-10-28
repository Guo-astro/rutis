//! Apps hosting Swift plugins with `RutisApp` (macOS here, as a stand-in;
//! the iOS simulator in tests/e2e/tests/ios_app.rs): apps dialing in to a
//! `rutis-host/listen` row install their plugins, which rutis sees as
//! `<id>:<plugin>`; their services are global, and a later app providing a
//! name another app provides is refused, saying why; `stop` stops that
//! app's users and `start` brings them back, as the app's observable state
//! says. An app that listens is reached by a `rutis-host/dial` row and comes
//! back after `stop`, `start`.
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

/// The host's row using an app's `device`: while it runs, the file named
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

    /// Until `text` has been said `times` times in all: stdout and stderr
    /// interleave as they come, so it may have been read already.
    async fn said(&mut self, text: &str, times: usize) {
        while self.seen.iter().filter(|line| *line == text).count() < times {
            self.line(text, |line| (line == text).then_some(())).await;
        }
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

    /// Reads every line the app says for `duration`. The fixture says
    /// `done …` and `state: …` from different tasks, so a state may come
    /// after the `done` line of the call that caused it.
    async fn settle(&mut self, duration: Duration) {
        let until = tokio::time::Instant::now() + duration;
        while let Ok(Some(line)) = tokio::time::timeout_at(until, self.lines.recv()).await {
            self.seen.push(line);
        }
    }

    /// Starts it listening; the address it says it bound (its port when
    /// asked for `:0`).
    async fn listening(&mut self) -> String {
        self.stdin.write_all(b"start\n").await.unwrap();
        let address = self
            .line("its address", |line| {
                line.strip_prefix("rutis: listening on ").map(str::to_owned)
            })
            .await;
        self.said("done start", 1).await;
        address
    }

    /// Every address it said it listens on so far, in order.
    fn addresses(&self) -> Vec<&str> {
        self.seen
            .iter()
            .filter_map(|line| line.strip_prefix("rutis: listening on "))
            .collect()
    }

    /// The states it reported so far, in order.
    fn states(&self) -> Vec<&str> {
        self.seen
            .iter()
            .filter_map(|line| line.strip_prefix("state: "))
            .collect()
    }
}

/// Two apps with the plugin `device` dial in to the listen row `phone`:
/// both are connected; the first's `kim-iphone:device` provides `device`,
/// which the host's row uses, and the host emits `rutis/service` for it;
/// the second's is refused, as the name is taken. The first's `stop`
/// withdraws it (`rutis/service`, not available) and stops its user; its
/// `start` brings both back.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn apps_on_one_listen_row_bring_their_plugins_the_later_name_refused() {
    use rutis_bridge::events::ServiceEvent;
    use std::sync::{Arc, Mutex};

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
        "runtimes": { "py": { "project": dir.path(), "python": python } },
        "rows": [
            { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } },
            { "id": "user", "name": "py:user", "config": { "dir": users } }
        ]
    }))
    .unwrap();
    config.rebase(dir.path());
    let host = Host::start(&config).await.unwrap();
    let heard: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let into = heard.clone();
    host.root()
        .on::<ServiceEvent>(move |event, _| {
            into.lock()
                .unwrap()
                .push((event.name.clone(), event.available));
            std::future::ready(Ok(None))
        })
        .unwrap();
    COLD.within("the Python runtime", host.runtimes_ready())
        .await
        .unwrap();
    host.load(config.rows()).await.unwrap();
    let address = format!("ws://{}/rutis", host.listening[0].1);
    let device = |available: bool| ("device".to_owned(), available);
    let said = |event: (String, bool)| {
        heard
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == event)
            .count()
    };
    let connected = || -> Vec<String> {
        host.connections()
            .into_iter()
            .flat_map(|row| row.connected)
            .map(|connected| connected.id)
            .collect()
    };
    let running = |user: &str| users.join(user).exists();

    let mut kim = App::new("dial", &address, "kim-iphone", "shared");
    kim.command("start").await;
    COLD.until(|| running("kim-iphone"), "the user, with kim's device")
        .await;
    assert_eq!(said(device(true)), 1, "{:?}", heard.lock().unwrap());
    let mut lee = App::new("dial", &address, "lee-iphone", "shared");
    lee.command("start").await;
    COLD.until(
        || connected() == ["kim-iphone", "lee-iphone"],
        "both connected",
    )
    .await;
    lee.said("state: connected", 1).await;
    // lee's device is refused: the service stays kim's.
    let name = || {
        let service = host.service("device")?;
        let value =
            tokio::task::block_in_place(|| service.invoke("name", json!([]).into())).ok()?;
        value.json().ok()
    };
    assert_eq!(name(), Some(json!("kim-iphone")));
    assert!(!running("lee-iphone"));

    // kim stops: its device is withdrawn and its user stops.
    kim.command("stop").await;
    COLD.until(|| !running("kim-iphone"), "kim's user stopped")
        .await;
    COLD.until(|| said(device(false)) == 1, "the device withdrawn")
        .await;
    COLD.until(|| connected() == ["lee-iphone"], "kim disconnected")
        .await;

    // kim starts again: a new connection, the device and its user back.
    kim.command("start").await;
    COLD.until(|| running("kim-iphone"), "kim's user back")
        .await;
    assert_eq!(said(device(true)), 2);
    kim.said("state: connected", 2).await;
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
        || !running("kim-iphone") && connected().is_empty(),
        "both gone",
    )
    .await;
    let _ = (kim.child.wait().await, lee.child.wait().await);
    host.shut_down(HANG_GUARD).await.ok();
}

/// What `run` prints when two apps on one listen row provide the same
/// name: each connection, and why the later app's plugin is refused.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn run_says_why_the_later_apps_service_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config = json!({
        "id": "test",
        "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
        "rows": [{ "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }]
    });
    std::fs::write(dir.path().join("rutis.json"), config.to_string()).unwrap();
    let mut host = tokio::task::block_in_place(|| {
        let mut host = common::process::Host::start_in_env(
            &["run", "rutis.json"],
            dir.path(),
            &[("RUTIS_TOKEN_PHONE", "shared")],
        );
        let address = host.wait_line(|line| {
            line.strip_prefix("rutis-host: listening on public ")
                .map(str::to_owned)
        });
        (host, address)
    });
    let address = format!("ws://{}/rutis", host.1);
    let mut kim = App::new("dial", &address, "kim-iphone", "shared");
    kim.command("start").await;
    tokio::task::block_in_place(|| {
        host.0
            .wait_line(|line| (line == "phone: connected kim-iphone").then_some(()))
    });
    let mut lee = App::new("dial", &address, "lee-iphone", "shared");
    lee.command("start").await;
    tokio::task::block_in_place(|| {
        host.0.wait_lines(&[
            "phone: connected lee-iphone",
            "lee-iphone:device: device is provided here by row kim-iphone:device: \
             not registered from lee-iphone",
        ])
    });
    kim.command("quit").await;
    lee.command("quit").await;
    let _ = (kim.child.wait().await, lee.child.wait().await);
    common::process::signal(&host.0, libc::SIGTERM, false);
    let (status, _, _) = tokio::task::block_in_place(|| host.0.exit());
    assert!(status.success(), "{status:?}");
}

/// An app listening (on loopback here): a dial row reaches it, and the
/// plugin it installs is `pad:device`; after `stop` the service is gone,
/// after `start` the row connects again and it is back.
/// guarantee: host/remote-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn an_app_that_listens_is_dialed_and_comes_back_after_stop_and_start() {
    let _environment = environment();
    // A free port the app picks (`:0`), and keeps across stop and start,
    // as a device's address stays the same.
    let mut app = App::new("listen", "ws://127.0.0.1:0/rutis", "pad", "pad-token");
    let address = app.listening().await;
    std::env::set_var("RUTIS_TOKEN_PAD", "pad-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "rows": [{ "id": "pad", "name": "rutis-host/dial", "config": { "address": address } }]
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
    app.said("state: connected", 1).await;

    app.command("stop").await;
    COLD.until(|| host.service("device").is_none(), "the device gone")
        .await;
    app.command("start").await;
    // stderr: it may come after `done start`.
    while app.addresses().len() < 2 {
        app.line("the second address", |line| {
            line.starts_with("rutis: listening on ").then_some(())
        })
        .await;
    }
    assert_eq!(
        app.addresses(),
        [address.as_str(), address.as_str()],
        "the same address again"
    );
    let named = COLD.eventually(name, "the device back").await;
    assert_eq!(named, json!("pad"));
    app.command("quit").await;
    host.shut_down(HANG_GUARD).await.ok();
}

/// `start()` and `stop()` called each in a Task of its own, as an app does
/// on its scene phase, take effect in call order: a start called while a
/// stop is under way (the user came back during the cleanup) starts again
/// once the stop is done, and a stop called right after a start undoes it.
/// guarantee: swift/app-start-stop
#[tokio::test(flavor = "multi_thread")]
async fn an_apps_start_and_stop_take_effect_in_call_order() {
    let _environment = environment();
    let mut app = App::new("listen", "ws://127.0.0.1:0/rutis", "pad", "pad-token");
    let listen = app.listening().await;
    std::env::set_var("RUTIS_TOKEN_PAD", "pad-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "rows": [{ "id": "pad", "name": "rutis-host/dial", "config": { "address": listen } }]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    COLD.until(|| host.service("device").is_some(), "the app's device")
        .await;
    app.said("state: connected", 1).await;

    // To the background, and back during the stop's cleanup (300 ms).
    app.stdin.write_all(b"stop&\nstart&\n").await.unwrap();
    app.said("done stop&", 1).await;
    app.said("done start&", 1).await;
    app.said("state: connected", 2).await;
    COLD.until(|| host.service("device").is_some(), "the device back")
        .await;
    assert_eq!(
        app.states().last(),
        Some(&"connected"),
        "started again: {:?}",
        app.states()
    );

    // Stopped; then started and at once stopped: it stays stopped.
    app.command("stop").await;
    app.stdin.write_all(b"start&\nstop&\n").await.unwrap();
    app.said("done start&", 2).await;
    app.said("done stop&", 2).await;
    // Every state it says meanwhile is read: the last one counts.
    app.settle(Duration::from_millis(500)).await;
    assert_eq!(
        app.states().last(),
        Some(&"stopped"),
        "stopped in the end: {:?}",
        app.states()
    );
    app.command("quit").await;
    host.shut_down(HANG_GUARD).await.ok();
}
