//! A host keeping third-party plugins apart
//! (`docs/design-host-plugin-limits-2026-10-27.md`): one Node process per
//! plugin, each a runtime of its own (`<name>:<file>` rows), and a runtime waiting for a
//! debugger as `Loader::status` shows it.
#![cfg(feature = "runtimes")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::runtimes::{greet_as_main, layer, repo, TestRuntime, COLD};
use rutis::host::CtxHost;
use rutis::host::FiberState;
use rutis::Ctx;
use rutis_bridge::session::{host_key, HostDispatch, Value as RpcValue};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::status::{RuntimeState, Status};
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin};
use serde_json::json;

/// A host with the runtimes `runtimes` (`<name>:<file>` rows, one
/// `SessionResolver` for all) and a loader.
struct Host {
    root: Ctx,
    loader: Loader,
    dir: tempfile::TempDir,
}

/// `started`: wait for each runtime to run before the loader is mounted.
async fn host(runtimes: Vec<TestRuntime>, started: bool) -> Host {
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    let options = LoaderOptions::default();
    let sessions = Arc::new(SessionResolver::new(&options.catalog));
    for runtime in runtimes {
        sessions.runtime(runtime.handle());
        let view = root.plugin(runtime);
        if started {
            view.await.unwrap();
        }
    }
    let plugin = LoaderPlugin::new(Chain::new().with_shared(sessions.clone()), options);
    let loader = plugin.handle();
    sessions.attach(&loader);
    root.plugin(plugin).await.unwrap();
    Host {
        root,
        loader,
        dir: tempfile::tempdir().unwrap(),
    }
}

/// A Node runtime of its own, named `name`.
fn node(name: &str) -> TestRuntime {
    TestRuntime::node(&repo().join("node/rutis-runtime/package.json")).named(name)
}

impl Host {
    /// Write a plugin, importing the SDK as `PLUGIN`; its absolute path.
    fn plugin(&self, file: &str, text: &str) -> PathBuf {
        let sdk = repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap();
        let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
        let path = self.dir.path().join(file);
        std::fs::write(&path, text.replace("PLUGIN", &sdk)).unwrap();
        path
    }

    async fn rows(&self, rows: &[(&str, &str, &Path)]) {
        let rows = rows
            .iter()
            .map(|(id, runtime, file)| {
                json!({ "id": id, "name": format!("{runtime}:{}", file.display()) })
            })
            .collect();
        self.loader.reconcile(layer(rows), None).await.unwrap();
    }

    /// The status once `until` holds of it.
    async fn status(&self, what: &str, until: impl Fn(&Status) -> bool) -> Status {
        COLD.eventually_with(
            || {
                let status = self.loader.status();
                until(&status).then_some(status)
            },
            what,
            || self.loader.status().to_string(),
        )
        .await
    }

    async fn service(&self, name: &str) -> Arc<dyn HostDispatch> {
        let key = host_key(name);
        COLD.eventually(|| self.root.get_at::<dyn HostDispatch>(&key), name)
            .await
    }
}

/// The line of `status` that starts with `start`.
fn line<'a>(status: &'a str, start: &str) -> &'a str {
    status
        .lines()
        .find(|line| line.starts_with(start))
        .unwrap_or_else(|| panic!("no line {start:?} in:\n{status}"))
}

/// Whether row `id` runs, as `status` says.
fn runs(status: &Status, id: &str) -> bool {
    status.rows.iter().any(|row| {
        row.id == id
            && matches!(&row.state, EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active)
    })
}

fn runtime_of<'a>(status: &'a Status, name: &str) -> &'a rutis_loader::status::Runtime {
    status
        .runtimes
        .iter()
        .find(|runtime| runtime.name == name)
        .unwrap_or_else(|| panic!("no runtime {name}"))
}

const PING: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { NAME: { ping: 'sync' } },
  apply(ctx) { ctx.provide('NAME', { ping: () => 'NAME pong' }) },
})
"#;

/// Two Node runtimes, each a process of its own: rows go to
/// the process their prefix names; one process ending leaves the other's
/// row running.
// guarantee: host/plugin-limits (a process per plugin)
#[tokio::test(flavor = "multi_thread")]
async fn each_plugin_runs_in_its_own_process() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let host = host(vec![node("alpha"), node("beta")], true).await;
    let a = host.plugin("a.mjs", &PING.replace("NAME", "a"));
    let b = host.plugin("b.mjs", &PING.replace("NAME", "b"));
    host.rows(&[("a", "alpha", &a), ("b", "beta", &b)]).await;
    let status = host
        .status("both rows running", |status| {
            runs(status, "a") && runs(status, "b")
        })
        .await;
    let (alpha, beta) = (runtime_of(&status, "alpha"), runtime_of(&status, "beta"));
    let (alpha_pid, beta_pid) = (alpha.pid.unwrap(), beta.pid.unwrap());
    assert_ne!(alpha_pid, beta_pid);

    kill(alpha_pid);
    host.status("alpha down", |status| {
        runtime_of(status, "alpha").state == RuntimeState::Down
    })
    .await;
    let b = host.service("b").await;
    assert_eq!(
        b.invoke("ping", RpcValue::List(Vec::new()))
            .unwrap()
            .json()
            .unwrap(),
        json!("b pong")
    );
    let status = host.loader.status();
    assert!(runs(&status, "b"), "{status}");
    assert_eq!(runtime_of(&status, "beta").pid, Some(beta_pid));
    drop(b);
    host.root.shutdown().await.unwrap();
}

/// §五 5: a runtime stopped at its first line for a debugger, with no
/// greeting limit, is starting, and its row waits for it.
// guarantee: host/plugin-limits (greeting(Duration::MAX) in the status)
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_waiting_for_a_debugger_stays_starting() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let runtime = node("wordcount")
        .launcher(|launcher| launcher.env("NODE_OPTIONS", "--inspect-brk=127.0.0.1:0"))
        .greeting(Duration::MAX);
    let host = host(vec![runtime], false).await;
    let ping = host.plugin("ping.mjs", &PING.replace("NAME", "ping"));
    host.rows(&[("wordcount", "wordcount", &ping)]).await;
    // Past the time Node takes to greet when nothing stops it, and past a
    // session's own 10 s handshake.
    tokio::time::sleep(Duration::from_secs(12)).await;
    let status = host.loader.status().to_string();
    // With its process id, to attach the debugger to. On Windows a local
    // runtime's process is known only once it connected back (loopback
    // handover), so a process that has not greeted shows none.
    let expected = match cfg!(windows) {
        false => {
            let pid = runtime_of(&host.loader.status(), "wordcount").pid;
            format!("  wordcount: starting (pid {})", pid.expect("a pid"))
        }
        true => "  wordcount: starting".to_string(),
    };
    assert_eq!(line(&status, "  wordcount: starting"), expected);
    assert_eq!(
        line(&status, "  wordcount: waiting"),
        "  wordcount: waiting for runtime wordcount (starting)"
    );
    host.root.shutdown().await.unwrap();
}

#[cfg(unix)]
fn kill(pid: u32) {
    // SAFETY: plain kill(2) on the runtime's process.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0, "kill");
}

#[cfg(windows)]
fn kill(pid: u32) {
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "taskkill");
}
