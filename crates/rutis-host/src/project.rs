//! `rutis-host dev`: the plugin project in the current directory as a host
//! configuration. A Node or Bun project (package.json) or a Python project
//! (pyproject.toml) becomes one row, the plugin itself; a Go project
//! (go.mod) is built, and each plugin of its binary becomes a row;
//! `rutis.dev.json` adds what it needs to run (fake services, config, other
//! plugins).
//!
//! A package.json project runs in Bun when `rutis.dev.json` configures
//! `runtimes.bun`, when Bun manages it (`bun.lock`), or when it depends on
//! `@arcships/rutis-bun`; in Node otherwise.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{read_text, HostConfig};
use crate::languages::{self, LANGUAGES};

pub use crate::languages::GoDev;

/// The configuration that runs the plugin project in `dir`, and the id of
/// its row: of a compiled project serving several plugins, those of its
/// rows `rutis.dev.json` writes (else all), `weather, llm`.
pub fn dev_config(dir: &Path) -> Result<(HostConfig, String), String> {
    dev_config_with(dir, None, None)
}

/// [`dev_config`] with `rutis.dev.json` as `text` says, not as on disk:
/// what `add` checks before it writes the file.
pub(crate) fn dev_config_of(dir: &Path, text: &str) -> Result<(HostConfig, String), String> {
    dev_config_with(dir, None, Some(text))
}

/// [`dev_config`] for a compiled project whose first build `binary` is
/// already made: its rows are read from that binary, not from one built
/// here.
pub fn dev_config_built(dir: &Path, binary: &Path) -> Result<(HostConfig, String), String> {
    dev_config_with(dir, Some(binary), None)
}

fn dev_config_with(
    dir: &Path,
    built: Option<&Path>,
    dev_text: Option<&str>,
) -> Result<(HostConfig, String), String> {
    // Absolute, not canonical: on Windows a canonical path is a `\\?\` one,
    // which the runtimes are then given as their project.
    let dir = std::path::absolute(dir)
        .ok()
        .filter(|dir| dir.is_dir())
        .ok_or_else(|| format!("{}: not a directory", dir.display()))?;
    let Some(language) = languages::of_project(&dir) else {
        let files: Vec<String> = languages::project_files()
            .into_iter()
            .map(str::to_owned)
            .collect();
        return Err(format!(
            "{} has no {}: run rutis-host dev in a plugin project",
            dir.display(),
            languages::either(&files)
        ));
    };
    language.available()?;
    let (rows, runtimes) = language.dev(&dir, built)?;
    let own_ids: Vec<Value> = rows.iter().map(|row| row["id"].clone()).collect();
    let mut config = HostConfig {
        format: crate::config::FORMAT,
        id: "dev".into(),
        runtimes,
        listen: Vec::new(),
        installed: Default::default(),
        rows,
        warnings: Vec::new(),
        unknown_row_fields: Vec::new(),
    };
    let extra = dir.join("rutis.dev.json");
    // Its rows also run in the project's own runtime, which this sets up:
    // not a runtime the dev file leaves undeclared.
    let path = extra.clone();
    let text = match dev_text {
        Some(text) => Some(text.to_owned()),
        None if extra.exists() => Some(read_text(&extra)?),
        None => None,
    };
    let extra = match &text {
        Some(text) => Some(HostConfig::parse_beside(text, &extra, &config.runtimes)?),
        None => None,
    };
    // A row's name as the dev file writes it, before it is made absolute.
    let written_name = |id: &Value| -> Option<Value> {
        let file: Value = serde_json::from_str(text.as_deref()?).ok()?;
        let rows = file["rows"].as_array()?;
        Some(rows.iter().find(|row| row["id"] == *id)?["name"].clone())
    };
    // The project's own rows the dev file writes: what `dev` says it runs
    // (of a binary serving several plugins, not merely the first, #938).
    let mut written: Vec<String> = Vec::new();
    if let Some(extra) = extra {
        // The dev file may configure the plugin's own rows.
        let (own, others): (Vec<Value>, Vec<Value>) = extra
            .rows
            .iter()
            .cloned()
            .partition(|row| own_ids.contains(&row["id"]));
        let mut ignored: Vec<String> = Vec::new();
        for own in own {
            let target = config.rows.iter_mut().find(|row| row["id"] == own["id"]);
            if let Some(object) = target.and_then(Value::as_object_mut) {
                written.extend(own["id"].as_str().map(str::to_owned));
                for (key, value) in own.as_object().into_iter().flatten() {
                    if key != "name" {
                        object.insert(key.clone(), value.clone());
                    } else if *value != object["name"] {
                        // The project's own plugin runs, whatever the
                        // dev file names: said, not silently dropped.
                        ignored.push(format!(
                            "{}: row {}: its name {} is ignored: {} is the project's own plugin ({})",
                            path.display(),
                            own["id"].as_str().unwrap_or_default(),
                            named(&written_name(&own["id"]).unwrap_or_else(|| value.clone())),
                            own["id"].as_str().unwrap_or_default(),
                            named(&object["name"]),
                        ));
                    }
                }
            }
        }
        config.warnings.extend(ignored);
        // What the dev file says of the plugin's own runtime, where its
        // language lets it win (Bun: the Bun to use).
        for language in LANGUAGES {
            language.dev_file(&mut config.runtimes, &extra.runtimes);
        }
        config.merge(HostConfig {
            rows: others,
            ..extra
        });
    }
    // Every row of the project when the dev file writes none of them.
    let id = match written.is_empty() {
        true => own_ids
            .iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect::<Vec<_>>()
            .join(", "),
        false => written.join(", "),
    };
    Ok((config, id))
}

