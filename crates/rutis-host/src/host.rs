//! A running host: the kernel, the local language runtimes the
//! configuration names, the listeners connection rows use, and the loader
//! that runs the rows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberView, TypeKey};
use rutis::{Ctx, Error as RutisError};
use rutis_bridge::runtime::{RuntimeHandle, RuntimeState};
use rutis_bridge::websocket::{Limits, ListenerConfig, ServerTls, Trust};
use rutis_bridge::{LocalId, PeerId, Session};
use rutis_loader::runtime::{SessionResolver, SessionRows};
use rutis_loader::{
    Builtins, Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog,
};

use crate::config::{HostConfig, Restart};
use crate::errln;
use crate::languages::{BinaryHost, Mount, LANGUAGES};

pub struct Host {
    /// The host runs while this lives.
    root: Ctx,
    pub loader: Loader,
    pub runtimes: Vec<(String, RuntimeHandle)>,
    /// What resolves the rows on sessions, the local runtimes' among them.
    sessions: Arc<SessionResolver>,
    /// The compiled languages' binaries and runtimes (Go, Swift), those
    /// the configuration has.
    pub binaries: Vec<BinaryHost>,
    /// The local runtimes' plugins, by name, to stop one
    /// ([`Host::stop_runtime`]).
    fibers: Vec<(String, FiberView)>,
    /// The processes each local runtime started, by name: to wait for its
    /// own once it is stopped ([`Host::reload`]).
    processes: Vec<(String, rutis_bridge::pipe::Processes)>,
    /// Each configured listener (`listen`) and the address it is bound to,
    /// in the configuration's order: with port 0, the port picked.
    pub listening: Vec<(String, std::net::SocketAddr)>,
    /// The configuration it runs: as started, then as last reloaded
    /// ([`Host::reload`]).
    config: HostConfig,
    /// What each local runtime's `restart` says, by name.
    restarts: Vec<(String, Restart)>,
    /// The file plugins' row edits are written back into.
    rows_file: Option<Arc<crate::rows_file::RowsFile>>,
    /// What each connection row (`rutis-host/dial`, `rutis-host/listen`)
    /// has connected.
    connections: crate::connect::Connections,
    /// The tasks watching the local runtimes ([`Host::supervise`]), by
    /// name.
    supervisors: Supervisors,
}

type Supervisors = Arc<Mutex<Vec<(String, tokio::task::JoinHandle<()>)>>>;

/// The local runtimes whose `restart` is `exit` that ended
/// ([`Host::supervise`]).
pub struct Exits(tokio::sync::mpsc::UnboundedReceiver<(String, String)>);

impl Exits {
    /// The next runtime that ended, and why; never, when none can.
    pub async fn next(&mut self) -> (String, String) {
        match self.0.recv().await {
            Some(ended) => ended,
            None => std::future::pending().await,
        }
    }
}

/// Registers rows compiled into a program ([`Embedding::builtins`]).
pub type RegisterBuiltins = Box<dyn FnOnce(&mut Builtins) + Send>;

/// Given each error the kernel reports ([`Embedding::observe`]).
pub type ObserveErrors = Arc<dyn Fn(&rutis::host::ErrorKind) + Send + Sync>;

/// What a Rust program embedding this host adds to what the configuration
/// names (the E2E fixture host, `tests/e2e/fixture-host`).
#[derive(Default)]
pub struct Embedding {
    /// Rows compiled into the program, next to `rutis-host/dial` and
    /// `rutis-host/listen`: a connection plugin of its own, say (a
    /// [`Session`] on a channel it opens).
    pub builtins: Option<RegisterBuiltins>,
    /// The file whose rows plugins may add and remove through the service
    /// `loader`, written back into it (`run`'s `rutis.json`); without one,
    /// they cannot.
    pub rows_file: Option<Arc<crate::rows_file::RowsFile>>,
    /// Also given every error the kernel reports, before the host prints
    /// it (`rutis-host test` keeps what it reports of each row).
    pub observe: Option<ObserveErrors>,
}

pub type Error = String;

/// How long the runtime processes may take to exit once every cleanup
/// finished, however little of the deadline is left: longer than the
/// local transport gives a process whose channel closed before killing it.
pub const PROCESS_EXIT: Duration = Duration::from_secs(3);

/// How long, once the runtime processes ended, a host waits for the rest
/// of their output before it exits ([`output_written`]). Their output ends
/// with them; only a process one of them started and left running keeps
/// it open, and the host does not wait for that.
pub const OUTPUT_GUARD: Duration = Duration::from_secs(2);

/// Wait (up to [`OUTPUT_GUARD`]) until what the runtime processes wrote
/// has all been passed on: what a program running a [`Host`] does last,
/// so that their last lines are not lost when it exits (#332).
pub fn output_written() {
    crate::output::output_ended(OUTPUT_GUARD);
}

/// How long killed runtime processes may take to be gone.
pub const KILLED_EXIT: Duration = Duration::from_secs(5);

/// Why a shutdown did not finish.
pub enum Unfinished {
    /// The cleanups did not finish: why.
    Cleanups(String),
    /// Every cleanup finished, but the runtime processes did not exit:
    /// why.
    Processes(String),
}

impl std::fmt::Display for Unfinished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unfinished::Cleanups(why) => write!(f, "{why}: cleanups did not finish"),
            Unfinished::Processes(why) => write!(
                f,
                "{why}: every cleanup finished, but the runtime processes did not exit"
            ),
        }
    }
}

impl Host {
    /// Start everything `config` names; the rows are not loaded yet.
    pub async fn start(config: &HostConfig) -> Result<Self, Error> {
        Self::start_with(config, Embedding::default()).await
    }

