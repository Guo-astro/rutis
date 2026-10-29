//! `rutis-host status` asks a running host what it does now (#678): each
//! row's state and why, each service and its provider, the runtimes with
//! their process ids, the calls not returned yet. The released binary,
//! with Node rows.
//!
//! Unix only: `status` is not available on Windows, and the host is ended
//! with SIGTERM, as in `signals.rs`.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::process::{file_url, signal, Host, GUARD};
use common::repo;
use serde_json::json;

/// A plugin injecting `inject` and providing `provides` (JavaScript
/// literals), its apply's body `body`.
fn plugin(inject: &str, provides: &str, body: &str) -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    format!(
        "import {{ definePlugin }} from '{}'\n\
         export default definePlugin({{ inject: {inject}, provides: {provides}, async apply(ctx) {{ {body} }} }})\n",
        file_url(&sdk)
    )
}

/// `rutis-host status <target>`, with the runtime directory `runtime`.
fn status(target: &Path, dir: &Path, runtime: &Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
        .arg("status")
        .arg(target)
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", runtime)
        .output()
        .unwrap()
}

/// Provides `slow`, whose `wait` does not return, cancelled or not.
const SLOW: &str = r#"
import asyncio

provides = {"slow": {"wait": "async"}}


class Slow:
    async def wait(self):
        while True:
            try:
                await asyncio.sleep(600)
            except asyncio.CancelledError:
                pass


def apply(ctx, config):
    ctx.provide("slow", Slow())
"#;

/// A Node row whose apply waits on a Python row's method that does not
/// return: `status` lists both rows, the service and its provider, the
/// runtimes and their process ids, and the call (calls within one runtime
/// are not seen); the socket only this user can open. `status` in the
/// directory finds the host started with its `rutis.json`. When stopping
/// does not finish, the call not returned is named; once the host ended,
/// `status` says no host runs it.
/// guarantee: host/status
#[test]
fn status_says_what_a_running_host_does() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let files = [
        (
            "package.json",
            r#"{ "name": "status", "type": "module" }"#.to_owned(),
        ),
        ("slow.py", SLOW.to_owned()),
        (
            "caller.mjs",
            plugin("['slow']", "{}", "await ctx.use('slow').wait()"),
        ),
    ];
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "runtimes": {
            "py": py,
            "node": { "project": ".", "runtime": repo().join("node/rutis-runtime") },
        },
        "rows": [
            { "id": "slow", "name": "py:slow" },
            { "id": "caller", "name": "./caller.mjs" },
        ]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let host = Host::start_in_env(
        &["run", "rutis.json", "--shutdown-timeout", "1"],
        dir.path(),
        &[("XDG_RUNTIME_DIR", runtime.path().to_str().unwrap())],
    );
    // The caller's apply waits, so the host does not say it runs: ask.
    let deadline = std::time::Instant::now() + rutis_test_support::scaled(GUARD);
    let said = loop {
        let asked = status(Path::new("."), dir.path(), runtime.path());
        let said = String::from_utf8_lossy(&asked.stdout).into_owned();
        if said.contains("  caller → slow.wait") {
            assert!(asked.status.success());
            break said;
        }
        assert!(std::time::Instant::now() < deadline, "no call yet: {said}");
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let lines: Vec<&str> = said.lines().collect();
    let pid = |name: &str| -> String {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(&format!("  {name}: running (pid ")))
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or_else(|| panic!("{name}'s pid: {said}"))
            .to_owned()
    };
    let (node, python) = (pid("node"), pid("py"));
    assert!(
        node.parse::<u32>().is_ok() && python.parse::<u32>().is_ok(),
        "{said}"
    );
    let call = lines.last().copied().unwrap_or_default();
    assert!(call.starts_with("  caller → slow.wait  "), "{said}");
    assert_eq!(
        lines[..lines.len() - 1],
        [
            "rows:",
            "  slow: running",
            "  caller: starting",
            "services:",
            "  loader: host",
            "  sessions: host",
            "  slow: slow",
            "runtimes:",
            &format!("  node: running (pid {node})"),
            &format!("  py: running (pid {python})"),
            "calls:",
        ],
        "{said}"
    );
    // Only this user can open the socket, or enter its directory.
    let dirs: Vec<_> = std::fs::read_dir(runtime.path()).unwrap().collect();
    assert_eq!(dirs.len(), 1);
    let own = dirs[0].as_ref().unwrap().path();
    assert_eq!(own.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    let socket = std::fs::read_dir(&own)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        socket.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );

    // The caller's apply never returns: stopping does not finish, and
    // says which call it waits on.
    signal(&host, libc::SIGTERM, false);
    let (exit, stderr, _) = host.exit();
    assert_eq!(exit.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("  call not returned: caller → slow.wait  "),
        "{stderr}"
    );
    // The host removed its socket.
    assert!(!socket.exists());
    let asked = status(&file, dir.path(), runtime.path());
    assert_eq!(asked.status.code(), Some(1));
    let said = String::from_utf8_lossy(&asked.stderr);
    assert!(
        said.starts_with("rutis-host: no rutis-host runs "),
        "{said}"
    );
}
