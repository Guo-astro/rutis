//! `rutis-host/spawn`: start a local runtime's process and install a
//! session on its channel (`docs/design-remote-session-2026-10-28.md` §六).
//! `runtimes.node`, `runtimes.py` and the others are its rows, one per
//! runtime, named after it: the process greets as that name (`--id py`),
//! and the rows `py:<module>` run on its session.
//!
//! Its fiber is the row: the session below it provides its handle under the
//! row's id, which the loader's rows find it by. While the session runs,
//! the runtime is `Ready`; the process ending on its own unloads the
//! session, and the runtime is `Down` with how it ended, its rows waiting,
//! until the plugin is restarted ([`crate::restart::supervise`]). Unloading
//! the plugin unloads the session, which closes the channel and ends the
//! process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error};
use rutis_bridge::pipe::Processes;
use rutis_bridge::runtime::{
    ended_status, Launcher, RuntimeHandle, RuntimeState, RuntimeStates, Stdio,
};
use rutis_bridge::{session_key, PeerId, Session};

/// How long a runtime's process may take to greet, by default.
const GREETING: Duration = Duration::from_secs(30);

/// One local runtime: what starts its process, and its state.
pub struct SpawnPlugin {
    name: String,
    label: String,
    launcher: Launcher,
    anchor: PathBuf,
    states: RuntimeStates,
    processes: Processes,
    /// How long its process may take to greet.
    greeting: Duration,
    /// What watches the running session, until the plugin unloads.
    watching: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl SpawnPlugin {
    /// The runtime `name`, its process started with `launcher`, given
    /// `--id <name>` and `anchor` (where its plugins resolve from) as its
    /// last arguments.
    pub fn new(name: &str, launcher: Launcher, anchor: impl Into<PathBuf>) -> Self {
        let anchor = anchor.into();
        let processes = Processes::default();
        Self {
            name: name.to_owned(),
            label: format!("runtime {name}"),
            states: RuntimeStates::new(name, anchor.clone()).processes(processes.clone()),
            launcher,
            anchor,
            processes,
            greeting: GREETING,
            watching: Arc::default(),
        }
    }

    /// How long its process may take to greet once started: 30 s by
    /// default; [`Duration::MAX`] waits for as long as it takes, for a
    /// process stopped at its first line for a debugger (`--inspect-brk`),
    /// which stays starting meanwhile.
    pub fn greeting(mut self, limit: Duration) -> Self {
        self.greeting = limit;
        self
    }

    /// Put `directory` ahead on the process's `PYTHONPATH`: a source checkout
    /// of the `rutis` package.
    pub fn python_path(mut self, directory: impl Into<PathBuf>) -> Self {
        let env = &mut self.launcher.env;
        let current = env
            .iter()
            .find(|(name, _)| name == "PYTHONPATH")
            .map(|(_, current)| current.clone());
        let mut entries = vec![directory.into()];
        entries.extend(current.iter().flat_map(std::env::split_paths));
        let path =
            std::env::join_paths(&entries).unwrap_or_else(|_| entries[0].clone().into_os_string());
        env.retain(|(name, _)| name != "PYTHONPATH");
        env.push(("PYTHONPATH".into(), path));
        self
    }

    /// Run the process with `program` instead of the launcher's.
    pub fn interpreter(mut self, program: impl Into<OsString>) -> Self {
        self.launcher.program = program.into();
        self
    }

    /// Set the environment variable `name` of the process.
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.launcher = self.launcher.env(name, value);
        self
    }

