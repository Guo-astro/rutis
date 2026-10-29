//! `rutis-host dev`: run the plugin project in a directory, and follow it as
//! it changes.
//!
//! - A changed file reloads the rows whose file it is (`dev/fake-llm.ts`,
//!   `src/fake_llm.py`); any other file of the project reloads the
//!   project's own row. A Go or Swift project is rebuilt instead when its
//!   sources change, and its runtime restarted on the new build; its other
//!   files rebuild nothing.
//! - `rutis.dev.json` is read again when its content changes (or on
//!   SIGUSR1), as `run --watch-config` reads `rutis.json`: all or nothing,
//!   the rows that changed restarting with their new fields.
//! - The rows that restart because a row they use reloaded restart by
//!   themselves (it is how services gate rows); each says so.
//! - A local runtime whose process ended is started again by the next
//!   change to one of its rows, instead of reloading rows nothing runs.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rutis::host::{FiberState, PluginId};
use rutis_bridge::runtime::RuntimeState;
use rutis_host::config::{read_text, HostConfig};
use rutis_host::host::{Host, RowChange};
use rutis_host::languages::{self, DevBuild};
use rutis_host::{errln, project, status};
use rutis_loader::report::{cannot_reload, EntryStatus};

use crate::reload::Triggers;
use crate::stop::{self, Ended, Stopped};
use crate::{start, warn, Started};

/// The file `dev` reads besides the project's own.
const DEV_FILE: &str = "rutis.dev.json";

/// How often the project's files are looked at.
const POLL: Duration = Duration::from_millis(400);

