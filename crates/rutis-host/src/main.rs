//! rutis-host: a rutis host that needs no Rust.
//!
//! ```text
//! rutis-host run [rutis.json]      run what the file names
//! rutis-host dev                   run the plugin project here, reloading it as it changes
//! rutis-host check [rutis.json]    describe every row, and fail on what cannot run
//! rutis-host new <name> --lang node|bun|python|go
//! rutis-host go add <module>@<version> [rutis.json]
//! ```

use rutis_host::{config, errln, host, languages, new, outln, project, status};
use rutis_loader::{RowStatus, Supply};

mod reload;
mod stop;

use stop::Stopped;

/// The plugin API of Go binaries this host runs (their manifests); Node, Bun
/// and Python runtimes check their own plugins against their own API. One
/// definition, shared with the loader that reads the manifests.
const PLUGIN_API: u64 = rutis_loader::PLUGIN_API;

const USAGE: &str = "\
rutis-host: run rutis plugins written in TypeScript, JavaScript, Python and Go

usage:
  rutis-host run [rutis.json]                 run what the configuration names
  rutis-host dev [dir]                        run the plugin project in dir (default .),
                                              reloading (Go: rebuilding) it as its files change
    run and dev: --shutdown-timeout <seconds> how long cleanups may take after
                                              Ctrl-C or SIGTERM (default 10, or
                                              RUTIS_SHUTDOWN_TIMEOUT)
    run: --watch-config                       read rutis.json again when it changes
                                              (SIGUSR1 does too, on Unix)
  rutis-host check [rutis.json]               describe every row and Go binary; fail on what
                                              cannot run (in a plugin project without
                                              rutis.json: the project)
  rutis-host new <name> --lang node|bun|python|go
                                              create a plugin project
  rutis-host go add <module>@<version> [rutis.json]
                                              install a Go plugin binary into runtimes.go.dir
                                              (go install; needs the Go toolchain)
  rutis-host --version

Credentials for links come from RUTIS_TOKEN (or RUTIS_TOKEN_<PEER>), RUTIS_CA,
RUTIS_CERT and RUTIS_KEY.

Exit status: 0 when it ends normally (also on Ctrl-C or SIGTERM once every
cleanup ran), 1 when it cannot start or run (configuration, loading, a row
check fails), 2 when stopping did not finish (past the deadline, a second
Ctrl-C, or runtime processes that did not exit).";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("new") => new_project(&args[1..]).map(|()| Stopped::Done),
        Some("go") => go_command(&args[1..]).map(|()| Stopped::Done),
        Some("--version" | "-V") => {
            outln!("rutis-host {}", env!("CARGO_PKG_VERSION"));
            Ok(Stopped::Done)
        }
        Some("--help" | "-h" | "help") | None => {
            outln!("{USAGE}");
            Ok(Stopped::Done)
        }
        Some(command @ ("run" | "dev" | "check")) => serve(command, &args[1..]),
        Some(other) => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    // The runtimes' last lines, still being passed on once they ended.
    host::output_written();
    match result {
        Ok(Stopped::Done) => {}
        Ok(Stopped::Failed) => std::process::exit(1),
        Ok(Stopped::Unfinished) => std::process::exit(stop::UNFINISHED),
        Err(error) => {
            errln!("rutis-host: {error}");
            std::process::exit(1);
        }
    }
}

/// On macOS, a downloaded binary carries the quarantine attribute, and the
/// system refuses to run it unsigned: say so, and what to do.
fn quarantine_hint(path: &std::path::Path) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let quarantined = std::process::Command::new("xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(path)
        .output()
        .ok()?
        .status
        .success();
    quarantined.then(|| {
        format!(
            "it was downloaded and is quarantined: once you trust it, run `xattr -d com.apple.quarantine {}`",
            path.display()
        )
    })
}

fn new_project(args: &[String]) -> Result<(), String> {
    let mut name = None;
    let mut lang = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lang" => lang = args.next().cloned(),
            other if other.starts_with("--lang=") => {
                lang = Some(other["--lang=".len()..].to_owned())
            }
            other if name.is_none() => name = Some(other.to_owned()),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    let names = languages::new_names();
    let name =
        name.ok_or_else(|| format!("usage: rutis-host new <name> --lang {}", names.join("|")))?;
    let lang = lang.ok_or_else(|| {
        let options: Vec<String> = names.iter().map(|name| format!("--lang {name}")).collect();
        format!("which language? {}", languages::either(&options))
    })?;
    new::create(std::path::Path::new("."), &name, &lang)?;
    // Created: the language is one of them.
    let next = languages::by_new_name(&lang).map_or("", |language| language.next_steps());
    outln!("created {name}/\nnext: cd {name} && {next}");
    Ok(())
}

