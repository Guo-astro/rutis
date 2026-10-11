//! `rutis-host check` on projects that can run and projects that cannot:
//! what it prints and its exit status, word for word (a change here is a
//! change users see, to be explained in review). It needs Node (with
//! node/rutis-runtime's dependencies installed) and Python, as the host's
//! own tests do.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{normalize, repo, write, VARIABLES};

struct Checked {
    status: i32,
    stdout: String,
    stderr: String,
}

/// `rutis-host check <args>` in `dir`, with only the given variables of
/// the host's own set.
async fn check(dir: &Path, args: &[&str], env: &[(&str, &Path)]) -> Checked {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"));
    command
        .arg("check")
        .args(args)
        .current_dir(dir)
        .kill_on_drop(true);
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().into_owned();
        if VARIABLES.contains(&name.as_str()) || name.starts_with("RUTIS_TOKEN_") {
            command.env_remove(name);
        }
    }
    for (name, value) in env {
        command.env(name, value);
    }
    // Only against a hang: a check takes seconds.
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("check ends")
        .expect("rutis-host runs");
    Checked {
        status: output.status.code().expect("an exit status"),
        stdout: normalize(&String::from_utf8_lossy(&output.stdout), dir),
        stderr: normalize(&String::from_utf8_lossy(&output.stderr), dir),
    }
}

/// The SDK of this checkout, for plugins to import.
fn sdk() -> String {
    rutis_host::config::file_url(
        &repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap(),
    )
}

fn node_plugin(provides: &str, extra: &str) -> String {
    format!(
        "import {{ definePlugin }} from '{}'\n\
         export default {{ ...definePlugin({{ provides: {provides}, apply() {{}} }}){extra} }}\n",
        sdk()
    )
}

/// `rutis.json` with Node and Python runtimes, and `rows`.
fn configuration(rows: serde_json::Value) -> String {
    let mut py = serde_json::json!({});
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = serde_json::json!(python.to_string_lossy());
    }
    serde_json::json!({
        "runtimes": {
            "node": { "runtime": repo().join("node/rutis-runtime") },
            "py": py
        },
        "rows": rows
    })
    .to_string()
}

/// What can run passes: each row and what it declares, status 0. The file
/// is named relative to the working directory, as usually.
/// risk: A6, B5
#[tokio::test(flavor = "multi_thread")]
async fn check_passes_what_can_run() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            (
                "plugins/greeter.mjs",
                &node_plugin("{ greeter: { hello: 'sync' } }", ""),
            ),
            (
                "geo.py",
                "provides = {'geo': {'find': 'sync'}}\ndef apply(ctx, config):\n    pass\n",
            ),
            (
                "rutis.json",
                &configuration(serde_json::json!([
                    { "id": "greeter", "name": "./plugins/greeter.mjs" },
                    { "id": "geo", "name": "py:geo" }
                ])),
            ),
        ],
    );
    let python = repo().join("python/rutis");
    let checked = check(
        dir.path(),
        &["rutis.json"],
        &[("RUTIS_PYTHON_PATH", &python)],
    )
    .await;
    assert_eq!(checked.stderr, "");
    assert_eq!(
        checked.stdout,
        "plugin API: 1 (supported by this host)\n\
         greeter (file://<dir>/plugins/greeter.mjs): ok\n  \
         inject: []\n  \
         provides: {\"greeter\":{\"hello\":\"sync\"}}\n\
         geo (py:geo): ok\n  \
         inject: []\n  \
         provides: {\"geo\":{\"find\":\"sync\"}}\n"
    );
    assert_eq!(checked.status, 0);
}

