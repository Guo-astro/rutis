//! Leases of remote runtimes: a runtime that listens serves one controller
//! at a time; whatever a session loaded is cleaned up when it ends; a newer
//! connection takes over only after the old lease is gone; a controller
//! that reconnects gets a new lease. Each plugin start and cleanup is
//! written to a log on the runtime's side. Python needs `websockets`
//! (RUTIS_PYTHON, else python3; python on Windows); Go (feature `go`) the
//! Go toolchain; Bun (feature `bun`, Unix only) `bun` on PATH.
#![cfg(all(feature = "node", feature = "python"))]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::runtimes::{id, repo, COLD};
use rutis::host::{CtxHost, FiberView};
use rutis::Ctx;
use rutis_bridge::runtime::RuntimeAccessPlugin;
use rutis_bridge::runtime::RuntimePlugin;
use rutis_bridge::websocket::{Config, WebSocketPlugin};
use rutis_bridge::{
    peer_key, Credential, IdentityPlugin, LinkConfig, LinkPlugin, LinkState, Peer, Retry,
    StaticIdentity,
};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch};
use serde_json::json;
use tokio::io::AsyncBufReadExt;

#[derive(Clone, Copy, Debug)]
enum Language {
    Python,
    Node,
    #[cfg_attr(not(feature = "go"), allow(dead_code))]
    Go,
    /// Unix only: Bun on Windows is not a declared platform.
    #[cfg(all(unix, feature = "bun"))]
    Bun,
}

/// A runtime listening for controllers, and the log its plugin writes.
struct Remote {
    _process: tokio::process::Child,
    address: String,
    log: PathBuf,
    language: Language,
    _project: tempfile::TempDir,
}

impl Remote {
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    async fn wait_for(&self, line: &str) {
        COLD.eventually(
            || self.lines().iter().any(|l| l == line).then_some(()),
            &format!("{line:?} in {:?}", self.lines()),
        )
        .await;
    }

    fn row(&self) -> &'static str {
        match self.language {
            Language::Python => "py:logger",
            Language::Node => "logger",
            Language::Go => "edge:logger",
            #[cfg(all(unix, feature = "bun"))]
            Language::Bun => "edge:logger",
        }
    }

    fn runtime(&self) -> &'static str {
        match self.language {
            Language::Python => "py",
            Language::Node => "node",
            Language::Go => "edge",
            #[cfg(all(unix, feature = "bun"))]
            Language::Bun => "edge",
        }
    }
}

async fn remote(language: Language) -> Remote {
    remote_with_start_delay(language, 0).await
}

