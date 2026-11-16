//! A runtime on another machine that listens for its controller
//! (`docs/design-remote-session-2026-10-28.md` §4.4,
//! `docs/guide/nodes.md`「别的机器上的语言运行时」): a real `rutis-host/dial`
//! row connects it and the rows `<row>:<plugin>` run there. The runtime
//! serves one lease at a time: a controller leaving has all its rows
//! cleaned up, a newer connection takes over only once that is done, and
//! the old lease's calls are refused, not served by the new lease.
//!
//! Each runtime is a real one listening on loopback: Python, and Go, Bun
//! and Kotlin when the run lists them (`RUTIS_TEST_RUNTIMES`, #624). Its
//! plugin writes every start, call and cleanup to a log, and its cleanup
//! waits for a gate the test opens: the log then says whether the new
//! controller was greeted before the old lease was gone, instead of the
//! test guessing at timings.
//!
//! guarantee: loader/remote-runtime-lease

// The tests hold the environment until they end, across their awaits: the
// variables they set are for the hosts they start.
#![allow(clippy::await_holding_lock)]

mod common;
#[path = "../../rutis-bridge/tests/kotlin/mod.rs"]
mod kotlin;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use common::{environment, repo};
use rutis_bridge::session::{settle, HostDispatch};
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt;

/// Twice HANG_GUARD: the Python runtime starts cold (#238), the Kotlin
/// one starts a JVM.
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// The token the runtime listens with and the dial row presents.
const TOKEN: &str = "lease-token";

/// The plugin the runtime runs for a controller: it logs its start, its
/// calls and its cleanup; the cleanup waits for `__STOP__`, and `hold`
/// waits for `__HOLD__`, so the test decides when those finish.
const PLUGIN: &str = r#"
import asyncio
import os

LOG = __LOG__
STOP = __STOP__
HOLD = __HOLD__


def write(line):
    with open(LOG, "a", encoding="utf-8") as out:
        out.write(line + "\n")


async def wait_for(path):
    while not os.path.exists(path):
        await asyncio.sleep(0.01)


class Logger:
    def __init__(self, who):
        self.name = who

    def who(self):
        return self.name

    async def hold(self):
        write("hold " + self.name)
        await wait_for(HOLD)
        return "held " + self.name


provides = {"log": Logger}


def apply(ctx, config):
    who = config["who"]
    write("start " + who)
    ctx.provide("log", Logger(who))

    async def stop():
        write("cleanup " + who)
        await wait_for(STOP)
        write("stop " + who)

    ctx.effect(stop)
"#;

#[cfg(unix)]
const BUN: &str = r#"
import { appendFileSync, existsSync } from 'node:fs'

export const provides = { log: { who: 'sync', hold: 'async' } }

function write(path, line) {
  appendFileSync(path, line + '\n')
}

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms)
}

export function apply(ctx, config) {
  write(config.log, `start ${config.who}`)
  ctx.provide('log', {
    who: () => config.who,
    hold: async () => {
      write(config.log, `hold ${config.who}`)
      while (!existsSync(config.hold)) await new Promise((resolve) => setTimeout(resolve, 10))
      return `held ${config.who}`
    },
  })
  ctx.effect(() => {
    write(config.log, `cleanup ${config.who}`)
    while (!existsSync(config.stop)) sleep(10)
    write(config.log, `stop ${config.who}`)
  })
}
"#;

/// The `python` to run: `RUTIS_PYTHON`, else `python3` (`python` on
/// Windows, where setup-python installs no `python3`).
fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// Which runtime listens for its controller.
#[derive(Clone, Copy, Debug)]
enum Language {
    Python,
    Go,
    Kotlin,
    /// Unix only: Bun on Windows is not a declared platform.
    #[cfg(unix)]
    Bun,
}

