//! A runtime whose host went away (#353): the session ends with no `dispose`, as
//! when `rutis-host` is killed. The runtime unloads its rows, running their
//! cleanups, and exits by itself — also when a plugin keeps work going that
//! would keep the process alive (a timer nobody clears), and when a cleanup
//! never finishes, which it gives up at its deadline
//! (`RUTIS_SHUTDOWN_TIMEOUT`, in seconds).
//!
//! Each plugin starts such work, and its cleanup writes a marker file and,
//! with `hang`, never returns.
// The Node rows run in every case; the other runtimes add theirs.
#![cfg(feature = "node")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis_bridge::runtime::{Launcher, Mount, Process};
use rutis_bridge::session::Error;
use rutis_test_support::eventually;
use serde_json::json;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const NODE: &str = r#"
import { writeFileSync } from 'node:fs'
export function apply(ctx, config) {
  setInterval(() => {}, 1 << 30)
  ctx.effect(() => () => {
    writeFileSync(config.marker, 'cleaned up')
    if (config.hang) return new Promise(() => {})
  })
}
"#;

#[cfg(feature = "python")]
const PYTHON: &str = r#"
import asyncio
import threading
import time


def apply(ctx, config):
    loop = asyncio.get_running_loop()
    loop.call_later(3600, lambda: None)
    threading.Thread(target=lambda: time.sleep(3600)).start()

    async def cleanup():
        with open(config["marker"], "w") as marker:
            marker.write("cleaned up")
        if config.get("hang"):
            await asyncio.Event().wait()

    return cleanup
"#;

#[cfg(all(unix, feature = "bun"))]
const BUN: &str = r#"
import { writeFileSync } from 'node:fs'
export function apply(ctx, config) {
  setInterval(() => {}, 1 << 30)
  return () => {
    writeFileSync(config.marker, 'cleaned up')
    if (config.hang) return new Promise(() => {})
  }
}
"#;

#[derive(Clone, Copy, Debug)]
enum Runtime {
    Node,
    #[cfg(feature = "python")]
    Python,
    #[cfg(feature = "go")]
    Go,
    #[cfg(all(unix, feature = "bun"))]
    Bun,
}

/// Start `runtime` on `dir` with cleanups given `deadline` seconds once its
/// session ends; the row entry of the lingering plugin.
async fn start(runtime: Runtime, dir: &Path, deadline: &str) -> (Arc<Process>, PathBuf) {
    let (package, launcher, anchor, entry) = match runtime {
        Runtime::Node => {
            let package = repo().join("node/rutis-runtime");
            let anchor = dir.join("package.json");
            std::fs::write(&anchor, "{}").unwrap();
            let entry = dir.join("lingering.mjs");
            std::fs::write(&entry, NODE).unwrap();
            (package.clone(), Launcher::node(&package), anchor, entry)
        }
        #[cfg(feature = "python")]
        Runtime::Python => {
            let sdk = repo().join("python/rutis");
            std::fs::write(dir.join("lingering.py"), PYTHON).unwrap();
            let path = std::env::join_paths([sdk.clone(), dir.to_owned()]).unwrap();
            let python = std::env::var("RUTIS_PYTHON")
                .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into());
            let launcher = Launcher::new(python)
                .arg("-m")
                .arg("rutis")
                .env("PYTHONPATH", path);
            (sdk, launcher, dir.to_owned(), PathBuf::from("lingering"))
        }
        #[cfg(feature = "go")]
        Runtime::Go => {
            let binary = go_binary("./internal/fixtures/lingering");
            (
                repo().join("go/rutis"),
                Launcher::go(&binary, dir),
                dir.to_owned(),
                PathBuf::from("lingering"),
            )
        }
        #[cfg(all(unix, feature = "bun"))]
        Runtime::Bun => {
            let package = repo().join("bun/rutis-bun");
            let entry = dir.join("lingering.ts");
            std::fs::write(&entry, BUN).unwrap();
            let launcher = Launcher::bun(None, &package, dir);
            (package, launcher, dir.to_owned(), entry)
        }
    };
    let launcher = launcher.env("RUTIS_SHUTDOWN_TIMEOUT", deadline);
    let process = Process::mount(
        &package,
        Mount {
            anchor: Some(&anchor),
            launcher: Some(&launcher),
            ..Mount::default()
        },
    )
    .await
    .unwrap_or_else(|error| panic!("{runtime:?} failed to start: {error}"));
    (process, entry)
}