async fn remote_with_start_delay(language: Language, start_delay_ms: u64) -> Remote {
    let project = tempfile::tempdir().unwrap();
    let log = project.path().join("lease.log");
    let mut command = match language {
        Language::Python => {
            std::fs::write(
                project.path().join("logger.py"),
                format!(
                    "def apply(ctx, config):\n    log = open({log:?}, 'a')\n    log.write(f\"start {{config['who']}}\\n\"); log.flush()\n    def stop():\n        log.write(f\"stop {{config['who']}}\\n\"); log.close()\n    return stop\n",
                    log = log.display().to_string()
                ),
            )
            .unwrap();
            let python = std::env::var("RUTIS_PYTHON")
                .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into());
            let mut command = tokio::process::Command::new(python);
            command
                .args(["-m", "rutis", "listen:ws://127.0.0.1:0/rutis"])
                .args(["--id", "remote", "--peer", "main"])
                .arg(project.path())
                .env("PYTHONPATH", repo().join("python/rutis"))
                .current_dir(project.path());
            command
        }
        Language::Node => {
            let anchor = project.path().join("package.json");
            std::fs::write(&anchor, "{}").unwrap();
            let package = project.path().join("node_modules/logger");
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(
                package.join("package.json"),
                json!({ "name": "logger", "version": "1.0.0", "type": "module", "main": "index.mjs" }).to_string(),
            )
            .unwrap();
            std::fs::write(
                package.join("index.mjs"),
                format!(
                    "import {{ appendFileSync }} from 'node:fs'\nawait new Promise(resolve => setTimeout(resolve, {start_delay_ms}))\nexport function apply(ctx, config) {{\n  appendFileSync({log:?}, `start ${{config.who}}\\n`)\n  ctx.effect(() => () => {{\n    if (config.stopDelayMs) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, config.stopDelayMs)\n    appendFileSync({log:?}, `stop ${{config.who}}\\n`)\n  }})\n}}\n",
                    log = log.display().to_string()
                ),
            )
            .unwrap();
            let runtime = repo().join("node/rutis-runtime");
            let mut command = tokio::process::Command::new("node");
            command
                .args(["--import", "tsx"])
                .arg(runtime.join("src/runner.mjs"))
                .arg("listen:ws://127.0.0.1:0/rutis")
                .args(["--id", "remote", "--peer", "main"])
                .arg(&anchor)
                .current_dir(&runtime);
            command
        }
        Language::Go => {
            let binary = project.path().join(if cfg!(windows) {
                "logger.exe"
            } else {
                "logger"
            });
            let status = std::process::Command::new("go")
                .args(["build", "-o"])
                .arg(&binary)
                .arg("./cmd/logger")
                .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go"))
                .status()
                .expect("go is on PATH");
            assert!(status.success(), "go build");
            let mut command = tokio::process::Command::new(binary);
            command
                .arg("listen:ws://127.0.0.1:0/rutis")
                .args(["--id", "remote", "--peer", "main"])
                .arg(project.path());
            command
        }
        #[cfg(all(unix, feature = "bun"))]
        Language::Bun => {
            let package = project.path().join("node_modules/logger");
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(
                package.join("package.json"),
                json!({ "name": "logger", "version": "1.0.0", "type": "module", "main": "index.mjs" }).to_string(),
            )
            .unwrap();
            std::fs::write(
                package.join("index.mjs"),
                format!(
                    "import {{ appendFileSync }} from 'node:fs'\nawait new Promise(resolve => setTimeout(resolve, {start_delay_ms}))\nexport function apply(ctx, config) {{\n  appendFileSync({log:?}, `start ${{config.who}}\\n`)\n  return () => {{\n    if (config.stopDelayMs) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, config.stopDelayMs)\n    appendFileSync({log:?}, `stop ${{config.who}}\\n`)\n  }}\n}}\n",
                    log = log.display().to_string()
                ),
            )
            .unwrap();
            let mut command = tokio::process::Command::new("bun");
            command
                .args(["--no-install", "--no-env-file"])
                .arg(repo().join("bun/rutis-bun/src/main.ts"))
                .arg("listen:ws://127.0.0.1:0/rutis")
                .args(["--id", "remote", "--peer", "main"])
                .arg(project.path())
                .current_dir(project.path());
            command
        }
    };
    let mut process = command
        .env("RUTIS_TOKEN", "main-token")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(process.stderr.take().unwrap()).lines();
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
    // What the runtime reports later goes to the test's output.
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("runtime: {line}");
        }
    });
    Remote {
        _process: process,
        address,
        log,
        language,
        _project: project,
    }
}

/// A controller: an application named `main` whose loader runs one row of
/// the remote runtime, configured with `who`.
struct Controller {
    root: Ctx,
    link: FiberView,
    states: tokio::sync::watch::Receiver<LinkState>,
}

async fn controller(remote: &Remote, who: &str) -> Controller {
    controller_with_options(remote, who, true, 0).await
}

async fn controller_with_options(
    remote: &Remote,
    who: &str,
    reconnect: bool,
    stop_delay_ms: u64,
) -> Controller {
    let root = Ctx::root().unwrap();
    (&root.plugin(WebSocketPlugin::new(Config::new()).unwrap()))
        .await
        .unwrap();
    root.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main"))
            .present(id("remote"), Credential::Bearer("main-token".into())),
    ));
    let mut config = LinkConfig::dial(id("remote"), "websocket", "main", &remote.address)
        .require("runtime")
        .retry(Retry {
            initial: Duration::from_millis(50),
            max: Duration::from_millis(300),
            jitter: 0.0,
            ..Retry::default()
        });
    config.reconnect = reconnect;
    let link = LinkPlugin::new(config);
    let states = link.state();
    let link = root.plugin(link);
    root.plugin(RuntimeAccessPlugin::new(id("remote"), remote.runtime()));
    let runtime = RuntimePlugin::remote(remote.runtime());
    let rows = Arc::new(match remote.language {
        Language::Node => RuntimeResolver::node(runtime.handle()),
        _ => RuntimeResolver::modules(runtime.handle()),
    });
    root.plugin(runtime);
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(rows.clone()),
        LoaderOptions::default(),
    );
    let loader = plugin.handle();
    (&root.plugin(plugin)).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(rows));
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
        { "id": "l", "name": remote.row(), "config": { "who": who, "stopDelayMs": stop_delay_ms, "log": remote.log } }
    ] }]))
    .unwrap();
    loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    Controller { root, link, states }
}

async fn successive_controllers_get_clean_leases(language: Language) {
    let remote = remote(language).await;
    let first = controller(&remote, "a").await;
    remote.wait_for("start a").await;
    // The controller leaves: its lease is cleaned up, the runtime stays.
    first.link.dispose().await.unwrap();
    remote.wait_for("stop a").await;
    let second = controller(&remote, "b").await;
    remote.wait_for("start b").await;
    drop(second);
    drop(first);
}

