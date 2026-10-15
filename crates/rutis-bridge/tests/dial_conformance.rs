//! The dial-out conformance suite (`docs/design-remote-runtime-dial-2026-10-14.md`
//! §六): rutis runs an access point (a shared token over a loopback
//! WebSocket), each language's runtime dials it out (`dial:ws://…`), and
//! every rule of the dial-out table is checked against what the access
//! point observes and what the runtime says on stderr.
//!
//! A column per language: Python, Go, Node and Bun (Unix only, as the whole
//! Bun runtime is); Swift and Kotlin with their SDKs (Swift A1, Android
//! K-P2). A language joins by a
//! [`Language`] variant (the command that dials, the conformance plugin in
//! its project) and a `column!` line. What every dialer must do:
//!
//! - take `dial:<address> --id <id> --peer main <project>`, the token in
//!   `RUTIS_TOKEN`, and shortened waits in
//!   `RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>`;
//! - say on stderr `rutis: connected to …` once greeted,
//!   `rutis: connection lost (…); retrying in <s>s` and
//!   `rutis: cannot connect (…); retrying in <s>s` for a retryable failure,
//!   `rutis: refused: auth-rejected (…); retrying in <s>s`, and
//!   `rutis: refused: incompatible (…); stopping` (then exit non-zero);
//! - stop on SIGTERM: unload every row, close, exit 0;
//! - serve the plugin `marker`, which says `conformance: loaded <config>`
//!   and `conformance: unloaded <config>` on stderr.
//!
//! Risk C10: the matrix is in docs/ci.md. guarantee: bridge/dial-out-rules
#![cfg(all(feature = "websocket", feature = "testing"))]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis::{BoxFuture, Ctx, FiberView};
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::Value;
use rutis_bridge::transport::websocket::{
    Config, ListenerConfig, WebSocketPlugin, WebSocketTransport,
};
use rutis_bridge::{
    Accept, Accepted, AccessEvent, AccessPointConfig, AccessPointPlugin, Admission,
    AdmissionPlugin, Presented, Registration, Rejection, Release, SharedToken, StaticIdentity,
    Transport,
};
use rutis_test_support::{within, HANG_GUARD};
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{broadcast, mpsc};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The token every dialer here presents, unless a case says otherwise.
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
    #[cfg(all(unix, feature = "bun"))]
    Bun,
    #[cfg(feature = "go")]
    Go,
}

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
#[cfg(feature = "go")]
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
            #[cfg(all(unix, feature = "bun"))]
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
            #[cfg(feature = "go")]
            Language::Go => {
                let mut command = tokio::process::Command::new(go_dialer());
                command
                    .arg(format!("dial:{address}"))
                    .args(["--id", id, "--peer", "main"])
                    .arg(project);
                command
            }
        }
    }
}

/// Mounts each connection as a runtime and loads the row `marker` on it,
/// its config naming the connection: `<id> <generation>`.
struct Rows;

impl Accept for Rows {
    fn accept(&self, connection: Accepted) -> BoxFuture<'_, Result<Release, String>> {
        Box::pin(async move {
            let session = connection.runtime_session().connection();
            let config = format!("{} {}", connection.id(), connection.peer().generation());
            session
                .invoke_async("", "mount", Value::Data(json!({ "emits": [] })))
                .await
                .map_err(|error| format!("mount: {error}"))?;
            session
                .invoke_async(
                    "",
                    "rows.load",
                    Value::Data(json!(["marker", "marker", config, [], []])),
                )
                .await
                .map_err(|error| format!("rows.load: {error}"))?;
            let release: Release = Box::new(|| Box::pin(async {}));
            Ok(release)
        })
    }
}

/// Admits whoever presents [`TOKEN`], then rejects every id it greets with.
struct Refusing(PeerId);

impl Admission for Refusing {
    fn local(&self) -> &PeerId {
        &self.0
    }
    fn verify(&self, presented: Presented<'_>) -> bool {
        matches!(presented, Presented::Bearer(token) if token == TOKEN)
    }
    fn admit(&self, reported: &PeerId) -> Result<PeerId, Rejection> {
        Err(Rejection(format!("{reported} is not known")))
    }
}

/// rutis, `main`: a loopback WebSocket listener and the access point
/// `phone` on it.
struct Rutis {
    websocket: FiberView,
    access: FiberView,
    events: broadcast::Receiver<AccessEvent>,
    port: u16,
    _root: Ctx,
}