pub async fn dev(args: &[String]) -> Result<Stopped, String> {
    let (deadline, args) = stop::deadline(args)?;
    let (debug, args) = crate::take_flag(&args, "--debug");
    // Each line a plugin writes says which row wrote it, unless asked not
    // to (#678).
    let (prefix_output, args) = crate::prefix_flag(args, true);
    stop::pass_on(deadline);
    let mut signals = stop::Signals::listen()?;
    let given = PathBuf::from(args.first().map(String::as_str).unwrap_or("."));
    let dir = std::path::absolute(&given).unwrap_or(given);
    // From the start, as `run` does: the dev file's content now is what the
    // host starts with.
    let mut triggers = Triggers::listen(&dir.join(DEV_FILE), true, Default::default())?;
    // Go runtimes warn of calls that carry no call chain.
    std::env::set_var("RUTIS_DEV", "1");
    // A compiled language's project (Go, Swift) is rebuilt, not reloaded:
    // the loop below does both, and its first build runs here, where a
    // signal still ends it.
    let language = languages::of_project(&dir);
    if let Some(language) = language {
        language.available()?;
        // Before building: a debugger that is missing is said at once.
        if debug {
            match language.debugging() {
                Some(Err(error)) => return Err(error),
                Some(Ok(_)) => {}
                None => {
                    return Err(format!(
                        "rutis-host dev --debug: a {} project has no debugger here; \
                         --debug runs Python, Node, Bun and Go projects",
                        language.key()
                    ))
                }
            }
        }
    }
    let compiled = language.and_then(|language| {
        let build = language.dev_build(&dir, debug)?;
        Some((language.key(), build))
    });
    let (config, id) = match &compiled {
        Some((_, project)) => {
            errln!("{}: building", project.name());
            let started = std::time::Instant::now();
            let built = tokio::select! {
                built = project.rebuild(1) => built,
                signal = signals.next() => {
                    errln!("rutis-host: {signal}: stopping");
                    return Ok(Stopped::Done);
                }
            };
            let binary = built?;
            errln!(
                "{}: built in {:.1}s",
                project.name(),
                started.elapsed().as_secs_f64()
            );
            project::dev_config_built(&dir, &binary)?
        }
        None => project::dev_config(&dir)?,
    };
    let mut config = config;
    config.runtimes.debug = debug;
    config.runtimes.prefix_output = prefix_output;
    warn(&config);
    if debug {
        for line in debugging(&config)? {
            errln!("{line}");
        }
    }
    let mut host = Host::start(&config).await?;
    let _status = crate::answer_status(&dir, &host);
    for line in status::listening(&host.listening) {
        errln!("{line}");
    }
    let started = host.start_rows(config.rows());
    match start(&host, started, deadline, &mut signals).await {
        Started::Running => {}
        Started::Signal(signal) => {
            return Ok(stop::stop(&host, signal, deadline, &mut signals).await)
        }
        Started::Failed(stopped) => return Ok(stopped.failed()),
    }
    // Before saying it runs: a change made once it says so is a change.
    let mut seen = project::sources(&dir);
    errln!("rutis-host dev: running {id}; changes reload it (Ctrl-C ends)");
    // Once every row settled: a row whose apply does not return keeps it
    // waiting, not the changes nor a signal.
    // After that, the services are said again whenever they changed: a
    // runtime can provide one after its row settled.
    let shown = std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        let loader = host.loader.clone();
        let shown = shown.clone();
        tokio::spawn(async move {
            loader.settled().await;
            show_services(&loader, &shown);
        });
    }
    status::follow(host.followed());
    let mut exits = host.supervise();
    let mut project = Project {
        dir,
        own: id,
        compiled,
        build: 1,
        debug,
        prefix_output,
        stale: Vec::new(),
        // Printed at the start (`warn`).
        warned: config.warnings.iter().cloned().collect(),
    };
    let stopped = 'dev: loop {
        remove_builds(&mut project.stale);
        tokio::select! {
            _ = tokio::time::sleep(POLL) => {
                let changed = shown
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|line| *line != services_line(&host.loader));
                if changed {
                    show_services(&host.loader, &shown);
                }
            }
            signal = signals.next() => {
                break 'dev stop::stop(&host, signal, deadline, &mut signals).await;
            }
            (runtime, why) = exits.next() => {
                let ended = Ended::Runtime(runtime, why);
                break 'dev stop::end(&host, ended, deadline, &mut signals).await;
            }
            _ = triggers.next() => {
                // A signal ends a reload half way: the host stops anyway.
                tokio::select! {
                    () = async {
                        project.reconfigure(&mut host, &mut triggers).await;
                        host.loader.settled().await;
                        show_services(&host.loader, &shown);
                    } => {}
                    signal = signals.next() => {
                        break 'dev stop::stop(&host, signal, deadline, &mut signals).await;
                    }
                }
                continue;
            }
        }
        let now = project::sources(&project.dir);
        if now == seen {
            continue;
        }
        let dev_file = project.dir.join(DEV_FILE);
        // The dev file is read again by its watch, once its content settled.
        let changed: Vec<PathBuf> = changed_files(&seen, &now)
            .into_iter()
            .filter(|path| *path != dev_file)
            .collect();
        seen = now;
        if changed.is_empty() {
            continue;
        }
        // A Go or Swift plugin is a binary: build it again, then restart its
        // runtime on the new build. A failed build keeps the old one. Nor
        // does a build or a runtime that hangs starting keep a signal out.
        let rebuild = project
            .compiled
            .as_ref()
            .is_some_and(|(_, build)| changed.iter().any(|path| build.is_source(path)));
        if rebuild {
            tokio::select! {
                () = project.rebuild(&host) => {}
                signal = signals.next() => {
                    break 'dev stop::stop(&host, signal, deadline, &mut signals).await;
                }
            }
        }
        // Then the rows whose files changed. A plugin whose new code hangs
        // does not keep a signal out either.
        tokio::select! {
            () = async {
                // Once the change ended, the services as they are now.
                if project.reload(&host, &changed).await || rebuild {
                    host.loader.settled().await;
                    show_services(&host.loader, &shown);
                }
            } => {}
            signal = signals.next() => {
                break 'dev stop::stop(&host, signal, deadline, &mut signals).await;
            }
        }
    };
    // Stopped: no runtime runs them now.
    remove_builds(&mut project.stale);
    Ok(stopped)
}

/// The services and the rows providing them, as `Loader::status()` has
/// them: `rutis-host dev: services: forecast (weather), llm (llm)`, by
/// name; `host` for one the host provides (`loader`); `none` when no row
/// provides one, whatever the host does.
fn services_line(loader: &rutis_loader::Loader) -> String {
    let status = loader.status();
    if status
        .services
        .iter()
        .all(|service| service.provider.is_none())
    {
        return "rutis-host dev: services: none".to_owned();
    }
    let services: Vec<String> = status
        .services
        .iter()
        .map(|service| {
            format!(
                "{} ({})",
                service.name,
                service.provider.as_deref().unwrap_or("host")
            )
        })
        .collect();
    format!("rutis-host dev: services: {}", services.join(", "))
}

