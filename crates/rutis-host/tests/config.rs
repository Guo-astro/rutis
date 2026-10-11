//! `rutis.json` as the host reads it: every field, the base of relative
//! paths, unknown fields, and the credentials from the environment.

mod common;

use std::path::{Path, PathBuf};

use common::{environment, normalize, write};
use rutis_host::config::{token, GoStart, HostConfig};
use serde_json::json;

fn read(dir: &Path, text: &str) -> Result<HostConfig, String> {
    write(dir, &[("rutis.json", text)]);
    HostConfig::read(&dir.join("rutis.json"))
}

/// Every field of the file, each as the host takes it.
/// risk: B5
#[test]
fn every_field_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let config = read(
        base,
        r#"{
          "id": "main",
          "runtimes": {
            "node": { "project": "web", "runtime": "vendor/rutis-runtime" },
            "py": { "project": "py", "python": "py/.venv/bin/python" },
            "bun": { "project": "edge", "runtime": "vendor/rutis-bun", "program": "tools/bun" },
            "go": { "dir": "plugins/go", "binaries": ["bin/netkit"], "start": "on-demand", "idle": 30, "project": "work" },
            "remote": [{ "name": "gpu", "language": "python" }]
          },
          "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "tls/server.pem", "key": "tls/server.key" }],
          "rows": [
            { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" }, "inject": ["llm"], "isolate": { "cache": true }, "disabled": false },
            { "id": "tools", "group": true, "config": [{ "id": "tool", "name": "./tools/tool.ts" }] },
            { "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis" } }
          ]
        }"#,
    )
    .unwrap();
    assert_eq!(config.id, "main");
    assert_eq!(config.peers(), vec![("office".to_owned(), true)]);
    let node = config.runtimes.node.unwrap();
    assert_eq!(node.project, base.join("web"));
    assert_eq!(node.runtime.unwrap(), base.join("vendor/rutis-runtime"));
    let py = config.runtimes.py.unwrap();
    assert_eq!(py.project, base.join("py"));
    assert_eq!(py.python.unwrap(), base.join("py/.venv/bin/python"));
    let bun = config.runtimes.bun.unwrap();
    assert_eq!(bun.project, base.join("edge"));
    assert_eq!(bun.runtime.unwrap(), base.join("vendor/rutis-bun"));
    assert_eq!(bun.program.unwrap(), base.join("tools/bun"));
    let go = config.runtimes.go.unwrap();
    assert_eq!(go.dir.as_deref(), Some(base.join("plugins/go").as_path()));
    assert_eq!(go.binaries, vec![base.join("bin/netkit")]);
    assert_eq!(go.start, GoStart::OnDemand);
    assert_eq!(go.idle, Some(30));
    assert_eq!(go.project, base.join("work"));
    assert!(go.validate().is_ok());
    assert_eq!(config.runtimes.remote[0].name, "gpu");
    assert_eq!(config.runtimes.remote[0].language, "python");
    let listener = &config.listen[0];
    assert_eq!(listener.name, "public");
    assert_eq!(listener.address, "0.0.0.0:7443".parse().unwrap());
    assert_eq!(
        listener.cert.as_deref(),
        Some(base.join("tls/server.pem").as_path())
    );
    assert_eq!(
        listener.key.as_deref(),
        Some(base.join("tls/server.key").as_path())
    );
    // Rows go to the loader as they are, but for a plugin file's name.
    assert_eq!(
        config.rows[0],
        json!({ "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" }, "inject": ["llm"], "isolate": { "cache": true }, "disabled": false })
    );
    assert_eq!(config.rows[1]["group"], true);
}

