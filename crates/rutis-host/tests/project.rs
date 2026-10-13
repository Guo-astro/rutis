//! A plugin project as `rutis-host dev` (and `check` without a
//! `rutis.json`) runs it: which kind it is, its entry point, and what
//! `rutis.dev.json` adds.

mod common;

use std::path::Path;

use common::{normalize, write};
use rutis_host::project::{dev_config, GoDev};
use serde_json::json;

/// The row name, as on any machine.
fn row_name(config: &rutis_host::config::HostConfig, dir: &Path) -> String {
    normalize(config.rows[0]["name"].as_str().unwrap(), dir)
}

/// A package.json project runs its source: src/index.ts, .mts, .js, .mjs
/// in that order, else what the package exports, else its main.
/// risk: B5
#[test]
fn a_node_projects_entry_point() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        project,
        &[(
            "package.json",
            r#"{ "name": "@acme/weather-plugin", "main": "lib/main.js" }"#,
        )],
    );
    let entry = || {
        let (config, id) = dev_config(project).unwrap();
        assert_eq!(id, "weather-plugin");
        assert!(config.runtimes.node.is_some() && config.runtimes.bun.is_none());
        row_name(&config, project)
    };
    assert_eq!(entry(), "file://<dir>/lib/main.js");
    write(
        project,
        &[(
            "package.json",
            r#"{ "name": "@acme/weather-plugin", "exports": "./lib/exports.js", "main": "lib/main.js" }"#,
        )],
    );
    assert_eq!(entry().rsplit('/').next(), Some("exports.js"));
    write(
        project,
        &[(
            "package.json",
            r#"{ "name": "@acme/weather-plugin", "exports": { ".": "./lib/dot.js" } }"#,
        )],
    );
    assert_eq!(entry().rsplit('/').next(), Some("dot.js"));
    write(
        project,
        &[(
            "package.json",
            r#"{ "name": "@acme/weather-plugin", "exports": { ".": { "default": "./lib/default.js" } } }"#,
        )],
    );
    assert_eq!(entry().rsplit('/').next(), Some("default.js"));
    for source in [
        "src/index.mjs",
        "src/index.js",
        "src/index.mts",
        "src/index.ts",
    ] {
        write(project, &[(source, "")]);
        assert_eq!(entry(), format!("file://<dir>/{source}"));
    }
}

/// A package.json project runs in Bun when Bun manages it, when it depends
/// on `@arcships/rutis-bun`, or when `rutis.dev.json` configures Bun.
/// risk: B5
#[test]
fn a_package_json_project_in_bun_or_node() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        project,
        &[
            ("package.json", r#"{ "name": "greeter" }"#),
            ("src/index.ts", ""),
        ],
    );
    let in_bun = || {
        let (config, _) = dev_config(project).unwrap();
        assert_ne!(
            config.runtimes.bun.is_some(),
            config.runtimes.node.is_some()
        );
        config.runtimes.bun.is_some()
    };
    assert!(!in_bun());
    for lock in ["bun.lock", "bun.lockb"] {
        write(project, &[(lock, "")]);
        assert!(in_bun(), "{lock}");
        std::fs::remove_file(project.join(lock)).unwrap();
    }
    for field in ["dependencies", "devDependencies"] {
        let manifest = json!({ "name": "greeter", field: { "@arcships/rutis-bun": "^0.10" } });
        write(project, &[("package.json", &manifest.to_string())]);
        assert!(in_bun(), "{field}");
    }
    write(project, &[("package.json", r#"{ "name": "greeter" }"#)]);
    write(
        project,
        &[(
            "rutis.dev.json",
            r#"{ "runtimes": { "bun": { "program": "tools/bun" } } }"#,
        )],
    );
    let (config, _) = dev_config(project).unwrap();
    // The source runs in Bun, as a path; the dev file's Bun is the one.
    assert_eq!(row_name(&config, project), "bun:<dir>/src/index.ts");
    let bun = config.runtimes.bun.unwrap();
    assert_eq!(bun.project, project);
    assert_eq!(bun.program.unwrap(), project.join("tools/bun"));
}

/// A pyproject.toml project runs the module of its first `rutis.plugins`
/// entry point, else its name as a module; from src/ in that layout, with
/// its .venv when there is one.
/// risk: B5
#[test]
fn a_python_projects_entry_point() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        project,
        &[(
            "pyproject.toml",
            "[project]\nname = \"weather-plugin\"\n\n[project.entry-points.\"rutis.plugins\"]\n# a comment\nweather = \"weather_plugin.main:plugin\"\nsecond = \"other\"\n",
        )],
    );
    let (config, id) = dev_config(project).unwrap();
    assert_eq!(
        (id.as_str(), row_name(&config, project).as_str()),
        ("weather", "py:weather_plugin.main")
    );
    let py = config.runtimes.py.unwrap();
    assert_eq!(py.project, project);
    assert_eq!(py.python, None, "no .venv yet");

    write(
        project,
        &[
            ("pyproject.toml", "[project]\nname = \"weather-plugin\"\n"),
            ("src/weather_plugin/__init__.py", ""),
        ],
    );
    let venv = rutis_host::config::venv_python(&project.join(".venv"));
    write(
        project,
        &[(venv.strip_prefix(project).unwrap().to_str().unwrap(), "")],
    );
    let (config, id) = dev_config(project).unwrap();
    assert_eq!(
        (id.as_str(), row_name(&config, project).as_str()),
        ("weather-plugin", "py:weather_plugin")
    );
    let py = config.runtimes.py.unwrap();
    assert_eq!(py.project, project.join("src"));
    assert_eq!(py.python, Some(venv));

    write(project, &[("pyproject.toml", "[tool.uv]\n")]);
    assert_eq!(
        dev_config(project).unwrap_err(),
        "pyproject.toml has no [project] name"
    );
}

