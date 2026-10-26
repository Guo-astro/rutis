//! A runtime the configuration does not declare, which its rows enable with
//! its defaults (#666), and which cannot start with them: as before 0.17.2
//! its rows fail and the host runs the others, saying why and what to do.
//! The released binary.
//!
//! guarantee: host/config-paths-and-defaults
//!
//! Unix only: the host is ended with SIGTERM, as in `signals.rs`.
#![cfg(unix)]

mod common;

use common::process::{signal, Host, PYTHON_PLUGIN};
use serde_json::json;

#[test]
fn an_implicit_runtime_that_cannot_start_leaves_the_host_running() {
    let dir = tempfile::tempdir().unwrap();
    // No package.json: the Node runtime cannot start with its defaults.
    std::fs::write(dir.path().join("marker.py"), PYTHON_PLUGIN).unwrap();
    std::fs::write(dir.path().join("a.mjs"), "export default {}\n").unwrap();
    let out = dir.path().to_str().unwrap();
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "runtimes": { "py": py },
        "rows": [
            { "id": "py-row", "name": "py:marker", "config": { "out": out, "row": "py-row" } },
            { "id": "js", "name": "./a.mjs" },
        ]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    // In either order: the other row runs, this one fails as before.
    let (mut running, mut failed) = (false, false);
    while !(running && failed) {
        let line = host.wait_line(|line| Some(line.to_owned()));
        running |= line == "py-row: running";
        failed |= line.starts_with("js: unresolved: no plugin named \"file://");
    }
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert!(
        stderr.contains(
            "rutis-host: warning: the node runtime, which rows use but runtimes does not declare, \
             cannot run with its defaults: the Node runtime needs a package.json in "
        ) && stderr.contains("run `npm init -y` there; its rows do not run"),
        "{context}"
    );
}
