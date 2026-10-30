//! Bun: `runtimes.bun`, the `@arcships/rutis-bun` package run by Bun, rows
//! `bun:<module>`.

use std::path::{Path, PathBuf};

use rutis_bridge::runtime::Launcher;
use serde_json::{json, Value};

use super::{id_of, package_entry, rebase_program, Language};
use crate::config::{BunRuntime, Restart, Runtimes};
use crate::connect::SpawnPlugin;
use crate::host::Error;

pub struct Bun;

impl Language for Bun {
    fn key(&self) -> &'static str {
        "bun"
    }

    fn configured(&self, runtimes: &Runtimes) -> bool {
        runtimes.bun.is_some()
    }

    fn settings(&self, runtimes: &Runtimes) -> Option<String> {
        runtimes.bun.as_ref().map(|bun| format!("{bun:?}"))
    }

    fn restart(&self, runtimes: &Runtimes) -> Restart {
        runtimes.bun.as_ref().map_or(Restart::No, |bun| bun.restart)
    }

    fn runs_by_default(&self, name: &str) -> bool {
        name.starts_with("bun:")
    }

    fn enable_default(&self, runtimes: &mut Runtimes) {
        runtimes.bun = Some(BunRuntime {
            project: PathBuf::from("."),
            runtime: None,
            program: None,
            restart: Restart::No,
        });
    }

    fn rebase(&self, runtimes: &mut Runtimes, at: &dyn Fn(&mut PathBuf)) {
        if let Some(bun) = &mut runtimes.bun {
            at(&mut bun.project);
            if let Some(runtime) = &mut bun.runtime {
                at(runtime);
            }
            rebase_program(&mut bun.program, at);
        }
    }

    fn merge(&self, runtimes: &mut Runtimes, other: &mut Runtimes) {
        if runtimes.bun.is_none() {
            runtimes.bun = other.bun.take();
        }
    }

    fn runtime(&self, runtimes: &Runtimes) -> Option<Result<SpawnPlugin, Error>> {
        runtimes
            .bun
            .as_ref()
            .map(|bun| runtime(bun, runtimes.debug))
    }

    fn debugging(&self) -> Option<Result<&'static str, String>> {
        Some(Ok(
            "the Bun runtime's inspector listens on ws://127.0.0.1:6499/rutis (open https://debug.bun.sh/#127.0.0.1:6499/rutis)",
        ))
    }

    fn new_names(&self) -> &'static [&'static str] {
        &["bun"]
    }

    fn template(&self) -> &'static [(&'static str, &'static str)] {
        TEMPLATE
    }

    fn next_steps(&self) -> &'static str {
        "bun install && bun test && bunx --bun rutis-host dev"
    }

    fn project_file(&self) -> &'static str {
        "package.json"
    }

    fn is_project(&self, dir: &Path) -> bool {
        dir.join("package.json").exists() && runs_in_bun(dir)
    }

    /// The package's source as the row, `bun:<file>`.
    fn dev(&self, dir: &Path, _built: Option<&Path>) -> Result<(Vec<Value>, Runtimes), String> {
        let (name, entry) = package_entry(dir)?;
        let row =
            json!({ "id": id_of(&name), "name": format!("bun:{}", entry.display()), "config": {} });
        let runtimes = Runtimes {
            bun: Some(BunRuntime {
                restart: Restart::No,
                project: dir.to_owned(),
                runtime: None,
                program: None,
            }),
            ..Runtimes::default()
        };
        Ok((vec![row], runtimes))
    }

    /// The dev file may name the Bun to use.
    fn dev_file(&self, own: &mut Runtimes, dev: &Runtimes) {
        if let (Some(_), Some(bun)) = (&own.bun, &dev.bun) {
            own.bun = Some(bun.clone());
        }
    }
}