/// A project that is both is a package.json project; one that is neither
/// is refused, saying what is looked for.
/// risk: A7
#[test]
fn which_kind_of_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let error = dev_config(project).unwrap_err();
    assert_eq!(
        normalize(&error, project),
        "<dir> has no package.json, pyproject.toml or go.mod: run rutis-host dev in a plugin project"
    );
    let error = dev_config(&project.join("absent")).unwrap_err();
    assert_eq!(normalize(&error, project), "<dir>/absent: not a directory");
    write(project, &[("package.json", r#"{ "name": "both" }"#)]);
    assert_eq!(
        dev_config(project).unwrap_err(),
        "package.json names no entry, and there is no src/index.ts"
    );
    write(
        project,
        &[
            ("pyproject.toml", "[project]\nname = \"both\"\n"),
            ("src/index.ts", ""),
        ],
    );
    assert!(dev_config(project).unwrap().0.runtimes.node.is_some());
}

/// The main package of a Go project: `cmd/<module name>`, else the first
/// command under cmd/ with a main.go, else the module itself. (It is not
/// built here: that needs Go, and a_go_project_is_built_into_rows in the
/// crate does it.)
/// risk: B5
#[test]
fn a_go_projects_main_package() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    assert!(GoDev::of(project).is_none());
    write(
        project,
        &[("go.mod", "module example.com/acme/Net-Probe\n\ngo 1.24\n")],
    );
    let go = GoDev::of(project).unwrap();
    assert_eq!((go.name.as_str(), go.package.as_str()), ("net-probe", "."));
    assert_eq!(go.runtime, "go-net-probe");
    write(
        project,
        &[
            ("cmd/zeta/main.go", ""),
            ("cmd/alpha/main.go", ""),
            ("cmd/empty/README", ""),
        ],
    );
    assert_eq!(GoDev::of(project).unwrap().package, "./cmd/alpha");
    write(project, &[("cmd/net-probe/main.go", "")]);
    assert_eq!(GoDev::of(project).unwrap().package, "./cmd/net-probe");
}

/// `rutis.dev.json` configures the plugin's own row (not its name), adds
/// rows, listeners and runtimes the project has not, and its relative
/// paths are the project's.
/// risk: B5
#[test]
fn the_dev_file_is_merged_into_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write(
        project,
        &[
            ("package.json", r#"{ "name": "weather-plugin" }"#),
            ("src/index.ts", ""),
            (
                "rutis.dev.json",
                r#"{
                  "id": "ignored",
                  "runtimes": {
                    "node": { "project": "elsewhere" },
                    "py": { "project": "fakes" },
                    "remote": [{ "name": "gpu", "language": "python" }]
                  },
                  "listen": [{ "name": "dev", "address": "127.0.0.1:0" }],
                  "rows": [
                    { "id": "weather-plugin", "name": "not-this", "config": { "city": "Oslo" }, "inject": ["llm"] },
                    { "id": "llm", "name": "./fakes/llm.mjs" },
                    { "id": "geo", "name": "py:geo" }
                  ]
                }"#,
            ),
        ],
    );
    let (config, id) = dev_config(project).unwrap();
    assert_eq!((config.id.as_str(), id.as_str()), ("dev", "weather-plugin"));
    let rows: Vec<serde_json::Value> = config
        .rows
        .iter()
        .map(|row| {
            let mut row = row.clone();
            row["name"] = json!(normalize(row["name"].as_str().unwrap(), project));
            row
        })
        .collect();
    assert_eq!(
        rows,
        [
            json!({ "id": "weather-plugin", "name": "file://<dir>/src/index.ts", "config": { "city": "Oslo" }, "inject": ["llm"] }),
            json!({ "id": "llm", "name": "file://<dir>/fakes/llm.mjs" }),
            json!({ "id": "geo", "name": "py:geo" }),
        ]
    );
    // The project's own runtime is the project's.
    assert_eq!(config.runtimes.node.unwrap().project, project);
    assert_eq!(config.runtimes.py.unwrap().project, project.join("fakes"));
    assert_eq!(config.runtimes.remote[0].name, "gpu");
    assert_eq!(config.listen[0].name, "dev");

    write(
        project,
        &[(
            "rutis.dev.json",
            r#"{ "rows": [{ "id": "x", "nam": "y" }] }"#,
        )],
    );
    // The dev file's warnings are the configuration's.
    let (config, _) = dev_config(project).unwrap();
    let warnings: Vec<String> = config
        .warnings
        .iter()
        .map(|warning| normalize(warning, project))
        .collect();
    assert_eq!(
        warnings,
        ["<dir>/rutis.dev.json: rows[0] (id \"x\"): unknown field `nam` is ignored; a row has `id`, `name`, `config`, `inject`, `isolate`, `disabled`, `group`, `instanced`"]
    );
}
