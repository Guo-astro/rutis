//! An error a plugin leaves unhandled, as `rutis-host` shows it (#688):
//! where the runtime traces rows (Node under `dev`, Python), that row
//! fails, saying what and where, and the other rows of the runtime run on;
//! where it does not (Node under `run`, Bun), the runtime reports the error
//! as from an unknown row, and no row fails. The runtime's process goes on
//! either way. A plugin ending its process (`process.exit`, `os._exit`)
//! still ends it, and the runtime says which row did.
//!
//! guarantee: loader/unhandled-errors
//!
//! Unix only: the host is driven as a process ([`common::process`]).
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::process::{file_url, signal, Host};
use common::repo;
use serde_json::{json, Value};

/// A leaf plugin written with the SDK of this checkout.
fn js(body: &str) -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    format!(
        "import {{ definePlugin }} from '{}'\nimport {{ existsSync, writeFileSync }} from 'node:fs'\nexport default definePlugin({body})\n",
        file_url(&sdk)
    )
}

/// Runs on.
const WITNESS_JS: &str = "{ apply() {} }";
const WITNESS_PY: &str = "def apply(ctx, config):\n    pass\n";

/// Leaves a promise rejection unhandled once it runs, then writes
/// `config.done`.
const FAULTY_JS: &str = r#"{ apply(ctx, config) {
  setTimeout(() => {
    Promise.reject(new Error('boom from ' + config.row))
    setTimeout(() => writeFileSync(config.done, ''), 100)
  }, 300)
} }"#;

/// Leaves an exception unhandled in a task once it runs.
const FAULTY_PY: &str = r#"
import asyncio


async def boom(row):
    await asyncio.sleep(0.3)
    raise ValueError("boom from " + row)


def apply(ctx, config):
    asyncio.get_running_loop().create_task(boom(config["row"]))
"#;

/// Ends its process once it runs and the test says so (the file
/// `config.quit` exists): `process.exit(config.code)`. Not in a tail
/// position: Bun leaves out the frame of a tail call, and with it the
/// plugin's file.
const QUITTER_JS: &str = r#"{ apply(ctx, config) {
  const quit = () => {
    if (existsSync(config.quit)) { process.exit(config.code) }
    else { setTimeout(quit, 20) }
  }
  setTimeout(quit, 20)
} }"#;

/// The same from a thread of its own, once the test says so:
/// `os._exit(config["code"])`.
const QUITTER_PY: &str = r#"
import os
import threading
import time


def apply(ctx, config):
    def quit():
        while not os.path.exists(config["quit"]):
            time.sleep(0.02)
        os._exit(config["code"])

    threading.Thread(target=quit, daemon=True).start()
"#;

/// A project in `dir` with the Node, Python and Bun runtimes, the plugins
/// above, and `rows`; its `rutis.json`.
fn project(dir: &Path, rows: Vec<Value>) -> PathBuf {
    for (name, text) in [
        ("witness.mjs", js(WITNESS_JS)),
        ("faulty.mjs", js(FAULTY_JS)),
        ("quitter.mjs", js(QUITTER_JS)),
        ("witness.py", WITNESS_PY.to_owned()),
        ("faulty.py", FAULTY_PY.to_owned()),
        ("quitter.py", QUITTER_PY.to_owned()),
        (
            "package.json",
            r#"{ "name": "unhandled", "type": "module" }"#.to_owned(),
        ),
    ] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "id": "unhandled",
        "runtimes": {
            "py": py,
            "node": { "project": ".", "runtime": repo().join("node/rutis-runtime") },
            "bun": { "project": ".", "runtime": repo().join("bun/rutis-bun") },
        },
        "rows": rows,
    });
    let file = dir.join("rutis.json");
    std::fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    file
}

/// The state lines of `row` in `output`.
fn states<'a>(output: &'a [String], row: &str) -> Vec<&'a String> {
    output
        .iter()
        .filter(|line| line.starts_with(&format!("{row}: ")))
        .collect()
}

