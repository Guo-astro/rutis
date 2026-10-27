//! Changing the configuration of a running host (#274): `Host::reload`,
//! all or nothing, and what triggers it in `rutis-host run` (SIGUSR1 on
//! Unix, `--watch-config` everywhere).
//!
//! guarantee: host/reload-config — rows added, removed and changed apply;
//! a row that would fail, or fails, leaves the old rows running untouched
//! (no cleanup ran: risk B4); what needs a restart is refused; a removed
//! local runtime stops after its rows; a peer row a reload adds presents
//! its token.

// The tests hold the environment until they end, across their awaits: the
// variables they set are for the hosts they start.
#![allow(clippy::await_holding_lock)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use common::{environment, repo};
use rutis_bridge::session::{settle, Value as CallValue};
use rutis_bridge::LinkState;
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{scaled, Guard, HANG_GUARD};
use serde_json::{json, Value};

/// Twice HANG_GUARD: the Python runtime starts cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// A Python plugin providing `service`, answering `<word>, <name>`; it
/// records each cleanup in `<out>/<row>.cleanup`; with `fail`, its apply
/// raises.
const GREETER: &str = r#"
import os

provides = {"SERVICE": {"hello": "sync"}}


class Greeter:
    def __init__(self, word):
        self.word = word

    def hello(self, name):
        return self.word + ", " + name


def apply(ctx, config):
    if config.get("fail"):
        raise RuntimeError("told to fail")
    ctx.provide("SERVICE", Greeter(config.get("word", "Hello")))

    def cleanup():
        with open(os.path.join(config["out"], config["row"] + ".cleanup"), "a") as f:
            f.write("cleanup\n")

    ctx.effect(cleanup)
"#;

/// The plugin modules in `dir`: `greeter`, `welcomer` and `spare`, each
/// providing the service of its name.
fn modules(dir: &Path) {
    for service in ["greeter", "welcomer", "spare"] {
        std::fs::write(
            dir.join(format!("{service}.py")),
            GREETER.replace("SERVICE", service),
        )
        .unwrap();
    }
}

/// A configuration of `rows` on a Python runtime in `dir`, with `more`
/// merged in at the top level.
fn config(dir: &Path, rows: Value, more: Value) -> HostConfig {
    let mut py = json!({ "project": dir });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let mut value = json!({ "id": "reload-test", "runtimes": { "py": py }, "rows": rows });
    for (key, field) in more.as_object().cloned().unwrap_or_default() {
        value[key] = field;
    }
    serde_json::from_value(value).unwrap()
}

/// The row `id` running the module of its name with `config` (and where
/// it records its cleanups).
fn greeter(dir: &Path, id: &str, config: Value) -> Value {
    let mut full = json!({ "out": dir, "row": id });
    for (key, field) in config.as_object().cloned().unwrap_or_default() {
        full[key] = field;
    }
    json!({ "id": id, "name": format!("py:{id}"), "config": full })
}