    /// [`Host::start`], with what an embedding program adds.
    pub async fn start_with(config: &HostConfig, embedding: Embedding) -> Result<Self, Error> {
        config.check_listener_tokens()?;
        // Event failures print as the protocol says (`rutis: listener of
        // <name> in <where> failed: <ErrorName>: <message>`); the rest as
        // the kernel prints them. A row that failed to start and a runtime
        // that went down are on the host's status lines already.
        let fallback = rutis::host::default_sink();
        let connections = crate::connect::Connections::default();
        let far_ends = connections.clone();
        let observe = embedding.observe.clone();
        let sink: rutis::host::ErrorSink = std::sync::Arc::new(move |error| {
            if let Some(observe) = &observe {
                observe(&error);
            }
            // A remote plugin is no row: why it failed (its service taken
            // here, say) is said as it is.
            if let rutis::host::ErrorKind::StartFailed { plugin, error } = &*error {
                if far_ends.is_far_plugin(plugin) {
                    crate::errln!("{error}");
                    return;
                }
            }
            if matches!(
                &*error,
                rutis::host::ErrorKind::StartFailed { .. }
                    | rutis::host::ErrorKind::RuntimeDown { .. }
            ) {
                return;
            }
            match rutis_bridge::events::report_line(&error) {
                Some(line) => eprintln!("{line}"),
                None => fallback(error),
            }
        });
        let root = Ctx::root_with_sink(
            tokio::runtime::Handle::try_current().map_err(|error| error.to_string())?,
            sink,
        );
        // Each row's line says what it waits for (`status`): the kernel's
        // report of a long wait would say it again.
        root.report_waiting_after(None);
        let catalog = ServiceCatalog::new();

        // The listeners and this node's id, for the rows that connect.
        let local = PeerId::new(config.id.clone()).map_err(|error| format!("id: {error}"))?;
        // The id every session greets with, and the `sessions` service.
        root.provide(LocalId(local.clone()))
            .map_err(|error| format!("id: {error}"))?;
        root.plugin(Session::service());
        let (trust, listeners) = websocket(config)?;
        let router = crate::connect::Router::new(listeners.iter().map(|l| l.name.clone()));
        let mut listening = Vec::new();
        let mut bound = Vec::new();
        for listener in &listeners {
            // The listen rows' tokens.
            let admit = router.admit(&listener.name);
            let started = rutis_bridge::websocket::listen(listener, &Limits::default(), admit)?;
            listening.push((listener.name.clone(), started.local_addr()));
            bound.push(started);
        }
        // Stopping, the listeners close; then the close frames of the
        // connections go out.
        root.effect(move || async move {
            drop(bound);
            rutis_bridge::websocket::closed(Duration::from_secs(1)).await;
            Ok(())
        })
        .map_err(|error| format!("the WebSocket listeners: {error}"))?;
        let mut builtins = Builtins::new();
        crate::connect::register(
            &mut builtins,
            &crate::connect::Connect {
                trust,
                router,
                connections: connections.clone(),
            },
        );
        if let Some(register) = embedding.builtins {
            register(&mut builtins);
        }
        let mut mount = Mount {
            root: &root,
            catalog: &catalog,
            chain: Chain::new().with(builtins),
            runtimes: Vec::new(),
            fibers: Vec::new(),
            processes: Vec::new(),
            later: Vec::new(),
            prefix_output: config.runtimes.prefix_output,
            debug: config.runtimes.debug,
        };
        // In reverse: Node's resolver, which takes bare npm names, last.
        for language in LANGUAGES.iter().rev() {
            let key = language.key();
            match language.start(config, &mut mount) {
                // A runtime the file does not declare, which rows enabled
                // with its defaults (#666): as before 0.17.2 its rows fail,
                // and the host runs the others, saying why.
                Err(error)
                    if config
                        .runtimes
                        .implicit
                        .iter()
                        .any(|implicit| implicit == key) =>
                {
                    errln!(
                        "rutis-host: warning: the {key} runtime, which rows use but runtimes does not declare, \
                         cannot run with its defaults: {error}; its rows do not run"
                    );
                }
                started => started?,
            }
        }

        // The rows `<row>:<plugin>` (a connection row's, a local runtime's),
        // and with a Node runtime the names no runtime prefixes. Last: a
        // name another resolver takes stays theirs.
        let node = mount.runtimes.iter().any(|(name, _)| name == "node");
        let sessions = Arc::new(match node {
            true => SessionResolver::npm(mount.catalog, "node"),
            false => SessionResolver::new(mount.catalog),
        });
        for (_, handle) in &mount.runtimes {
            sessions.runtime(handle.clone());
        }
        mount.chain = std::mem::take(&mut mount.chain).with_shared(sessions.clone());
        let Mount {
            chain,
            runtimes,
            fibers,
            processes,
            later,
            ..
        } = mount;

        let plugin = LoaderPlugin::new(
            chain,
            LoaderOptions {
                catalog,
                persist: match &embedding.rows_file {
                    Some(file) => file.clone(),
                    None => Arc::new(rutis_loader::edit::NoPersist),
                },
                ..LoaderOptions::default()
            },
        );
        let loader = plugin.handle();
        sessions.attach(&loader);
        // Extra config keys, in order with what plugins print (config
        // schema design §3.2).
        loader.on_config_warning(std::sync::Arc::new(
            |row: &str, issue: &rutis::ConfigIssue| {
                crate::errln!("{row}: config: {}", issue.message)
            },
        ));
        (&root.plugin(plugin))
            .await
            .map_err(|error| format!("the loader: {error}"))?;
        // The local runtimes' rows are released once each runs.
        for (name, _) in &runtimes {
            sessions.follow_row(name);
        }
        let restarts = LANGUAGES
            .iter()
            .filter(|language| fibers.iter().any(|(name, _)| name == language.key()))
            .map(|language| {
                (
                    language.key().to_owned(),
                    language.restart(&config.runtimes),
                )
            })
            .collect();
        // What waits for the loader: the compiled languages' runtimes, many,
        // coming and going ([`Host::supervise`]).
        let mut binaries = Vec::new();
        for later in later {
            binaries.extend(later(root.clone()).await?);
        }
        Ok(Self {
            root,
            loader,
            runtimes,
            sessions,
            binaries,
            fibers,
            processes,
            listening,
            config: config.clone(),
            restarts,
            rows_file: embedding.rows_file,
            connections,
            supervisors: Supervisors::default(),
        })
    }

    /// Watch each local runtime as its `restart` says, from now on
    /// ([`crate::restart::supervise`]): `on-failure` starts it again when
    /// its process ends on its own; one whose `restart` is `exit` comes out
    /// of the [`Exits`] returned when it ends, for the host to stop. Each
    /// runtime of a compiled language (Go, Swift), which start and stop as
    /// rows use them, is watched on its own, those that start later too,
    /// its backoff kept across its starts.
    pub fn supervise(&self) -> Exits {
        let (send, exits) = tokio::sync::mpsc::unbounded_channel();
        for binaries in &self.binaries {
            let restart = binaries.restart;
            if restart == Restart::No {
                continue;
            }
            let (send, supervisors) = (send.clone(), self.supervisors.clone());
            let (host, tokio) = (binaries.clone(), tokio::runtime::Handle::current());
            binaries.starts.watch(move |name, starts| {
                let (host, runtime) = (host.clone(), name.to_owned());
                let runtime = crate::restart::Compiled::new(starts, move || {
                    let (host, runtime) = (host.clone(), runtime.clone());
                    Box::pin(async move {
                        // A failure is said by the restart itself.
                        let _ = host.restart_runtime(&runtime).await;
                    })
                });
                let (name, send) = (name.to_owned(), send.clone());
                let task = tokio.spawn({
                    let name = name.clone();
                    async move {
                        use crate::restart::{supervise, Backoff, Supervised};
                        let mut backoff = Backoff::default();
                        let exit = loop {
                            match supervise(&name, &runtime, restart, &mut backoff).await {
                                // Its next start is watched again.
                                Supervised::GaveUp => {}
                                Supervised::Exit(why) => break Some(why),
                                Supervised::Closed => break None,
                            }
                        };
                        if let Some(why) = exit {
                            let _ = send.send((name, why));
                        }
                    }
                });
                supervisors.lock().unwrap().push((name, task));
            });
        }
        let mut supervisors = self.supervisors.lock().unwrap();
        for (name, restart) in &self.restarts {
            let fiber = self.fibers.iter().find(|(runtime, _)| runtime == name);
            let handle = self.runtimes.iter().find(|(runtime, _)| runtime == name);
            let (Some((_, fiber)), Some((_, handle)), Restart::OnFailure | Restart::Exit) =
                (fiber, handle, restart)
            else {
                continue;
            };
            let runtime = crate::restart::Local {
                handle: handle.clone(),
                fiber: fiber.clone(),
            };
            let (name, restart, send) = (name.clone(), *restart, send.clone());
            let task = tokio::spawn({
                let name = name.clone();
                async move {
                    let mut backoff = crate::restart::Backoff::default();
                    let supervised =
                        crate::restart::supervise(&name, &runtime, restart, &mut backoff).await;
                    if let crate::restart::Supervised::Exit(why) = supervised {
                        let _ = send.send((name, why));
                    }
                }
            });
            supervisors.push((name, task));
        }
        Exits(exits)
    }

