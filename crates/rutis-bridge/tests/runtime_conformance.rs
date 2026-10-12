//! The session conformance suite against every implementation of the
//! session: Rust, Node, Python, Go and Bun, each serving `conformance` as
//! endpoint of an endpoint-format session, on each channel it can take: a
//! Unix socket path, a loopback TCP address it presents a token on (how
//! runtimes are started on Windows), and a loopback WebSocket it listens on. Risk C10: the matrix, with
//! every skipped cell's reason, is in docs/ci.md.
#![cfg(feature = "testing")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis_bridge::channel::{Channel, PeerId};
use rutis_bridge::session::testing::{malformed, session, Fixture};
use rutis_bridge::session::{Connection, Endpoint, Format};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// No calls into `main`: the suite only calls the far end.
struct Nothing;
impl rutis_bridge::session::Dispatch for Nothing {
    fn invoke(
        &self,
        _: &Connection,
        _: &str,
        _: &str,
        _: rutis_bridge::session::Value,
    ) -> rutis_bridge::session::Reply {
        Err(rutis_bridge::session::Error::Value(
            "main serves nothing".into(),
        ))
    }
}

fn main_endpoint(far: &str) -> Format {
    Format::Endpoint(Endpoint::rust(id("main")).expect(id(far)))
}

async fn run(far: Connection) {
    far.ready().await.unwrap();
    tokio::task::spawn_blocking(move || session(&far))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_meets_the_session_contract() {
    let (a, b) = rutis_bridge::transport::memory::pair();
    let _far = Connection::open_with(
        b,
        Arc::new(Fixture::default()),
        Format::Endpoint(Endpoint::rust(id("rust")).expect(id("main"))),
    )
    .unwrap();
    run(Connection::open_with(a, Arc::new(Nothing), main_endpoint("rust")).unwrap()).await;
}

/// The channel to the far end: one it dials, framed by the local transport,
/// or a WebSocket it listens on.
#[derive(Clone, Copy, Debug)]
enum Via {
    /// A Unix socket path. Not on Windows: see [`NO_UNIX_SOCKET`].
    UnixSocket,
    /// A loopback TCP address (`tcp:127.0.0.1:<port>`); the far end sends
    /// the token in `RUTIS_CHANNEL_TOKEN` and a newline first.
    Loopback,
    /// `listen:ws://127.0.0.1:0/rutis`: the far end listens, announces its
    /// address on stderr, and takes a dial presenting `RUTIS_TOKEN`; it
    /// greets as the endpoint named here.
    #[cfg(feature = "websocket")]
    WebSocket(&'static str),
}

/// Why the Unix socket column does not run on Windows (Q7.8). Kept in step
/// with the `ignore` reasons below, which must be literals.
#[cfg(not(unix))]
const NO_UNIX_SOCKET: &str = "Unix socket column: Windows runtimes connect on loopback TCP";

/// The token the far end presents on a loopback channel.
const TOKEN: &str = "conformance-token";

/// A started far end: killed when dropped, with what its channel needs kept
/// alive as long (the WebSocket transport's threads).
#[allow(dead_code)]
struct Child(tokio::process::Child, Option<Box<dyn std::any::Any + Send>>);

/// Shuts the stream down: wakes the channel's reader and writer.
struct Shut(Box<dyn Fn() + Send + Sync>);
impl rutis_bridge::channel::Closer for Shut {
    fn close(&self, _: &str) {
        (self.0)()
    }
}

/// Start `command` on `via`, the address as its last argument, and take
/// the channel. Fails if the process ends before it connects.
async fn connect(mut command: tokio::process::Command, via: Via) -> (Channel, Child) {
    command.kill_on_drop(true);
    match via {
        #[cfg(unix)]
        Via::UnixSocket => {
            let directory = tempfile::tempdir().unwrap();
            let socket = directory.path().join("conformance.sock");
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let mut child = command.arg(&socket).spawn().unwrap();
            let stream = tokio::select! {
                accepted = listener.accept() => accepted.unwrap().0,
                status = child.wait() => panic!("the far end ended before connecting: {status:?}"),
            };
            let stream = stream.into_std().unwrap();
            stream.set_nonblocking(false).unwrap();
            let reader = stream.try_clone().unwrap();
            let shut = stream.try_clone().unwrap();
            let closer = Arc::new(Shut(Box::new(move || {
                let _ = shut.shutdown(std::net::Shutdown::Both);
            })));
            (
                rutis_bridge::transport::local::framed(reader, stream, closer),
                Child(child, None),
            )
        }
        #[cfg(not(unix))]
        Via::UnixSocket => unreachable!("{NO_UNIX_SOCKET}"),
        Via::Loopback => {
            use tokio::io::AsyncReadExt;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let mut child = command
                .arg(format!("tcp:{address}"))
                .env("RUTIS_CHANNEL_TOKEN", TOKEN)
                .spawn()
                .unwrap();
            let mut stream = tokio::select! {
                accepted = listener.accept() => accepted.unwrap().0,
                status = child.wait() => panic!("the far end ended before connecting: {status:?}"),
            };
            let mut presented = vec![0; TOKEN.len() + 1];
            stream.read_exact(&mut presented).await.unwrap();
            assert_eq!(
                presented,
                format!("{TOKEN}\n").as_bytes(),
                "the token first"
            );
            let stream = stream.into_std().unwrap();
            stream.set_nonblocking(false).unwrap();
            stream.set_nodelay(true).unwrap();
            let reader = stream.try_clone().unwrap();
            let shut = stream.try_clone().unwrap();
            let closer = Arc::new(Shut(Box::new(move || {
                let _ = shut.shutdown(std::net::Shutdown::Both);
            })));
            (
                rutis_bridge::transport::local::framed(reader, stream, closer),
                Child(child, None),
            )
        }
        #[cfg(feature = "websocket")]
        Via::WebSocket(far) => listening(command, far).await,
    }
}

/// Start `command` listening on a loopback WebSocket, and dial it.
#[cfg(feature = "websocket")]
async fn listening(mut command: tokio::process::Command, far: &str) -> (Channel, Child) {
    use rutis_bridge::transport::websocket::{Config, WebSocketTransport};
    use rutis_bridge::{Credential, Dial, Identity, StaticIdentity, Transport};
    use tokio::io::AsyncBufReadExt;

    let mut child = command
        .arg("listen:ws://127.0.0.1:0/rutis")
        .env("RUTIS_TOKEN", TOKEN)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = loop {
        let line = tokio::select! {
            line = lines.next_line() => line.unwrap(),
            status = child.wait() => panic!("the far end ended before listening: {status:?}"),
        };
        let line = line.expect("the far end's address");
        match line.strip_prefix("rutis: listening on ") {
            Some(address) => break address.to_owned(),
            None => eprintln!("{line}"),
        }
    };
    // Pass the rest of its stderr on, for a failure's record.
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("{line}");
        }
    });
    let transport = WebSocketTransport::start(Config::new()).unwrap();

    let far = id(far);
    let identity: Arc<dyn Identity> = Arc::new(
        StaticIdentity::new(id("main")).present(far.clone(), Credential::Bearer(TOKEN.into())),
    );
    let channel = transport
        .dial(
            &Dial::address(address)
                .peer(far)
                .identity(identity)
                .protocol(format!(
                    "rutis.{}",
                    rutis_bridge::session::ENDPOINT_PROTOCOL
                )),
        )
        .await
        .unwrap();
    (channel, Child(child, Some(Box::new(transport))))
}