fn cleanups(dir: &Path, row: &str) -> usize {
    std::fs::read_to_string(dir.join(format!("{row}.cleanup")))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// What the service `service` answers to `hello("Ada")`, once it is there.
async fn hello(host: &Host, service: &str) -> String {
    let found = COLD
        .eventually(|| host.service(service), "the service")
        .await;
    let reply = found
        .invoke("hello", CallValue::List(vec![json!("Ada").into()]))
        .unwrap();
    settle(reply)
        .await
        .unwrap()
        .json()
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

/// A host running `config`, its rows loaded.
async fn started(config: &HostConfig) -> Host {
    std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
    let host = Host::start(config).await.unwrap();
    COLD.within("the Python runtime", host.runtimes_ready())
        .await
        .unwrap();
    host.load(config.rows()).await.unwrap();
    host
}

/// guarantee: host/reload-config
#[tokio::test(flavor = "multi_thread")]
async fn rows_added_removed_and_changed_apply() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    modules(d);
    let rows = json!([
        greeter(d, "greeter", json!({})),
        greeter(d, "spare", json!({}))
    ]);
    let mut host = started(&config(d, rows, json!({}))).await;
    assert_eq!(hello(&host, "greeter").await, "Hello, Ada");

    let rows = json!([
        greeter(d, "greeter", json!({ "word": "Welcome" })),
        greeter(d, "welcomer", json!({ "word": "Hi" })),
    ]);
    let reloaded = host.reload(config(d, rows, json!({}))).await.unwrap();
    assert_eq!(reloaded.to_string(), "3 rows changed");
    assert_eq!(hello(&host, "greeter").await, "Welcome, Ada");
    assert_eq!(hello(&host, "welcomer").await, "Hi, Ada");
    assert!(host.service("spare").is_none());
    // The changed row restarted, the removed one stopped: each cleanup once.
    assert_eq!(cleanups(d, "greeter"), 1);
    assert_eq!(cleanups(d, "spare"), 1);
    assert_eq!(cleanups(d, "welcomer"), 0);
    host.shut_down(HANG_GUARD).await.ok();
}

/// A row that does not resolve: refused before anything changes; no
/// row restarted, no cleanup ran (B4).
/// guarantee: host/reload-config
#[tokio::test(flavor = "multi_thread")]
async fn a_row_that_would_fail_changes_nothing() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    modules(d);
    let rows = json!([greeter(d, "greeter", json!({}))]);
    let mut host = started(&config(d, rows, json!({}))).await;
    assert_eq!(hello(&host, "greeter").await, "Hello, Ada");

    let rows = json!([
        greeter(d, "greeter", json!({ "word": "Welcome" })),
        { "id": "missing", "name": "py:no_such_module" }
    ]);
    let error = host.reload(config(d, rows, json!({}))).await.unwrap_err();
    assert!(error.starts_with("row missing: "), "{error}");
    assert_eq!(hello(&host, "greeter").await, "Hello, Ada");
    assert_eq!(cleanups(d, "greeter"), 0);
    assert!(host
        .loader
        .entries()
        .iter()
        .all(|entry| entry.id != "missing"));
    assert_eq!(host.config().rows.len(), 1);
    host.shut_down(HANG_GUARD).await.ok();
}

/// The running copy of the row `id`: its plugin and that plugin's
/// generation (a restart changes either).
fn copy(host: &Host, id: &str) -> (Option<rutis::host::PluginId>, Option<u64>) {
    let entry = host
        .loader
        .entries()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("no row {id}"));
    let generation = entry.view.as_ref().map(|view| view.state().generation);
    (entry.plugin, generation)
}

/// A row whose apply fails passes the check, fails once loaded: the old
/// rows come back, the error says which row and why. A row the reload
/// does not change is not restarted.
/// guarantee: host/reload-config
#[tokio::test(flavor = "multi_thread")]
async fn a_row_failing_as_it_starts_brings_the_old_rows_back() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    modules(d);
    let rows = json!([
        greeter(d, "greeter", json!({})),
        greeter(d, "spare", json!({}))
    ]);
    let mut host = started(&config(d, rows.clone(), json!({}))).await;
    assert_eq!(hello(&host, "greeter").await, "Hello, Ada");
    assert_eq!(hello(&host, "spare").await, "Hello, Ada");
    let spare = copy(&host, "spare");
    assert!(spare.0.is_some() && spare.1.is_some(), "{spare:?}");

    let failing = json!([
        greeter(d, "greeter", json!({ "fail": true })),
        greeter(d, "spare", json!({}))
    ]);
    let error = host
        .reload(config(d, failing, json!({})))
        .await
        .unwrap_err();
    assert!(
        error.starts_with("row greeter: ") && error.contains("told to fail"),
        "{error}"
    );
    assert_eq!(hello(&host, "greeter").await, "Hello, Ada");
    assert_eq!(host.config().rows, rows.as_array().unwrap().clone());
    // The replaced row stopped once (for the copy that failed); the
    // unchanged one ran on, the same copy, no cleanup.
    assert_eq!(cleanups(d, "greeter"), 1);
    assert_eq!(cleanups(d, "spare"), 0);
    assert_eq!(copy(&host, "spare"), spare, "the unchanged row restarted");
    assert_eq!(hello(&host, "spare").await, "Hello, Ada");
    // A reload that applies after it is counted from the old rows.
    let reloaded = host.reload(config(d, rows, json!({}))).await.unwrap();
    assert_eq!(reloaded.to_string(), "0 rows changed");
    host.shut_down(HANG_GUARD).await.ok();
}