    /// Stop watching the runtime `name`, or every runtime: the host is
    /// about to stop it.
    fn unsupervise(&self, name: Option<&str>) {
        if name.is_none() {
            for binaries in &self.binaries {
                binaries.starts.unwatch();
            }
        }
        self.supervisors.lock().unwrap().retain(|(runtime, task)| {
            let stop = name.is_none_or(|name| name == runtime);
            if stop {
                task.abort();
            }
            !stop
        });
    }

    /// Stop the local runtime `name` (`node`, `py`, `bun`): its plugin
    /// unloads, which closes its channel, and its process ends. Rows it
    /// runs then wait; unload them first.
    pub async fn stop_runtime(&mut self, name: &str) -> Result<(), Error> {
        let index = self
            .fibers
            .iter()
            .position(|(runtime, _)| runtime == name)
            .ok_or_else(|| format!("no {name} runtime runs"))?;
        self.unsupervise(Some(name));
        let (_, fiber) = self.fibers.remove(index);
        self.runtimes.retain(|(runtime, _)| runtime != name);
        self.sessions.forget_runtime(name);
        fiber
            .dispose()
            .await
            .map_err(|error| format!("stopping the {name} runtime: {error}"))
    }

    /// Wait until every runtime runs and its rows may start, or say which
    /// one could not start.
    pub async fn runtimes_ready(&self) -> Result<(), Error> {
        for (name, handle) in &self.runtimes {
            self.runtime_ready(name, handle).await?;
        }
        Ok(())
    }

    /// Whether the local runtime `name` is started again by its `restart`
    /// when it ends: `on-failure`, not given up yet ([`Host::supervise`]).
    pub fn restarts_by_itself(&self, name: &str) -> bool {
        let restarts = self
            .restarts
            .iter()
            .any(|(runtime, restart)| runtime == name && *restart == Restart::OnFailure);
        restarts
            && self
                .supervisors
                .lock()
                .unwrap()
                .iter()
                .any(|(runtime, task)| runtime == name && !task.is_finished())
    }

    /// Start the local runtime `name` (`node`, `py`, `bun`) again, one
    /// whose process ended and that nothing restarts (`restart` is `no`, or
    /// `on-failure` gave up), and wait until it runs and its rows may start
    /// ([`Host::runtimes_ready`]), or say why it did not start. Its rows
    /// come back by themselves, resolved again first: their code as it is
    /// now.
    pub async fn start_runtime_again(&self, name: &str) -> Result<(), Error> {
        let fiber = self
            .fibers
            .iter()
            .find(|(runtime, _)| runtime == name)
            .map(|(_, fiber)| fiber.clone())
            .ok_or_else(|| format!("no {name} runtime runs"))?;
        let handle = self
            .runtimes
            .iter()
            .find(|(runtime, _)| runtime == name)
            .map(|(_, handle)| handle.clone())
            .ok_or_else(|| format!("no {name} runtime runs"))?;
        if let Err(error) = fiber.restart().await {
            // Down again: its state says why in the runtime's words.
            let why = match handle.state() {
                RuntimeState::Down(reason) => reason,
                _ => error.to_string(),
            };
            return Err(format!("the {name} runtime did not start: {why}"));
        }
        self.runtime_ready(name, &handle).await
    }

    /// Wait until the runtime `name` runs and its rows may start.
    async fn runtime_ready(&self, name: &str, handle: &RuntimeHandle) -> Result<(), Error> {
        let state = handle
            .wait_for(|state| matches!(state, RuntimeState::Ready(_) | RuntimeState::Down(_)))
            .await;
        if let RuntimeState::Down(reason) = state {
            return Err(format!("the {name} runtime did not start: {reason}"));
        }
        // A running runtime releases its rows ([`SessionRows`]) once it
        // has resolved again those resolved before it ran. Rows loaded
        // before then wait for it, and the host would say it runs while
        // they do (#314). A plugin injecting it applies once it is there.
        let (released, rows) = tokio::sync::oneshot::channel();
        // Disposed however the wait ends, a signal dropping it included:
        // left mounted, it would count among what still runs.
        let mut waiter = Unmount(Some(self.root.plugin(Released {
            injects: [SessionRows::key(name).erase()],
            released: std::sync::Mutex::new(Some(released)),
        })));
        let down = async {
            match handle
                .wait_for(|state| matches!(state, RuntimeState::Down(_)))
                .await
            {
                RuntimeState::Down(reason) => reason,
                _ => unreachable!("only a down runtime is waited for"),
            }
        };
        let result = tokio::select! {
            released = rows => released.map_err(|_| {
                // Only a plugin that never applied drops its sender.
                format!("the {name} runtime did not start: its rows were never released")
            }),
            reason = down => Err(format!("the {name} runtime did not start: {reason}")),
        };
        if let Some(waiter) = waiter.0.take() {
            let _ = waiter.dispose().await;
        }
        result
    }

    /// Load `rows` while the runtimes start, not after: a runtime that
    /// takes long to start (one waiting for its debugger) holds up only its
    /// own rows, which wait for it; the others start as soon as they can.
    /// Done, as before, once every runtime runs ([`Host::runtimes_ready`])
    /// and every row has been through its load: the rows are loaded again
    /// then, which changes nothing but waits for the rows the runtimes
    /// released. An error says which runtime could not start, or why the
    /// rows could not load.
    pub async fn start_rows(&self, rows: Vec<serde_json::Value>) -> Result<(), Error> {
        let (ready, loaded) = tokio::join!(self.runtimes_ready(), self.load(rows.clone()));
        ready?;
        loaded?;
        self.load(rows).await
    }

