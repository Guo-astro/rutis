//! A host keeping third-party plugins in check
//! (`docs/design-host-plugin-limits-2026-10-27.md`): one Node process per
//! plugin with `RuntimeResolver::modules`, the services a runtime's plugins
//! may use, and what a developer reads for each case of its §五, word for
//! word.
#![cfg(feature = "node")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::runtimes::{layer, repo, COLD};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, settle, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::status::{RowState, RuntimeState, Status};
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin};
use serde_json::{json, Value};

const LIMIT: Duration = Duration::from_millis(500);

/// A host service answering `name()` with its own name.
struct Named(&'static str);

impl HostDispatch for Named {
    fn invoke(&self, _method: &str, _args: RpcValue) -> Reply {
        Ok(json!(self.0).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "name": "sync" }))
    }
}

/// A host with the services `tools`, `notes` and `llm`, the runtimes
/// `runtimes` (each with its own resolver, `<name>:<file>` rows) and a
/// loader.
struct Host {
    root: Ctx,
    loader: Loader,
    dir: tempfile::TempDir,
}

/// `started`: wait for each runtime to run before the loader is mounted.
async fn host(runtimes: Vec<LocalRuntime>, started: bool) -> Host {
    let root = Ctx::root().unwrap();
    for name in ["tools", "notes", "llm"] {
        root.provide_at(host_key(name), Arc::new(Named(name)))
            .unwrap();
    }
    let mut chain = Chain::new();
    let mut resolvers = Vec::new();
    for runtime in runtimes {
        let rows = Arc::new(RuntimeResolver::modules(runtime.handle()));
        let view = root.plugin(runtime);
        if started {
            view.await.unwrap();
        }
        chain = chain.with_shared(rows.clone());
        resolvers.push(rows);
    }
    let plugin = LoaderPlugin::new(chain, LoaderOptions::default());
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    for rows in resolvers {
        root.plugin(RuntimeRowsPlugin::new(rows));
    }
    Host {
        root,
        loader,
        dir: tempfile::tempdir().unwrap(),
    }
}

/// A Node runtime of its own, named `name`.
fn node(name: &str) -> LocalRuntime {
    let package = repo().join("node/rutis-runtime");
    LocalRuntime::node(&package, package.join("package.json")).named(name)
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

fn row_state(status: &Status, id: &str) -> Option<RowState> {
    status
        .rows
        .iter()
        .find(|row| row.id == id)
        .map(|row| row.state)
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

/// Two Node runtimes, each a process of its own with `modules`: rows go to
/// the process their prefix names; one process ending leaves the other's
/// row running.
// guarantee: host/plugin-limits (a process per plugin with modules)
#[tokio::test(flavor = "multi_thread")]
async fn each_plugin_runs_in_its_own_process() {
    let host = host(vec![node("alpha"), node("beta")], true).await;
    let a = host.plugin("a.mjs", &PING.replace("NAME", "a"));
    let b = host.plugin("b.mjs", &PING.replace("NAME", "b"));
    host.rows(&[("a", "alpha", &a), ("b", "beta", &b)]).await;
    let status = host
        .status("both rows running", |status| {
            row_state(status, "a") == Some(RowState::Running)
                && row_state(status, "b") == Some(RowState::Running)
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
    assert_eq!(row_state(&status, "b"), Some(RowState::Running));
    assert_eq!(runtime_of(&status, "beta").pid, Some(beta_pid));
    drop(b);
    host.root.shutdown().await.unwrap();
}

/// Uses `tools` and `llm`.
const WORDCOUNT: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['tools', 'llm'],
  apply(ctx) {},
})
"#;

/// Uses `tools` and `notes`, provides `counter`.
const ALLOWED: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['tools', 'notes'],
  provides: { counter: { count: 'sync' } },
  apply(ctx) { ctx.provide('counter', { count: () => 1 }) },
})
"#;

/// Uses `counter`, which a row of its own runtime provides.
const NEIGHBOUR: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['counter'],
  apply(ctx) {},
})
"#;

/// §五 3: the services on the list run; one from outside that is not on it
/// fails the row, saying so; one a row of the same runtime provides is not
/// limited, whichever row comes first.
// guarantee: host/plugin-limits (allowed_services)
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_uses_only_the_services_it_is_allowed() {
    let host = host(
        vec![node("wordcount").allowed_services(["tools", "notes"])],
        true,
    )
    .await;
    let wordcount = host.plugin("wordcount.mjs", WORDCOUNT);
    let allowed = host.plugin("allowed.mjs", ALLOWED);
    let neighbour = host.plugin("neighbour.mjs", NEIGHBOUR);
    host.rows(&[
        ("neighbour", "wordcount", &neighbour),
        ("wordcount", "wordcount", &wordcount),
        ("allowed", "wordcount", &allowed),
    ])
    .await;
    let status = host
        .status("each row settled", |status| {
            row_state(status, "wordcount") == Some(RowState::Failed)
                && row_state(status, "allowed") == Some(RowState::Running)
                && row_state(status, "neighbour") == Some(RowState::Running)
        })
        .await
        .to_string();
    assert_eq!(
        line(&status, "  wordcount: uses"),
        "  wordcount: uses llm, which the host does not let runtime wordcount use \
         (allowed: tools, notes)"
    );
    host.root.shutdown().await.unwrap();
}

