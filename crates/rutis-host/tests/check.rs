//! `rutis-host check` on projects that can run and projects that cannot:
//! what it prints and its exit status, word for word (a change here is a
//! change users see, to be explained in review). It needs Node (with
//! node/rutis-runtime's dependencies installed) and Python, as the host's
//! own tests do.

mod common;

use std::path::Path;

use common::{normalize, repo, write, VARIABLES};
use rutis_test_support::{Guard, HANG_GUARD};

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
    // Only against a hang: a check takes seconds, but starts the runtimes
    // (Node, Python) cold; six times HANG_GUARD, as before.
    let output = Guard(HANG_GUARD * 6)
        .within("check ends", command.output())
        .await
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
/// guarantee: host/check
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
        "greeter (file://<dir>/plugins/greeter.mjs): ok\n  \
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
/// guarantee: host/exit-codes, host/check
#[tokio::test(flavor = "multi_thread")]
async fn check_fails_what_cannot_run() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            ("greeter.mjs", &node_plugin("{}", "")),
            ("future.mjs", &node_plugin("{}", ", api: 4")),
            ("broken.mjs", "throw new Error('broken on import')\n"),
            (
                "future_py.py",
                "from rutis import Plugin\nplugin = Plugin(lambda ctx, config: None, api=4)\n",
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
        "greeter (file://<dir>/greeter.mjs): ok\n  \
         inject: []\n  \
         provides: {}\n\
         future (file://<dir>/future.mjs): cannot load \"file://<dir>/future.mjs\": Error: plugin <dir>/future.mjs needs plugin API 4; this runtime supports 3: upgrade @arcships/rutis-runtime where the host runs\n\
         broken (file://<dir>/broken.mjs): cannot load \"file://<dir>/broken.mjs\": Error: broken on import\n\
         missing (weather-plugin): no plugin named \"weather-plugin\"\n  \
         install it where its runtime finds it (a runtime under runtimes), or correct the row's name\n\
         nowhere (py:nowhere): cannot load \"py:nowhere\": ModuleNotFoundError: No module named 'nowhere'\n\
         future-py (py:future_py): cannot load \"py:future_py\": RuntimeError: plugin future_py needs plugin API 4; this runtime supports 3: upgrade the rutis package where the runtime runs\n"
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
/// guarantee: host/exit-codes, host/check, host/unknown-fields
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
        "w (weather): no plugin named \"weather\"\n  \
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
/// guarantee: host/check
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
        "weather (file://<dir>/src/index.mjs): ok\n  \
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

/// A row that waits for a service no row provides, and no row may provide
/// at runtime either, cannot run: check says which and what to do, and
/// fails. One provided only by a disabled row says so; one a link imports
/// is provided. It passed before (#258). With another row that may provide
/// services it does not declare (every plugin of a 0.12 SDK), they are
/// warnings, and check passes.
/// risk: A6
/// guarantee: host/check
#[tokio::test(flavor = "multi_thread")]
async fn check_fails_on_a_service_nothing_provides() {
    let dir = tempfile::tempdir().unwrap();
    let rows = |more: &[serde_json::Value]| {
        let mut rows = vec![
            serde_json::json!({ "id": "chat", "name": "./chat.mjs" }),
            serde_json::json!({ "id": "indexer", "name": "./indexer.mjs", "disabled": true }),
            serde_json::json!({ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "import": ["calendar"] } }),
        ];
        rows.extend_from_slice(more);
        configuration(serde_json::Value::Array(rows))
    };
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            ("chat.mjs", &injecting("['llm', 'index', 'calendar']", "{}")),
            (
                "indexer.mjs",
                &injecting("[]", "{ index: { find: 'sync' } }"),
            ),
            ("notes.mjs", &injecting("[]", "{}")),
            ("rutis.json", &rows(&[])),
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
            "chat (file://<dir>/chat.mjs): cannot run: nothing provides index, llm",
            "  inject: [\"llm\",\"index\",\"calendar\"]",
            "  provides: {}",
            "  index: only disabled rows provide it (indexer): enable one, or add a row that provides it",
            "  llm: add a row that provides it, or import it from a peer (import: [\"llm\"])",
            "indexer (file://<dir>/indexer.mjs): disabled",
            "office (rutis-bridge/peer): ok",
        ]
    );
    assert_eq!(
        checked.stderr,
        "rutis-host: 1 row(s) or binaries cannot run\n"
    );
    assert_eq!(checked.status, 1);

    // A row that may provide them once it runs: warnings, not failures.
    write(
        dir.path(),
        &[(
            "rutis.json",
            &rows(&[serde_json::json!({ "id": "notes", "name": "./notes.mjs" })]),
        )],
    );
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
            "chat (file://<dir>/chat.mjs): ok",
            "  inject: [\"llm\",\"index\",\"calendar\"]",
            "  provides: {}",
            "  index: only disabled rows provide it (indexer): enable one, or add a row that provides it",
            "indexer (file://<dir>/indexer.mjs): disabled",
            "office (rutis-bridge/peer): ok",
            "notes (file://<dir>/notes.mjs): ok",
            "  inject: []",
            "  provides: {}",
        ]
    );
    // The disabled row that declares `index` is named, not `notes` (#666).
    assert_eq!(
        checked.stderr,
        "rutis-host: warning: chat: index is declared only by row indexer, which is disabled: enable it, or add a row that provides index\n\
         rutis-host: warning: chat: nothing declares llm; it may be provided at runtime by notes\n"
    );
    assert_eq!(checked.status, 0);
}