    /// Load `rows` (replacing what was loaded before): those of the rows
    /// file as last read, when the host has one.
    pub async fn load(&self, rows: Vec<serde_json::Value>) -> Result<(), Error> {
        self.loader
            .reconcile(vec![layer(rows)?], self.editable())
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// The layer plugins edit, at the rows file's content last read.
    fn editable(&self) -> Option<rutis_loader::edit::Editable> {
        self.rows_file.as_ref().map(|file| file.editable())
    }

    /// Run `new` instead of the configuration this host runs, all or
    /// nothing (`docs/guide/rutis-host.md`, "Changing the configuration
    /// while it runs"):
    ///
    /// - What a running host cannot change refuses it, saying to restart
    ///   the host: `format`, `id`, `listen`, and a local
    ///   runtime added or changed (`runtimes.go` and `runtimes.swift` in any
    ///   way).
    /// - The new rows are checked first ([`Loader::check`]): a row that
    ///   would fail, and does not fail now, refuses it.
    /// - Then they replace the old ones in one reconcile; a row failing
    ///   after it that did not before brings the old rows back, and refuses
    ///   it.
    /// - A local runtime `new` no longer has is stopped once its rows are
    ///   gone, and its process has ended when this returns.
    ///
    /// Refused, nothing changed (rows brought back are restarted), and the
    /// error says why. Tokens are read when connection rows dial or take a
    /// connection, so a row `new` adds has its token.
    pub async fn reload(&mut self, new: HostConfig) -> Result<Reloaded, Error> {
        restart_needed(&self.config, &new)?;
        new.check_listener_tokens()?;
        let removed: Vec<&'static str> = LANGUAGES
            .iter()
            .filter(|language| {
                language.settings(&self.config.runtimes).is_some()
                    && language.settings(&new.runtimes).is_none()
            })
            .map(|language| language.key())
            .collect();

        // Rows that would fail, unless they fail already.
        let failing_now: std::collections::HashSet<String> = self
            .loader
            .entries()
            .into_iter()
            .filter(|entry| {
                entry.rejected.is_some()
                    || matches!(
                        &entry.status,
                        rutis_loader::report::EntryStatus::Unresolved(_)
                    )
                    || matches!(&entry.status, rutis_loader::report::EntryStatus::Running(snapshot)
                        if snapshot.state == rutis::host::FiberState::Failed)
            })
            .map(|entry| entry.id)
            .collect();
        let checked = self.loader.check(&[layer(new.rows())?]).await;
        let failing: Vec<(String, String)> = checked
            .rows
            .iter()
            .filter(|row| !failing_now.contains(&row.id))
            .filter_map(|row| match &row.status {
                rutis_loader::report::RowStatus::Failed(error) => {
                    Some((row.id.clone(), error.to_string()))
                }
                _ => None,
            })
            .collect();
        if !failing.is_empty() {
            return Err(rejection(&failing));
        }

        // The rows as they run: those plugins added or removed too.
        let (layers, editable) = (self.loader.layers(), self.loader.editable());
        let running = rutis_loader::patch::apply_patches(&layers).rows;
        let old = std::mem::replace(&mut self.config, new);
        let applied = self
            .loader
            .reconcile(vec![layer(self.config.rows())?], self.editable())
            .await
            .map_err(|error| error.to_string());
        let failed = match applied {
            Ok(report) if report.new_failures.is_empty() => None,
            Ok(report) => Some(rejection(
                &report
                    .new_failures
                    .iter()
                    .map(|failure| (failure.id.clone(), failure.error.clone()))
                    .collect::<Vec<_>>(),
            )),
            Err(error) => Some(error),
        };
        if let Some(why) = failed {
            // The old rows again: what failed was new.
            self.config = old;
            self.loader
                .reconcile(layers, editable)
                .await
                .map_err(|error| format!("{why}; and the old rows did not load again: {error}"))?;
            return Err(why);
        }

        let mut reloaded = Reloaded {
            rows: changed_rows(&running, &self.config.rows()),
            stopped: Vec::new(),
            warnings: Vec::new(),
        };
        for name in removed {
            match self.stop_runtime_process(name).await {
                Ok(()) => reloaded.stopped.push(name.to_owned()),
                Err(error) => reloaded.warnings.push(error),
            }
        }
        Ok(reloaded)
    }

    /// [`Host::stop_runtime`], then wait (up to [`RUNTIME_EXIT`]) until its
    /// process has ended: its own, whichever generation started it (one a
    /// restart started meanwhile too), not whichever ends first. A runtime
    /// that is down has none left, and returns at once.
    async fn stop_runtime_process(&mut self, name: &str) -> Result<(), Error> {
        let processes = self
            .processes
            .iter()
            .position(|(runtime, _)| runtime == name)
            .map(|index| self.processes.remove(index).1);
        self.stop_runtime(name).await?;
        let Some(processes) = processes else {
            return Ok(());
        };
        let ended = async {
            while processes.running() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(RUNTIME_EXIT, ended)
            .await
            .map_err(|_| {
                format!(
                    "the {name} runtime's process did not end within {}s",
                    RUNTIME_EXIT.as_secs()
                )
            })
    }

    /// The configuration this host runs.
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Check `rows` without running them ([`rutis_loader::Loader::check`]).
    pub async fn check(
        &self,
        rows: Vec<serde_json::Value>,
    ) -> Result<rutis_loader::report::CheckReport, Error> {
        let patches: Vec<Patch> = serde_json::from_value(serde_json::json!([{ "insert": rows }]))
            .map_err(|error| format!("rows: {error}"))?;
        Ok(self
            .loader
            .check(&[Layer::new("rutis.json", patches)])
            .await)
    }

    /// Shut the root down: every row and runtime unloads, each cleanup
    /// running once. Waits up to `deadline` for the cleanups; after it,
    /// the shutdown goes on and the error says so.
    pub async fn shut_down(&self, deadline: Duration) -> Result<(), Unfinished> {
        // A runtime ending now is not restarted, nor does it end the host.
        self.unsupervise(None);
        self.root
            .shutdown_with_timeout(deadline)
            .await
            .map_err(|error| {
                Unfinished::Cleanups(match error {
                    rutis::host::DisposeWaitError::TimedOut { .. } => {
                        format!("{}s passed", deadline.as_secs_f64())
                    }
                    rutis::host::DisposeWaitError::Failed(error) => {
                        format!("the shutdown failed: {error}")
                    }
                    error => error.to_string(),
                })
            })
    }

    /// Once the root has shut down, wait up to `time` for the runtime
    /// processes to end: their channels closed, so they end by themselves
    /// (or are killed after a grace period of their own).
    pub async fn processes_exit(&self, time: Duration) -> Result<(), Unfinished> {
        tokio::time::timeout(time, rutis_bridge::pipe::processes_ended())
            .await
            .map_err(|_| Unfinished::Processes(format!("{}s passed", time.as_secs_f64())))
    }

    /// End the runtime processes now, without their cleanups, and wait
    /// (briefly) until they are gone.
    pub async fn kill_processes(&self) {
        rutis_bridge::pipe::kill_processes();
        let gone = rutis_bridge::pipe::processes_ended();
        if tokio::time::timeout(KILLED_EXIT, gone).await.is_err() {
            let left = rutis_bridge::pipe::running_processes();
            errln!("rutis-host: processes {left:?} did not end when killed");
        }
    }

    /// For each row, the other rows whose services it uses now, as rutis
    /// resolved them: a service its fiber (or a fiber under it) depends on,
    /// provided by another row's fiber (or one under it). A dependency
    /// resolved inside a runtime only (between Cordis rows), or one not
    /// resolved now, is not known here.
    pub fn row_dependencies(
        &self,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        use std::collections::{BTreeMap, BTreeSet, HashMap};
        let rows: HashMap<rutis::host::PluginId, String> = self
            .loader
            .entries()
            .into_iter()
            .filter_map(|entry| Some((entry.plugin?, entry.id)))
            .collect();
        let plugins = self.root.diagnostics().plugins;
        let parents: HashMap<rutis::host::PluginId, Option<rutis::host::PluginId>> = plugins
            .iter()
            .map(|plugin| (plugin.id, plugin.parent))
            .collect();
        // The row a fiber belongs to: its own, or the nearest above it.
        let row_of = |mut id: rutis::host::PluginId| -> Option<&String> {
            for _ in 0..parents.len() {
                if let Some(row) = rows.get(&id) {
                    return Some(row);
                }
                id = (*parents.get(&id)?)?;
            }
            None
        };
        let mut dependencies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for plugin in &plugins {
            let Some(row) = row_of(plugin.id) else {
                continue;
            };
            for dependency in &plugin.resolved_dependencies {
                if let Some(provider) = row_of(dependency.provider).filter(|other| *other != row) {
                    dependencies
                        .entry(row.clone())
                        .or_default()
                        .insert(provider.clone());
                }
            }
        }
        dependencies
    }

    /// What has not stopped yet: the rows (and runtimes) stopping, or
    /// still starting, and how many other rows wait for them to stop
    /// first.
    pub fn still_running(&self) -> (Vec<String>, usize) {
        use rutis::host::FiberState;
        let root = self.root.root_view().map(|root| root.id);
        let mut stopping = Vec::new();
        let mut waiting = 0;
        let mut rows = std::collections::HashSet::new();
        for entry in self.loader.entries() {
            rows.extend(entry.plugin);
            let rutis_loader::report::EntryStatus::Running(snapshot) = &entry.status else {
                continue;
            };
            match snapshot.state {
                FiberState::Disposed | FiberState::Failed => {}
                FiberState::Unloading => stopping.push(entry.id),
                FiberState::Loading => stopping.push(format!("{} (still starting)", entry.id)),
                _ => waiting += 1,
            }
        }
        // What is not a row (a runtime, a session) and still stops is named
        // too; the rest of rutis's own plugins are not counted.
        for plugin in self.root.diagnostics().plugins {
            if Some(plugin.id) == root || rows.contains(&plugin.id) {
                continue;
            }
            match plugin.state {
                FiberState::Unloading => stopping.push(plugin.name),
                FiberState::Loading => stopping.push(format!("{} (still starting)", plugin.name)),
                _ => {}
            }
        }
        (stopping, waiting)
    }

    /// The host's root, as Rust embedding has it: what `rutis-host call`
    /// calls from.
    pub fn root(&self) -> Ctx {
        self.root.clone()
    }

    /// The service `name` as rutis sees it (what a row provides).
    #[allow(dead_code)]
    pub fn service(&self, name: &str) -> Option<Arc<dyn rutis_bridge::session::HostDispatch>> {
        self.root.get_at::<dyn rutis_bridge::session::HostDispatch>(
            &rutis_bridge::session::host_key(name),
        )
    }

    /// The binaries and runtimes of the compiled language `key` (`go`,
    /// `swift`), when the configuration has it.
    pub fn binaries_of(&self, key: &str) -> Option<&BinaryHost> {
        self.binaries.iter().find(|binaries| binaries.key == key)
    }

    /// What the status lines follow ([`crate::status::follow`]): the rows
    /// and the runtimes.
    pub fn followed(&self) -> crate::status::Followed {
        crate::status::Followed {
            loader: self.loader.clone(),
            runtimes: self.runtimes.clone(),
            binaries: self.binaries.clone(),
        }
    }

    /// What `rutis-host status` answers: the loader's status, then what
    /// each connection row has connected.
    pub fn status_report(&self) -> crate::control::Report {
        self.status_report_with(|_| {})
    }

    /// [`Host::status_report`] with the loader's status as `adjust` leaves
    /// it (`dev`: a row whose reload failed runs its previous version).
    pub fn status_report_with(
        &self,
        adjust: impl Fn(&mut rutis_loader::status::Status) + Send + Sync + 'static,
    ) -> crate::control::Report {
        let (loader, connections) = (self.loader.clone(), self.connections.clone());
        Arc::new(move || {
            let mut status = loader.status();
            adjust(&mut status);
            format!(
                "{status}{}",
                crate::status::connections(&connections.rows())
            )
        })
    }

    /// What each connection row has connected now.
    pub fn connections(&self) -> Vec<crate::connect::Row> {
        self.connections.rows()
    }
}

/// Who a dialing row trusts (`RUTIS_CA`, besides the system's roots), and
/// the listeners of `listen`.
fn websocket(config: &HostConfig) -> Result<(Trust, Vec<ListenerConfig>), Error> {
    // A PEM file, and where it was named (a field, or an environment
    // variable).
    let read = |path: &Path, named: &str, pem: Pem| {
        let bytes =
            std::fs::read(path).map_err(|error| format!("{named} {}: {error}", path.display()))?;
        pem.check(&bytes)
            .map_err(|error| format!("{named} {}: {error}", path.display()))?;
        Ok::<_, Error>(bytes)
    };
    let env_path = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let mut trust = Trust::default();
    if let Some(ca) = env_path("RUTIS_CA") {
        trust = Trust {
            system: true,
            ca_pem: vec![read(&ca, "RUTIS_CA", Pem::Certificate)?],
        };
    }
    let mut listeners: Vec<ListenerConfig> = Vec::new();
    for listener in &config.listen {
        if listeners.iter().any(|known| known.name == listener.name) {
            return Err(format!("listener {} is configured twice", listener.name));
        }
        let mut listening = ListenerConfig::new(&listener.name, listener.address);
        let name = &listener.name;
        let from = |field: &Option<PathBuf>, field_name: &str, variable: &str| match (
            field,
            env_path(variable),
        ) {
            (Some(path), _) => Some((path.clone(), format!("listener {name}: {field_name}"))),
            (None, Some(path)) => Some((path, format!("listener {name}: {variable}"))),
            (None, None) => None,
        };
        let cert = from(&listener.cert, "cert", "RUTIS_CERT");
        let key = from(&listener.key, "key", "RUTIS_KEY");
        match (cert, key) {
            (Some((cert, cert_named)), Some((key, key_named))) => {
                listening = listening.tls(ServerTls {
                    certificate_pem: read(&cert, &cert_named, Pem::Certificate)?,
                    key_pem: read(&key, &key_named, Pem::Key)?,
                    client_ca_pem: None,
                });
            }
            (None, None) => {}
            // Not a listener without TLS: one half was meant to enable it.
            (Some((_, named)), None) | (None, Some((_, named))) => {
                return Err(format!(
                    "{named} is set alone: TLS needs a certificate and its key, \
                     set both cert and key (or RUTIS_CERT and RUTIS_KEY)"
                ))
            }
        }
        listeners.push(listening);
    }
    Ok((trust, listeners))
}

/// What a PEM file holds.
#[derive(Clone, Copy)]
enum Pem {
    Certificate,
    Key,
}

impl Pem {
    fn check(self, bytes: &[u8]) -> Result<(), String> {
        use rustls_pki_types::pem::PemObject;
        use rustls_pki_types::{CertificateDer, PrivateKeyDer};
        let found = match self {
            Pem::Certificate => CertificateDer::pem_slice_iter(bytes)
                .next()
                .is_some_and(|first| first.is_ok()),
            Pem::Key => PrivateKeyDer::from_pem_slice(bytes).is_ok(),
        };
        match (found, self) {
            (true, _) => Ok(()),
            (false, Pem::Certificate) => {
                Err("no certificate in it: give a PEM file (-----BEGIN CERTIFICATE-----)".into())
            }
            (false, Pem::Key) => {
                Err("no private key in it: give a PEM file (-----BEGIN PRIVATE KEY-----)".into())
            }
        }
    }
}

/// The rows of `rows` as the one layer the host runs.
fn layer(rows: Vec<serde_json::Value>) -> Result<Layer, Error> {
    let patches: Vec<Patch> = serde_json::from_value(serde_json::json!([{ "insert": rows }]))
        .map_err(|error| format!("rows: {error}"))?;
    Ok(Layer::new("rutis.json", patches))
}

/// How long a runtime a reload removed may take to end once stopped:
/// longer than the grace the local transport gives it before killing it.
pub const RUNTIME_EXIT: Duration = Duration::from_secs(15);

/// What [`Host::reload`] changed.
#[derive(Debug, Default)]
pub struct Reloaded {
    /// How many rows were added, removed or changed (a group's rows each
    /// count, the group not).
    pub rows: usize,
    /// The local runtimes stopped: the configuration no longer has them.
    pub stopped: Vec<String>,
    /// What went wrong once the rows had changed (a runtime that did not
    /// stop): the reload stands.
    pub warnings: Vec<String>,
}

impl std::fmt::Display for Reloaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.rows {
            1 => write!(f, "1 row changed")?,
            rows => write!(f, "{rows} rows changed")?,
        }
        for runtime in &self.stopped {
            write!(f, ", runtime {runtime} stopped")?;
        }
        Ok(())
    }
}