/// guarantee: host/reload-config
#[tokio::test(flavor = "multi_thread")]
async fn what_needs_a_restart_changes_nothing() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    modules(d);
    let rows = json!([greeter(d, "greeter", json!({}))]);
    let listen = |port: u16| json!({ "listen": [{ "name": "local", "address": format!("127.0.0.1:{port}") }] });
    let mut host = started(&config(d, rows, listen(0))).await;
    let rows = json!([greeter(d, "greeter", json!({ "word": "Welcome" }))]);
    let error = host.reload(config(d, rows, listen(1))).await.unwrap_err();
    assert_eq!(error, "listen changed: restart the host");
    assert_eq!(hello(&host, "greeter").await, "Hello, Ada");
    assert_eq!(cleanups(d, "greeter"), 0);
    host.shut_down(HANG_GUARD).await.ok();
}

/// The configuration no longer has the Python runtime: its rows unload,
/// then it stops, and its process has ended when the reload returns.
/// guarantee: host/reload-config
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_runtime_stops_after_its_rows() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    modules(d);
    let rows = json!([greeter(d, "greeter", json!({}))]);
    let mut host = started(&config(d, rows, json!({}))).await;
    assert_eq!(hello(&host, "greeter").await, "Hello, Ada");
    let handle = host.runtimes[0].1.clone();

    let empty: HostConfig = serde_json::from_value(json!({ "id": "reload-test" })).unwrap();
    let reloaded = host.reload(empty).await.unwrap();
    assert_eq!(reloaded.to_string(), "1 row changed, runtime py stopped");
    assert!(reloaded.warnings.is_empty(), "{:?}", reloaded.warnings);
    assert_eq!(cleanups(d, "greeter"), 1, "its row unloaded first");
    assert!(host.runtimes.is_empty());
    assert!(matches!(
        handle.state(),
        rutis_bridge::runtime::RuntimeState::Idle
    ));
    // Added back: a new runtime, which needs a restart.
    let rows = json!([greeter(d, "greeter", json!({}))]);
    let error = host.reload(config(d, rows, json!({}))).await.unwrap_err();
    assert_eq!(error, "runtimes.py added: restart the host");
    host.shut_down(HANG_GUARD).await.ok();
}

/// A removed runtime's process: written down by its row, killed, and
/// waited for (Unix: kill(2)).
#[cfg(unix)]
mod removed_process {
    use super::*;
    use common::process::{alive, file_url};
    use rutis_bridge::runtime::{RuntimeHandle, RuntimeState};

    /// A plugin writing its process's id to `<out>/<row>.pid`, and
    /// `<out>/<row>.cleanup` as it cleans up; with `linger`, its cleanup leaves a second of work to the loop's
    /// executor, which the runtime finishes before its process ends.
    const PID: &str = r#"
import asyncio
import os
import time


def apply(ctx, config):
    with open(os.path.join(config["out"], config["row"] + ".pid"), "w") as f:
        f.write(str(os.getpid()))

    def cleanup():
        with open(os.path.join(config["out"], config["row"] + ".cleanup"), "w") as f:
            f.write("cleanup\n")
        if config.get("linger"):
            asyncio.get_running_loop().run_in_executor(None, time.sleep, 1.0)

    ctx.effect(cleanup)
"#;

    /// The same in JavaScript, without `linger`.
    fn node_pid() -> String {
        let sdk = repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap();
        format!(
            "import {{ definePlugin }} from '{}'\n\
             import {{ writeFileSync }} from 'node:fs'\n\
             import {{ join }} from 'node:path'\n\
             export default definePlugin({{ apply(ctx, config) {{\n\
               writeFileSync(join(config.out, config.row + '.pid'), String(process.pid))\n\
             }} }})\n",
            file_url(&sdk)
        )
    }

