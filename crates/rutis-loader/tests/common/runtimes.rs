//! What the tests of language runtime rows share: the probe plugins record
//! lines into, the checkout, the rows as a layer, a row's state, and the
//! helpers of the instance tests.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rutis::host::FiberState;
use rutis_bridge::session::{HostDispatch, Reply, Value as RpcValue};
use rutis_bridge::PeerId;
use rutis_loader::report::{EntryInfo, EntryStatus};
use rutis_loader::resolve::Build;
use rutis_loader::{Layer, Loader, Patch};
use rutis_test_support::{Guard, HANG_GUARD};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// Twice HANG_GUARD: language runtimes start cold (#238).
pub const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// This checkout.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// The host service `probe`: plugins `record` lines into it.
#[derive(Clone, Default)]
pub struct Probe(pub Arc<Mutex<Vec<String>>>);

impl HostDispatch for Probe {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        assert_eq!(method, "record");
        let [line]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "record": "sync" }))
    }
}

impl Probe {
    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    pub fn has(&self, line: &str) -> bool {
        self.0.lock().unwrap().iter().any(|l| l == line)
    }

    pub fn position(&self, line: &str) -> Option<usize> {
        self.0.lock().unwrap().iter().position(|l| l == line)
    }

    /// The lines recorded so far, forgotten.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    /// Wait until `line` is recorded.
    pub async fn wait_for(&self, line: &str) {
        COLD.eventually_with(
            || self.has(line).then_some(()),
            &format!("{line:?} recorded"),
            || format!("{:?}", self.lines()),
        )
        .await
    }
}

/// The host service `llm`: answers `ask` with 42.
pub struct Llm;

impl HostDispatch for Llm {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "ask");
        Ok(json!("42").into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "ask": "sync" }))
    }
}

/// `rows` inserted, as the one layer.
pub fn layer(rows: Vec<Value>) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

/// The state of the row `id`'s fiber, when it has one.
pub fn row_state(loader: &Loader, id: &str) -> Option<FiberState> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

/// The Go plugins' binary (`tests/fixtures/go`, `./cmd/multilang`), built
/// once.
pub fn go_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let out = tempfile::tempdir().unwrap().keep();
        let binary = out.join(if cfg!(windows) {
            "multilang.exe"
        } else {
            "multilang"
        });
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./cmd/multilang")
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build");
        binary
    })
}

/// The product `product` of the Swift package (the repository's root
/// `Package.swift`: `RutisMultilang`, `RutisPing`, `RutisConformance`),
/// built with the `swift` on PATH, copied once into a directory of its own
/// as `name`.
#[allow(dead_code)]
pub fn swift_binary(product: &str, name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let bin = BUILT.get_or_init(|| {
        let status = std::process::Command::new("swift")
            .args(["build"])
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    });
    let out = tempfile::tempdir().unwrap().keep().join(name);
    std::fs::copy(bin.join(product), &out).unwrap();
    out
}

// ── Instances ───────────────────────────────────────────────────

/// The instance's `tools`: tells its instance's title, and remembers the
/// rows that ask ([`tool_callers`]).
pub struct Tools(pub String);

/// `(title, row id, scope)` of every row that asked a `tools` its title.
static TOOL_CALLERS: Mutex<Vec<(String, String, Option<String>)>> = Mutex::new(Vec::new());

/// The rows that asked the `tools` titled `title`, as `(row id, scope)`.
#[allow(dead_code)]
pub fn tool_callers(title: &str) -> Vec<(String, Option<String>)> {
    TOOL_CALLERS
        .lock()
        .unwrap()
        .iter()
        .filter(|(asked, _, _)| asked == title)
        .map(|(_, id, scope)| (id.clone(), scope.clone()))
        .collect()
}

impl HostDispatch for Tools {
    fn invoke(&self, method: &str, _: RpcValue) -> Reply {
        assert_eq!(method, "title");
        if let Some(row) = rutis_bridge::session::caller_row() {
            let scope = row.scope().map(str::to_owned);
            TOOL_CALLERS
                .lock()
                .unwrap()
                .push((self.0.clone(), row.id().to_owned(), scope));
        }
        Ok(json!(self.0).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "title": "sync" }))
    }
}

/// The instance's title, given with `with`.
pub struct Title(pub String);

