//! rutis-host installed with npm (#664, in #666): the process a service
//! manager knows is the launcher, `bin/rutis-host.mjs`, which runs this
//! binary as npm installs it (`@arcships/rutis-host-<platform>`). SIGUSR1
//! to the launcher reaches rutis-host, which reads its configuration again
//! (`systemctl reload` with `ExecReload=/bin/kill -USR1 $MAINPID`), and
//! Node does not open its debugger for it.
//!
//! Unix only: the signals are Unix's.
#![cfg(unix)]

mod common;

use std::path::Path;

use common::process::{signal, Host};
use common::repo;
use serde_json::json;

/// An npm installation in `dir`: the launcher of this checkout and this
/// binary as its platform's package.
fn install(dir: &Path) -> std::path::PathBuf {
    let modules = dir.join("node_modules/@arcships");
    let launcher = modules.join("rutis-host/bin/rutis-host.mjs");
    std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    std::fs::copy(repo().join("node/rutis-host/bin/rutis-host.mjs"), &launcher).unwrap();
    // As Node names the platform.
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        arch => arch,
    };
    let package = modules.join(format!("rutis-host-{os}-{arch}"));
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(
        package.join("package.json"),
        json!({ "name": format!("@arcships/rutis-host-{os}-{arch}") }).to_string(),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        env!("CARGO_BIN_EXE_rutis-host"),
        package.join("bin/rutis-host"),
    )
    .unwrap();
    launcher
}

/// guarantee: host/reload-config
#[test]
fn sigusr1_to_the_npm_launcher_reloads_the_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = install(dir.path());
    let file = dir.path().join("rutis.json");
    let config = |rows: serde_json::Value| {
        json!({ "id": "launched", "listen": [{ "name": "local", "address": "127.0.0.1:0" }], "rows": rows })
            .to_string()
    };
    std::fs::write(&file, config(json!([]))).unwrap();
    let mut host = Host::start_through(
        &["node", launcher.to_str().unwrap()],
        &["run", "rutis.json"],
        dir.path(),
    );
    host.wait_line(|line| {
        line.starts_with("rutis-host: listening on local ")
            .then_some(())
    });

    // A dial row to a port nothing listens on: a row that runs, not
    // connected.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let rows = json!([{ "id": "office", "name": "rutis-host/dial", "config": { "address": format!("ws://{address}/rutis") } }]);
    std::fs::write(&file, config(rows)).unwrap();
    // To the launcher alone, as systemd's `kill -USR1 $MAINPID`.
    signal(&host, libc::SIGUSR1, false);
    host.wait_lines(&[
        "rutis-host: reloaded rutis.json (1 row changed)",
        "office: running",
    ]);

    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert!(!stderr.contains("Debugger listening"), "{context}");
}
