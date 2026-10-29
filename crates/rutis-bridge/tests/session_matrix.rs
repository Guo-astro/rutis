//! guarantee: loader/announced-services
//! The runtime conformance suite (`rutis_bridge::runtime::testing::runtime`) over
//! every channel: the Node, the Python, the Go and (macOS) the Swift
//! runtime, each connected on an inherited socket (`fd:3`), on a socket
//! path it dials back, on a loopback address it dials presenting a token,
//! and over a loopback WebSocket it listens on (dialed by Rust and
//! attached); the Bun runtime (Unix only) on the first two and the
//! WebSocket; the Kotlin one on all but the first. The Go, Swift, Bun and
//! Kotlin cells run when `RUTIS_TEST_RUNTIMES` lists their language. Risk
//! C10: the matrix, with every skipped cell's reason, is in docs/ci.md.
#![cfg(all(feature = "testing", feature = "websocket"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::{Launcher, Mount, Process};
use rutis_test_support::runtimes;
use serde_json::{json, Value};

mod common;
use common::runtime::launcher;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Clone, Copy, Debug)]
enum Runtime {
    Node,
    Python,
    Go,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Swift,
    /// Unix only: Bun on Windows is not a declared platform.
    #[cfg(unix)]
    Bun,
    /// The Kotlin SDK's jvm target (`Rutis.serve`), started by the generic
    /// launcher: a socket path it dials back, loopback, and a WebSocket it
    /// listens on (K-P2: the listener an Android application has too).
    Kotlin,
}

