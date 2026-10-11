//! The session conformance suite against every implementation of the
//! session: Rust, Node, Python and Bun, each serving `conformance` as endpoint
//! of an endpoint-format session.
#![cfg(all(unix, feature = "testing"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis_bridge::channel::PeerId;
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

/// Listen on a Unix socket, start `command` with it, open the session.
async fn far_end(
    mut command: tokio::process::Command,
    far: &str,
) -> (Connection, tokio::process::Child) {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("conformance.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let child = command.arg(&socket).kill_on_drop(true).spawn().unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let stream = stream.into_std().unwrap();
    stream.set_nonblocking(false).unwrap();
    let reader = stream.try_clone().unwrap();
    struct Shut(std::os::unix::net::UnixStream);
    impl rutis_bridge::channel::Closer for Shut {
        fn close(&self, _: &str) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }
    let closer = Arc::new(Shut(stream.try_clone().unwrap()));
    // The newline framing local runtimes speak, here from the transport crate.
    let channel = rutis_bridge::transport::local::framed(reader, stream, closer);
    std::mem::forget(directory);
    (
        Connection::open_with(channel, Arc::new(Nothing), main_endpoint(far)).unwrap(),
        child,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn node_meets_the_session_contract() {
    let mut command = tokio::process::Command::new("node");
    command
        .args(["--import", "tsx"])
        .arg(repo().join("node/rutis-runtime/test/fixtures/conformance-session.mjs"))
        .current_dir(repo().join("node/rutis-runtime"));
    let (far, _child) = far_end(command, "node").await;
    run(far).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_meets_the_session_contract() {
    let mut command = tokio::process::Command::new("python3");
    command
        .arg(repo().join("python/rutis/tests/conformance_session.py"))
        .env("PYTHONPATH", repo().join("python/rutis"));
    let (far, _child) = far_end(command, "python").await;
    run(far).await;
}

#[cfg(feature = "go")]
/// Build the Go package `package` (a path under `go/rutis`) into a fresh
/// directory, with the toolchain on PATH (`go`).
fn go_binary(package: &str) -> PathBuf {
    let directory = tempfile::tempdir().unwrap().keep();
    let binary = directory.join("conformance");
    let status = std::process::Command::new("go")
        .args(["build", "-o"])
        .arg(&binary)
        .arg(package)
        .current_dir(repo().join("go/rutis"))
        .status()
        .expect("go is on PATH");
    assert!(status.success(), "go build {package}");
    binary
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_meets_the_session_contract() {
    let command = tokio::process::Command::new(go_binary("./internal/fixtures/session"));
    let (far, _child) = far_end(command, "go").await;
    run(far).await;
}

#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn bun_meets_the_session_contract() {
    let mut command = tokio::process::Command::new("bun");
    command
        .args(["--no-install", "--no-env-file"])
        .arg(repo().join("bun/rutis-bun/test/fixtures/conformance-session.ts"))
        .current_dir(repo().join("bun/rutis-bun"));
    let (far, _child) = far_end(command, "bun").await;
    run(far).await;
}

/// Far ends started one per channel: a command given a Unix socket path to
/// dial, framed by the local transport. Dropped, it kills them all.
struct Started(std::sync::Mutex<Vec<std::process::Child>>);
impl Drop for Started {
    fn drop(&mut self) {
        for mut child in self.0.lock().unwrap().drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Started {
    fn open(&self, mut command: std::process::Command) -> rutis_bridge::channel::Channel {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("malformed.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let child = command.arg(&socket).spawn().unwrap();
        self.0.lock().unwrap().push(child);
        let (stream, _) = listener.accept().unwrap();
        let reader = stream.try_clone().unwrap();
        struct Shut(std::os::unix::net::UnixStream);
        impl rutis_bridge::channel::Closer for Shut {
            fn close(&self, _: &str) {
                let _ = self.0.shutdown(std::net::Shutdown::Both);
            }
        }
        let closer = Arc::new(Shut(stream.try_clone().unwrap()));
        rutis_bridge::transport::local::framed(reader, stream, closer)
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

#[tokio::test(flavor = "multi_thread")]
async fn node_ends_the_session_on_malformed_frames() {
    tokio::task::spawn_blocking(|| {
        let started = Started(Default::default());
        malformed(|| {
            let mut command = std::process::Command::new("node");
            command
                .args(["--import", "tsx"])
                .arg(repo().join("node/rutis-runtime/test/fixtures/conformance-session.mjs"))
                .current_dir(repo().join("node/rutis-runtime"));
            started.open(command)
        })
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn python_ends_the_session_on_malformed_frames() {
    tokio::task::spawn_blocking(|| {
        let started = Started(Default::default());
        malformed(|| {
            let mut command = std::process::Command::new("python3");
            command
                .arg(repo().join("python/rutis/tests/conformance_session.py"))
                .env("PYTHONPATH", repo().join("python/rutis"));
            started.open(command)
        })
    })
    .await
    .unwrap();
}

#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn bun_ends_the_session_on_malformed_frames() {
    tokio::task::spawn_blocking(|| {
        let started = Started(Default::default());
        malformed(|| {
            let mut command = std::process::Command::new("bun");
            command
                .args(["--no-install", "--no-env-file"])
                .arg(repo().join("bun/rutis-bun/test/fixtures/conformance-session.ts"))
                .current_dir(repo().join("bun/rutis-bun"));
            started.open(command)
        })
    })
    .await
    .unwrap();
}