/// What cannot run fails, each row saying why; what can still passes.
/// risk: A6, A7
#[tokio::test(flavor = "multi_thread")]
async fn check_fails_what_cannot_run() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            ("greeter.mjs", &node_plugin("{}", "")),
            ("future.mjs", &node_plugin("{}", ", api: 2")),
            ("broken.mjs", "throw new Error('broken on import')\n"),
            (
                "future_py.py",
                "from rutis import Plugin\nplugin = Plugin(lambda ctx, config: None, api=2)\n",
            ),
            (
                "rutis.json",
                &configuration(serde_json::json!([
                    { "id": "greeter", "name": "./greeter.mjs" },
                    { "id": "future", "name": "./future.mjs" },
                    { "id": "broken", "name": "./broken.mjs" },
                    { "id": "missing", "name": "weather-plugin" },
                    { "id": "nowhere", "name": "py:nowhere" },
                    { "id": "future-py", "name": "py:future_py" }
                ])),
            ),
        ],
    );
    let python = repo().join("python/rutis");
    let checked = check(
        dir.path(),
        &["rutis.json"],
        &[("RUTIS_PYTHON_PATH", &python)],
    )
    .await;
    assert_eq!(
        checked.stdout,
        "plugin API: 1 (supported by this host)\n\
         greeter (file://<dir>/greeter.mjs): ok\n  \
         inject: []\n  \
         provides: {}\n\
         future (file://<dir>/future.mjs): cannot load \"file://<dir>/future.mjs\": Error: plugin <dir>/future.mjs needs plugin API 2; this runtime supports 1: upgrade @arcships/rutis-runtime where the host runs\n\
         broken (file://<dir>/broken.mjs): cannot load \"file://<dir>/broken.mjs\": Error: broken on import\n\
         missing (weather-plugin): no plugin named \"weather-plugin\"\n  \
         install it where its runtime finds it (a runtime under runtimes), or correct the row's name\n\
         nowhere (py:nowhere): cannot load \"py:nowhere\": ModuleNotFoundError: No module named 'nowhere'\n\
         future-py (py:future_py): cannot load \"py:future_py\": RuntimeError: plugin future_py needs plugin API 2; this runtime supports 1: upgrade the rutis package where the runtime runs\n"
    );
    assert_eq!(
        checked.stderr,
        "rutis-host: 5 row(s) or binaries cannot run\n"
    );
    assert_eq!(checked.status, 1);
}

/// A configuration that is not valid is refused before anything starts;
/// a row's unknown field is a warning, and the check goes on.
/// risk: A6, B5
#[tokio::test(flavor = "multi_thread")]
async fn check_refuses_a_configuration_that_is_not_valid() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("rutis.json", r#"{ "row": [] }"#)]);
    let checked = check(dir.path(), &["rutis.json"], &[]).await;
    assert_eq!(checked.stdout, "");
    assert_eq!(
        checked.stderr,
        "rutis-host: rutis.json: unknown field `row`, expected one of `format`, `id`, `runtimes`, `listen`, `rows` at line 1 column 7\n"
    );
    assert_eq!(checked.status, 1);

    write(
        dir.path(),
        &[(
            "rutis.json",
            r#"{ "rows": [{ "id": "w", "name": "weather", "confg": {} }] }"#,
        )],
    );
    let checked = check(dir.path(), &["rutis.json"], &[]).await;
    assert_eq!(
        checked.stdout,
        "plugin API: 1 (supported by this host)\n\
         w (weather): no plugin named \"weather\"\n  \
         install it where its runtime finds it (a runtime under runtimes), or correct the row's name\n"
    );
    assert_eq!(
        checked.stderr,
        "rutis-host: warning: rutis.json: rows[0] (id \"w\"): unknown field `confg` is ignored; a row has `id`, `name`, `config`, `inject`, `isolate`, `disabled`, `group`, `instanced`\n\
         rutis-host: 1 row(s) or binaries cannot run\n"
    );
    assert_eq!(checked.status, 1);

    let checked = check(dir.path(), &["elsewhere.json"], &[]).await;
    assert_eq!(
        checked.stderr,
        "rutis-host: elsewhere.json: <not found>: name the configuration, as `rutis-host run path/to/rutis.json`\n"
    );
    assert_eq!(checked.status, 1);
}

/// In a plugin project without a rutis.json, check checks the project.
/// risk: A6
#[tokio::test(flavor = "multi_thread")]
async fn check_without_a_configuration_checks_the_project() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            (
                "package.json",
                r#"{ "name": "@acme/weather", "type": "module" }"#,
            ),
            (
                "src/index.mjs",
                &node_plugin("{ weather: { now: 'async' } }", ""),
            ),
        ],
    );
    let runtime = repo().join("node/rutis-runtime");
    let checked = check(dir.path(), &[], &[("RUTIS_NODE_RUNTIME", &runtime)]).await;
    assert_eq!(checked.stderr, "");
    assert_eq!(
        checked.stdout,
        "plugin API: 1 (supported by this host)\n\
         weather (file://<dir>/src/index.mjs): ok\n  \
         inject: []\n  \
         provides: {\"weather\":{\"now\":\"async\"}}\n"
    );
    assert_eq!(checked.status, 0);
}