/// Why `new` needs the host restarted to run instead of `old`: what a
/// running host does not change.
pub fn restart_needed(old: &HostConfig, new: &HostConfig) -> Result<(), Error> {
    let restart = |what: String| Err(format!("{what}: restart the host"));
    if new.format != old.format {
        return restart(format!("format {} is now {}", old.format, new.format));
    }
    if new.id != old.id {
        return restart(format!("id {:?} is now {:?}", old.id, new.id));
    }
    if format!("{:?}", new.listen) != format!("{:?}", old.listen) {
        return restart("listen changed".into());
    }
    for language in LANGUAGES {
        let key = language.key();
        match (
            language.settings(&old.runtimes),
            language.settings(&new.runtimes),
        ) {
            (None, Some(_)) => return restart(format!("runtimes.{key} added")),
            (Some(_), None) if !language.stops_when_removed() => {
                return restart(format!("runtimes.{key} removed"))
            }
            (Some(before), Some(now)) if before != now => {
                return restart(format!("runtimes.{key} changed"))
            }
            _ => {}
        }
    }
    Ok(())
}

/// How many rows differ between `old` and `new`, by id: added, removed or
/// changed. A group's rows are counted, the group itself is not.
fn changed_rows(old: &[serde_json::Value], new: &[serde_json::Value]) -> usize {
    row_changes(old, new).len()
}