/// What `dev --debug` says of each local runtime `config` starts: where its
/// debugger is. A debugger that is not installed is an error, also for a
/// runtime the dev file adds; a language without one runs as usual.
fn debugging(config: &HostConfig) -> Result<Vec<String>, String> {
    languages::LANGUAGES
        .iter()
        .filter(|language| language.configured(&config.runtimes))
        .filter_map(|language| language.debugging())
        .map(|how| how.map(|how| format!("rutis-host dev: debugging: {how}")))
        .collect()
}

/// The plugin project `dev` runs.
struct Project {
    /// Its directory, absolute.
    dir: PathBuf,
    /// The id of its own row (of a Go or Swift project, the first of its
    /// rows).
    own: String,
    /// A compiled language's project (Go, Swift): its language's key and its
    /// build.
    compiled: Option<(&'static str, Box<dyn DevBuild>)>,
    /// The build its runtime runs.
    build: u32,
    /// Its runtimes run under their debuggers (`--debug`).
    debug: bool,
    /// Its plugins' lines start with their row's name.
    prefix_output: bool,
    /// Builds no runtime runs any more, still to delete: Windows refuses
    /// while it still holds the image of a process that just ended.
    stale: Vec<PathBuf>,
    /// The configuration's warnings printed so far: reading `rutis.dev.json`
    /// again prints a warning once, not on every reload.
    warned: HashSet<String>,
}

impl Project {
    /// Build the compiled project again and restart its runtime on the new
    /// build; the rows of other runtimes that restart with it say so.
    async fn rebuild(&mut self, host: &Host) {
        let Some((key, project)) = &self.compiled else {
            return;
        };
        let Some(running) = host.binaries_of(key) else {
            return;
        };
        errln!("{}: building", project.name());
        let started = std::time::Instant::now();
        let binary = match project.rebuild(self.build + 1).await {
            Ok(binary) => binary,
            Err(errors) => {
                errln!(
                    "{}: the build failed; the last build keeps running\n{errors}",
                    project.name()
                );
                return;
            }
        };
        errln!(
            "{}: built in {:.1}s",
            project.name(),
            started.elapsed().as_secs_f64()
        );
        let before = Rows::of(host);
        running.replace(project.runtime(), &binary);
        match running.restart_runtime(project.runtime()).await {
            Ok(()) => {
                self.build += 1;
                errln!("{}: rebuilt and restarted", project.runtime());
                // Not the build the next start uses: once nothing runs it,
                // it can go.
                self.stale.push(project.binary(self.build - 1));
                remove_builds(&mut self.stale);
                host.loader.settled().await;
                let own = |id: &str| row_name(host, id).is_some_and(|name| owns(key, &name));
                // Its rows restarted with it: a row using one says so.
                let said = format!("{} restarted", project.runtime());
                let causes: Vec<(String, String)> = host
                    .loader
                    .entries()
                    .into_iter()
                    .filter(|entry| own(&entry.id))
                    .map(|entry| (entry.id, said.clone()))
                    .collect();
                for line in Rows::of(host).restarted(&before, &causes, &|id| own(id)) {
                    errln!("{line}");
                }
            }
            Err(error) => {
                // The new binary did not run: keep it and the previous
                // build, and point the runtime back at the previous one, so
                // a later change can retry.
                running.replace(project.runtime(), &project.binary(self.build));
                errln!(
                    "{}: cannot restart: {error}; the previous build is kept",
                    project.runtime()
                );
            }
        }
    }