    fn py(dir: &Path, restart: &str) -> Value {
        let mut py = json!({ "project": dir, "restart": restart });
        if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
            py["python"] = json!(python.to_str().unwrap());
        }
        py
    }

    fn row(dir: &Path, id: &str, name: &str, linger: bool) -> Value {
        json!({
            "id": id,
            "name": name,
            "config": { "out": dir, "row": id, "linger": linger },
        })
    }

    fn host_config(value: Value) -> HostConfig {
        let mut value = value;
        value["id"] = json!("reload-test");
        serde_json::from_value(value).unwrap()
    }

    /// The process id the row `row` wrote, once it has.
    async fn pid_of(dir: &Path, row: &str) -> i32 {
        let file = dir.join(format!("{row}.pid"));
        COLD.eventually(
            || {
                std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|text| text.trim().parse().ok())
            },
            "the row's process id",
        )
        .await
    }

    fn kill(pid: i32) {
        // SAFETY: plain kill(2).
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0, "kill");
    }

    /// Wait until `state` says what `want` looks for.
    async fn until(handle: &RuntimeHandle, what: &str, want: fn(&RuntimeState) -> bool) {
        COLD.within(what, async {
            handle.wait_for(want).await;
        })
        .await;
    }

    /// The Python runtime removed after its process ended (`restart: "no"`):
    /// nothing to wait for, so the reload says it stopped, at once.
    /// guarantee: host/reload-config
    #[tokio::test(flavor = "multi_thread")]
    async fn a_removed_runtime_that_was_down_stops_at_once() {
        let _environment = environment();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("pid.py"), PID).unwrap();
        let mut host = started(&host_config(json!({
            "runtimes": { "py": py(d, "no") },
            "rows": [row(d, "py-row", "py:pid", false)],
        })))
        .await;
        let handle = host.runtimes[0].1.clone();
        kill(pid_of(d, "py-row").await);
        until(&handle, "the runtime going down", |state| {
            matches!(state, RuntimeState::Down(_))
        })
        .await;

        let begun = std::time::Instant::now();
        let reloaded = host.reload(host_config(json!({}))).await.unwrap();
        assert!(
            begun.elapsed() < Duration::from_secs(5),
            "{:?}",
            begun.elapsed()
        );
        assert_eq!(reloaded.stopped, ["py"]);
        assert!(reloaded.warnings.is_empty(), "{:?}", reloaded.warnings);
        host.shut_down(HANG_GUARD).await.ok();
    }

    /// The Python runtime removed while its supervisor starts it again
    /// (`on-failure`): the process that restart started is waited for too,
    /// and has ended when the reload returns.
    /// guarantee: host/reload-config
    #[tokio::test(flavor = "multi_thread")]
    async fn a_removed_runtime_being_restarted_stops_with_its_new_process() {
        let _environment = environment();
        // Processes earlier tests started are left listed, their tests'
        // async runtimes gone: only those may be there at the end.
        let earlier = rutis_bridge::transport::local::running_processes();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("pid.py"), PID).unwrap();
        let mut host = started(&host_config(json!({
            "runtimes": { "py": py(d, "on-failure") },
            "rows": [row(d, "py-row", "py:pid", false)],
        })))
        .await;
        let _exits = host.supervise();
        let handle = host.runtimes[0].1.clone();
        kill(pid_of(d, "py-row").await);
        until(&handle, "the runtime going down", |state| {
            matches!(state, RuntimeState::Down(_))
        })
        .await;
        // The supervisor starts it again after its backoff (a second).
        until(&handle, "the restart", |state| {
            matches!(state, RuntimeState::Starting | RuntimeState::Ready(_))
        })
        .await;

        let begun = std::time::Instant::now();
        let reloaded = host.reload(host_config(json!({}))).await.unwrap();
        assert!(
            begun.elapsed() < Duration::from_secs(5),
            "{:?}",
            begun.elapsed()
        );
        assert_eq!(reloaded.stopped, ["py"]);
        assert!(reloaded.warnings.is_empty(), "{:?}", reloaded.warnings);
        let running = rutis_bridge::transport::local::running_processes();
        assert!(
            running.iter().all(|pid| earlier.contains(pid)),
            "a runtime process runs on: {running:?}"
        );
        host.shut_down(HANG_GUARD).await.ok();
    }

    /// Another runtime's process ending while the removed one's is still
    /// finishing: the reload waits for the removed runtime's own process,
    /// not for whichever ends first.
    /// guarantee: host/reload-config
    #[tokio::test(flavor = "multi_thread")]
    async fn a_removed_runtime_is_waited_for_while_another_one_ends() {
        let _environment = environment();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("pid.py"), PID).unwrap();
        std::fs::write(d.join("pid.mjs"), node_pid()).unwrap();
        std::fs::write(
            d.join("package.json"),
            r#"{ "name": "reload", "type": "module" }"#,
        )
        .unwrap();
        let node = json!({ "project": d, "runtime": repo().join("node/rutis-runtime") });
        let node_row = row(d, "node-row", d.join("pid.mjs").to_str().unwrap(), false);
        let mut host = started(&host_config(json!({
            "runtimes": { "py": py(d, "no"), "node": node },
            "rows": [row(d, "py-row", "py:pid", true), node_row],
        })))
        .await;
        let (python, node_pid) = (pid_of(d, "py-row").await, pid_of(d, "node-row").await);

        // While the Python process finishes the second its row's cleanup
        // left, the Node one is killed.
        let cleaned = d.join("py-row.cleanup");
        let killer = std::thread::spawn(move || {
            while !cleaned.exists() {
                std::thread::sleep(Duration::from_millis(10));
            }
            kill(node_pid);
        });
        let reloaded = host
            .reload(host_config(json!({
                "runtimes": { "node": node },
                "rows": [node_row],
            })))
            .await
            .unwrap();
        killer.join().unwrap();
        assert_eq!(reloaded.stopped, ["py"]);
        assert!(reloaded.warnings.is_empty(), "{:?}", reloaded.warnings);
        assert!(!alive(python), "the Python runtime's process runs on");
        host.shut_down(HANG_GUARD).await.ok();
    }
}

