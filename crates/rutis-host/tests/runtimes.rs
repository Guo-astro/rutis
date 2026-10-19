//! A local runtime process that ends while `rutis-host run` (or `dev`)
//! runs: the released binary, with a Python and a Node row, the Python
//! runtime killed with SIGKILL.
//!
//! guarantee: host/runtime-state-lines — `run` says which runtime went
//! down and how its process ended; the other runtime and its rows go on.
//! guarantee: host/runtime-restart — as its `restart` says: nothing by
//! default; `on-failure` starts it again and its rows come back; `exit`
//! stops the host, which exits 3; nothing restarts while the host stops.
//!
//! Unix only: the runtime is killed with kill(2).
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::process::{alive, cleanups, dev_project, run_project, signal, Host};
use common::repo;
use serde_json::{json, Value};

/// Set `runtimes.py.restart` in the configuration `file` (`run`'s, or the
/// dev file).
fn restart(file: &Path, restart: &str) {
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    if config["runtimes"]["py"].is_null() {
        config["runtimes"]["py"] = json!({});
    }
    config["runtimes"]["py"]["restart"] = json!(restart);
    std::fs::write(file, config.to_string()).unwrap();
}

/// SIGKILL to `pid`.
fn kill(pid: i32) {
    // SAFETY: plain kill(2).
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0, "kill");
}

/// SIGKILL to the Python runtime: `runtime py: down (exited with signal:
/// 9 …)`, its row waits, the Node row runs on in the same process, and the
/// host still stops normally.
/// guarantee: host/runtime-state-lines
#[test]
fn a_killed_runtime_is_reported_with_how_it_ended() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    let pids = host.started(&["py-row", "node-row"]);
    let (python, node) = (pids[0], pids[1]);
    kill(python);
    let line = host.wait_line(|line| line.starts_with("runtime ").then(|| line.to_owned()));
    assert!(
        line.starts_with("runtime py: down (exited with signal: 9"),
        "{line}"
    );
    host.wait_line(|line| (line == "py-row: waiting for its services").then_some(()));
    assert!(alive(node), "the Node runtime ended too");
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    // One line for the one runtime that went down.
    let runtime_lines: Vec<&String> = output
        .iter()
        .filter(|line| line.starts_with("runtime "))
        .collect();
    assert_eq!(runtime_lines.len(), 1, "{context}");
    assert!(
        !alive(node),
        "the Node runtime outlived the host; {context}"
    );
}

/// `restart: "on-failure"`: the killed runtime starts again, its row
/// with it (a new process); the Node runtime is the same process.
/// guarantee: host/runtime-restart, host/runtime-state-lines
#[test]
fn a_runtime_restarted_on_failure_brings_its_rows_back() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    restart(&file, "on-failure");
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    let pids = host.started(&["py-row", "node-row"]);
    let (python, node) = (pids[0], pids[1]);
    kill(python);
    host.wait_line(|line| {
        line.starts_with("runtime py: down (exited with signal: 9")
            .then_some(())
    });
    host.wait_line(|line| (line == "runtime py: restarting").then_some(()));
    let again = host.started(&["py-row"])[0];
    assert_ne!(again, python);
    // Printed before its row runs again: the runtime is ready first.
    assert!(
        host.seen.iter().any(|line| line == "runtime py: running"),
        "{:#?}",
        host.seen
    );
    assert!(alive(node), "the Node runtime ended");
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    // The killed process ran no cleanup; the new one's ran once.
    assert_eq!(cleanups(dir.path(), "py-row"), 1, "{context}");
    assert_eq!(cleanups(dir.path(), "node-row"), 1, "{context}");
    assert!(
        !alive(again),
        "the new Python runtime outlived the host; {context}"
    );
}

/// `restart: "exit"`: the host stops (every other row's cleanup runs) and
/// exits 3.
/// guarantee: host/runtime-restart, host/exit-codes
#[test]
fn a_runtime_ending_with_restart_exit_stops_the_host_with_3() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    restart(&file, "exit");
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    let pids = host.started(&["py-row", "node-row"]);
    kill(pids[0]);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(3), "{context}");
    assert!(
        stderr.contains("rutis-host: runtime py ended (exited with signal: 9")
            && stderr.contains("its restart is \"exit\": stopping"),
        "{context}"
    );
    assert_eq!(cleanups(dir.path(), "node-row"), 1, "{context}");
    assert!(
        !alive(pids[1]),
        "the Node runtime outlived the host; {context}"
    );
}