/// A row's `name` as a warning shows it: a string as it is, else as JSON.
fn named(name: &Value) -> String {
    match name {
        Value::String(name) => name.clone(),
        other => other.to_string(),
    }
}

/// The source file of the row named `name` under `runtimes`: the module its
/// runtime imports again when the row reloads (a TypeScript or JavaScript
/// file, a Python module's file). `None` for a row whose plugin is not a
/// file of this machine: a package, a compiled plugin, a connection row's,
/// a peer.
pub fn row_file(name: &str, runtimes: &crate::config::Runtimes) -> Option<PathBuf> {
    let file = |name: &str, base: &Path| -> Option<PathBuf> {
        if name.starts_with("file:") {
            return url::Url::parse(name).ok()?.to_file_path().ok();
        }
        let path = Path::new(name);
        if path.is_absolute() {
            return Some(path.to_owned());
        }
        let relative = ["./", "../"]
            .iter()
            .chain(if cfg!(windows) {
                &[".\\", "..\\"][..]
            } else {
                &[]
            })
            .any(|prefix| name.starts_with(prefix));
        relative.then(|| base.join(path).components().collect())
    };
    if let Some((prefix, rest)) = name.split_once(':') {
        if let (Some(py), "py") = (&runtimes.py, prefix) {
            // `a.b`: a/b.py, or the package a/b/__init__.py.
            let module = py.project.join(rest.replace('.', "/"));
            return [module.with_extension("py"), module.join("__init__.py")]
                .into_iter()
                .find(|path| path.is_file());
        }
        if let (Some(bun), "bun") = (&runtimes.bun, prefix) {
            return file(rest, &bun.project);
        }
        if rutis_loader::runtime::is_runtime_name(prefix) {
            // Another runtime's: Go, Swift, a connection row's.
            return None;
        }
    }
    let node = runtimes.node.as_ref()?;
    file(name, &node.project)
}