/// A plugin that injects `inject` and provides `provides` (JavaScript
/// literals).
fn injecting(inject: &str, provides: &str) -> String {
    format!(
        "import {{ definePlugin }} from '{}'\n\
         export default definePlugin({{ inject: {inject}, provides: {provides}, apply() {{}} }})\n",
        sdk()
    )
}

/// A row that waits for a service no row provides cannot run: check says
/// which and what to do, and fails. One provided only by a disabled row
/// says so; one a link imports is provided. It passed before (#258).
/// risk: A6
#[tokio::test(flavor = "multi_thread")]
async fn check_fails_on_a_service_nothing_provides() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            ("chat.mjs", &injecting("['llm']", "{}")),
            ("search.mjs", &injecting("['index']", "{}")),
            (
                "indexer.mjs",
                &injecting("[]", "{ index: { find: 'sync' } }"),
            ),
            ("notes.mjs", &injecting("['calendar']", "{}")),
            (
                "rutis.json",
                &configuration(serde_json::json!([
                    { "id": "chat", "name": "./chat.mjs" },
                    { "id": "search", "name": "./search.mjs" },
                    { "id": "indexer", "name": "./indexer.mjs", "disabled": true },
                    { "id": "notes", "name": "./notes.mjs" },
                    { "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "import": ["calendar"] } }
                ])),
            ),
        ],
    );
    let python = repo().join("python/rutis");
    let checked = check(
        dir.path(),
        &["rutis.json"],
        &[("RUTIS_PYTHON_PATH", &python)],
    )
    .await;
    let stdout: Vec<&str> = checked
        .stdout
        .lines()
        .filter(|line| !line.starts_with("  config: "))
        .collect();
    assert_eq!(
        stdout,
        [
            "plugin API: 1 (supported by this host)",
            "chat (file://<dir>/chat.mjs): cannot run: nothing provides llm",
            "  inject: [\"llm\"]",
            "  provides: {}",
            "  llm: add a row that provides it, or import it from a peer (import: [\"llm\"])",
            "search (file://<dir>/search.mjs): cannot run: nothing provides index",
            "  inject: [\"index\"]",
            "  provides: {}",
            "  index: only disabled rows provide it (indexer): enable one, or add a row that provides it",
            "indexer (file://<dir>/indexer.mjs): disabled",
            "notes (file://<dir>/notes.mjs): ok",
            "  inject: [\"calendar\"]",
            "  provides: {}",
            "office (rutis-bridge/peer): ok",
        ]
    );
    assert_eq!(
        checked.stderr,
        "rutis-host: 2 row(s) or binaries cannot run\n"
    );
    assert_eq!(checked.status, 1);
}

/// A service only a row whose declarations are not known may provide (a
/// remote runtime's, not connected during the check) is not known to be
/// missing: check says so and passes.
/// risk: A6
#[tokio::test(flavor = "multi_thread")]
async fn check_passes_a_service_that_may_come_from_a_remote_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let configuration = serde_json::json!({
        "runtimes": {
            "node": { "runtime": repo().join("node/rutis-runtime") },
            "remote": [{ "name": "gpu", "language": "python" }]
        },
        "rows": [
            { "id": "chat", "name": "./chat.mjs" },
            { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com/rutis", "runtime": "gpu" } },
            { "id": "embedder", "name": "gpu:embedder" }
        ]
    });
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            ("chat.mjs", &injecting("['embeddings']", "{}")),
            ("rutis.json", &configuration.to_string()),
        ],
    );
    let checked = check(dir.path(), &["rutis.json"], &[]).await;
    let stdout: Vec<&str> = checked
        .stdout
        .lines()
        .filter(|line| !line.starts_with("  config: "))
        .collect();
    assert_eq!(
        stdout,
        [
            "plugin API: 1 (supported by this host)",
            "chat (file://<dir>/chat.mjs): ok",
            "  inject: [\"embeddings\"]",
            "  provides: {}",
            "  embeddings: not known to be provided; it may come from embedder (the gpu runtime is not running)",
            "gpu (rutis-bridge/peer): ok",
            "embedder (gpu:embedder): ok",
        ]
    );
    assert_eq!(checked.stderr, "");
    assert_eq!(checked.status, 0);
}
