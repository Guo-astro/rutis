//! Bun: `runtimes.bun`, the `@arcships/rutis-bun` package run by Bun, rows
//! `bun:<module>`.

use std::path::{Path, PathBuf};

use rutis_bridge::runtime::LocalRuntime;
use serde_json::{json, Value};

use super::{id_of, package_entry, rebase_program, Language};
use crate::config::{BunRuntime, Runtimes};
use crate::host::Error;

pub struct Bun;

impl Language for Bun {
    fn key(&self) -> &'static str {
        "bun"
    }

    fn configured(&self, runtimes: &Runtimes) -> bool {
        runtimes.bun.is_some()
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

    fn remote_names(&self) -> &'static [&'static str] {
        &["bun"]
    }

    fn runtime(&self, runtimes: &Runtimes) -> Option<Result<LocalRuntime, Error>> {
        runtimes.bun.as_ref().map(runtime)
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
    fn dev(&self, dir: &Path) -> Result<(Vec<Value>, Runtimes), String> {
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
    let depends = read("package.json").is_some_and(|manifest| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|field| !manifest[field]["@arcships/rutis-bun"].is_null())
    });
    configured || managed || depends
}

/// The Bun runtime of `bun.project`, with the `@arcships/rutis-bun` package
/// installed there (or given), run by a Bun that answers.
fn runtime(bun: &BunRuntime) -> Result<LocalRuntime, Error> {
    let candidates = [
        bun.runtime.clone(),
        Some(bun.project.join("node_modules/@arcships/rutis-bun")),
    ];
    let package = candidates
        .into_iter()
        .flatten()
        .find(|package| package.join("package.json").exists())
        .ok_or_else(|| {
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
    Ok(LocalRuntime::bun(package, &bun.project).interpreter(program))
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