impl Language {
    /// Whether this run has it: Python is the suite's own baseline, Go,
    /// Kotlin and Bun only when `RUTIS_TEST_RUNTIMES` lists them (#624).
    fn enabled(self) -> bool {
        match self {
            Language::Python => true,
            Language::Go => rutis_test_support::runtimes::enabled("go"),
            Language::Kotlin => rutis_test_support::runtimes::enabled("kotlin"),
            #[cfg(unix)]
            Language::Bun => rutis_test_support::runtimes::enabled("bun"),
        }
    }
}

/// The Go binary of `go/rutis/internal/fixtures/lease`, built once with the
/// `go` on PATH.
fn go_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let binary = directory.join(format!("lease{}", std::env::consts::EXE_SUFFIX));
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./internal/fixtures/lease")
            .current_dir(repo().join("go/rutis"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build ./internal/fixtures/lease");
        binary
    })
}

/// A runtime listening on loopback, its project holding the logger plugin
/// [`PLUGIN`] for Python, or where the Go, Kotlin and Bun plugins write.
struct Runtime {
    language: Language,
    _process: tokio::process::Child,
    address: String,
    /// What the plugin writes, one line per event.
    log: PathBuf,
    /// Created, the plugin's cleanup may finish.
    stop_gate: PathBuf,
    /// Created, a call of the lease that waits in `hold` finishes.
    hold_gate: PathBuf,
    _project: tempfile::TempDir,
}

impl Runtime {
    async fn start(language: Language) -> Self {
        let project = tempfile::tempdir().unwrap();
        let log = project.path().join("lease.log");
        let (stop_gate, hold_gate) = (
            project.path().join("stop-gate"),
            project.path().join("hold-gate"),
        );
        let mut child = match language {
            Language::Python => {
                let source = PLUGIN
                    .replace("__LOG__", &format!("{:?}", log.display().to_string()))
                    .replace(
                        "__STOP__",
                        &format!("{:?}", stop_gate.display().to_string()),
                    )
                    .replace(
                        "__HOLD__",
                        &format!("{:?}", hold_gate.display().to_string()),
                    );
                std::fs::write(project.path().join("logger.py"), source).unwrap();
                tokio::process::Command::new(python())
                    .args(["-m", "rutis", "listen:ws://127.0.0.1:0/rutis"])
                    .args(["--id", "remote", "--peer", "main"])
                    .arg(project.path())
                    .env("PYTHONPATH", repo().join("python/rutis"))
                    .env("RUTIS_TOKEN", TOKEN)
                    .current_dir(project.path())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap()
            }
            Language::Go => tokio::process::Command::new(go_binary())
                .args(["listen:ws://127.0.0.1:0/rutis"])
                .args(["--id", "remote", "--peer", "main"])
                .arg(project.path())
                .env("RUTIS_TOKEN", TOKEN)
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap(),
            Language::Kotlin => {
                // The conformance jar's plugin `kt_lease`, from the SDK's
                // Gradle build or `RUTIS_KOTLIN_JAR`.
                tokio::process::Command::new("java")
                    .args(kotlin::JVM_OPTIONS)
                    .arg("-jar")
                    .arg(kotlin::jar())
                    .args(["listen:ws://127.0.0.1:0/rutis"])
                    .args(["--id", "remote", "--peer", "main"])
                    .arg(project.path())
                    .env("RUTIS_TOKEN", TOKEN)
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap()
            }
            #[cfg(unix)]
            Language::Bun => {
                std::fs::write(project.path().join("package.json"), "{}").unwrap();
                std::fs::write(project.path().join("logger.ts"), BUN).unwrap();
                tokio::process::Command::new("bun")
                    .args(["--no-install", "--no-env-file"])
                    .arg(repo().join("bun/rutis-bun/src/main.ts"))
                    .args(["listen:ws://127.0.0.1:0/rutis"])
                    .args(["--id", "remote", "--peer", "main"])
                    .arg(project.path())
                    .env("RUTIS_TOKEN", TOKEN)
                    .current_dir(project.path())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap()
            }
        };
        let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let address = COLD
            .within("the runtime's address", async {
                loop {
                    let line = lines.next_line().await.unwrap().expect("its address");
                    if let Some(address) = line.strip_prefix("rutis: listening on ") {
                        break address.to_owned();
                    }
                }
            })
            .await;
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("runtime: {line}");
            }
        });
        Self {
            language,
            _process: child,
            address,
            log,
            stop_gate,
            hold_gate,
            _project: project,
        }
    }

    /// What the plugin wrote so far, in order.
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Until the plugin wrote `line`.
    async fn wait_for(&self, line: &str) {
        COLD.eventually(
            || self.lines().iter().any(|l| l == line).then_some(()),
            &format!("{line:?} in {:?}", self.lines()),
        )
        .await;
    }

    /// Let the old lease's cleanup finish.
    fn open_stop_gate(&self) {
        std::fs::write(&self.stop_gate, b"open").unwrap();
    }

    /// A host `main` whose dial row `remote` connects this runtime, with
    /// one row of its plugin configured as `who`.
    async fn controller(&self, who: &str) -> Controller {
        // What the row says: its name, and where the plugin writes. The
        // Python plugin has its paths in its source, Go, Kotlin and Bun in
        // their config.
        let row = match self.language {
            Language::Python => {
                json!({ "id": "log", "name": "remote:logger", "config": { "who": who } })
            }
            Language::Go => json!({
                "id": "log",
                "name": "remote:logger",
                "config": {
                    "who": who,
                    "log": self.log,
                    "stop": self.stop_gate,
                    "hold": self.hold_gate,
                }
            }),
            Language::Kotlin => json!({
                "id": "log",
                "name": "remote:kt_lease",
                "config": {
                    "who": who,
                    "log": self.log,
                    "stop": self.stop_gate,
                    "hold": self.hold_gate,
                }
            }),
            #[cfg(unix)]
            Language::Bun => json!({
                "id": "log",
                "name": "remote:./logger.ts",
                "config": {
                    "who": who,
                    "log": self.log,
                    "stop": self.stop_gate,
                    "hold": self.hold_gate,
                }
            }),
        };
        let config: HostConfig = serde_json::from_value(json!({
            "id": "main",
            "rows": [
                { "id": "remote", "name": "rutis-host/dial", "config": { "address": self.address } },
                row
            ]
        }))
        .unwrap();
        let host = Host::start(&config).await.unwrap();
        host.load(config.rows()).await.unwrap();
        Controller { host }
    }
}