/// A reload adding a listening peer row on the listener a runtime dials in
/// to, both falling back to RUTIS_TOKEN, is refused, naming both and the
/// variable to set; nothing changes (#702).
/// guarantee: host/reload-config, host/credentials-from-env
#[tokio::test(flavor = "multi_thread")]
async fn a_reload_making_two_entries_accept_one_token_is_refused() {
    let _environment = environment();
    std::env::set_var("RUTIS_TOKEN", "shared");
    let phone: HostConfig = serde_json::from_value(json!({
        "id": "office",
        "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
        "runtimes": {
            "remote": [{ "name": "phone", "language": "python", "listen": "public", "instances": "phone" }]
        },
        "rows": [{ "id": "phone", "group": true, "instanced": true, "config": [] }]
    }))
    .unwrap();
    let mut host = Host::start(&phone).await.unwrap();
    host.load(phone.rows()).await.unwrap();
    let mut both = phone.clone();
    both.rows.push(
        json!({ "id": "main", "name": "rutis-bridge/peer", "config": { "peer": "main", "listen": "public" } }),
    );
    let error = host.reload(both).await.unwrap_err();
    assert_eq!(
        error,
        "listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a \
         connection presenting it cannot be told apart: set RUTIS_TOKEN_MAIN or \
         RUTIS_TOKEN_PHONE to a token of its own"
    );
    assert!(host.loader.entries().iter().all(|entry| entry.id != "main"));
    host.shut_down(HANG_GUARD).await.ok();
    std::env::remove_var("RUTIS_TOKEN");
}