/// Whether `row` ran until the host stopped: until the stop's line
/// (`rutis-host: SIGTERM: stopping; …`) it was only running, never failed
/// or waiting; after it, only the states of a row being stopped. The host
/// prints what it sees each time it looks, so which of these come out, if
/// any, depends on the machine's speed: `unloading` while its cleanups
/// run, `stopped` once they are done and before the loader lets the row
/// go, `inactive` after.
fn ran_on(output: &[String], row: &str) -> bool {
    let stop = output
        .iter()
        .position(|line| line.starts_with("rutis-host: SIGTERM: stopping"))
        .expect("the host prints the stop's line");
    let line = |state: &str| format!("{row}: {state}");
    let before = states(&output[..stop], row);
    let after = states(&output[stop..], row);
    let stopping = ["unloading", "stopped", "inactive"].map(line);
    !before.is_empty()
        && before.iter().all(|state| **state == line("running"))
        && after.iter().all(|state| stopping.contains(state))
}

/// Wait until every file of `files` exists.
fn wait_for_files(host: &mut Host, files: &[PathBuf]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !files.iter().all(|file| file.exists()) {
        if std::time::Instant::now() > deadline {
            host.fail(&format!("waiting for {files:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What `bun --version` says: which Bun ran, for a failure's message.
fn bun_version() -> String {
    std::process::Command::new("bun")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| version.trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}

/// As under `rutis-host dev` (which sets `RUTIS_DEV=1` for its runtimes):
/// the row that left the error unhandled fails, in Node and in Python,
/// saying what and where; the rows beside it run on, and no runtime goes
/// down.
#[test]
fn where_rows_are_traced_an_unhandled_error_fails_its_row() {
    let dir = tempfile::tempdir().unwrap();
    let done = |row: &str| dir.path().join(format!("{row}.done"));
    let rows = vec![
        json!({ "id": "node-witness", "name": "./witness.mjs" }),
        json!({ "id": "node-faulty", "name": "./faulty.mjs", "config": { "row": "node-faulty", "done": done("node-faulty") } }),
        json!({ "id": "py-witness", "name": "py:witness" }),
        json!({ "id": "py-faulty", "name": "py:faulty", "config": { "row": "py-faulty" } }),
    ];
    let file = project(dir.path(), rows);
    let mut host = Host::start_in_env(
        &["run", file.to_str().unwrap()],
        dir.path(),
        &[("RUTIS_DEV", "1")],
    );
    let mut failed = Vec::new();
    while failed.len() < 2 {
        failed.push(host.wait_line(|line| {
            (line.starts_with("node-faulty: failed") || line.starts_with("py-faulty: failed"))
                .then(|| line.to_owned())
        }));
    }
    failed.sort();
    let context = format!("{:#?}", host.seen);
    assert_eq!(
        failed[0], "node-faulty: failed: unhandled rejection: Error: boom from node-faulty",
        "{context}"
    );
    assert_eq!(
        failed[1], "py-faulty: failed: unhandled exception: ValueError: boom from py-faulty",
        "{context}"
    );
    // A moment for anything else to happen.
    std::thread::sleep(Duration::from_millis(600));
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    for row in ["node-witness", "py-witness"] {
        assert!(ran_on(&output, row), "{row}: {context}");
    }
    // Where: the plugin's own frames, printed under each failure.
    for (row, file) in [("node-faulty", "faulty.mjs:"), ("py-faulty", "faulty.py\"")] {
        let at = output
            .iter()
            .position(|line| line.starts_with(&format!("{row}: failed")))
            .unwrap();
        assert!(
            output[at + 1..]
                .iter()
                .take_while(|line| line.starts_with("    "))
                .any(|line| line.contains(file)),
            "{row}: {context}"
        );
    }
    assert!(!stderr.contains(": down ("), "{context}");
    assert!(!stderr.contains("unknown row"), "{context}");
}

/// Under `run`, Node and Bun do not trace rows: the runtime reports the
/// error as from an unknown row, every row runs on, and no runtime goes
/// down.
#[test]
fn where_rows_are_not_traced_an_unhandled_error_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let done = |row: &str| dir.path().join(format!("{row}.done"));
    let rows = vec![
        json!({ "id": "node-witness", "name": "./witness.mjs" }),
        json!({ "id": "node-faulty", "name": "./faulty.mjs", "config": { "row": "node-faulty", "done": done("node-faulty") } }),
        json!({ "id": "bun-witness", "name": "bun:./witness.mjs" }),
        json!({ "id": "bun-faulty", "name": "bun:./faulty.mjs", "config": { "row": "bun-faulty", "done": done("bun-faulty") } }),
    ];
    let file = project(dir.path(), rows);
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    wait_for_files(&mut host, &[done("node-faulty"), done("bun-faulty")]);
    std::thread::sleep(Duration::from_millis(600));
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    for (runtime, row) in [("node", "node-faulty"), ("bun", "bun-faulty")] {
        assert!(
            stderr.contains(&format!(
                "runtime {runtime}: an unhandled error from an unknown row: Error: boom from {row}"
            )),
            "{context}"
        );
    }
    for row in ["node-witness", "node-faulty", "bun-witness", "bun-faulty"] {
        assert!(ran_on(&output, row), "{row}: {context}");
    }
    assert!(!stderr.contains(": down ("), "{context}");
}

/// A plugin ending its process ends it, in every runtime; the line before
/// says which row called it, here from the plugin's file on the stack
/// (outside `dev`, from a timer, from a thread).
///
/// They end it once the test says so (`config.quit`), not a moment after
/// they apply: a runtime whose process ends while the host still starts its
/// runtimes is one that "did not start", and `rutis-host` exits 1 then
/// (#1020). A wait of a moment was a race with just how long the runtimes
/// before it take to start, which on a loaded machine is longer.
#[test]
fn a_plugin_ending_its_process_is_named() {
    let dir = tempfile::tempdir().unwrap();
    // Written once every row runs: see above.
    let quit = dir.path().join("quit");
    let go = quit.to_str().unwrap();
    let rows = vec![
        json!({ "id": "node-witness", "name": "./witness.mjs" }),
        json!({ "id": "node-quitter", "name": "./quitter.mjs", "config": { "code": 7, "quit": go } }),
        json!({ "id": "py-witness", "name": "py:witness" }),
        json!({ "id": "py-quitter", "name": "py:quitter", "config": { "code": 5, "quit": go } }),
        json!({ "id": "bun-witness", "name": "bun:./witness.mjs" }),
        json!({ "id": "bun-quitter", "name": "bun:./quitter.mjs", "config": { "code": 6, "quit": go } }),
    ];
    let file = project(dir.path(), rows);
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    // Which Bun ran, said by every failure below, the waits here first: they
    // are where a machine's speed shows (#1020).
    let bun = bun_version();
    host.note(format!("bun {bun}"));
    // Every row running: the host started, so a runtime ending from here on
    // leaves its rows waiting instead of failing the start.
    host.wait_lines(&[
        "node-witness: running",
        "node-quitter: running",
        "py-witness: running",
        "py-quitter: running",
        "bun-witness: running",
        "bun-quitter: running",
    ]);
    std::fs::write(&quit, "").unwrap();
    // What each runtime says before its process goes.
    let said = [
        "runtime node: row node-quitter called process.exit(7)",
        "runtime py: row py-quitter called os._exit(5)",
        "runtime bun: row bun-quitter called process.exit(6)",
    ];
    host.wait_lines(&said);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, _output) = host.exit();
    let context = format!("bun {bun}; status {status:?}; stderr: {stderr}");
    for line in said {
        assert!(stderr.contains(line), "{line}: {context}");
    }
    assert_eq!(status.code(), Some(0), "{context}");
}
