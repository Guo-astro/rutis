//! Installing plugins from where they come from: npm, PyPI, Go modules
//! and local paths, into the places the runtimes use
//! (docs/design-plugin-install-2026-10-27.md §五). `rutis-host add`,
//! `remove` and `upgrade` use it ([`crate::install`]).
//!
//! The package managers run as commands (npm, bun, uv or pip, go), their
//! output shown as it is; what a runtime lacks is installed with the
//! plugin. The records ([`Installed`]) are the caller's to keep, and so is
//! checking that what was installed is a plugin (`check`'s rows). What a
//! package manager installed is not undone when a later step fails: the
//! error says how to uninstall it.

use std::path::{Path, PathBuf};

use rutis_bridge::runtime::RowSchema;
use serde::Deserialize;

use crate::config::{venv_python, DEFAULT_PYTHON};
use crate::languages::python::venv_of;

mod reference;
mod tools;

use reference::{Reference, Source};
pub(crate) use tools::quoted_path;
use tools::{io_error, Run, Tools};

/// The version of the runtime packages installed where they are missing
/// (`@arcships/rutis-runtime`, `@arcships/rutis-bun`, `rutis`): this
/// crate's, which is the host's.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where each language's plugins are installed.
#[derive(Clone, Debug)]
pub struct Places {
    /// Where relative local paths in references are from (the directory of
    /// the configuration that names them).
    pub base: PathBuf,
    /// The Node project: its `package.json` (made with `npm init -y` when
    /// missing) and `node_modules`.
    pub node: PathBuf,
    /// The Node runtime package the configuration names
    /// (`runtimes.node.runtime`): when there, none is installed.
    pub node_runtime: Option<PathBuf>,
    /// The Bun project.
    pub bun: PathBuf,
    /// The Bun runtime package the configuration names, as for Node.
    pub bun_runtime: Option<PathBuf>,
    /// The Python environment, created (with the `rutis` package) when
    /// missing; or an interpreter, used as it is.
    pub python: PathBuf,
    /// The Python runtime's project (`runtimes.py.project`): a uv project
    /// when it has a `pyproject.toml` and a `uv.lock` and the environment
    /// is its `.venv`, whose packages `uv sync` keeps only when they are
    /// recorded in it.
    pub python_project: PathBuf,
    /// The directory of Go binaries, created when missing; or why there is
    /// none (`runtimes.go` declared without a `dir`).
    pub go: Result<PathBuf, String>,
    /// The directory of Swift binaries, as for Go.
    pub swift: Result<PathBuf, String>,
}

/// One installed plugin: its record (`installed.<row name>` in
/// `rutis.json`: `from` only, [`Installed::row_name`] its key), its name
/// and its language.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    /// The plugin's name: its row name without the language prefix. Not
    /// recorded: the key says it ([`Installed::of_row`]).
    #[serde(skip)]
    pub name: String,
    /// The reference it was installed from, without a version
    /// (`npm:@ana/wordcount`, `pypi:bo-llm`, a Go module path, a local
    /// path). The plugins of one package have the same.
    pub from: String,
    /// The runtime it runs in: `node`, `bun`, `py`, `go` or `swift`. Not
    /// recorded: the key's prefix says it.
    #[serde(skip)]
    pub language: String,
    /// The version just installed, when known; not recorded (the package
    /// manager's own files keep it).
    #[serde(skip)]
    pub version: Option<String>,
}

impl Installed {
    /// The row name a loader with rutis-host's runtimes resolves: the
    /// package name (Node), `bun:<package>`, `py:<entry point>`,
    /// `go:<plugin>`, `swift:<plugin>`.
    pub fn row_name(&self) -> String {
        match self.language.as_str() {
            "node" => self.name.clone(),
            language => format!("{language}:{}", self.name),
        }
    }

    /// The plugin a record kept under the row name `row` is: its language
    /// from the prefix (`bun:`, `py:`, `go:`, `swift:`; none for Node).
    /// `None` when `row` is not a row name `add` writes.
    pub fn of_row(row: &str, from: impl Into<String>) -> Option<Self> {
        let (language, name) = match row.split_once(':') {
            Some((prefix, name)) => (
                Lang::of_key(prefix).filter(|lang| *lang != Lang::Node)?,
                name,
            ),
            None => (Lang::Node, row),
        };
        if name.is_empty() || language == Lang::Node && is_local_name(name) {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            from: from.into(),
            language: language.key().to_owned(),
            version: None,
        })
    }
}

/// Whether a Node row name is a path, which `add` never writes.
fn is_local_name(name: &str) -> bool {
    reference::is_local(name)
}