/// A peer row a reload adds dials with the token the environment has for
/// its peer, and the far end accepts it; neither host had a peer row at
/// first.
/// guarantee: host/reload-config, host/credentials-from-env
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_row_a_reload_adds_presents_its_token() {
    let _environment = environment();
    std::env::set_var("RUTIS_TOKEN", "shared");
    let office: HostConfig = serde_json::from_value(json!({
        "id": "office",
        "listen": [{ "name": "local", "address": "127.0.0.1:0" }],
    }))
    .unwrap();
    let mut office_host = Host::start(&office).await.unwrap();
    office_host.load(office.rows()).await.unwrap();
    let address = office_host.listening[0].1;
    let main: HostConfig = serde_json::from_value(json!({ "id": "main" })).unwrap();
    let mut main_host = Host::start(&main).await.unwrap();
    main_host.load(main.rows()).await.unwrap();

    // The listening end first: its row names who may dial in.
    let mut accepting = office.clone();
    accepting.rows = vec![
        json!({ "id": "main", "name": "rutis-bridge/peer", "config": { "peer": "main", "listen": "local" } }),
    ];
    office_host.reload(accepting).await.unwrap();
    let mut dialing = main.clone();
    dialing.rows = vec![
        json!({ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": format!("ws://{address}/rutis") } }),
    ];
    main_host.reload(dialing).await.unwrap();

    let connected = |host: &Host, row: &str| {
        let entry = host
            .loader
            .entries()
            .into_iter()
            .find(|entry| entry.id == row)?;
        let link = host.peers.link_state(entry.view?.instance())?;
        let ready = matches!(*link.borrow(), LinkState::Ready { .. });
        ready.then_some(())
    };
    COLD.eventually(|| connected(&main_host, "office"), "main's link to office")
        .await;
    COLD.eventually(
        || connected(&office_host, "main"),
        "office's link from main",
    )
    .await;
    main_host.shut_down(HANG_GUARD).await.ok();
    office_host.shut_down(HANG_GUARD).await.ok();
}

/// `rutis-host run --watch-config`, its output read line by line; ended
/// by killing it (it runs no runtime).
struct Running {
    child: Child,
    lines: Receiver<String>,
    stderr: Receiver<String>,
    seen: Vec<String>,
}

impl Running {
    fn start(args: &[&str], dir: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rutis-host"))
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the rutis-host binary");
        let (send, lines) = mpsc::channel();
        // The host's own lines are on stderr (#678): read with stdout's.
        let lines_too = send.clone();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let (send, stderr) = mpsc::channel();
        let error = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(error).lines().map_while(Result::ok) {
                text.push_str(&line);
                text.push('\n');
                let _ = lines_too.send(line);
            }
            let _ = send.send(text);
        });
        Self {
            child,
            lines,
            stderr,
            seen: Vec::new(),
        }
    }

    /// Read lines until each of `lines` was read, in any order.
    fn lines_in_any_order(&mut self, lines: &[&str]) {
        let mut missing: Vec<&str> = lines.to_vec();
        while !missing.is_empty() {
            let line = self.line("");
            missing.retain(|want| *want != line);
        }
    }

    /// The first line, not read before, starting with `prefix`.
    fn line(&mut self, prefix: &str) -> String {
        loop {
            match self.lines.recv_timeout(scaled(HANG_GUARD)) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if line.starts_with(prefix) {
                        return line;
                    }
                }
                Err(error) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    let stderr = self
                        .stderr
                        .recv_timeout(scaled(HANG_GUARD))
                        .unwrap_or_default();
                    panic!(
                        "waiting for {prefix:?}: {error}; output: {:#?}; stderr: {stderr}",
                        self.seen
                    );
                }
            }
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A file written broken, then fixed: rejected, then reloaded, the host
/// the same process all along. Every platform (on Windows the only
/// trigger).
/// guarantee: host/reload-config
#[test]
fn a_watched_configuration_reloads_when_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rutis.json");
    let config = |rows: Value| {
        json!({ "id": "watched", "listen": [{ "name": "local", "address": "127.0.0.1:0" }], "rows": rows })
            .to_string()
    };
    std::fs::write(&file, config(json!([]))).unwrap();
    let mut host = Running::start(
        &["run", "--watch-config", file.to_str().unwrap()],
        dir.path(),
    );
    host.line("rutis-host: listening on local ");

    std::fs::write(&file, "{ \"rows\": [ ").unwrap();
    let rejected = host.line("rutis-host: reload rejected: ");
    assert!(rejected.ends_with("; nothing changed"), "{rejected}");
    assert!(rejected.contains("rutis.json"), "{rejected}");

    // A link to a port nothing listens on: a row that runs, its link down.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let rows = json!([{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": format!("ws://{address}/rutis") } }]);
    std::fs::write(&file, config(rows)).unwrap();
    // The row's state line and the reload's come either way round.
    let reloaded = format!("rutis-host: reloaded {} (1 row changed)", file.display());
    host.lines_in_any_order(&[&reloaded, "office: running"]);
}

/// The binary on Unix: SIGUSR1 reloads, the host runs on in the same
/// process; a signal while a reload hangs still stops the host.
#[cfg(unix)]
mod sigusr1 {
    use super::*;
    use common::process::{alive, cleanups, run_project, signal, Host, HANGING_PLUGIN};