fn serve(command: &str, args: &[String]) -> Result<Stopped, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        match command {
            "run" => run(args).await,
            "dev" => dev(args).await,
            _ => check(args).await.map(|()| Stopped::Done),
        }
    })
}

/// Show what reading the configuration went on without.
fn warn(config: &config::HostConfig) {
    for warning in &config.warnings {
        errln!("rutis-host: warning: {warning}");
    }
}

fn config_file(args: &[String]) -> std::path::PathBuf {
    std::path::PathBuf::from(args.first().map(String::as_str).unwrap_or("rutis.json"))
}

async fn run(args: &[String]) -> Result<Stopped, String> {
    let (deadline, args) = stop::deadline(args)?;
    let (watch, args) = reload::watch_flag(args);
    stop::pass_on(deadline);
    let mut signals = stop::Signals::listen()?;
    let path = config_file(&args);
    // From the start: SIGUSR1 before the host runs reloads once it does,
    // instead of ending it.
    let mut triggers = reload::Triggers::listen(&path, watch)?;
    let config = config::HostConfig::read(&path)?;
    warn(&config);
    let mut host = host::Host::start(&config).await?;
    for line in status::listening(&host.listening) {
        outln!("{line}");
    }
    let started = async {
        host.runtimes_ready().await?;
        host.load(config.rows()).await
    };
    let signal = match start(&host, started, deadline, &mut signals).await {
        Started::Running => {
            status::follow(host.followed());
            loop {
                tokio::select! {
                    signal = signals.next() => break signal,
                    _ = triggers.next() => {}
                }
                // A signal ends a reload half way: the host stops anyway.
                tokio::select! {
                    () = reload::reload(&mut host, &path, &mut triggers) => {}
                    signal = signals.next() => break signal,
                }
            }
        }
        Started::Signal(signal) => signal,
        Started::Failed(stopped) => return Ok(stopped.failed()),
    };
    Ok(stop::stop(&host, signal, deadline, &mut signals).await)
}

/// `rutis-host go add <module>@<version> [rutis.json]`: `go install` the
/// binary into the configuration's Go plugin directory.
fn go_command(args: &[String]) -> Result<(), String> {
    let usage = "usage: rutis-host go add <module>@<version> [rutis.json]";
    let (Some("add"), Some(module)) = (args.first().map(String::as_str), args.get(1)) else {
        return Err(usage.into());
    };
    if !module.contains('@') {
        return Err(format!("{module}: name a version, as {module}@latest"));
    }
    let config = config::HostConfig::read(&config_file(&args[2..]))?;
    warn(&config);
    let dir = config
        .runtimes
        .go
        .and_then(|go| go.dir)
        .ok_or("the configuration has no runtimes.go.dir to install into")?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let status = std::process::Command::new("go")
        .args(["install", module.as_str()])
        .env("GOBIN", &dir)
        .status()
        .map_err(|error| {
            format!(
                "cannot run go ({error}): install the Go toolchain, or download the plugin's binary \
                 for this platform into {} yourself",
                dir.display()
            )
        })?;
    if !status.success() {
        return Err(format!("go install {module} failed"));
    }
    outln!("installed {module} into {}", dir.display());
    Ok(())
}