    /// The process's standard input, output and error ([`Launcher::stdin`]).
    pub fn stdio(mut self, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> Self {
        self.launcher = self.launcher.stdin(stdin).stdout(stdout).stderr(stderr);
        self
    }

    /// The runtime's state, for those that watch it.
    pub fn handle(&self) -> RuntimeHandle {
        self.states.handle()
    }

    /// The processes it started, every start's.
    pub fn processes(&self) -> Processes {
        self.processes.clone()
    }

    /// Where its plugins resolve from.
    pub fn anchor(&self) -> &Path {
        &self.anchor
    }

    fn failed(&self, why: String) -> Error {
        self.states.failed(why.clone());
        Error::from(ErrorKind::PluginFailed(why.into()))
    }
}

impl DynPlugin for SpawnPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            // The session below provides its handle under this row's id.
            ctx.mark_row(&self.name)?;
            self.states.announce(ctx);
            self.states.starting();
            let id =
                PeerId::new(self.name.clone()).map_err(|error| self.failed(error.to_string()))?;
            let started = tokio::select! {
                started = self.launcher.start(&id, &self.anchor, &self.processes) => started,
                () = ctx.cancelled() => {
                    self.states.stopped();
                    return Ok(Effect::Done);
                }
            };
            let channel = started.map_err(|error| self.failed(error.to_string()))?;
            // A cold interpreter, or one stopped for a debugger, may take
            // longer than a session's handshake waits.
            let channel = tokio::select! {
                greeted = rutis_bridge::pipe::greeted(channel, self.greeting) => {
                    greeted.map_err(|why| self.failed(why))?
                }
                () = ctx.cancelled() => {
                    self.states.stopped();
                    return Ok(Effect::Done);
                }
            };
            let session = ctx.plugin(Session::new(channel));
            // Running once the session is: it greeted, and runs plugins.
            let mut states = session.watch();
            loop {
                let snapshot = states.borrow_and_update().clone();
                match snapshot.state {
                    FiberState::Active => break,
                    FiberState::Failed | FiberState::Disposed => {
                        let why = snapshot
                            .error
                            .map(|error| error.to_string())
                            .unwrap_or_else(|| "the runtime ended as it started".into());
                        return Err(self.failed(why));
                    }
                    _ => {}
                }
                tokio::select! {
                    changed = states.changed() => if changed.is_err() {
                        return Err(self.failed("the runtime ended as it started".into()));
                    },
                    () = ctx.cancelled() => {
                        self.states.stopped();
                        return Ok(Effect::Done);
                    }
                }
            }
            let process = ctx
                .get_at(&session_key(&self.name))
                .and_then(|handle| handle.process())
                .ok_or_else(|| self.failed(format!("the {} runtime runs no plugins", self.name)))?;
            self.states.ready(process.clone());
            // The process ending on its own unloads the session: the runtime
            // is down, and says how it ended. Unloading this plugin is no
            // end of the runtime's.
            let (states_now, owner) = (self.states.clone(), ctx.clone());
            let watcher = tokio::spawn(async move {
                let mut states = session.watch();
                let _ = states
                    .wait_for(|snapshot| {
                        matches!(snapshot.state, FiberState::Failed | FiberState::Disposed)
                    })
                    .await;
                if !owner.cancellation_token().is_cancelled() {
                    states_now.failed(ended_status(&process));
                }
            });
            *self.watching.lock().unwrap() = Some(watcher);
            let (watching, states_now) = (self.watching.clone(), self.states.clone());
            ctx.effect(move || async move {
                if let Some(watcher) = watching.lock().unwrap().take() {
                    watcher.abort();
                }
                if !matches!(states_now.handle().state(), RuntimeState::Down(_)) {
                    states_now.stopped();
                }
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use rutis::host::CtxHost;

    /// A runtime whose process never greets.
    fn mute(dir: &Path) -> SpawnPlugin {
        let launcher = Launcher::new("sh").arg("-c").arg("sleep 60").inherit_fd();
        SpawnPlugin::new("mute", launcher, dir)
    }

    /// A process that never greets: with `greeting(1s)` the runtime is down
    /// within a few seconds, not the default 30.
    // guarantee: host/plugin-limits (greeting bounds the start)
    #[tokio::test(flavor = "multi_thread")]
    async fn a_short_greeting_limit_ends_a_silent_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = mute(dir.path()).greeting(Duration::from_secs(1));
        let handle = runtime.handle();
        let root = Ctx::root().unwrap();
        let started = std::time::Instant::now();
        let view = root.plugin(runtime);
        tokio::time::timeout(Duration::from_secs(10), &view)
            .await
            .expect("down within the greeting limit")
            .unwrap_err();
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert!(
            matches!(handle.state(), RuntimeState::Down(why) if why == "the runtime did not greet within 1s"),
            "{:?}",
            handle.state()
        );
        root.shutdown().await.unwrap();
    }

    /// With `greeting(Duration::MAX)` a process that has not greeted yet (one
    /// stopped at its first line for a debugger) stays starting, past the
    /// session's own 10 s handshake; stopping it does not wait for it.
    // guarantee: host/plugin-limits (greeting(Duration::MAX) waits)
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unbounded_greeting_waits_for_a_silent_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = mute(dir.path()).greeting(Duration::MAX);
        let handle = runtime.handle();
        let root = Ctx::root().unwrap();
        let view = root.plugin(runtime);
        assert!(
            tokio::time::timeout(Duration::from_secs(12), &view)
                .await
                .is_err(),
            "still starting"
        );
        assert!(matches!(handle.state(), RuntimeState::Starting));
        tokio::time::timeout(Duration::from_secs(5), root.shutdown())
            .await
            .expect("stopped at once")
            .unwrap();
    }
}
