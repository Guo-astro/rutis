//! Go: `runtimes.go`, a directory of compiled binaries, one runtime each
//! (`go-<binary>`), started when a row uses it (or with the host); rows
//! `go:<plugin>` and `go-<binary>:<plugin>`.
//!
//! Unlike the interpreted languages it has no single runtime: it overrides
//! [`Language::start`] with what the compiled languages share
//! ([`super::binary`]).

use std::path::{Path, PathBuf};

use rutis::host::BoxFuture;
use rutis_bridge::runtime::{Launcher, LocalRuntime};
use serde_json::{json, Value};

use super::binary::{self, DevBuild};
use super::{id_of, Language, Mount};
use crate::config::{BinaryStart, GoRuntime, HostConfig, Restart, Runtimes};
use crate::host::Error;

pub struct Go;

impl Language for Go {
    fn key(&self) -> &'static str {
        "go"
    }

    fn configured(&self, runtimes: &Runtimes) -> bool {
        runtimes.go.is_some()
    }

    fn settings(&self, runtimes: &Runtimes) -> Option<String> {
        runtimes.go.as_ref().map(|go| format!("{go:?}"))
    }

    /// Each Go runtime on its own: the host supervises them all
    /// ([`crate::host::Host::supervise`]).
    fn restart(&self, runtimes: &Runtimes) -> Restart {
        runtimes.go.as_ref().map_or(Restart::No, |go| go.restart)
    }

    /// The dev file's `restart`; the project's other settings stay.
    fn dev_file(&self, own: &mut Runtimes, dev: &Runtimes) {
        if let (Some(own), Some(dev)) = (&mut own.go, &dev.go) {
            own.restart = dev.restart;
        }
    }

    /// Its runtimes start as rows use them, from a resolver the loader was
    /// built with: a running host does not take it away.
    fn stops_when_removed(&self) -> bool {
        false
    }

    fn rebase(&self, runtimes: &mut Runtimes, at: &dyn Fn(&mut PathBuf)) {
        if let Some(go) = &mut runtimes.go {
            rebase(go, at);
        }
    }

    fn merge(&self, runtimes: &mut Runtimes, other: &mut Runtimes) {
        if runtimes.go.is_none() {
            runtimes.go = other.go.take();
        }
    }

    /// `go:<plugin>` and `go-<binary>:<plugin>` are the Go runtimes' rows.
    fn claims(&self, name: &str) -> Option<String> {
        binary::claims("go", "Go", name)
    }

    fn remote_names(&self) -> &'static [&'static str] {
        &["go"]
    }

    /// The binaries' resolver now; their runtimes once the loader runs,
    /// under Delve with `dev --debug`.
    fn start(&self, config: &HostConfig, mount: &mut Mount<'_>) -> Result<(), Error> {
        match (&config.runtimes.go, config.runtimes.debug) {
            (Some(go), false) => {
                binary::start::<rutis_loader::runtime::go::Go>(self, go, config, mount)
            }
            (Some(go), true) => binary::start::<Delve>(self, go, config, mount),
            (None, _) => Ok(()),
        }
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

    /// Build it (unless `built`, the first build the caller made without
    /// blocking), then one row per plugin of its binary, named with its
    /// runtime (`go-<name>:<plugin>`).
    fn dev(&self, dir: &Path, built: Option<&Path>) -> Result<(Vec<Value>, Runtimes), String> {
        let go = GoDev::of(dir).ok_or_else(|| format!("{}: no go.mod", dir.display()))?;
        let binary = match built {
            Some(binary) => binary.to_owned(),
            None => binary::building(&go.name, || go.build(1))?,
        };
        let (rows, settings) = dev_rows(&binary, &go.runtime, &go.dir, &go.package)?;
        Ok((
            rows,
            Runtimes {
                go: Some(settings),
                ..Runtimes::default()
            },
        ))
    }

    fn dev_build(&self, dir: &Path, debug: bool) -> Option<Box<dyn DevBuild>> {
        GoDev::of(dir).map(|go| Box::new(GoDev { debug, ..go }) as Box<dyn DevBuild>)
    }

    /// Delve runs each Go runtime ([`Delve`]); it must be installed.
    fn debugging(&self) -> Option<Result<&'static str, String>> {
        let installed = std::process::Command::new(DELVE)
            .arg("version")
            .output()
            .is_ok_and(|output| output.status.success());
        Some(match installed {
            true => Ok(
                "the Go runtime runs under Delve, listening on 127.0.0.1:2345 (`dlv connect 127.0.0.1:2345`); after a rebuild, connect again",
            ),
            false => Err(
                "rutis-host dev --debug runs the Go runtime under Delve, and dlv does not run: install it with \
                 `go install github.com/go-delve/delve/cmd/dlv@latest` (and put $(go env GOPATH)/bin on PATH)"
                    .into(),
            ),
        })
    }
}

