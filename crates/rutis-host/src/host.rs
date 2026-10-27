//! A running host: the kernel, the local language runtimes the
//! configuration names, the transports and identity links use, and the
//! loader that runs the rows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberView, TypeKey};
use rutis::{Ctx, Error as RutisError};
use rutis_bridge::runtime::{RuntimeHandle, RuntimePlugin, RuntimeState};
use rutis_bridge::transport::local::LocalPlugin;
use rutis_bridge::transport::websocket::{
    Config, ListenerConfig, ServerTls, Trust, WebSocketPlugin,
};
use rutis_bridge::{
    AccessPointConfig, AccessPointPlugin, AdmissionPlugin, Credential, Identity, IdentityPlugin,
    PeerId, Presented, SharedToken,
};
use rutis_loader::peer::{register_peer_node, PeerResolver};
use rutis_loader::runtime::binary::RuntimeEnd;
use rutis_loader::runtime::{RuntimeInstances, RuntimeResolver, RuntimeRows, RuntimeRowsPlugin};
use rutis_loader::{
    Builtins, Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog,
};

use crate::config::{token, HostConfig, Restart};
use crate::languages::{self, BinaryHost, Mount, LANGUAGES};
use crate::{errln, outln};

pub struct Host {
    /// The host runs while this lives.
    root: Ctx,
    pub loader: Loader,
    pub runtimes: Vec<(String, RuntimeHandle)>,
    /// The resolvers of the runtimes' rows, to invalidate after a change.
    pub resolvers: Vec<Arc<RuntimeResolver>>,
    /// The compiled languages' binaries and runtimes (Go, Swift), those
    /// the configuration has.
    pub binaries: Vec<BinaryHost>,
    /// The local runtimes' plugins, by name, to stop one
    /// ([`Host::stop_runtime`]).
    fibers: Vec<(String, FiberView)>,
    /// The processes each local runtime started, by name: to wait for its
    /// own once it is stopped ([`Host::reload`]).
    processes: Vec<(String, rutis_bridge::transport::local::Processes)>,
    /// The `rutis-bridge/peer` rows' resolver, which also knows their links'
    /// states ([`PeerResolver::link_state`]).
    pub peers: Arc<PeerResolver>,
    /// Each configured listener (`listen`) and the address it is bound to,
    /// in the configuration's order: with port 0, the port picked.
    pub listening: Vec<(String, std::net::SocketAddr)>,
    /// The configuration it runs: as started, then as last reloaded
    /// ([`Host::reload`]).
    config: HostConfig,
    /// The peers whose links this host's identity accepts (peer rows that
    /// listen); none when an embedding brought its own identity.
    accepted: Option<Accepted>,
    /// What each local runtime's `restart` says, by name.
    restarts: Vec<(String, Restart)>,
    /// The tasks watching the local runtimes ([`Host::supervise`]), by
    /// name.
    supervisors: Mutex<Vec<(String, tokio::task::JoinHandle<()>)>>,
}

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

/// What a Rust program embedding this host adds to what the configuration
/// names (the E2E fixture host, `tests/e2e/fixture-host`).
#[derive(Default)]
pub struct Embedding {
    /// Rows compiled into the program, next to `rutis-bridge/peer`.
    pub builtins: Option<RegisterBuiltins>,
    /// The identity links use instead of the host's own, which presents
    /// and accepts the tokens of the configuration's peer rows from the
    /// environment. A host with one of its own does not change it on
    /// [`Host::reload`].
    pub identity: Option<IdentityPlugin>,
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
    rutis_bridge::transport::local::output_ended(OUTPUT_GUARD);
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
        config.check_runtime_names()?;
        config.check_dialing_runtimes()?;
        // The token of each runtime that dials in, before anything starts.
        let mut dialing = Vec::new();
        for remote in &config.runtimes.remote {
            if remote.dials_in()?.is_some() && token(&remote.name).is_none() {
                return Err(format!(
                    "remote runtime {}: set RUTIS_TOKEN_{} (or RUTIS_TOKEN): the token its \
                     runtimes present when they dial in",
                    remote.name,
                    remote.name.to_uppercase().replace('-', "_")
                ));
            }
        }
        // Event failures print as the protocol says (`rutis: listener of
        // <name> in <where> failed: <ErrorName>: <message>`); the rest as
        // the kernel prints them.
        let fallback = rutis::host::default_sink();
        let sink: rutis::host::ErrorSink =
            std::sync::Arc::new(
                move |error| match rutis_bridge::events::report_line(&error) {
                    Some(line) => eprintln!("{line}"),
                    None => fallback(error),
                },
            );
        let root = Ctx::root_with_sink(
            tokio::runtime::Handle::try_current().map_err(|error| error.to_string())?,
            sink,
        );
        // Each row's line says what it waits for (`status`): the kernel's
        // report of a long wait would say it again.
        root.report_waiting_after(None);
        let mut catalog = ServiceCatalog::new();
        catalog.share_by_name();

