//! `rutis-host add`, `remove` and `upgrade`: plugins installed from where
//! they come from (npm, PyPI, a Go module, a local path), recorded in
//! `installed` and given a row (docs/design-plugin-install-2026-10-27.md).
//!
//! The installing is [`crate::installer`]'s; here are the file (which
//! one, its records, its rows and runtimes, written back in its own key
//! order) and what is said.

use std::path::{Path, PathBuf};

use crate::installer::{Error as InstallError, Installed, Installer, Places};
use rutis_loader::report::{CheckReport, RowCheck, RowStatus};
use serde_json::{json, Map, Value};

use crate::config::HostConfig;
use crate::languages::id_of;
use crate::{errln, outln};

/// The configuration `add`, `remove` and `upgrade` change: its text as
/// written (keys in their order) and as read.
pub(crate) struct Target {
    path: PathBuf,
    /// The file as written; `{}` for one not there yet.
    raw: Map<String, Value>,
    config: HostConfig,
    /// Its records (`installed`), by row name.
    installed: std::collections::BTreeMap<String, Installed>,
}

impl Target {
    /// `named`, else `rutis.json` here, else `rutis.dev.json` here (a
    /// plugin project), else a new `rutis.json`.
    fn open(named: Option<&str>) -> Result<Self, String> {
        let path = match named {
            Some(path) => PathBuf::from(path),
            None => ["rutis.json", "rutis.dev.json"]
                .iter()
                .map(PathBuf::from)
                .find(|path| path.exists())
                .unwrap_or_else(|| PathBuf::from("rutis.json")),
        };
        let text = match path.exists() {
            true => crate::config::read_text(&path)?,
            false => "{}".to_owned(),
        };
        Self::parse(path, &text)
    }

    /// The file at `path`, its content `text`.
    pub(crate) fn parse(path: PathBuf, text: &str) -> Result<Self, String> {
        let config = HostConfig::parse(text, &path)?;
        let raw = match serde_json::from_str(text) {
            Ok(Value::Object(raw)) => raw,
            _ => return Err(format!("{}: not a JSON object", path.display())),
        };
        let installed = config.installed.clone();
        Ok(Self {
            path,
            raw,
            config,
            installed,
        })
    }

