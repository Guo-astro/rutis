//! Local language runtimes: a process this machine starts, the session
//! with it, and the runtime plugin running its rows on that session,
//! composed:
//! [`crate::pipe::spawn`] starts it on an inherited socket
//! with `--id <name>`, the endpoint it greets as; it is started once, since
//! a local runtime process is not restarted.
//!
//! When the process ends, its session and then the runtime
//! plugin go, and the runtime's state says how it ended. Restarting the
//! composition (`FiberView::restart`) starts a new process: that is the
//! application's decision.
//!
//! What a language's process is (its program, arguments, how it takes its
//! channel) comes from [`crate::runtime`] ([`Launcher`]); `pipe::spawn`
//! only starts it.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::channel::{Channel, Close, Closer, ConnectError, PeerId};
use crate::peer::{peer_key, Peer};
use crate::runtime::{Launcher, RuntimeHandle, RuntimePlugin, RuntimeState, Stdio};
use crate::session::plugin::Greet;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind};
use rutis::{Ctx, Error};
use serde_json::Value;
use tokio::sync::watch;

use crate::pipe::{Handover, Processes, Spawn};

/// Set to report every message crossing a runtime channel (direction and
/// length, never content) on stderr, as for runtimes `Process` starts.
const TRACE_VARIABLE: &str = "RUTIS_TRACE";

/// A local runtime: mount it before the plugins (and the loader rows) that
/// load into it. It provides what [`RuntimePlugin`] does (`Runtime#<name>`).
pub struct LocalRuntime {
    label: String,
    /// The runtime's name (`Runtime#<name>`).
    runtime_name: String,
    launcher: Launcher,
    anchor: PathBuf,
    runtime: RuntimePlugin,
    /// The processes it started, every generation's.
    processes: Processes,
    /// How long its process may take to greet; `None`: the default, 30 s.
    greeting: Option<Duration>,
}

impl LocalRuntime {
    /// The Node runtime, named `"node"`. `node_package`: the rutis-bridge
    /// npm runtime (`node/rutis-runtime`, or a deployed `@arcships/rutis-runtime`).
    /// `anchor`: the `package.json` plugins and Cordis resolve from.
    #[cfg(feature = "node")]
    pub fn node(node_package: impl Into<PathBuf>, anchor: impl Into<PathBuf>) -> Self {
        let launcher = Launcher::node(&node_package.into());
        Self::with("node", launcher, anchor.into())
    }

    /// A Python runtime named `"py"`: `python3 -m rutis`, importing plugin
    /// modules from `project` and the interpreter's environment. The `rutis`
    /// package must be installed there; Python 3.12 or later. Choose the
    /// interpreter (a project's venv) with [`LocalRuntime::interpreter`].
    #[cfg(feature = "python")]
    pub fn python(project: impl Into<PathBuf>) -> Self {
        let project = project.into();
        let launcher = Launcher::python(&project);
        Self::with("py", launcher, project)
    }

    /// A Go runtime: `binary`, built with the Go SDK (`go/rutis`), serving
    /// the plugins compiled into it, in `project`. It is named after the
    /// file ([`crate::runtime::go_runtime_name`]: `go-<name>`); rows are
    /// `<that name>:<plugin>` with `RuntimeResolver::modules`.
    #[cfg(feature = "go")]
    pub fn go(binary: impl Into<PathBuf>, project: impl Into<PathBuf>) -> Self {
        let (binary, project) = (binary.into(), project.into());
        let launcher = Launcher::go(&binary, &project);
        Self::with(&crate::runtime::go_runtime_name(&binary), launcher, project)
    }

    /// A Swift runtime: `binary`, built with the Swift SDK (`swift/rutis`),
    /// serving the plugins compiled into it, in `project`. It is named after
    /// the file ([`crate::runtime::swift_runtime_name`]: `swift-<name>`);
    /// rows are `<that name>:<plugin>` with `RuntimeResolver::modules`.
    #[cfg(feature = "swift")]
    pub fn swift(binary: impl Into<PathBuf>, project: impl Into<PathBuf>) -> Self {
        let (binary, project) = (binary.into(), project.into());
        let launcher = Launcher::swift(&binary, &project);
        Self::with(
            &crate::runtime::swift_runtime_name(&binary),
            launcher,
            project,
        )
    }

