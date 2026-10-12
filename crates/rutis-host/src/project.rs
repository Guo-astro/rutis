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

use serde_json::{json, Value};

use crate::config::{
    BunRuntime, GoRuntime, GoStart, HostConfig, NodeRuntime, PythonRuntime, Runtimes,
};

/// The configuration that runs the plugin project in `dir`, and the id of
/// its row.
pub fn dev_config(dir: &Path) -> Result<(HostConfig, String), String> {
    // Absolute, not canonical: on Windows a canonical path is a `\\?\` one,
    // which the runtimes are then given as their project.
    let dir = std::path::absolute(dir)
        .ok()
        .filter(|dir| dir.is_dir())
        .ok_or_else(|| format!("{}: not a directory", dir.display()))?;
    let (rows, runtimes) = if dir.join("package.json").exists() && is_bun(&dir) {
        let (row, runtimes) = bun_row(&dir)?;
        (vec![row], runtimes)
    } else if dir.join("package.json").exists() {
        let (row, runtimes) = node_row(&dir)?;
        (vec![row], runtimes)
    } else if dir.join("pyproject.toml").exists() {
        let (row, runtimes) = python_row(&dir)?;
        (vec![row], runtimes)
    } else if let Some(go) = GoDev::of(&dir) {
        go_rows(&go)?
    } else {
        return Err(format!(
            "{} has no package.json, pyproject.toml or go.mod: run rutis-host dev in a plugin project",
            dir.display()
        ));
    };
    let id = rows[0]["id"].as_str().unwrap_or("plugin").to_owned();
    let own_ids: Vec<Value> = rows.iter().map(|row| row["id"].clone()).collect();
    let mut config = HostConfig {
        format: crate::config::FORMAT,
        id: "dev".into(),
        runtimes,
        listen: Vec::new(),
        rows,
        warnings: Vec::new(),
    };
    let extra = dir.join("rutis.dev.json");
    if extra.exists() {
        let extra = HostConfig::read(&extra)?;
        // The dev file may configure the plugin's own rows.
        let (own, others): (Vec<Value>, Vec<Value>) = extra
            .rows
            .iter()
            .cloned()
            .partition(|row| own_ids.contains(&row["id"]));
        for own in own {
            let target = config.rows.iter_mut().find(|row| row["id"] == own["id"]);
            if let Some(object) = target.and_then(Value::as_object_mut) {
                for (key, value) in own.as_object().into_iter().flatten() {
                    if key != "name" {
                        object.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        // What the dev file says of the plugin's own runtime wins.
        if let (Some(_), Some(bun)) = (&config.runtimes.bun, &extra.runtimes.bun) {
            config.runtimes.bun = Some(bun.clone());
        }
        config.merge(HostConfig {
            rows: others,
            ..extra
        });
    }
    Ok((config, id))
}

/// A Go plugin project as `rutis-host dev` builds it: its main package and
/// the runtime its binary runs as. Builds go to `.rutis/go/<name>-<n>`, a
/// new file each time (a running binary cannot be overwritten on Windows).
#[derive(Debug, Clone)]
pub struct GoDev {
    pub dir: PathBuf,
    pub name: String,
    /// The main package (`./cmd/<name>`, or `.`).
    pub package: String,
    pub runtime: String,
}

impl GoDev {
    /// The Go project in `dir`, if it is one (a go.mod).
    pub fn of(dir: &Path) -> Option<Self> {
        let module = std::fs::read_to_string(dir.join("go.mod")).ok()?;
        let path = module
            .lines()
            .find_map(|line| line.trim().strip_prefix("module "))?
            .trim()
            .trim_matches('"');
        let name = id_of(path).to_lowercase();
        let package = if dir.join("cmd").join(&name).is_dir() {
            format!("./cmd/{name}")
        } else {
            std::fs::read_dir(dir.join("cmd"))
                .ok()
                .and_then(|entries| {
                    let mut commands: Vec<String> = entries
                        .flatten()
                        .filter(|entry| entry.path().join("main.go").exists())
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect();
                    commands.sort();
                    commands.into_iter().next()
                })
                .map(|command| format!("./cmd/{command}"))
                .unwrap_or_else(|| ".".into())
        };
        let runtime = rutis_bridge::runtime::go_runtime_name(Path::new(&name));
        Some(Self {
            dir: dir.to_owned(),
            name,
            package,
            runtime,
        })
    }

    /// Build #`n`: its binary, or the compiler's errors.
    pub fn build(&self, n: u32) -> Result<PathBuf, String> {
        let (mut command, binary) = self.build_command(n)?;
        built(command.output(), binary)
    }

    /// [`GoDev::build`], without blocking; dropped, it kills the go
    /// command.
    pub async fn rebuild(&self, n: u32) -> Result<PathBuf, String> {
        let (command, binary) = self.build_command(n)?;
        let output = tokio::process::Command::from(command)
            .kill_on_drop(true)
            .output()
            .await;
        built(output, binary)
    }

    /// The command that builds #`n`, and the binary it writes.
    fn build_command(&self, n: u32) -> Result<(std::process::Command, PathBuf), String> {
        let builds = self.dir.join(".rutis").join("go");
        std::fs::create_dir_all(&builds).map_err(|error| error.to_string())?;
        let file = match cfg!(windows) {
            true => format!("{}-{n}.exe", self.name),
            false => format!("{}-{n}", self.name),
        };
        let binary = builds.join(file);
        let mut command = std::process::Command::new("go");
        command
            .args(["build", "-o"])
            .arg(&binary)
            .arg(&self.package)
            .current_dir(&self.dir);
        Ok((command, binary))
    }
}

/// `binary`, if the build that wrote it succeeded; else the compiler's
/// errors.
fn built(
    output: std::io::Result<std::process::Output>,
    binary: PathBuf,
) -> Result<PathBuf, String> {
    let output =
        output.map_err(|error| format!("cannot run go (is the Go toolchain on PATH?): {error}"))?;
    match output.status.success() {
        true => Ok(binary),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

/// The rows of a Go project: build it, then one row per plugin of its
/// binary, named with its runtime (`go-<name>:<plugin>`).
fn go_rows(go: &GoDev) -> Result<(Vec<Value>, Runtimes), String> {
    let binary = go.build(1)?;
    let manifest = rutis_loader::read_go_manifest(&binary)
        .map_err(|error| format!("{}: {error}", binary.display()))?;
    let rows: Vec<Value> = manifest
        .plugins
        .keys()
        .map(|plugin| json!({ "id": id_of(plugin), "name": format!("{}:{plugin}", go.runtime), "config": {} }))
        .collect();
    if rows.is_empty() {
        return Err(format!("{} serves no plugins", go.package));
    }
    let runtimes = Runtimes {
        go: Some(GoRuntime {
            dir: None,
            binaries: Vec::new(),
            start: GoStart::OnDemand,
            idle: None,
            project: go.dir.clone(),
            dev: Some((binary, go.runtime.clone())),
        }),
        ..Runtimes::default()
    };
    Ok((rows, runtimes))
}

/// Whether the package.json project in `dir` runs in Bun.
fn is_bun(dir: &Path) -> bool {
    let read = |file: &str| -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(dir.join(file)).ok()?).ok()
    };
    let configured = read("rutis.dev.json").is_some_and(|dev| !dev["runtimes"]["bun"].is_null());
    let managed = ["bun.lock", "bun.lockb"]
        .iter()
        .any(|lock| dir.join(lock).exists());
    let depends = read("package.json").is_some_and(|manifest| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|field| !manifest[field]["@arcships/rutis-bun"].is_null())
    });
    configured || managed || depends
}

/// The package's name and the source file of its plugin while developing:
/// src/index.ts or .js, else what the package exports.
fn package_entry(dir: &Path) -> Result<(String, PathBuf), String> {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("package.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("package.json: {error}"))?;
    let name = manifest["name"].as_str().unwrap_or("plugin").to_owned();
    let entry = [
        "src/index.ts",
        "src/index.mts",
        "src/index.js",
        "src/index.mjs",
    ]
    .iter()
    .map(|path| dir.join(path))
    .find(|path| path.exists())
    .or_else(|| {
        let exported = manifest["exports"]["."]["default"]
            .as_str()
            .or_else(|| manifest["exports"]["."].as_str())
            .or_else(|| manifest["exports"].as_str())
            .or_else(|| manifest["main"].as_str())?;
        Some(dir.join(exported))
    })
    .ok_or("package.json names no entry, and there is no src/index.ts")?;
    Ok((name, entry))
}

fn node_row(dir: &Path) -> Result<(Value, Runtimes), String> {
    let (name, entry) = package_entry(dir)?;
    let row = json!({ "id": id_of(&name), "name": file_url(&entry), "config": {} });
    let runtimes = Runtimes {
        node: Some(NodeRuntime {
            project: dir.to_owned(),
            runtime: None,
        }),
        ..Runtimes::default()
    };
    Ok((row, runtimes))
}

fn bun_row(dir: &Path) -> Result<(Value, Runtimes), String> {
    let (name, entry) = package_entry(dir)?;
    let row =
        json!({ "id": id_of(&name), "name": format!("bun:{}", entry.display()), "config": {} });
    let runtimes = Runtimes {
        bun: Some(BunRuntime {
            project: dir.to_owned(),
            runtime: None,
            program: None,
        }),
        ..Runtimes::default()
    };
    Ok((row, runtimes))
}

fn python_row(dir: &Path) -> Result<(Value, Runtimes), String> {
    let text =
        std::fs::read_to_string(dir.join("pyproject.toml")).map_err(|error| error.to_string())?;
    // The module of the first entry point of the group rutis.plugins (so it
    // runs from the source, installed or not), else the project name as a
    // module.
    let (id, module) = entry_point(&text)
        .or_else(|| project_name(&text).map(|name| (name.clone(), name.replace('-', "_"))))
        .ok_or("pyproject.toml has no [project] name")?;
    let row = json!({ "id": id_of(&id), "name": format!("py:{module}"), "config": {} });
    // Plugin modules are found in the project, or in src/ for that layout.
    let project = match dir.join("src").is_dir() {
        true => dir.join("src"),
        false => dir.to_owned(),
    };
    let runtimes = Runtimes {
        py: Some(PythonRuntime {
            project,
            python: Some(crate::config::venv_python(&dir.join(".venv")))
                .filter(|venv| venv.exists()),
        }),
        ..Runtimes::default()
    };
    Ok((row, runtimes))
}

/// The first entry under `[project.entry-points."rutis.plugins"]`: its name
/// and its module.
fn entry_point(pyproject: &str) -> Option<(String, String)> {
    let mut inside = false;
    for line in pyproject.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == r#"[project.entry-points."rutis.plugins"]"#;
            continue;
        }
        if inside {
            if let Some((name, value)) = line.split_once('=') {
                let name = name.trim().trim_matches('"');
                let module = value
                    .trim()
                    .trim_matches('"')
                    .split(':')
                    .next()
                    .unwrap_or("");
                if !name.is_empty() && !name.starts_with('#') && !module.is_empty() {
                    return Some((name.to_owned(), module.to_owned()));
                }
            }
        }
    }
    None
}

fn project_name(pyproject: &str) -> Option<String> {
    let mut inside = false;
    for line in pyproject.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[project]";
            continue;
        }
        if let (true, Some(("name", value))) =
            (inside, line.split_once('=').map(|(k, v)| (k.trim(), v)))
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

/// A row id from a package or module name.
fn id_of(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn file_url(path: &Path) -> String {
    crate::config::file_url(path)
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

    /// A project `rutis-host new --lang go` makes builds, passes its own
    /// test, and becomes one row per plugin of its binary.
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
        for check in [&["vet", "./..."][..], &["test", "./..."][..]] {
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
        assert_eq!(config.rows[0]["name"], "go-net-probe:net-probe");
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
}