        // Transports and this node's identity, for the rows that link.
        (&root.plugin(LocalPlugin::new()))
            .await
            .map_err(|error| format!("the local transport: {error}"))?;
        let websocket = WebSocketPlugin::new(websocket(config)?)?;
        (&root.plugin(websocket.clone()))
            .await
            .map_err(|error| format!("the WebSocket transport: {error}"))?;
        let listening = match websocket.transport() {
            Some(transport) => config
                .listen
                .iter()
                .filter_map(|listener| {
                    let address = transport.local_addr(&listener.name)?;
                    Some((listener.name.clone(), address))
                })
                .collect(),
            None => Vec::new(),
        };
        let accepted = match embedding.identity {
            Some(identity) => {
                root.plugin(identity);
                None
            }
            None => {
                let identity = EnvIdentity::new(config)?;
                let accepted = identity.accepted.clone();
                root.plugin(IdentityPlugin::new("host", identity));
                Some(accepted)
            }
        };

        let peers = Arc::new(PeerResolver::new());
        let mut builtins = Builtins::new();
        register_peer_node(&mut builtins, peers.clone());
        if let Some(register) = embedding.builtins {
            register(&mut builtins);
        }
        let mut mount = Mount {
            root: &root,
            catalog: &catalog,
            chain: Chain::new().with(builtins).with_shared(peers.clone()),
            runtimes: Vec::new(),
            resolvers: Vec::new(),
            fibers: Vec::new(),
            processes: Vec::new(),
            later: Vec::new(),
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

        for remote in &config.runtimes.remote {
            let language = languages::by_remote_name(&remote.language).ok_or_else(|| {
                let names: Vec<String> = languages::remote_names()
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                format!(
                    "remote runtime {}: the language is {}, not {}",
                    remote.name,
                    languages::either(&names),
                    remote.language
                )
            })?;
            // Each connection an instance, its rows `<name>:<plugin>` on it.
            if let Some((listen, group)) = remote.dials_in()? {
                let instances = RuntimeInstances::new(&remote.name, group, &catalog);
                mount.chain = std::mem::take(&mut mount.chain).with(instances.clone());
                dialing.push((remote.name.clone(), listen.to_owned(), instances));
                continue;
            }
            let runtime = RuntimePlugin::remote(&remote.name);
            let handle = runtime.handle();
            let rows = Arc::new(language.rows(handle.clone()).with_catalog(&catalog));
            root.plugin(runtime);
            mount.resolve(rows.clone());
            mount.resolvers.push(rows);
            // Not waited for at start: it runs when its link does.
        }
        let Mount {
            chain,
            runtimes,
            resolvers,
            fibers,
            processes,
            later,
            ..
        } = mount;

        let plugin = LoaderPlugin::new(
            chain,
            LoaderOptions {
                catalog,
                ..LoaderOptions::default()
            },
        );
        let loader = plugin.handle();
        (&root.plugin(plugin))
            .await
            .map_err(|error| format!("the loader: {error}"))?;
        for rows in &resolvers {
            root.plugin(RuntimeRowsPlugin::new(rows.clone()));
        }
        // An access point per runtime that dials in: no link row, the
        // connections come and go with their instances.
        let local = PeerId::new(config.id.clone()).map_err(|error| format!("id: {error}"))?;
        for (name, listen, instances) in dialing {
            let token = token(&name).expect("checked above");
            root.plugin(AdmissionPlugin::new(
                &name,
                SharedToken::new(local.clone(), token),
            ));
            let id = PeerId::new(name.clone())
                .map_err(|error| format!("remote runtime {name}: {error}"))?;
            let access = AccessPointPlugin::new(
                AccessPointConfig::new(id, "websocket", &name, &listen).require("runtime"),
            )
            .accept(instances.accept(loader.clone(), root.clone()));
            (&root.plugin(access))
                .await
                .map_err(|error| format!("remote runtime {name}: {error}"))?;
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
        // coming and going: their plugins supervise them ([`Host::supervise`]).
        let mut binaries = Vec::new();
        for later in later {
            binaries.extend(later(root.clone()).await?);
        }
        Ok(Self {
            root,
            loader,
            runtimes,
            resolvers,
            binaries,
            fibers,
            processes,
            peers,
            listening,
            config: config.clone(),
            accepted,
            restarts,
            supervisors: Mutex::default(),
        })
    }

    /// Watch each local runtime as its `restart` says, from now on
    /// ([`crate::restart::supervise`]): `on-failure` starts it again when
    /// its process ends on its own; one whose `restart` is `exit` comes out
    /// of the [`Exits`] returned when it ends, for the host to stop. The
    /// runtimes of compiled languages (Go, Swift), which start and stop as
    /// rows use them, are supervised by their plugin
    /// ([`rutis_loader::runtime::binary::BinaryRuntimesHandle::supervise`]), each on its
    /// own, with the same numbers and lines.
    pub fn supervise(&self) -> Exits {
        let (send, exits) = tokio::sync::mpsc::unbounded_channel();
        for binaries in &self.binaries {
            if let Restart::No = binaries.restart {
                // None will come: the ends noted while starting are not
                // kept for it.
                binaries.unsupervise();
                continue;
            }
            let send = send.clone();
            binaries.supervise(
                binaries.restart.into(),
                Box::new(move |end| match end {
                    RuntimeEnd::Exit { runtime, why } => {
                        let _ = send.send((runtime, why));
                    }
                    end => {
                        if let Some(line) = end.line() {
                            outln!("{line}");
                        }
                    }
                }),
            );
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
            let (name, fiber, handle, restart) =
                (name.clone(), fiber.clone(), handle.clone(), *restart);
            let send = send.clone();
            let task = tokio::spawn({
                let name = name.clone();
                async move {
                    let supervised =
                        crate::restart::supervise(&name, &handle, &fiber, restart).await;
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
                binaries.unsupervise();
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
    /// come back by themselves, resolved again first when
    /// [`Host::invalidate`] was called: their code as it is now.
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
        loop {
            match handle.state() {
                RuntimeState::Ready(_) => break,
                RuntimeState::Down(reason) => {
                    return Err(format!("the {name} runtime did not start: {reason}"))
                }
                _ => handle.changed().await,
            }
        }
        // A running runtime releases its rows ([`RuntimeRows`]) once it
        // has resolved again those resolved before it ran. Rows loaded
        // before then wait for it, and the host would say it runs while
        // they do (#314). A plugin injecting it applies once it is there.
        let (released, rows) = tokio::sync::oneshot::channel();
        // Disposed however the wait ends, a signal dropping it included:
        // left mounted, it would count among what still runs.
        let mut waiter = Unmount(Some(self.root.plugin(Released {
            injects: [RuntimeRows::key(name)],
            released: std::sync::Mutex::new(Some(released)),
        })));
        let down = async {
            loop {
                if let RuntimeState::Down(reason) = handle.state() {
                    return reason;
                }
                handle.changed().await;
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

    /// Load `rows` (replacing what was loaded before).
    pub async fn load(&self, rows: Vec<serde_json::Value>) -> Result<(), Error> {
        self.loader
            .reconcile(vec![layer(rows)?], None)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Run `new` instead of the configuration this host runs, all or
    /// nothing (`docs/guide/rutis-host.md`, "Changing the configuration
    /// while it runs"):
    ///
    /// - What a running host cannot change refuses it, saying to restart
    ///   the host: `format`, `id`, `listen`, `runtimes.remote`, and a local
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
    /// error says why. Credentials are read when links dial or are
    /// dialed, so a peer row `new` adds presents its token.
    pub async fn reload(&mut self, new: HostConfig) -> Result<Reloaded, Error> {
        restart_needed(&self.config, &new)?;
        new.check_runtime_names()?;
        new.check_dialing_runtimes()?;
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
        let failing: Vec<String> = checked
            .rows
            .iter()
            .filter(|row| !failing_now.contains(&row.id))
            .filter_map(|row| match &row.status {
                rutis_loader::report::RowStatus::Failed(error) => {
                    Some(format!("row {}: {error}", row.id))
                }
                _ => None,
            })
            .collect();
        if !failing.is_empty() {
            return Err(failing.join("; "));
        }

        let (accepted, accepted_before) = (listening_peers(&new)?, listening_peers(&self.config)?);
        let old = std::mem::replace(&mut self.config, new);
        self.accept(accepted);
        let applied = self
            .loader
            .reconcile(vec![layer(self.config.rows())?], None)
            .await
            .map_err(|error| error.to_string());
        let failed = match applied {
            Ok(report) if report.new_failures.is_empty() => None,
            Ok(report) => Some(
                report
                    .new_failures
                    .iter()
                    .map(|failure| format!("row {}: {}", failure.id, failure.error))
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Err(error) => Some(error),
        };
        if let Some(why) = failed {
            // The old rows again: what failed was new.
            self.config = old;
            self.accept(accepted_before);
            self.load(self.config.rows())
                .await
                .map_err(|error| format!("{why}; and the old rows did not load again: {error}"))?;
            return Err(why);
        }

        let mut reloaded = Reloaded {
            rows: changed_rows(&old.rows, &self.config.rows),
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

    /// Accept the links of `peers` (those of the peer rows that listen).
    fn accept(&self, peers: Vec<PeerId>) {
        if let Some(accepted) = &self.accepted {
            *accepted.write().unwrap() = peers;
        }
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
        tokio::time::timeout(time, rutis_bridge::transport::local::processes_ended())
            .await
            .map_err(|_| Unfinished::Processes(format!("{}s passed", time.as_secs_f64())))
    }

    /// End the runtime processes now, without their cleanups, and wait
    /// (briefly) until they are gone.
    pub async fn kill_processes(&self) {
        rutis_bridge::transport::local::kill_processes();
        let gone = rutis_bridge::transport::local::processes_ended();
        if tokio::time::timeout(KILLED_EXIT, gone).await.is_err() {
            let left = rutis_bridge::transport::local::running_processes();
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
        // What is not a row (a runtime, a link) and still stops is named
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

    /// What the status lines follow ([`crate::status::follow`]): the rows,
    /// their links, and the runtimes.
    pub fn followed(&self) -> crate::status::Followed {
        crate::status::Followed {
            loader: self.loader.clone(),
            peers: self.peers.clone(),
            runtimes: self.runtimes.clone(),
            binaries: self.binaries.clone(),
        }
    }

    /// Forget what the runtimes' resolvers cached, so changed plugins are
    /// described again.
    pub fn invalidate(&self) {
        for rows in &self.resolvers {
            rows.invalidate_all();
        }
    }
}

fn websocket(config: &HostConfig) -> Result<Config, Error> {
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
    let mut websocket = Config::new();
    if let Some(ca) = env_path("RUTIS_CA") {
        websocket = websocket.trust(Trust {
            system: true,
            ca_pem: vec![read(&ca, "RUTIS_CA", Pem::Certificate)?],
        });
    }
    let local = PeerId::new(config.id.clone()).map_err(|error| format!("id: {error}"))?;
    for listener in &config.listen {
        let mut listening = ListenerConfig::new(&listener.name, listener.address, local.clone());
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
        websocket = websocket.listener(listening);
    }
    websocket.validate()?;
    Ok(websocket)
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
    let names = |config: &HostConfig| -> Vec<String> {
        config
            .runtimes
            .remote
            .iter()
            .map(|remote| remote.name.clone())
            .collect()
    };
    for remote in &new.runtimes.remote {
        let before = old.runtimes.remote.iter().find(|r| r.name == remote.name);
        match before {
            None => return restart(format!("runtimes.remote {} added", remote.name)),
            Some(before) if format!("{before:?}") != format!("{remote:?}") => {
                return restart(format!("runtimes.remote {} changed", remote.name))
            }
            _ => {}
        }
    }
    if let Some(gone) = names(old)
        .into_iter()
        .find(|name| !names(new).contains(name))
    {
        return restart(format!("runtimes.remote {gone} removed"));
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

/// The peers of `config`'s peer rows that listen: their links are
/// accepted with their tokens.
fn listening_peers(config: &HostConfig) -> Result<Vec<PeerId>, Error> {
    config
        .peers()
        .into_iter()
        .filter(|(_, dials)| !dials)
        .map(|(peer, _)| PeerId::new(peer.clone()).map_err(|error| format!("peer {peer}: {error}")))
        .collect()
}

/// The peers whose links an [`EnvIdentity`] accepts.
type Accepted = Arc<RwLock<Vec<PeerId>>>;

/// This host's identity. Each peer's token comes from the environment
/// ([`token`]: `RUTIS_TOKEN_<PEER>`, else `RUTIS_TOKEN`) when a link dials
/// it, or when a peer that a listening peer row names dials in: a peer row
/// a reload adds has its token too.
struct EnvIdentity {
    local: PeerId,
    accepted: Accepted,
}

impl EnvIdentity {
    fn new(config: &HostConfig) -> Result<Self, Error> {
        Ok(Self {
            local: PeerId::new(config.id.clone()).map_err(|error| format!("id: {error}"))?,
            accepted: Arc::new(RwLock::new(listening_peers(config)?)),
        })
    }
}

impl Identity for EnvIdentity {
    fn local(&self) -> &PeerId {
        &self.local
    }

    fn credential(&self, peer: &PeerId) -> Option<Credential> {
        token(peer.as_str()).map(Credential::Bearer)
    }

    fn verify(&self, presented: Presented<'_>) -> Option<PeerId> {
        let Presented::Bearer(presented) = presented else {
            return None;
        };
        // Every peer is compared, each in constant time, so timing tells
        // nothing about which token came close; the last one matching wins.
        let mut found = None;
        for peer in self.accepted.read().unwrap().iter() {
            let Some(token) = token(peer.as_str()) else {
                continue;
            };
            if constant_time_eq(token.as_bytes(), presented.as_bytes()) {
                found = Some(peer.clone());
            }
        }
        found
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
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
    use crate::config::{BunRuntime, NodeRuntime, PythonRuntime, RemoteRuntime, Runtimes};
    use rutis::internal::CtxInternal;
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
            rows: vec![json!({ "id": "greeter", "name": crate::config::file_url(&entry) })],
            warnings: Vec::new(),
        };
        let host = Host::start(&config).await.unwrap();
        host.runtimes_ready().await.unwrap();
        host.load(config.rows()).await.unwrap();
        assert_eq!(greeting(&host).await, "Hello, Ada");
        // A different size, so the edit shows even within the same instant.
        std::fs::write(&entry, node_plugin("Welcome")).unwrap();
        host.invalidate();
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
            rows: vec![json!({ "id": "greeter", "name": "py:greeter" })],
            warnings: Vec::new(),
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
    /// what they wait for ([`rutis_loader::RuntimeRows`]) is there, so a row
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
            rows: vec![json!({ "id": "plain", "name": "py:plain" })],
            warnings: Vec::new(),
        };
        let host = Host::start(&config).await.unwrap();
        RUNTIME_START
            .within("the Python runtime", host.runtimes_ready())
            .await
            .unwrap();
        // What the runtime's rows wait for.
        let rows = host.root.get_key::<RuntimeRows>(RuntimeRows::key("py"));
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
            rows: vec![json!({ "id": "greeter", "name": "bun:./greeter.ts" })],
            warnings: Vec::new(),
        };
        let host = Host::start(&config).await.unwrap();
        host.runtimes_ready().await.unwrap();
        host.load(config.rows()).await.unwrap();
        assert_eq!(greeting(&host).await, "Hello, Ada");
        std::fs::write(dir.path().join("greeter.ts"), bun_plugin("Welcome")).unwrap();
        host.loader.reload("greeter").await.unwrap();
        assert_eq!(greeting(&host).await, "Welcome, Ada");
    }

    /// A runtime's name is its rows' prefix, so it names one runtime.
    #[tokio::test(flavor = "multi_thread")]
    async fn runtime_names_are_unique() {
        let config = |remote: &str| HostConfig {
            format: crate::config::FORMAT,
            id: "test".into(),
            runtimes: Runtimes {
                bun: Some(BunRuntime {
                    restart: crate::config::Restart::No,
                    project: PathBuf::from("."),
                    runtime: None,
                    program: None,
                }),
                remote: vec![RemoteRuntime {
                    name: remote.into(),
                    language: "python".into(),
                    listen: None,
                    instances: None,
                }],
                ..Runtimes::default()
            },
            listen: Vec::new(),
            rows: Vec::new(),
            warnings: Vec::new(),
        };
        for taken in ["bun", "file", "g", "GPU"] {
            let error = Host::start(&config(taken)).await.err().expect(taken);
            assert!(error.contains("remote runtime"), "{taken}: {error}");
        }
        assert!(config("gpu").check_runtime_names().is_ok());
    }

    /// A runtime on another machine (here, a process listening on loopback):
    /// a peer row links to it, and its rows are `<runtime>:<module>`.
    /// guarantee: loader/remote-runtime-rows
    #[tokio::test(flavor = "multi_thread")]
    async fn a_remote_runtime_runs_rows_named_after_it() {
        use tokio::io::AsyncBufReadExt;
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("greeter.py"),
            "provides = {'greeter': {'hello': 'sync'}}\n\
             class Greeter:\n    def hello(self, name):\n        return 'Remote, ' + name\n\
             def apply(ctx, config):\n    ctx.provide('greeter', Greeter())\n",
        )
        .unwrap();
        let python =
            std::env::var("RUTIS_PYTHON").unwrap_or_else(|_| crate::config::DEFAULT_PYTHON.into());
        let mut child = tokio::process::Command::new(python)
            .args([
                "-m",
                "rutis",
                "listen:ws://127.0.0.1:0/rutis",
                "--id",
                "gpu",
                "--peer",
                "remote-test",
            ])
            .arg(project.path())
            .env("PYTHONPATH", repo().join("python/rutis"))
            .env("RUTIS_TOKEN", "remote-token")
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let address = within("the runtime's address", async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

        std::env::set_var("RUTIS_TOKEN_GPU", "remote-token");
        let config: HostConfig = serde_json::from_value(json!({
            "id": "remote-test",
            "runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
            "rows": [
                { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": address, "runtime": "gpu" } },
                { "id": "greeter", "name": "gpu:greeter" }
            ]
        }))
        .unwrap();
        let host = Host::start(&config).await.unwrap();
        host.load(config.rows()).await.unwrap();
        assert_eq!(greeting(&host).await, "Remote, Ada");
        child.start_kill().unwrap();
    }

    /// A Bun runtime on another machine (here, listening on loopback): a
    /// peer row links to it, and its rows are `<runtime>:<module>`. Bun on
    /// Windows is not tested yet.
    /// guarantee: loader/remote-runtime-rows
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_remote_bun_runtime_runs_rows_named_after_it() {
        use tokio::io::AsyncBufReadExt;
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("package.json"), "{}").unwrap();
        std::fs::write(
            project.path().join("greeter.ts"),
            "export const provides = { greeter: { hello: 'sync' } }\n\
             export function apply(ctx) { ctx.provide('greeter', { hello: (name: string) => 'Bun, ' + name }) }\n",
        )
        .unwrap();
        let mut child = tokio::process::Command::new("bun")
            .args(["--no-install", "--no-env-file"])
            .arg(repo().join("bun/rutis-bun/src/main.ts"))
            .args([
                "listen:ws://127.0.0.1:0/rutis",
                "--id",
                "edge",
                "--peer",
                "remote-bun",
            ])
            .arg(project.path())
            .current_dir(project.path())
            .env("RUTIS_TOKEN", "edge-token")
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let address = within("the runtime's address", async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

        std::env::set_var("RUTIS_TOKEN_EDGE", "edge-token");
        let config: HostConfig = serde_json::from_value(json!({
            "id": "remote-bun",
            "runtimes": { "remote": [{ "name": "edge", "language": "bun" }] },
            "rows": [
                { "id": "edge", "name": "rutis-bridge/peer", "config": { "peer": "edge", "dial": address, "runtime": "edge" } },
                { "id": "greeter", "name": "edge:./greeter.ts" }
            ]
        }))
        .unwrap();
        let host = Host::start(&config).await.unwrap();
        host.load(config.rows()).await.unwrap();
        assert_eq!(greeting(&host).await, "Bun, Ada");
        child.start_kill().unwrap();
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
                "node": {}, "py": {}, "go": { "dir": "plugins" },
                "remote": [
                    { "name": "gpu", "language": "python" },
                    { "name": "phone", "language": "python", "listen": "public", "instances": "phone" }
                ]
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
                changed(&|c| c["runtimes"]["remote"][0]["language"] = json!("go")),
                "runtimes.remote gpu changed",
            ),
            (
                changed(&|c| c["runtimes"]["remote"][1]["instances"] = json!("tablet")),
                "runtimes.remote phone changed",
            ),
            (
                changed(&|c| c["runtimes"]["remote"][1]["listen"] = json!("other")),
                "runtimes.remote phone changed",
            ),
            (
                changed(&|c| {
                    c["runtimes"]["remote"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({ "name": "edge", "language": "bun" }))
                }),
                "runtimes.remote edge added",
            ),
            (
                changed(&|c| {
                    c["runtimes"]["remote"].as_array_mut().unwrap().remove(0);
                }),
                "runtimes.remote gpu removed",
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

    /// A peer's token is read when its link dials, or when it dials in: one
    /// set after the host started counts, and a peer that a listening row
    /// names is accepted with its own token once a reload adds the row.
    /// guarantee: host/credentials-from-env
    #[test]
    fn tokens_are_read_when_links_need_them() {
        let config = parsed(json!({ "id": "main" }));
        let identity = EnvIdentity::new(&config).unwrap();
        let peer = PeerId::new("reload-dial-test").unwrap();
        assert!(identity.credential(&peer).is_none());
        std::env::set_var("RUTIS_TOKEN_RELOAD_DIAL_TEST", "dial-secret");
        assert!(matches!(
            identity.credential(&peer),
            Some(Credential::Bearer(token)) if token == "dial-secret"
        ));
        std::env::remove_var("RUTIS_TOKEN_RELOAD_DIAL_TEST");

        std::env::set_var("RUTIS_TOKEN_RELOAD_LISTEN_TEST", "listen-secret");
        assert_eq!(identity.verify(Presented::Bearer("listen-secret")), None);
        let reloaded = parsed(json!({
            "id": "main",
            "rows": [{ "id": "in", "name": "rutis-bridge/peer", "config": { "peer": "reload-listen-test", "listen": "public" } }]
        }));
        *identity.accepted.write().unwrap() = listening_peers(&reloaded).unwrap();
        assert_eq!(
            identity.verify(Presented::Bearer("listen-secret")),
            Some(PeerId::new("reload-listen-test").unwrap())
        );
        assert_eq!(identity.verify(Presented::Bearer("listen-secre")), None);
        assert_eq!(
            identity.verify(Presented::Certificate(b"listen-secret")),
            None
        );
        std::env::remove_var("RUTIS_TOKEN_RELOAD_LISTEN_TEST");
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

    /// A Go binary on another machine (here, listening on loopback): a peer
    /// row links to it, and its rows are `<runtime>:<plugin>`.
    /// guarantee: loader/remote-runtime-rows
    #[tokio::test(flavor = "multi_thread")]
    async fn a_remote_go_runtime_runs_rows_named_after_it() {
        use tokio::io::AsyncBufReadExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join(if cfg!(windows) {
            "netkit.exe"
        } else {
            "netkit"
        });
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./cmd/netkit")
            .current_dir(repo().join("crates/rutis-loader/tests/fixtures/go"))
            .status()
            .expect("go is on PATH");
        assert!(status.success());
        let mut child = tokio::process::Command::new(&binary)
            .args([
                "listen:ws://127.0.0.1:0/rutis",
                "--id",
                "edge",
                "--peer",
                "remote-go",
            ])
            .arg(dir.path())
            .env("RUTIS_TOKEN", "edge-token")
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let address = within("the runtime's address", async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
        std::env::set_var("RUTIS_TOKEN_EDGE", "edge-token");
        let config: HostConfig = serde_json::from_value(json!({
            "id": "remote-go",
            "runtimes": { "remote": [{ "name": "edge", "language": "go" }] },
            "rows": [
                { "id": "edge", "name": "rutis-bridge/peer", "config": { "peer": "edge", "dial": address, "runtime": "edge" } },
                { "id": "ping", "name": "edge:ping" }
            ]
        }))
        .unwrap();
        let host = Host::start(&config).await.unwrap();
        host.load(config.rows()).await.unwrap();
        let service = RUNTIME_START
            .eventually(|| host.service("ping"), "the ping service")
            .await;
        let reply = tokio::task::spawn_blocking(move || {
            service.invoke("echo", Value::List(vec![json!("far").into()]))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reply.json().unwrap(), json!("netkit pong far"));
        child.start_kill().unwrap();
    }
}