    /// A Bun runtime named `"bun"`: the npm package `package` (`bun/rutis-bun`,
    /// or a deployed `@arcships/rutis-bun`) run by `bun`, importing plugin
    /// modules from `project` (its `package.json` and `node_modules`). Choose
    /// the Bun executable with [`LocalRuntime::interpreter`].
    #[cfg(feature = "bun")]
    pub fn bun(package: impl Into<PathBuf>, project: impl Into<PathBuf>) -> Self {
        let project = project.into();
        let launcher = Launcher::bun(None, &package.into(), &project);
        Self::with("bun", launcher, project)
    }

    /// Put `directory` ahead on the runtime's `PYTHONPATH`: a source checkout
    /// of the `rutis` package, or plugins that are not installed.
    pub fn python_path(mut self, directory: impl Into<PathBuf>) -> Self {
        let current = self
            .launcher
            .env
            .iter()
            .find(|(name, _)| name == "PYTHONPATH")
            .map(|(_, current)| current.clone());
        let path = crate::runtime::process::search_path(
            directory.into().into_os_string(),
            current.as_deref(),
        );
        self.launcher.env.retain(|(name, _)| name != "PYTHONPATH");
        self.launcher.env.push(("PYTHONPATH".into(), path));
        self
    }

    /// A runtime started with `launcher`; it receives its channel,
    /// `--id <name>` and the anchor as its last arguments.
    pub fn launcher(
        name: impl Into<String>,
        launcher: Launcher,
        anchor: impl Into<PathBuf>,
    ) -> Self {
        Self::with(&name.into(), launcher, anchor.into())
    }

    fn with(name: &str, launcher: Launcher, anchor: PathBuf) -> Self {
        let processes = Processes::default();
        Self {
            label: format!("runtime {name}"),
            runtime_name: name.to_owned(),
            launcher,
            runtime: RuntimePlugin::session(name, anchor.clone()).processes(processes.clone()),
            anchor,
            processes,
            greeting: None,
        }
    }

    /// The process to start: the launcher, given its channel, `--id` and
    /// the endpoint it greets as, and the anchor as its last arguments.
    fn spawn(&self, peer: &PeerId) -> Spawn {
        let mut spawn = Spawn::new(self.launcher.program.clone());
        spawn.args = self.launcher.args.clone();
        spawn.env = self.launcher.env.clone();
        spawn.cwd = self.launcher.cwd.clone();
        spawn.stdin = self.launcher.stdin;
        spawn.stdout = self.launcher.stdout;
        spawn.stderr = self.launcher.stderr;
        spawn.output_prefix = self.launcher.output_prefix.clone();
        spawn.handover = match self.launcher.inherit_fd {
            true => Handover::Inherit,
            false => Handover::DialBack,
        };
        spawn.trailing = vec![
            OsString::from("--id"),
            OsString::from(peer.as_str()),
            OsString::from(&self.anchor),
        ];
        spawn
    }

    /// Declare the methods of the rutis service at `host_key(name)`, as
    /// [`RuntimePlugin::host`].
    pub fn host(mut self, name: &str, methods: Value) -> Self {
        self.runtime = self.runtime.host(name, methods);
        self
    }