    /// `run` restarting a Node row runs its file as it is now: a plugin
    /// file edited (no reload asked for), then the row's config changed and
    /// the configuration read again, runs the edit, as in 0.17.1. Only
    /// `dev` keeps a row on the version it last described (#665).
    /// guarantee: host/reload-config
    #[test]
    fn a_node_row_restarted_by_a_reload_runs_its_edited_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "name": "edited", "type": "module" }"#,
        )
        .unwrap();
        let sdk = common::process::file_url(
            &common::repo()
                .join("node/rutis/src/index.mjs")
                .canonicalize()
                .unwrap(),
        );
        let module = dir.path().join("plugin.mjs");
        let write = |version: &str, later: u64| {
            std::fs::write(
                &module,
                format!(
                    "import {{ definePlugin }} from '{sdk}'\n\
                     export default definePlugin({{ apply(ctx, config) {{ console.log(`plugin {version} applied tag=${{config.tag}}`) }} }})\n"
                ),
            )
            .unwrap();
            std::fs::File::options()
                .write(true)
                .open(&module)
                .unwrap()
                .set_modified(std::time::SystemTime::now() + Duration::from_secs(later))
                .unwrap();
        };
        write("v1", 0);
        let file = dir.path().join("rutis.json");
        let config = |tag: &str| {
            json!({
                "runtimes": { "node": { "project": ".", "runtime": common::repo().join("node/rutis-runtime") } },
                "rows": [{ "id": "edited", "name": "./plugin.mjs", "config": { "tag": tag } }],
            })
            .to_string()
        };
        std::fs::write(&file, config("a")).unwrap();
        let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
        host.wait_lines(&["plugin v1 applied tag=a", "edited: running"]);

        write("v2", 5);
        std::fs::write(&file, config("b")).unwrap();
        signal(&host, libc::SIGUSR1, false);
        let reloaded = format!("rutis-host: reloaded {} (1 row changed)", file.display());
        host.wait_lines(&[&reloaded, "plugin v2 applied tag=b"]);

        signal(&host, libc::SIGTERM, false);
        let (status, stderr, output) = host.exit();
        let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
        assert_eq!(status.code(), Some(0), "{context}");
    }

    /// guarantee: host/reload-config
    #[test]
    fn sigusr1_reloads_and_the_host_runs_on() {
        let dir = tempfile::tempdir().unwrap();
        let file = run_project(dir.path(), false);
        // The Node row's cleanup takes a while: the row is gone from the
        // state lines before the reload that removes it ends.
        let mut config: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        config["rows"][1]["config"]["delay"] = json!(1000);
        std::fs::write(&file, config.to_string()).unwrap();
        let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
        let pids = host.started(&["py-row", "node-row"]);

        // Broken: refused, nothing changed.
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, "{").unwrap();
        signal(&host, libc::SIGUSR1, false);
        let rejected = host.wait_line(|line| {
            line.starts_with("rutis-host: reload rejected: ")
                .then(|| line.to_owned())
        });
        assert!(rejected.ends_with("; nothing changed"), "{rejected}");

        // The Node row removed.
        let mut config: Value = serde_json::from_str(&text).unwrap();
        config["rows"]
            .as_array_mut()
            .unwrap()
            .retain(|row| row["id"] != "node-row");
        std::fs::write(&file, config.to_string()).unwrap();
        signal(&host, libc::SIGUSR1, false);
        let expected = format!("rutis-host: reloaded {} (1 row changed)", file.display());
        host.wait_lines(&[&expected, "node-row: removed"]);
        assert_eq!(cleanups(dir.path(), "node-row"), 1);
        assert!(host.child.try_wait().unwrap().is_none(), "the host exited");
        assert!(pids.iter().all(|pid| alive(*pid)), "a runtime ended");

        signal(&host, libc::SIGTERM, false);
        let (status, stderr, output) = host.exit();
        let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
        assert_eq!(status.code(), Some(0), "{context}");
        assert_eq!(cleanups(dir.path(), "py-row"), 1, "{context}");
        assert_eq!(cleanups(dir.path(), "node-row"), 1, "{context}");
    }

    /// A reload adds a row whose apply never ends; Ctrl-C ends the host
    /// all the same (past the deadline: 2), its runtimes with it.
    /// guarantee: host/reload-config, host/stop-deadline
    #[test]
    fn a_signal_during_a_reload_stops_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let file = run_project(dir.path(), false);
        std::fs::write(dir.path().join("hangs.py"), HANGING_PLUGIN).unwrap();
        // clock: the deadline must pass, and does whatever its length: the
        // apply never ends. 1 s only keeps the test short.
        let mut host = Host::start(
            &["run", "--shutdown-timeout", "1", file.to_str().unwrap()],
            dir.path(),
        );
        let pids = host.started(&["py-row", "node-row"]);
        let mut config: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        config["rows"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "hangs", "name": "py:hangs", "config": { "row": "hangs" } }));
        std::fs::write(&file, config.to_string()).unwrap();
        signal(&host, libc::SIGUSR1, false);
        host.wait_line(|line| line.starts_with("plugin hangs hangs in ").then_some(()));
        signal(&host, libc::SIGINT, true);
        let (status, stderr, output) = host.exit();
        let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
        assert_eq!(status.code(), Some(2), "{context}");
        assert!(
            !output
                .iter()
                .any(|line| line.starts_with("rutis-host: reloaded")),
            "{context}"
        );
        for pid in pids {
            assert!(
                !alive(pid),
                "runtime process {pid} outlived the host; {context}"
            );
        }
    }
}