/// What an absent field means.
/// risk: B5
#[test]
fn absent_fields_take_their_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let config = read(dir.path(), "{}").unwrap();
    assert_eq!(config.id, "host");
    assert!(config.listen.is_empty() && config.rows.is_empty());
    let runtimes = &config.runtimes;
    assert!(runtimes.node.is_none() && runtimes.py.is_none() && runtimes.bun.is_none());
    assert!(runtimes.go.is_none() && runtimes.remote.is_empty());

    let config = read(
        dir.path(),
        r#"{ "runtimes": { "node": {}, "py": {}, "bun": {}, "go": { "dir": "g" } },
             "listen": [{ "name": "local", "address": "[::1]:7443" }] }"#,
    )
    .unwrap();
    let runtimes = config.runtimes;
    // `project` is the file's directory; the others are found at start.
    assert_eq!(runtimes.node.as_ref().unwrap().project, dir.path());
    assert!(runtimes.node.unwrap().runtime.is_none());
    assert_eq!(runtimes.py.as_ref().unwrap().project, dir.path());
    assert!(runtimes.py.unwrap().python.is_none());
    let bun = runtimes.bun.unwrap();
    assert!(bun.runtime.is_none() && bun.program.is_none());
    let go = runtimes.go.unwrap();
    assert_eq!((go.start, go.idle), (GoStart::OnDemand, None));
    assert!(go.binaries.is_empty());
    assert_eq!(go.project, dir.path());
    let listener = &config.listen[0];
    assert_eq!(listener.address, "[::1]:7443".parse().unwrap());
    assert!(listener.cert.is_none() && listener.key.is_none());
}

/// Relative paths are relative to the file's directory; bare program names
/// are looked up on PATH, and absolute paths stay as they are.
/// risk: B5
#[test]
fn relative_paths_are_relative_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("app");
    std::fs::create_dir(&base).unwrap();
    let elsewhere = std::path::absolute("/opt/rutis-runtime").unwrap();
    let config = read(
        &base,
        &json!({
            "runtimes": {
                "node": { "runtime": elsewhere },
                "py": { "python": "python3" },
                "bun": { "program": "bun" }
            },
            "rows": [
                { "id": "a", "name": "./tools/a.ts" },
                { "id": "b", "name": "../shared/b.mjs" },
                { "id": "c", "name": "weather-plugin" },
                { "id": "d", "name": "bun:./d.ts" },
                { "id": "e", "name": "py:e" }
            ]
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(config.runtimes.node.unwrap().runtime.unwrap(), elsewhere);
    assert_eq!(
        config.runtimes.py.unwrap().python.unwrap(),
        Path::new("python3")
    );
    assert_eq!(
        config.runtimes.bun.unwrap().program.unwrap(),
        Path::new("bun")
    );
    let names: Vec<String> = config
        .rows
        .iter()
        .map(|row| normalize(row["name"].as_str().unwrap(), dir.path()))
        .collect();
    assert_eq!(
        names,
        [
            "file://<dir>/app/tools/a.ts",
            "file://<dir>/app/../shared/b.mjs",
            "weather-plugin",
            // Modules of a runtime resolve in its project, not here.
            "bun:./d.ts",
            "py:e",
        ]
    );
}

/// A file named relative to the working directory (`rutis-host run
/// rutis.json`, the usual way) gives its plugin files' rows `file:` URLs
/// that resolve: they were `file://./<path>`, which names no file.
/// risk: B5
#[test]
fn a_file_named_relative_to_the_working_directory() {
    let dir = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    write(
        dir.path(),
        &[
            (
                "rutis.json",
                r#"{ "runtimes": { "node": {} }, "rows": [{ "id": "p", "name": "./plugin.mjs" }] }"#,
            ),
            ("plugin.mjs", ""),
        ],
    );
    let cwd = std::env::current_dir().unwrap();
    let relative = relative_to(&dir.path().join("rutis.json"), &cwd);
    assert!(relative.is_relative(), "{}", relative.display());
    let config = HostConfig::read(&relative).unwrap();
    let name = config.rows[0]["name"].as_str().unwrap();
    let url = url::Url::parse(name).unwrap();
    let file = url.to_file_path().expect("a local file");
    // Both canonical: on Windows, `\\?\` paths.
    assert_eq!(
        file.canonicalize().expect("the plugin file"),
        dir.path().join("plugin.mjs").canonicalize().unwrap(),
        "{name}"
    );
    assert!(config.runtimes.node.unwrap().project.is_absolute());
}

/// The same configuration read by a relative and by an absolute path, and
/// rebased onto a relative directory, resolves `./`, `../` and subdirectory
/// rows to the same files.
/// risk: B5 (#226)
#[test]
fn relative_and_absolute_configurations_name_the_same_files() {
    let dir = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let rows = r#"{ "runtimes": { "node": {} }, "rows": [
        { "id": "a", "name": "./a.mjs" },
        { "id": "b", "name": "../shared/b.mjs" },
        { "id": "c", "name": "./sub/c.mjs" }
    ] }"#;
    write(
        dir.path(),
        &[
            ("app/rutis.json", rows),
            ("app/a.mjs", ""),
            ("shared/b.mjs", ""),
            ("app/sub/c.mjs", ""),
        ],
    );
    let files = |config: &HostConfig| -> Vec<PathBuf> {
        config
            .rows
            .iter()
            .map(|row| {
                let name = row["name"].as_str().unwrap();
                let file = url::Url::parse(name)
                    .unwrap()
                    .to_file_path()
                    .unwrap_or_else(|()| panic!("{name}: not a local file"));
                file.canonicalize()
                    .unwrap_or_else(|error| panic!("{name}: {error}"))
            })
            .collect()
    };
    let expected: Vec<PathBuf> = ["app/a.mjs", "shared/b.mjs", "app/sub/c.mjs"]
        .iter()
        .map(|file| dir.path().join(file).canonicalize().unwrap())
        .collect();
    let absolute = dir.path().join("app/rutis.json");
    let cwd = std::env::current_dir().unwrap();
    let relative = relative_to(&absolute, &cwd);
    assert!(relative.is_relative(), "{}", relative.display());
    assert_eq!(files(&HostConfig::read(&absolute).unwrap()), expected);
    assert_eq!(files(&HostConfig::read(&relative).unwrap()), expected);
    // `rebase` itself, as a library caller would use it.
    let mut config: HostConfig = serde_json::from_str(rows).unwrap();
    config.rebase(relative.parent().unwrap());
    assert_eq!(files(&config), expected);
    assert!(config.runtimes.node.unwrap().project.is_absolute());
}