    /// Name the runtime: its services are keyed by the name.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        let name: String = name.into();
        self.runtime = self.runtime.named(name.clone());
        self.label = format!("runtime {name}");
        self.runtime_name = name;
        self
    }

    /// The runtime process's standard input: none by default; with
    /// [`Stdio::Inherit`], this process's, for a plugin that reads the
    /// terminal. See [`Launcher::stdin`]: it is the whole process's, so give
    /// such a plugin a runtime of its own ([`LocalRuntime::named`]), and the
    /// terminal to one runtime at most.
    pub fn stdin(mut self, stdio: Stdio) -> Self {
        self.launcher.stdin = stdio;
        self
    }

    /// The runtime process's standard output: this process's by default;
    /// [`Stdio::Null`] discards it, [`Stdio::Lines`] passes it on a whole
    /// line at a time ([`Launcher::stdout`]).
    pub fn stdout(mut self, stdio: Stdio) -> Self {
        self.launcher.stdout = stdio;
        self
    }

    /// The runtime process's standard error: this process's by default;
    /// [`Stdio::Null`] discards it, [`Stdio::Lines`] passes it on a whole
    /// line at a time ([`Launcher::stderr`]).
    pub fn stderr(mut self, stdio: Stdio) -> Self {
        self.launcher.stderr = stdio;
        self
    }

    /// Start each line of the runtime process's output passed on as
    /// [`Stdio::Lines`] with `[<name>] ` ([`Launcher::output_prefix`]).
    pub fn output_prefix(mut self, name: &str) -> Self {
        self.launcher = self.launcher.output_prefix(name);
        self
    }

    /// Set the environment variable `name` of the runtime process
    /// ([`Launcher::env`]).
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.launcher = self.launcher.env(name, value);
        self
    }

    /// Run the runtime with this interpreter instead of its default
    /// (`python3` for Python, `bun` for Bun).
    pub fn interpreter(mut self, program: impl Into<OsString>) -> Self {
        self.launcher.program = program.into();
        self
    }

    /// How long the runtime's process may take to greet once started: 30 s
    /// by default; [`Duration::MAX`] waits for as long as it takes, for a
    /// process stopped at its first line for a debugger (`--inspect-brk`).
    /// A process that never greets then stays starting, as
    /// `Loader::status` shows.
    pub fn greeting(mut self, limit: Duration) -> Self {
        self.greeting = Some(limit);
        self
    }

    /// Observe the runtime from code that is not a plugin (a loader resolver).
    pub fn handle(&self) -> RuntimeHandle {
        self.runtime.handle()
    }

    /// The processes it started, every generation's: once it is stopped,
    /// [`Processes::running`] says whether its own process has ended yet,
    /// whatever other runtimes' processes do.
    pub fn processes(&self) -> Processes {
        self.processes.clone()
    }
}

impl DynPlugin for LocalRuntime {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let peer = crate::runtime::runtime_peer(&self.runtime_name);
            let carrier = Carrier {
                label: format!("local:{} carrier", self.runtime_name),
                open: Arc::default(),
            };
            let start = carrier.start(self.spawn(&peer), self.processes.clone());
            ctx.plugin(carrier);
            let session = ProcessSession {
                label: format!("session with {peer}"),
                peer,
                start,
                greeting: self.greeting.unwrap_or(LOCAL_RUNTIME_GREETING),
                ended: Arc::new(watch::channel(None).0),
            };
            let mut ended = session.ended.subscribe();
            self.runtime.announce(ctx);
            self.runtime.starting();
            ctx.plugin(session);
            ctx.plugin(self.runtime.clone());
            // Started once the runtime runs, as a runtime plugin starting its
            // own process is: while it starts this is Loading, and a restart
            // or dispose abandons the start (its process ends with the
            // session).
            // What is looked at is marked seen, so going Ready right after
            // the look still wakes the wait: missed, the next change would
            // be the process ending, taken for a failed start.
            let mut states = self.runtime.handle().subscribe();
            let failure = loop {
                match states.borrow_and_update().clone() {
                    RuntimeState::Ready(_) => return Ok(Effect::Done),
                    RuntimeState::Down(reason) => break reason,
                    _ => {}
                }
                if let Some(error) = &*ended.borrow() {
                    break error.clone();
                }
                tokio::select! {
                    _ = states.changed() => {}
                    _ = ended.changed() => {}
                    _ = ctx.cancelled() => {
                        self.runtime.stopped();
                        return Ok(Effect::Done);
                    }
                }
            };
            self.runtime.failed(failure.clone());
            Err(rutis::Error::from(ErrorKind::PluginFailed(failure.into())))
        })
    }
}

/// How long a local runtime process may take to greet, counted from its
/// spawn: the interpreter starting (a JVM, Python with its imports, Node or
/// Bun loading their modules) is part of it, and a healthy one on a busy or
/// slow machine (several runtimes starting at once at boot, a small board)
/// can take longer than the network's 10 s. A process that ends is noticed
/// at once, by its channel closing; this only bounds one that lives and says
/// nothing.
const LOCAL_RUNTIME_GREETING: Duration = Duration::from_secs(30);

