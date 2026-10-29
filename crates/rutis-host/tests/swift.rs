//! Swift binaries in rutis-host (macOS; needs `swift` on PATH): rows of
//! `runtimes.swift` run in their binary's runtime, which `restart` brings
//! back; `check` describes each binary and fails on one built with an SDK
//! older than rutis 0.13.
#![cfg(target_os = "macos")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::{normalize, repo};
use rutis_bridge::session::HostDispatch;
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

/// Runtimes start cold: twice the hang guard.
const COLD: Guard = Guard(std::time::Duration::from_secs(2 * HANG_GUARD.as_secs()));

/// The Swift package's product `product` (the repository's Package.swift),
/// built once, copied to `dir` as `name`.
fn install(product: &str, dir: &Path, name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let bin = BUILT.get_or_init(|| {
        let status = std::process::Command::new("swift")
            .arg("build")
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    });
    std::fs::create_dir_all(dir).unwrap();
    let target = dir.join(name);
    std::fs::copy(bin.join(product), &target).unwrap();
    target
}

/// A program that says it is a Swift binary of plugin API 2 (an SDK before
/// rutis 0.13) when asked for its manifest.
fn old_sdk(path: &Path) {
    std::fs::write(
        path,
        "#!/bin/sh\n# rutis-swift-runtime:1\n\
         echo '{\"manifest\":1,\"sdk\":\"0.12.0\",\"pluginApi\":2,\"runtime\":\"rutis-swift-runtime:1\",\"plugins\":{\"old\":{\"inject\":[],\"provides\":{}}}}'\n",
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
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

/// A row `swift:ping` runs in the runtime of the binary that has it,
/// started when the row loads; `restart: on-failure` starts it again when
/// it crashes, and the row comes back.
/// guarantee: host/runtime-restart, loader/swift-runtimes, swift/tcc-belongs-to-the-binary
#[tokio::test(flavor = "multi_thread")]
async fn a_swift_row_runs_in_its_binarys_runtime_and_comes_back_after_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    install("RutisPing", &dir.path().join("plugins"), "netkit");
    let mut config: HostConfig = serde_json::from_value(json!({
        "id": "swift-test",
        "runtimes": { "swift": { "dir": "plugins", "restart": "on-failure" } },
        "rows": [{ "id": "ping", "name": "swift:ping" }]
    }))
    .unwrap();
    config.rebase(dir.path());
    let host = COLD
        .within("the host's start", Host::start(&config))
        .await
        .unwrap();
    let _exits = host.supervise();
    COLD.within("the rows", host.load(config.rows()))
        .await
        .unwrap();
    let service = COLD
        .eventually(|| host.service("ping"), "the ping service")
        .await;
    assert_eq!(
        call(service, "echo", json!(["swift"])).await.unwrap(),
        json!("netkit pong swift")
    );
    // The runtime answers for its own access to protected data (TCC):
    // macOS holds it responsible, not the host that started it.
    let identity = call(host.service("ping").unwrap(), "responsible", json!([]))
        .await
        .unwrap();
    assert_eq!(identity[0], identity[1], "pid, responsible pid: {identity}");
    let swift = host.binaries_of("swift").unwrap();
    assert_eq!(swift.runtimes()[0].name, "swift-netkit");
    assert_eq!(swift.title(), "Swift");

    let _ = call(host.service("ping").unwrap(), "crash", json!([])).await;
    // Gone, then back after the backoff's first wait (1 s).
    COLD.until(|| host.service("ping").is_none(), "the crash")
        .await;
    let service = COLD
        .eventually(|| host.service("ping"), "the ping service again")
        .await;
    assert_eq!(
        call(service, "echo", json!(["back"])).await.unwrap(),
        json!("netkit pong back")
    );
    // Stopped, its runtime ends with it: nothing is left running.
    COLD.within("the host's stop", host.shut_down(HANG_GUARD))
        .await
        .ok()
        .expect("the host stops");
}

/// `check` lists each Swift binary with its platform, SDK and plugin API;
/// one built with an SDK older than rutis 0.13 fails it, saying to rebuild.
/// guarantee: host/check, loader/swift-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn check_describes_swift_binaries_and_fails_an_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    install("RutisPing", &plugins, "netkit");
    old_sdk(&plugins.join("legacy"));
    std::fs::write(
        dir.path().join("rutis.json"),
        json!({
            "runtimes": { "swift": { "dir": "plugins" } },
            "rows": [{ "id": "ping", "name": "swift:ping" }]
        })
        .to_string(),
    )
    .unwrap();
    let output = Guard(HANG_GUARD * 6)
        .within(
            "check ends",
            tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
                .arg("check")
                .current_dir(dir.path())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap();
    let stdout = normalize(&String::from_utf8_lossy(&output.stdout), dir.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let context = format!("stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(output.status.code(), Some(1), "{context}");
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        arch => arch,
    };
    for line in [
        "plugin API for Swift binaries: 3 (checked by this host; the Node, Bun and Python runtimes check their own plugins)".to_owned(),
        "ping (swift:ping): ok".to_owned(),
        "  runtime: \"swift-netkit\"".to_owned(),
        "swift binaries:".to_owned(),
        "  swift-legacy (<dir>/plugins/legacy): sdk 0.12.0, plugin API 2 (incompatible: built with an SDK older than rutis 0.13; rebuild it with the 0.13 SDK or later), plugins: old".to_owned(),
        format!("  swift-netkit (<dir>/plugins/netkit): macos/{arch}, sdk {}, plugin API 3 (compatible), plugins: ping", sdk_version()),
    ] {
        assert!(stdout.lines().any(|out| out == line), "{line}\n{context}");
    }
    assert!(
        stderr.contains("1 row(s) or binaries cannot run"),
        "{context}"
    );
}

/// The Swift SDK's version (swift/rutis/Sources/Rutis/Version.swift).
fn sdk_version() -> String {
    let source =
        std::fs::read_to_string(repo().join("swift/rutis/Sources/Rutis/Version.swift")).unwrap();
    let start = source.find("sdkVersion").unwrap();
    let rest = &source[start..];
    let rest = &rest[rest.find('"').unwrap() + 1..];
    rest[..rest.find('"').unwrap()].to_owned()
}

/// The Swift binary `product` running remotely on `channel` (`listen:`) as
/// the endpoint `id`, for the host `test`, expecting `token`; its stderr
/// lines come out of the receiver.
fn remote(
    product: &str,
    channel: &str,
    id: &str,
    token: &str,
) -> (
    tokio::process::Child,
    tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    use tokio::io::AsyncBufReadExt;
    let dir = tempfile::tempdir().unwrap().keep();
    let binary = install(product, &dir, "netkit");
    let mut child = tokio::process::Command::new(binary)
        .arg(channel)
        .args(["--id", id, "--peer", "test"])
        .arg(&dir)
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

/// A Swift binary listening on another machine (here, on loopback): a
/// dial row connects it, and its rows are `<row>:<plugin>`.
/// guarantee: loader/remote-runtime-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_dial_row_runs_swift_rows_named_after_it() {
    let (_child, mut lines) = remote(
        "RutisPing",
        "listen:ws://127.0.0.1:0/rutis",
        "edge",
        "edge-token",
    );
    let address = COLD
        .within("the runtime's address", async {
            loop {
                let line = lines.recv().await.expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
    std::env::set_var("RUTIS_TOKEN_EDGE", "edge-token");
    let config: HostConfig = serde_json::from_value(json!({
        "id": "test",
        "rows": [
            { "id": "edge", "name": "rutis-host/dial", "config": { "address": address } },
            { "id": "ping", "name": "edge:ping" }
        ]
    }))
    .unwrap();
    let host = Host::start(&config).await.unwrap();
    host.load(config.rows()).await.unwrap();
    let service = COLD
        .eventually(|| host.service("ping"), "the ping service")
        .await;
    assert_eq!(
        call(service, "echo", json!(["far"])).await.unwrap(),
        json!("netkit pong far")
    );
    host.shut_down(HANG_GUARD).await.ok();
}

/// The Swift template, built against this checkout's Swift package, its
/// row configured as its `rutis.dev.json` says: `check --strict` passes,
/// with no config error and no extra key (config schema design §11, 1c).
/// guarantee: host/new, sdk/config-schema
#[test]
fn the_swift_template_passes_check_with_its_dev_config() {
    let dir = tempfile::tempdir().unwrap();
    rutis_host::new::create(dir.path(), "greeter", "swift").unwrap();
    let project = dir.path().join("greeter");
    let manifest = project.join("Package.swift");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let from = text
        .lines()
        .find(|line| line.contains(".package(url: \"https://github.com/arcships/rutis\""))
        .expect("the rutis dependency")
        .trim()
        .trim_end_matches(',')
        .to_owned();
    // A path dependency is named after its directory.
    let checkout = repo().canonicalize().unwrap();
    let identity = checkout
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_lowercase();
    let text = text
        .replace(
            &from,
            &format!(".package(path: {:?})", checkout.display().to_string()),
        )
        .replace("package: \"rutis\"", &format!("package: {identity:?}"));
    std::fs::write(&manifest, text).unwrap();
    let status = std::process::Command::new("swift")
        .args(["build", "--product", "greeter"])
        .current_dir(&project)
        .status()
        .expect("swift is on PATH");
    assert!(status.success(), "swift build");
    let output = std::process::Command::new("swift")
        .args(["build", "--show-bin-path"])
        .current_dir(&project)
        .output()
        .unwrap();
    let bin = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::copy(bin.join("greeter"), plugins.join("greeter")).unwrap();
    let dev: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join("rutis.dev.json")).unwrap())
            .unwrap();
    std::fs::write(
        dir.path().join("rutis.json"),
        json!({
            "runtimes": { "swift": { "dir": "plugins" } },
            "rows": [{ "id": "greeter", "name": "swift:greeter", "config": dev["rows"][0]["config"] }]
        })
        .to_string(),
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
        .args(["check", "--strict", "rutis.json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let context = format!("stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(output.status.code(), Some(0), "{context}");
    assert_eq!(stderr, "", "{context}");
    assert!(
        stdout
            .lines()
            .any(|line| line == "greeter (swift:greeter): ok"),
        "{context}"
    );
}