/// The files a change in which reloads the plugin: sources, not
/// dependencies or build output.
pub fn sources(dir: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    const SKIP: &[&str] = &[
        "node_modules",
        ".git",
        ".venv",
        "venv",
        "target",
        "dist",
        "build",
        "__pycache__",
        ".pytest_cache",
    ];
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') && name != ".env" || SKIP.contains(&name.as_ref()) {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => {
                    if let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) {
                        found.push((path, modified));
                    }
                }
                _ => {}
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A package.json project Bun manages runs in Bun, its source as the
    /// row; the dev file may name the Bun to use.
    #[test]
    fn a_bun_project_runs_in_bun() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{ "name": "greeter" }"#).unwrap();
        std::fs::write(dir.path().join("bun.lock"), "").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/index.ts"), "").unwrap();
        let (config, id) = dev_config(dir.path()).unwrap();
        assert_eq!(id, "greeter");
        let name = config.rows[0]["name"].as_str().unwrap();
        assert!(
            name.starts_with("bun:") && name.ends_with("index.ts"),
            "{name}"
        );
        assert!(config.runtimes.bun.is_some() && config.runtimes.node.is_none());

        std::fs::remove_file(dir.path().join("bun.lock")).unwrap();
        assert!(dev_config(dir.path()).unwrap().0.runtimes.node.is_some());
        std::fs::write(
            dir.path().join("rutis.dev.json"),
            r#"{ "runtimes": { "bun": { "program": "/opt/bun/bin/bun" } } }"#,
        )
        .unwrap();
        let (config, _) = dev_config(dir.path()).unwrap();
        let bun = config.runtimes.bun.unwrap();
        // On Windows `/opt/...` is relative to the drive, and rebased as such.
        assert!(bun.program.unwrap().ends_with("opt/bun/bin/bun"));
    }

    #[test]
    fn a_node_project_runs_its_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "name": "@acme/weather-plugin" }"#,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/index.ts"), "").unwrap();
        std::fs::write(
            dir.path().join("rutis.dev.json"),
            r#"{ "rows": [{ "id": "weather-plugin", "config": { "city": "Oslo" } }, { "id": "llm", "name": "./fake.mjs" }] }"#,
        )
        .unwrap();
        let (config, id) = dev_config(dir.path()).unwrap();
        assert_eq!(id, "weather-plugin");
        assert!(config.rows[0]["name"]
            .as_str()
            .unwrap()
            .ends_with("/src/index.ts"));
        assert_eq!(config.rows[0]["config"]["city"], "Oslo");
        assert_eq!(config.rows[1]["id"], "llm");
        assert!(config.runtimes.node.is_some());
    }

    /// guarantee: host/dev-reload
    /// The dev file's rows in the project's own runtime run in the runtime
    /// `dev` sets up: no word of an undeclared runtime (#666). A row of
    /// another language's runtime the file does not declare still has
    /// one, and that runtime runs with its defaults.
    #[test]
    fn the_projects_own_runtime_is_not_undeclared() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{ "name": "weather" }"#).unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/index.ts"), "").unwrap();
        let dev = |rows: &str| {
            std::fs::write(
                dir.path().join("rutis.dev.json"),
                format!(r#"{{ "rows": [{{ "id": "weather" }}, {rows}] }}"#),
            )
            .unwrap();
            dev_config(dir.path()).unwrap().0
        };
        let config = dev(r#"{ "id": "llm", "name": "./dev/fake-llm.ts" }"#);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert!(config.runtimes.implicit.is_empty());
        let config = dev(r#"{ "id": "llm", "name": "py:fake_llm" }"#);
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
        assert!(config.warnings[0].contains("rows use the py runtime (py:fake_llm)"));
        assert_eq!(config.runtimes.implicit, ["py"]);

        let python = tempfile::tempdir().unwrap();
        std::fs::write(
            python.path().join("pyproject.toml"),
            "[project]\nname = \"weather\"\n",
        )
        .unwrap();
        std::fs::write(
            python.path().join("rutis.dev.json"),
            r#"{ "rows": [{ "id": "weather" }, { "id": "llm", "name": "py:fake_llm" }] }"#,
        )
        .unwrap();
        let config = dev_config(python.path()).unwrap().0;
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert!(config.runtimes.implicit.is_empty());
    }

    /// A Python project with a package.json beside its pyproject.toml, for
    /// a fake service in TypeScript (`npm init`'s `"main": "index.js"`, no
    /// such file): still a Python project (#938). One whose package.json
    /// names a plugin that is there is a Node project, as before.
    #[test]
    fn a_package_json_beside_pyproject_is_not_a_node_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"weather\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "name": "weather", "main": "index.js", "devDependencies": { "@arcships/rutis": "*" } }"#,
        )
        .unwrap();
        let (config, id) = dev_config(dir.path()).unwrap();
        assert_eq!(
            (id.as_str(), &config.rows[0]["name"]),
            ("weather", &json!("py:weather"))
        );
        assert!(config.runtimes.node.is_none());

        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/index.ts"), "").unwrap();
        let (config, _) = dev_config(dir.path()).unwrap();
        assert!(config.runtimes.node.is_some());
    }

    #[test]
    fn a_python_project_runs_its_entry_point() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"weather-plugin\"\n\n[project.entry-points.\"rutis.plugins\"]\nweather = \"weather_plugin\"\n",
        )
        .unwrap();
        let (config, id) = dev_config(dir.path()).unwrap();
        assert_eq!(
            (id.as_str(), &config.rows[0]["name"]),
            ("weather", &json!("py:weather_plugin"))
        );
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"weather-plugin\"\n",
        )
        .unwrap();
        assert_eq!(
            dev_config(dir.path()).unwrap().0.rows[0]["name"],
            "py:weather_plugin"
        );
    }

    /// A project `rutis-host new --lang go` makes builds and becomes one row
    /// per plugin of its binary (its own test needs `rutis-host test`: it
    /// runs in `tests/templates.rs`, with the binary just built).
    #[test]
    fn a_go_project_is_built_into_rows() {
        let dir = tempfile::tempdir().unwrap();
        crate::new::create(dir.path(), "net-probe", "go").unwrap();
        let project = dir.path().join("net-probe");
        // The SDK of this checkout, not a published one.
        let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../go/rutis");
        let module = std::fs::read_to_string(project.join("go.mod")).unwrap();
        std::fs::write(
            project.join("go.mod"),
            format!(
                "{module}\nreplace github.com/arcships/rutis/go/rutis => {}\n",
                sdk.display()
            ),
        )
        .unwrap();
        {
            let check = ["vet", "./..."];
            let output = std::process::Command::new("go")
                .args(check)
                .current_dir(&project)
                .output()
                .expect("go is on PATH");
            assert!(
                output.status.success(),
                "go {check:?}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let (config, id) = dev_config(&project).unwrap();
        assert_eq!(id, "net-probe");
        assert_eq!(config.rows[0]["name"], "go:net-probe");
        assert_eq!(
            config.rows[0]["config"]["greeting"], "Hello",
            "rutis.dev.json configures it"
        );
        let go = config.runtimes.go.unwrap();
        let (binary, runtime) = go.dev.unwrap();
        assert_eq!(runtime, "go-net-probe");
        assert!(binary.starts_with(project.join(".rutis/go")));
        // A failed build says why.
        std::fs::write(
            project.join("plugin.go"),
            "package netprobe\nfunc broken(\n",
        )
        .unwrap();
        let go = GoDev::of(&project).unwrap();
        let error = go.build(2).unwrap_err();
        assert!(error.contains("plugin.go"), "{error}");
    }

    /// A Swift project is refused where its binaries cannot run, before
    /// `swift` is run.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_swift_project_needs_macos_before_building() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Package.swift"),
            "let package = Package(name: \"probe\")",
        )
        .unwrap();
        let error = dev_config(dir.path()).unwrap_err();
        assert_eq!(
            error,
            "runtimes.swift needs macOS: Swift plugin binaries run on macOS only"
        );
    }

    /// A `rutis.json` that configures Swift is refused where its binaries
    /// cannot run, saying which file.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_swift_runtime_in_a_config_needs_macos() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rutis.json");
        std::fs::write(
            &path,
            r#"{ "runtimes": { "swift": { "binaries": ["bin/MacKit"] } } }"#,
        )
        .unwrap();
        let error = HostConfig::read(&path).unwrap_err();
        assert_eq!(
            error,
            format!(
                "{}: runtimes.swift needs macOS: Swift plugin binaries run on macOS only",
                path.display()
            )
        );
    }

    /// A compiled project's first build may be made by the caller: its
    /// rows come from that binary, without building again.
    #[cfg(unix)]
    #[test]
    fn a_built_go_binary_makes_the_dev_rows() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("go.mod"),
            "module example.com/net-probe\n\ngo 1.24\n",
        )
        .unwrap();
        let binary = dir.path().join("net-probe-1");
        std::fs::write(
            &binary,
            "#!/bin/sh\n[ \"$1\" = --rutis-manifest ] || exit 1\n\
             echo '{\"pluginApi\":3,\"plugins\":{\"probe\":{\"inject\":[],\"provides\":{}}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (config, id) = dev_config_built(dir.path(), &binary).unwrap();
        assert_eq!(id, "probe");
        assert_eq!(config.rows[0]["name"], "go:probe");
        let go = config.runtimes.go.unwrap();
        assert_eq!(go.dev, Some((binary, "go-net-probe".to_owned())));
    }

    /// A binary serving two plugins (`rutis.Serve(weather.Plugin,
    /// llm.Plugin)`): the project is the rows the dev file writes of them,
    /// not the first by name, else all of them; a name the dev file gives
    /// one of them is said to be ignored, not dropped silently (#938).
    #[cfg(unix)]
    #[test]
    fn a_binary_of_several_plugins_is_the_rows_the_dev_file_writes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("go.mod"),
            "module example.com/weather\n\ngo 1.24\n",
        )
        .unwrap();
        let binary = dir.path().join("weather-1");
        std::fs::write(
            &binary,
            "#!/bin/sh\n[ \"$1\" = --rutis-manifest ] || exit 1\n\
             echo '{\"pluginApi\":3,\"plugins\":{\"llm\":{\"inject\":[],\"provides\":{}},\"weather\":{\"inject\":[],\"provides\":{}}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (config, id) = dev_config_built(dir.path(), &binary).unwrap();
        assert_eq!(id, "llm, weather");
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);

        let dev = dir.path().join("rutis.dev.json");
        std::fs::write(
            &dev,
            r#"{ "rows": [{ "id": "weather", "config": { "city": "Oslo" } }] }"#,
        )
        .unwrap();
        let (config, id) = dev_config_built(dir.path(), &binary).unwrap();
        assert_eq!(id, "weather");
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);

        std::fs::write(
            &dev,
            r#"{ "rows": [{ "id": "weather" }, { "id": "llm", "name": "./dev/fake-llm.ts" }] }"#,
        )
        .unwrap();
        let (config, id) = dev_config_built(dir.path(), &binary).unwrap();
        assert_eq!(id, "weather, llm");
        let llm = config.rows.iter().find(|row| row["id"] == "llm").unwrap();
        assert_eq!(llm["name"], "go:llm");
        assert_eq!(
            config.warnings[0],
            format!(
                "{}: row llm: its name ./dev/fake-llm.ts is ignored: llm is the project's own plugin (go:llm)",
                std::path::absolute(&dev).unwrap().display()
            ),
            "{:?}",
            config.warnings
        );
    }
}