/// `path` relative to `base` (both absolute).
fn relative_to(path: &Path, base: &Path) -> PathBuf {
    let path = path
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join(path.file_name().unwrap());
    let base = base.canonicalize().unwrap();
    let common = path
        .components()
        .zip(base.components())
        .take_while(|(a, b)| a == b)
        .count();
    let mut relative = PathBuf::new();
    for _ in base.components().skip(common) {
        relative.push("..");
    }
    relative.extend(path.components().skip(common));
    relative
}

/// A field the host does not know is an error that names it, where it is,
/// and what is known there; in a row, a warning.
/// risk: B5, A7
#[test]
fn unknown_fields_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        (
            "{\n  \"id\": \"a\",\n  \"runtime\": {}\n}",
            "<dir>/rutis.json: unknown field `runtime`, expected one of `id`, `runtimes`, `listen`, `rows` at line 3 column 11",
        ),
        (
            r#"{ "runtimes": { "deno": {} } }"#,
            "<dir>/rutis.json: unknown field `deno`, expected one of `node`, `py`, `bun`, `go`, `remote` at line 1 column 22",
        ),
        (
            r#"{ "runtimes": { "node": { "projects": "." } } }"#,
            "<dir>/rutis.json: unknown field `projects`, expected `project` or `runtime` at line 1 column 36",
        ),
        (
            r#"{ "runtimes": { "py": { "pythn": "python3" } } }"#,
            "<dir>/rutis.json: unknown field `pythn`, expected `project` or `python` at line 1 column 31",
        ),
        (
            r#"{ "runtimes": { "bun": { "bun": "bun" } } }"#,
            "<dir>/rutis.json: unknown field `bun`, expected one of `project`, `runtime`, `program` at line 1 column 30",
        ),
        (
            r#"{ "runtimes": { "go": { "dirs": "g" } } }"#,
            "<dir>/rutis.json: unknown field `dirs`, expected one of `dir`, `binaries`, `start`, `idle`, `project` at line 1 column 30",
        ),
        (
            r#"{ "runtimes": { "remote": [{ "name": "gpu", "lang": "python" }] } }"#,
            "<dir>/rutis.json: unknown field `lang`, expected `name` or `language` at line 1 column 50",
        ),
        (
            r#"{ "listen": [{ "name": "p", "address": "127.0.0.1:0", "tls": true }] }"#,
            "<dir>/rutis.json: unknown field `tls`, expected one of `name`, `address`, `cert`, `key` at line 1 column 59",
        ),
        (
            r#"{ "rows": ["weather"] }"#,
            "<dir>/rutis.json: rows[0]: a row is an object",
        ),
    ];
    for (text, expected) in cases {
        let error = read(dir.path(), text).expect_err(text);
        assert_eq!(normalize(&error, dir.path()), expected, "{text}");
    }
    // In a row, the loader would drop it: the row runs without it, and a
    // warning says so, for every one.
    let config = read(
        dir.path(),
        r#"{ "rows": [
             { "id": "w", "name": "weather", "confg": { "city": "Oslo" }, "note": "x" },
             { "id": "g", "group": true, "config": [{ "id": "a", "name": "a" }, { "name": "x", "enabled": true }] }
           ] }"#,
    )
    .unwrap();
    let warnings: Vec<String> = config
        .warnings
        .iter()
        .map(|warning| normalize(warning, dir.path()))
        .collect();
    let fields = "`id`, `name`, `config`, `inject`, `isolate`, `disabled`, `group`, `instanced`";
    assert_eq!(
        warnings,
        [
            format!("<dir>/rutis.json: rows[0] (id \"w\"): unknown field `confg` is ignored; a row has {fields}"),
            format!("<dir>/rutis.json: rows[0] (id \"w\"): unknown field `note` is ignored; a row has {fields}"),
            format!("<dir>/rutis.json: rows[1] (id \"g\").config[1]: unknown field `enabled` is ignored; a row has {fields}"),
        ]
    );
    assert_eq!(
        config.rows[0]["confg"]["city"], "Oslo",
        "passed on as it was"
    );
    // Every field the loader reads, and a group's own rows, pass quietly.
    let config = read(
        dir.path(),
        r#"{ "rows": [
             { "id": "a", "name": "a", "config": {}, "inject": ["x"], "isolate": { "x": "l" }, "disabled": true },
             { "id": "g", "group": true, "instanced": true, "config": [{ "id": "b", "name": "b" }] }
           ] }"#,
    )
    .unwrap();
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
}