/// A host stopping does not restart a runtime that ended meanwhile: the
/// signal comes during the first wait, and the stop lasts past it (the
/// Node row's cleanup takes 2 s, the first wait 1 s).
/// guarantee: host/runtime-restart
#[test]
fn nothing_restarts_once_the_host_stops() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    restart(&file, "on-failure");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    config["rows"][1]["config"]["delay"] = json!(2000);
    std::fs::write(&file, config.to_string()).unwrap();
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    let pids = host.started(&["py-row", "node-row"]);
    kill(pids[0]);
    host.wait_line(|line| line.starts_with("runtime py: down (").then_some(()));
    // Within the first wait (1 s).
    let stopping = std::time::Instant::now();
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert!(
        stopping.elapsed() >= std::time::Duration::from_secs(2),
        "the stop outlasted the first wait: {:?}; {context}",
        stopping.elapsed()
    );
    assert_eq!(cleanups(dir.path(), "node-row"), 1, "{context}");
    assert!(
        !output.iter().any(|line| line == "runtime py: restarting"),
        "{context}"
    );
    assert!(
        !output
            .iter()
            .any(|line| line.starts_with("plugin py-row applied")
                && !line.ends_with(&pids[0].to_string())),
        "{context}"
    );
}

/// `dev` takes `restart` from `rutis.dev.json`, and restarts as `run`
/// does.
/// guarantee: host/runtime-restart
#[test]
fn dev_restarts_as_run_does() {
    let dir = tempfile::tempdir().unwrap();
    dev_project(dir.path());
    restart(&dir.path().join("rutis.dev.json"), "on-failure");
    let mut host = Host::start(&["dev", "."], dir.path());
    let python = host.started(&["marker"])[0];
    kill(python);
    host.wait_line(|line| (line == "runtime py: restarting").then_some(()));
    let again = host.started(&["marker"])[0];
    assert_ne!(again, python);
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert_eq!(cleanups(dir.path(), "marker"), 1, "{context}");
}

/// A project of one Go row (`go:ping`, the loader's netkit fixture built
/// into `plugins/`) whose runtimes' `restart` is `restart`.
fn go_project(dir: &Path, restart: &str) -> PathBuf {
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let status = Command::new("go")
        .args(["build", "-o"])
        .arg(plugins.join("netkit"))
        .arg("./cmd/netkit")
        .current_dir(repo().join("crates/rutis-loader/tests/fixtures/go"))
        .status()
        .expect("go is on PATH");
    assert!(status.success(), "go build");
    let config = json!({
        "runtimes": { "go": { "dir": "plugins", "restart": restart } },
        "rows": [{ "id": "ping", "name": "go:ping" }]
    });
    let file = dir.join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    file
}

/// The processes `parent` started, as pgrep(1) lists them.
fn children(parent: i32) -> Vec<i32> {
    let output = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|pid| pid.trim().parse().ok())
        .collect()
}

/// `runtimes.go.restart: "on-failure"`: the killed Go runtime starts again
/// (a new process), and its row with it (run in the Go CI job: the name
/// has `go`).
/// guarantee: host/runtime-restart
#[test]
fn a_go_runtime_restarted_on_failure_brings_its_rows_back() {
    let dir = tempfile::tempdir().unwrap();
    let file = go_project(dir.path(), "on-failure");
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    host.wait_line(|line| (line == "ping: running").then_some(()));
    let pids = children(host.pid());
    assert_eq!(pids.len(), 1, "the Go runtime process: {pids:?}");
    host.runtimes.extend(&pids);
    kill(pids[0]);
    host.wait_line(|line| line.starts_with("runtime go-netkit: down (").then_some(()));
    host.wait_line(|line| (line == "runtime go-netkit: restarting").then_some(()));
    host.wait_line(|line| (line == "ping: running").then_some(()));
    let again = children(host.pid());
    assert!(
        again.len() == 1 && again[0] != pids[0],
        "the restarted Go runtime: {again:?}, before {pids:?}"
    );
    host.runtimes.extend(&again);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert!(
        !alive(again[0]),
        "the new Go runtime outlived the host; {context}"
    );
}

/// `runtimes.go.restart: "exit"`: the Go runtime's end stops the host,
/// which exits 3.
/// guarantee: host/runtime-restart, host/exit-codes
#[test]
fn a_go_runtime_ending_with_restart_exit_stops_the_host_with_3() {
    let dir = tempfile::tempdir().unwrap();
    let file = go_project(dir.path(), "exit");
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    host.wait_line(|line| (line == "ping: running").then_some(()));
    let pids = children(host.pid());
    assert_eq!(pids.len(), 1, "the Go runtime process: {pids:?}");
    host.runtimes.extend(&pids);
    kill(pids[0]);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(3), "{context}");
    assert!(
        stderr.contains("rutis-host: runtime go-netkit ended (")
            && stderr.contains("its restart is \"exit\": stopping"),
        "{context}"
    );
    assert!(
        !output.iter().any(|line| line.contains("restarting")),
        "{context}"
    );
}
