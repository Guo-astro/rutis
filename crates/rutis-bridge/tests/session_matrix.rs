//! The runtime conformance suite (`rutis_bridge::runtime::testing::runtime`) over
//! every channel: the Node, the Python, (feature `go`) the Go and (feature
//! `swift`, local channels) the Swift runtime, each connected on an
//! inherited socket (`fd:3`), on a socket path it dials back, on a loopback
//! address it dials presenting a token, and over a loopback WebSocket it
//! listens on (dialed by Rust and attached); the Bun runtime (feature `bun`,
//! Unix only) on the first two and the WebSocket. Risk C10: the matrix, with every skipped
//! cell's reason, is in docs/ci.md.
#![cfg(all(feature = "testing", feature = "websocket"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis::Ctx;
use rutis_bridge::runtime::{Launcher, Mount, Process};
use serde_json::{json, Value};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Clone, Copy, Debug)]
enum Runtime {
    Node,
    Python,
    #[cfg_attr(not(feature = "go"), allow(dead_code))]
    Go,
    #[cfg_attr(not(all(feature = "swift", target_os = "macos")), allow(dead_code))]
    Swift,
    /// Unix only: Bun on Windows is not a declared platform.
    #[cfg(all(unix, feature = "bun"))]
    Bun,
    /// The Kotlin SDK's jvm target (`Rutis.serve`), started by the generic
    /// launcher: a socket path it dials back, and loopback (K-P1; the
    /// WebSocket column comes with K-P2).
    #[cfg(feature = "kotlin")]
    Kotlin,
}

#[cfg(feature = "kotlin")]
mod kotlin;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Via {
    /// `fd:3`. Not on Windows, which hands no descriptor over.
    Inherit,
    /// A socket path the runtime dials. Not on Windows: no Unix sockets.
    DialBack,
    /// A loopback address the runtime dials, presenting a token
    /// (`Handover::Loopback` of the local transport, as `LocalRuntime`
    /// starts runtimes): the only handover on Windows.
    Loopback,
    WebSocket,
}

/// The mount every conformance cell uses: the runtime's listeners join
/// `ctx`'s bus, where the host emits what `weather.expected.json` says.
fn weather_mount<'a>(anchor: &'a Path, launcher: &'a Launcher, ctx: &Ctx) -> Mount<'a> {
    Mount {
        anchor: Some(anchor),
        launcher: Some(launcher),
        ctx: Some(ctx.clone()),
        ..Mount::default()
    }
}

/// The same bus, for a mount built by hand (`Process::attach`).
fn on_bus<'a>(ctx: &Ctx) -> Mount<'a> {
    Mount {
        ctx: Some(ctx.clone()),
        ..Mount::default()
    }
}