mod kotlin;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Via {
    /// `fd:3`. Not on Windows, which hands no descriptor over.
    Inherit,
    /// A socket path the runtime dials. Not on Windows: no Unix sockets.
    DialBack,
    /// A loopback address the runtime dials, presenting a token
    /// (`Handover::Loopback` of `pipe::spawn`, as `Launcher::start`
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

/// Start `runtime` on a fresh project, connected as `via` says. The
/// Python interpreter is `RUTIS_PYTHON` (one with `websockets` for
/// WebSocket channels), else `python3`.
async fn start(runtime: Runtime, via: Via, dir: &Path, ctx: &Ctx) -> (Arc<Process>, PathBuf) {
    let (mut launcher, anchor, entry) = match runtime {
        Runtime::Node => {
            let anchor = dir.join("package.json");
            std::fs::write(&anchor, "{}").unwrap();
            // Loaded where it is: it imports the SDK by path.
            let entry = repo().join("node/rutis-runtime/test/fixtures/conformance-weather.mjs");
            (launcher(runtimes::node_runtime()), anchor, entry)
        }
        Runtime::Python => {
            std::fs::copy(
                repo().join("python/rutis/tests/conformance_weather.py"),
                dir.join("weather_plugin.py"),
            )
            .unwrap();
            (
                launcher(runtimes::python_runtime(dir)),
                dir.to_owned(),
                PathBuf::from("weather_plugin"),
            )
        }
        Runtime::Go => {
            let binary = go_binary("./internal/fixtures/weather");
            (
                launcher(runtimes::binary(&binary, dir)),
                dir.to_owned(),
                PathBuf::from("weather"),
            )
        }
        Runtime::Swift => (
            launcher(runtimes::binary(&swift_binary("RutisConformance"), dir)),
            dir.to_owned(),
            PathBuf::from("weather"),
        ),
        #[cfg(unix)]
        Runtime::Bun => {
            let entry = dir.join("weather.ts");
            std::fs::copy(
                repo().join("bun/rutis-bun/test/fixtures/conformance-weather.ts"),
                &entry,
            )
            .unwrap();
            (launcher(runtimes::bun_runtime(dir)), dir.to_owned(), entry)
        }
        Runtime::Kotlin => (
            kotlin::launcher().cwd(dir),
            dir.to_owned(),
            PathBuf::from("weather"),
        ),
    };
    // Inheriting fd:3 is the commands' default; `via` decides.
    launcher.inherit_fd = false;
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

/// Start the runtime with `Handover::Loopback` through `pipe::spawn`, on
/// every platform, and attach.
async fn handed_over(launcher: Launcher, anchor: &Path, ctx: &Ctx) -> Arc<Process> {
    use rutis_bridge::pipe::{self, Handover, Processes, Spawn};

    let mut spawn = Spawn::new(launcher.program);
    spawn.args = launcher.args;
    spawn.env = launcher.env;
    spawn.cwd = launcher.cwd;
    spawn.handover = Handover::Loopback;
    spawn.trailing = vec!["--id".into(), "runtime".into(), anchor.into()];
    let channel = pipe::spawn(&spawn, &Processes::default()).await.unwrap();
    let main = rutis_bridge::channel::PeerId::new("main").unwrap();
    let runtime = rutis_bridge::channel::PeerId::new("runtime").unwrap();
    let endpoint = rutis_bridge::session::Endpoint::rust(main).expect(runtime);
    Process::attach(channel, on_bus(ctx), endpoint)
        .await
        .unwrap()
}

/// Start the runtime listening on a loopback WebSocket, dial it, attach.
async fn listening(launcher: Launcher, anchor: &Path, ctx: &Ctx) -> Arc<Process> {
    use rutis_bridge::channel::PeerId;
    use rutis_bridge::websocket::Credential;
    use rutis_bridge::websocket::{Limits, Trust};
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
    let channel = rutis_bridge::websocket::dial(
        &address,
        &format!("rutis.{}", rutis_bridge::session::ENDPOINT_PROTOCOL),
        Some(&Credential::Bearer("controller-token".into())),
        &Trust::default(),
        &Limits::default(),
    )
    .await
    .unwrap();
    let format = rutis_bridge::session::Endpoint::rust(PeerId::new("main").unwrap())
        .expect(PeerId::new("runtime").unwrap());
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

// Swift (macOS): every channel.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn swift_on_an_inherited_socket() {
    if !rutis_test_support::runtimes::enabled("swift") {
        return;
    }
    semantics(Runtime::Swift, Via::Inherit).await;
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn swift_dialing_a_socket_path() {
    if !rutis_test_support::runtimes::enabled("swift") {
        return;
    }
    semantics(Runtime::Swift, Via::DialBack).await;
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn swift_dialing_a_loopback_address() {
    if !rutis_test_support::runtimes::enabled("swift") {
        return;
    }
    semantics(Runtime::Swift, Via::Loopback).await;
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn swift_listening_on_a_websocket() {
    if !rutis_test_support::runtimes::enabled("swift") {
        return;
    }
    semantics(Runtime::Swift, Via::WebSocket).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "fd:3 column: Windows hands no descriptor over (loopback handover only)"
)]
async fn go_on_an_inherited_socket() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    semantics(Runtime::Go, Via::Inherit).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "socket path column: Windows has no Unix sockets (loopback handover only)"
)]
async fn go_dialing_a_socket_path() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    semantics(Runtime::Go, Via::DialBack).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn go_listening_on_a_websocket() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    semantics(Runtime::Go, Via::WebSocket).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn go_dialing_a_loopback_address() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_on_an_inherited_socket() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
    semantics(Runtime::Bun, Via::Inherit).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_dialing_a_socket_path() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
    semantics(Runtime::Bun, Via::DialBack).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_listening_on_a_websocket() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
    semantics(Runtime::Bun, Via::WebSocket).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "socket path column: Windows has no Unix sockets (loopback handover only)"
)]
async fn kotlin_dialing_a_socket_path() {
    if !rutis_test_support::runtimes::enabled("kotlin") {
        return;
    }
    semantics(Runtime::Kotlin, Via::DialBack).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kotlin_dialing_a_loopback_address() {
    if !rutis_test_support::runtimes::enabled("kotlin") {
        return;
    }
    semantics(Runtime::Kotlin, Via::Loopback).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kotlin_listening_on_a_websocket() {
    if !rutis_test_support::runtimes::enabled("kotlin") {
        return;
    }
    semantics(Runtime::Kotlin, Via::WebSocket).await;
}