/// Starts the runtime process on a channel.
type Start = Arc<dyn Fn() -> BoxFuture<'static, Result<Channel, ConnectError>> + Send + Sync>;

/// The session with the runtime process: started once, greeted as `main`
/// by the runtime `peer`, provided as `Peer#<peer>` while it lasts.
/// `ended` says why it stopped: the process could not start, did not greet,
/// or ended.
struct ProcessSession {
    label: String,
    peer: PeerId,
    start: Start,
    greeting: Duration,
    ended: Arc<watch::Sender<Option<String>>>,
}

impl ProcessSession {
    async fn run(ctx: Ctx, peer: PeerId, start: Start, greeting: Duration) -> String {
        let channel = match start().await {
            Ok(channel) => channel,
            Err(error) => return error.to_string(),
        };
        let greet = Greet {
            local: PeerId::new("main").expect("a valid id"),
            expected: Some(peer.clone()),
            declare: &[],
            require: &[],
            timeout: greeting,
        };
        let (session, operations) = match greet.open(channel).await {
            Ok(opened) => opened,
            Err(error) => return error.to_string(),
        };
        // Closes the session however this ends, aborted included.
        let _open = Open(session.clone(), operations.clone());
        let provided = ctx.provide_at(
            peer_key(&peer),
            Arc::new(Peer::new(peer, session.clone(), operations.clone())),
        );
        let ended = match provided {
            Err(error) => format!("cannot provide the session: {error}"),
            Ok(provided) => {
                session.closed().await;
                let ended = session.close_reason().map_or_else(
                    || "the connection ended".to_owned(),
                    |error| error.to_string(),
                );
                // What depends on the session stops before it closes.
                let _ = provided.dispose().await;
                ended
            }
        };
        ended
    }
}

/// A started session: dropped, it closes the session.
struct Open(
    crate::session::rpc::Connection,
    Arc<crate::peer::Operations>,
);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.close(crate::session::Error::Transport(
            "the connection ended".into(),
        ));
        self.1.detach();
    }
}

impl DynPlugin for ProcessSession {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let (ended, run) = (
                self.ended.clone(),
                Self::run(
                    ctx.clone(),
                    self.peer.clone(),
                    self.start.clone(),
                    self.greeting,
                ),
            );
            let task = tokio::spawn(async move {
                let why = run.await;
                ended.send_replace(Some(why));
            });
            ctx.effect(move || async move {
                // Waited for: a start in the middle of spawning a process
                // (fork and exec, which do not yield) finishes that first,
                // and the process must have been recorded, and its channel
                // closed, when this has stopped.
                task.abort();
                let _ = task.await;
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// What starts the process. Unloaded, it closes the channels it started,
/// ending their processes.
struct Carrier {
    label: String,
    open: Arc<Mutex<Vec<Weak<dyn Closer>>>>,
}

impl Carrier {
    /// Start `spawn` on a channel, recording the process in `processes`
    /// and the channel here.
    fn start(&self, spawn: Spawn, processes: Processes) -> Start {
        let open = self.open.clone();
        Arc::new(move || {
            let (spawn, processes, open) = (spawn.clone(), processes.clone(), open.clone());
            Box::pin(async move {
                let channel = crate::pipe::spawn(&spawn, &processes).await?;
                let channel = match std::env::var_os(TRACE_VARIABLE) {
                    Some(_) => crate::channel::trace::trace(
                        channel,
                        "runtime",
                        Arc::new(|line: &str| eprintln!("rutis trace: {line}")),
                    ),
                    None => channel,
                };
                let mut open = open.lock().unwrap();
                open.retain(|closer| closer.strong_count() > 0);
                open.push(Arc::downgrade(&channel.closer));
                Ok(channel)
            })
        })
    }
}

impl DynPlugin for Carrier {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let open = self.open.clone();
            ctx.effect(move || async move {
                for closer in std::mem::take(&mut *open.lock().unwrap()) {
                    if let Some(closer) = closer.upgrade() {
                        closer.close(Close::Done);
                    }
                }
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}