/// The title `build`'s instance was given.
pub fn title(build: &Build) -> String {
    build
        .value::<Title>()
        .map(|t| t.0.clone())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct NoConfig {}

pub fn active(entry: &EntryInfo) -> bool {
    matches!(&entry.status, EntryStatus::Running(s) if s.state == FiberState::Active)
}

// ── Kotlin ──────────────────────────────────────────────────────

/// The Kotlin SDK's conformance jar (`kotlin/rutis/rutis-conformance`,
/// Gradle task `fatJar`: the test plugins served by `Rutis.serve` on the
/// jvm target), built once with the Gradle wrapper, or the one
/// `RUTIS_KOTLIN_JAR` names.
#[cfg(feature = "kotlin")]
pub fn kotlin_jar() -> &'static Path {
    static JAR: OnceLock<PathBuf> = OnceLock::new();
    JAR.get_or_init(|| {
        if let Some(jar) = std::env::var_os("RUTIS_KOTLIN_JAR") {
            return PathBuf::from(jar);
        }
        let project = repo().join("kotlin/rutis");
        let wrapper = project.join(if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        });
        let status = std::process::Command::new(wrapper)
            .args(["--quiet", ":rutis-conformance:fatJar"])
            .current_dir(&project)
            .status()
            .expect("the Gradle wrapper runs (a JDK 17 on PATH or JAVA_HOME)");
        assert!(status.success(), "gradlew :rutis-conformance:fatJar");
        project.join("rutis-conformance/build/libs/rutis-conformance-all.jar")
    })
}

/// The Kotlin runtime serving the test plugins, as the generic launcher
/// starts it (design K-P1: no Kotlin runtime in rutis-host), in `project`.
#[cfg(feature = "kotlin")]
pub fn kotlin_launcher(project: &Path) -> rutis_bridge::runtime::Launcher {
    rutis_bridge::runtime::Launcher::new("java")
        .arg("-XX:TieredStopAtLevel=1")
        .arg("-XX:+UseSerialGC")
        .arg("-jar")
        .arg(kotlin_jar())
        .cwd(project)
}

/// Wait until the session of the local runtime `name` releases its rows
/// (`SessionRows`), its second stage mounted now. It does that from a task
/// of its own, and `reconcile` does not wait for rows still waiting for it:
/// without this wait a row whose plugin has already recorded a line from
/// its `apply` may still be `Loading` in rutis. Only for a runtime that
/// starts: the stage waits for it. `sessions` must be attached to a loader
/// that runs.
#[cfg(feature = "runtimes")]
pub async fn rows_stage(
    root: &rutis::Ctx,
    sessions: &rutis_loader::runtime::SessionResolver,
    name: &str,
) {
    use rutis::internal::CtxInternal;
    use rutis_loader::runtime::SessionRows;
    sessions.follow_row(name);
    let key = SessionRows::key(name);
    COLD.until(
        || root.get_key::<SessionRows>(key.clone()).is_some(),
        "the runtime's rows released",
    )
    .await;
}

/// The id sessions greet with (rutis-host provides its `id`): once on
/// `root`, before any runtime starts.
#[cfg(feature = "runtimes")]
pub fn greet_as_main(root: &rutis::Ctx) {
    root.provide(rutis_bridge::LocalId(id("main"))).unwrap();
}

/// A local runtime as rutis-host's `rutis-host/spawn` starts one: its
/// process (`Launcher::start`, `--id <name>`) the far end of a session, its
/// fiber the row `name`, under which the session provides its handle. Rows
/// `<name>:<plugin>` (`SessionResolver`) run on it. The root needs
/// [`greet_as_main`].
#[cfg(feature = "runtimes")]
pub struct TestRuntime {
    name: String,
    label: String,
    launcher: rutis_bridge::runtime::Launcher,
    anchor: PathBuf,
    states: rutis_bridge::runtime::RuntimeStates,
    processes: rutis_bridge::pipe::Processes,
    /// How long its process may take to greet (rutis-host's default).
    greeting: std::time::Duration,
}

#[cfg(feature = "runtimes")]
impl TestRuntime {
    pub fn new(name: &str, launcher: rutis_bridge::runtime::Launcher, anchor: &Path) -> Self {
        let processes = rutis_bridge::pipe::Processes::default();
        Self {
            name: name.to_owned(),
            label: format!("runtime {name}"),
            states: rutis_bridge::runtime::RuntimeStates::new(name, anchor)
                .processes(processes.clone()),
            launcher,
            anchor: anchor.to_owned(),
            processes,
            greeting: std::time::Duration::from_secs(30),
        }
    }

    /// How long its process may take to greet: 30 s, as rutis-host's;
    /// `Duration::MAX` for as long as it takes.
    pub fn greeting(mut self, limit: std::time::Duration) -> Self {
        self.greeting = limit;
        self
    }