/// The Go package `package` (under `go/rutis`), built into a fresh
/// directory with the `go` on PATH.
#[cfg(feature = "go")]
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

/// Load the lingering plugin into `runtime`, end the session the way a
/// killed host does, and return how the process ended.
async fn host_goes_away(runtime: Runtime, hang: bool, deadline: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let (process, entry) = start(runtime, dir.path(), deadline).await;
    let marker = dir.path().join("cleaned-up");
    process
        .load_row(
            "lingering",
            &entry,
            json!({ "marker": marker, "hang": hang }),
            &[],
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|error| panic!("{runtime:?}: rows.load: {error}"));
    assert!(process.exit_status().is_none(), "{runtime:?} runs");
    // The host is gone: its end of the channel closes, and no `dispose`
    // came first.
    process
        .connection()
        .close(Error::Transport("the host went away".into()));
    let status = eventually(
        || process.exit_status(),
        &format!("{runtime:?} exits once its host went away"),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(&marker).ok().as_deref(),
        Some("cleaned up"),
        "{runtime:?} ran the row's cleanup before exiting"
    );
    status
}

/// Cleanups that finish: the runtime exits once they did, not at its
/// deadline (an hour here), though the plugin's work would keep it.
async fn exits_after_its_cleanups(runtime: Runtime) {
    let status = host_goes_away(runtime, false, "3600").await;
    assert_eq!(status, "exited normally", "{runtime:?}");
}

/// A cleanup that never finishes: the runtime gives it up at its deadline,
/// exits, and says so in its exit status.
async fn gives_up_a_cleanup_at_its_deadline(runtime: Runtime) {
    let status = host_goes_away(runtime, true, "0.5").await;
    assert!(
        status.starts_with("exited with exit ") && status.ends_with(" 1"),
        "{runtime:?}: {status}"
    );
}

// ── A plugin keeping the runtime busy (#682) ────────────────────

/// Unix: the exit statuses read as Unix ones (on Windows the runtimes end
/// with their host's job anyway).
#[cfg(unix)]
mod busy {
    use super::*;

    /// Marks `config.marker`, then keeps the runtime's main thread busy for
    /// ever: the runtime cannot run anything else, its session's end included.
    const NODE_BUSY: &str = r#"
import { writeFileSync } from 'node:fs'
export function apply(ctx, config) {
  setTimeout(() => { writeFileSync(config.marker, 'busy'); for (;;) {} }, 50)
}
"#;

    #[cfg(feature = "python")]
    const PYTHON_BUSY: &str = r#"
import asyncio


def apply(ctx, config):
    def spin():
        with open(config["marker"], "w") as marker:
            marker.write("busy")
        while True:
            pass

    asyncio.get_running_loop().call_later(0.05, spin)
"#;

    /// The busy plugin's row entry, written next to the lingering one.
    fn busy_entry(runtime: Runtime, dir: &Path) -> PathBuf {
        match runtime {
            #[cfg(feature = "python")]
            Runtime::Python => {
                std::fs::write(dir.join("busy.py"), PYTHON_BUSY).unwrap();
                PathBuf::from("busy")
            }
            #[cfg(feature = "go")]
            Runtime::Go => {
                unreachable!("a busy goroutine does not keep the Go runtime from its end")
            }
            _ => {
                let entry = dir.join("busy.mjs");
                std::fs::write(&entry, NODE_BUSY).unwrap();
                entry
            }
        }
    }