/// How starting went.
enum Started {
    Running,
    /// A signal came first: its name.
    Signal(&'static str),
    /// It failed (the error is printed), and what did start was shut down.
    Failed(Stopped),
}

/// Wait for `started`, unless a signal comes first. When it fails, what
/// did start is shut down, as after a signal.
async fn start(
    host: &host::Host,
    started: impl std::future::Future<Output = Result<(), String>>,
    deadline: std::time::Duration,
    signals: &mut stop::Signals,
) -> Started {
    let started = tokio::select! {
        started = started => started,
        signal = signals.next() => return Started::Signal(signal),
    };
    match started {
        Ok(()) => Started::Running,
        Err(error) => {
            errln!("rutis-host: {error}");
            Started::Failed(stop::finish(host, deadline, signals).await)
        }
    }
}

async fn dev(args: &[String]) -> Result<Stopped, String> {
    use std::time::Duration;

    let (deadline, args) = stop::deadline(args)?;
    stop::pass_on(deadline);
    let mut signals = stop::Signals::listen()?;
    let dir = std::path::PathBuf::from(args.first().map(String::as_str).unwrap_or("."));
    // Go runtimes warn of calls that carry no call chain.
    std::env::set_var("RUTIS_DEV", "1");
    let (config, id) = project::dev_config(&dir)?;
    warn(&config);
    // A Go project is rebuilt, not reloaded: the loop below does both.
    let go_project = match languages::of_project(&dir) {
        Some(language) if language.key() == "go" => {
            project::GoDev::of(&std::path::absolute(&dir).unwrap_or(dir.clone()))
        }
        _ => None,
    };
    let mut build = 1;
    let host = host::Host::start(&config).await?;
    for line in status::listening(&host.listening) {
        outln!("{line}");
    }
    let started = async {
        host.runtimes_ready().await?;
        host.load(config.rows()).await
    };
    match start(&host, started, deadline, &mut signals).await {
        Started::Running => {}
        Started::Signal(signal) => {
            return Ok(stop::stop(&host, signal, deadline, &mut signals).await)
        }
        Started::Failed(stopped) => return Ok(stopped.failed()),
    }
    // Before saying it runs: a change made once it says so is a change.
    let mut seen = project::sources(&dir);
    outln!("rutis-host dev: running {id}; changes reload it (Ctrl-C ends)");
    status::follow(host.followed());
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(400)) => {}
            signal = signals.next() => {
                return Ok(stop::stop(&host, signal, deadline, &mut signals).await);
            }
        }
        let now = project::sources(&dir);
        if now == seen {
            continue;
        }
        seen = now;
        if let (Some(go), Some(running)) = (&go_project, &host.go) {
            // A Go plugin is a binary: build it again, then restart its
            // runtime on the new build. A failed build keeps the old one.
            // Nor does a build: a signal ends it (and the go command).
            let built = tokio::select! {
                built = go.rebuild(build + 1) => built,
                signal = signals.next() => {
                    return Ok(stop::stop(&host, signal, deadline, &mut signals).await);
                }
            };
            match built {
                Ok(binary) => {
                    build += 1;
                    running.resolver.replace(&go.runtime, &binary);
                    // A runtime that hangs starting does not keep a signal out.
                    let restarted = tokio::select! {
                        restarted = running.runtimes.restart(&go.runtime) => restarted,
                        signal = signals.next() => {
                            return Ok(stop::stop(&host, signal, deadline, &mut signals).await);
                        }
                    };
                    match restarted {
                        Ok(()) => outln!("{}: rebuilt and restarted", go.runtime),
                        Err(error) => outln!("{}: cannot restart: {error}", go.runtime),
                    }
                    let previous = go.dir.join(".rutis/go").join(match cfg!(windows) {
                        true => format!("{}-{}.exe", go.name, build - 1),
                        false => format!("{}-{}", go.name, build - 1),
                    });
                    let _ = std::fs::remove_file(previous);
                }
                Err(errors) => outln!(
                    "{}: the build failed; the last build keeps running\n{errors}",
                    go.name
                ),
            }
            continue;
        }
        // Plugin rows load their new code; peer rows keep their sessions.
        host.invalidate();
        let rows: Vec<String> = host
            .loader
            .entries()
            .into_iter()
            .filter(|entry| entry.options["name"] != "rutis-bridge/peer")
            .map(|entry| entry.id)
            .collect();
        for row in rows {
            // A plugin whose new code hangs does not keep a signal out.
            let reloaded = tokio::select! {
                reloaded = host.loader.reload(&row) => reloaded,
                signal = signals.next() => {
                    return Ok(stop::stop(&host, signal, deadline, &mut signals).await);
                }
            };
            match reloaded {
                Ok(_) => outln!("{row}: reloaded"),
                Err(error) => outln!("{row}: cannot reload: {error}"),
            }
        }
    }
}