    /// The Python runtime `py` of `project`, the repository's rutis
    /// package on its path.
    #[cfg(feature = "python")]
    pub fn python(project: &Path) -> Self {
        Self::new(
            "py",
            rutis_bridge::runtime::Launcher::python(project),
            project,
        )
        .python_path(repo().join("python/rutis"))
    }

    /// The Node runtime `node` of the repository's `node/rutis-runtime`,
    /// its plugins resolved from `anchor` (a `package.json`).
    #[cfg(feature = "node")]
    pub fn node(anchor: &Path) -> Self {
        Self::new(
            "node",
            rutis_bridge::runtime::Launcher::node(&repo().join("node/rutis-runtime")),
            anchor,
        )
    }

    /// The Bun runtime `bun` of the repository's `bun/rutis-bun`, the `bun`
    /// on PATH, in `project`.
    #[cfg(feature = "bun")]
    pub fn bun(project: &Path) -> Self {
        Self::new(
            "bun",
            rutis_bridge::runtime::Launcher::bun(None, &repo().join("bun/rutis-bun"), project),
            project,
        )
    }

    /// The runtime under another name (a Node runtime per plugin, say).
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self.label = format!("runtime {name}");
        self.states = rutis_bridge::runtime::RuntimeStates::new(name, &self.anchor)
            .processes(self.processes.clone());
        self
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
    pub fn launcher(
        mut self,
        change: impl FnOnce(rutis_bridge::runtime::Launcher) -> rutis_bridge::runtime::Launcher,
    ) -> Self {
        self.launcher = change(self.launcher);
        self
    }

    pub fn handle(&self) -> rutis_bridge::runtime::RuntimeHandle {
        self.states.handle()
    }

    pub fn processes(&self) -> rutis_bridge::pipe::Processes {
        self.processes.clone()
    }
}

/// What starts the compiled languages' runtimes (`BinaryRuntimes::new`):
/// a [`TestRuntime`] each.
#[cfg(feature = "binary")]
pub fn spawner() -> rutis_loader::runtime::binary::Spawn {
    Arc::new(|name: &str, launcher, anchor: &Path| {
        let runtime = TestRuntime::new(name, launcher, anchor);
        rutis_loader::runtime::binary::Spawned {
            handle: runtime.handle(),
            processes: runtime.processes(),
            plugin: Box::new(runtime),
        }
    })
}

#[cfg(feature = "runtimes")]
impl rutis::host::DynPlugin for TestRuntime {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(
        &'a self,
        ctx: &'a rutis::Ctx,
    ) -> rutis::host::BoxFuture<'a, Result<rutis::host::Effect, rutis::Error>> {
        use rutis::host::{CtxHost, Effect, ErrorKind, FiberState};
        use rutis::internal::CtxInternal;
        use rutis_bridge::runtime::RuntimeState;
        Box::pin(async move {
            let failed = |why: String| {
                self.states.failed(why.clone());
                rutis::Error::from(ErrorKind::PluginFailed(why.into()))
            };
            ctx.mark_row(&self.name)?;
            self.states.announce(ctx);
            self.states.starting();
            let peer = id(&self.name);
            let channel = tokio::select! {
                started = async {
                    let channel = self
                        .launcher
                        .start(&peer, &self.anchor, &self.processes)
                        .await
                        .map_err(|error| error.to_string())?;
                    rutis_bridge::pipe::greeted(channel, self.greeting).await
                } => started.map_err(failed)?,
                () = ctx.cancelled() => {
                    self.states.stopped();
                    return Ok(Effect::Done);
                }
            };
            let session = ctx.plugin(rutis_bridge::Session::new(channel));
            let mut watch = session.watch();
            loop {
                let snapshot = watch.borrow_and_update().clone();
                match snapshot.state {
                    FiberState::Active => break,
                    FiberState::Failed | FiberState::Disposed => {
                        return Err(failed(
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
                        return Err(failed("the runtime ended as it started".into()));
                    },
                    () = ctx.cancelled() => {
                        self.states.stopped();
                        return Ok(Effect::Done);
                    }
                }
            }
            let process = ctx
                .get_at(&rutis_bridge::session_key(&self.name))
                .and_then(|handle| handle.process())
                .ok_or_else(|| failed(format!("{} runs no plugins", self.name)))?;
            self.states.ready(process.clone());
            let (states, owner) = (self.states.clone(), ctx.clone());
            let watcher = tokio::spawn(async move {
                let mut watch = session.watch();
                let _ = watch
                    .wait_for(|s| matches!(s.state, FiberState::Failed | FiberState::Disposed))
                    .await;
                if !owner.cancellation_token().is_cancelled() {
                    states.failed(rutis_bridge::runtime::ended_status(&process));
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