fn node() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("node");
    command
        .args(["--import", "tsx"])
        .arg(repo().join("node/rutis-runtime/test/fixtures/conformance-session.mjs"))
        .current_dir(repo().join("node/rutis-runtime"));
    command
}

/// The Python interpreter: one with `websockets` for the WebSocket column
/// (`RUTIS_PYTHON`), else `python3` (`python` on Windows, where
/// setup-python installs no `python3`).
fn python() -> tokio::process::Command {
    let program = std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into());
    let mut command = tokio::process::Command::new(program);
    command
        .arg(repo().join("python/rutis/tests/conformance_session.py"))
        .env("PYTHONPATH", repo().join("python/rutis"));
    command
}

/// The Go session fixture (`go/rutis/internal/fixtures/session`), built
/// once per test binary with the `go` on PATH.
#[cfg(feature = "go")]
fn go() -> tokio::process::Command {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let binary = BINARY.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let binary = directory.join(format!("conformance{}", std::env::consts::EXE_SUFFIX));
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./internal/fixtures/session")
            .current_dir(repo().join("go/rutis"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build ./internal/fixtures/session");
        binary
    });
    tokio::process::Command::new(binary)
}

/// Bun: Unix only, as the whole Bun runtime is (Bun on Windows is not a
/// declared platform; docs/ci.md).
#[cfg(all(unix, feature = "bun"))]
fn bun() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("bun");
    command
        .args(["--no-install", "--no-env-file"])
        .arg(repo().join("bun/rutis-bun/test/fixtures/conformance-session.ts"))
        .current_dir(repo().join("bun/rutis-bun"));
    command
}