/// How a row differs between two configurations ([`row_changes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowChange {
    Added,
    Removed,
    /// Any of its fields (its config, its name, `disabled`, ...).
    Changed,
}

/// The rows that differ between `old` and `new`, by id, in the order of
/// `old` then the rows `new` adds: a group's rows each, the group itself
/// not.
pub fn row_changes(
    old: &[serde_json::Value],
    new: &[serde_json::Value],
) -> Vec<(String, RowChange)> {
    fn flat(rows: &[serde_json::Value], into: &mut Vec<(String, serde_json::Value)>) {
        for row in rows {
            let id = row["id"].as_str().unwrap_or_default().to_owned();
            if row["group"] == true {
                let mut own = row.clone();
                own["config"] = serde_json::Value::Null;
                into.push((id, own));
                if let Some(children) = row["config"].as_array() {
                    flat(children, into);
                }
            } else {
                into.push((id, row.clone()));
            }
        }
    }
    let (mut before, mut after) = (Vec::new(), Vec::new());
    flat(old, &mut before);
    flat(new, &mut after);
    let find = |rows: &[(String, serde_json::Value)], id: &str| {
        rows.iter()
            .find(|(row, _)| row == id)
            .map(|(_, value)| value.clone())
    };
    let group =
        |row: &Option<serde_json::Value>| row.as_ref().is_some_and(|row| row["group"] == true);
    let mut ids: Vec<&String> = before.iter().map(|(id, _)| id).collect();
    ids.extend(
        after
            .iter()
            .map(|(id, _)| id)
            .filter(|id| !before.iter().any(|(row, _)| row == *id)),
    );
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| {
            let (was, is) = (find(&before, id), find(&after, id));
            if group(&was) || group(&is) {
                return None;
            }
            let change = match (&was, &is) {
                (None, Some(_)) => RowChange::Added,
                (Some(_), None) => RowChange::Removed,
                (Some(was), Some(is)) if was != is => RowChange::Changed,
                _ => return None,
            };
            Some((id.clone(), change))
        })
        .collect()
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Says once a runtime's rows may start: it injects what they wait for.
/// Temporary: [`Host::runtimes_ready`] mounts it while it waits, and
/// disposes it once the wait ends.
struct Released {
    injects: [TypeKey; 1],
    released: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// Disposes the plugin it holds when dropped: the dispose starts at once,
/// nothing waits for it.
struct Unmount(Option<FiberView>);

impl Drop for Unmount {
    fn drop(&mut self) {
        if let Some(view) = self.0.take() {
            drop(view.dispose());
        }
    }
}

impl DynPlugin for Released {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "runtime-rows-released"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        if let Some(released) = self.released.lock().unwrap().take() {
            let _ = released.send(());
        }
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BunRuntime, NodeRuntime, PythonRuntime, Runtimes};
    use rutis_bridge::session::{settle, Value};
    use rutis_test_support::{within, Guard, HANG_GUARD};
    use serde_json::json;
    use std::time::Duration;

