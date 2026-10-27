//! A runtime a plugin keeps busy (a handler that never returns) does not
//! outlive `rutis-host` (#682): killed (`kill -9`, a crash), the host
//! cannot stop its runtimes, which end by themselves a second after their
//! deadline; stopped by a signal, it ends them past its deadline.
//!
//! Unix only, as `signals.rs`.
#![cfg(unix)]

mod common;

use common::process::{alive, file_url, signal, Host};
use common::repo;
use rutis_test_support::eventually_blocking;
use serde_json::json;

/// Says its process when it applied, then, half a second later, marks
/// `<out>/<row>.busy` and keeps the runtime's event loop busy for ever.
const PYTHON_BUSY: &str = r#"
import asyncio, os, sys


def apply(ctx, config):
    sys.stdout.write(f"plugin {config['row']} applied in {os.getpid()}\n")
    sys.stdout.flush()

    def spin():
        open(os.path.join(config["out"], config["row"] + ".busy"), "w").close()
        while True:
            pass

    asyncio.get_running_loop().call_later(0.5, spin)
"#;

fn node_busy() -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    format!(
        "import {{ definePlugin }} from '{}'\n\
         import {{ writeFileSync }} from 'node:fs'\n\
         import {{ join }} from 'node:path'\n\
         export default definePlugin({{ apply(ctx, config) {{\n\
           console.log(`plugin ${{config.row}} applied in ${{process.pid}}`)\n\
           setTimeout(() => {{ writeFileSync(join(config.out, config.row + '.busy'), ''); for (;;) {{}} }}, 500)\n\
         }} }})\n",
        file_url(&sdk)
    )
}

/// A project with a Python and a Node row, both keeping their runtime busy.
fn busy_project(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::write(dir.join("busy.py"), PYTHON_BUSY).unwrap();
    std::fs::write(dir.join("busy.mjs"), node_busy()).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{ "name": "busy", "type": "module" }"#,
    )
    .unwrap();
    let out = dir.to_str().unwrap();
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "id": "busy",
        "runtimes": {
            "py": py,
            "node": { "project": ".", "runtime": repo().join("node/rutis-runtime") },
        },
        "rows": [
            { "id": "py-row", "name": "py:busy", "config": { "out": out, "row": "py-row" } },
            { "id": "node-row", "name": "./busy.mjs", "config": { "out": out, "row": "node-row" } },
        ]
    });
    let file = dir.join("rutis.json");
    std::fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    file
}

/// The host, its runtimes busy: the processes of its rows' runtimes.
fn started_busy(dir: &std::path::Path) -> (Host, Vec<i32>) {
    let file = busy_project(dir);
    let mut host = Host::start(
        &["run", "--shutdown-timeout", "1", file.to_str().unwrap()],
        dir,
    );
    let pids = host.started(&["py-row", "node-row"]);
    eventually_blocking(
        || {
            ["py-row", "node-row"]
                .iter()
                .all(|row| dir.join(format!("{row}.busy")).exists())
                .then_some(())
        },
        "both runtimes to be busy",
    );
    (host, pids)
}

/// `kill -9` of the host: its busy runtimes end once their deadline and a
/// second more passed.
/// guarantee: host/killed-host-leaves-no-runtimes
#[test]
fn busy_runtimes_end_when_the_host_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let (host, pids) = started_busy(dir.path());
    signal(&host, libc::SIGKILL, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    for pid in pids {
        eventually_blocking(
            || (!alive(pid)).then_some(()),
            &format!("busy runtime process {pid} to end after the host was killed; {context}"),
        );
    }
}

/// Ctrl-C: the busy runtimes cannot unload their rows; past the deadline
/// the host ends them and exits with 2.
/// guarantee: host/stop-deadline
#[test]
fn ctrl_c_ends_busy_runtimes_past_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let (host, pids) = started_busy(dir.path());
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}
