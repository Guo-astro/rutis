//! The dial-out conformance suite (`docs/design-remote-runtime-dial-2026-10-14.md`
//! §六, `docs/design-remote-session-2026-10-28.md` §六): a rutis-host with a
//! `rutis-host/listen` row on a loopback WebSocket listener, each language's
//! runtime dialing it (`dial:ws://…`), and every rule of the dial-out table
//! checked against what the host's row has connected and what the runtime
//! says on stderr.
//!
//! A column per language: Python, Go, Node and Bun (Unix only, as the whole
//! Bun runtime is), Swift (macOS, as its plugin binaries are), Kotlin
//! (feature `kotlin`: the SDK's jvm target, the same code an Android
//! application dials with). A language joins by a [`Language`] variant (the
//! command that dials, its project) and a `column!` line. What every dialer
//! must do:
//!
//! - take `dial:<address> --id <id> --peer main <project>`, the token in
//!   `RUTIS_TOKEN`, and shortened waits in
//!   `RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>`;
//! - say on stderr `rutis: connected to …` once greeted,
//!   `rutis: connection lost (…); retrying in <s>s` and
//!   `rutis: cannot connect (…); retrying in <s>s` for a retryable failure,
//!   `rutis: refused: auth-rejected (…); retrying in <s>s`, and
//!   `rutis: refused: incompatible (…); stopping` (then exit non-zero);
//! - give way to a newer connection of its id (4002), and dial again;
//! - stop on SIGTERM: close, exit 0.
//!
//! Risk C10: the matrix is in docs/ci.md. guarantee: bridge/dial-out-rules

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis_bridge::websocket::{self, Limits, ListenerConfig};
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{within, HANG_GUARD};
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The token every dialer here presents, unless a case says otherwise:
/// `RUTIS_TOKEN_PHONE` of every host here.
const TOKEN: &str = "dial-token";

/// Waits short enough for a test (`RUTIS_RETRY`): backoff from 100 ms
/// doubling to 400 ms, reset after 60 s, 300 ms after a rejection. The
/// defaults (500 ms to 30 s, 60 s, 30 s) are each language's unit tests'.
const RETRY: &str = "100,400,60000,300";
const BACKOFF: [f64; 4] = [0.1, 0.2, 0.4, 0.4];

#[derive(Clone, Copy, Debug)]
enum Language {
    Python,
    Node,
    /// Unix only: Bun on Windows is not a declared platform.
    #[cfg(unix)]
    Bun,
    Go,
    /// macOS only, as Swift plugin binaries are.
    #[cfg(target_os = "macos")]
    Swift,
    /// The Kotlin SDK's jvm target (`Rutis.serve` in the conformance jar).
    #[cfg(feature = "kotlin")]
    Kotlin,
}

/// The Kotlin programs (`tests/kotlin`); `kotlin` is the column.
#[cfg(feature = "kotlin")]
#[path = "../../rutis-bridge/tests/kotlin/mod.rs"]
mod kotlin_programs;

/// The conformance plugin as the npm package `marker` in `project`, which
/// the Node and Bun runtimes resolve the row's entry `marker` to.
fn marker_package(project: &Path, fixture: &str) {
    let package = project.join("node_modules/marker");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"marker","version":"1.0.0","type":"module","main":"index.mjs"}"#,
    )
    .unwrap();
    std::fs::copy(repo().join(fixture), package.join("index.mjs")).unwrap();
}

/// The Python interpreter: one with `websockets` (`RUTIS_PYTHON`), else
/// `python3` (`python` on Windows, where setup-python installs no
/// `python3`).
fn python_program() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// The Go conformance dialer (`go/rutis/internal/fixtures/dial`, the
/// plugin `marker` in it), built once per test binary with the `go` on
/// PATH.
fn go_dialer() -> &'static Path {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let binary = directory.join(format!("dial{}", std::env::consts::EXE_SUFFIX));
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./internal/fixtures/dial")
            .current_dir(repo().join("go/rutis"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build ./internal/fixtures/dial");
        binary
    })
}

/// The Swift conformance dialer (the product `RutisDial` of the root
/// `Package.swift`, the plugin `marker` in it), built once per test binary
/// with the `swift` on PATH.
#[cfg(target_os = "macos")]
fn swift_dialer() -> &'static Path {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let status = std::process::Command::new("swift")
            .args(["build", "--product", "RutisDial"])
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build --product RutisDial");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).join("RutisDial")
    })
}