/// Rows that inject each other, rows that declare one service, and a
/// misspelt name: check warns of the first two and suggests the name meant
/// (#676); the exit status is what it was (a cycle and a clash are
/// warnings in 0.17). Rows of a language whose runtime is not declared
/// enable it with its defaults, saying so (#666).
/// risk: A6
/// guarantee: host/check
#[tokio::test(flavor = "multi_thread")]
async fn check_warns_of_cycles_clashes_and_misspelt_names() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "runtimes": { "node": { "runtime": repo().join("node/rutis-runtime") } },
        "rows": [
            { "id": "a", "name": "./a.mjs" },
            { "id": "b", "name": "./b.mjs" },
            { "id": "one", "name": "./llm.mjs" },
            { "id": "two", "name": "./llm.mjs" },
            { "id": "weather", "name": "./weather.mjs" },
            { "id": "geo", "name": "py:geo" },
        ]
    });
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            ("a.mjs", &injecting("['bsvc']", "{ asvc: { get: 'sync' } }")),
            ("b.mjs", &injecting("['asvc']", "{ bsvc: { get: 'sync' } }")),
            ("llm.mjs", &injecting("[]", "{ llm: { ask: 'sync' } }")),
            ("weather.mjs", &injecting("['lmm']", "{}")),
            (
                "geo.py",
                "provides = {'geo': {'find': 'sync'}}\ndef apply(ctx, config):\n    pass\n",
            ),
            ("rutis.json", &config.to_string()),
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
            "a (file://<dir>/a.mjs): ok",
            "  inject: [\"bsvc\"]",
            "  provides: {\"asvc\":{\"get\":\"sync\"}}",
            "b (file://<dir>/b.mjs): ok",
            "  inject: [\"asvc\"]",
            "  provides: {\"bsvc\":{\"get\":\"sync\"}}",
            "one (file://<dir>/llm.mjs): ok",
            "  inject: []",
            "  provides: {\"llm\":{\"ask\":\"sync\"}}",
            "two (file://<dir>/llm.mjs): ok",
            "  inject: []",
            "  provides: {\"llm\":{\"ask\":\"sync\"}}",
            "weather (file://<dir>/weather.mjs): ok",
            "  inject: [\"lmm\"]",
            "  provides: {}",
            "geo (py:geo): ok",
            "  inject: []",
            "  provides: {\"geo\":{\"find\":\"sync\"}}",
        ]
    );
    assert_eq!(
        checked.stderr,
        "rutis-host: warning: rutis.json: rows use the py runtime (py:geo), which runtimes does not declare: it runs with its defaults; declare \"runtimes\": { \"py\": {} } to say so\n\
         rutis-host: warning: weather: nothing declares lmm; it may be provided at runtime by a, b, one and 2 more; did you mean llm?\n\
         rutis-host: warning: rows a, b wait for each other (a waits for bsvc from b, b waits for asvc from a): none of them starts; break the cycle in their inject\n\
         rutis-host: warning: rows one, two all provide llm: only the first to start provides it, the others fail; keep one, or isolate the name in each row\n"
    );
    assert_eq!(checked.status, 0);
}

