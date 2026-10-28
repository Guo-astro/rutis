//! The host's limits on a local runtime
//! (`docs/design-host-plugin-limits-2026-10-27.md` §4.2, §4.4): a call into
//! it over `call_limit` is cancelled and fails with code `timeout`; with
//! `kill_on_timeout` its process is ended too, and other runtimes go on; a
//! function it passed by reference is limited the same way; `greeting`
//! bounds, or unbounds, how long its process may take to greet.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::{row_projection, LocalRuntime, Process, RuntimeState};
use rutis_bridge::session::{host_key, settle, HostDispatch, Value as RpcValue};
use rutis_test_support::{until, within};
use serde_json::{json, Value};

const LIMIT: Duration = Duration::from_millis(500);

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// Load `entry` as row `key` into the runtime `process` runs, its services
/// published on `ctx`; the service `name` once it is there.
async fn load(
    process: &Arc<Process>,
    ctx: &Ctx,
    key: &str,
    entry: &Path,
    name: &str,
) -> Arc<dyn HostDispatch> {
    let described = process.describe_row(entry).await.unwrap();
    let projection = row_projection(&described.provides);
    projection.attach(ctx, process.clone()).unwrap();
    process
        .load_row_exporting(
            key,
            entry,
            json!({}),
            &[],
            &[],
            &described.provides,
            projection,
            &[],
        )
        .await
        .unwrap();
    let key = host_key(name);
    until(|| ctx.get_at::<dyn HostDispatch>(&key).is_some(), name).await;
    ctx.get_at::<dyn HostDispatch>(&key).unwrap()
}

/// A synchronous call, on a thread that may block.
async fn call_sync(service: &Arc<dyn HostDispatch>, method: &str) -> Result<Value, String> {
    let service = service.clone();
    let method = method.to_owned();
    tokio::task::spawn_blocking(move || {
        service
            .invoke(&method, RpcValue::List(Vec::new()))
            .and_then(|value| value.json())
            .map_err(|error| error.to_string())
    })
    .await
    .unwrap()
}

/// The code and data of a call's error.
fn code_and_data(error: &rutis_bridge::session::Error) -> (Option<&str>, Option<Value>) {
    (error.code(), error.data())
}

/// What a host does with each kind of plugin, the same for every language.
struct Plugin {
    /// The runtime, not yet named or limited.
    runtime: fn(&Path) -> LocalRuntime,
    /// Its row's entry, written in the project.
    entry: fn(&Path) -> PathBuf,
}

#[cfg(feature = "node")]
mod node {
    use super::*;

    /// `wait` runs until cancelled, `spin` never returns and holds the
    /// whole process, `handler` returns a function that never finishes.
    const SLOW: &str = r#"
export function apply(ctx) {
  let cancelled = false
  ctx.provide('slow', {
    async wait(signal) {
      await new Promise((resolve, reject) => {
        signal.addEventListener('abort', () => { cancelled = true; reject(signal.reason) })
      })
    },
    spin() { for (;;) {} },
    cancelled() { return cancelled },
    ping() { return 'pong' },
    handler() { return async () => new Promise(() => {}) },
  })
}
"#;

    const PACKAGE: &str = r#"{ "rutis": { "provides": { "slow": {
  "wait": "async", "spin": "sync", "cancelled": "sync", "ping": "sync", "handler": "sync"
} } } }"#;

    pub(super) fn plugin() -> Plugin {
        Plugin {
            runtime: |project| {
                LocalRuntime::node(
                    repo().join("node/rutis-runtime"),
                    project.join("package.json"),
                )
            },
            entry: |project| {
                write(project, "package.json", "{}");
                write(project, "slow/package.json", PACKAGE);
                write(project, "slow/index.mjs", SLOW)
            },
        }
    }

    // guarantee: host/plugin-limits (an async call over the limit is cancelled)
    #[tokio::test(flavor = "multi_thread")]
    async fn an_async_call_over_the_limit_is_cancelled() {
        super::an_async_call_over_the_limit_is_cancelled(plugin(), true).await;
    }

    // guarantee: host/plugin-limits (kill_on_timeout ends only that runtime)
    #[tokio::test(flavor = "multi_thread")]
    async fn a_busy_runtime_is_ended_and_others_go_on() {
        super::a_busy_runtime_is_ended_and_others_go_on(plugin()).await;
    }

    // guarantee: host/plugin-limits (a function passed by reference is limited)
    #[tokio::test(flavor = "multi_thread")]
    async fn a_function_over_the_limit_is_cancelled() {
        super::a_function_over_the_limit_is_cancelled(plugin()).await;
    }
}