impl Rutis {
    /// Listening on `port` (0: any free one).
    async fn start(port: u16, admission: impl Admission) -> Self {
        let root = Ctx::root().unwrap();
        let plugin = WebSocketPlugin::new(Config::new().listener(ListenerConfig::new(
            "public",
            ([127, 0, 0, 1], port).into(),
            id("main"),
        )))
        .unwrap();
        let handle = plugin.clone();
        let websocket = root.plugin(plugin);
        (&websocket).await.unwrap();
        let port = handle
            .transport()
            .unwrap()
            .local_addr("public")
            .unwrap()
            .port();
        (&root.plugin(AdmissionPlugin::new("phone", admission)))
            .await
            .unwrap();
        let plugin = AccessPointPlugin::new(
            AccessPointConfig::new(id("phone"), "websocket", "phone", "public")
                .handshake(HANG_GUARD)
                .require("runtime"),
        )
        .accept(Rows);
        let events = plugin.events();
        let access = root.plugin(plugin);
        (&access).await.unwrap();
        Self {
            websocket,
            access,
            events,
            port,
            _root: root,
        }
    }

    fn address(&self) -> String {
        format!("ws://127.0.0.1:{}/rutis", self.port)
    }

    async fn next(&mut self) -> AccessEvent {
        within("an access point event", self.events.recv())
            .await
            .unwrap()
    }

    async fn connected(&mut self, who: &str) -> u64 {
        match self.next().await {
            AccessEvent::Connected {
                id: again,
                generation,
            } if again == id(who) => generation,
            other => panic!("expected {who} to connect: {other:?}"),
        }
    }