/// What [`Installer::add`] installed: its plugins, and the command that
/// uninstalls the package (for messages when they turn out not to be
/// plugins).
#[derive(Clone, Debug)]
pub struct Added {
    pub plugins: Vec<Installed>,
    pub uninstall: String,
}

/// What [`Installer::upgrade`] did: the package's version before, and its
/// plugins now (with the version installed).
#[derive(Clone, Debug)]
pub struct Upgraded {
    pub before: Option<String>,
    pub plugins: Vec<Installed>,
}

/// Why installing failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A package manager ran and failed (its own output was shown).
    Failed(String),
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Failed(text) | Error::Other(text) => f.write_str(text),
        }
    }
}

/// Installs plugins into [`Places`].
#[derive(Clone, Debug)]
pub struct Installer {
    core: Core,
    installed: Vec<Installed>,
}

/// What the blocking steps take: the places and how commands run.
#[derive(Clone, Debug)]
struct Core {
    places: Places,
    tools: Tools,
}

/// A language plugins install for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lang {
    Node,
    Bun,
    Python,
    Go,
    Swift,
}

impl Lang {
    fn key(self) -> &'static str {
        match self {
            Lang::Node => "node",
            Lang::Bun => "bun",
            Lang::Python => "py",
            Lang::Go => "go",
            Lang::Swift => "swift",
        }
    }

    fn of_key(key: &str) -> Option<Self> {
        [Lang::Node, Lang::Bun, Lang::Python, Lang::Go, Lang::Swift]
            .into_iter()
            .find(|lang| lang.key() == key)
    }

    fn title(self) -> &'static str {
        match self {
            Lang::Node => "Node",
            Lang::Bun => "Bun",
            Lang::Python => "Python",
            Lang::Go => "Go",
            Lang::Swift => "Swift",
        }
    }
}

/// A package installed: its plugins, and the command that uninstalls it
/// (for messages).
struct Package {
    plugins: Vec<Installed>,
    uninstall: String,
}

impl Installer {
    pub fn new(places: Places) -> Self {
        Self {
            core: Core {
                places,
                tools: Tools::default(),
            },
            installed: Vec::new(),
        }
    }

    /// The records the caller keeps: `add` refuses a row name another
    /// package's record has.
    pub fn with_installed(mut self, records: impl IntoIterator<Item = Installed>) -> Self {
        self.installed = records.into_iter().collect();
        self
    }

    /// Install the package `reference` names, with what its runtime lacks:
    /// one record per plugin in it.
    pub async fn add(&self, reference: &str) -> Result<Added, Error> {
        let reference = Reference::parse(reference, &self.core.places.base)?;
        // An npm package's plugin is the package, in Node or Bun: refused
        // before npm runs.
        if let Source::Npm { package, .. } = &reference.source {
            let rows = [package.clone(), format!("bun:{package}")];
            self.free(&rows, &reference.from, None)?;
        }
        let (lang, known) = {
            let (core, reference) = (self.core.clone(), reference.clone());
            blocking(move || core.language(&reference)).await?
        };
        if let Some(name) = &known {
            self.free(&[row_of(lang, name)], &reference.from, None)?;
        }
        let package = {
            let (core, reference) = (self.core.clone(), reference.clone());
            blocking(move || core.install(&reference, lang, false)).await?
        };
        self.free_after(&package)?;
        Ok(Added {
            plugins: package.plugins,
            uninstall: package.uninstall,
        })
    }

    /// Install the newest version of the package `plugin` is in (or
    /// `version`), from where it was installed.
    pub async fn upgrade(
        &self,
        plugin: &Installed,
        version: Option<&str>,
    ) -> Result<Upgraded, Error> {
        let lang = self.core.lang_of(plugin)?;
        let mut reference = Reference::parse(&plugin.from, &self.core.places.base)?;
        if let Some(version) = version {
            if let Source::Local(_) = reference.source {
                return Err(Error::Other(format!(
                    "{} is a local path, which has no versions: upgrade it without @{version}",
                    plugin.from
                )));
            }
            reference = Reference::parse(
                &format!("{}@{version}", plugin.from),
                &self.core.places.base,
            )?;
        }
        let (core, plugin) = (self.core.clone(), plugin.clone());
        let (before, package) = blocking(move || {
            let before = core.version_of(lang, &plugin);
            let package = core.install(&reference, lang, true)?;
            Ok((before, package))
        })
        .await?;
        self.free_after(&package)?;
        Ok(Upgraded {
            before,
            plugins: package.plugins,
        })
    }

