//! How long a local runtime's process may take to greet
//! (`docs/design-host-plugin-limits-2026-10-27.md` §4.4): `greeting`
//! bounds, or unbounds, it.
#![cfg(unix)]

use std::time::Duration;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::{LocalRuntime, RuntimeState};

/// A process that never greets: with `greeting(1s)` the runtime is down
/// within a few seconds, not the default 30.
// guarantee: host/plugin-limits (greeting bounds the start)
#[tokio::test(flavor = "multi_thread")]
async fn a_short_greeting_limit_ends_a_silent_runtime() {
    use rutis_bridge::runtime::Launcher;
    let dir = tempfile::tempdir().unwrap();
    let launcher = Launcher::new("sh").arg("-c").arg("sleep 60").inherit_fd();
    let runtime =
        LocalRuntime::launcher("mute", launcher, dir.path()).greeting(Duration::from_secs(1));
    let handle = runtime.handle();
    let root = Ctx::root().unwrap();
    let started = std::time::Instant::now();
    let view = root.plugin(runtime);
    tokio::time::timeout(Duration::from_secs(10), &view)
        .await
        .expect("down within the greeting limit")
        .unwrap_err();
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(matches!(handle.state(), RuntimeState::Down(_)));
    root.shutdown().await.unwrap();
}

/// With `greeting(Duration::MAX)` a process that has not greeted yet (one
/// stopped at its first line for a debugger) stays starting.
// guarantee: host/plugin-limits (greeting(Duration::MAX) waits)
#[tokio::test(flavor = "multi_thread")]
async fn an_unbounded_greeting_waits_for_a_silent_runtime() {
    use rutis_bridge::runtime::Launcher;
    let dir = tempfile::tempdir().unwrap();
    let launcher = Launcher::new("sh").arg("-c").arg("sleep 60").inherit_fd();
    let runtime = LocalRuntime::launcher("mute", launcher, dir.path()).greeting(Duration::MAX);
    let handle = runtime.handle();
    let root = Ctx::root().unwrap();
    let view = root.plugin(runtime);
    assert!(
        tokio::time::timeout(Duration::from_secs(3), &view)
            .await
            .is_err(),
        "still starting"
    );
    assert!(matches!(handle.state(), RuntimeState::Starting));
    root.shutdown().await.unwrap();
}
