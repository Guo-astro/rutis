//! Go: `runtimes.go`, a directory of compiled binaries, one runtime each
//! (`go-<binary>`), started when a row uses it (or with the host); rows
//! `go:<plugin>` and `go-<binary>:<plugin>`.
//!
//! Unlike the other languages it has no single runtime: it overrides
//! [`Language::start`] to add the binaries' resolver to the chain, and
//! mounts the runtimes once the loader runs ([`super::Later`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis::Ctx;
use rutis_bridge::runtime::Stdio;
use rutis_loader::{GoBinaries, GoResolver, GoRuntimes, ServiceCatalog};
use serde_json::{json, Value};

use super::{id_of, Language, Mount, LANGUAGES};
use crate::config::{GoRuntime, GoStart, HostConfig, Runtimes};
use crate::host::{Error, GoHost};

pub struct Go;

impl Language for Go {
    fn key(&self) -> &'static str {
        "go"
    }

    fn configured(&self, runtimes: &Runtimes) -> bool {
        runtimes.go.is_some()
    }

    fn rebase(&self, runtimes: &mut Runtimes, at: &dyn Fn(&mut PathBuf)) {
        if let Some(go) = &mut runtimes.go {
            if let Some(dir) = &mut go.dir {
                at(dir);
            }
            for binary in &mut go.binaries {
                at(binary);
            }
            at(&mut go.project);
        }
    }

    fn merge(&self, runtimes: &mut Runtimes, other: &mut Runtimes) {
        if runtimes.go.is_none() {
            runtimes.go = other.go.take();
        }
    }

    /// `go:<plugin>` and `go-<binary>:<plugin>` are the Go runtimes' rows.
    fn claims(&self, name: &str) -> Option<String> {
        (name == "go" || name.starts_with("go-")).then(|| {
            format!(
                "remote runtime {name}: go and names starting with go- are the local Go runtimes'"
            )
        })
    }

    fn remote_names(&self) -> &'static [&'static str] {
        &["go"]
    }

    /// The binaries' resolver now; their runtimes once the loader runs.
    fn start(&self, config: &HostConfig, mount: &mut Mount<'_>) -> Result<(), Error> {
        let Some(go) = &config.runtimes.go else {
            return Ok(());
        };
        let resolver = Arc::new(resolver(config, go, mount.catalog)?);
        mount.resolve(resolver.clone());
        let (project, start, idle) = (go.project.clone(), go.start, go.idle);
        mount.later.push(Box::new(move |root: Ctx| {
            Box::pin(async move {
                let mut plugin =
                    GoRuntimes::new(resolver.clone(), &project).output(Stdio::Lines, Stdio::Lines);
                plugin = match start {
                    GoStart::Eager => plugin.eager(),
                    GoStart::OnDemand => {
                        plugin.idle(Some(std::time::Duration::from_secs(idle.unwrap_or(60))))
                    }
                };
                let runtimes = plugin.handle();
                (&root.plugin(plugin))
                    .await
                    .map_err(|error| format!("the Go runtimes: {error}"))?;
                Ok(Some(GoHost { resolver, runtimes }))
            })
        }));
        Ok(())
    }

    fn new_names(&self) -> &'static [&'static str] {
        &["go", "golang"]
    }

    fn template(&self) -> &'static [(&'static str, &'static str)] {
        TEMPLATE
    }

    fn next_steps(&self) -> &'static str {
        "go mod tidy && go test ./... && rutis-host dev"
    }

    fn project_file(&self) -> &'static str {
        "go.mod"
    }

    fn is_project(&self, dir: &Path) -> bool {
        GoDev::of(dir).is_some()
    }

    /// Build it, then one row per plugin of its binary, named with its
    /// runtime (`go-<name>:<plugin>`).
    fn dev(&self, dir: &Path) -> Result<(Vec<Value>, Runtimes), String> {
        let go = GoDev::of(dir).ok_or_else(|| format!("{}: no go.mod", dir.display()))?;
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
}

/// The resolver of the Go binaries `go` names. A Go runtime may not take
/// another runtime's name: another language's, or a remote runtime's.
fn resolver(
    config: &HostConfig,
    go: &GoRuntime,
    catalog: &ServiceCatalog,
) -> Result<GoResolver, Error> {
    go.validate()?;
    let mut binaries = GoBinaries::new();
    if let Some(dir) = &go.dir {
        binaries = binaries.dir(dir);
    }
    for binary in &go.binaries {
        binaries = binaries.file(binary);
    }
    if let Some((binary, name)) = &go.dev {
        binaries = binaries.file_named(binary, name);
    }
    let reserved = LANGUAGES
        .iter()
        .map(|language| language.key())
        .filter(|key| *key != Go.key())
        .map(str::to_owned)
        .chain(
            config
                .runtimes
                .remote
                .iter()
                .map(|remote| remote.name.clone()),
        );
    Ok(GoResolver::new(binaries)
        .with_catalog(catalog)
        .reserve(reserved))
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

const TEMPLATE: &[(&str, &str)] = &[
    ("go.mod", include_str!("../../templates/go/go.mod")),
    ("plugin.go", include_str!("../../templates/go/plugin.go")),
    (
        "plugin_test.go",
        include_str!("../../templates/go/plugin_test.go"),
    ),
    (
        "cmd/__name__/main.go",
        include_str!("../../templates/go/cmd/__name__/main.go"),
    ),
    (
        "rutis.dev.json",
        include_str!("../../templates/go/rutis.dev.json"),
    ),
    ("README.md", include_str!("../../templates/go/README.md")),
    (".gitignore", include_str!("../../templates/go/gitignore")),
    (
        ".github/workflows/release.yml",
        include_str!("../../templates/go/.github/workflows/release.yml"),
    ),
];