/// A link's token is `RUTIS_TOKEN_<PEER>` (upper case, `-` as `_`), else
/// `RUTIS_TOKEN`; none without either.
/// risk: B5
#[test]
fn a_peers_token_comes_before_the_shared_one() {
    let _environment = environment();
    assert_eq!(token("office-a"), None);
    std::env::set_var("RUTIS_TOKEN", "shared");
    assert_eq!(token("office-a").as_deref(), Some("shared"));
    std::env::set_var("RUTIS_TOKEN_OFFICE_A", "office");
    assert_eq!(token("office-a").as_deref(), Some("office"));
    assert_eq!(token("gpu").as_deref(), Some("shared"));
    // The variable is the upper-case one only (on Windows, where names
    // ignore case, there is no other).
    std::env::remove_var("RUTIS_TOKEN_OFFICE_A");
    if cfg!(not(windows)) {
        std::env::set_var("RUTIS_TOKEN_office_a", "lower");
        assert_eq!(token("office-a").as_deref(), Some("shared"));
        std::env::remove_var("RUTIS_TOKEN_office_a");
    }
    std::env::remove_var("RUTIS_TOKEN");
    std::env::set_var("RUTIS_TOKEN_GPU", "gpu");
    assert_eq!(token("gpu").as_deref(), Some("gpu"));
    assert_eq!(token("office-a"), None);
    std::env::remove_var("RUTIS_TOKEN_GPU");
}

/// A runtime's name is its rows' prefix: a remote runtime may not take a
/// local one's, nor a name that is not one.
/// risk: B5
#[test]
fn remote_runtime_names_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let check = |runtimes: &str| {
        read(dir.path(), &format!(r#"{{ "runtimes": {runtimes} }}"#))
            .unwrap()
            .check_runtime_names()
    };
    assert_eq!(
        check(r#"{ "node": {}, "remote": [{ "name": "node", "language": "node" }] }"#).unwrap_err(),
        "remote runtime node: the name is already a runtime's; rows `node:<module>` must name one"
    );
    assert_eq!(
        check(r#"{ "remote": [{ "name": "GPU", "language": "python" }] }"#).unwrap_err(),
        "remote runtime \"GPU\": a runtime name is two or more of a-z, 0-9 and -, and not file"
    );
    assert_eq!(
        check(r#"{ "go": { "dir": "g" }, "remote": [{ "name": "go-edge", "language": "go" }] }"#)
            .unwrap_err(),
        "remote runtime go-edge: go and names starting with go- are the local Go runtimes'"
    );
    assert!(check(r#"{ "remote": [{ "name": "go-edge", "language": "go" }] }"#).is_ok());
    assert!(check(r#"{ "py": {}, "remote": [{ "name": "gpu", "language": "python" }] }"#).is_ok());
}
