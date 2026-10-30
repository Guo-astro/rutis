//! The languages rutis-host runs on this machine: one [`Language`] each,
//! listed in [`LANGUAGES`].
//!
//! Each knows its part of `rutis.json` (`runtimes.<key>`: making its paths
//! relative to the file, merging a dev file), how its runtime is found and
//! started, how its rows are named, its `rutis-host new` template, and how
//! `rutis-host dev` recognizes and runs a project of it.
//!
//! Adding a language: a file here implementing [`Language`], its line in
//! [`LANGUAGES`], its
//! field in [`crate::config::Runtimes`] (the `rutis.json` format), and its
//! templates under `templates/<language>/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis::host::{BoxFuture, FiberView};
use rutis::Ctx;
use rutis_bridge::pipe::Processes;
use rutis_bridge::runtime::{RuntimeHandle, Stdio};
use rutis_loader::{Chain, Resolver, ServiceCatalog};
use serde_json::Value;

use crate::config::{HostConfig, Restart, Runtimes};
use crate::connect::SpawnPlugin;
use crate::host::Error;

mod binary;
pub mod bun;
pub mod go;
pub mod node;
pub mod python;
pub mod swift;

pub use binary::{BinaryHost, DevBuild};
pub use go::GoDev;
pub use swift::SwiftDev;

/// Every local language, in the order messages list them (`new --lang`)
/// and `rutis-host dev` tries them on a project (Node before Bun: Node's
/// recognizes a package.json project only when it is not Bun's).
///
/// The host starts them in the reverse order: their resolvers are tried in
/// that order, and Node's, which takes bare npm names and `file:` URLs,
/// comes last, after every prefixed one (`go:`, `py:`, `bun:`).
pub static LANGUAGES: &[&dyn Language] = &[
    &node::Node,
    &bun::Bun,
    &python::Python,
    &go::Go,
    &swift::Swift,
];

/// One local language.
pub trait Language: Sync {
    /// Its field under `runtimes` in `rutis.json`, and the name of the
    /// runtime it starts: the id of its `rutis-host/spawn` row (rows
    /// `<key>:<module>`; Node's take npm names).
    fn key(&self) -> &'static str;

    /// Whether `runtimes` configures it.
    fn configured(&self, runtimes: &Runtimes) -> bool;

    /// Its settings in `runtimes` as text, `None` when it is not
    /// configured: a running host compares them with the configuration
    /// read again ([`crate::host::Host::reload`]), and refuses a change.
    fn settings(&self, runtimes: &Runtimes) -> Option<String>;

    /// Whether a running host stops its runtime when the configuration
    /// read again no longer has it, once its rows are gone; otherwise
    /// removing it is refused like a change.
    fn stops_when_removed(&self) -> bool {
        true
    }

    /// What the host does when its runtime's process ends on its own
    /// (`runtimes.<key>.restart`); a language without the setting leaves it
    /// down.
    fn restart(&self, _runtimes: &Runtimes) -> Restart {
        Restart::No
    }

    /// Whether a row named `name` (as written in the file) runs in its
    /// runtime with that runtime's defaults: such rows enable the runtime
    /// when `runtimes` does not declare it (#666). Never for a language
    /// without defaults (Go and Swift need their binaries named).
    fn runs_by_default(&self, _name: &str) -> bool {
        false
    }

    /// Declare its runtime with every setting at its default.
    fn enable_default(&self, _runtimes: &mut Runtimes) {}

    /// Make its relative paths absolute with `at`.
    fn rebase(&self, runtimes: &mut Runtimes, at: &dyn Fn(&mut PathBuf));

    /// Take `other`'s settings when `runtimes` has none.
    fn merge(&self, runtimes: &mut Runtimes, other: &mut Runtimes);

    /// Its runtime as `runtimes` configures it, found (its package, its
    /// interpreter) and ready to mount: the `rutis-host/spawn` row
    /// [`Language::key`]; `None` when it is not configured. One runtime per
    /// language: a language with several overrides [`Language::start`]
    /// instead.
    fn runtime(&self, _runtimes: &Runtimes) -> Option<Result<SpawnPlugin, Error>> {
        None
    }

    /// Mount what `config` configures of it: by default its
    /// [`Language::runtime`], its rows `<key>:<module>` on its session.
    fn start(&self, config: &HostConfig, mount: &mut Mount<'_>) -> Result<(), Error> {
        if let Some(runtime) = self.runtime(&config.runtimes) {
            mount.local(self.key(), runtime?);
        }
        Ok(())
    }