    /// Reload the rows whose files are among `changed`; a runtime that is
    /// down starts again instead, its rows with it. Whether any row had a
    /// file among them.
    async fn reload(&self, host: &Host, changed: &[PathBuf]) -> bool {
        let plan = self.plan(host, changed);
        if plan.is_empty() {
            return false;
        }
        let mut skip: HashSet<String> = HashSet::new();
        for (runtime, rows) in down_runtimes(host, &plan) {
            let why = plan
                .iter()
                .filter(|(id, _)| rows.contains(id))
                .flat_map(|(_, files)| files.iter().cloned())
                .collect::<Vec<_>>();
            start_again(host, &runtime, &format!("{} changed", self.files(&why))).await;
            skip.extend(rows);
        }
        let mut reloaded: HashSet<String> = HashSet::new();
        let mut affected = Affected::default();
        for (id, files) in &plan {
            if skip.contains(id) {
                continue;
            }
            let before = Rows::of(host);
            match host.loader.reload(id).await {
                Ok(_) => {
                    host.loader.settled().await;
                    errln!("{id}: reloaded ({} changed)", self.files(files));
                    reloaded.insert(id.clone());
                    let cause = [(id.clone(), format!("{id} reloaded"))];
                    affected.add(&before, &Rows::of(host), &cause);
                }
                Err(error) => {
                    let running = Rows::of(host).active(id);
                    errln!("{}", cannot_reload(id, &error, running));
                }
            }
        }

        for line in affected.lines(&|id| reloaded.contains(id) || skip.contains(id)) {
            errln!("{line}");
        }
        true
    }

    /// Of the project's rows, those whose files are among `changed`, each
    /// with them, in the rows' order. A file no row has is the project's:
    /// its own row reloads for it, unless the project is compiled (its
    /// rebuild is for its sources, and a file of neither is nothing).
    fn plan(&self, host: &Host, changed: &[PathBuf]) -> Vec<(String, Vec<PathBuf>)> {
        let runtimes = &host.config().runtimes;
        let own_language = self.compiled.as_ref().map(|(key, _)| *key);
        let changed: Vec<(PathBuf, PathBuf)> = changed
            .iter()
            .map(|path| (path.clone(), same(path)))
            .collect();
        let mut claimed: HashSet<PathBuf> = HashSet::new();
        let mut plan: Vec<(String, Vec<PathBuf>)> = Vec::new();
        for entry in host.loader.entries() {
            if entry.instance.is_some() {
                continue;
            }
            let Some(name) = entry.options["name"].as_str() else {
                continue;
            };
            if own_language.is_some_and(|key| owns(key, name)) {
                continue;
            }
            let Some(file) = project::row_file(name, runtimes).map(|file| same(&file)) else {
                continue;
            };
            let files: Vec<PathBuf> = changed
                .iter()
                .filter(|(_, canonical)| *canonical == file)
                .map(|(path, _)| path.clone())
                .collect();
            claimed.extend(files.iter().cloned());
            if !files.is_empty() {
                plan.push((entry.id, files));
            }
        }
        if self.compiled.is_some() {
            return plan;
        }
        let rest: Vec<PathBuf> = changed
            .into_iter()
            .map(|(path, _)| path)
            .filter(|path| !claimed.contains(path))
            .collect();
        if !rest.is_empty() {
            match plan.iter_mut().find(|(id, _)| *id == self.own) {
                Some((_, files)) => files.extend(rest),
                None => plan.insert(0, (self.own.clone(), rest)),
            }
        }
        plan
    }