async fn check(args: &[String]) -> Result<(), String> {
    let path = config_file(args);
    let config = match (args.first(), path.exists()) {
        (None, false) => project::dev_config(std::path::Path::new("."))?.0,
        _ => config::HostConfig::read(&path)?,
    };
    warn(&config);
    let host = host::Host::start(&config).await?;
    host.runtimes_ready().await?;
    outln!(
        "plugin API for Go binaries: {PLUGIN_API} (checked by this host; the Node, Bun and Python runtimes check their own plugins)"
    );
    // Each local runtime: what implements it and what it runs on.
    for (name, runtime) in &host.runtimes {
        if let Some(process) = runtime.ready().await {
            if let Some(line) = runtime_line(name, process.about()) {
                outln!("{line}");
            }
        }
    }
    let report = host.check(config.rows()).await?;
    for issue in &report.issues {
        errln!("rutis-host: warning: {issue}");
    }
    let mut failed = report.failing().count();
    for row in &report.rows {
        let (id, name) = (&row.id, &row.name);
        match &row.status {
            RowStatus::Disabled => outln!("{id} ({name}): disabled"),
            RowStatus::Failed(error) => {
                outln!("{id} ({name}): {error}");
                if let rutis_loader::LoaderError::NotFound { .. } = error {
                    outln!(
                        "  install it where its runtime finds it (a runtime under runtimes), \
                         or correct the row's name"
                    );
                }
            }
            RowStatus::Checked {
                resolved, requires, ..
            } => {
                let missing: Vec<&str> = requires
                    .iter()
                    .filter(|requirement| matches!(requirement.supply, Supply::Missing { .. }))
                    .map(|requirement| requirement.service.as_str())
                    .collect();
                match missing.is_empty() {
                    true => outln!("{id} ({name}): ok"),
                    false => outln!(
                        "{id} ({name}): cannot run: nothing provides {}",
                        missing.join(", ")
                    ),
                }
                // A plugin needing a newer plugin API does not resolve: its
                // runtime refuses it, saying what to upgrade.
                let meta = &resolved.meta;
                for field in ["runtime", "binary", "version", "inject", "provides"] {
                    if let Some(value) = meta.get(field).filter(|value| !value.is_null()) {
                        outln!("  {field}: {value}");
                    }
                }
                if let Some(schema) = &resolved.schema {
                    outln!("  config: {schema}");
                }
                for requirement in requires {
                    if let Some(line) = supply_line(&requirement.service, &requirement.supply) {
                        outln!("  {line}");
                    }
                }
            }
            _ => outln!("{id} ({name}): not checked"),
        }
    }
    if let Some(go) = &host.go {
        outln!("go binaries:");
        for binary in go.resolver.binaries() {
            match &binary.manifest {
                Ok(manifest) => {
                    let plugins: Vec<String> = manifest
                        .plugins
                        .iter()
                        .map(|(name, plugin)| match plugin.version.as_deref() {
                            Some(version) => format!("{name} {version}"),
                            None => name.clone(),
                        })
                        .collect();
                    let api = match manifest.plugin_api <= PLUGIN_API {
                        true => "compatible",
                        false => "incompatible: upgrade the host",
                    };
                    outln!(
                        "  {} ({}): {}sdk {}, plugin API {} ({api}), plugins: {}",
                        binary.runtime,
                        binary.path.display(),
                        manifest
                            .platform
                            .as_ref()
                            .map(|platform| format!("{platform}, "))
                            .unwrap_or_default(),
                        manifest.sdk,
                        manifest.plugin_api,
                        plugins.join(", ")
                    );
                    if manifest.plugin_api > PLUGIN_API {
                        failed += 1;
                    }
                }
                Err(error) => {
                    failed += 1;
                    outln!("  {} ({}): {error}", binary.runtime, binary.path.display());
                    if let Some(hint) = quarantine_hint(&binary.path) {
                        outln!("    {hint}");
                    }
                }
            }
        }
        for skipped in go.resolver.diagnostics() {
            failed += 1;
            outln!("  skipped {skipped}");
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} row(s) or binaries cannot run")),
    }
}

/// `check`'s line for a service a row waits for: what to do when nothing
/// provides it, what may when that is not known; none when it is provided.
fn supply_line(service: &str, supply: &Supply) -> Option<String> {
    match supply {
        Supply::Missing { disabled } if !disabled.is_empty() => Some(format!(
            "{service}: only disabled rows provide it ({}): enable one, or add a row that provides it",
            disabled.join(", ")
        )),
        Supply::Missing { .. } => Some(format!(
            "{service}: add a row that provides it, or import it from a peer (import: [\"{service}\"])"
        )),
        Supply::Unknown(sources) => Some(format!(
            "{service}: not known to be provided; it may come from {}",
            sources
                .iter()
                .map(|source| format!("{} ({})", source.row, source.why))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => None,
    }
}

/// `check`'s line for the runtime `name`: what implements it and what it
/// runs on, as it said when mounted (`Process::about`); none when it said
/// neither.
fn runtime_line(name: &str, about: &serde_json::Value) -> Option<String> {
    let said = |field: &str| {
        let part = &about[field];
        part["name"]
            .as_str()
            .map(|name| match part["version"].as_str() {
                Some(version) => format!("{name} {version}"),
                None => name.to_owned(),
            })
    };
    let described: Vec<String> = ["implementation", "engine"]
        .iter()
        .filter_map(|field| said(field))
        .collect();
    (!described.is_empty()).then(|| format!("runtime {name}: {}", described.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::runtime_line;
    use serde_json::json;

    /// guarantee: host/check
    #[test]
    fn check_says_what_runs_each_runtime() {
        let about = json!({
            "implementation": { "name": "@arcships/rutis-bun", "version": "0.11.0" },
            "engine": { "name": "bun", "version": "1.4.3" },
        });
        assert_eq!(
            runtime_line("bun", &about).as_deref(),
            Some("runtime bun: @arcships/rutis-bun 0.11.0, bun 1.4.3")
        );
        let partial = json!({ "implementation": { "name": "rutis" } });
        assert_eq!(
            runtime_line("py", &partial).as_deref(),
            Some("runtime py: rutis")
        );
        assert_eq!(runtime_line("node", &serde_json::Value::Null), None);
    }
}