    /// The `--lang` names `rutis-host new` takes for it, the first the one
    /// messages show.
    fn new_names(&self) -> &'static [&'static str];

    /// The files of a new project: (path, text), with `__module__`,
    /// `__Type__` and `__name__` in paths and `{{name}}`, `{{id}}`,
    /// `{{module}}`, `{{Type}}` (`NetProbe`), `{{package}}`, `{{version}}`
    /// and `{{next}}` in texts filled in.
    fn template(&self) -> &'static [(&'static str, &'static str)];

    /// What to run in a new project, as `rutis-host new` says.
    fn next_steps(&self) -> &'static str;

    /// The file that marks a project of it, as the error of `rutis-host
    /// dev` in a directory without one names it.
    fn project_file(&self) -> &'static str;

    /// Whether `dir` is a project of it.
    fn is_project(&self, dir: &Path) -> bool;

    /// Why its runtime cannot run on this machine, or `Ok(())`: Swift's
    /// binaries run on macOS only.
    fn available(&self) -> Result<(), String> {
        Ok(())
    }

    /// The rows and runtime that run the project in `dir` (one of it);
    /// `built` is its build #1 when the caller made it (a compiled
    /// language's project, built without blocking).
    fn dev(&self, dir: &Path, built: Option<&Path>) -> Result<(Vec<Value>, Runtimes), String>;

    /// What `rutis.dev.json` (`dev`) says of the runtime the project
    /// configures in `own`, when it should win over it; by default the
    /// project's own settings stay. A language with a `restart` setting
    /// takes the dev file's: `dev` restarts as `run` does.
    fn dev_file(&self, _own: &mut Runtimes, _dev: &Runtimes) {}

    /// A compiled language's project in `dir`, which `rutis-host dev`
    /// rebuilds as it changes and restarts its runtime on, instead of
    /// reloading its rows; `None` for the others. With `debug`
    /// (`dev --debug`), built for a debugger: without optimisation.
    fn dev_build(&self, _dir: &Path, _debug: bool) -> Option<Box<dyn DevBuild>> {
        None
    }

    /// How its runtime is debugged under `rutis-host dev --debug`
    /// ([`Runtimes::debug`]), as `dev` says it; `None` when it has no
    /// debugger there, an error when the debugger is not installed.
    fn debugging(&self) -> Option<Result<&'static str, String>> {
        None
    }
}

/// The language whose `--lang` name `rutis-host new` takes is `name`.
pub fn by_new_name(name: &str) -> Option<&'static dyn Language> {
    LANGUAGES
        .iter()
        .copied()
        .find(|language| language.new_names().contains(&name))
}

/// The language of the project in `dir`.
pub fn of_project(dir: &Path) -> Option<&'static dyn Language> {
    LANGUAGES
        .iter()
        .copied()
        .find(|language| language.is_project(dir))
}

/// The `--lang` name of each language, as messages show them.
pub fn new_names() -> Vec<&'static str> {
    LANGUAGES
        .iter()
        .map(|language| language.new_names()[0])
        .collect()
}

/// The files that mark a project, each once, as messages show them.
pub fn project_files() -> Vec<&'static str> {
    let mut files: Vec<&str> = Vec::new();
    for language in LANGUAGES {
        if !files.contains(&language.project_file()) {
            files.push(language.project_file());
        }
    }
    files
}

/// `a, b or c`.
pub fn either(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// What the languages mount as the host starts: what resolves their rows,
/// their runtimes, and what starts once the loader runs.
pub struct Mount<'a> {
    pub root: &'a Ctx,
    pub catalog: &'a ServiceCatalog,
    /// The loader's resolvers, tried in order.
    pub chain: Chain,
    /// The local runtimes by name, to wait for until they run.
    pub runtimes: Vec<(String, RuntimeHandle)>,
    /// The local runtimes' plugins by name, to stop one.
    pub fibers: Vec<(String, FiberView)>,
    /// The processes each local runtime started, by name.
    pub processes: Vec<(String, Processes)>,
    /// What mounts once the loader runs (the compiled languages' runtimes,
    /// which start as rows use them), and the [`BinaryHost`] it gives the
    /// host.
    pub later: Vec<Later>,
    /// Whether the plugins' output lines start with their row's name
    /// ([`Runtimes::prefix_output`](crate::config::Runtimes::prefix_output)).
    pub prefix_output: bool,
    /// `dev --debug`: the runtimes run under their debuggers.
    pub debug: bool,
}

/// What tells a Node, Bun or Python runtime to start each line its plugins
/// write with the row's name (#678).
pub const PREFIX_OUTPUT: &str = "RUTIS_PREFIX_OUTPUT";

/// What tells a Node, Bun or Python runtime that it runs under a debugger,
/// which stops it for as long as the person likes: it says nothing of a
/// method holding it (#678).
pub const DEBUGGING: &str = "RUTIS_DEBUG";