#[cfg(feature = "python")]
mod python {
    use super::*;

    const SLOW: &str = r#"
import asyncio


class Slow:
    def __init__(self):
        self.was_cancelled = False

    async def wait(self):
        try:
            await asyncio.sleep(3600)
        except asyncio.CancelledError:
            self.was_cancelled = True
            raise

    def spin(self):
        while True:
            pass

    def cancelled(self):
        return self.was_cancelled

    def ping(self):
        return "pong"

    def handler(self):
        async def never():
            await asyncio.sleep(3600)
        return never


provides = {"slow": Slow}


def apply(ctx, config):
    ctx.provide("slow", Slow())
"#;

    /// The Python interpreter: `RUTIS_PYTHON`, else `python3` (`python` on
    /// Windows).
    fn python3() -> String {
        std::env::var("RUTIS_PYTHON")
            .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
    }

    pub(super) fn plugin() -> Plugin {
        Plugin {
            runtime: |project| {
                LocalRuntime::python(project)
                    .python_path(repo().join("python/rutis"))
                    .interpreter(python3())
            },
            entry: |project| {
                write(project, "slow_plugin.py", SLOW);
                PathBuf::from("slow_plugin")
            },
        }
    }

    // guarantee: host/plugin-limits (an async call over the limit is cancelled)
    #[tokio::test(flavor = "multi_thread")]
    async fn an_async_call_over_the_limit_is_cancelled() {
        super::an_async_call_over_the_limit_is_cancelled(plugin(), false).await;
    }

    // guarantee: host/plugin-limits (kill_on_timeout ends only that runtime)
    #[tokio::test(flavor = "multi_thread")]
    async fn a_busy_runtime_is_ended_and_others_go_on() {
        super::a_busy_runtime_is_ended_and_others_go_on(plugin()).await;
    }

    // guarantee: host/plugin-limits (a function passed by reference is limited)
    #[tokio::test(flavor = "multi_thread")]
    async fn a_function_over_the_limit_is_cancelled() {
        super::a_function_over_the_limit_is_cancelled(plugin()).await;
    }
}

/// An async method still running at the limit: its caller gets `timeout`,
/// the method is cancelled, and the runtime goes on. `signal`: the method
/// takes an AbortSignal (TypeScript) rather than being cancelled itself.
#[allow(dead_code)]
async fn an_async_call_over_the_limit_is_cancelled(plugin: Plugin, signal: bool) {
    within("an async call over the limit", async {
        let dir = tempfile::tempdir().unwrap();
        let entry = (plugin.entry)(dir.path());
        let runtime = (plugin.runtime)(dir.path()).named("slowpoke").call_limit(LIMIT);
        let handle = runtime.handle();
        let root = Ctx::root().unwrap();
        let view = root.plugin(runtime);
        (&view).await.unwrap();
        let process = handle.ready().await.unwrap();
        let slow = load(&process, &root, "slow", &entry, "slow").await;

        let args = match signal {
            true => vec![RpcValue::Signal],
            false => Vec::new(),
        };
        let started = std::time::Instant::now();
        let error = settle(slow.invoke("wait", RpcValue::List(args)).unwrap())
            .await
            .unwrap_err();
        assert!(started.elapsed() >= LIMIT);
        assert_eq!(
            error.to_string(),
            "call to slow.wait did not finish within 500ms (the host's limit for runtime \
             slowpoke); the call was cancelled"
        );
        assert_eq!(
            code_and_data(&error),
            (
                Some("timeout"),
                Some(json!({ "runtime": "slowpoke", "limit": 500, "service": "slow", "method": "wait" }))
            )
        );
        // The cancellation reached the method; the runtime still answers.
        until(
            || {
                slow.invoke("cancelled", RpcValue::List(Vec::new()))
                    .unwrap()
                    .json()
                    .unwrap()
                    == json!(true)
            },
            "the method cancelled",
        )
        .await;
        assert!(matches!(handle.state(), RuntimeState::Ready(_)));
        drop(slow);
        root.shutdown().await.unwrap();
    })
    .await;
}