impl Language {
    /// The command that starts this language's runtime dialing `address`
    /// as `id`, its project in `project` with the conformance plugin
    /// `marker` in it.
    fn dialer(self, address: &str, id: &str, project: &Path) -> tokio::process::Command {
        match self {
            Language::Python => {
                std::fs::copy(
                    repo().join("python/rutis/tests/conformance_dial.py"),
                    project.join("marker.py"),
                )
                .unwrap();
                let mut command = tokio::process::Command::new(python_program());
                command
                    .args(["-m", "rutis"])
                    .arg(format!("dial:{address}"))
                    .args(["--id", id, "--peer", "main"])
                    .arg(project)
                    .env("PYTHONPATH", repo().join("python/rutis"));
                command
            }
            Language::Node => {
                marker_package(
                    project,
                    "node/rutis-runtime/test/fixtures/conformance-dial.mjs",
                );
                let anchor = project.join("package.json");
                std::fs::write(&anchor, "{}").unwrap();
                let runtime = repo().join("node/rutis-runtime");
                let mut command = tokio::process::Command::new("node");
                command
                    .arg(runtime.join("src/runner.mjs"))
                    .arg(format!("dial:{address}"))
                    .args(["--id", id, "--peer", "main"])
                    .arg(anchor)
                    .current_dir(runtime);
                command
            }
            #[cfg(unix)]
            Language::Bun => {
                marker_package(project, "bun/rutis-bun/test/fixtures/conformance-dial.ts");
                let mut command = tokio::process::Command::new("bun");
                command
                    .args(["--no-install", "--no-env-file"])
                    .arg(repo().join("bun/rutis-bun/src/main.ts"))
                    .arg(format!("dial:{address}"))
                    .args(["--id", id, "--peer", "main"])
                    .arg(project)
                    .current_dir(project);
                command
            }
            Language::Go => {
                let mut command = tokio::process::Command::new(go_dialer());
                command
                    .arg(format!("dial:{address}"))
                    .args(["--id", id, "--peer", "main"])
                    .arg(project);
                command
            }
            #[cfg(target_os = "macos")]
            Language::Swift => {
                let mut command = tokio::process::Command::new(swift_dialer());
                command
                    .arg(format!("dial:{address}"))
                    .args(["--id", id, "--peer", "main"])
                    .arg(project);
                command
            }
            #[cfg(feature = "kotlin")]
            Language::Kotlin => {
                let mut command = tokio::process::Command::new("java");
                command
                    .args(kotlin_programs::JVM_OPTIONS)
                    .arg("-jar")
                    .arg(kotlin_programs::jar())
                    .arg(format!("dial:{address}"))
                    .args(["--id", id, "--peer", "main"])
                    .arg(project);
                command
            }
        }
    }
}

/// A host `main` with the listener `public` on loopback and the listen row
/// `phone` on it, its token [`TOKEN`].
struct Rutis {
    host: Host,
    port: u16,
}

