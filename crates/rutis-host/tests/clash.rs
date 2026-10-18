//! A row that provides, after it has loaded, a service another row
//! provides already: it fails as the later provider, as it would have had
//! it provided the service while loading, and `run` prints why (#446).
//!
//! guarantee: loader/announced-services — the later provider fails,
//! naming both, also when it provides the service after loading.
//!
//! Unix only: the host is driven as a process ([`common::process`]).
#![cfg(unix)]

mod common;

use common::process::{file_url, signal, Host};
use common::repo;
use serde_json::json;

/// Python: provides `weather` and `ready` while loading; once `go` is
/// there (from Node), provides `forecast` after loading.
const PYTHON: &str = r#"
import threading

class Weather:
    def today(self):
        return "sun"

def apply(ctx, config):
    if config["late"]:
        threading.Timer(0.1, lambda: ctx.provide("forecast", Weather())).start()
    else:
        ctx.provide("weather", Weather())
        ctx.provide("ready", Weather())
"#;

/// Node: the same the other way round.
fn node() -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    format!(
        "import {{ definePlugin }} from '{}'\n\
         class Weather {{ today() {{ return 'sun' }} }}\n\
         export const early = definePlugin({{ apply(ctx) {{\n\
           ctx.provide('forecast', new Weather())\n\
           ctx.provide('go', new Weather())\n\
         }} }})\n\
         export const late = definePlugin({{ inject: ['ready'], apply(ctx) {{\n\
           setTimeout(() => ctx.provide('weather', new Weather()), 100)\n\
         }} }})\n",
        file_url(&sdk)
    )
}

/// The later provider of each name, Node and Python, fails with the
/// message a clash while loading gives; the earlier ones run on.
#[test]
fn a_service_provided_after_loading_twice_fails_the_later_row() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("weather.py"), PYTHON).unwrap();
    std::fs::write(
        dir.path().join("late_weather.py"),
        format!("inject = [\"go\"]\n{PYTHON}"),
    )
    .unwrap();
    std::fs::write(dir.path().join("weather.mjs"), node()).unwrap();
    std::fs::write(
        dir.path().join("early.mjs"),
        "export { early as default } from './weather.mjs'\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("late.mjs"),
        "export { late as default } from './weather.mjs'\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{ "name": "clash", "type": "module" }"#,
    )
    .unwrap();
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "id": "clash",
        "runtimes": {
            "py": py,
            "node": { "project": ".", "runtime": repo().join("node/rutis-runtime") },
        },
        "rows": [
            { "id": "py-early", "name": "py:weather", "config": { "late": false } },
            { "id": "py-late", "name": "py:late_weather", "config": { "late": true } },
            { "id": "node-early", "name": "./early.mjs" },
            { "id": "node-late", "name": "./late.mjs" },
        ]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    let mut failed = Vec::new();
    while failed.len() < 2 {
        let line = host.wait_line(|line| {
            (line.starts_with("py-late: failed") || line.starts_with("node-late: failed"))
                .then(|| line.to_owned())
        });
        failed.push(line);
    }
    failed.sort();
    let context = format!("{:#?}", host.seen);
    assert!(
        failed[0].starts_with("node-late: failed: ")
            && failed[0].ends_with(
                "service weather is already provided by row py-early: row node-late cannot provide it too"
            ),
        "{context}"
    );
    assert!(
        failed[1].starts_with("py-late: failed: ")
            && failed[1].ends_with(
                "service forecast is already provided by row node-early: row py-late cannot provide it too"
            ),
        "{context}"
    );
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    // The earlier providers ran on: neither printed another state.
    for row in ["py-early", "node-early"] {
        let states: Vec<&String> = output
            .iter()
            .filter(|line| line.starts_with(&format!("{row}: ")))
            .collect();
        assert_eq!(
            states.last().map(|line| line.as_str()),
            Some(format!("{row}: running").as_str()),
            "{context}"
        );
    }
}