    /// A row's service appears once its runtime (Node, Python, Bun, a Go
    /// binary) has started cold and loaded it: on a busy CI machine that took
    /// longer than HANG_GUARD (#238). Twice it, as before; only against a
    /// hang.
    const RUNTIME_START: Guard = Guard(Duration::from_secs(2 * HANG_GUARD.as_secs()));

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    async fn greeting(host: &Host) -> String {
        let service = RUNTIME_START
            .eventually(|| host.service("greeter"), "the greeter")
            .await;
        let reply = service
            .invoke("hello", Value::List(vec![json!("Ada").into()]))
            .unwrap();
        settle(reply)
            .await
            .unwrap()
            .json()
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Reloading a row runs the plugin's edited code, in Node and Python.
    /// guarantee: host/dev-reload
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reloaded_row_runs_the_edited_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let sdk = repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "name": "try", "type": "module" }"#,
        )
        .unwrap();
        let node_plugin = |word: &str| {
            format!(
                "import {{ definePlugin }} from '{}'\n\
                 export default definePlugin({{ provides: {{ greeter: {{ hello: 'sync' }} }}, apply(ctx) {{\n\
                 ctx.provide('greeter', {{ hello: name => '{word}, ' + name }}) }} }})\n",
                crate::config::file_url(&sdk)
            )
        };
        let entry = dir.path().join("greeter.mjs");
        std::fs::write(&entry, node_plugin("Hello")).unwrap();
        let config = HostConfig {
            format: crate::config::FORMAT,
            id: "test".into(),
            runtimes: Runtimes {
                node: Some(NodeRuntime {
                    restart: crate::config::Restart::No,
                    project: dir.path().to_owned(),
                    runtime: Some(repo().join("node/rutis-runtime")),
                }),
                ..Runtimes::default()
            },
            listen: Vec::new(),
            installed: Default::default(),
            rows: vec![json!({ "id": "greeter", "name": crate::config::file_url(&entry) })],
            warnings: Vec::new(),
            unknown_row_fields: Vec::new(),
        };
        let host = Host::start(&config).await.unwrap();
        host.runtimes_ready().await.unwrap();
        host.load(config.rows()).await.unwrap();
        assert_eq!(greeting(&host).await, "Hello, Ada");
        // A different size, so the edit shows even within the same instant.
        std::fs::write(&entry, node_plugin("Welcome")).unwrap();
        host.loader.reload("greeter").await.unwrap();
        assert_eq!(greeting(&host).await, "Welcome, Ada");
    }

    /// guarantee: host/dev-reload
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reloaded_python_row_runs_the_edited_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let python_plugin = |word: &str| {
            format!(
                "provides = {{'greeter': {{'hello': 'sync'}}}}\n\
                 class Greeter:\n    def hello(self, name):\n        return '{word}, ' + name\n\
                 def apply(ctx, config):\n    ctx.provide('greeter', Greeter())\n"
            )
        };
        std::fs::write(dir.path().join("greeter.py"), python_plugin("Hello")).unwrap();
        // The repository's rutis package, for an interpreter without it.
        std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
        let config = HostConfig {
            format: crate::config::FORMAT,
            id: "test".into(),
            runtimes: Runtimes {
                py: Some(PythonRuntime {
                    restart: crate::config::Restart::No,
                    project: dir.path().to_owned(),
                    python: std::env::var_os("RUTIS_PYTHON").map(PathBuf::from),
                }),
                ..Runtimes::default()
            },
            listen: Vec::new(),
            installed: Default::default(),
            rows: vec![json!({ "id": "greeter", "name": "py:greeter" })],
            warnings: Vec::new(),
            unknown_row_fields: Vec::new(),
        };
        let host = Host::start(&config).await.unwrap();
        host.runtimes_ready().await.unwrap();
        host.load(config.rows()).await.unwrap();
        assert_eq!(greeting(&host).await, "Hello, Ada");
        std::fs::write(dir.path().join("greeter.py"), python_plugin("Welcome")).unwrap();
        host.loader.reload("greeter").await.unwrap();
        assert_eq!(greeting(&host).await, "Welcome, Ada");
    }

    /// Once `runtimes_ready` returns, the rows of each runtime may start:
    /// what they wait for ([`SessionRows`]) is there, so a row
    /// loaded then runs once loading returns. It used to return once the
    /// runtime process was ready, and such a row could still wait: `run`
    /// said it was running, and printed its state, while it did (#314).
    #[tokio::test(flavor = "multi_thread")]
    async fn rows_loaded_once_the_runtimes_are_ready_do_not_wait_for_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plain.py"),
            "def apply(ctx, config):\n    pass\n",
        )
        .unwrap();
        // The repository's rutis package, for an interpreter without it.
        std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
        let config = HostConfig {
            format: crate::config::FORMAT,
            id: "test".into(),
            runtimes: Runtimes {
                py: Some(PythonRuntime {
                    restart: crate::config::Restart::No,
                    project: dir.path().to_owned(),
                    python: std::env::var_os("RUTIS_PYTHON").map(PathBuf::from),
                }),
                ..Runtimes::default()
            },
            listen: Vec::new(),
            installed: Default::default(),
            rows: vec![json!({ "id": "plain", "name": "py:plain" })],
            warnings: Vec::new(),
            unknown_row_fields: Vec::new(),
        };
        let host = Host::start(&config).await.unwrap();
        RUNTIME_START
            .within("the Python runtime", host.runtimes_ready())
            .await
            .unwrap();
        // What the runtime's rows wait for.
        let rows = host.root.get_at(&SessionRows::key("py"));
        assert!(rows.is_some(), "the runtime is ready, its rows not yet");
        within("loading the row", host.load(config.rows()))
            .await
            .unwrap();
        let entry = host
            .loader
            .entries()
            .into_iter()
            .find(|entry| entry.id == "plain")
            .unwrap();
        assert_eq!(crate::status::describe(&entry.status), "running");
    }

    /// Bun on Windows is not tested yet.
    /// guarantee: host/dev-reload
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reloaded_bun_row_runs_the_edited_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let bun_plugin = |word: &str| {
            format!(
                "export const provides = {{ greeter: {{ hello: 'sync' }} }}\n\
                 export function apply(ctx) {{ ctx.provide('greeter', {{ hello: name => '{word}, ' + name }}) }}\n"
            )
        };
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("greeter.ts"), bun_plugin("Hello")).unwrap();
        let config = HostConfig {
            format: crate::config::FORMAT,
            id: "test".into(),
            runtimes: Runtimes {
                bun: Some(BunRuntime {
                    restart: crate::config::Restart::No,
                    project: dir.path().to_owned(),
                    runtime: Some(repo().join("bun/rutis-bun")),
                    program: None,
                }),
                ..Runtimes::default()
            },
            listen: Vec::new(),
            installed: Default::default(),
            rows: vec![json!({ "id": "greeter", "name": "bun:./greeter.ts" })],
            warnings: Vec::new(),
            unknown_row_fields: Vec::new(),
        };
        let host = Host::start(&config).await.unwrap();
        host.runtimes_ready().await.unwrap();
        host.load(config.rows()).await.unwrap();
        assert_eq!(greeting(&host).await, "Hello, Ada");
        std::fs::write(dir.path().join("greeter.ts"), bun_plugin("Welcome")).unwrap();
        host.loader.reload("greeter").await.unwrap();
        assert_eq!(greeting(&host).await, "Welcome, Ada");
    }

    fn parsed(value: serde_json::Value) -> HostConfig {
        serde_json::from_value(value).unwrap()
    }

    /// What a running host cannot change is refused, saying to restart it;
    /// rows, and a local runtime removed, are not.
    /// guarantee: host/reload-config
    #[test]
    fn what_needs_a_restart_is_refused() {
        let base = json!({
            "id": "main",
            "listen": [{ "name": "public", "address": "127.0.0.1:7443" }],
            "runtimes": {
                "node": {}, "py": {}, "go": { "dir": "plugins" }
            },
            "rows": [{ "id": "a", "name": "py:a" }]
        });
        let old = parsed(base.clone());
        let changed = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut new = base.clone();
            change(&mut new);
            restart_needed(&old, &parsed(new)).err()
        };
        let refused = [
            (
                changed(&|c| c["id"] = json!("other")),
                "id \"main\" is now \"other\"",
            ),
            (
                changed(&|c| c["listen"][0]["address"] = json!("127.0.0.1:7444")),
                "listen changed",
            ),
            (
                changed(&|c| c["runtimes"]["go"]["idle"] = json!(30)),
                "runtimes.go changed",
            ),
            (
                changed(&|c| {
                    c["runtimes"].as_object_mut().unwrap().remove("go");
                }),
                "runtimes.go removed",
            ),
            (
                changed(&|c| c["runtimes"]["node"]["project"] = json!("other")),
                "runtimes.node changed",
            ),
            (
                changed(&|c| c["runtimes"]["bun"] = json!({})),
                "runtimes.bun added",
            ),
        ];
        for (error, why) in refused {
            assert_eq!(error, Some(format!("{why}: restart the host")));
        }
        assert_eq!(
            changed(&|c| c["rows"] = json!([{ "id": "b", "name": "py:b" }])),
            None
        );
        assert_eq!(
            changed(&|c| {
                c["runtimes"].as_object_mut().unwrap().remove("py");
            }),
            None
        );
    }

    /// Rows are told apart by id: added, removed or changed, a group's rows
    /// each, the group not.
    #[test]
    fn changed_rows_are_counted_by_id() {
        let old = [
            json!({ "id": "a", "name": "py:a" }),
            json!({ "id": "b", "name": "py:b", "config": { "x": 1 } }),
            json!({ "id": "g", "group": true, "config": [{ "id": "c", "name": "py:c" }] }),
        ];
        assert_eq!(changed_rows(&old, &old), 0);
        let new = [
            json!({ "id": "b", "name": "py:b", "config": { "x": 2 } }),
            json!({ "id": "g", "group": true, "config": [{ "id": "c", "name": "py:c" }, { "id": "d", "name": "py:d" }] }),
            json!({ "id": "e", "name": "py:e" }),
        ];
        // a removed, b changed, d and e added.
        assert_eq!(changed_rows(&old, &new), 4);
        let reloaded = |rows| Reloaded {
            rows,
            stopped: vec!["py".into()],
            warnings: Vec::new(),
        };
        assert_eq!(reloaded(1).to_string(), "1 row changed, runtime py stopped");
        assert_eq!(
            reloaded(0).to_string(),
            "0 rows changed, runtime py stopped"
        );
    }

    /// Go binaries in a directory: a row names a plugin, and its binary's
    /// runtime starts when the row is loaded.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_go_row_runs_in_its_binarys_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(plugins.join(if cfg!(windows) {
                "netkit.exe"
            } else {
                "netkit"
            }))
            .arg("./cmd/netkit")
            .current_dir(repo().join("crates/rutis-loader/tests/fixtures/go"))
            .status()
            .expect("go is on PATH");
        assert!(status.success());
        let mut config: HostConfig = serde_json::from_value(json!({
            "id": "go-test",
            "runtimes": { "go": { "dir": "plugins" } },
            "rows": [{ "id": "ping", "name": "go:ping" }]
        }))
        .unwrap();
        config.rebase(dir.path());
        let host = Host::start(&config).await.unwrap();
        host.load(config.rows()).await.unwrap();
        let service = RUNTIME_START
            .eventually(|| host.service("ping"), "the ping service")
            .await;
        let reply = tokio::task::spawn_blocking(move || {
            service.invoke("echo", Value::List(vec![json!("go").into()]))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reply.json().unwrap(), json!("netkit pong go"));
        let go = host.binaries_of("go").unwrap();
        assert_eq!(go.runtimes()[0].name, "go-netkit");
    }
}