/// Whether the package.json project in `dir` runs in Bun: when
/// `rutis.dev.json` configures `runtimes.bun`, when Bun manages it
/// (`bun.lock`), or when it depends on `@arcships/rutis-bun`.
pub(super) fn runs_in_bun(dir: &Path) -> bool {
    let read = |file: &str| -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(dir.join(file)).ok()?).ok()
    };
    let configured = read("rutis.dev.json").is_some_and(|dev| !dev["runtimes"]["bun"].is_null());
    let managed = ["bun.lock", "bun.lockb"]
        .iter()
        .any(|lock| dir.join(lock).exists());
    let depends = read("package.json").is_some_and(|manifest| depends_on_runtime(&manifest));
    configured || managed || depends
}

/// Whether the package `manifest` (a package.json) depends on
/// `@arcships/rutis-bun`, in its dependencies or devDependencies (the
/// template's plugin has it there): it runs in Bun.
pub(crate) fn depends_on_runtime(manifest: &Value) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .any(|field| !manifest[field]["@arcships/rutis-bun"].is_null())
}

/// The Bun runtime package (`@arcships/rutis-bun`) for the project
/// `project`: `configured` (`runtimes.bun.runtime`), else the project's.
/// A `configured` package that is not there is an error, not passed over.
pub(crate) fn runtime_package(
    configured: Option<&Path>,
    project: &Path,
) -> Result<Option<PathBuf>, String> {
    if let Some(configured) = configured {
        super::configured_package("runtimes.bun.runtime", configured)?;
        return Ok(Some(configured.to_owned()));
    }
    let package = project.join("node_modules/@arcships/rutis-bun");
    Ok(Some(package).filter(|package| package.join("package.json").exists()))
}

/// Where the Bun runtime's inspector listens under `dev --debug`: Bun's
/// own default port, with a fixed path, so its address is the same on
/// every start.
pub const INSPECT: &str = "127.0.0.1:6499/rutis";

/// The Bun runtime of `bun.project`, with the `@arcships/rutis-bun` package
/// installed there (or given), run by a Bun that answers; with `debug`, its
/// inspector on [`INSPECT`].
fn runtime(bun: &BunRuntime, debug: bool) -> Result<SpawnPlugin, Error> {
    let package = runtime_package(bun.runtime.as_deref(), &bun.project)?.ok_or_else(|| {
        format!(
            "the Bun runtime is not installed in {}: run `bun add -d @arcships/rutis-bun` there",
            bun.project.display()
        )
    })?;
    let program = bun.program.clone().unwrap_or_else(|| PathBuf::from("bun"));
    let answers = std::process::Command::new(&program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !answers {
        return Err(format!(
            "the Bun runtime needs Bun: {} does not run (install it from https://bun.com, or set `program`)",
            program.display()
        ));
    }
    let mut launcher = launcher(&program, &package, &bun.project);
    if debug {
        launcher
            .args
            .insert(0, format!("--inspect={INSPECT}").into());
    }
    Ok(SpawnPlugin::new("bun", launcher, &bun.project))
}

const TEMPLATE: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../../templates/bun/package.json"),
    ),
    (
        "tsconfig.json",
        include_str!("../../templates/bun/tsconfig.json"),
    ),
    (
        "src/index.ts",
        include_str!("../../templates/bun/src/index.ts"),
    ),
    (
        "test/index.test.ts",
        include_str!("../../templates/bun/test/index.test.ts"),
    ),
    (
        "rutis.dev.json",
        include_str!("../../templates/bun/rutis.dev.json"),
    ),
    ("README.md", include_str!("../../templates/bun/README.md")),
    (".gitignore", include_str!("../../templates/bun/gitignore")),
    (
        ".github/workflows/publish.yml",
        include_str!("../../templates/bun/.github/workflows/publish.yml"),
    ),
];

/// The Bun runtime of the npm package `package` (`bun/rutis-bun`, or an
/// installed `@arcships/rutis-bun`), run by the Bun executable `program`,
/// in `project`, on an inherited socket (on Windows, a loopback address).
/// Auto-install and `.env` files are always off: plugins resolve only what
/// the project has installed, and the environment is only what this
/// process gives.
pub fn launcher(program: &Path, package: &Path, project: &Path) -> Launcher {
    Launcher::new(program)
        .arg("--no-install")
        .arg("--no-env-file")
        .arg(package.join("src/main.ts"))
        .cwd(project)
        .inherit_fd()
}