/// One controller of the runtime: a rutis host whose `remote:logger` row
/// runs `logger` on the far end.
struct Controller {
    host: Host,
}

impl Controller {
    /// The service the lease's plugin provides, once the row is loaded.
    async fn service(&self) -> Arc<dyn HostDispatch> {
        COLD.eventually(|| self.host.service("log"), "the lease's log service")
            .await
    }

    /// What its plugin answers to `who`.
    async fn who(&self) -> Value {
        let service = self.service().await;
        let reply = service.invoke("who", json!([]).into()).unwrap();
        COLD.within("who", settle(reply))
            .await
            .unwrap()
            .json()
            .unwrap()
    }
}

/// A controller leaves: the runtime cleans its rows up (all of them, in
/// the log) and stays for the next controller, which gets a new lease and
/// has its rows loaded there.
async fn successive_controllers(language: Language) {
    let _environment = environment();
    std::env::set_var("RUTIS_TOKEN_REMOTE", TOKEN);
    let runtime = Runtime::start(language).await;

    let first = runtime.controller("a").await;
    assert_eq!(first.who().await, json!("a"));

    // The controller goes: the runtime cleans its lease up (the cleanup
    // may finish) and stays, waiting for the next controller.
    runtime.open_stop_gate();
    assert!(first.host.shut_down(HANG_GUARD).await.is_ok());
    runtime.wait_for("stop a").await;

    // The next controller gets a new lease: its rows load there, and its
    // service answers.
    let second = runtime.controller("b").await;
    assert_eq!(second.who().await, json!("b"));
    assert_eq!(
        runtime.lines(),
        ["start a", "cleanup a", "stop a", "start b"]
    );
}