    /// Uninstall the package `plugins` are in (all of its plugins: a
    /// binary serving any of them is deleted): what was uninstalled, as its
    /// package manager names it, or `None` when it was no longer there.
    pub async fn remove(&self, plugins: &[Installed]) -> Result<Option<String>, Error> {
        let Some(first) = plugins.first() else {
            return Ok(None);
        };
        let lang = self.core.lang_of(first)?;
        let (core, plugins) = (self.core.clone(), plugins.to_vec());
        blocking(move || core.remove(lang, &plugins)).await
    }

    /// Refuse a plugin of `package`, just installed, whose row name another
    /// package's record has; the package stays installed.
    fn free_after(&self, package: &Package) -> Result<(), Error> {
        for plugin in &package.plugins {
            self.free(&[plugin.row_name()], &plugin.from, Some(&package.uninstall))?;
        }
        Ok(())
    }

    /// Refuse each of `rows` another package's record has: two records
    /// cannot share a row name.
    fn free(&self, rows: &[String], from: &str, uninstall: Option<&str>) -> Result<(), Error> {
        match self
            .installed
            .iter()
            .find(|record| rows.contains(&record.row_name()) && record.from != from)
        {
            Some(record) => Err(Error::Other(format!(
                "{} is already installed from {}: remove it first{}",
                record.row_name(),
                record.from,
                stays(uninstall)
            ))),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    fn with_path(mut self, path: impl Into<std::ffi::OsString>) -> Self {
        self.core.tools.path = Some(path.into());
        self
    }
}

/// The row name of `lang`'s plugin `name`.
fn row_of(lang: Lang, name: &str) -> String {
    match lang {
        Lang::Node => name.to_owned(),
        lang => format!("{}:{name}", lang.key()),
    }
}

/// `; it stays installed: <command>`, or nothing.
fn stays(uninstall: Option<&str>) -> String {
    uninstall
        .map(|command| format!("; it stays installed: {command}"))
        .unwrap_or_default()
}

/// Run `f` off the async threads: it runs commands and waits for them.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|error| Error::Other(error.to_string()))?
}

impl Core {
    fn lang_of(&self, plugin: &Installed) -> Result<Lang, Error> {
        Lang::of_key(&plugin.language).ok_or_else(|| {
            Error::Other(format!(
                "{}: language {} is not one plugins install for (node, bun, py, go or swift)",
                plugin.name, plugin.language
            ))
        })
    }

    // ── 1. The language ─────────────────────────────────────────

    /// The language of what `reference` names, and the plugin's name when
    /// it is known before installing (an npm package's is its name).
    fn language(&self, reference: &Reference) -> Result<(Lang, Option<String>), Error> {
        let (lang, name) = match &reference.source {
            Source::Npm { package, version } => {
                let spec = npm_spec(package, version.as_deref());
                let run = Run::new("npm").args(["view", spec.as_str(), "--json"]);
                let text = self.tools.output(&run)?;
                let manifest: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|error| Error::Other(format!("{}: {error}", run.shown())))?;
                // Several versions match a range: the last is the newest.
                let manifest = match manifest {
                    serde_json::Value::Array(mut all) => all.pop().unwrap_or_default(),
                    one => one,
                };
                (js_lang(&manifest), Some(package.clone()))
            }
            Source::Pypi { .. } => (Lang::Python, None),
            Source::GoModule { .. } => (Lang::Go, None),
            Source::Local(path) => self.local_language(&reference.from, path)?,
        };
        Ok((lang, name))
    }

