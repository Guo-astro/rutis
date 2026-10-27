//! How `rutis-host run` and `rutis-host dev` end on a signal (#174): the
//! released binary, started as a user or a service manager starts it, with a
//! Python and a Node row that record their cleanup and their process.
//!
//! guarantee: host/stop-on-signal, host/stop-deadline, host/exit-codes,
//! host/ignored-signals-stay-ignored — every cleanup runs exactly once on
//! unload, also when a signal ends the host; the runtime processes it
//! started have exited when it has (Q5.4.4, Q6.6.3; risks B2, B3); the exit
//! code follows docs/guide/rutis-host.md.
//!
//! Unix only: the test sends SIGINT and SIGTERM with kill(2), to the process
//! group as a terminal does on Ctrl-C and to the host alone as a service
//! manager does. The Windows console events (Ctrl-C, Ctrl-Break, closing the
//! console) need a console shared with the test process, and CI does not run
//! the rutis-host tests on Windows; they are not covered here (#186 / S8).
#![cfg(unix)]

mod common;

use std::process::Command;

use common::process::{
    alive, cleanups, dev_project, run_project, signal, unprefixed, Host, HANGING_PLUGIN,
};
use common::repo;
use serde_json::json;

/// `run` ended by `signal`: every cleanup ran once, the runtimes exited,
/// the host exited 0.
fn run_ends_cleanly(sig: i32, group: bool) {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, sig, group);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    for row in ["py-row", "node-row"] {
        assert_eq!(cleanups(dir.path(), row), 1, "{row}'s cleanup; {context}");
    }
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// Ctrl-C in a terminal: SIGINT reaches the host and every process of its
/// group.
/// guarantee: host/stop-on-signal
#[test]
fn run_ends_on_ctrl_c_after_every_cleanup() {
    run_ends_cleanly(libc::SIGINT, true);
}

/// A service manager stopping the host: SIGTERM to the host.
/// guarantee: host/stop-on-signal, host/exit-codes
#[test]
fn run_ends_on_sigterm_after_every_cleanup() {
    run_ends_cleanly(libc::SIGTERM, false);
}

/// The terminal closing: SIGHUP to the host.
/// guarantee: host/stop-on-signal
#[test]
fn run_ends_on_sighup_after_every_cleanup() {
    run_ends_cleanly(libc::SIGHUP, false);
}

/// A signal while the rows still start (one hangs in apply) ends the host
/// too: the deadline passes, so 2, and the runtime process is gone.
/// guarantee: host/stop-deadline
#[test]
fn a_signal_while_starting_ends_the_host() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hangs.py"), HANGING_PLUGIN).unwrap();
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "runtimes": { "py": py },
        "rows": [{ "id": "py-row", "name": "py:hangs", "config": { "row": "py-row" } }]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    // clock: the deadline must pass, and does whatever its length: the
    // apply never ends. 1 s only keeps the test short.
    let mut host = Host::start(
        &["run", "--shutdown-timeout", "1", file.to_str().unwrap()],
        dir.path(),
    );
    let pid = host.wait_line(|line| {
        line.strip_prefix("plugin py-row hangs in ")
            .and_then(|pid| pid.trim().parse::<i32>().ok())
    });
    host.runtimes.push(pid);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    // Row states are printed only once the host has started.
    assert!(
        !output.iter().any(|line| line.starts_with("py-row:")),
        "{context}"
    );
    assert!(
        !alive(pid),
        "runtime process {pid} outlived the host; {context}"
    );
}

/// `dev` reloading a plugin whose new code hangs in apply still ends on
/// Ctrl-C (past the deadline, so 2), and the runtime process with it.
/// guarantee: host/stop-deadline
#[test]
fn dev_ends_on_ctrl_c_while_a_reload_hangs() {
    let dir = tempfile::tempdir().unwrap();
    dev_project(dir.path());
    // clock: the deadline must pass, and does whatever its length: the new
    // code's apply never ends. 1 s only keeps the test short.
    let mut host = Host::start(&["dev", "--shutdown-timeout", "1", "."], dir.path());
    let pids = host.started(&["marker"]);
    std::fs::write(dir.path().join("marker.py"), HANGING_PLUGIN).unwrap();
    host.wait_line(|line| {
        unprefixed(line)
            .starts_with("plugin marker hangs in ")
            .then_some(())
    });
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// guarantee: host/stop-on-signal
#[test]
fn dev_ends_on_ctrl_c_after_its_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    dev_project(dir.path());
    let mut host = Host::start(&["dev", "."], dir.path());
    let pids = host.started(&["marker"]);
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert_eq!(cleanups(dir.path(), "marker"), 1, "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// A cleanup that never ends: past the deadline the host exits 2, names
/// what was still stopping, and ends the runtime processes anyway, with
/// the processes they started (the output ends).
/// guarantee: host/stop-deadline, host/exit-codes
#[test]
fn run_past_the_cleanup_deadline_exits_2_and_ends_the_runtimes() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), true);
    // clock: the deadline must pass, and does whatever its length: both
    // cleanups never end (`hold`). 1 s only keeps the test short.
    let mut host = Host::start(
        &["run", "--shutdown-timeout", "1", file.to_str().unwrap()],
        dir.path(),
    );
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    assert!(stderr.contains("did not finish"), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// A second Ctrl-C while cleanups run ends the host at once, with 2, and
/// the runtime processes with it.
/// guarantee: host/stop-deadline
#[test]
fn a_second_ctrl_c_exits_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), true);
    // A deadline the test never reaches: only the second Ctrl-C ends it.
    let mut host = Host::start(
        &["run", "--shutdown-timeout", "600", file.to_str().unwrap()],
        dir.path(),
    );
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, libc::SIGINT, true);
    // Rows unload one after another: the first cleanup holds the others.
    host.wait_line(|line| line.ends_with(" cleanup started").then_some(()));
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// Started ignoring SIGHUP (`nohup`), the host keeps ignoring it: the
/// SIGTERM after it is what stops the host, which then ends normally.
/// guarantee: host/ignored-signals-stay-ignored
#[test]
fn a_signal_ignored_when_the_host_started_stays_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    let mut host = Host::start_ignoring(
        &["run", file.to_str().unwrap()],
        dir.path(),
        Some(libc::SIGHUP),
    );
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, libc::SIGHUP, false);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert!(stderr.contains("SIGTERM: stopping"), "{context}");
    assert!(!stderr.contains("SIGHUP"), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
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

/// A Go row's runtime ends with the host on Ctrl-C too: out of the
/// terminal's process group, it is unloaded over its channel, and the host
/// waits for it to exit (run in the Go CI job: the name has `go`).
/// guarantee: host/stop-on-signal, host/row-state-lines
#[test]
fn run_with_a_go_row_ends_on_ctrl_c() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let status = Command::new("go")
        .args(["build", "-o"])
        .arg(plugins.join("netkit"))
        .arg("./cmd/netkit")
        .current_dir(repo().join("crates/rutis-loader/tests/fixtures/go"))
        .status()
        .expect("go is on PATH");
    assert!(status.success());
    let config = json!({
        "runtimes": { "go": { "dir": "plugins" } },
        "rows": [{ "id": "ping", "name": "go:ping" }]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    host.wait_line(|line| (line == "ping: running").then_some(()));
    let pids = children(host.pid());
    assert!(!pids.is_empty(), "the Go runtime process");
    host.runtimes.extend(&pids);
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}