/// Why a reload is refused, the rows that would fail and their errors:
/// `row <id>: <error>` joined by `; `. A config that does not match its
/// schema (`config: …`, a line per problem) comes first as one
/// `<id>: config: …` line per problem, and the summary says `row <id>'s
/// config does not match its plugin's schema` ([`rejected_lines`] prints
/// them).
fn rejection(failing: &[(String, String)]) -> String {
    let mut lines = Vec::new();
    let mut why = Vec::new();
    for (id, error) in failing {
        let problems: Vec<&str> = error.lines().collect();
        if !problems.is_empty() && problems.iter().all(|line| line.starts_with("config: ")) {
            lines.extend(problems.iter().map(|line| format!("{id}: {line}")));
            why.push(format!(
                "row {id}'s config does not match its plugin's schema"
            ));
        } else {
            why.push(format!("row {id}: {error}"));
        }
    }
    lines.push(why.join("; "));
    lines.join("\n")
}

/// The lines printed for a reload refused with `error`: its lines but the
/// last as they are (rows' config problems, `<row>: config: …`), then
/// `rutis-host: reload rejected: <last>; nothing changed`.
pub fn rejected_lines(error: &str) -> Vec<String> {
    let mut lines: Vec<String> = error.split('\n').map(str::to_owned).collect();
    let last = lines.pop().unwrap_or_default();
    let problems = !lines.is_empty() && lines.iter().all(|line| line.contains(": config: "));
    if !problems {
        return vec![format!(
            "rutis-host: reload rejected: {error}; nothing changed"
        )];
    }
    lines.push(format!(
        "rutis-host: reload rejected: {last}; nothing changed"
    ));
    lines
}

#[cfg(test)]
mod rejection_tests {
    use super::{rejected_lines, rejection};

    /// A row's config problems print a line each, named after the row;
    /// the reload's own line stays one line (config schema design §9).
    #[test]
    fn config_problems_print_a_line_each() {
        let error = rejection(&[
            (
                "weather".into(),
                "config: a is required\nconfig: b is required".into(),
            ),
            ("chat".into(), "SyntaxError".into()),
        ]);
        assert_eq!(
            rejected_lines(&error),
            [
                "weather: config: a is required",
                "weather: config: b is required",
                "rutis-host: reload rejected: row weather's config does not match its plugin's schema; row chat: SyntaxError; nothing changed",
            ]
        );
        assert_eq!(
            rejected_lines("rows.json:3: expected ,\nat line 3"),
            ["rutis-host: reload rejected: rows.json:3: expected ,\nat line 3; nothing changed"]
        );
    }
}