    /// The language of the local `path` (`text` as written).
    fn local_language(&self, text: &str, path: &Path) -> Result<(Lang, Option<String>), Error> {
        if path.is_dir() {
            if path.join("package.json").exists() {
                let manifest = read_json(&path.join("package.json"))?;
                let name = manifest["name"].as_str().map(str::to_owned);
                return Ok((js_lang(&manifest), name));
            }
            if path.join("pyproject.toml").exists() {
                return Ok((Lang::Python, None));
            }
            if path.join("go.mod").exists() {
                return Ok((Lang::Go, None));
            }
            return Err(Error::Other(format!(
                "{text} has no package.json, pyproject.toml or go.mod"
            )));
        }
        if !path.is_file() {
            return Err(Error::Other(format!("{text} does not exist")));
        }
        let file = path.to_string_lossy();
        if file.ends_with(".tgz") {
            let run = Run::new("tar")
                .arg("-xzOf")
                .arg(path)
                .arg("package/package.json");
            let text = self.tools.output(&run)?;
            let manifest: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
                Error::Other(format!("{}: package/package.json: {error}", path.display()))
            })?;
            let name = manifest["name"].as_str().map(str::to_owned);
            return Ok((js_lang(&manifest), name));
        }
        if file.ends_with(".whl") {
            return Ok((Lang::Python, None));
        }
        // A binary: as the runtimes find theirs in a directory.
        let dir = path.parent().unwrap_or(Path::new("."));
        for lang in [Lang::Go, Lang::Swift] {
            if binaries_in(lang, dir)
                .iter()
                .any(|(found, _)| found.file_name() == path.file_name())
            {
                return Ok((lang, None));
            }
        }
        Err(Error::Other(format!(
            "{text} is not a plugin package: name a directory, a .tgz, a .whl, or a binary built with the rutis Go or Swift SDK"
        )))
    }

    // ── 2–3. Install, and find the plugins ─────────────────────

    /// Install what `reference` names, with what the runtime lacks;
    /// `upgrade`: to its newest version (or the one named).
    fn install(&self, reference: &Reference, lang: Lang, upgrade: bool) -> Result<Package, Error> {
        let places = &self.places;
        match lang {
            Lang::Node => self.install_js(reference, lang, &places.node, upgrade),
            Lang::Bun => self.install_js(reference, lang, &places.bun, upgrade),
            Lang::Python => self.install_python(reference, &places.python, upgrade),
            Lang::Go | Lang::Swift => self.install_binary(reference, lang, &self.binary_dir(lang)?),
        }
    }

    /// The directory of `lang`'s binaries (Go, Swift).
    fn binary_dir(&self, lang: Lang) -> Result<PathBuf, Error> {
        match lang {
            Lang::Go => &self.places.go,
            _ => &self.places.swift,
        }
        .clone()
        .map_err(Error::Other)
    }

    /// Node and Bun: the package into the project `project`.
    fn install_js(
        &self,
        reference: &Reference,
        lang: Lang,
        project: &Path,
        upgrade: bool,
    ) -> Result<Package, Error> {
        std::fs::create_dir_all(project).map_err(|error| {
            Error::Other(format!("{}: {}", project.display(), io_error(&error)))
        })?;
        let spec = match &reference.source {
            Source::Npm { package, version } => {
                let version = version.as_deref().or(upgrade.then_some("latest"));
                npm_spec(package, version)
            }
            Source::Local(path) => path.display().to_string(),
            _ => unreachable!("only npm packages and local paths are Node or Bun"),
        };
        let places = &self.places;
        let (runtime_package, runtime_there) = match lang {
            Lang::Node => (
                "@arcships/rutis-runtime",
                crate::languages::node::runtime_package(places.node_runtime.as_deref(), project)
                    .is_some(),
            ),
            _ => (
                "@arcships/rutis-bun",
                [
                    places.bun_runtime.clone(),
                    Some(project.join("node_modules/@arcships/rutis-bun")),
                ]
                .into_iter()
                .flatten()
                .any(|package| package.join("package.json").exists()),
            ),
        };
        let runtime = format!("{runtime_package}@{VERSION}");
        match lang {
            Lang::Node => {
                if !project.join("package.json").exists() {
                    self.tools
                        .run(&Run::new("npm").args(["init", "-y"]).cwd(project))?;
                }
                let mut run = Run::new("npm").args(["install", spec.as_str()]);
                if !runtime_there {
                    run = run.arg(&runtime);
                }
                self.tools.run(&run.cwd(project))?;
            }
            _ => {
                if !runtime_there {
                    self.tools.run(
                        &Run::new("bun")
                            .args(["add", "-d", runtime.as_str()])
                            .cwd(project),
                    )?;
                }
                // Bun keeps a local package it has, changed or not: upgraded,
                // it is removed first.
                if let (true, Source::Local(_)) = (upgrade, &reference.source) {
                    if let Some(name) = self
                        .local_language(&reference.from, &local_path(reference))?
                        .1
                    {
                        if project.join("node_modules").join(&name).exists() {
                            self.tools.run(
                                &Run::new("bun").args(["remove", name.as_str()]).cwd(project),
                            )?;
                        }
                    }
                }
                self.tools
                    .run(&Run::new("bun").args(["add", spec.as_str()]).cwd(project))?;
            }
        }
        // One plugin, the package: its name as installed.
        let name = match &reference.source {
            Source::Npm { package, .. } => package.clone(),
            Source::Local(path) => {
                self.local_language(&reference.from, path)?
                    .1
                    .ok_or_else(|| {
                        Error::Other(format!("{}: its package.json has no name", reference.from))
                    })?
            }
            _ => unreachable!(),
        };
        let version = js_version(project, &name);
        let uninstall = match lang {
            Lang::Node => format!("npm uninstall --prefix {} {name}", quoted_path(project)),
            _ => format!("bun remove --cwd {} {name}", quoted_path(project)),
        };
        Ok(Package {
            plugins: vec![Installed {
                name,
                from: reference.from.clone(),
                language: lang.key().into(),
                version,
            }],
            uninstall,
        })
    }

    /// Python: the package into the environment (or interpreter) `place`.
    fn install_python(
        &self,
        reference: &Reference,
        place: &Path,
        upgrade: bool,
    ) -> Result<Package, Error> {
        let python = self.python_ready(place)?;
        let (spec, distribution) = match &reference.source {
            Source::Pypi { package, version } => (
                match version {
                    Some(version) => format!("{package}=={version}"),
                    None => package.clone(),
                },
                package.clone(),
            ),
            Source::Local(path) => (
                path.display().to_string(),
                python_distribution(path).ok_or_else(|| {
                    Error::Other(format!(
                        "{}: its pyproject.toml has no [project] name",
                        reference.from
                    ))
                })?,
            ),
            _ => unreachable!("only PyPI packages and local paths are Python"),
        };
        let local = matches!(reference.source, Source::Local(_));
        let mut options: Vec<String> = Vec::new();
        let mut spec = spec;
        if upgrade && python.project.is_some() {
            // `uv add` keeps a requirement given again without a version
            // as the project has it (`bo-llm==0.3.0` stays): from the
            // version there to the newest.
            if let Source::Pypi { version: None, .. } = &reference.source {
                let found = self.python_query(&python, "distribution", &distribution)?;
                if let Some(version) = found.and_then(|found| found.version) {
                    spec = format!("{distribution}>={version}");
                }
            }
            let option = match local {
                true => "--reinstall-package",
                false => "--upgrade-package",
            };
            options.extend([option.into(), distribution.clone()]);
        } else if upgrade {
            match (python.uv, local) {
                (true, true) => {
                    options.extend(["--reinstall-package".into(), distribution.clone()])
                }
                (false, true) => options.extend(["--force-reinstall".into(), "--no-deps".into()]),
                (_, false) => options.push("--upgrade".into()),
            }
        }
        self.tools.run(&python.install(&options, &spec))?;
        let uninstall = python.uninstall_shown(&distribution);
        let found = self.python_query(&python, "distribution", &distribution)?;
        let Some(found) = found else {
            return Err(Error::Other(format!(
                "{distribution} is not in {} after installing it",
                python.interpreter.display()
            )));
        };
        let plugins: Vec<Installed> = found
            .plugins
            .iter()
            .map(|name| Installed {
                name: name.clone(),
                from: reference.from.clone(),
                language: Lang::Python.key().into(),
                version: found.version.clone(),
            })
            .collect();
        if plugins.is_empty() {
            return Err(Error::Other(format!(
                "{distribution} has no rutis.plugins entry point: it is not a plugin; it stays installed: {uninstall}"
            )));
        }
        Ok(Package { plugins, uninstall })
    }

    /// Go and Swift: the binary into `dir`.
    fn install_binary(
        &self,
        reference: &Reference,
        lang: Lang,
        dir: &Path,
    ) -> Result<Package, Error> {
        std::fs::create_dir_all(dir)
            .map_err(|error| Error::Other(format!("{}: {}", dir.display(), io_error(&error))))?;
        let binaries: Vec<PathBuf> = match &reference.source {
            Source::GoModule { path, version } => {
                let module = format!("{path}@{}", version.as_deref().unwrap_or("latest"));
                go_install(&self.tools, &module, dir)?;
                vec![dir.join(executable(go_command_name(path)))]
            }
            Source::Local(source) if source.is_dir() => {
                let run = Run::new("go")
                    .args(["install", "./..."])
                    .cwd(source)
                    .env("GOBIN", dir);
                self.tools.run_or(&run, |error| no_go(error, dir))?;
                let list = Run::new("go")
                    .args([
                        "list",
                        "-f",
                        "{{if eq .Name \"main\"}}{{.Target}}{{end}}",
                        "./...",
                    ])
                    .cwd(source)
                    .env("GOBIN", dir);
                let listed = self.tools.output_or(&list, |error| no_go(error, dir))?;
                listed
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from)
                    .collect()
            }
            Source::Local(source) => {
                let file = source
                    .file_name()
                    .ok_or_else(|| Error::Other(format!("{}: not a file", reference.from)))?;
                let target = dir.join(file);
                if target != *source {
                    std::fs::copy(source, &target).map_err(|error| {
                        Error::Other(format!(
                            "cannot copy {} into {}: {}",
                            source.display(),
                            dir.display(),
                            io_error(&error)
                        ))
                    })?;
                }
                vec![target]
            }
            _ => unreachable!("only Go modules and local paths are Go or Swift"),
        };
        let uninstall = binaries
            .iter()
            .map(|binary| {
                format!(
                    "{} {}",
                    if cfg!(windows) { "del" } else { "rm" },
                    quoted_path(binary)
                )
            })
            .collect::<Vec<_>>()
            .join(" && ");
        let found = binaries_in(lang, dir);
        let mut plugins = Vec::new();
        for binary in &binaries {
            let manifest = manifest_of(lang, &found, binary).map_err(|why| {
                Error::Other(format!(
                    "{} {why}: it is not a plugin; it stays installed: {uninstall}",
                    binary.display()
                ))
            })?;
            plugins.extend(manifest.iter().map(|(name, schema)| Installed {
                name: name.clone(),
                from: reference.from.clone(),
                language: lang.key().into(),
                version: schema.version.clone(),
            }));
        }
        if plugins.is_empty() {
            return Err(Error::Other(format!(
                "{} serves no plugins: it is not a plugin; it stays installed: {uninstall}",
                reference.from
            )));
        }
        Ok(Package { plugins, uninstall })
    }

    // ── Upgrade and remove ──────────────────────────────────────

    /// The version of the package `plugin` is in, as installed now.
    fn version_of(&self, lang: Lang, plugin: &Installed) -> Option<String> {
        match lang {
            Lang::Node => js_version(&self.places.node, &plugin.name),
            Lang::Bun => js_version(&self.places.bun, &plugin.name),
            Lang::Python => {
                let python = self.python_at(&self.places.python);
                self.python_query(&python, "plugin", &plugin.name)
                    .ok()??
                    .version
            }
            Lang::Go | Lang::Swift => binaries_serving(
                lang,
                &self.binary_dir(lang).ok()?,
                std::slice::from_ref(&plugin.name),
            )
            .into_iter()
            .find_map(|(_, schema)| schema.version),
        }
    }

    fn remove(&self, lang: Lang, plugins: &[Installed]) -> Result<Option<String>, Error> {
        // One package: Node and Bun's one plugin is it, and Python finds it
        // by any of its entry points.
        let plugin = &plugins[0];
        match lang {
            Lang::Node | Lang::Bun => {
                let project = match lang {
                    Lang::Node => &self.places.node,
                    _ => &self.places.bun,
                };
                if !project.join("node_modules").join(&plugin.name).exists() {
                    return Ok(None);
                }
                let run = match lang {
                    Lang::Node => Run::new("npm").args(["uninstall", plugin.name.as_str()]),
                    _ => Run::new("bun").args(["remove", plugin.name.as_str()]),
                };
                self.tools.run(&run.cwd(project))?;
                Ok(Some(plugin.name.clone()))
            }
            Lang::Python => {
                let python = self.python_at(&self.places.python);
                let Some(found) = self.python_query(&python, "plugin", &plugin.name)? else {
                    return Ok(None);
                };
                self.tools.run(&python.uninstall(&found.name))?;
                Ok(Some(found.name))
            }
            Lang::Go | Lang::Swift => {
                let names: Vec<String> = plugins.iter().map(|plugin| plugin.name.clone()).collect();
                let binaries: Vec<PathBuf> =
                    binaries_serving(lang, &self.binary_dir(lang)?, &names)
                        .into_iter()
                        .map(|(path, _)| path)
                        .collect();
                if binaries.is_empty() {
                    return Ok(None);
                }
                let mut names = Vec::new();
                for binary in binaries {
                    std::fs::remove_file(&binary).map_err(|error| {
                        Error::Other(format!(
                            "cannot delete {}: {}",
                            binary.display(),
                            io_error(&error)
                        ))
                    })?;
                    names.push(
                        binary
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                Ok(Some(names.join(", ")))
            }
        }
    }

    // ── Python ──────────────────────────────────────────────────

    /// The interpreter of `place`, and the environment to create when it is
    /// not there (`None` for an interpreter outside one).
    fn python_at(&self, place: &Path) -> Python {
        let (interpreter, environment) = match venv_of(place) {
            Some(venv) => (place.to_owned(), Some(venv)),
            None if place.is_file() => (place.to_owned(), None),
            None => (venv_python(place), Some(place.to_owned())),
        };
        let uv = self.tools.succeeds(&Run::new("uv").arg("--version"));
        let project = &self.places.python_project;
        let same = |a: &Path, b: &Path| {
            a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
        };
        let project = (uv
            && project.join("pyproject.toml").is_file()
            && project.join("uv.lock").is_file()
            && environment
                .as_deref()
                .is_some_and(|env| same(env, &project.join(".venv"))))
        .then(|| project.clone());
        Python {
            interpreter,
            environment,
            uv,
            project,
        }
    }

    /// The interpreter of `place`, its environment created when missing,
    /// with the `rutis` package of this version in it.
    fn python_ready(&self, place: &Path) -> Result<Python, Error> {
        let python = self.python_at(place);
        if !python.interpreter.exists() {
            let Some(venv) = &python.environment else {
                return Err(Error::Other(format!(
                    "cannot run {}: it does not exist",
                    python.interpreter.display()
                )));
            };
            let create = match python.uv {
                true => Run::new("uv").arg("venv").arg(venv),
                false => Run::new(DEFAULT_PYTHON).args(["-m", "venv"]).arg(venv),
            };
            self.tools.run_or(&create, |_| {
                format!(
                    "cannot create the Python environment {}: install uv, or Python 3.12 or later",
                    venv.display()
                )
            })?;
        }
        let mut check = Run::new(&python.interpreter).args(["-c", "import rutis"]);
        if let Some(source) = std::env::var_os("RUTIS_PYTHON_PATH") {
            check = check.env("PYTHONPATH", source);
        }
        if !self.tools.succeeds(&check) {
            let package = format!("rutis=={VERSION}");
            self.tools.run(&python.install(&[], &package))?;
        }
        Ok(python)
    }

    /// Ask the interpreter about a distribution: by its name
    /// (`distribution`), or by a plugin's entry point name (`plugin`).
    fn python_query(&self, python: &Python, by: &str, name: &str) -> Result<Option<Found>, Error> {
        let run = Run::new(&python.interpreter).args(["-c", PYTHON_QUERY, by, name]);
        let text = self.tools.output(&run)?;
        Ok(serde_json::from_str(text.trim()).unwrap_or(None))
    }
}

/// The Python of a place.
struct Python {
    interpreter: PathBuf,
    environment: Option<PathBuf>,
    /// Whether uv runs (else pip).
    uv: bool,
    /// The uv project whose `.venv` the environment is: packages are added
    /// to it (`uv add --dev`), so that `uv sync` keeps them.
    project: Option<PathBuf>,
}

impl Python {
    /// The command installing `spec`, with `options`.
    fn install(&self, options: &[String], spec: &str) -> Run {
        if let Some(project) = &self.project {
            return Run::new("uv")
                .args(["add", "--dev", "--project"])
                .arg(project)
                .args(options)
                .arg(spec);
        }
        match self.uv {
            true => Run::new("uv")
                .args(["pip", "install", "--python"])
                .arg(&self.interpreter)
                .args(options)
                .arg(spec),
            false => Run::new(&self.interpreter)
                .args(["-m", "pip", "install"])
                .args(options)
                .arg(spec),
        }
    }

    fn uninstall(&self, distribution: &str) -> Run {
        if let Some(project) = &self.project {
            return Run::new("uv")
                .args(["remove", "--dev", "--project"])
                .arg(project)
                .arg(distribution);
        }
        match self.uv {
            true => Run::new("uv")
                .args(["pip", "uninstall", "--python"])
                .arg(&self.interpreter)
                .arg(distribution),
            false => Run::new(&self.interpreter)
                .args(["-m", "pip", "uninstall", "-y"])
                .arg(distribution),
        }
    }

    /// The command that uninstalls `distribution`, as messages give it.
    fn uninstall_shown(&self, distribution: &str) -> String {
        self.uninstall(distribution).shown()
    }
}

/// What the interpreter says of a distribution.
#[derive(serde::Deserialize)]
struct Found {
    name: String,
    version: Option<String>,
    plugins: Vec<String>,
}

/// Finds a distribution by name (`distribution <name>`) or by an entry
/// point of the group `rutis.plugins` (`plugin <name>`): its name, version
/// and entry points there, as JSON; `null` when there is none.
const PYTHON_QUERY: &str = r#"
import importlib.metadata as m, json, re, sys
by, want = sys.argv[1], sys.argv[2]
norm = lambda s: re.sub(r"[-_.]+", "-", s or "").lower()
for d in m.distributions():
    points = [e.name for e in d.entry_points if e.group == "rutis.plugins"]
    if (by == "distribution" and norm(d.metadata["Name"]) == norm(want)) or (by == "plugin" and want in points):
        print(json.dumps({"name": d.metadata["Name"], "version": d.version, "plugins": sorted(set(points))}))
        break
else:
    print("null")
"#;

/// The distribution name of the local Python package `path`: a wheel's
/// file name, or a project's `[project] name`.
fn python_distribution(path: &Path) -> Option<String> {
    if path.is_file() {
        let file = path.file_name()?.to_str()?;
        return file.split('-').next().map(str::to_owned);
    }
    let text = std::fs::read_to_string(path.join("pyproject.toml")).ok()?;
    let mut inside = false;
    for line in text.lines().map(str::trim) {
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

// ── Node and Bun ────────────────────────────────────────────────

/// The path a local reference names.
fn local_path(reference: &Reference) -> PathBuf {
    match &reference.source {
        Source::Local(path) => path.clone(),
        _ => PathBuf::new(),
    }
}

/// `<package>[@<version>]`.
fn npm_spec(package: &str, version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{package}@{version}"),
        None => package.to_owned(),
    }
}

/// Bun when the package depends on `@arcships/rutis-bun`, as a project
/// that runs in Bun does; else Node.
fn js_lang(manifest: &serde_json::Value) -> Lang {
    match crate::languages::bun::depends_on_runtime(manifest) {
        true => Lang::Bun,
        false => Lang::Node,
    }
}

/// The version of the package `name` installed in `project`.
fn js_version(project: &Path, name: &str) -> Option<String> {
    let manifest = read_json(&project.join("node_modules").join(name).join("package.json")).ok()?;
    manifest["version"].as_str().map(str::to_owned)
}

fn read_json(path: &Path) -> Result<serde_json::Value, Error> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::Other(format!("{}: {}", path.display(), io_error(&error))))?;
    serde_json::from_str(&text)
        .map_err(|error| Error::Other(format!("{}: {error}", path.display())))
}