/// The Python interpreter: one with `websockets` for WebSocket channels
/// (`RUTIS_PYTHON`), else `python3` (`python` on Windows, where
/// setup-python installs no `python3`).
fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// Start `runtime` on a fresh project, connected as `via` says.
async fn start(runtime: Runtime, via: Via, dir: &Path, ctx: &Ctx) -> (Arc<Process>, PathBuf) {
    let (launcher, anchor, entry) = match runtime {
        Runtime::Node => {
            let package = repo().join("node/rutis-runtime");
            let anchor = dir.join("package.json");
            std::fs::write(&anchor, "{}").unwrap();
            // Loaded where it is: it imports the SDK by path.
            let entry = repo().join("node/rutis-runtime/test/fixtures/conformance-weather.mjs");
            let launcher = Launcher::new("node")
                .arg("--import")
                .arg("tsx")
                .arg(package.join("src/runner.mjs"))
                .cwd(&package);
            (launcher, anchor, entry)
        }
        Runtime::Python => {
            std::fs::copy(
                repo().join("python/rutis/tests/conformance_weather.py"),
                dir.join("weather_plugin.py"),
            )
            .unwrap();
            let path = std::env::join_paths([repo().join("python/rutis"), dir.to_owned()]).unwrap();
            let launcher = Launcher::new(python())
                .arg("-m")
                .arg("rutis")
                .env("PYTHONPATH", path);
            (launcher, dir.to_owned(), PathBuf::from("weather_plugin"))
        }
        Runtime::Go => {
            let binary = go_binary("./internal/fixtures/weather");
            (
                Launcher::new(binary),
                dir.to_owned(),
                PathBuf::from("weather"),
            )
        }
        Runtime::Swift => (
            Launcher::new(swift_binary("RutisConformance")),
            dir.to_owned(),
            PathBuf::from("weather"),
        ),
        #[cfg(all(unix, feature = "bun"))]
        Runtime::Bun => {
            let entry = dir.join("weather.ts");
            std::fs::copy(
                repo().join("bun/rutis-bun/test/fixtures/conformance-weather.ts"),
                &entry,
            )
            .unwrap();
            let mut launcher = Launcher::bun(None, &repo().join("bun/rutis-bun"), dir);
            // Inheriting fd:3 is the Bun launcher's default; `via` decides.
            launcher.inherit_fd = false;
            (launcher, dir.to_owned(), entry)
        }
        #[cfg(feature = "kotlin")]
        Runtime::Kotlin => (
            kotlin::launcher().cwd(dir),
            dir.to_owned(),
            PathBuf::from("weather"),
        ),
    };
    match via {
        Via::WebSocket => return (listening(launcher, &anchor, ctx).await, entry),
        Via::Loopback => return (handed_over(launcher, &anchor, ctx).await, entry),
        _ => {}
    }
    let launcher = match via {
        Via::Inherit => launcher.inherit_fd(),
        _ => launcher,
    };
    let process = Process::mount(
        &repo().join("node/rutis-runtime"),
        weather_mount(&anchor, &launcher, ctx),
    )
    .await
    .unwrap_or_else(|error| panic!("{runtime:?} ({via:?}) failed to start: {error}"));
    (process, entry)
}

/// Start the runtime with `Handover::Loopback` through the local transport,
/// on every platform, and attach.
async fn handed_over(launcher: Launcher, anchor: &Path, ctx: &Ctx) -> Arc<Process> {
    use rutis_bridge::channel::PeerId;
    use rutis_bridge::transport::local::{Handover, LocalTransport, Spawn};
    use rutis_bridge::{Dial, Transport};

    let mut spawn = Spawn::new(launcher.program, PeerId::new("runtime").unwrap());
    spawn.args = launcher.args;
    spawn.env = launcher.env;
    spawn.cwd = launcher.cwd;
    spawn.handover = Handover::Loopback;
    spawn.trailing = vec![anchor.into()];
    let transport = LocalTransport::default();
    transport.spawner("runtime", spawn);
    let channel = transport
        .dial(&Dial::address("spawn:runtime"))
        .await
        .unwrap();
    assert_eq!(channel.info.transport, "tcp", "a loopback channel");
    let format = rutis_bridge::session::Format::Compat;
    let process = Process::attach(channel, on_bus(ctx), format).await.unwrap();
    // The transport ends its processes when dropped: keep it for the test.
    std::mem::forget(transport);
    process
}