/// A language's part of the start that waits for the loader.
pub type Later =
    Box<dyn FnOnce(Ctx) -> BoxFuture<'static, Result<Option<BinaryHost>, Error>> + Send>;

impl Mount<'_> {
    /// Try `resolver` after the resolvers added before it.
    pub fn resolve(&mut self, resolver: Arc<dyn Resolver>) {
        self.chain = std::mem::take(&mut self.chain).with_shared(resolver);
    }

    /// Mount the local runtime `runtime` as the row `name`, its output
    /// passed on a whole line at a time: a line a plugin writes in pieces
    /// (a Python `print` writes its newline separately) never has one of
    /// this host's lines, or another runtime's, inside it (#332). With
    /// [`Mount::prefix_output`], the runtime starts each line with the row
    /// that wrote it.
    pub fn local(&mut self, name: &str, runtime: SpawnPlugin) {
        use crate::output::{lines, OutputStream};
        let mut runtime = runtime.stdio(
            Stdio::Null,
            lines(OutputStream::Stdout, None),
            lines(OutputStream::Stderr, None),
        );
        if self.prefix_output {
            runtime = runtime.env(PREFIX_OUTPUT, "1");
        }
        self.local_as_is(name, runtime);
    }

    /// [`Mount::local`] for a runtime that reads this terminal (a debugger
    /// prompting there, `dev --debug`): its standard input is this
    /// process's, and its output is not held back until a line ends, or a
    /// prompt would not show. Nor is it prefixed: the prompt would not be
    /// the debugger's.
    pub fn local_terminal(&mut self, name: &str, runtime: SpawnPlugin) {
        let runtime = runtime.stdio(Stdio::Inherit, Stdio::Inherit, Stdio::Inherit);
        self.local_as_is(name, runtime);
    }

    fn local_as_is(&mut self, name: &str, mut runtime: SpawnPlugin) {
        if self.debug {
            runtime = runtime.env(DEBUGGING, "1");
        }
        let handle = runtime.handle();
        self.processes.push((name.to_owned(), runtime.processes()));
        self.fibers
            .push((name.to_owned(), self.root.plugin(runtime)));
        self.runtimes.push((name.to_owned(), handle));
    }
}

/// Make `path` absolute with `at`, unless it is a bare program name, which
/// is looked up on `PATH`, not under the base.
pub(crate) fn rebase_program(path: &mut Option<PathBuf>, at: &dyn Fn(&mut PathBuf)) {
    if let Some(program) = path {
        if program.components().count() > 1 {
            at(program);
        }
    }
}

/// The npm package `path` the setting `field` names (a runtime's
/// `runtime`) is there: an error naming the path when it does not exist,
/// or is no package (no package.json in it).
pub(crate) fn configured_package(field: &str, path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!(
            "{field} names {}, which does not exist",
            path.display()
        ));
    }
    if !path.join("package.json").is_file() {
        return Err(format!(
            "{field} names {}, which is no npm package (it has no package.json)",
            path.display()
        ));
    }
    Ok(())
}

/// Whether `dir` is a package.json project (Node or Bun): it has a
/// package.json, and is not a Python project (pyproject.toml) whose
/// package.json has no plugin to run, one only there for the dev file's
/// rows, say (#938): the file its plugin would be is not there.
pub(crate) fn is_package_project(dir: &Path) -> bool {
    if !dir.join("package.json").exists() {
        return false;
    }
    !dir.join("pyproject.toml").exists()
        || package_entry(dir).is_ok_and(|(_, entry)| entry.is_file())
}

/// The package's name and the source file of its plugin while developing:
/// src/index.ts or .js, else what the package exports (Node and Bun).
pub(crate) fn package_entry(dir: &Path) -> Result<(String, PathBuf), String> {
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

/// A row id from a package or module name.
pub(crate) fn id_of(name: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Each name a language is known by belongs to it alone.
    #[test]
    fn names_are_unique() {
        for names in [
            LANGUAGES.iter().map(|l| vec![l.key()]).collect::<Vec<_>>(),
            LANGUAGES.iter().map(|l| l.new_names().to_vec()).collect(),
        ] {
            let mut all: Vec<&str> = names.concat();
            let count = all.len();
            all.sort();
            all.dedup();
            assert_eq!(all.len(), count, "{all:?}");
        }
    }

    #[test]
    fn messages_list_the_languages() {
        let names: Vec<String> = new_names().iter().map(|n| n.to_string()).collect();
        assert_eq!(either(&names), "node, bun, python, go or swift");
        let files: Vec<String> = project_files().iter().map(|f| f.to_string()).collect();
        assert_eq!(
            either(&files),
            "package.json, pyproject.toml, go.mod or Package.swift"
        );
    }
}