// ── Go and Swift ────────────────────────────────────────────────

fn go_install(tools: &Tools, module: &str, dir: &Path) -> Result<(), Error> {
    tools.run_or(
        &Run::new("go").args(["install", module]).env("GOBIN", dir),
        |error| no_go(error, dir),
    )
}

/// rutis-host's words when go cannot be run.
fn no_go(error: &str, dir: &Path) -> String {
    format!(
        "cannot run go ({error}): install the Go toolchain, or download the plugin's binary \
         for this platform into {} yourself",
        dir.display()
    )
}

/// The command `go install` writes for the package path `path`: its last
/// element, or the one before a major version suffix (`/v2`).
fn go_command_name(path: &str) -> &str {
    let mut parts = path.rsplit('/');
    let last = parts.next().unwrap_or(path);
    let major =
        last.len() > 1 && last.starts_with('v') && last[1..].bytes().all(|b| b.is_ascii_digit());
    match (major, parts.next()) {
        (true, Some(before)) => before,
        _ => last,
    }
}

fn executable(name: &str) -> String {
    match cfg!(windows) {
        true => format!("{name}.exe"),
        false => name.to_owned(),
    }
}

// ── Runtimes ────────────────────────────────────────────────────

/// A binary's plugins, by name, as its manifest says.
type Plugins = std::collections::BTreeMap<String, RowSchema>;