/// Start the runtime listening on a loopback WebSocket, dial it, attach.
async fn listening(launcher: Launcher, anchor: &Path, ctx: &Ctx) -> Arc<Process> {
    use rutis_bridge::channel::PeerId;
    use rutis_bridge::{Credential, Dial, Identity, StaticIdentity, Transport};
    use tokio::io::AsyncBufReadExt;

    let mut command = tokio::process::Command::new(&launcher.program);
    command
        .args(&launcher.args)
        .envs(launcher.env.iter().map(|(name, value)| (name, value)))
        .arg("listen:ws://127.0.0.1:0/rutis")
        .args(["--id", "runtime", "--peer", "main"])
        .arg(anchor)
        .env("RUTIS_TOKEN", "controller-token")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &launcher.cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = loop {
        let line = lines
            .next_line()
            .await
            .unwrap()
            .expect("the runtime's address");
        if let Some(address) = line.strip_prefix("rutis: listening on ") {
            break address.to_owned();
        }
    };
    // Keep reading its stderr, and keep it running for the test's length.
    tokio::spawn(async move {
        while let Ok(Some(_)) = lines.next_line().await {}
        let _ = child.wait().await;
    });
    let transport = rutis_bridge::transport::websocket::WebSocketTransport::start(
        rutis_bridge::transport::websocket::Config::new(),
    )
    .unwrap();
    let runtime = PeerId::new("runtime").unwrap();
    let identity: Arc<dyn Identity> =
        Arc::new(StaticIdentity::new(PeerId::new("main").unwrap()).present(
            runtime.clone(),
            Credential::Bearer("controller-token".into()),
        ));
    let channel = transport
        .dial(
            &Dial::address(address)
                .peer(runtime)
                .identity(identity)
                .protocol(format!(
                    "rutis.{}",
                    rutis_bridge::session::ENDPOINT_PROTOCOL
                )),
        )
        .await
        .unwrap();
    // The transport's threads must outlive the channel: leak it for the test.
    std::mem::forget(transport);
    let format = rutis_bridge::session::Format::Endpoint(
        rutis_bridge::session::Endpoint::rust(PeerId::new("main").unwrap())
            .expect(PeerId::new("runtime").unwrap()),
    );
    let process = Process::attach(channel, on_bus(ctx), format).await.unwrap();
    assert!(
        process.connection().supports("runtime"),
        "a runner declares its contract"
    );
    process
}