    /// A plugin keeps the runtime busy when its host goes away: the runtime
    /// cannot unload its rows, but it does not outlive its host either. Once
    /// the deadline (1 s here) and a second more have passed, it ends: Node and
    /// Bun from a watchdog thread (killed), Python from the thread reading its
    /// channel (exit status 1).
    async fn a_busy_runtime_ends_when_its_host_goes_away(runtime: Runtime) {
        let dir = tempfile::tempdir().unwrap();
        let (process, _) = start(runtime, dir.path(), "1").await;
        let entry = busy_entry(runtime, dir.path());
        let marker = dir.path().join("busy");
        process
            .load_row("busy", &entry, json!({ "marker": marker }), &[], &[], &[])
            .await
            .unwrap_or_else(|error| panic!("{runtime:?}: rows.load: {error}"));
        rutis_test_support::until(|| marker.exists(), "the plugin to keep its runtime busy").await;
        let gone = std::time::Instant::now();
        process
            .connection()
            .close(Error::Transport("the host went away".into()));
        let status = eventually(
            || process.exit_status(),
            &format!("{runtime:?} ends once its host went away, busy as it is"),
        )
        .await;
        let took = gone.elapsed();
        match runtime {
            #[cfg(feature = "python")]
            Runtime::Python => assert_eq!(status, "exited with exit status: 1", "{runtime:?}"),
            _ => assert_eq!(status, "exited with signal: 9 (SIGKILL)", "{runtime:?}"),
        }
        // Not before its deadline: it gave its cleanups their time.
        assert!(
            took >= std::time::Duration::from_secs(1),
            "{runtime:?} ended after {took:?}"
        );
    }

    /// guarantee: host/killed-host-leaves-no-runtimes
    #[tokio::test(flavor = "multi_thread")]
    async fn node_kept_busy_ends_when_its_host_goes_away() {
        a_busy_runtime_ends_when_its_host_goes_away(Runtime::Node).await;
    }

    /// guarantee: host/killed-host-leaves-no-runtimes
    #[cfg(feature = "python")]
    #[tokio::test(flavor = "multi_thread")]
    async fn python_kept_busy_ends_when_its_host_goes_away() {
        a_busy_runtime_ends_when_its_host_goes_away(Runtime::Python).await;
    }

    /// guarantee: host/killed-host-leaves-no-runtimes
    #[cfg(all(unix, feature = "bun"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn bun_kept_busy_ends_when_its_host_goes_away() {
        a_busy_runtime_ends_when_its_host_goes_away(Runtime::Bun).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn node_exits_after_its_cleanups_when_its_host_goes_away() {
    exits_after_its_cleanups(Runtime::Node).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn node_gives_up_a_cleanup_at_its_deadline_when_its_host_goes_away() {
    gives_up_a_cleanup_at_its_deadline(Runtime::Node).await;
}

#[cfg(feature = "python")]
#[tokio::test(flavor = "multi_thread")]
async fn python_exits_after_its_cleanups_when_its_host_goes_away() {
    exits_after_its_cleanups(Runtime::Python).await;
}

#[cfg(feature = "python")]
#[tokio::test(flavor = "multi_thread")]
async fn python_gives_up_a_cleanup_at_its_deadline_when_its_host_goes_away() {
    gives_up_a_cleanup_at_its_deadline(Runtime::Python).await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_exits_after_its_cleanups_when_its_host_goes_away() {
    exits_after_its_cleanups(Runtime::Go).await;
}

#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_gives_up_a_cleanup_at_its_deadline_when_its_host_goes_away() {
    gives_up_a_cleanup_at_its_deadline(Runtime::Go).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_exits_after_its_cleanups_when_its_host_goes_away() {
    exits_after_its_cleanups(Runtime::Bun).await;
}

#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn bun_gives_up_a_cleanup_at_its_deadline_when_its_host_goes_away() {
    gives_up_a_cleanup_at_its_deadline(Runtime::Bun).await;
}