/// The binaries in `dir` built with `lang`'s SDK, as its runtime finds
/// them (`BinaryResolver::binaries`): each with its plugins, or why it has
/// none.
fn binaries_in(lang: Lang, dir: &Path) -> Vec<(PathBuf, Result<Plugins, String>)> {
    use rutis_loader::runtime::binary::{Binaries, BinaryResolver};
    use rutis_loader::runtime::{go::Go, swift::Swift};
    let binaries = Binaries::new().dir(dir);
    let found = match lang {
        Lang::Go => BinaryResolver::<Go>::new(binaries).binaries(),
        Lang::Swift => BinaryResolver::<Swift>::new(binaries).binaries(),
        _ => Vec::new(),
    };
    found
        .into_iter()
        .map(|binary| {
            let plugins = binary.manifest.map(|manifest| manifest.plugins.clone());
            (binary.path, plugins)
        })
        .collect()
}

/// The manifest of `binary`, one of `found` ([`binaries_in`] of its
/// directory): its plugins, or why it is not a plugin.
fn manifest_of(
    lang: Lang,
    found: &[(PathBuf, Result<Plugins, String>)],
    binary: &Path,
) -> Result<Plugins, String> {
    match found
        .iter()
        .find(|(path, _)| path.file_name() == binary.file_name())
    {
        None => Err(format!("was not built with the rutis {} SDK", lang.title())),
        Some((_, manifest)) => manifest
            .clone()
            .map_err(|why| format!("has no manifest ({why})")),
    }
}

/// The binaries in `dir` serving any of the plugins `names`, each with
/// the schema of the first it serves.
fn binaries_serving(lang: Lang, dir: &Path, names: &[String]) -> Vec<(PathBuf, RowSchema)> {
    binaries_in(lang, dir)
        .into_iter()
        .filter_map(|(path, plugins)| {
            let plugins = plugins.ok()?;
            let schema = names.iter().find_map(|name| plugins.get(name))?.clone();
            Some((path, schema))
        })
        .collect()
}

#[cfg(test)]
mod tests;