    /// `files` as the lines name them: relative to the project.
    fn files(&self, files: &[PathBuf]) -> String {
        files
            .iter()
            .map(|file| {
                file.strip_prefix(&self.dir)
                    .unwrap_or(file)
                    .display()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Read `rutis.dev.json` again and run what it now says, as `run
    /// --watch-config` does with `rutis.json`
    /// ([`rutis_host::host::Host::reload`]): `rutis-host: reloaded
    /// rutis.dev.json (<n> rows changed)` and a line for each row that
    /// changed, or `rutis-host: reload rejected: <why>; nothing changed`.
    async fn reconfigure(&mut self, host: &mut Host, triggers: &mut Triggers) {
        let path = self.dir.join(DEV_FILE);
        if let Ok(text) = read_text(&path) {
            triggers.read(&text);
        }
        let new = match self.config(host.config()) {
            Ok(new) => new,
            Err(error) => {
                errln!("rutis-host: reload rejected: {error}; nothing changed");
                return;
            }
        };
        let changes = rutis_host::host::row_changes(&host.config().rows, &new.rows);
        let warnings = new.warnings.clone();
        let before = Rows::of(host);
        match host.reload(new).await {
            Ok(reloaded) => {
                for warning in warnings {
                    if self.warned.insert(warning.clone()) {
                        errln!("rutis-host: warning: {warning}");
                    }
                }
                errln!("rutis-host: reloaded {DEV_FILE} ({reloaded})");
                for warning in &reloaded.warnings {
                    errln!("rutis-host: warning: {warning}");
                }
                for (id, change) in &changes {
                    match change {
                        RowChange::Added => errln!("{id}: added"),
                        RowChange::Removed => errln!("{id}: removed"),
                        RowChange::Changed => {
                            errln!("{id}: updated (its row in {DEV_FILE} changed)")
                        }
                    }
                }
                host.loader.settled().await;
                let causes: Vec<(String, String)> = changes
                    .iter()
                    .map(|(id, change)| {
                        let verb = match change {
                            RowChange::Added => "added",
                            RowChange::Removed => "removed",
                            RowChange::Changed => "updated",
                        };
                        (id.clone(), format!("{id} {verb}"))
                    })
                    .collect();
                let changed: HashSet<&str> = changes.iter().map(|(id, _)| id.as_str()).collect();
                for line in Rows::of(host).restarted(&before, &causes, &|id| changed.contains(id)) {
                    errln!("{line}");
                }
                // Rows that changed, of a runtime that is down: it starts
                // again for them.
                let plan: Vec<(String, Vec<PathBuf>)> = changes
                    .iter()
                    .filter(|(_, change)| *change != RowChange::Removed)
                    .map(|(id, _)| (id.clone(), Vec::new()))
                    .collect();
                for (runtime, _) in down_runtimes(host, &plan) {
                    start_again(host, &runtime, &format!("{DEV_FILE} changed")).await;
                }
            }
            Err(error) => {
                for line in rutis_host::host::rejected_lines(&error) {
                    errln!("{line}");
                }
            }
        }
    }

    /// The configuration of the project with its dev file as it is now. A
    /// compiled project's rows are those of the build it runs, which the
    /// configuration `running` names.
    fn config(&self, running: &HostConfig) -> Result<HostConfig, String> {
        let mut config = self.config_as_read(running)?;
        config.runtimes.debug = self.debug;
        config.runtimes.prefix_output = self.prefix_output;
        Ok(config)
    }

    /// [`Project::config`] as the files say it.
    fn config_as_read(&self, running: &HostConfig) -> Result<HostConfig, String> {
        let Some((key, project)) = &self.compiled else {
            return project::dev_config(&self.dir).map(|(config, _)| config);
        };
        let (mut config, _) = project::dev_config_built(&self.dir, &project.binary(self.build))?;
        // The build a runtime runs is the dev loop's to change, not the
        // dev file's: it stays the one the host started with.
        let mut started = running.runtimes.clone();
        if let (Some(dev), Some(started)) = (
            binary_settings(key, &mut config.runtimes),
            binary_settings(key, &mut started),
        ) {
            dev.dev = started.dev.clone();
        }
        Ok(config)
    }
}

/// The settings of the compiled language `key` (`go`, `swift`) in
/// `runtimes`.
fn binary_settings<'a>(
    key: &str,
    runtimes: &'a mut rutis_host::config::Runtimes,
) -> Option<&'a mut rutis_host::config::BinaryRuntime> {
    match key {
        "go" => runtimes.go.as_mut(),
        "swift" => runtimes.swift.as_mut(),
        _ => None,
    }
}

/// Whether the row named `name` is one of the compiled language `key`'s
/// (`<key>:<plugin>`): under `dev` its runtimes run only the project's
/// build.
fn owns(key: &str, name: &str) -> bool {
    name.strip_prefix(key)
        .is_some_and(|rest| rest.starts_with(':'))
}

/// The name of the row `id`.
fn row_name(host: &Host, id: &str) -> Option<String> {
    host.loader
        .get(id)
        .and_then(|entry| entry.options["name"].as_str().map(str::to_owned))
}

/// `path` as it is compared: canonical when it exists (a temporary
/// directory reached through a symbolic link is one directory).
fn same(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// The local runtime (`node`, `py`, `bun`) of the row named `name`, among
/// `runtimes`: the prefix of its name, or Node for a file or package.
fn runtime_of<'a>(name: &str, runtimes: &'a [String]) -> Option<&'a str> {
    if name == rutis_host::connect::DIAL || name == rutis_host::connect::LISTEN {
        return None;
    }
    let prefixed = name
        .split_once(':')
        .map(|(prefix, _)| prefix)
        .filter(|prefix| rutis_loader::runtime::is_runtime_name(prefix));
    let runtime = prefixed.unwrap_or("node");
    runtimes
        .iter()
        .find(|local| local.as_str() == runtime)
        .map(String::as_str)
}

/// The state of the local runtime `name`.
fn runtime_state(host: &Host, name: &str) -> Option<RuntimeState> {
    host.runtimes
        .iter()
        .find(|(runtime, _)| runtime == name)
        .map(|(_, handle)| handle.state())
}

/// The local runtimes that do not run (down, or starting again) and run
/// rows of `plan`, each with those rows: reloading those rows would only
/// describe them without their runtime.
fn down_runtimes(host: &Host, plan: &[(String, Vec<PathBuf>)]) -> Vec<(String, Vec<String>)> {
    let names: Vec<String> = host.runtimes.iter().map(|(name, _)| name.clone()).collect();
    let mut down: Vec<(String, Vec<String>)> = Vec::new();
    for (id, _) in plan {
        let Some(name) = row_name(host, id) else {
            continue;
        };
        let Some(runtime) = runtime_of(&name, &names) else {
            continue;
        };
        if !matches!(
            runtime_state(host, runtime),
            Some(RuntimeState::Down(_) | RuntimeState::Starting)
        ) {
            continue;
        }
        match down.iter_mut().find(|(name, _)| name == runtime) {
            Some((_, rows)) => rows.push(id.clone()),
            None => down.push((runtime.to_owned(), vec![id.clone()])),
        }
    }
    down
}

/// Start the local runtime `runtime`, which does not run, again for a
/// change (`why`), unless it is starting already or its `restart` starts it
/// (`on-failure`, not given up): its rows come back with it either way,
/// their code as it is then.
async fn start_again(host: &Host, runtime: &str, why: &str) {
    if let Some(RuntimeState::Starting) = runtime_state(host, runtime) {
        errln!("runtime {runtime}: starting; its rows start with the code as it is then ({why})");
        return;
    }
    if host.restarts_by_itself(runtime) {
        errln!(
            "runtime {runtime}: down; its restart starts it again, its rows with the code as it is then ({why})"
        );
        return;
    }
    errln!("runtime {runtime}: starting again ({why})");
    if let Err(error) = host.start_runtime_again(runtime).await {
        // Down again: its state line says why, unless it is not.
        if !matches!(runtime_state(host, runtime), Some(RuntimeState::Down(_))) {
            errln!("runtime {runtime}: {error}");
        }
    }
}

/// Each row's fiber, its generation and its state, for the rows whose
/// fiber exists (copies in instances apart): a row that restarted has a new
/// generation, or a new fiber. And the rows each uses
/// ([`Host::row_dependencies`]).
struct Rows {
    states: BTreeMap<String, (Option<PluginId>, u64, FiberState)>,
    uses: BTreeMap<String, BTreeSet<String>>,
}

impl Rows {
    fn of(host: &Host) -> Self {
        Self {
            states: host
                .loader
                .entries()
                .into_iter()
                .filter(|entry| entry.instance.is_none())
                .filter_map(|entry| match &entry.status {
                    EntryStatus::Running(snapshot) => Some((
                        entry.id,
                        (entry.plugin, snapshot.generation, snapshot.state),
                    )),
                    _ => None,
                })
                .collect(),
            uses: host.row_dependencies(),
        }
    }