impl Rutis {
    /// Listening on `port` (0: any free one).
    async fn start(port: u16) -> Self {
        // Every host here has this token: set by any test, it is the same.
        std::env::set_var("RUTIS_TOKEN_PHONE", TOKEN);
        let config: HostConfig = serde_json::from_value(json!({
            "id": "main",
            "listen": [{ "name": "public", "address": format!("127.0.0.1:{port}") }],
            "rows": [{ "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }]
        }))
        .unwrap();
        let host = Host::start(&config).await.unwrap();
        host.load(config.rows()).await.unwrap();
        let port = host.listening[0].1.port();
        Self { host, port }
    }

    fn address(&self) -> String {
        format!("ws://127.0.0.1:{}/rutis", self.port)
    }

    /// The far ends `phone` has connected now.
    fn connected(&self) -> Vec<String> {
        self.host
            .connections()
            .into_iter()
            .filter(|row| row.id == "phone")
            .flat_map(|row| row.connected)
            .map(|connected| connected.id)
            .collect()
    }

    /// Until `phone` has connected exactly `ids`.
    async fn until_connected(&self, ids: &[&str]) {
        rutis_test_support::until(|| self.connected() == ids, &format!("connected {ids:?}")).await;
    }

    /// Goes away: its rows, then its listener.
    async fn stop(self) {
        self.host.shut_down(HANG_GUARD).await.ok();
    }
}

/// A started dialer, killed when dropped, and what it says on stderr.
struct Dialer {
    child: tokio::process::Child,
    lines: mpsc::UnboundedReceiver<String>,
    /// Lines read and not yet expected.
    unread: VecDeque<String>,
    _project: tempfile::TempDir,
}

impl Dialer {
    fn start(language: Language, address: &str, who: &str, token: &str) -> Self {
        let project = tempfile::tempdir().unwrap();
        let mut child = language
            .dialer(address, who, project.path())
            .env("RUTIS_TOKEN", token)
            .env("RUTIS_RETRY", RETRY)
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap());
        let (sender, lines) = mpsc::unbounded_channel();
        let who = who.to_owned();
        tokio::spawn(async move {
            // Bytes, not UTF-8 lines: a line that is not UTF-8 (an OS error
            // in a Windows code page) must not end the reading.
            let mut line = Vec::new();
            while let Ok(read) = stderr.read_until(b'\n', &mut line).await {
                if read == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                line.clear();
                // For a failure's record.
                eprintln!("[{who}] {text}");
                let _ = sender.send(text);
            }
        });
        Self {
            child,
            lines,
            unread: VecDeque::new(),
            _project: project,
        }
    }

    /// The next line, in the order said, that starts with `prefix`; lines
    /// before it stay for later expectations.
    async fn expect(&mut self, prefix: &str) -> String {
        if let Some(at) = self.unread.iter().position(|line| line.starts_with(prefix)) {
            return self.unread.remove(at).unwrap();
        }
        loop {
            let line = within(prefix, self.lines.recv())
                .await
                .unwrap_or_else(|| panic!("the dialer ended without saying {prefix:?}"));
            if line.starts_with(prefix) {
                return line;
            }
            self.unread.push_back(line);
        }
    }

    async fn exit(&mut self) -> std::process::ExitStatus {
        within("the dialer's exit", self.child.wait())
            .await
            .unwrap()
    }

    fn running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

/// The wait a `…; retrying in <s>s` line announces, in seconds.
fn retrying_in(line: &str) -> f64 {
    let (_, wait) = line
        .rsplit_once("; retrying in ")
        .unwrap_or_else(|| panic!("no wait in {line:?}"));
    wait.trim_end_matches('s').parse().unwrap()
}

/// Connected under the id it chose.
async fn connects_with_its_id(language: Language) {
    let rutis = Rutis::start(0).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    let line = pixel.expect("rutis: connected to ").await;
    assert!(line.contains(&rutis.address()), "{line}");
    rutis.until_connected(&["pixel"]).await;
}

/// Credentials refused at the upgrade (403): an authentication failure,
/// waited out and retried, never given up.
async fn a_wrong_token_is_retried_as_rejected(language: Language) {
    let rutis = Rutis::start(0).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", "guess");
    for _ in 0..2 {
        let line = pixel.expect("rutis: refused: auth-rejected (").await;
        assert!(line.contains("403"), "{line}");
        assert!(!line.contains("guess"), "the token is not said: {line}");
        assert!((retrying_in(&line) - 0.3).abs() < 0.05, "{line}");
    }
    assert!(pixel.running());
    assert!(rutis.connected().is_empty());
}

/// A listener speaking another session protocol (400 at the upgrade): the
/// dialer stops and reports, exiting non-zero.
async fn an_incompatible_listener_stops_the_dialer(language: Language) {
    // Takes the token, speaking only `rutis.9`.
    let listener = websocket::listen(
        &ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap()),
        &Limits::default(),
        Arc::new(|upgrade: websocket::Upgrade<'_>| {
            if upgrade.bearer != Some(TOKEN) {
                return Err(websocket::Refused::Forbidden("not accepted here".into()));
            }
            Ok(websocket::Admitted {
                protocol: "rutis.9".into(),
                deliver: Box::new(|_| {}),
            })
        }),
    )
    .unwrap();
    let address = format!("ws://{}/rutis", listener.local_addr());
    let mut pixel = Dialer::start(language, &address, "pixel", TOKEN);
    let line = pixel.expect("rutis: refused: incompatible (").await;
    assert!(line.contains("400"), "{line}");
    assert!(line.ends_with("; stopping"), "{line}");
    assert!(!pixel.exit().await.success());
}

