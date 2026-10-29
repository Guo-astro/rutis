//! rutis-host: a rutis host that needs no Rust.
//!
//! ```text
//! rutis-host run [rutis.json]      run what the file names
//! rutis-host dev                   run the plugin project here, reloading it as it changes
//! rutis-host check [rutis.json]    describe every row, and fail on what cannot run
//! rutis-host status [rutis.json]   what a running host is doing now
//! rutis-host call <service>.<method> [arguments…]
//!                                  call a service of a running host
//! rutis-host new <name> --lang node|bun|python|go|swift
//! rutis-host add <plugin> [rutis.json]
//! rutis-host remove <plugin> [rutis.json]
//! rutis-host upgrade [<plugin>[@<version>]] [rutis.json]
//! rutis-host types [crate dir] [-o file] [--python file]
//! rutis-host test <plugin> -- <runtime command…> <project>
//! rutis-host test <plugin> --channel <channel>
//! ```

use std::collections::HashSet;

use rutis_host::{call, config, control, errln, host, languages, new, outln, project, status};
use rutis_loader::report::{
    CheckReport, Clash, ConfigStatus, CycleLink, Provider, Requirement, RowCheck, RowStatus, Supply,
};

mod dev;
mod reload;
mod stop;

use stop::{Ended, Stopped};

/// The plugin API of Go and Swift binaries this host runs (their
/// manifests); Node, Bun and Python runtimes check their own plugins
/// against their own API. One definition, shared with the loader that
/// reads the manifests.
const PLUGIN_API: u64 = rutis_loader::internal::PLUGIN_API;
const _: () = assert!(PLUGIN_API == rutis_loader::internal::SWIFT_PLUGIN_API);

/// The oldest plugin API of a binary this host runs: an SDK older than rutis
/// 0.13 (plugin API 2 or lower) does not declare the event protocol of 0.13,
/// so its session is refused at the handshake; `check` says so first.
const OLDEST_PLUGIN_API: u64 = 3;

const USAGE: &str = "\
rutis-host: run rutis plugins written in TypeScript, JavaScript, Python, Go and Swift

usage:
  rutis-host run [rutis.json]                 run what the configuration names
  rutis-host dev [dir]                        run the plugin project in dir (default .),
                                              reloading the rows whose files change (Go, Swift:
                                              rebuilding), and rutis.dev.json when it changes
    dev: --debug                              run the runtimes under their debuggers: Python
                                              stops at breakpoint() in this terminal, Node
                                              and Bun open their inspectors (127.0.0.1:9229,
                                              127.0.0.1:6499), Go runs under Delve
                                              (127.0.0.1:2345)
    run and dev: --shutdown-timeout <seconds> how long cleanups may take after
                                              Ctrl-C or SIGTERM (default 10, or
                                              RUTIS_SHUTDOWN_TIMEOUT)
    run and dev: --prefix-output, --no-prefix-output
                                              start each line a plugin writes with its
                                              row's name ([weather]); on in dev, off in run
    run: --watch-config                       read rutis.json again when it changes
                                              (SIGUSR1 does too, on Unix)
  rutis-host check [rutis.json] [--strict]    describe every row and Go or Swift binary; fail on what
                                              cannot run (in a plugin project without
                                              rutis.json: the project); warn of a service
                                              no row declares, which --strict fails
  rutis-host status [rutis.json | dir]        what the host running that configuration (or
                                              that dev project) does now: each row's state and
                                              why, each service's provider, the runtimes, the
                                              calls not returned yet (not on Windows)
  rutis-host call [--config <rutis.json | dir>] <service>.<method> [argument...]
                                              call a method of a service of the host running that
                                              configuration (or that dev project), as the host;
                                              each argument a JSON value ('\"Oslo\"', 3, '{\"days\":
                                              3}'); prints the result as JSON (not on Windows)
  rutis-host new <name> --lang node|bun|python|go|swift
                                              create a plugin project
  rutis-host add <plugin> [rutis.json]        install a plugin and add its row: <plugin> is
                                              npm:<package>, pypi:<package>, a Go module path or
                                              a local path (directory, .tgz, .whl, binary), with
                                              @<version> or not (the latest); its language's
                                              package manager runs (npm, bun, uv or pip, go)
  rutis-host remove <plugin> [rutis.json]     uninstall what add installed, and remove its rows
  rutis-host upgrade [<plugin>[@<version>]] [rutis.json]
                                              install the newest version (or the one named) of
                                              what add installed; a running host keeps the old
                                              one until it restarts
  rutis-host types [dir] [-o file] [--python file]
                                              write the TypeScript declarations of the Rust
                                              services marked #[rutis::service] in the crate in
                                              dir (default .) and the crates it uses (to stdout,
                                              or to file); --python also writes their Python
                                              types
  rutis-host test <plugin> -- <runtime command...> <project>
                                              for a test tool: a host with no rows of its own,
                                              which starts the runtime as a local runtime
                                              (<runtime command...> <channel> --id test
                                              <project>), loads <plugin> there to drive the
                                              test, and serves rutis-host/test (load, update,
                                              unload, reported) to run plugins in that runtime;
                                              it exits when the runtime ends
                  <plugin> --channel <channel>
                                              the same with a runtime the test tool runs itself,
                                              on the channel it handed over as a local runtime
                                              gets one (fd:<n>, or tcp:<host>:<port> with the
                                              token in RUTIS_CHANNEL_TOKEN)
  rutis-host --version