/// Make the paths of a compiled language's settings absolute with `at`.
pub(super) fn rebase(settings: &mut GoRuntime, at: &dyn Fn(&mut PathBuf)) {
    if let Some(dir) = &mut settings.dir {
        at(dir);
    }
    for binary in &mut settings.binaries {
        at(binary);
    }
    at(&mut settings.project);
}

/// The rows of a compiled language's development build `binary`, one per
/// plugin of its manifest named with its runtime (`<runtime>:<plugin>`),
/// and the settings that run it from `project`; `what` names it when it
/// serves nothing.
pub(super) fn dev_rows(
    binary: &Path,
    runtime: &str,
    project: &Path,
    what: &str,
) -> Result<(Vec<Value>, GoRuntime), String> {
    let manifest = rutis_loader::internal::read_binary_manifest(binary)
        .map_err(|error| format!("{}: {error}", binary.display()))?;
    let rows: Vec<Value> = manifest
        .plugins
        .keys()
        .map(|plugin| json!({ "id": id_of(plugin), "name": format!("{runtime}:{plugin}"), "config": {} }))
        .collect();
    if rows.is_empty() {
        return Err(format!("{what} serves no plugins"));
    }
    let settings = GoRuntime {
        dir: None,
        binaries: Vec::new(),
        start: BinaryStart::OnDemand,
        idle: None,
        project: project.to_owned(),
        restart: Restart::No,
        dev: Some((binary.to_owned(), runtime.to_owned())),
    };
    Ok((rows, settings))
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
    /// Built for a debugger (`dev --debug`): without optimisations or
    /// inlining, so that every variable and line can be stopped at.
    pub debug: bool,
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
            debug: false,
        })
    }

    /// Build #`n`: its binary, or the compiler's errors.
    pub fn build(&self, n: u32) -> Result<PathBuf, String> {
        let (mut command, binary) = self.build_command(n)?;
        built(command.output(), binary)
    }

    /// The command that builds #`n`, and the binary it writes.
    fn build_command(&self, n: u32) -> Result<(std::process::Command, PathBuf), String> {
        let binary = self.binary(n);
        if let Some(builds) = binary.parent() {
            std::fs::create_dir_all(builds).map_err(|error| error.to_string())?;
        }
        let mut command = std::process::Command::new("go");
        command.arg("build");
        if self.debug {
            command.arg("-gcflags=all=-N -l");
        }
        command
            .arg("-o")
            .arg(&binary)
            .arg(&self.package)
            .current_dir(&self.dir);
        Ok((command, binary))
    }
}

impl DevBuild for GoDev {
    fn name(&self) -> &str {
        &self.name
    }

    fn runtime(&self) -> &str {
        &self.runtime
    }

    /// [`GoDev::build`], without blocking; dropped, it kills the go
    /// command.
    fn rebuild(&self, n: u32) -> BoxFuture<'_, Result<PathBuf, String>> {
        Box::pin(async move {
            let (command, binary) = self.build_command(n)?;
            built(binary::run(command).await, binary)
        })
    }

    fn binary(&self, n: u32) -> PathBuf {
        let file = match cfg!(windows) {
            true => format!("{}-{n}.exe", self.name),
            false => format!("{}-{n}", self.name),
        };
        self.dir.join(".rutis").join("go").join(file)
    }

    fn is_source(&self, path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension == "go")
            || path
                .file_name()
                .is_some_and(|name| name == "go.mod" || name == "go.sum" || name == "go.work")
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

/// The Delve program.
const DELVE: &str = "dlv";

/// Where Delve listens under `dev --debug`: its own default address.
pub const DELVE_ADDRESS: &str = "127.0.0.1:2345";

/// Go as `dev --debug` runs it: each runtime's binary started by Delve
/// (`dlv exec`), headless on [`DELVE_ADDRESS`], running at once and
/// taking clients as they come and go. Delve does not pass an inherited
/// socket on, so the binary dials its channel back (a socket path; on
/// Windows, as always, a loopback address).
struct Delve;

impl rutis_loader::runtime::binary::BinaryLanguage for Delve {
    const PREFIX: &'static str = rutis_loader::runtime::go::Go::PREFIX;
    const TITLE: &'static str = rutis_loader::runtime::go::Go::TITLE;
    const MARKER: &'static [u8] = rutis_loader::runtime::go::Go::MARKER;
    const PLUGIN_API: u64 = rutis_loader::runtime::go::Go::PLUGIN_API;

    fn runtime_name(path: &Path) -> String {
        rutis_bridge::runtime::go_runtime_name(path)
    }

    fn launch(binary: &Path, project: &Path) -> LocalRuntime {
        let launcher = Launcher::new(DELVE)
            .arg("exec")
            .arg("--headless")
            .arg(format!("--listen={DELVE_ADDRESS}"))
            .arg("--api-version=2")
            .arg("--accept-multiclient")
            .arg("--continue")
            .arg(binary)
            .arg("--")
            .cwd(project);
        LocalRuntime::launcher(Self::runtime_name(binary), launcher, project)
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