/// Cordis rows that declare nothing (published packages): what one injects
/// another may provide once they run, resolved in Cordis, so check warns,
/// naming those rows, and passes; a declared provider is named as usual
/// (#428). Between leaf plugins the same, without the words about Cordis
/// (#446). With `--strict` the warnings are errors, and check fails.
/// risk: A6, F8
/// guarantee: host/check
#[tokio::test(flavor = "multi_thread")]
async fn check_passes_what_cordis_rows_may_provide_at_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let leaf = |body: &str| {
        format!(
            "import {{ definePlugin }} from '{}'\nexport default definePlugin({body})\n",
            sdk()
        )
    };
    write(
        dir.path(),
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            (
                "storage.mjs",
                "export const name = 'storage'\nexport function apply(ctx) { ctx.provide('storage', { get() {} }) }\n",
            ),
            (
                "domain.mjs",
                "export const name = 'domain'\nexport const inject = ['storage']\nexport function apply(ctx) { ctx.provide('storageDomain', { open() {} }) }\n",
            ),
            (
                "workspace.mjs",
                "export const name = 'workspace'\nexport const inject = ['storageDomain']\nexport function apply() {}\n",
            ),
            (
                "forecast.mjs",
                &leaf("{ apply(ctx) { ctx.provide('forecast', { today() {} }) } }"),
            ),
            (
                "planner.mjs",
                &leaf("{ inject: ['forecast'], apply() {} }"),
            ),
            (
                "rutis.json",
                &configuration(serde_json::json!([
                    { "id": "storage", "name": "./storage.mjs" },
                    { "id": "domain", "name": "./domain.mjs" },
                    { "id": "workspace", "name": "./workspace.mjs" },
                    { "id": "forecast", "name": "./forecast.mjs" },
                    { "id": "planner", "name": "./planner.mjs" }
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
            "storage (file://<dir>/storage.mjs): ok",
            "  inject: []",
            "  provides: {}",
            "domain (file://<dir>/domain.mjs): ok",
            "  inject: [\"storage\"]",
            "  provides: {}",
            "workspace (file://<dir>/workspace.mjs): ok",
            "  inject: [\"storageDomain\"]",
            "  provides: {}",
            "forecast (file://<dir>/forecast.mjs): ok",
            "  inject: []",
            "  provides: {}",
            "planner (file://<dir>/planner.mjs): ok",
            "  inject: [\"forecast\"]",
            "  provides: {}",
        ]
    );
    let warnings = "\
        domain: nothing declares storage; it may be provided at runtime by storage, workspace, forecast and 1 more; between Cordis rows of one runtime it resolves in Cordis\n\
        workspace: nothing declares storageDomain; it may be provided at runtime by storage, domain, forecast and 1 more; between Cordis rows of one runtime it resolves in Cordis\n\
        planner: nothing declares forecast; it may be provided at runtime by storage, domain, workspace and 1 more\n";
    let prefixed = |level: &str| {
        warnings
            .lines()
            .map(|line| format!("rutis-host: {level}: {line}\n"))
            .collect::<String>()
    };
    assert_eq!(checked.stderr, prefixed("warning"));
    assert_eq!(checked.status, 0);

    // `--strict`: those are errors, and the rows waiting for them cannot run.
    let checked = check(
        dir.path(),
        &["rutis.json", "--strict"],
        &[("RUTIS_PYTHON_PATH", &python)],
    )
    .await;
    let stdout: Vec<&str> = checked
        .stdout
        .lines()
        .filter(|line| !line.starts_with("  "))
        .collect();
    assert_eq!(
        stdout,
        [
            "storage (file://<dir>/storage.mjs): ok",
            "domain (file://<dir>/domain.mjs): cannot run with --strict: nothing declares storage",
            "workspace (file://<dir>/workspace.mjs): cannot run with --strict: nothing declares storageDomain",
            "forecast (file://<dir>/forecast.mjs): ok",
            "planner (file://<dir>/planner.mjs): cannot run with --strict: nothing declares forecast",
        ]
    );
    assert_eq!(
        checked.stderr,
        format!(
            "{}rutis-host: 3 row(s) or binaries cannot run\n",
            prefixed("error")
        )
    );
    assert_eq!(checked.status, 1);
}

/// A service only a row whose declarations are not known may provide (a
/// remote runtime's, not connected during the check) is not known to be
/// missing: check says so and passes.
/// risk: A6
/// guarantee: host/check
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
            "chat (file://<dir>/chat.mjs): ok",
            "  inject: [\"embeddings\"]",
            "  provides: {}",
            "  embeddings: not known to be provided; it may come from embedder (the gpu runtime is not running)",
            "gpu (rutis-bridge/peer): ok",
            "embedder (gpu:embedder): ok",
            "  config not checked: its schema is known only when it runs",
        ]
    );
    assert_eq!(checked.stderr, "");
    assert_eq!(checked.status, 0);
}

/// The weather plugin's schema (config schema design §9).
fn weather_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["city"],
        "properties": {
            "city": { "type": "string" },
            "units": { "enum": ["metric", "imperial"], "default": "metric" },
            "days": { "type": "integer" },
            "home": { "type": "object", "properties": { "lat": { "type": "number" } } }
        }
    })
}