    /// Its file name, as the lines say it (`rutis.json`).
    fn name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// The directory relative paths in it are from, absolute.
    fn base(&self) -> PathBuf {
        let dir = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned())
    }

    /// Whether the file itself declares `runtimes.<key>` (not a runtime
    /// rows enabled with its defaults).
    fn declares(&self, key: &str) -> bool {
        self.raw
            .get("runtimes")
            .and_then(|runtimes| runtimes.get(key))
            .is_some()
    }

    /// Where each language's plugins go: where its runtime, as the file
    /// configures it, finds them (I4).
    fn places(&self) -> Places {
        let base = self.base();
        let runtimes = &self.config.runtimes;
        let project = |project: Option<&Path>| project.map_or_else(|| base.clone(), Path::to_owned);
        let python = runtimes
            .py
            .as_ref()
            .and_then(|py| py.python.clone())
            .or_else(|| std::env::var_os("VIRTUAL_ENV").map(PathBuf::from))
            .unwrap_or_else(|| {
                project(runtimes.py.as_ref().map(|py| py.project.as_path())).join(".venv")
            });
        // Declared without a `dir`: `add` does not know where.
        let binaries = |key: &str, settings: Option<&crate::config::GoRuntime>| {
            match self.declares(key) {
                true => settings.and_then(|settings| settings.dir.clone()).ok_or_else(|| {
                    format!(
                        "runtimes.{key} has no dir to install into: add one, or remove runtimes.{key} to use plugins/{key}"
                    )
                }),
                false => Ok(base.join("plugins").join(key)),
            }
        };
        Places {
            base: base.clone(),
            node: project(runtimes.node.as_ref().map(|node| node.project.as_path())),
            node_runtime: runtimes.node.as_ref().and_then(|node| node.runtime.clone()),
            bun: project(runtimes.bun.as_ref().map(|bun| bun.project.as_path())),
            bun_runtime: runtimes.bun.as_ref().and_then(|bun| bun.runtime.clone()),
            python,
            go: binaries("go", runtimes.go.as_ref()),
            swift: binaries("swift", runtimes.swift.as_ref()),
        }
    }

    /// Where `language`'s plugins are, as `installed … into` says it.
    fn place(&self, language: &str) -> String {
        let places = self.places();
        let place = match language {
            "node" => Ok(places.node),
            "bun" => Ok(places.bun),
            "py" => Ok(places.python),
            "go" => places.go,
            _ => places.swift,
        };
        place
            .map(|place| place.display().to_string())
            .unwrap_or_default()
    }

    fn installer(&self) -> Installer {
        Installer::new(self.places()).with_installed(self.installed.values().cloned())
    }

    /// Its text as it is now: its keys in their order, two-space indents.
    fn text(&self) -> Result<String, String> {
        Ok(
            serde_json::to_string_pretty(&Value::Object(self.raw.clone()))
                .map_err(|error| error.to_string())?
                + "\n",
        )
    }

    /// Write it back ([`Target::text`]) through a file renamed into place
    /// (`run --watch-config` never reads half of it).
    fn write(&self) -> Result<(), String> {
        self.store()
            .map(|_| ())
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }

    /// [`Target::write`], returning the text written, or why it was not.
    pub(crate) fn store(&self) -> std::io::Result<String> {
        let text = self.text().map_err(std::io::Error::other)?;
        let partial =
            self.path
                .with_file_name(format!(".{}.{}.tmp", self.name(), std::process::id()));
        std::fs::write(&partial, &text)
            .and_then(|()| std::fs::rename(&partial, &self.path))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&partial);
            })?;
        Ok(text)
    }

    /// Set `key` to `value`; a new key goes before the first of `before`
    /// there is, else last.
    pub(crate) fn set(&mut self, key: &str, value: Value, before: &[&str]) {
        if let Some(slot) = self.raw.get_mut(key) {
            *slot = value;
            return;
        }
        let at = before
            .iter()
            .find_map(|next| self.raw.keys().position(|key| key == next));
        match at {
            Some(at) => {
                self.raw.shift_insert(at, key.to_owned(), value);
            }
            None => {
                self.raw.insert(key.to_owned(), value);
            }
        }
    }

    fn records(&mut self) -> Map<String, Value> {
        match self.raw.get("installed") {
            Some(Value::Object(records)) => records.clone(),
            _ => Map::new(),
        }
    }

    pub(crate) fn rows(&self) -> Vec<Value> {
        match self.raw.get("rows") {
            Some(Value::Array(rows)) => rows.clone(),
            _ => Vec::new(),
        }
    }
}

/// The package a record's `from` names, as lines say it: without the
/// `npm:` or `pypi:` prefix.
fn package(from: &str) -> &str {
    from.strip_prefix("npm:")
        .or_else(|| from.strip_prefix("pypi:"))
        .unwrap_or(from)
}

/// `: plugin a` or `: plugins a, b`; nothing for an npm package, whose one
/// plugin is the package.
fn plugins_said(plugins: &[Installed]) -> String {
    let names: Vec<&str> = plugins.iter().map(|plugin| plugin.name.as_str()).collect();
    match names.as_slice() {
        [one] if *one == package(&plugins[0].from) => String::new(),
        [one] => format!(": plugin {one}"),
        all => format!(": plugins {}", all.join(", ")),
    }
}

/// `<package> <version> (<language>)`.
fn package_said(plugins: &[Installed]) -> String {
    let first = &plugins[0];
    match &first.version {
        Some(version) => format!("{} {version} ({})", package(&first.from), first.language),
        None => format!("{} ({})", package(&first.from), first.language),
    }
}

/// The error of an install step, as rutis-host says it.
fn install_error(error: InstallError, file: &str) -> String {
    match error {
        InstallError::Failed(message) => format!("{message}; {file} is unchanged"),
        other => other.to_string(),
    }
}