    /// Goes away: the access point, then the listener.
    async fn stop(self) {
        self.access.dispose().await.unwrap();
        self.websocket.dispose().await.unwrap();
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

/// Accepted under the id it chose: the access point's connection, and a
/// session with its rows loaded.
async fn connects_and_is_accepted_with_its_id(language: Language) {
    let mut rutis = Rutis::start(0, SharedToken::new(id("main"), TOKEN)).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    let generation = rutis.connected("pixel").await;
    let line = pixel.expect("rutis: connected to ").await;
    assert!(line.contains(&rutis.address()), "{line}");
    pixel
        .expect(&format!("conformance: loaded pixel {generation}"))
        .await;
}

/// Credentials refused at the upgrade (403): an authentication failure,
/// waited out and retried, never given up.
async fn a_wrong_token_is_retried_as_rejected(language: Language) {
    let rutis = Rutis::start(0, SharedToken::new(id("main"), TOKEN)).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", "guess");
    for _ in 0..2 {
        let line = pixel.expect("rutis: refused: auth-rejected (").await;
        assert!(line.contains("403"), "{line}");
        assert!(!line.contains("guess"), "the token is not said: {line}");
        assert!((retrying_in(&line) - 0.3).abs() < 0.05, "{line}");
    }
    assert!(pixel.running());
}

/// The admission rejecting the id after the hello (close 4003): an
/// authentication failure too.
async fn a_rejected_id_is_retried_as_rejected(language: Language) {
    let mut rutis = Rutis::start(0, Refusing(id("main"))).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    for _ in 0..2 {
        let event = rutis.next().await;
        assert!(
            matches!(&event, AccessEvent::Rejected { reason } if reason.contains("pixel is not known")),
            "{event:?}"
        );
        let line = pixel.expect("rutis: refused: auth-rejected (").await;
        assert!(line.contains("pixel is not known"), "{line}");
    }
    assert!(pixel.running());
}

/// An access point speaking another session protocol (400 at the
/// upgrade): the dialer stops and reports, exiting non-zero.
async fn an_incompatible_access_point_stops_the_dialer(language: Language) {
    let transport = WebSocketTransport::start(Config::new().listener(ListenerConfig::new(
        "public",
        "127.0.0.1:0".parse().unwrap(),
        id("main"),
    )))
    .unwrap();
    let _registered = transport
        .register(Registration {
            listener: "public".into(),
            peer: id("phone"),
            identity: Arc::new(StaticIdentity::new(id("main")).accept_token(TOKEN, id("phone"))),
            protocol: "rutis.9".into(),
            deliver: Box::new(|_| {}),
        })
        .unwrap();
    let address = format!("ws://{}/rutis", transport.local_addr("public").unwrap());
    let mut pixel = Dialer::start(language, &address, "pixel", TOKEN);
    let line = pixel.expect("rutis: refused: incompatible (").await;
    assert!(line.contains("400"), "{line}");
    assert!(line.ends_with("; stopping"), "{line}");
    assert!(!pixel.exit().await.success());
}

/// An id no access point could admit (`[a-z0-9-]+`) is not dialed with:
/// retrying cannot fix it.
async fn an_invalid_id_is_refused_before_dialing(language: Language) {
    let rutis = Rutis::start(0, SharedToken::new(id("main"), TOKEN)).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "Pixel 7", TOKEN);
    assert!(!pixel.exit().await.success());
}

/// The access point going away: the rows go, the dialer backs off
/// (doubling, capped), and connects again once it is back, as a new
/// session that loads its rows afresh.
async fn it_backs_off_and_reconnects_as_a_new_session(language: Language) {
    let mut rutis = Rutis::start(0, SharedToken::new(id("main"), TOKEN)).await;
    let port = rutis.port;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    let generation = rutis.connected("pixel").await;
    pixel
        .expect(&format!("conformance: loaded pixel {generation}"))
        .await;
    rutis.stop().await;
    pixel
        .expect(&format!("conformance: unloaded pixel {generation}"))
        .await;
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

    let mut rutis = Rutis::start(port, SharedToken::new(id("main"), TOKEN)).await;
    let generation = rutis.connected("pixel").await;
    pixel.expect("rutis: connected to ").await;
    pixel
        .expect(&format!("conformance: loaded pixel {generation}"))
        .await;
}

/// A second dialer with the same id takes over: the first is replaced,
/// its rows go, and it dials again by its backoff, taking over in turn.
/// Two live dialers with one id keep displacing each other at the pace of
/// their backoffs (up to every 30 s each, as a session shorter than 60 s
/// does not reset it): ids must be unique, which nothing but the
/// deployment ensures while ids are self-reported (design §五).
async fn the_same_id_takes_over(language: Language) {
    let mut rutis = Rutis::start(0, SharedToken::new(id("main"), TOKEN)).await;
    let mut first = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    let one = rutis.connected("pixel").await;
    first
        .expect(&format!("conformance: loaded pixel {one}"))
        .await;

    let mut second = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    assert_eq!(
        rutis.next().await,
        AccessEvent::Replaced {
            id: id("pixel"),
            generation: one
        }
    );
    let two = rutis.connected("pixel").await;
    second
        .expect(&format!("conformance: loaded pixel {two}"))
        .await;
    let line = first.expect("rutis: connection lost (").await;
    assert!(line.contains("replaced by a new connection"), "{line}");
    first
        .expect(&format!("conformance: unloaded pixel {one}"))
        .await;

    assert_eq!(
        rutis.next().await,
        AccessEvent::Replaced {
            id: id("pixel"),
            generation: two
        }
    );
    let three = rutis.connected("pixel").await;
    first
        .expect(&format!("conformance: loaded pixel {three}"))
        .await;
    let line = second.expect("rutis: connection lost (").await;
    assert!(line.contains("replaced by a new connection"), "{line}");
}

/// Stopped by its own side (SIGTERM): the rows go, the connection closes,
/// the process ends with success and dials no more.
#[cfg(unix)]
async fn stopping_unloads_and_ends_without_reconnecting(language: Language) {
    let mut rutis = Rutis::start(0, SharedToken::new(id("main"), TOKEN)).await;
    let mut pixel = Dialer::start(language, &rutis.address(), "pixel", TOKEN);
    let generation = rutis.connected("pixel").await;
    pixel
        .expect(&format!("conformance: loaded pixel {generation}"))
        .await;
    let pid = pixel.child.id().unwrap().to_string();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(status.success());
    pixel
        .expect(&format!("conformance: unloaded pixel {generation}"))
        .await;
    assert!(pixel.exit().await.success());
    let event = rutis.next().await;
    assert!(
        matches!(&event, AccessEvent::Disconnected { generation: g, .. } if *g == generation),
        "{event:?}"
    );
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
            async fn connects_and_is_accepted_with_its_id() {
                super::connects_and_is_accepted_with_its_id($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn a_wrong_token_is_retried_as_rejected() {
                super::a_wrong_token_is_retried_as_rejected($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn a_rejected_id_is_retried_as_rejected() {
                super::a_rejected_id_is_retried_as_rejected($language).await;
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn an_incompatible_access_point_stops_the_dialer() {
                super::an_incompatible_access_point_stops_the_dialer($language).await;
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
            async fn stopping_unloads_and_ends_without_reconnecting() {
                super::stopping_unloads_and_ends_without_reconnecting($language).await;
            }
        }
    };
}

column!(python, Language::Python);
column!(node, Language::Node);
#[cfg(all(unix, feature = "bun"))]
column!(bun, Language::Bun);
#[cfg(feature = "go")]
column!(go, Language::Go);
// Swift and Kotlin: with their SDKs (Swift A1, Android K-P2).