/// A Node plugin and a Python module with the same schema.
fn weather_plugins() -> Vec<(&'static str, String)> {
    let schema = weather_schema();
    vec![
        (
            "package.json",
            r#"{ "name": "app", "type": "module" }"#.to_owned(),
        ),
        (
            "weather.mjs",
            format!(
                "import {{ definePlugin }} from '{}'\n\
                 export default definePlugin({{ config: {schema}, apply() {{}} }})\n",
                sdk()
            ),
        ),
        (
            "weather.py",
            format!(
                "import json\nConfig = json.loads({:?})\ndef apply(ctx, config):\n    pass\n",
                schema.to_string()
            ),
        ),
    ]
}

async fn check_rows(rows: serde_json::Value, args: &[&str]) -> Checked {
    let dir = tempfile::tempdir().unwrap();
    let mut files = weather_plugins();
    files.push(("rutis.json", configuration(rows)));
    let files: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    write(dir.path(), &files);
    let python = repo().join("python/rutis");
    let mut all = vec!["rutis.json"];
    all.extend_from_slice(args);
    check(dir.path(), &all, &[("RUTIS_PYTHON_PATH", &python)]).await
}

/// Lines of `check`'s output without what it prints of each plugin
/// (indented).
fn rows_only(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| !line.starts_with("  "))
        .collect()
}

/// A config that does not match its plugin's schema fails its row, one
/// line per problem sorted by path, the same for every language; exit
/// status 1 (config schema design §7).
/// guarantee: host/check
#[tokio::test(flavor = "multi_thread")]
async fn check_fails_a_config_that_does_not_match_the_schema() {
    let wrong = serde_json::json!({ "days": 1.5, "home": { "lat": "north" }, "units": "kelvin" });
    let checked = check_rows(
        serde_json::json!([
            { "id": "weather", "name": "./weather.mjs", "config": wrong },
            { "id": "weather-py", "name": "py:weather", "config": wrong },
            { "id": "oslo", "name": "./weather.mjs", "config": "Oslo" },
            { "id": "fine", "name": "py:weather", "config": { "city": "Oslo" } }
        ]),
        &[],
    )
    .await;
    assert_eq!(
        rows_only(&checked.stdout),
        [
            "weather (file://<dir>/weather.mjs): cannot run: config: city is required",
            "weather (file://<dir>/weather.mjs): cannot run: config: days should be an integer, got 1.5",
            "weather (file://<dir>/weather.mjs): cannot run: config: home.lat should be a number, got \"north\"",
            "weather (file://<dir>/weather.mjs): cannot run: config: units should be \"metric\" or \"imperial\", got \"kelvin\"",
            "weather-py (py:weather): cannot run: config: city is required",
            "weather-py (py:weather): cannot run: config: days should be an integer, got 1.5",
            "weather-py (py:weather): cannot run: config: home.lat should be a number, got \"north\"",
            "weather-py (py:weather): cannot run: config: units should be \"metric\" or \"imperial\", got \"kelvin\"",
            "oslo (file://<dir>/weather.mjs): cannot run: config: config should be an object, got \"Oslo\"",
            "fine (py:weather): ok",
        ]
    );
    assert_eq!(
        checked.stderr,
        "rutis-host: 3 row(s) or binaries cannot run\n"
    );
    assert_eq!(checked.status, 1);
}

/// An extra key is a warning on standard error, with exit status 0; with
/// `--strict` the row cannot run, exit status 1 (config schema design §7).
/// guarantee: host/check
#[tokio::test(flavor = "multi_thread")]
async fn check_warns_of_extra_config_keys() {
    let rows = serde_json::json!([
        { "id": "weather", "name": "./weather.mjs", "config": { "city": "Oslo", "cty": "Bergen" } },
        { "id": "weather-py", "name": "py:weather", "config": { "city": "Oslo", "zz": 1 } }
    ]);
    let checked = check_rows(rows.clone(), &[]).await;
    assert_eq!(
        rows_only(&checked.stdout),
        [
            "weather (file://<dir>/weather.mjs): ok",
            "weather-py (py:weather): ok"
        ]
    );
    assert_eq!(
        checked.stderr,
        "rutis-host: warning: weather: config: cty is not a config key (did you mean city?)\n\
         rutis-host: warning: weather-py: config: zz is not a config key\n"
    );
    assert_eq!(checked.status, 0);

    let strict = check_rows(rows, &["--strict"]).await;
    assert_eq!(
        rows_only(&strict.stdout),
        [
            "weather (file://<dir>/weather.mjs): cannot run with --strict: config: cty is not a config key (did you mean city?)",
            "weather-py (py:weather): cannot run with --strict: config: zz is not a config key"
        ]
    );
    assert_eq!(
        strict.stderr,
        "rutis-host: error: weather: config: cty is not a config key (did you mean city?)\n\
         rutis-host: error: weather-py: config: zz is not a config key\n\
         rutis-host: 2 row(s) or binaries cannot run\n"
    );
    assert_eq!(strict.status, 1);
}