/// `rutis-host add <reference> [rutis.json]`; `print_row` prints a new
/// row's lines as `check` does.
pub async fn add(
    args: &[String],
    print_row: impl Fn(&HostConfig, &CheckReport, &RowCheck),
) -> Result<(), String> {
    let (reference, file) = match args {
        [reference] => (reference.as_str(), None),
        [reference, file] => (reference.as_str(), Some(file.as_str())),
        _ => {
            return Err(
                "usage: rutis-host add <npm:package | pypi:package | Go module | ./path>[@version] [rutis.json]"
                    .into(),
            )
        }
    };
    let mut target = Target::open(file)?;
    let installer = target.installer();
    let crate::installer::Added {
        plugins: added,
        uninstall,
    } = installer
        .add(reference)
        .await
        .map_err(|error| install_error(error, &target.name()))?;
    let language = added[0].language.clone();
    outln!(
        "installed {} into {}{}",
        package_said(&added),
        target.place(&language),
        plugins_said(&added)
    );

    // The file as it will be: records, rows, the runtime.
    let mut records = target.records();
    for plugin in &added {
        records.insert(plugin.row_name(), json!({ "from": plugin.from }));
    }
    let mut said = Vec::new();
    if !target.declares(&language) {
        let settings = match language.as_str() {
            "go" | "swift" => json!({ "dir": format!("plugins/{language}") }),
            _ => json!({}),
        };
        let mut runtimes = match target.raw.get("runtimes") {
            Some(Value::Object(runtimes)) => runtimes.clone(),
            _ => Map::new(),
        };
        runtimes.insert(language.clone(), settings);
        target.set(
            "runtimes",
            Value::Object(runtimes),
            &["listen", "installed", "rows"],
        );
        said.push(format!("runtimes.{language}"));
    }
    target.set("installed", Value::Object(records), &["rows"]);
    let mut rows = target.rows();
    let mut ids = Vec::new();
    let mut names = Vec::new();
    collect_rows(&rows, &mut ids, &mut names);
    let mut new_rows = Vec::new();
    for plugin in &added {
        if names.contains(&plugin.row_name()) {
            continue;
        }
        let base = id_of(&plugin.name);
        let id = (1..)
            .map(|n| match n {
                1 => base.clone(),
                n => format!("{base}-{n}"),
            })
            .find(|id| !ids.contains(id))
            .expect("a free id");
        ids.push(id.clone());
        rows.push(json!({ "id": id, "name": plugin.row_name() }));
        new_rows.push(id);
    }
    target.set("rows", Value::Array(rows), &[]);
    // A new file says its format first.
    if !target.path.exists() {
        target.set("format", json!(crate::config::FORMAT), &["runtimes"]);
    }

    // The plugins' rows checked as `check` does, before the file is
    // written: a row that does not resolve is not a plugin.
    let names: Vec<String> = added.iter().map(Installed::row_name).collect();
    check_rows(&target, &names, &print_row)
        .await
        .map_err(|error| format!("{error}; it stays installed: {uninstall}"))?;
    target.write()?;
    if !new_rows.is_empty() {
        said.insert(
            0,
            match new_rows.len() {
                1 => format!("row {}", new_rows[0]),
                _ => format!("rows {}", new_rows.join(", ")),
            },
        );
    }
    match said.is_empty() {
        true => outln!(
            "{}: rows naming {} were there already",
            target.name(),
            names.join(", ")
        ),
        false => outln!("{}: added {}", target.name(), said.join(", ")),
    }
    Ok(())
}

/// Each row's id and name in `rows` and its groups.
fn collect_rows(rows: &[Value], ids: &mut Vec<String>, names: &mut Vec<String>) {
    for row in rows {
        if let Some(id) = row["id"].as_str() {
            ids.push(id.to_owned());
        }
        if let Some(name) = row["name"].as_str() {
            names.push(name.to_owned());
        }
        if is_group(row) {
            if let Some(children) = row["config"].as_array() {
                collect_rows(children, ids, names);
            }
        }
    }
}

/// Check the configuration as it will be written, as `check` does, and
/// print the rows named `names`; a row of them that does not resolve
/// fails.
async fn check_rows(
    target: &Target,
    names: &[String],
    print_row: &impl Fn(&HostConfig, &CheckReport, &RowCheck),
) -> Result<(), String> {
    let text = target.text()?;
    let mut config = match target.name() == "rutis.dev.json" {
        true => crate::project::dev_config_of(&target.base(), &text)?.0,
        false => HostConfig::parse(&text, &target.path)?,
    };
    // The new rows run in local runtimes: the check binds no listener and
    // connects no remote runtime, so a host running this file keeps its
    // ports and links (§十 I10). Remote rows say their runtime is not
    // running, as `check` does when it is not connected.
    config.listen.clear();
    for remote in &mut config.runtimes.remote {
        remote.dial = None;
        remote.listen = None;
        remote.instances = None;
    }
    let host = crate::host::Host::start(&config).await?;
    host.runtimes_ready().await?;
    let report = host.check(config.rows()).await?;
    let rows: Vec<&RowCheck> = report
        .rows
        .iter()
        .filter(|row| names.contains(&row.name))
        .collect();
    for row in &rows {
        match &row.status {
            RowStatus::Failed(error) if error.config().is_none() => {
                return Err(error.to_string());
            }
            _ => {}
        }
    }
    for row in rows {
        print_row(&config, &report, row);
    }
    Ok(())
}