/// A synchronous method that never returns holds the whole process: with
/// `kill_on_timeout` the process is ended, its runtime is down saying why,
/// another runtime answers as before, and a restart brings it back.
#[allow(dead_code)]
async fn a_busy_runtime_is_ended_and_others_go_on(plugin: Plugin) {
    within("a busy runtime ended", async {
        let dir = tempfile::tempdir().unwrap();
        let entry = (plugin.entry)(dir.path());
        let spinner = (plugin.runtime)(dir.path())
            .named("spinner")
            .call_limit(LIMIT)
            .kill_on_timeout();
        let steady = (plugin.runtime)(dir.path()).named("steady");
        let (spinning, other) = (spinner.handle(), steady.handle());
        let root = Ctx::root().unwrap();
        let view = root.plugin(spinner);
        (&view).await.unwrap();
        (&root.plugin(steady)).await.unwrap();
        let process = spinning.ready().await.unwrap();
        // Each runtime's service published apart: both are `slow`.
        let (busy, calm) = (Ctx::root().unwrap(), Ctx::root().unwrap());
        let slow = load(&process, &busy, "slow", &entry, "slow").await;
        let other_process = other.ready().await.unwrap();
        let steady_slow = load(&other_process, &calm, "slow", &entry, "slow").await;

        assert_eq!(
            call_sync(&slow, "spin").await.unwrap_err(),
            "call to slow.spin did not finish within 500ms (the host's limit for runtime \
             spinner); the call was cancelled; runtime spinner was ended"
        );
        let down = spinning
            .wait_for(|state| matches!(state, RuntimeState::Down(_)))
            .await;
        let RuntimeState::Down(why) = down else {
            unreachable!()
        };
        assert_eq!(
            why,
            "ended by the host: call to slow.spin did not finish within 500ms"
        );
        assert_eq!(
            call_sync(&steady_slow, "ping").await.unwrap(),
            json!("pong")
        );
        assert!(matches!(other.state(), RuntimeState::Ready(_)));

        // The host restarts it: a new process, under the same limit.
        drop(slow);
        view.restart().await.unwrap();
        let process = spinning.ready().await.unwrap();
        let slow = load(&process, &busy, "slow", &entry, "slow").await;
        assert_eq!(call_sync(&slow, "ping").await.unwrap(), json!("pong"));
        drop((slow, steady_slow));
        root.shutdown().await.unwrap();
        busy.shutdown().await.unwrap();
        calm.shutdown().await.unwrap();
    })
    .await;
}

/// A function a plugin passed by reference, called by the host, still
/// running at the limit: `a function of …` fails with `timeout`.
#[allow(dead_code)]
async fn a_function_over_the_limit_is_cancelled(plugin: Plugin) {
    within("a function over the limit", async {
        let dir = tempfile::tempdir().unwrap();
        let entry = (plugin.entry)(dir.path());
        let runtime = (plugin.runtime)(dir.path())
            .named("wordcount")
            .call_limit(LIMIT);
        let handle = runtime.handle();
        let root = Ctx::root().unwrap();
        (&root.plugin(runtime)).await.unwrap();
        let process = handle.ready().await.unwrap();
        let slow = load(&process, &root, "slow", &entry, "slow").await;

        let handler = slow.invoke("handler", RpcValue::List(Vec::new())).unwrap();
        let RpcValue::Reference(function) = handler else {
            panic!("a function: {handler:?}");
        };
        let error = async { settle(function.call_async(RpcValue::List(Vec::new())).await?).await }
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "a function of wordcount did not finish within 500ms (the host's limit for \
             runtime wordcount); the call was cancelled"
        );
        assert_eq!(
            code_and_data(&error),
            (
                Some("timeout"),
                Some(json!({ "runtime": "wordcount", "limit": 500 }))
            )
        );
        assert_eq!(call_sync(&slow, "ping").await.unwrap(), json!("pong"));
        drop((slow, function));
        root.shutdown().await.unwrap();
    })
    .await;
}

/// A process that never greets: with `greeting(1s)` the runtime is down
/// within a few seconds, not the default 30.
// guarantee: host/plugin-limits (greeting bounds the start)
#[cfg(unix)]
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
#[cfg(unix)]
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