async fn semantics(runtime: Runtime, via: Via) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::root().unwrap();
    let (process, entry) = start(runtime, via, dir.path(), &ctx).await;
    // Only the `Process` facade starts the process itself and reports its
    // exit; the other columns attach to a channel.
    let started_here = matches!(via, Via::Inherit | Via::DialBack);
    rutis_bridge::runtime::testing::runtime(&ctx, process, &entry, started_here).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "fd:3 column: Windows hands no descriptor over (loopback handover only)"
)]
async fn node_on_an_inherited_socket() {
    semantics(Runtime::Node, Via::Inherit).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "socket path column: Windows has no Unix sockets (loopback handover only)"
)]
async fn node_dialing_a_socket_path() {
    semantics(Runtime::Node, Via::DialBack).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn node_listening_on_a_websocket() {
    semantics(Runtime::Node, Via::WebSocket).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn node_dialing_a_loopback_address() {
    semantics(Runtime::Node, Via::Loopback).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "fd:3 column: Windows hands no descriptor over (loopback handover only)"
)]
async fn python_on_an_inherited_socket() {
    semantics(Runtime::Python, Via::Inherit).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "socket path column: Windows has no Unix sockets (loopback handover only)"
)]
async fn python_dialing_a_socket_path() {
    semantics(Runtime::Python, Via::DialBack).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_listening_on_a_websocket() {
    semantics(Runtime::Python, Via::WebSocket).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn python_dialing_a_loopback_address() {
    semantics(Runtime::Python, Via::Loopback).await;
}

/// The Go package `package` (under `go/rutis`), built into a fresh
/// directory with the `go` on PATH.
fn go_binary(package: &str) -> PathBuf {
    let directory = tempfile::tempdir().unwrap().keep();
    let binary = directory.join(format!("runtime{}", std::env::consts::EXE_SUFFIX));
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

/// A product of the Swift package (the repository's root `Package.swift`),
/// built with the `swift` on PATH.
fn swift_binary(product: &str) -> PathBuf {
    let status = std::process::Command::new("swift")
        .args(["build", "--product", product])
        .current_dir(repo())
        .status()
        .expect("swift is on PATH");
    assert!(status.success(), "swift build --product {product}");
    let output = std::process::Command::new("swift")
        .args(["build", "--show-bin-path"])
        .current_dir(repo())
        .output()
        .expect("swift is on PATH");
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).join(product)
}

// Swift (feature `swift`): the local channels. No WebSocket column:
// listening is a later phase of the Swift SDK (A1, design §15).
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_on_an_inherited_socket() {
    semantics(Runtime::Swift, Via::Inherit).await;
}

#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_dialing_a_socket_path() {
    semantics(Runtime::Swift, Via::DialBack).await;
}

#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_dialing_a_loopback_address() {
    semantics(Runtime::Swift, Via::Loopback).await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "fd:3 column: Windows hands no descriptor over (loopback handover only)"
)]
async fn go_on_an_inherited_socket() {
    semantics(Runtime::Go, Via::Inherit).await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "socket path column: Windows has no Unix sockets (loopback handover only)"
)]
async fn go_dialing_a_socket_path() {
    semantics(Runtime::Go, Via::DialBack).await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_listening_on_a_websocket() {
    semantics(Runtime::Go, Via::WebSocket).await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_dialing_a_loopback_address() {
    semantics(Runtime::Go, Via::Loopback).await;
}

/// A runtime package that does not list `fd` in `rutisChannels` is started
/// the old way, dialing a socket path.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "fd:3 and socket path handovers: Windows has the loopback handover only"
)]
async fn a_runtime_without_fd_support_dials_back() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("runtime");
    std::fs::create_dir_all(package.join("src")).unwrap();
    let real = repo().join("node/rutis-runtime");
    for file in std::fs::read_dir(real.join("src")).unwrap() {
        let file = file.unwrap();
        if file.file_type().unwrap().is_dir() {
            continue;
        }
        std::fs::copy(file.path(), package.join("src").join(file.file_name())).unwrap();
    }
    copy_dir(&real.join("src/channel"), &package.join("src/channel"));
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(real.join("package.json")).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("rutisChannels");
    std::fs::write(package.join("package.json"), manifest.to_string()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(real.join("node_modules"), package.join("node_modules")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(real.join("node_modules"), package.join("node_modules"))
        .unwrap();
    // The runner sees its channel argument: record it.
    let anchor = dir.path().join("package.json");
    std::fs::write(&anchor, "{}").unwrap();
    let probe = dir.path().join("argv.mjs");
    std::fs::write(
        &probe,
        "export function apply(ctx) { ctx.provide('argv', { channel() { return process.argv[2] } }) }",
    )
    .unwrap();
    let process = Process::launch(&package, &probe, json!({}), json!({ "argv": ["channel"] }))
        .await
        .unwrap();
    let channel = process.call("argv", "channel", json!([])).unwrap();
    let channel = channel.as_str().unwrap();
    assert!(!channel.starts_with("fd:"), "{channel}");
    assert!(channel.ends_with("peer.sock"), "{channel}");
    process.dispose().await.unwrap();

    // The real package takes fd 3.
    let process = Process::launch(&real, &probe, json!({}), json!({ "argv": ["channel"] }))
        .await
        .unwrap();
    assert_eq!(
        process.call("argv", "channel", json!([])).unwrap(),
        json!("fd:3")
    );
    process.dispose().await.unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for file in std::fs::read_dir(from).unwrap() {
        let file = file.unwrap();
        std::fs::copy(file.path(), to.join(file.file_name())).unwrap();
    }
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_on_an_inherited_socket() {
    semantics(Runtime::Bun, Via::Inherit).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_dialing_a_socket_path() {
    semantics(Runtime::Bun, Via::DialBack).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_listening_on_a_websocket() {
    semantics(Runtime::Bun, Via::WebSocket).await;
}

#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "socket path column: Windows has no Unix sockets (loopback handover only)"
)]
async fn kotlin_dialing_a_socket_path() {
    semantics(Runtime::Kotlin, Via::DialBack).await;
}

#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn kotlin_dialing_a_loopback_address() {
    semantics(Runtime::Kotlin, Via::Loopback).await;
}