/// The records of the package `wanted` names: a plugin's row name, its
/// name (refused when plugins of several languages have it), or its
/// reference (`from`, with no version).
fn package_of<'a>(
    target: &'a Target,
    wanted: &str,
) -> Result<Option<(String, Vec<&'a Installed>)>, String> {
    let installed = &target.installed;
    let named: Vec<&Installed> = installed
        .values()
        .filter(|record| record.name == wanted)
        .collect();
    if installed.get(wanted).is_none() && named.len() > 1 {
        let rows: Vec<String> = named.iter().map(|record| record.row_name()).collect();
        return Err(format!(
            "{wanted} is the name of more than one plugin: name it by its row ({})",
            rows.join(", ")
        ));
    }
    let Some(from) = installed
        .get(wanted)
        .or(named.first().copied())
        .or_else(|| installed.values().find(|record| record.from == wanted))
        .map(|record| record.from.clone())
    else {
        return Ok(None);
    };
    let plugins = installed
        .values()
        .filter(|record| record.from == from)
        .collect();
    Ok(Some((from, plugins)))
}

/// `rutis-host remove <plugin or reference> [rutis.json]`.
pub async fn remove(args: &[String]) -> Result<(), String> {
    let (wanted, file) = match args {
        [wanted] => (wanted.as_str(), None),
        [wanted, file] => (wanted.as_str(), Some(file.as_str())),
        _ => return Err("usage: rutis-host remove <plugin or reference> [rutis.json]".into()),
    };
    let mut target = Target::open(file)?;
    let Some((from, plugins)) = package_of(&target, wanted)? else {
        return Err(format!(
            "{wanted} was not installed by add: uninstall it with the package manager that installed it, and remove its row"
        ));
    };
    let plugins: Vec<Installed> = plugins.into_iter().cloned().collect();
    let place = target.place(&plugins[0].language);
    match target
        .installer()
        .remove(&plugins)
        .await
        .map_err(|error| install_error(error, &target.name()))?
    {
        Some(uninstalled) => outln!("uninstalled {uninstalled} from {place}"),
        None => outln!("{} was no longer installed in {place}", package(&from)),
    }
    let names: Vec<String> = plugins.iter().map(|plugin| plugin.name.clone()).collect();
    let rows_of: Vec<String> = plugins.iter().map(Installed::row_name).collect();
    if names.len() > 1 {
        outln!(
            "{} had the plugins {}: all of them are removed",
            package(&from),
            names.join(", ")
        );
    }
    let mut records = target.records();
    records.retain(|row, _| !rows_of.contains(row));
    target.set("installed", Value::Object(records), &["rows"]);
    let mut rows = target.rows();
    let mut removed = Vec::new();
    remove_rows(&mut rows, &rows_of, &mut removed);
    target.set("rows", Value::Array(rows), &[]);
    target.write()?;
    for id in removed {
        outln!("{}: removed row {id}", target.name());
    }
    Ok(())
}

/// Take the rows named one of `names` out of `rows` and its groups; their
/// ids into `removed`.
fn remove_rows(rows: &mut Vec<Value>, names: &[String], removed: &mut Vec<String>) {
    rows.retain(|row| {
        let gone = row["name"]
            .as_str()
            .is_some_and(|name| names.iter().any(|n| n == name));
        if gone {
            removed.push(row["id"].as_str().unwrap_or_default().to_owned());
        }
        !gone
    });
    for row in rows {
        if is_group(row) {
            if let Some(children) = row["config"].as_array_mut() {
                remove_rows(children, names, removed);
            }
        }
    }
}

