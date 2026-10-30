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
        chat |= line == "chat: waiting for llm (row model failed: Error: the model is gone)";
    }
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    assert_eq!(
        status.code(),
        Some(0),
        "output: {output:#?}; stderr: {stderr}"
    );
}

/// A disabled row's plugin, which declares in its code what it provides,
/// is never resolved while the host runs: the row waiting for that
/// service is not told that no row provides it, but that no enabled row
/// does, and where to look (acceptance of #676).
#[test]
fn a_disabled_provider_is_not_taken_for_none() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        (
            "package.json",
            r#"{ "name": "waiting", "type": "module" }"#.to_owned(),
        ),
        ("model.mjs", plugin("[]", "{ llm: { ask: 'sync' } }", false)),
        ("chat.mjs", plugin("['llm']", "{}", false)),
    ];
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let config = json!({
        "runtimes": { "node": { "runtime": repo().join("node/rutis-runtime") } },
        "rows": [
            { "id": "model", "name": "./model.mjs", "disabled": true },
            { "id": "chat", "name": "./chat.mjs" },
        ]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    host.wait_line(|line| {
        line.starts_with("chat: waiting for llm (").then(|| {
            assert_eq!(
                line,
                "chat: waiting for llm (no enabled row provides it; \
                 rutis-host check says whether a disabled row does)"
            );
        })
    });
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    assert_eq!(
        status.code(),
        Some(0),
        "output: {output:#?}; stderr: {stderr}"
    );
}

/// At start, a row whose config does not match its plugin's schema fails
/// as `check` and a reload say, in Node and Python alike: its schema is
/// known only once its runtime is up, and the row does not start on the
/// config it was given (#935). The row waiting for it says why.
/// guarantee: host/check, sdk/config-schema
#[test]
fn a_config_that_does_not_match_the_schema_fails_its_row_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let schema = json!({
        "type": "object",
        "properties": { "units": { "enum": ["metric", "imperial"], "default": "metric" } }
    });
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let files = [
        (
            "package.json",
            r#"{ "name": "waiting", "type": "module" }"#.to_owned(),
        ),
        (
            "weather.mjs",
            format!(
                "import {{ definePlugin }} from '{}'\n\
                 export default definePlugin({{ config: {schema}, \
                 provides: {{ forecast: {{ today: 'sync' }} }}, \
                 apply(ctx) {{ ctx.provide('forecast', {{ today: () => 'sunny' }}) }} }})\n",
                file_url(&sdk)
            ),
        ),
        (
            "weather.py",
            format!(
                "import json\nConfig = json.loads({:?})\ndef apply(ctx, config):\n    pass\n",
                schema.to_string()
            ),
        ),
        ("chat.mjs", plugin("['forecast']", "{}", false)),
    ];
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "runtimes": { "node": { "runtime": repo().join("node/rutis-runtime") }, "py": py },
        "rows": [
            { "id": "weather", "name": "./weather.mjs", "config": { "units": "kelvin" } },
            { "id": "weather-py", "name": "py:weather", "config": { "units": 3 } },
            { "id": "chat", "name": "./chat.mjs" },
        ]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let python = repo().join("python/rutis");
    let mut host = Host::start_in_env(
        &["run", file.to_str().unwrap()],
        dir.path(),
        &[("RUTIS_PYTHON_PATH", python.to_str().unwrap())],
    );
    // In any order.
    let mut want = vec![
        "weather: config: units should be \"metric\" or \"imperial\", got \"kelvin\"",
        "weather-py: config: units should be \"metric\" or \"imperial\", got 3",
        "chat: waiting for forecast (row weather failed: config: units should be \
         \"metric\" or \"imperial\", got \"kelvin\")",
    ];
    while !want.is_empty() {
        let line = host.wait_line(|line| Some(line.to_owned()));
        want.retain(|wanted| *wanted != line);
    }
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    assert_eq!(
        status.code(),
        Some(0),
        "output: {output:#?}; stderr: {stderr}"
    );
    // Neither started on the config it was given.
    assert!(
        !output
            .iter()
            .any(|line| line.starts_with("weather") && line.ends_with(": running")),
        "output: {output:#?}"
    );
}
