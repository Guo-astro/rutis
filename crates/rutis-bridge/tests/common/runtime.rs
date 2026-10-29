//! A local runtime as rutis-host's `rutis-host/spawn` starts one, for the
//! bridge's tests (they cannot depend on rutis-host): its process
//! (`Launcher::start`, `--id <name>`) the far end of a session, its fiber
//! the row `name`, under which the session provides its handle. Mount it on
//! a root from [`root`] (or one given [`greet_as_main`]).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error};
use rutis_bridge::pipe::Processes;
use rutis_bridge::runtime::{ended_status, Launcher, RuntimeHandle, RuntimeState, RuntimeStates};
use rutis_bridge::{session_key, LocalId, PeerId, Session};

/// This checkout.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The id sessions greet with (rutis-host provides its `id`): once on
/// `root`, before any runtime starts.
pub fn greet_as_main(root: &Ctx) {
    root.provide(LocalId(PeerId::new("main").unwrap())).unwrap();
}

/// A root whose sessions greet as `main`.
pub fn root() -> Ctx {
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    root
}

/// One local runtime: what starts its process, and its state.
pub struct TestRuntime {
    row: String,
    label: String,
    launcher: Launcher,
    anchor: PathBuf,
    states: RuntimeStates,
    processes: Processes,
}

/// How long a runtime's process may take to greet: rutis-host's default.
const GREETING: std::time::Duration = std::time::Duration::from_secs(30);

impl TestRuntime {
    /// The runtime `name`, its process started with `launcher`, given
    /// `--id <name>` and `anchor` as its last arguments.
    pub fn new(name: &str, launcher: Launcher, anchor: &Path) -> Self {
        let processes = Processes::default();
        Self {
            row: name.to_owned(),
            label: format!("runtime {name}"),
            states: RuntimeStates::new(name, anchor).processes(processes.clone()),
            launcher,
            anchor: anchor.to_owned(),
            processes,
        }
    }

    /// The Node runtime `node` of the repository's `node/rutis-runtime`,
    /// its plugins resolved from that package.
    pub fn node() -> Self {
        let package = repo().join("node/rutis-runtime");
        Self::new(
            "node",
            launcher(rutis_test_support::runtimes::node_runtime()),
            &package.join("package.json"),
        )
    }

    /// The Python runtime `py` of `project`, the repository's rutis package
    /// on its path.
    pub fn python(project: &Path) -> Self {
        Self::new(
            "py",
            launcher(rutis_test_support::runtimes::python_runtime(project)),
            project,
        )
    }

    /// The Bun runtime `bun` of the repository's `bun/rutis-bun`, importing
    /// plugin modules from `project`.
    pub fn bun(project: &Path) -> Self {
        Self::new(
            "bun",
            launcher(rutis_test_support::runtimes::bun_runtime(project)),
            project,
        )
    }

    /// Put `directory` ahead on the process's `PYTHONPATH`.
    pub fn python_path(mut self, directory: impl Into<PathBuf>) -> Self {
        let env = &mut self.launcher.env;
        let current = env
            .iter()
            .find(|(name, _)| name == "PYTHONPATH")
            .map(|(_, current)| current.clone());
        let mut entries = vec![directory.into()];
        entries.extend(current.iter().flat_map(std::env::split_paths));
        let path = std::env::join_paths(&entries).unwrap();
        env.retain(|(name, _)| name != "PYTHONPATH");
        env.push(("PYTHONPATH".into(), path));
        self
    }

    /// Change the launcher (`stdout`, `env`, …).
    pub fn launcher(mut self, change: impl FnOnce(Launcher) -> Launcher) -> Self {
        self.launcher = change(self.launcher);
        self
    }

    pub fn handle(&self) -> RuntimeHandle {
        self.states.handle()
    }

    pub fn processes(&self) -> Processes {
        self.processes.clone()
    }

    fn failed(&self, why: String) -> Error {
        self.states.failed(why.clone());
        Error::from(ErrorKind::PluginFailed(why.into()))
    }
}

impl DynPlugin for TestRuntime {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.mark_row(&self.row)?;
            self.states.announce(ctx);
            self.states.starting();
            let id = PeerId::new(self.row.clone()).map_err(|e| self.failed(e.to_string()))?;
            let channel = tokio::select! {
                started = async {
                    let channel = self
                        .launcher
                        .start(&id, &self.anchor, &self.processes)
                        .await
                        .map_err(|error| error.to_string())?;
                    rutis_bridge::pipe::greeted(channel, GREETING).await
                } => started.map_err(|why| self.failed(why))?,
                () = ctx.cancelled() => {
                    self.states.stopped();
                    return Ok(Effect::Done);
                }
            };
            let session = ctx.plugin(Session::new(channel));
            let mut watch = session.watch();
            loop {
                let snapshot = watch.borrow_and_update().clone();
                match snapshot.state {
                    FiberState::Active => break,
                    FiberState::Failed | FiberState::Disposed => {
                        return Err(self.failed(
                            snapshot
                                .error
                                .map(|error| error.to_string())
                                .unwrap_or_else(|| "the runtime ended as it started".into()),
                        ));
                    }
                    _ => {}
                }
                tokio::select! {
                    changed = watch.changed() => if changed.is_err() {
                        return Err(self.failed("the runtime ended as it started".into()));
                    },
                    () = ctx.cancelled() => {
                        self.states.stopped();
                        return Ok(Effect::Done);
                    }
                }
            }
            let process = ctx
                .get_at(&session_key(&self.row))
                .and_then(|handle| handle.process())
                .ok_or_else(|| self.failed(format!("{} runs no plugins", self.row)))?;
            self.states.ready(process.clone());
            // The process ending on its own unloads the session: the runtime
            // is down, and says how it ended. Unloading this plugin is no end
            // of the runtime's.
            let (states, owner) = (self.states.clone(), ctx.clone());
            let watcher = tokio::spawn(async move {
                let mut watch = session.watch();
                let _ = watch
                    .wait_for(|s| matches!(s.state, FiberState::Failed | FiberState::Disposed))
                    .await;
                if !owner.cancellation_token().is_cancelled() {
                    states.failed(ended_status(&process));
                }
            });
            let states = self.states.clone();
            ctx.effect(move || async move {
                watcher.abort();
                if !matches!(states.handle().state(), RuntimeState::Down(_)) {
                    states.stopped();
                }
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// `command` (`rutis_test_support::runtimes`) as a launcher.
pub fn launcher(command: rutis_test_support::runtimes::Command) -> Launcher {
    let mut launcher = Launcher::new(command.program);
    launcher.args = command.args;
    launcher.env = command.env;
    launcher.cwd = command.cwd;
    launcher.inherit_fd = command.inherit_fd;
    launcher
}
