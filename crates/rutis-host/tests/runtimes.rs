//! A local runtime process that ends while `rutis-host run` runs: the
//! released binary, with a Python and a Node row, one of the runtimes
//! killed with SIGKILL.
//!
//! guarantee: host/runtime-state-lines — `run` says which runtime went
//! down and how its process ended; the other runtime and its rows go on.
//!
//! Unix only: the runtime is killed with kill(2).
#![cfg(unix)]

mod common;

use common::process::{alive, run_project, signal, Host};

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
    // SAFETY: plain kill(2).
    assert_eq!(unsafe { libc::kill(python, libc::SIGKILL) }, 0, "kill");
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