/// A row a reload adds providing, from a thread of its own while it loads,
/// a name another row provides (Unix: [`common::process::file_url`]).
#[cfg(unix)]
mod thread_announced {
    use super::*;
    use common::process::file_url;

    /// Provides `weather` from a worker thread, which `apply` waits for.
    const THIEF: &str = r#"
import threading


class Weather:
    def hello(self, name):
        return "Rain, " + name


def apply(ctx, config):
    worker = threading.Thread(target=lambda: ctx.provide("weather", Weather()))
    worker.start()
    worker.join()
"#;

    /// The refusal of a name provided from a thread reaches the reload
    /// before the row is said to have loaded: the reload is refused, and
    /// the old rows run on.
    /// guarantee: host/reload-config, loader/announced-services
    #[tokio::test(flavor = "multi_thread")]
    async fn a_name_a_thread_provides_twice_refuses_the_reload() {
        let _environment = environment();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("thief.py"), THIEF).unwrap();
        let sdk = repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap();
        std::fs::write(
            d.join("weather.mjs"),
            format!(
                "import {{ definePlugin }} from '{}'\n\
                 class Weather {{ hello(name) {{ return `Sun, ${{name}}` }} }}\n\
                 export default definePlugin({{ apply(ctx) {{\n\
                   ctx.provide('weather', new Weather())\n\
                 }} }})\n",
                file_url(&sdk)
            ),
        )
        .unwrap();
        std::fs::write(
            d.join("package.json"),
            r#"{ "name": "reload", "type": "module" }"#,
        )
        .unwrap();
        let mut py = json!({ "project": d });
        if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
            py["python"] = json!(python.to_str().unwrap());
        }
        let runtimes = json!({
            "py": py,
            "node": { "project": d, "runtime": repo().join("node/rutis-runtime") },
        });
        let weather = json!({ "id": "weather", "name": d.join("weather.mjs").to_str().unwrap() });
        let first: HostConfig = serde_json::from_value(json!({
            "id": "reload-test",
            "runtimes": runtimes,
            "rows": [weather],
        }))
        .unwrap();
        let mut host = started(&first).await;
        assert_eq!(hello(&host, "weather").await, "Sun, Ada");

        let both: HostConfig = serde_json::from_value(json!({
            "id": "reload-test",
            "runtimes": runtimes,
            "rows": [weather, { "id": "thief", "name": "py:thief" }],
        }))
        .unwrap();
        let error = host.reload(both).await.unwrap_err();
        assert!(
            error.contains("row thief") && error.contains("already provided"),
            "{error}"
        );
        assert!(host.loader.get("thief").is_none(), "the new row stays out");
        assert_eq!(hello(&host, "weather").await, "Sun, Ada");
        host.shut_down(HANG_GUARD).await.ok();
    }
}
