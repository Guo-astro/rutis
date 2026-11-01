//! A runtime whose process starts but never greets: its mount fails at the
//! greeting limit — [`GREETING`], or the mount's own (`Mount::greeting`) —
//! saying that the process started and did not greet, and ends it, instead
//! of waiting for ever (#985).
//!
//! Unix only, like `local_spawn.rs`: the process is a `sh` script, and the
//! limit is a fraction of a second, so no test waits the 30 s a real mount
//! gives.
#![cfg(unix)]

use std::time::Duration;

use rutis_bridge::pipe;
use rutis_bridge::runtime::{Launcher, Mount, Process};
use rutis_test_support::{scaled, within};

/// A process that starts, holds its channel, and never greets.
fn mute() -> Launcher {
    Launcher::new("sh").arg("-c").arg("sleep 60").inherit_fd()
}

/// A process that started and says nothing fails the mount at the limit
/// the mount gives it: the error says it started, which runtime, and the
/// limit; its process is ended, not left behind.
// guarantee: host/plugin-limits (the greeting limit bounds a mount)
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_never_greets_fails_the_mount_at_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = mute();
    // Scaled on a loaded machine, like every hang guard here; the wait
    // itself is the mount's own limit, which the test measures.
    let limit = scaled(Duration::from_millis(300));
    let started = std::time::Instant::now();
    // The mount's own limit ends it well within this test's guard (ten
    // seconds, scaled), which only catches a mount that never returns: the
    // behaviour before #985.
    let error = within(
        "the mount of a runtime that never greets to fail at its limit",
        Process::mount(
            dir.path(),
            Mount {
                anchor: Some(dir.path()),
                launcher: Some(&launcher),
                greeting: Some(limit),
                ..Mount::default()
            },
        ),
    )
    .await
    .map(|mounted| {
        panic!(
            "the runtime never greets, yet it mounted ({} calls)",
            mounted.calls().len()
        )
    })
    .unwrap_err();
    let took = started.elapsed();
    assert_eq!(
        error.to_string(),
        format!(
            "the runtime process runtime started but did not greet within {}s",
            limit.as_secs_f64()
        )
    );
    assert!(
        took >= limit,
        "the mount failed after {took:?}, before its greeting limit"
    );
    // The mount closed the session, which ends the process: nothing is
    // left of a runtime that never greeted.
    within("its process to end", pipe::processes_ended()).await;
}