/// `rutis-host upgrade [<plugin or reference>[@version]] [rutis.json]`.
pub async fn upgrade(args: &[String]) -> Result<(), String> {
    let (wanted, file) = match args {
        [] => (None, None),
        [one] if one.ends_with(".json") => (None, Some(one.as_str())),
        [wanted] => (Some(wanted.as_str()), None),
        [wanted, file] => (Some(wanted.as_str()), Some(file.as_str())),
        _ => {
            return Err(
                "usage: rutis-host upgrade [<plugin or reference>[@version]] [rutis.json]".into(),
            )
        }
    };
    let mut target = Target::open(file)?;
    // The packages: the one named (with its version, if one is given), or
    // every package recorded.
    let mut packages: Vec<(String, Option<String>)> = Vec::new();
    match wanted {
        Some(wanted) => {
            let mut found = package_of(&target, wanted)?.map(|(from, _)| (from, None));
            if found.is_none() {
                if let Some(at) = wanted.rfind('@').filter(|at| *at > 0) {
                    found = package_of(&target, &wanted[..at])?
                        .map(|(from, _)| (from, Some(wanted[at + 1..].to_owned())));
                }
            }
            let found = found.ok_or_else(|| {
                format!(
                    "{wanted} was not installed by add: upgrade it with the package manager that installed it"
                )
            })?;
            packages.push(found);
        }
        None => {
            for record in target.installed.values() {
                if !packages.iter().any(|(from, _)| *from == record.from) {
                    packages.push((record.from.clone(), None));
                }
            }
        }
    }
    if packages.is_empty() {
        outln!("{}: no plugins were installed by add", target.name());
        return Ok(());
    }
    let installer = target.installer();
    let mut records = target.records();
    let rows = target.rows();
    let mut failed = false;
    for (from, version) in &packages {
        let before: Vec<Installed> = target
            .installed
            .values()
            .filter(|record| record.from == *from)
            .cloned()
            .collect();
        let upgraded = match installer.upgrade(&before[0], version.as_deref()).await {
            Ok(upgraded) => upgraded,
            Err(error) => {
                errln!("rutis-host: {}", install_error(error, &target.name()));
                failed = true;
                continue;
            }
        };
        let now = upgraded
            .plugins
            .first()
            .and_then(|plugin| plugin.version.clone());
        let package = package(from);
        match (&upgraded.before, &now) {
            (Some(old), Some(new)) if old == new && version.is_none() => {
                outln!("{package}: {new} is the latest")
            }
            (Some(old), Some(new)) if old == new => outln!("{package}: {new} is installed"),
            (Some(old), Some(new)) => outln!("{package}: {old} → {new}"),
            (_, Some(new)) => outln!("{package}: {new}"),
            _ => outln!("{package}: upgraded"),
        }
        for plugin in &upgraded.plugins {
            if !before.iter().any(|record| record.name == plugin.name) {
                records.insert(plugin.row_name(), json!({ "from": plugin.from }));
                outln!(
                    "{package}: new plugin {name}: add a row {{ \"id\": \"{id}\", \"name\": \"{row}\" }} to run it",
                    name = plugin.name,
                    row = plugin.row_name(),
                    id = id_of(&plugin.name)
                );
            }
        }
        for record in &before {
            if upgraded
                .plugins
                .iter()
                .any(|plugin| plugin.name == record.name)
            {
                continue;
            }
            records.remove(&record.row_name());
            let mut ids = Vec::new();
            rows_named(&rows, &record.row_name(), &mut ids);
            match ids.is_empty() {
                true => outln!("{package}: the new version has no plugin {}", record.name),
                false => {
                    for id in ids {
                        outln!("{id}: the new version has no plugin {}", record.name);
                    }
                }
            }
        }
    }
    if records != target.records() {
        target.set("installed", Value::Object(records), &["rows"]);
        target.write()?;
    }
    outln!("a running host keeps the old versions until it restarts");
    match failed {
        true => Err("not every plugin was upgraded".into()),
        false => Ok(()),
    }
}

/// The ids of the rows in `rows` and its groups named `name`.
fn rows_named(rows: &[Value], name: &str, ids: &mut Vec<String>) {
    for row in rows {
        if row["name"] == name {
            ids.push(row["id"].as_str().unwrap_or_default().to_owned());
        }
        if is_group(row) {
            if let Some(children) = row["config"].as_array() {
                rows_named(children, name, ids);
            }
        }
    }
}

/// Whether `row` is a group (its `config` holds rows), as the loader reads
/// `group`.
fn is_group(row: &Value) -> bool {
    !matches!(
        row.get("group"),
        None | Some(Value::Null | Value::Bool(false))
    )
}