Credentials for links come from RUTIS_TOKEN_<PEER>, RUTIS_CA, RUTIS_CERT and
RUTIS_KEY.

Exit status: 0 when it ends normally (also on Ctrl-C or SIGTERM once every
cleanup ran), 1 when it cannot start or run (configuration, loading, a row
check fails), 2 when stopping did not finish (past the deadline, a second
Ctrl-C, or runtime processes that did not exit), 3 when a runtime whose
restart is \"exit\" ended and the host stopped.";

/// The commands `USAGE` describes, each with its own lines there.
const COMMANDS: [&str; 11] = [
    "run", "dev", "check", "status", "call", "new", "add", "remove", "upgrade", "types", "test",
];

/// The lines of `USAGE` about `command`: its own entry and the options
/// that name it (`run and dev: …`), as `rutis-host <command> --help`
/// prints them.
fn command_usage(command: &str) -> String {
    let lines = USAGE
        .lines()
        .skip_while(|line| *line != "usage:")
        .skip(1)
        .take_while(|line| !line.is_empty());
    let mut kept = vec!["usage:"];
    let mut keep = false;
    for line in lines {
        if let Some(entry) = line.strip_prefix("  rutis-host ") {
            keep = entry.split_whitespace().next() == Some(command);
        } else if let Some((names, _)) = line
            .strip_prefix("    ")
            .filter(|rest| !rest.starts_with(' '))
            .and_then(|rest| rest.split_once(": "))
        {
            keep = names.split(" and ").any(|name| name == command);
        }
        if keep {
            kept.push(line);
        }
    }
    kept.join("\n")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [command, rest @ ..] = args.as_slice() {
        if COMMANDS.contains(&command.as_str())
            && rest.iter().any(|arg| arg == "--help" || arg == "-h")
        {
            outln!("{}", command_usage(command));
            return;
        }
    }
    let result = match args.first().map(String::as_str) {
        Some("new") => new_project(&args[1..]).map(|()| Stopped::Done),
        Some("types") => types(&args[1..]).map(|()| Stopped::Done),
        Some("--version" | "-V") => {
            outln!("rutis-host {}", env!("CARGO_PKG_VERSION"));
            Ok(Stopped::Done)
        }
        Some("--help" | "-h" | "help") | None => {
            outln!("{USAGE}");
            Ok(Stopped::Done)
        }
        Some("status") => status_command(&args[1..]).map(|()| Stopped::Done),
        Some(command @ ("add" | "remove" | "upgrade")) => {
            install_command(command, &args[1..]).map(|()| Stopped::Done)
        }
        Some("call") => call_command(&args[1..]),
        Some(command @ ("run" | "dev" | "check" | "test")) => serve(command, &args[1..]),
        Some(other) => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    // The runtimes' last lines, still being passed on once they ended.
    host::output_written();
    match result {
        Ok(Stopped::Done) => {}
        Ok(Stopped::Failed) => std::process::exit(1),
        Ok(Stopped::RuntimeEnded) => std::process::exit(stop::RUNTIME_ENDED),
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

/// `rutis-host types [dir] [-o file] [--python file]`: the declarations of
/// the marked Rust services and their Python types, as `rutis_types::Exports`
/// writes them.
fn types(args: &[String]) -> Result<(), String> {
    let mut dir = None;
    let mut output = None;
    let mut python = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => output = Some(args.next().ok_or("-o needs a file")?.to_owned()),
            "--python" => python = Some(args.next().ok_or("--python needs a file")?.to_owned()),
            other if dir.is_none() && !other.starts_with('-') => dir = Some(other.to_owned()),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    let dir = std::path::PathBuf::from(dir.unwrap_or_else(|| ".".into()));
    if !dir.join("Cargo.toml").is_file() {
        return Err(format!(
            "{} has no Cargo.toml: name the Rust crate's directory",
            dir.display()
        ));
    }
    let mut exports = rutis_types::Exports::new();
    if let Some(file) = &output {
        exports = exports.typescript(file);
    }
    if let Some(file) = &python {
        exports = exports.python(file);
    }
    let typescript = exports.write(&dir).map_err(|error| error.to_string())?;
    if output.is_none() {
        outln!("{}", typescript.trim_end());
    }
    Ok(())
}

/// `rutis-host status [rutis.json | dir]`: ask the host running that
/// configuration (by default the one here) what it does now.
fn status_command(args: &[String]) -> Result<(), String> {
    let target = match args {
        [] => std::path::PathBuf::from("."),
        [target] if !target.starts_with('-') => std::path::PathBuf::from(target),
        _ => return Err("usage: rutis-host status [rutis.json | dir]".into()),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let said = runtime.block_on(control::status(&target))?;
    outln!("{}", said.trim_end());
    Ok(())
}

/// `rutis-host call [--config <rutis.json | dir>] <service>.<method>
/// [argument…]`: call the service on the host running that configuration
/// (by default the one here), print the result as JSON. Ctrl-C ends it
/// with 130, which cancels the call on the host.
fn call_command(args: &[String]) -> Result<Stopped, String> {
    let usage =
        "usage: rutis-host call [--config <rutis.json | dir>] <service>.<method> [argument...]";
    let (target, args) = match args {
        [flag, target, rest @ ..] if flag == "--config" => (target.as_str(), rest),
        [flag, ..] if flag == "--config" => return Err(usage.into()),
        _ => (".", args),
    };
    let Some((name, args)) = args.split_first() else {
        return Err(usage.into());
    };
    if cfg!(windows) {
        errln!("rutis-host call is not available on Windows");
        return Ok(Stopped::Failed);
    }
    let request = call::request(name, args)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let target = std::path::PathBuf::from(target);
    runtime.block_on(async {
        tokio::select! {
            called = control::call(&target, &request) => match called {
                Ok(result) => {
                    outln!("{result}");
                    Ok(Stopped::Done)
                }
                Err(control::Refused::NoHost(why) | control::Refused::Error(why)) => Err(why),
            },
            _ = tokio::signal::ctrl_c() => std::process::exit(130),
        }
    })
}

/// Answer `rutis-host status` and `rutis-host call` for `target` while the
/// host runs; a host that cannot says why and runs on.
pub(crate) fn answer_status(
    target: &std::path::Path,
    host: &host::Host,
) -> Option<control::Serving> {
    match control::serve(
        target,
        host.loader.clone(),
        host.status_report(),
        host.root(),
    ) {
        Ok(serving) => Some(serving),
        Err(_) if cfg!(windows) => None,
        Err(why) => {
            errln!("rutis-host: warning: rutis-host status cannot reach this host: {why}");
            None
        }
    }
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

/// `rutis-host add`, `remove` and `upgrade`.
fn install_command(command: &str, args: &[String]) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        match command {
            // Its new rows as `check` prints them.
            "add" => {
                rutis_host::install::add(args, |config, report, row| {
                    print_row(config, report, row, false);
                })
                .await
            }
            "remove" => rutis_host::install::remove(args).await,
            _ => rutis_host::install::upgrade(args).await,
        }
    })
}

fn serve(command: &str, args: &[String]) -> Result<Stopped, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        match command {
            "run" => run(args).await,
            "dev" => dev::dev(args).await,
            "test" => test(args).await,
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
    let (watch, args) = take_flag(&args, "--watch-config");
    // Plugins' output as they write it, unless asked (#678).
    let (prefix, args) = prefix_flag(args, false);
    stop::pass_on(deadline);
    let mut signals = stop::Signals::listen()?;
    let path = config_file(&args);
    // From the start: SIGUSR1 before the host runs reloads once it does,
    // instead of ending it.
    let seen = rutis_host::rows_file::Seen::default();
    let mut triggers = reload::Triggers::listen(&path, watch, seen.clone())?;
    // Its rows are the layer plugins edit through `loader`, written back.
    let file = rutis_host::rows_file::RowsFile::new(&path, seen);
    let mut config = file.read()?;
    config.runtimes.prefix_output = prefix;
    warn(&config);
    let embedding = host::Embedding {
        rows_file: Some(file),
        ..host::Embedding::default()
    };
    let mut host = host::Host::start_with(&config, embedding).await?;
    let _status = answer_status(&path, &host);
    for line in status::listening(&host.listening) {
        errln!("{line}");
    }
    let started = host.start_rows(config.rows());
    let ended = match start(&host, started, deadline, &mut signals).await {
        Started::Running => {
            status::follow(host.followed());
            let mut exits = host.supervise();
            loop {
                tokio::select! {
                    signal = signals.next() => break Ended::Signal(signal),
                    (runtime, why) = exits.next() => break Ended::Runtime(runtime, why),
                    _ = triggers.next() => {}
                }
                // A signal ends a reload half way: the host stops anyway.
                tokio::select! {
                    () = reload::reload(&mut host, &path, &mut triggers) => {}
                    signal = signals.next() => break Ended::Signal(signal),
                }
            }
        }
        Started::Signal(signal) => Ended::Signal(signal),
        Started::Failed(stopped) => return Ok(stopped.failed()),
    };
    Ok(stop::end(&host, ended, deadline, &mut signals).await)
}

/// `rutis-host test <plugin> -- <runtime command…> <project>`, or
/// `rutis-host test <plugin> --channel <channel>`
/// (`docs/design-platform-cleanup-2026-10-28.md` §7.5).
async fn test(args: &[String]) -> Result<Stopped, String> {
    use rutis_host::test::{Session, Test};
    let (deadline, args) = stop::deadline(args)?;
    let usage = "usage: rutis-host test <plugin> -- <runtime command...> <project>\n       rutis-host test <plugin> --channel <channel>";
    let (plugin, testing) = match &args[..] {
        [plugin, flag, channel] if flag == "--channel" => (plugin, Test::given(channel)),
        _ => {
            let (plugin, runtime) = match args.iter().position(|arg| arg == "--") {
                Some(at) => (&args[..at], &args[at + 1..]),
                None => return Err(usage.into()),
            };
            let ([plugin], [command @ .., project]) = (plugin, runtime) else {
                return Err(usage.into());
            };
            if command.is_empty() {
                return Err(usage.into());
            }
            (plugin, Test::new(command, project))
        }
    };
    if plugin.starts_with('-') {
        return Err(usage.into());
    }
    let config: config::HostConfig =
        serde_json::from_value(serde_json::json!({})).map_err(|error| error.to_string())?;
    stop::pass_on(deadline);
    let mut signals = stop::Signals::listen()?;
    let (registering, observing) = (testing.clone(), testing.clone());
    let embedding = host::Embedding {
        builtins: Some(Box::new(move |builtins| registering.register(builtins))),
        observe: Some(std::sync::Arc::new(move |error| observing.observe(error))),
        ..host::Embedding::default()
    };
    let host = host::Host::start_with(&config, embedding).await?;
    let started = async {
        rutis_host::test::start(&host.loader).await?;
        // The plugin that drives the test, once the runtime greeted.
        let mut session = testing.session();
        let greeted = session
            .wait_for(|state| *state != Session::Starting)
            .await
            .map(|state| *state == Session::Connected)
            .unwrap_or(false);
        if !greeted {
            return Ok(());
        }
        testing
            .load(&host.loader, plugin, serde_json::Value::Null)
            .await
            .map(drop)
    };
    let ended = match start(&host, started, deadline, &mut signals).await {
        Started::Running => {
            let mut session = testing.session();
            tokio::select! {
                signal = signals.next() => Ended::Signal(signal),
                ended = session.wait_for(|state| matches!(state, Session::Ended(_))) => {
                    let failed = match ended.as_deref() {
                        Ok(Session::Ended(Some(why))) => Some(why.clone()),
                        _ => None,
                    };
                    drop(ended);
                    let stopped = stop::finish(&host, deadline, &mut signals).await;
                    return Ok(match failed {
                        Some(why) => {
                            errln!("rutis-host: test: {why}");
                            stopped.failed()
                        }
                        None => stopped,
                    });
                }
            }
        }
        Started::Signal(signal) => Ended::Signal(signal),
        Started::Failed(stopped) => return Ok(stopped.failed()),
    };
    Ok(stop::end(&host, ended, deadline, &mut signals).await)
}

/// `flag` (`--watch-config`, `--debug`) taken out of
/// `args`: whether it was there, and the rest.
pub(crate) fn take_flag(args: &[String], flag: &str) -> (bool, Vec<String>) {
    let rest: Vec<String> = args.iter().filter(|arg| *arg != flag).cloned().collect();
    (rest.len() < args.len(), rest)
}

/// `--prefix-output` and `--no-prefix-output` taken out of `args`: whether
/// each line a plugin writes starts with its row's name (the last of them
/// given, else `default`), and the rest.
fn prefix_flag(args: Vec<String>, default: bool) -> (bool, Vec<String>) {
    let mut prefix = default;
    let mut rest = Vec::with_capacity(args.len());
    for arg in args {
        match arg.as_str() {
            "--prefix-output" => prefix = true,
            "--no-prefix-output" => prefix = false,
            _ => rest.push(arg),
        }
    }
    (prefix, rest)
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

async fn check(args: &[String]) -> Result<(), String> {
    // `--strict`: a service no row declares fails too.
    let strict = args.iter().any(|arg| arg == "--strict");
    let args: Vec<String> = args
        .iter()
        .filter(|arg| *arg != "--strict")
        .cloned()
        .collect();
    let args = args.as_slice();
    let path = config_file(args);
    let config = match (args.first(), path.exists()) {
        (None, false) => project::dev_config(std::path::Path::new("."))?.0,
        _ => config::HostConfig::read(&path)?,
    };
    warn(&config);
    let host = host::Host::start(&config).await?;
    host.runtimes_ready().await?;
    // Only with binaries to check (#668), naming the languages there are.
    if let Some(line) = plugin_api_line(
        config.runtimes.go.is_some(),
        config.runtimes.swift.is_some(),
    ) {
        outln!("{line}");
    }
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
    // The rows that cannot run, each once however many reasons it has;
    // `failed` counts the binaries.
    let mut cannot: std::collections::BTreeSet<&str> =
        report.failing().map(|row| row.id.as_str()).collect();
    let mut failed = 0;
    for row in &report.rows {
        if print_row(&config, &report, row, strict) {
            cannot.insert(row.id.as_str());
        }
    }
    // Errors from 0.18 (warnings in 0.17.2, #676): no row of a cycle
    // starts, and of rows providing one name all but the first to start
    // fail.
    for cycle in &report.cycles {
        errln!("rutis-host: error: {}", cycle_line(cycle));
        cannot.extend(cycle.iter().map(|link| link.row.as_str()));
    }
    for clash in &report.clashes {
        errln!("rutis-host: error: {}", clash_line(clash));
        cannot.extend(clash.rows.iter().skip(1).map(String::as_str));
    }
    failed += cannot.len();
    for binaries in &host.binaries {
        outln!("{} binaries:", binaries.key);
        for binary in binaries.binaries() {
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
                    let api = if manifest.plugin_api > binaries.plugin_api() {
                        "incompatible: upgrade the host"
                    } else if manifest.plugin_api < OLDEST_PLUGIN_API {
                        "incompatible: built with an SDK older than rutis 0.13; rebuild it with the 0.13 SDK or later"
                    } else {
                        "compatible"
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
                    if manifest.plugin_api > binaries.plugin_api()
                        || manifest.plugin_api < OLDEST_PLUGIN_API
                    {
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
        for skipped in binaries.diagnostics() {
            failed += 1;
            outln!("  skipped {skipped}");
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} row(s) or binaries cannot run")),
    }
}

/// The lines `check` prints for `row` of `report` (of `config`), and
/// whether it cannot run only because of `strict` (`--strict`); `add`
/// prints its new rows with it.
fn print_row(
    config: &config::HostConfig,
    report: &CheckReport,
    row: &RowCheck,
    strict: bool,
) -> bool {
    let mut cannot = false;
    // The Cordis rows (the Node runtime's, not leaf plugins): between them a
    // dependency resolves in Cordis.
    let cordis: HashSet<&str> = report
        .rows
        .iter()
        .filter(|row| match &row.status {
            RowStatus::Checked { resolved, .. } => resolved.meta["cordis"] == true,
            _ => false,
        })
        .map(|row| row.id.as_str())
        .collect();
    let (id, name) = (&row.id, &row.name);
    // Extra keys in its config: warnings, errors with `--strict`
    // (printed after the row's lines).
    let extra: Vec<String> = row
        .config_warnings
        .iter()
        .map(|warning| format!("config: {}", warning.message))
        .collect();
    match &row.status {
        RowStatus::Disabled => outln!("{id} ({name}): disabled"),
        // A config that does not match its plugin's schema: one line
        // per problem.
        RowStatus::Failed(error) if error.config().is_some() => {
            for line in error.config().into_iter().flat_map(|config| config.lines()) {
                outln!("{id} ({name}): cannot run: {line}");
            }
        }
        RowStatus::Failed(error) => {
            outln!("{id} ({name}): {error}");
            if let rutis_loader::LoaderError::NotFound { name } = error {
                // A compiled language's row with no runtime declared:
                // its binaries have to be named (#666).
                // (`go:<plugin>`; not a connection row's rows.)
                let remote = config.rows.iter().any(|row| {
                    (row["name"] == rutis_host::connect::DIAL
                        || row["name"] == rutis_host::connect::LISTEN)
                        && row["id"]
                            .as_str()
                            .is_some_and(|id| name.starts_with(&format!("{id}:")))
                });
                let undeclared = languages::LANGUAGES.iter().find(|language| {
                    let key = language.key();
                    let named = name.starts_with(&format!("{key}:"));
                    matches!(key, "go" | "swift")
                        && named
                        && !remote
                        && !language.configured(&config.runtimes)
                });
                match undeclared {
                    Some(language) => outln!(
                        "  declare runtimes.{key} with the binaries that have it \
                         (\"runtimes\": {{ \"{key}\": {{ \"dir\": \"<directory>\" }} }})",
                        key = language.key()
                    ),
                    None => outln!(
                        "  install it where its runtime finds it (a runtime under runtimes), \
                         or correct the row's name"
                    ),
                }
            }
        }
        RowStatus::Checked {
            resolved,
            requires,
            config: checked_config,
            ..
        } => {
            let missing: Vec<&str> = requires
                .iter()
                .filter(|requirement| matches!(requirement.supply, Supply::Missing { .. }))
                .map(|requirement| requirement.service.as_str())
                .collect();
            // Names no row declares, which undeclared services may
            // provide once the rows run: a warning, an error with
            // `--strict`.
            let undeclared: Vec<&Requirement> = requires
                .iter()
                .filter(|requirement| {
                    matches!(
                        requirement.supply,
                        Supply::Announced(_) | Supply::OnlyDisabled { .. }
                    )
                })
                .collect();
            let strictly = strict && (!undeclared.is_empty() || !extra.is_empty());
            match (missing.is_empty(), strictly) {
                (true, false) => outln!("{id} ({name}): ok"),
                (true, true) => {
                    cannot = true;
                    // What no row declares, and what only disabled
                    // rows do.
                    let names = |only_disabled: bool| -> Vec<&str> {
                        undeclared
                            .iter()
                            .filter(|requirement| {
                                matches!(requirement.supply, Supply::OnlyDisabled { .. })
                                    == only_disabled
                            })
                            .map(|requirement| requirement.service.as_str())
                            .collect()
                    };
                    let mut why = Vec::new();
                    if !names(false).is_empty() {
                        why.push(format!("nothing declares {}", names(false).join(", ")));
                    }
                    if !names(true).is_empty() {
                        why.push(format!(
                            "only disabled rows declare {}",
                            names(true).join(", ")
                        ));
                    }
                    if !why.is_empty() {
                        outln!(
                            "{id} ({name}): cannot run with --strict: {}",
                            why.join("; ")
                        );
                    }
                    for line in &extra {
                        outln!("{id} ({name}): cannot run with --strict: {line}");
                    }
                }
                (false, _) => {
                    outln!(
                        "{id} ({name}): cannot run: nothing provides {}",
                        missing.join(", ")
                    );
                    if strict {
                        for line in &extra {
                            outln!("{id} ({name}): cannot run with --strict: {line}");
                        }
                    }
                }
            }
            for requirement in &undeclared {
                let line = match &requirement.supply {
                    Supply::Announced(rows) => {
                        let between_cordis_rows = cordis.contains(id.as_str())
                            && rows.iter().any(|row| cordis.contains(row.as_str()));
                        undeclared_line(id, &requirement.service, rows, between_cordis_rows)
                    }
                    Supply::OnlyDisabled { disabled, .. } => {
                        disabled_line(id, &requirement.service, disabled)
                    }
                    _ => continue,
                };
                errln!(
                    "rutis-host: {}: {line}{}",
                    match strict {
                        true => "error",
                        false => "warning",
                    },
                    did_you_mean(requirement.like.as_deref())
                );
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
            if matches!(checked_config, ConfigStatus::NotYet) {
                outln!("  config not checked: its schema is known only when it runs");
            }
            for requirement in requires {
                if let Some(line) = supply_line(&requirement.service, &requirement.supply) {
                    outln!("  {line}{}", did_you_mean(requirement.like.as_deref()));
                }
            }
        }
        _ => outln!("{id} ({name}): not checked"),
    }
    for line in &extra {
        errln!(
            "rutis-host: {}: {id}: {line}",
            match strict {
                true => "error",
                false => "warning",
            }
        );
    }
    cannot
}

/// `check`'s warning for the row `row` waiting for `service`, which no row
/// declares and `rows` may provide once they run. `between_cordis_rows`:
/// the row and one of those are Cordis rows, where the dependency resolves
/// in Cordis.
fn undeclared_line(row: &str, service: &str, rows: &[String], between_cordis_rows: bool) -> String {
    format!(
        "{row}: nothing declares {service}; it may be provided at runtime by {}{}",
        some_of(rows),
        match between_cordis_rows {
            true => "; between Cordis rows of one runtime it resolves in Cordis",
            false => "",
        }
    )
}

/// `check`'s warning for the row `row` waiting for `service`, which only
/// the disabled rows `disabled` declare (#666).
fn disabled_line(row: &str, service: &str, disabled: &[String]) -> String {
    let rows = match disabled {
        [one] => format!("row {one}, which is disabled: enable it"),
        _ => format!("rows {}, which are disabled: enable one", some_of(disabled)),
    };
    format!("{row}: {service} is declared only by {rows}, or add a row that provides {service}")
}

/// `; did you mean <like>?` for a service name close to one that is
/// declared; nothing without one.
fn did_you_mean(like: Option<&str>) -> String {
    like.map(|like| format!("; did you mean {like}?"))
        .unwrap_or_default()
}

/// `check`'s error for rows waiting for each other's services.
fn cycle_line(cycle: &[CycleLink]) -> String {
    let rows: Vec<&str> = cycle.iter().map(|link| link.row.as_str()).collect();
    let links: Vec<String> = cycle
        .iter()
        .map(|link| {
            format!(
                "{} waits for {} from {}",
                link.row, link.service, link.provider
            )
        })
        .collect();
    format!(
        "rows {} wait for each other ({}): none of them starts; break the cycle in their inject",
        rows.join(", "),
        links.join(", ")
    )
}

/// `check`'s error for several rows declaring one service in one scope.
fn clash_line(clash: &Clash) -> String {
    format!(
        "rows {} all provide {}: only the first to start provides it, the others fail; \
         keep one, or isolate the name in each row",
        clash.rows.join(", "),
        clash.service
    )
}

/// `check`'s line for a service a row waits for: what to do when nothing
/// provides it, what may when that is not known; none when it is provided
/// (or no row declares it: [`undeclared_line`] says so).
fn supply_line(service: &str, supply: &Supply) -> Option<String> {
    match supply {
        Supply::Missing { disabled } | Supply::OnlyDisabled { disabled, .. }
            if !disabled.is_empty() =>
        {
            Some(format!(
                "{service}: only disabled rows provide it ({}): enable one, or add a row that provides it",
                disabled.join(", ")
            ))
        }
        Supply::Missing { .. } => Some(format!(
            "{service}: add a row that provides it"
        )),
        Supply::Provided(by)
            if by
                .iter()
                .all(|provider| matches!(provider, Provider::Marked(_))) =>
        {
            let types: Vec<&str> = by
                .iter()
                .filter_map(|provider| match provider {
                    Provider::Marked(type_name) => Some(type_name.as_str()),
                    _ => None,
                })
                .collect();
            Some(format!(
                "{service}: served by the host's #[rutis::service] type {} once it is provided",
                types.join(", ")
            ))
        }
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

/// The first three of `rows`, and how many more there are.
fn some_of(rows: &[String]) -> String {
    match rows.len() {
        0..=3 => rows.join(", "),
        n => format!("{} and {} more", rows[..3].join(", "), n - 3),
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

/// What `check` says of the plugin API the binaries are checked against:
/// only for the languages the configuration has (#668, #680).
fn plugin_api_line(go: bool, swift: bool) -> Option<String> {
    let languages = match (go, swift) {
        (true, true) => "Go and Swift",
        (true, false) => "Go",
        (false, true) => "Swift",
        (false, false) => return None,
    };
    Some(format!(
        "plugin API for {languages} binaries: {PLUGIN_API} (checked by this host; the Node, Bun and Python runtimes check their own plugins)"
    ))
}

#[cfg(test)]
mod tests {
    use super::{plugin_api_line, runtime_line, take_flag};
    use serde_json::json;

    #[test]
    fn a_flag_is_taken_out() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(
            take_flag(&args(&["--watch-config", "a.json"]), "--watch-config"),
            (true, args(&["a.json"]))
        );
        assert_eq!(
            take_flag(&args(&["a.json"]), "--watch-config"),
            (false, args(&["a.json"]))
        );
    }

    /// `check` names the plugin API only for the binaries' languages the
    /// configuration has (#680).
    #[test]
    fn the_plugin_api_line_names_only_the_languages_there_are() {
        assert_eq!(plugin_api_line(false, false), None);
        let line = |go, swift| plugin_api_line(go, swift).unwrap();
        assert!(line(true, false).starts_with("plugin API for Go binaries: "));
        assert!(line(false, true).starts_with("plugin API for Swift binaries: "));
        assert!(line(true, true).starts_with("plugin API for Go and Swift binaries: "));
    }

    /// guarantee: host/check
    #[test]
    fn check_says_what_runs_each_runtime() {
        let about = json!({
            "implementation": { "name": "@arcships/rutis-bun", "version": "0.19.0" },
            "engine": { "name": "bun", "version": "1.4.3" },
        });
        assert_eq!(
            runtime_line("bun", &about).as_deref(),
            Some("runtime bun: @arcships/rutis-bun 0.19.0, bun 1.4.3")
        );
        let partial = json!({ "implementation": { "name": "rutis" } });
        assert_eq!(
            runtime_line("py", &partial).as_deref(),
            Some("runtime py: rutis")
        );
        assert_eq!(runtime_line("node", &serde_json::Value::Null), None);
    }

    /// The rows that may provide a service at runtime: three named, then a
    /// count (a project of published Cordis packages has many).
    /// guarantee: host/check
    #[test]
    fn check_names_a_few_rows_that_may_provide_a_service() {
        let rows = |n: usize| (0..n).map(|i| format!("r{i}")).collect::<Vec<_>>();
        assert_eq!(super::some_of(&rows(2)), "r0, r1");
        assert_eq!(super::some_of(&rows(3)), "r0, r1, r2");
        assert_eq!(super::some_of(&rows(9)), "r0, r1, r2 and 6 more");
    }

    /// Every command `USAGE` lists has its own `--help`, and only those.
    #[test]
    fn the_commands_are_those_of_the_usage() {
        let listed: Vec<&str> = super::USAGE
            .lines()
            .filter_map(|line| line.strip_prefix("  rutis-host "))
            .filter_map(|entry| entry.split_whitespace().next())
            .filter(|command| !command.starts_with('-'))
            .collect();
        assert_eq!(listed, super::COMMANDS);
        for command in super::COMMANDS {
            assert!(super::command_usage(command)
                .lines()
                .nth(1)
                .is_some_and(|line| line.starts_with(&format!("  rutis-host {command} "))));
        }
    }
}