    /// Whether the row `id` runs.
    fn active(&self, id: &str) -> bool {
        self.states
            .get(id)
            .is_some_and(|(_, _, state)| *state == FiberState::Active)
    }

    /// The rows that restarted, started or stopped since `before`, but
    /// those `skip` takes: one line each, naming what of `causes` it uses
    /// ([`Affected::add`]).
    fn restarted(
        &self,
        before: &Rows,
        causes: &[(String, String)],
        skip: &dyn Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut affected = Affected::default();
        affected.add(before, self, causes);
        affected.lines(skip)
    }
}

/// The rows that restarted, started or stopped because other rows changed,
/// each with the changes it uses.
#[derive(Default)]
struct Affected(BTreeMap<String, (&'static str, Vec<String>)>);

impl Affected {
    /// Note the rows that differ between `before` and `after`, but the
    /// changed rows `causes` (each with what to say of it: `weather
    /// reloaded`). Each is said to have restarted for the changed rows it
    /// uses, else for the rows noted with it that it uses (`greeter
    /// restarted`: through it), else for nothing: a row whose dependency is
    /// not known here (inside a runtime, between Cordis rows) names no
    /// cause rather than a wrong one.
    fn add(&mut self, before: &Rows, after: &Rows, causes: &[(String, String)]) {
        let mut noted: Vec<(&String, &'static str)> = Vec::new();
        for (id, (plugin, generation, state)) in &before.states {
            if causes.iter().any(|(cause, _)| cause == id) {
                continue;
            }
            let now = after.states.get(id);
            let restarted = now.is_some_and(|(p, g, _)| (p, g) != (plugin, generation));
            let active = now.is_some_and(|(_, _, s)| *s == FiberState::Active);
            let was_active = *state == FiberState::Active;
            let what = match (restarted, was_active, active) {
                (true, true, true) => "restarted",
                (true, false, true) => "started",
                (_, true, false) => "stopped",
                _ => continue,
            };
            noted.push((id, what));
        }
        for (id, what) in &noted {
            // Used before the change or after it: a removed row, or one a
            // stopped row no longer reaches, counts.
            let uses = |other: &str| {
                [before, after]
                    .iter()
                    .any(|rows| rows.uses.get(*id).is_some_and(|uses| uses.contains(other)))
            };
            let mut said: Vec<String> = causes
                .iter()
                .filter(|(cause, _)| uses(cause))
                .map(|(_, said)| said.clone())
                .collect();
            if said.is_empty() {
                said = noted
                    .iter()
                    .filter(|(other, _)| other != id && uses(other))
                    .map(|(other, what)| format!("{other} {what}"))
                    .collect();
            }
            let entry = self.0.entry((*id).clone()).or_insert((what, Vec::new()));
            entry.0 = what;
            for cause in said {
                if !entry.1.contains(&cause) {
                    entry.1.push(cause);
                }
            }
        }
    }

    /// A line for each row noted but those `skip` takes: `caller:
    /// restarted (weather reloaded)`, or `caller: restarted` when what it
    /// uses is not known.
    fn lines(&self, skip: &dyn Fn(&str) -> bool) -> Vec<String> {
        self.0
            .iter()
            .filter(|(id, _)| !skip(id))
            .map(|(id, (what, causes))| match causes.is_empty() {
                true => format!("{id}: {what}"),
                false => format!("{id}: {what} ({})", causes.join(", ")),
            })
            .collect()
    }
}

/// Print the services line, and keep it as the one last said.
fn show_services(loader: &rutis_loader::Loader, shown: &std::sync::Mutex<Option<String>>) {
    let line = services_line(loader);
    errln!("{line}");
    *shown.lock().unwrap() = Some(line);
}

/// Delete the builds in `stale`, keeping those that cannot be deleted yet
/// (on Windows, while a process that ran one is not quite gone) for the
/// next try.
fn remove_builds(stale: &mut Vec<PathBuf>) {
    stale.retain(|build| match std::fs::remove_file(build) {
        Ok(()) => false,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    });
}

/// The files added, removed or modified between two scans of
/// [`project::sources`].
fn changed_files(
    before: &[(PathBuf, std::time::SystemTime)],
    after: &[(PathBuf, std::time::SystemTime)],
) -> Vec<PathBuf> {
    let before_set: HashSet<_> = before.iter().collect();
    let after_set: HashSet<_> = after.iter().collect();
    let mut changed: Vec<PathBuf> = before_set
        .symmetric_difference(&after_set)
        .map(|(path, _)| path.clone())
        .collect();
    changed.sort();
    changed.dedup();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that changed, was added or was removed between two scans.
    #[test]
    fn dev_sees_which_files_changed() {
        use std::time::{Duration, SystemTime};
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let before = vec![
            (PathBuf::from("a.go"), at(1)),
            (PathBuf::from("dev/probe.py"), at(1)),
            (PathBuf::from("gone.go"), at(1)),
        ];
        let after = vec![
            (PathBuf::from("a.go"), at(1)),
            (PathBuf::from("dev/probe.py"), at(2)),
            (PathBuf::from("new.go"), at(1)),
        ];
        assert_eq!(
            changed_files(&before, &after),
            ["dev/probe.py", "gone.go", "new.go"].map(PathBuf::from)
        );
        assert!(changed_files(&before, &before).is_empty());
    }

    /// The runtime a row runs in: its prefix, or Node; none for a peer, or
    /// a runtime not local.
    #[test]
    fn a_rows_runtime_is_its_prefix_or_node() {
        let runtimes = ["node", "py", "bun"].map(String::from);
        let of = |name| runtime_of(name, &runtimes);
        assert_eq!(of("py:weather"), Some("py"));
        assert_eq!(of("bun:./report.ts"), Some("bun"));
        assert_eq!(of("file:///p/dev/fake.ts"), Some("node"));
        assert_eq!(of("/p/dev/fake.ts"), Some("node"));
        assert_eq!(of("@acme/weather"), Some("node"));
        assert_eq!(of("go:weather"), None);
        assert_eq!(of("rutis-host/dial"), None);
        assert_eq!(of("rutis-host/listen"), None);
        assert_eq!(runtime_of("dev/fake.ts", &["py".to_owned()]), None);
        assert!(owns("go", "go:weather"));
        assert!(!owns("go", "swift:weather"));
    }

    /// A row restarted by a change names the changed rows it uses, else
    /// the restarted rows it uses, through which the change reached it,
    /// else nothing: never a row it does not use (#665: `caller` uses
    /// `greeter`, which uses `llm`). One that stopped says so; the changed
    /// rows themselves are not counted.
    #[test]
    fn rows_restarted_by_a_change_name_what_they_use() {
        let rows = |list: &[(&str, u64, FiberState)], uses: &[(&str, &[&str])]| Rows {
            states: list
                .iter()
                .map(|(id, generation, state)| {
                    (id.to_string(), (Some(PluginId(1)), *generation, *state))
                })
                .collect(),
            uses: uses
                .iter()
                .map(|(id, used)| (id.to_string(), used.iter().map(|u| u.to_string()).collect()))
                .collect(),
        };
        let uses: &[(&str, &[&str])] = &[
            ("greeter", &["llm"]),
            ("caller", &["greeter"]),
            ("audit", &["greeter"]),
        ];
        let before = rows(
            &[
                ("greeter", 1, FiberState::Active),
                ("llm", 1, FiberState::Active),
                ("caller", 1, FiberState::Active),
                ("audit", 1, FiberState::Active),
                ("idle", 1, FiberState::Active),
                ("loose", 1, FiberState::Active),
            ],
            uses,
        );
        // audit does not come back; loose restarted, what it uses unknown.
        let after = rows(
            &[
                ("greeter", 2, FiberState::Active),
                ("llm", 2, FiberState::Active),
                ("caller", 2, FiberState::Active),
                ("audit", 1, FiberState::Pending),
                ("idle", 1, FiberState::Active),
                ("loose", 2, FiberState::Active),
            ],
            &uses[..2],
        );
        let mut affected = Affected::default();
        affected.add(&before, &after, &[("llm".into(), "llm reloaded".into())]);
        assert_eq!(
            affected.lines(&|id| id == "llm"),
            [
                "audit: stopped (greeter restarted)",
                "caller: restarted (greeter restarted)",
                "greeter: restarted (llm reloaded)",
                "loose: restarted",
            ]
        );

        // Two changes, one after the other, both used: both named, once.
        let uses: &[(&str, &[&str])] = &[("caller", &["weather", "llm"])];
        let before = rows(
            &[
                ("weather", 1, FiberState::Active),
                ("llm", 1, FiberState::Active),
                ("caller", 1, FiberState::Active),
            ],
            uses,
        );
        let middle = rows(
            &[
                ("weather", 2, FiberState::Active),
                ("llm", 1, FiberState::Active),
                ("caller", 2, FiberState::Active),
            ],
            uses,
        );
        let after = rows(
            &[
                ("weather", 2, FiberState::Active),
                ("llm", 2, FiberState::Active),
                ("caller", 3, FiberState::Active),
            ],
            uses,
        );
        let mut affected = Affected::default();
        affected.add(
            &before,
            &middle,
            &[("weather".into(), "weather reloaded".into())],
        );
        affected.add(&middle, &after, &[("llm".into(), "llm reloaded".into())]);
        assert_eq!(
            affected.lines(&|id| id == "llm" || id == "weather"),
            ["caller: restarted (weather reloaded, llm reloaded)"]
        );
    }
}