/// Provides `weather`, whose `tomorrow` never finishes.
const WEATHER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { weather: { tomorrow: 'async' } },
  apply(ctx) { ctx.provide('weather', { tomorrow: () => new Promise(() => {}) }) },
})
"#;

/// Calls `weather.tomorrow` while it applies.
const CHAT: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['weather'],
  async apply(ctx) { await ctx.use('weather').tomorrow() },
})
"#;

/// §五 1, by name: a plugin of another runtime calling a service of a
/// limited one reads which call ran out of whose limit.
// guarantee: host/plugin-limits (call_limit by name, from another runtime)
#[tokio::test(flavor = "multi_thread")]
async fn a_call_by_name_over_the_limit_says_whose_limit() {
    let host = host(vec![node("agent").call_limit(LIMIT), node("chat")], true).await;
    let weather = host.plugin("weather.mjs", WEATHER);
    let chat = host.plugin("chat.mjs", CHAT);
    host.rows(&[("weather", "agent", &weather), ("chat", "chat", &chat)])
        .await;
    let status = host
        .status("chat failed", |status| {
            row_state(status, "chat") == Some(RowState::Failed)
        })
        .await
        .to_string();
    assert_eq!(
        line(&status, "  chat: "),
        "  chat: call to weather.tomorrow did not finish within 500ms (the host's limit \
         for runtime agent); the call was cancelled"
    );
    // The runtime called is not ended without `kill_on_timeout`.
    assert!(line(&status, "  agent: ").starts_with("  agent: running (pid "));
    host.root.shutdown().await.unwrap();
}

/// Hands out a tool handler stuck in synchronous code.
const TOOL: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { tool: { handler: 'sync' } },
  apply(ctx) { ctx.provide('tool', { handler: () => () => { for (;;) {} } }) },
})
"#;

/// §五 1 and 2, by reference: the host calls a function the plugin handed
/// over; with `kill_on_timeout` its runtime is ended, and its status says
/// why.
// guarantee: host/plugin-limits (call_limit by reference, kill_on_timeout)
#[tokio::test(flavor = "multi_thread")]
async fn a_function_over_the_limit_ends_its_runtime() {
    let host = host(
        vec![node("wordcount").call_limit(LIMIT).kill_on_timeout()],
        true,
    )
    .await;
    let tool = host.plugin("tool.mjs", TOOL);
    host.rows(&[("wordcount", "wordcount", &tool)]).await;
    let service = host.service("tool").await;
    let handler = {
        let service = service.clone();
        tokio::task::spawn_blocking(move || {
            service
                .invoke("handler", RpcValue::List(Vec::new()))
                .unwrap()
        })
        .await
        .unwrap()
    };
    let RpcValue::Reference(function) = handler else {
        panic!("a function: {handler:?}");
    };
    let error = async { settle(function.call_async(RpcValue::List(Vec::new())).await?).await }
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "a function of wordcount did not finish within 500ms (the host's limit for runtime \
         wordcount); the call was cancelled; runtime wordcount was ended"
    );
    assert_eq!(error.code(), Some("timeout"));
    let status = host
        .status("the runtime down", |status| {
            runtime_of(status, "wordcount").state == RuntimeState::Down
        })
        .await
        .to_string();
    assert_eq!(
        line(&status, "  wordcount: down"),
        "  wordcount: down (ended by the host: a function of wordcount did not finish \
         within 500ms)"
    );
    drop((service, function));
    host.root.shutdown().await.unwrap();
}

/// §五 5: a runtime stopped at its first line for a debugger, with no
/// greeting limit, is starting, and its row waits for it.
// guarantee: host/plugin-limits (greeting(Duration::MAX) in the status)
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_waiting_for_a_debugger_stays_starting() {
    let runtime = node("wordcount")
        .env("NODE_OPTIONS", "--inspect-brk=127.0.0.1:0")
        .greeting(Duration::MAX);
    let host = host(vec![runtime], false).await;
    let tool = host.plugin("tool.mjs", TOOL);
    host.rows(&[("wordcount", "wordcount", &tool)]).await;
    // Well past the time Node takes to greet when nothing stops it.
    tokio::time::sleep(Duration::from_secs(3)).await;
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