async fn a_newer_controller_takes_over_after_the_old_lease_is_gone(language: Language) {
    takeover(language, 0).await;
}

async fn takeover(language: Language, stop_delay_ms: u64) {
    let remote = remote_with_start_delay(language, stop_delay_ms).await;
    // A replaced dial link normally reconnects. Disable that before takeover,
    // not after start b: otherwise a can replace b while its child is still
    // loading, and the two controllers keep evicting one another.
    let first = controller_with_options(&remote, "a", false, stop_delay_ms).await;
    remote.wait_for("start a").await;
    let second = controller(&remote, "b").await;
    remote.wait_for("start b").await;
    let lines = remote.lines();
    let stop_a = lines
        .iter()
        .position(|l| l == "stop a")
        .expect("the old lease cleaned up");
    let start_b = lines.iter().position(|l| l == "start b").unwrap();
    assert!(
        stop_a < start_b,
        "the old lease goes before the new one starts: {lines:?}"
    );
    COLD.eventually(
        || matches!(*first.states.borrow(), LinkState::Stopped { .. }).then_some(()),
        "the replaced controller stopped without reconnecting",
    )
    .await;
    // Read again: once a stopped, it adds nothing, so b keeps its lease.
    assert_eq!(remote.lines(), ["start a", "stop a", "start b"]);
    first.link.dispose().await.unwrap();
    drop(second);
}

async fn a_reconnecting_controller_gets_a_new_lease(language: Language) {
    let remote = remote(language).await;
    let controller = controller(&remote, "a").await;
    remote.wait_for("start a").await;
    let peer = COLD
        .eventually(
            || controller.root.get_at::<Peer>(&peer_key(&id("remote"))),
            "the peer",
        )
        .await;
    let generation = peer.generation();
    peer.connection()
        .close(rutis_bridge::session::Error::Transport("cut".into()));
    drop(peer);
    remote.wait_for("stop a").await;
    COLD.eventually(
        || matches!(*controller.states.borrow(), LinkState::Ready { generation: g } if g > generation).then_some(()),
        "the link back",
    )
    .await;
    COLD.eventually(
        || (remote.lines().iter().filter(|l| *l == "start a").count() == 2).then_some(()),
        "the row loaded again",
    )
    .await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn python_successive_controllers_get_clean_leases() {
    successive_controllers_get_clean_leases(Language::Python).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn python_takeover_after_the_old_lease_is_gone() {
    a_newer_controller_takes_over_after_the_old_lease_is_gone(Language::Python).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn python_reconnecting_controller_gets_a_new_lease() {
    a_reconnecting_controller_gets_a_new_lease(Language::Python).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn node_successive_controllers_get_clean_leases() {
    successive_controllers_get_clean_leases(Language::Node).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn node_takeover_after_the_old_lease_is_gone() {
    a_newer_controller_takes_over_after_the_old_lease_is_gone(Language::Node).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn node_takeover_with_slow_start_and_cleanup() {
    // Stretch both cleanup and module loading beyond the maximum retry delay
    // (no jitter). With reconnect left enabled on a, b is repeatedly
    // evicted before applying its row; this used to time out waiting for b.
    // clock: widen — the delays only make the window the regression needs;
    // what `takeover` asserts holds whatever they are.
    takeover(Language::Node, 600).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn node_reconnecting_controller_gets_a_new_lease() {
    a_reconnecting_controller_gets_a_new_lease(Language::Node).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_successive_controllers_get_clean_leases() {
    successive_controllers_get_clean_leases(Language::Go).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_takeover_after_the_old_lease_is_gone() {
    a_newer_controller_takes_over_after_the_old_lease_is_gone(Language::Go).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_takeover_with_a_slow_cleanup() {
    // clock: widen — a cleanup slower than the maximum retry delay; what
    // `takeover` asserts holds whatever it is.
    takeover(Language::Go, 400).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_reconnecting_controller_gets_a_new_lease() {
    a_reconnecting_controller_gets_a_new_lease(Language::Go).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_successive_controllers_get_clean_leases() {
    successive_controllers_get_clean_leases(Language::Bun).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_takeover_after_the_old_lease_is_gone() {
    a_newer_controller_takes_over_after_the_old_lease_is_gone(Language::Bun).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_takeover_with_a_slow_cleanup() {
    // clock: widen — a cleanup slower than the maximum retry delay; what
    // `takeover` asserts holds whatever it is.
    takeover(Language::Bun, 400).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_reconnecting_controller_gets_a_new_lease() {
    a_reconnecting_controller_gets_a_new_lease(Language::Bun).await;
}
