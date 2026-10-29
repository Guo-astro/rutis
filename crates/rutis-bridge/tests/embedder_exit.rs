//! An application embedding rutis (a local runtime started under a session,
//! as rutis-host's `rutis-host/spawn` does) leaves no runtime
//! process behind, also when a plugin keeps that runtime busy (#682):
//! killed by a signal it does not handle (Ctrl-C's SIGINT, SIGTERM), shut
//! down with a deadline the busy runtime does not meet, or dropped without
//! shutting down.
//!
//! The killed embedder is this test binary run again as one
//! (`embedder_role`), the runtime being its child as in any application.
#![cfg(all(unix, feature = "node"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::Process;
use rutis_test_support::{eventually_blocking, scaled, HANG_GUARD};
use serde_json::json;

mod common;
use common::runtime::{root, TestRuntime};

/// Writes its process's id into `config.marker`, then keeps the runtime's
/// main thread busy for ever.
const NODE_BUSY: &str = r#"
import { writeFileSync } from 'node:fs'
export function apply(ctx, config) {
  setTimeout(() => { writeFileSync(config.marker, String(process.pid)); for (;;) {} }, 50)
}
"#;

#[cfg(feature = "python")]
const PYTHON_BUSY: &str = r#"
import asyncio
import os


def apply(ctx, config):
    def spin():
        with open(config["marker"], "w") as marker:
            marker.write(str(os.getpid()))
        while True:
            pass

    asyncio.get_running_loop().call_later(0.05, spin)
"#;

/// The runtimes the tests run.
#[derive(Clone, Copy, Debug)]
enum Language {
    Node,
    #[cfg(feature = "python")]
    Python,
    #[cfg(feature = "bun")]
    Bun,
}

impl Language {
    fn name(self) -> &'static str {
        match self {
            Language::Node => "node",
            #[cfg(feature = "python")]
            Language::Python => "python",
            #[cfg(feature = "bun")]
            Language::Bun => "bun",
        }
    }

    fn named(name: &str) -> Self {
        match name {
            #[cfg(feature = "python")]
            "python" => Language::Python,
            #[cfg(feature = "bun")]
            "bun" => Language::Bun,
            _ => Language::Node,
        }
    }

    /// The local runtime on `dir`, and the busy plugin's row entry there.
    fn runtime(self, dir: &Path) -> (TestRuntime, PathBuf) {
        match self {
            Language::Node => {
                let entry = dir.join("busy.mjs");
                std::fs::write(&entry, NODE_BUSY).unwrap();
                (TestRuntime::node(), entry)
            }
            #[cfg(feature = "python")]
            Language::Python => {
                std::fs::write(dir.join("busy.py"), PYTHON_BUSY).unwrap();
                (TestRuntime::python(dir), PathBuf::from("busy"))
            }
            #[cfg(feature = "bun")]
            Language::Bun => {
                std::fs::write(dir.join("package.json"), "{}").unwrap();
                let entry = dir.join("busy.mjs");
                std::fs::write(&entry, NODE_BUSY).unwrap();
                (TestRuntime::bun(dir), entry)
            }
        }
    }
}

/// Start `language`'s runtime on `dir` under `root` and keep it busy; its
/// process's id.
async fn busy_runtime(root: &Ctx, language: Language, dir: &Path) -> (i32, RuntimeParts) {
    let (runtime, entry) = language.runtime(dir);
    let handle = runtime.handle();
    let processes = runtime.processes();
    root.plugin(runtime).await.unwrap();
    let process: Arc<Process> = handle.ready().await.expect("the runtime runs");
    let marker = dir.join("busy");
    process
        .load_row("busy", &entry, json!({ "marker": marker }), &[], &[], &[])
        .await
        .unwrap();
    let pid = rutis_test_support::eventually(
        || {
            std::fs::read_to_string(&marker)
                .ok()
                .and_then(|pid| pid.trim().parse::<i32>().ok())
        },
        "the plugin to keep its runtime busy",
    )
    .await;
    (pid, RuntimeParts { processes })
}

/// What a test keeps of a runtime it started.
struct RuntimeParts {
    processes: rutis_bridge::pipe::Processes,
}

/// Whether the process `pid` still runs (not gone, not a zombie).
fn alive(pid: i32) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    !(stat.trim().is_empty() || stat.trim().starts_with('Z'))
}

/// This binary as an embedding application: it starts the runtime the
/// environment names, keeps it busy, prints the runtime's process id and
/// then waits to be killed. Not a test of its own: without the
/// environment, it does nothing.
#[test]
fn embedder_role() {
    let (Some(dir), Some(language)) = (
        std::env::var_os("RUTIS_TEST_EMBEDDER_DIR"),
        std::env::var("RUTIS_TEST_EMBEDDER_LANGUAGE").ok(),
    ) else {
        return;
    };
    let tasks = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    tasks.block_on(async {
        let root = root();
        let (pid, parts) = busy_runtime(&root, Language::named(&language), Path::new(&dir)).await;
        // On a line of its own: the test harness's `test embedder_role ...`
        // has no newline.
        println!("\nbusy-runtime {pid}");
        // Kept: the application holds its runtime until it is killed.
        std::future::pending::<()>().await;
        drop((root, parts));
    });
}