/// An id no listener could take (`[a-z0-9-]+`) is not dialed with:
/// retrying cannot fix it.
async fn an_invalid_id_is_refused_before_dialing(language: Language) {
    let rutis = Rutis::start(0).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "Pixel 7", TOKEN);
    assert!(!pixel.exit().await.success());
    assert!(rutis.connected().is_empty());
}

/// The host going away: the dialer backs off (doubling, capped), and
/// connects again once it is back, as a new session.
async fn it_backs_off_and_reconnects_as_a_new_session(language: Language) {
    let rutis = Rutis::start(0).await;
    let port = rutis.port;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    pixel.expect("rutis: connected to ").await;
    rutis.until_connected(&["pixel"]).await;
    rutis.stop().await;
    let mut waits = vec![retrying_in(&pixel.expect("rutis: connection lost (").await)];
    while waits.len() < BACKOFF.len() {
        waits.push(retrying_in(&pixel.expect("rutis: cannot connect (").await));
    }
    for (wait, base) in waits.iter().zip(BACKOFF) {
        // ± 20 % jitter, said to a tenth of a second.
        assert!(
            (base * 0.8 - 0.05..=base * 1.2 + 0.05).contains(wait),
            "{waits:?} is not a backoff of {BACKOFF:?}"
        );
    }

    let rutis = Rutis::start(port).await;
    pixel.expect("rutis: connected to ").await;
    rutis.until_connected(&["pixel"]).await;
}

/// A second dialer with the same id takes over: the first is replaced
/// (4002) and dials again by its backoff, taking over in turn. Two live
/// dialers with one id keep displacing each other at the pace of their
/// backoffs: ids must be unique, which nothing but the deployment ensures
/// while ids are self-reported.
async fn the_same_id_takes_over(language: Language) {
    let rutis = Rutis::start(0).await;
    let mut first = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    first.expect("rutis: connected to ").await;
    rutis.until_connected(&["pixel"]).await;

    let mut second = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    second.expect("rutis: connected to ").await;
    let line = first.expect("rutis: connection lost (").await;
    assert!(line.contains("replaced by a new connection"), "{line}");
    // The first dials again and takes over in turn.
    first.expect("rutis: connected to ").await;
    let line = second.expect("rutis: connection lost (").await;
    assert!(line.contains("replaced by a new connection"), "{line}");
    // Installed only once the replaced one closed with 4002 (listen's
    // order): one `pixel` connected, after that close.
    rutis.until_connected(&["pixel"]).await;
}

/// Stopped by its own side (SIGTERM): the connection closes, the process
/// ends with success and dials no more.
#[cfg(unix)]
async fn stopping_ends_without_reconnecting(language: Language) {
    let rutis = Rutis::start(0).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    pixel.expect("rutis: connected to ").await;
    rutis.until_connected(&["pixel"]).await;
    let pid = pixel.child.id().unwrap().to_string();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(pixel.exit().await.success());
    rutis.until_connected(&[]).await;
    while let Some(line) = pixel.lines.recv().await {
        assert!(!line.starts_with("rutis: "), "said after stopping: {line}");
    }
}

/// One language's column: every case, as `<language>::<case>`.
macro_rules! column {
    ($column:ident, $language:expr) => {
        mod $column {
            use super::*;

            #[tokio::test(flavor = "multi_thread")]
            async fn connects_with_its_id() {
                super::connects_with_its_id($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn a_wrong_token_is_retried_as_rejected() {
                super::a_wrong_token_is_retried_as_rejected($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn an_incompatible_listener_stops_the_dialer() {
                super::an_incompatible_listener_stops_the_dialer($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn an_invalid_id_is_refused_before_dialing() {
                super::an_invalid_id_is_refused_before_dialing($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn it_backs_off_and_reconnects_as_a_new_session() {
                super::it_backs_off_and_reconnects_as_a_new_session($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn the_same_id_takes_over() {
                super::the_same_id_takes_over($language).await;
            }

            #[cfg(unix)]
            #[tokio::test(flavor = "multi_thread")]
            async fn stopping_ends_without_reconnecting() {
                super::stopping_ends_without_reconnecting($language).await;
            }
        }
    };
}

column!(python, Language::Python);
column!(node, Language::Node);
#[cfg(unix)]
column!(bun, Language::Bun);
column!(go, Language::Go);
#[cfg(target_os = "macos")]
column!(swift, Language::Swift);
#[cfg(feature = "kotlin")]
column!(kotlin, Language::Kotlin);