/// A newer connection takes over a connected controller only once the old
/// lease is cleaned up; the old lease's call is refused (not answered by
/// the new lease), and the new lease's rows work.
async fn a_newer_controller_takes_over(language: Language) {
    let _environment = environment();
    std::env::set_var("RUTIS_TOKEN_REMOTE", TOKEN);
    let runtime = Runtime::start(language).await;

    let first = runtime.controller("a").await;
    assert_eq!(first.who().await, json!("a"));
    let service = first.service().await;
    // An instruction of the old lease, held on the far end.
    let held = tokio::spawn(async move {
        let reply = service.invoke("hold", json!([]).into())?;
        settle(reply).await.map(|value| value.json().unwrap())
    });
    runtime.wait_for("hold a").await;

    // A newer controller of the same id takes over while that call waits:
    // the runtime closes the old channel as replaced, and the old call is
    // refused, not answered by the new lease.
    let second = runtime.controller("b").await;
    let refused = COLD.within("the old lease's call", held).await.unwrap();
    let error = refused.expect_err("the old lease's call is answered by nobody");
    assert_eq!(error.code(), Some("peer-gone"), "{error}");
    assert_eq!(
        error
            .data()
            .as_ref()
            .and_then(|data| data["reason"].as_str()),
        Some("replaced by a new connection"),
        "{error}"
    );
    // The old controller stops dialing at once: its retry would take over
    // in turn half a second later.
    assert!(first.host.shut_down(HANG_GUARD).await.is_ok());

    // The old lease's cleanup is still held while the new connection is
    // already there: the runtime greets it only once the cleanup is done.
    runtime.wait_for("cleanup a").await;
    runtime.open_stop_gate();
    runtime.wait_for("stop a").await;
    assert_eq!(second.who().await, json!("b"));
    // The old lease goes (`stop a`) before the new one starts (`start b`):
    // the new connection waited for the cleanup, which the gate held.
    let lines = runtime.lines();
    assert_eq!(
        lines,
        ["start a", "hold a", "cleanup a", "stop a", "start b"]
    );
    // The held call's answer crossed to nothing: the new lease is b's, and
    // the old controller's result never became a row here.
    assert!(
        !lines.iter().any(|line| line.starts_with("held")),
        "{lines:?}"
    );
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn successive_controllers_each_get_a_clean_lease() {
    successive_controllers(Language::Python).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn go_controllers_each_get_a_clean_lease() {
    if !Language::Go.enabled() {
        return;
    }
    successive_controllers(Language::Go).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_controller_takes_over_after_the_old_lease_and_the_old_call_is_refused() {
    a_newer_controller_takes_over(Language::Python).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_go_controller_takes_over_after_the_old_lease_and_the_old_call_is_refused() {
    if !Language::Go.enabled() {
        return;
    }
    a_newer_controller_takes_over(Language::Go).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_controllers_each_get_a_clean_lease() {
    if !Language::Bun.enabled() {
        return;
    }
    successive_controllers(Language::Bun).await;
}

/// guarantee: loader/remote-runtime-lease
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_bun_controller_takes_over_after_the_old_lease_and_the_old_call_is_refused() {
    if !Language::Bun.enabled() {
        return;
    }
    a_newer_controller_takes_over(Language::Bun).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn kotlin_controllers_each_get_a_clean_lease() {
    if !Language::Kotlin.enabled() {
        return;
    }
    successive_controllers(Language::Kotlin).await;
}

/// guarantee: loader/remote-runtime-lease
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_kotlin_controller_takes_over_after_the_old_lease_and_the_old_call_is_refused() {
    if !Language::Kotlin.enabled() {
        return;
    }
    a_newer_controller_takes_over(Language::Kotlin).await;
}