/// The embedding application, killed with `signal`, which it does not
/// handle: its runtime, busy as it is, ends within its deadline (1 s) and
/// a second more.
fn killed_embedder_leaves_no_runtime(language: Language, signal: i32) {
    use std::io::BufRead;
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "embedder_role",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("RUTIS_TEST_EMBEDDER_DIR", dir.path())
        .env("RUTIS_TEST_EMBEDDER_LANGUAGE", language.name())
        .env("RUTIS_SHUTDOWN_TIMEOUT", "1")
        .stdout(Stdio::piped());
    // With the default action for `signal`, whatever this test was started
    // with (a background job ignores SIGINT, and its children with it).
    // SAFETY: only signal(2), async-signal-safe, between fork and exec.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            libc::signal(signal, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut embedder = Killed(command.spawn().unwrap(), Vec::new());
    let stdout = embedder.0.stdout.take().unwrap();
    let (send, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if let Some((_, pid)) = line.split_once("busy-runtime ") {
                let _ = send.send(pid.trim().parse::<i32>().unwrap());
            }
        }
    });
    let pid = lines
        .recv_timeout(scaled(HANG_GUARD * 3))
        .unwrap_or_else(|_| panic!("{language:?}: the embedder did not start its runtime"));
    embedder.1.push(pid);
    // SAFETY: kill(2) on the embedder this test started.
    unsafe {
        libc::kill(embedder.0.id() as i32, signal);
    }
    let status = embedder.0.wait().unwrap();
    assert!(!status.success(), "{language:?}: the embedder was killed");
    eventually_blocking(
        || (!alive(pid)).then_some(()),
        &format!("{language:?}: the busy runtime {pid} ends after its embedder was killed"),
    );
}

/// The embedder, and the runtime processes seen: killed when the test ends,
/// so a failing test leaves nothing running.
struct Killed(std::process::Child, Vec<i32>);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        for pid in &self.1 {
            if alive(*pid) {
                // SAFETY: kill(2) on a runtime process this test saw start.
                unsafe {
                    libc::kill(*pid, libc::SIGKILL);
                }
            }
        }
    }
}

/// guarantee: host/killed-host-leaves-no-runtimes
#[test]
fn node_ends_when_its_embedder_gets_ctrl_c() {
    killed_embedder_leaves_no_runtime(Language::Node, libc::SIGINT);
}

/// guarantee: host/killed-host-leaves-no-runtimes
#[test]
fn node_ends_when_its_embedder_gets_sigterm() {
    killed_embedder_leaves_no_runtime(Language::Node, libc::SIGTERM);
}

/// guarantee: host/killed-host-leaves-no-runtimes
#[cfg(feature = "python")]
#[test]
fn python_ends_when_its_embedder_gets_ctrl_c() {
    killed_embedder_leaves_no_runtime(Language::Python, libc::SIGINT);
}

/// guarantee: host/killed-host-leaves-no-runtimes
#[cfg(feature = "bun")]
#[test]
fn bun_ends_when_its_embedder_gets_ctrl_c() {
    killed_embedder_leaves_no_runtime(Language::Bun, libc::SIGINT);
}

/// Shutting down: a busy runtime is ended once its grace after its channel
/// closed passed. A shutdown that does not finish in time (rows a busy
/// runtime cannot unload) is followed by `Processes::kill`, as rutis-host
/// does.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_runtime_ends_with_a_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let root = root();
    let (pid, parts) = busy_runtime(&root, Language::Node, dir.path()).await;
    if root
        .shutdown_with_timeout(Duration::from_secs(5))
        .await
        .is_err()
    {
        parts.processes.kill();
    }
    rutis_test_support::within("the busy runtime to be gone", parts.processes.ended()).await;
    assert!(!alive(pid));
}

/// An application that drops its tokio runtime without shutting rutis
/// down: the runtime processes go with it.
#[test]
fn a_busy_runtime_goes_with_a_dropped_application() {
    let dir = tempfile::tempdir().unwrap();
    let tasks = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (pid, root) = tasks.block_on(async {
        let root = root();
        let (pid, _parts) = busy_runtime(&root, Language::Node, dir.path()).await;
        (pid, root)
    });
    drop(tasks);
    drop(root);
    eventually_blocking(
        || (!alive(pid)).then_some(()),
        "the busy runtime to go with the application",
    );
}
