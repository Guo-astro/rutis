//! What `rutis-host run` says of a row that waits (#676): which service,
//! and why it is not there — a misspelt name (with the one meant), a
//! provider that failed. The released binary, with Node rows.
//!
//! guarantee: host/row-state-lines
//!
//! Unix only: the host is ended with SIGTERM, as in `signals.rs`.
#![cfg(unix)]

mod common;

use common::process::{file_url, signal, Host};
use common::repo;
use serde_json::json;

/// A plugin injecting `inject` and providing `provides` (JavaScript
/// literals); `fail`: its apply throws.
fn plugin(inject: &str, provides: &str, fail: bool) -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let body = match fail {
        true => "throw new Error('the model is gone')",
        false => "",
    };
    format!(
        "import {{ definePlugin }} from '{}'\n\
         export default definePlugin({{ inject: {inject}, provides: {provides}, apply() {{ {body} }} }})\n",
        file_url(&sdk)
    )
}

#[test]
fn a_waiting_row_says_what_it_waits_for_and_why() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        (
            "package.json",
            r#"{ "name": "waiting", "type": "module" }"#.to_owned(),
        ),
        ("model.mjs", plugin("[]", "{ llm: { ask: 'sync' } }", true)),
        ("weather.mjs", plugin("['lmm']", "{}", false)),
        ("chat.mjs", plugin("['llm']", "{}", false)),
    ];
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let config = json!({
        "runtimes": { "node": { "runtime": repo().join("node/rutis-runtime") } },
        "rows": [
            { "id": "model", "name": "./model.mjs" },
            { "id": "weather", "name": "./weather.mjs" },
            { "id": "chat", "name": "./chat.mjs" },
        ]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    // In either order.
    let (mut weather, mut chat) = (false, false);
    while !(weather && chat) {
        let line = host.wait_line(|line| Some(line.to_owned()));
        weather |= line == "weather: waiting for lmm (no row provides it; did you mean llm?)";
        chat |= line
            == "chat: waiting for llm (row model failed: plugin failed: Error: the model is gone)";
    }
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    assert_eq!(
        status.code(),
        Some(0),
        "output: {output:#?}; stderr: {stderr}"
    );
}