/// The session contract against `command`'s implementation, on `via`.
async fn contract(command: tokio::process::Command, via: Via, far: &str) {
    let (channel, _child) = connect(command, via).await;
    run(Connection::open_with(channel, Arc::new(Nothing), main_endpoint(far)).unwrap()).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "Unix socket column: Windows runtimes connect on loopback TCP"
)]
async fn node_meets_the_session_contract_on_a_unix_socket() {
    contract(node(), Via::UnixSocket, "node").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn node_meets_the_session_contract_on_loopback() {
    contract(node(), Via::Loopback, "node").await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "Unix socket column: Windows runtimes connect on loopback TCP"
)]
async fn python_meets_the_session_contract_on_a_unix_socket() {
    contract(python(), Via::UnixSocket, "python").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_meets_the_session_contract_on_loopback() {
    contract(python(), Via::Loopback, "python").await;
}

#[cfg(feature = "websocket")]
#[tokio::test(flavor = "multi_thread")]
async fn node_meets_the_session_contract_on_a_websocket() {
    contract(node(), Via::WebSocket("node"), "node").await;
}

#[cfg(feature = "websocket")]
#[tokio::test(flavor = "multi_thread")]
async fn python_meets_the_session_contract_on_a_websocket() {
    contract(python(), Via::WebSocket("python"), "python").await;
}

// No Go cell on a WebSocket: the Go SDK's WebSocket listener is internal to
// the runtime (package rutis, `listen.go`), and the session fixture is a
// separate program; the Go runtime's WebSocket column is in
// session_matrix.rs. No Bun cell: the Bun runtime has no WebSocket channel.

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "Unix socket column: Windows runtimes connect on loopback TCP"
)]
async fn go_meets_the_session_contract_on_a_unix_socket() {
    contract(go(), Via::UnixSocket, "go").await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_meets_the_session_contract_on_loopback() {
    contract(go(), Via::Loopback, "go").await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_meets_the_session_contract_on_a_unix_socket() {
    contract(bun(), Via::UnixSocket, "bun").await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_meets_the_session_contract_on_loopback() {
    contract(bun(), Via::Loopback, "bun").await;
}

/// Far ends started one per channel by `command`, on `via`. Dropped, it
/// kills them all.
struct Started {
    command: fn() -> tokio::process::Command,
    via: Via,
    children: std::sync::Mutex<Vec<Child>>,
}
impl Started {
    fn new(command: fn() -> tokio::process::Command, via: Via) -> Self {
        Self {
            command,
            via,
            children: Default::default(),
        }
    }

    /// A fresh channel to a fresh far end. Called from a blocking thread.
    fn open(&self) -> Channel {
        let (channel, child) =
            tokio::runtime::Handle::current().block_on(connect((self.command)(), self.via));
        self.children.lock().unwrap().push(child);
        channel
    }
}

/// Q6.2.2, Q6.2.3, risk P3: every implementation ends the session on the
/// same malformed and dangling frames.
#[tokio::test(flavor = "multi_thread")]
async fn rust_ends_the_session_on_malformed_frames() {
    tokio::task::spawn_blocking(|| {
        let kept = std::sync::Mutex::new(Vec::new());
        malformed(|| {
            let (a, b) = rutis_bridge::transport::memory::pair();
            let far = Connection::open_with(
                b,
                Arc::new(Fixture::default()),
                Format::Endpoint(Endpoint::rust(id("rust")).expect(id("main"))),
            )
            .unwrap();
            kept.lock().unwrap().push(far);
            a
        })
    })
    .await
    .unwrap();
}

/// The malformed-frame checks against `command`'s implementation, on `via`.
async fn ends_on_malformed(command: fn() -> tokio::process::Command, via: Via) {
    tokio::task::spawn_blocking(move || {
        let started = Started::new(command, via);
        malformed(|| started.open())
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "Unix socket column: Windows runtimes connect on loopback TCP"
)]
async fn node_ends_the_session_on_malformed_frames_on_a_unix_socket() {
    ends_on_malformed(node, Via::UnixSocket).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn node_ends_the_session_on_malformed_frames_on_loopback() {
    ends_on_malformed(node, Via::Loopback).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "Unix socket column: Windows runtimes connect on loopback TCP"
)]
async fn python_ends_the_session_on_malformed_frames_on_a_unix_socket() {
    ends_on_malformed(python, Via::UnixSocket).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_ends_the_session_on_malformed_frames_on_loopback() {
    ends_on_malformed(python, Via::Loopback).await;
}

#[cfg(feature = "websocket")]
#[tokio::test(flavor = "multi_thread")]
async fn node_ends_the_session_on_malformed_frames_on_a_websocket() {
    ends_on_malformed(node, Via::WebSocket("node")).await;
}

#[cfg(feature = "websocket")]
#[tokio::test(flavor = "multi_thread")]
async fn python_ends_the_session_on_malformed_frames_on_a_websocket() {
    ends_on_malformed(python, Via::WebSocket("python")).await;
}

/// Q6.2.2, Q6.2.3, risk P3: Go ends the session on the same malformed and
/// dangling frames as every other implementation (#294); Go's
/// `encoding/json` replaces invalid UTF-8 instead of failing, so its
/// channel checks each frame.
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "Unix socket column: Windows runtimes connect on loopback TCP"
)]
async fn go_ends_the_session_on_malformed_frames_on_a_unix_socket() {
    ends_on_malformed(go, Via::UnixSocket).await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_ends_the_session_on_malformed_frames_on_loopback() {
    ends_on_malformed(go, Via::Loopback).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_ends_the_session_on_malformed_frames() {
    ends_on_malformed(bun, Via::UnixSocket).await;
}
