//! The projects `rutis-host new` makes, as a host uses them: each template
//! declares what its plugin provides (#666), so a configuration whose rows
//! use the template's service passes `rutis-host check --strict` before
//! anything runs; the template's own row is configured as its
//! `rutis.dev.json` configures it, and that config passes the plugin's
//! schema with no error and no extra key (config schema design §11, 1c).
//! Needs Node (with node/rutis-runtime's dependencies installed) and
//! Python, as the host's own tests do; the Go test (its name has `go`, so
//! the Go CI job runs it too) needs Go, the Bun test (its name has `bun`,
//! so the Bun CI job runs it too) needs Bun.
//!
//! Unix only: the generated projects are the same everywhere, and the Go
//! binary is named without Windows' `.exe`.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Command;

use common::{repo, VARIABLES};
use serde_json::json;

/// `rutis-host check --strict rutis.json` in `dir`: its exit status,
/// stdout and stderr.
fn check_strict(dir: &Path) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rutis-host"));
    command
        .args(["check", "--strict", "rutis.json"])
        .current_dir(dir);
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().into_owned();
        if VARIABLES.contains(&name.as_str()) || name.starts_with("RUTIS_TOKEN_") {
            command.env_remove(name);
        }
    }
    command.env("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
    let output = command.output().expect("rutis-host runs");
    (
        output.status.code().expect("an exit status"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// The checkout's SDK, by file URL, for plugin files to import.
fn sdk() -> String {
    rutis_host::config::file_url(
        &repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap(),
    )
}

/// A row that uses `greeter`: what a host's own plugin, or a plugin of
/// someone else's, would be.
fn user(dir: &Path) {
    let source = format!(
        "import {{ definePlugin }} from '{}'\n\
         export default definePlugin({{ inject: ['greeter'], apply() {{}} }})\n",
        sdk()
    );
    std::fs::write(dir.join("user.mjs"), source).unwrap();
}

/// The config `rutis.dev.json` in `project` gives the template's own row
/// (its first row).
fn dev_config(project: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(project.join("rutis.dev.json")).unwrap();
    let dev: serde_json::Value = serde_json::from_str(&text).unwrap();
    let config = dev["rows"][0]["config"].clone();
    assert!(config.is_object(), "{text}");
    config
}

/// What `check` says of a healthy configuration with a template's plugin
/// (row `greeter`) and a row using its service: no warning, every row ok,
/// the template's service declared.
fn passes(dir: &Path, provides: &str) {
    let (status, stdout, stderr) = check_strict(dir);
    let context = format!("status {status}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(status, 0, "{context}");
    assert_eq!(stderr, "", "{context}");
    assert!(
        stdout.contains(&format!("  provides: {provides}\n")),
        "{context}"
    );
    for row in ["greeter (", "user ("] {
        assert!(
            stdout
                .lines()
                .any(|line| line.starts_with(row) && line.ends_with("): ok")),
            "{row}: {context}"
        );
    }
}

/// The TypeScript and Python templates, each with a row using what it
/// provides: `check --strict` passes, the template's `provides` read
/// without running the plugin.
/// guarantee: host/new
#[test]
fn the_templates_declare_what_they_provide() {
    let dir = tempfile::tempdir().unwrap();
    // TypeScript: the generated plugin as written, but for where its SDK
    // comes from (the checkout's, not an installed package).
    let node = dir.path().join("node");
    std::fs::create_dir_all(&node).unwrap();
    rutis_host::new::create(&node, "greeter", "node").unwrap();
    let index = node.join("greeter/src/index.ts");
    let source = std::fs::read_to_string(&index).unwrap();
    assert!(source.contains("from '@arcships/rutis'"), "{source}");
    let source = source.replace("from '@arcships/rutis'", &format!("from '{}'", sdk()));
    std::fs::write(&index, source).unwrap();
    std::fs::write(
        node.join("package.json"),
        r#"{ "name": "app", "type": "module" }"#,
    )
    .unwrap();
    user(&node);
    let config = json!({
        "runtimes": { "node": { "runtime": repo().join("node/rutis-runtime") } },
        "rows": [
            { "id": "greeter", "name": "./greeter/src/index.ts", "config": dev_config(&node.join("greeter")) },
            { "id": "user", "name": "./user.mjs" }
        ]
    });
    std::fs::write(node.join("rutis.json"), config.to_string()).unwrap();
    passes(&node, r#"{"greeter":{"hello":"async"}}"#);

    // Python: the module the template makes, in the Python runtime's
    // project; the row using it in Node.
    let python = dir.path().join("python");
    std::fs::create_dir_all(&python).unwrap();
    rutis_host::new::create(&python, "hello", "python").unwrap();
    std::fs::write(
        python.join("package.json"),
        r#"{ "name": "app", "type": "module" }"#,
    )
    .unwrap();
    user(&python);
    let config = json!({
        "runtimes": {
            "node": { "runtime": repo().join("node/rutis-runtime") },
            "py": { "project": "hello/src" }
        },
        "rows": [
            { "id": "greeter", "name": "py:hello", "config": dev_config(&python.join("hello")) },
            { "id": "user", "name": "./user.mjs" }
        ]
    });
    std::fs::write(python.join("rutis.json"), config.to_string()).unwrap();
    passes(&python, r#"{"greeter":{"hello":"async"}}"#);
}

/// The Go template, built against the checkout's Go SDK: its binary's
/// manifest declares the service, and `check --strict` passes.
/// guarantee: host/new
#[test]
fn the_go_template_declares_what_it_provides() {
    let dir = tempfile::tempdir().unwrap();
    rutis_host::new::create(dir.path(), "greeter", "go").unwrap();
    let project = dir.path().join("greeter");
    let module = project.join("go.mod");
    let mut text = std::fs::read_to_string(&module).unwrap();
    text.push_str(&format!(
        "\nreplace github.com/arcships/rutis/go/rutis => {}\n",
        repo().join("go/rutis").canonicalize().unwrap().display()
    ));
    std::fs::write(&module, text).unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let status = Command::new("go")
        .args(["build", "-o"])
        .arg(plugins.join("greeter"))
        .arg("./cmd/greeter")
        .current_dir(&project)
        .env("GOFLAGS", "-mod=mod")
        .env("GOPROXY", "off")
        .status()
        .expect("go is on PATH");
    assert!(status.success(), "go build");
    std::fs::write(
        dir.path().join("package.json"),
        r#"{ "name": "app", "type": "module" }"#,
    )
    .unwrap();
    user(dir.path());
    let config = json!({
        "runtimes": {
            "node": { "runtime": repo().join("node/rutis-runtime") },
            "go": { "dir": "plugins" }
        },
        "rows": [
            { "id": "greeter", "name": "go:greeter", "config": dev_config(&project) },
            { "id": "user", "name": "./user.mjs" }
        ]
    });
    std::fs::write(dir.path().join("rutis.json"), config.to_string()).unwrap();
    passes(dir.path(), r#"{"greeter":{"hello":"async"}}"#);
}

/// The Bun template: its plugin in the Bun runtime, configured as its
/// `rutis.dev.json` says; `check --strict` passes. Only its own row: the
/// row using its service would need Node as well, which the other tests
/// cover.
/// guarantee: host/new
#[test]
fn the_bun_template_declares_what_it_provides() {
    let dir = tempfile::tempdir().unwrap();
    rutis_host::new::create(dir.path(), "greeter", "bun").unwrap();
    let project = dir.path().join("greeter");
    let index = project.join("src/index.ts");
    let source = std::fs::read_to_string(&index).unwrap();
    assert!(source.contains("from '@arcships/rutis'"), "{source}");
    let source = source.replace("from '@arcships/rutis'", &format!("from '{}'", sdk()));
    std::fs::write(&index, source).unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{ "name": "app", "type": "module" }"#,
    )
    .unwrap();
    let config = json!({
        "runtimes": { "bun": { "runtime": repo().join("bun/rutis-bun") } },
        "rows": [
            { "id": "greeter", "name": "bun:./greeter/src/index.ts", "config": dev_config(&project) }
        ]
    });
    std::fs::write(dir.path().join("rutis.json"), config.to_string()).unwrap();
    let (status, stdout, stderr) = check_strict(dir.path());
    let context = format!("status {status}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(status, 0, "{context}");
    assert_eq!(stderr, "", "{context}");
    assert!(
        stdout.contains("  provides: {\"greeter\":{\"hello\":\"async\"}}\n"),
        "{context}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("greeter (") && line.ends_with("): ok")),
        "{context}"
    );
}
