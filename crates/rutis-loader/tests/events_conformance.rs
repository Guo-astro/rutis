//! The unified events conformance suite on the real runtimes (#464 step 7):
//! every case of `events.expected.json`
//! (`rutis_bridge::runtime::testing::events`) in the placements
//! `RUTIS_EVENTS_PLACEMENTS` asks for (default `sampled`), with X each
//! runtime language in turn.
//!
//! One rutis host per language under test: a loader whose rows are the
//! participants, the runtime `rt-x` in X's language, one runtime `rt-y-<l>`
//! per language for Y (X's own language again among them), the Rust plugin
//! `conformance-events` for `host`, and a second node linked to it for
//! `node`. The scopes of the file are instances of the loader groups
//! `tenant` (t1), `session` (t1/s1, inside t1) and `team` (t2).
//!
//! Each test prints a table of every case with its first failing placement,
//! and fails if any case fails, or ran in no placement although it has
//! some for X (a language it needs is not available here).
//! `RUTIS_EVENTS_CASE=<prefix>[,<prefix>...]` runs only the cases whose id
//! starts with one of them (each must match a case).
//! `RUTIS_EVENTS_REQUIRE=<language>[,...]` (CI: `node,python,go,bun`; the
//! Kotlin job adds `kotlin`)
//! fails instead of skipping when one of them cannot run here.
//!
//! The suite runs one test at a time (each starts every runtime; a lock
//! keeps them from running side by side, whatever `--test-threads` says).
//! The runtimes are started through `sh` on Unix and through a small Node
//! script on Windows (`events_conformance/recorded.mjs`), which record
//! their pid and stderr.
//!
//! The `host` and `node` locations are the Rust plugin of
//! `events_conformance/rust_participant.rs`; `viaHost`, `hold` and the
//! host's own failure reports are the host bus's test hooks
//! (`rutis_bridge::events::testing::Tap`).
#![cfg(all(feature = "node", feature = "python"))]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::runtimes::{repo, COLD};
use rutis::{BoxFuture, CordisError, Ctx, Effect, FiberState, FiberView, Plugin, PluginFactory};
use rutis_bridge::events::testing::Tap;
use rutis_bridge::runtime::testing::events::{
    self, Coverage, EventsLog, EventsLogService, Location, Row, Tally, World,
};
use rutis_bridge::runtime::{Launcher, LocalRuntime};
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_loader::{
    Builtins, Chain, EntryStatus, Layer, Loader, LoaderOptions, LoaderPlugin, Patch,
    RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};
use serde_json::{json, Value as Json};

// ── Languages ───────────────────────────────────────────────────

/// Whether `language` can run here: Bun, Go and Kotlin need their features
/// and their programs on PATH (Kotlin: `java`).
fn available(language: &str) -> bool {
    let on_path = |program: &str| {
        std::process::Command::new(program)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    };
    match language {
        "node" | "node-leaf" | "python" => true,
        "bun" => cfg!(feature = "bun") && on_path("bun"),
        "go" => cfg!(feature = "go") && on_path("go"),
        "kotlin" => cfg!(feature = "kotlin") && on_path("java"),
        "swift" => cfg!(all(feature = "swift", target_os = "macos")) && on_path("swift"),
        _ => false,
    }
}

fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

#[cfg(feature = "go")]
/// The Go fixture runtime (`go/rutis/internal/fixtures/runtime`, serving
/// `conformance-events` among its test plugins), built once.
fn go_binary() -> &'static Path {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let out = tempfile::tempdir().unwrap().keep();
        let binary = out.join(format!("fixtures{}", std::env::consts::EXE_SUFFIX));
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./internal/fixtures/runtime")
            .current_dir(repo().join("go/rutis"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build ./internal/fixtures/runtime");
        binary
    })
}

#[cfg(all(feature = "swift", target_os = "macos"))]
/// The Swift fixture runtime (`swift/rutis/Sources/RutisConformance`,
/// serving `conformance-events` among its test plugins), built once.
fn swift_binary() -> &'static Path {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let product = "RutisConformance";
        let status = std::process::Command::new("swift")
            .args(["build", "--product", product])
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build --product {product}");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).join(product)
    })
}

/// The plugins of `conformance-events` a runtime loads: `""` for listener
/// rows, `"e"` and `"f"` for the emitter rows of those labels (each
/// declares only its own `emitter-<label>`: a declared service is exported
/// whether or not the row provides it).
type Modules = BTreeMap<&'static str, String>;

/// A runtime of `language` named `name`, started through [`recorded`] so
/// that its pid and stderr are recorded under `dir` (`kill` and
/// `reported`); and the modules its rows load.
fn runtime(language: &str, name: &str, dir: &Path) -> (LocalRuntime, Modules) {
    let fixtures = repo().join("node/rutis-runtime/test/fixtures");
    let path = |file: PathBuf| file.to_string_lossy().into_owned();
    let variants = ["", "e", "f"];
    let (launcher, anchor, modules): (Launcher, PathBuf, Modules) = match language {
        "node" | "node-leaf" => {
            let package = repo().join("node/rutis-runtime");
            let modules = variants
                .iter()
                .map(|v| {
                    let dash = if v.is_empty() {
                        String::new()
                    } else {
                        format!("-{v}")
                    };
                    let file = match language {
                        "node" => fixtures.join(format!("conformance-events{dash}/index.mjs")),
                        _ => fixtures.join(format!("conformance-events-leaf{dash}.mjs")),
                    };
                    (*v, path(file))
                })
                .collect();
            (
                Launcher::node(&package),
                package.join("package.json"),
                modules,
            )
        }
        "python" => {
            let project = repo().join("python/rutis/tests/fixtures");
            let search =
                std::env::join_paths([repo().join("python/rutis"), project.clone()]).unwrap();
            let mut launcher = Launcher::python(&project);
            launcher.program = python().into();
            launcher.env.retain(|(name, _)| name != "PYTHONPATH");
            launcher.env.push(("PYTHONPATH".into(), search));
            let modules = variants
                .iter()
                .map(|v| {
                    let under = if v.is_empty() {
                        String::new()
                    } else {
                        format!("_{v}")
                    };
                    (*v, format!("conformance_events{under}"))
                })
                .collect();
            (launcher, project, modules)
        }
        #[cfg(feature = "go")]
        "go" => {
            let project = dir.join(format!("{name}-project"));
            std::fs::create_dir_all(&project).unwrap();
            // One Go plugin provides `emitter-<label>` without declaring it
            // (its sync method on the wire: `dispatchSync`).
            let modules = variants
                .iter()
                .map(|v| (*v, "conformance-events".to_owned()))
                .collect();
            (Launcher::go(go_binary(), &project), project, modules)
        }
        // The Kotlin runtime (`Rutis.serve` on the jvm target), started by
        // the generic launcher; one plugin provides every emitter undeclared
        // (its sync method on the wire: `dispatchSync`), as in Go.
        #[cfg(feature = "kotlin")]
        "kotlin" => {
            let project = dir.join(format!("{name}-project"));
            std::fs::create_dir_all(&project).unwrap();
            let modules = variants
                .iter()
                .map(|v| (*v, "conformance-events".to_owned()))
                .collect();
            (
                common::runtimes::kotlin_launcher(&project),
                project,
                modules,
            )
        }
        #[cfg(all(feature = "swift", target_os = "macos"))]
        "swift" => {
            let project = dir.join(format!("{name}-project"));
            std::fs::create_dir_all(&project).unwrap();
            // As Go: one plugin provides `emitter-<label>` without declaring
            // it (its sync method on the wire: `dispatchSync`).
            let modules = variants
                .iter()
                .map(|v| (*v, "conformance-events".to_owned()))
                .collect();
            (Launcher::swift(swift_binary(), &project), project, modules)
        }
        #[cfg(feature = "bun")]
        "bun" => {
            let project = dir.join(format!("{name}-project"));
            std::fs::create_dir_all(&project).unwrap();
            std::fs::write(project.join("package.json"), "{}").unwrap();
            let modules = variants
                .iter()
                .map(|v| {
                    let dash = if v.is_empty() {
                        String::new()
                    } else {
                        format!("-{v}")
                    };
                    let file = format!("conformance-events{dash}.ts");
                    let entry = project.join(&file);
                    std::fs::copy(
                        repo().join("bun/rutis-bun/test/fixtures").join(&file),
                        &entry,
                    )
                    .unwrap_or_else(|e| panic!("{file}: {e}"));
                    (*v, path(entry))
                })
                .collect();
            let launcher = Launcher::bun(None, &repo().join("bun/rutis-bun"), &project);
            (launcher, project, modules)
        }
        other => panic!("no runtime language {other}"),
    };
    let recorded = recorded(
        launcher,
        &dir.join(format!("{name}.pid")),
        &dir.join(format!("{name}.stderr")),
    );
    let local =
        LocalRuntime::launcher(name, recorded, anchor).host("events-log", EventsLog::methods());
    (local, modules)
}

/// `launcher` run by `sh`, which writes its pid to `pid` and appends its
/// stderr to `stderr`, then becomes it (so the channel descriptor and the
/// pid stay the runtime's).
#[cfg(unix)]
fn recorded(launcher: Launcher, pid: &Path, stderr: &Path) -> Launcher {
    let mut wrapped = Launcher::new("sh")
        .arg("-c")
        .arg("echo $$ > \"$RUTIS_EVENTS_PID\"; exec \"$0\" \"$@\" 2>>\"$RUTIS_EVENTS_STDERR\"")
        .arg(launcher.program.clone());
    for arg in &launcher.args {
        wrapped = wrapped.arg(arg.clone());
    }
    wrapped.env = launcher.env.clone();
    wrapped.env.push(("RUTIS_EVENTS_PID".into(), pid.into()));
    wrapped
        .env
        .push(("RUTIS_EVENTS_STDERR".into(), stderr.into()));
    wrapped.cwd = launcher.cwd.clone();
    wrapped.inherit_fd = launcher.inherit_fd;
    wrapped.stdin = launcher.stdin;
    wrapped.stdout = launcher.stdout;
    wrapped.stderr = launcher.stderr;
    wrapped
}

/// `launcher` run by `events_conformance/recorded.mjs` with Node (Windows
/// has no `sh`): it starts the runtime with its stderr appended to
/// `stderr`, writes the runtime's pid to `pid`, and ends as it ends. The
/// channel is a loopback one here, reached through the environment, so
/// the runtime being a child of the script changes nothing for it.
#[cfg(windows)]
fn recorded(launcher: Launcher, pid: &Path, stderr: &Path) -> Launcher {
    let script = repo().join("crates/rutis-loader/tests/events_conformance/recorded.mjs");
    let mut wrapped = Launcher::new("node")
        .arg(script.into_os_string())
        .arg(pid.as_os_str().to_owned())
        .arg(stderr.as_os_str().to_owned())
        .arg(launcher.program.clone());
    for arg in &launcher.args {
        wrapped = wrapped.arg(arg.clone());
    }
    wrapped.env = launcher.env.clone();
    wrapped.cwd = launcher.cwd.clone();
    wrapped.inherit_fd = launcher.inherit_fd;
    wrapped.stdin = launcher.stdin;
    wrapped.stdout = launcher.stdout;
    wrapped.stderr = launcher.stderr;
    wrapped
}

/// End the process `pid` at once, as a crash would.
fn kill_now(pid: &str) -> Result<(), String> {
    let mut command = match cfg!(windows) {
        true => {
            let mut taskkill = std::process::Command::new("taskkill");
            taskkill.args(["/F", "/PID", pid]);
            taskkill
        }
        false => {
            let mut kill = std::process::Command::new("kill");
            kill.args(["-9", pid]);
            kill
        }
    };
    let status = command
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    match status.success() {
        true => Ok(()),
        false => Err(format!("could not kill {pid}: {status}")),
    }
}

/// Whether the process `pid` still runs.
fn alive(pid: &str) -> bool {
    if cfg!(windows) {
        let listed = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        return listed
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")));
    }
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// ── Scopes ──────────────────────────────────────────────────────

/// A Rust row in each scope, recording its context: where the world finds
/// that scope's services and creates the nested instance.
#[derive(Clone, Default)]
struct Anchors(Arc<Mutex<BTreeMap<String, Ctx>>>);

struct Anchor {
    path: String,
    anchors: Anchors,
}

impl Plugin for Anchor {
    fn name(&self) -> &str {
        "scope-anchor"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            self.anchors
                .0
                .lock()
                .unwrap()
                .insert(self.path.clone(), ctx.clone());
            let (anchors, path) = (self.anchors.clone(), self.path.clone());
            Ok(Effect::Disposer(Box::new(move || {
                anchors.0.lock().unwrap().remove(&path);
                Ok(())
            })))
        })
    }
}

struct AnchorFactory(Anchors);

impl PluginFactory<Json> for AnchorFactory {
    fn name(&self) -> &str {
        "scope-anchor"
    }

    fn build(&self, config: &Json) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(Anchor {
            path: config["path"].as_str().unwrap_or("root").to_owned(),
            anchors: self.0.clone(),
        }))
    }
}

/// The group of each scope path and where it nests, from the file.
fn groups(expected: &events::Expected) -> Vec<(String, String, Option<String>)> {
    expected
        .scopes
        .iter()
        .map(|(path, group)| {
            let parent = path.rsplit_once('/').map(|(parent, _)| parent.to_owned());
            (path.clone(), group.clone(), parent)
        })
        .collect()
}

// ── The host ────────────────────────────────────────────────────

/// A runtime of the host: its fiber, what its rows load, its records.
struct Runtime {
    view: FiberView,
    modules: Modules,
    pid: PathBuf,
    stderr: PathBuf,
}

#[derive(Default)]
struct State {
    /// The rows of the case, in the order they were loaded.
    rows: Vec<(Row, u64)>,
    /// Y's language in the case.
    y: String,
    killed: BTreeSet<Location>,
    /// Where each runtime's stderr was when the case started.
    marks: BTreeMap<String, u64>,
    /// How many lines the host's and the node's error sinks had reported
    /// when the case started.
    reported_marks: (usize, usize),
    generation: u64,
}

struct Host {
    root: Ctx,
    loader: Loader,
    expected: events::Expected,
    anchors: Anchors,
    runtimes: BTreeMap<String, Runtime>,
    state: Mutex<State>,
    rust: rust_participant::Hooks,
    node: rust_participant::Node,
    _dir: tempfile::TempDir,
}

/// The runtime a location of the case is in.
fn runtime_name(at: Location, y: &str) -> String {
    match at {
        Location::X => "rt-x".into(),
        _ => format!("rt-y-{y}"),
    }
}

impl Host {
    async fn start(x: &str, log: &Arc<EventsLog>) -> Host {
        let expected = events::expected();
        let dir = tempfile::tempdir().unwrap();
        // The host prints event failures as rutis-host does; the tap
        // records them and counts and holds the runtimes' dispatches.
        let tap = Tap::new();
        let print: rutis::ErrorSink =
            Arc::new(|error| match rutis_bridge::events::report_line(&error) {
                Some(line) => eprintln!("{line}"),
                None => rutis::default_sink()(error),
            });
        let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), tap.sink(print));
        tap.attach(&root).unwrap();
        root.provide_as::<dyn HostDispatch>(
            host_key("events-log"),
            Arc::new(EventsLogService(log.clone())),
        )
        .unwrap();
        let mut catalog = ServiceCatalog::new();
        for name in ["events-log", "emitter-e", "emitter-f", "zsvc"] {
            catalog.register_shared(name);
        }
        let anchors = Anchors::default();
        let mut builtins = Builtins::new();
        builtins.register::<Json, _>("scope-anchor", AnchorFactory(anchors.clone()));
        let rust = rust_participant::register(&mut builtins, log.clone(), tap);

        let mut chain = Chain::new().with(builtins);
        let mut resolvers = Vec::new();
        let mut runtimes = BTreeMap::new();
        let names = std::iter::once(("rt-x".to_owned(), x.to_owned())).chain(
            events::runtime_languages(&expected)
                .into_iter()
                .filter(|l| available(l))
                .map(|l| (format!("rt-y-{l}"), l.to_owned())),
        );
        for (name, language) in names {
            let (local, modules) = runtime(&language, &name, dir.path());
            let resolver =
                Arc::new(RuntimeResolver::modules(local.handle()).with_catalog(&catalog));
            let view = root.plugin(local);
            chain = chain.with_shared(resolver.clone());
            resolvers.push(resolver);
            runtimes.insert(
                name.clone(),
                Runtime {
                    view,
                    modules,
                    pid: dir.path().join(format!("{name}.pid")),
                    stderr: dir.path().join(format!("{name}.stderr")),
                },
            );
        }
        let plugin = LoaderPlugin::new(
            chain,
            LoaderOptions {
                catalog,
                ..LoaderOptions::default()
            },
        );
        let loader = plugin.handle();
        root.plugin(plugin).await.unwrap();
        for resolver in resolvers {
            root.plugin(RuntimeRowsPlugin::new(resolver));
        }
        let node = rust_participant::Node::start(&root, log.clone()).await;
        let host = Host {
            root,
            loader,
            expected,
            anchors,
            runtimes,
            state: Mutex::default(),
            rust,
            node,
            _dir: dir,
        };
        host.reconcile().await.expect("the scopes");
        // The instances, outermost first, each once.
        let mut created = BTreeSet::new();
        while created.len() < host.expected.scopes.len() {
            for (path, group, parent) in groups(&host.expected) {
                let parent_path = parent.clone().unwrap_or_else(|| "root".into());
                if created.contains(&path) || (parent.is_some() && !created.contains(&parent_path))
                {
                    continue;
                }
                let at = host.anchor(&parent_path).await;
                host.loader
                    .create_instance(&at, &group)
                    .await
                    .unwrap_or_else(|e| panic!("the instance {path}: {e}"));
                host.anchor(&path).await;
                created.insert(path);
            }
        }
        host.mark();
        host
    }

    async fn anchor(&self, path: &str) -> Ctx {
        COLD.eventually_with(
            || self.anchors.0.lock().unwrap().get(path).cloned(),
            &format!("the scope {path}"),
            || {
                format!(
                    "{:?}",
                    self.anchors.0.lock().unwrap().keys().collect::<Vec<_>>()
                )
            },
        )
        .await
    }

    /// The rows of the case and the scope anchors, as one layer.
    fn layer(&self) -> Vec<Layer> {
        let state = self.state.lock().unwrap();
        let row = |(row, generation): &(Row, u64)| -> Option<Json> {
            let name = match row.at {
                Location::X | Location::Y => {
                    let runtime = runtime_name(row.at, &state.y);
                    let variant = if row.config.get("emitter").is_some() {
                        row.label.as_str()
                    } else {
                        ""
                    };
                    format!(
                        "{runtime}:{}",
                        self.runtimes.get(&runtime)?.modules.get(variant)?
                    )
                }
                Location::Host => "conformance-events".into(),
                // On the node's own loader.
                Location::Node => return None,
            };
            let mut config = row.config.clone();
            if *generation > 0 {
                config["generation"] = json!(generation);
            }
            let mut entry = json!({ "id": row.label, "name": name, "config": config });
            // The row's own `inject` list, as a configuration gives it.
            if let Some(inject) = row.config.get("inject") {
                entry["inject"] = inject.clone();
            }
            Some(entry)
        };
        let in_scope = |path: &str| -> Vec<Json> {
            let mut rows = vec![json!({
                "id": format!("anchor-{}", path.replace('/', "-")),
                "name": "scope-anchor",
                "config": { "path": path },
            })];
            rows.extend(
                state
                    .rows
                    .iter()
                    .filter(|(r, _)| r.scope == path)
                    .filter_map(row),
            );
            rows
        };
        fn nest(
            path: &str,
            groups: &[(String, String, Option<String>)],
            in_scope: &dyn Fn(&str) -> Vec<Json>,
        ) -> Vec<Json> {
            let mut rows = in_scope(path);
            for (child, group, parent) in groups {
                if parent.as_deref().unwrap_or("root") == path {
                    rows.push(json!({
                        "id": group, "group": true, "instanced": true,
                        "config": nest(child, groups, in_scope),
                    }));
                }
            }
            rows
        }
        let rows = nest("root", &groups(&self.expected), &in_scope);
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
        vec![Layer::new("rows", patches)]
    }

    async fn reconcile(&self) -> Result<(), String> {
        self.loader
            .reconcile(self.layer(), None)
            .await
            .map(drop)
            .map_err(|error| format!("reconcile: {error}"))
    }

    /// The state of the row `label`'s fiber (its copy in its instance).
    fn row_state(&self, label: &str, scope: &str) -> Option<(FiberState, Option<String>)> {
        self.loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == label && entry.instance.is_some() == (scope != "root"))
            .find_map(|entry| match entry.status {
                EntryStatus::Running(snapshot) => {
                    Some((snapshot.state, snapshot.error.map(|e| e.to_string())))
                }
                _ => None,
            })
    }

    /// The fiber snapshot's generation of the row `label` (its copy in its
    /// instance), while it has one.
    fn row_generation(&self, label: &str, scope: &str) -> Option<u64> {
        self.loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == label && entry.instance.is_some() == (scope != "root"))
            .find_map(|entry| match entry.status {
                EntryStatus::Running(snapshot) => Some(snapshot.generation),
                _ => None,
            })
    }

    async fn settled(&self, row: &Row) -> Result<(), String> {
        if row.at == Location::Node {
            return Ok(());
        }
        let fails = row.config.get("fails").is_some();
        let deadline = tokio::time::Instant::now() + COLD.limit();
        loop {
            let state = self.row_state(&row.label, &row.scope);
            match &state {
                Some((FiberState::Active, _)) if !fails => return Ok(()),
                Some((FiberState::Failed, _)) if fails => return Ok(()),
                Some((FiberState::Failed, error)) => {
                    return Err(format!("row {} failed: {error:?}", row.label))
                }
                _ => {}
            }
            // A runtime that refused the host (or failed) loads nothing:
            // say so at once rather than at the deadline.
            if matches!(row.at, Location::X | Location::Y) {
                let name = runtime_name(row.at, &self.state.lock().unwrap().y);
                if let Some(runtime) = self.runtimes.get(&name) {
                    let state = runtime.view.state();
                    if state.state == FiberState::Failed {
                        return Err(format!("runtime {name} failed: {:?}", state.error));
                    }
                }
            }
            if tokio::time::Instant::now() > deadline {
                // Each runtime's state and the end of all it wrote.
                let runtimes: Vec<String> = self
                    .runtimes
                    .iter()
                    .map(|(name, runtime)| {
                        let text = std::fs::read_to_string(&runtime.stderr).unwrap_or_default();
                        let last: Vec<&str> = text.lines().rev().take(5).collect();
                        format!("{name} {:?} {last:?}", runtime.view.state().state)
                    })
                    .collect();
                return Err(format!(
                    "row {} did not start: {state:?} (runtimes: {})",
                    row.label,
                    runtimes.join(", ")
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The listener of `row` is on the other node's bus too, when it is
    /// shared there (a name the link shares, at the root): the link
    /// announces it after it registered, and the case goes on once it has,
    /// as it does once a row runs.
    async fn announced(&self, row: &Row) -> Result<(), String> {
        let Some(name) = row.config["name"].as_str() else {
            return Ok(());
        };
        let shared = name.starts_with("t/") || name.starts_with("rutis/");
        if !shared
            || row.scope != "root"
            || row.config.get("fails").is_some()
            || row.config.get("typed").is_some()
        {
            return Ok(());
        }
        let on_node = row.at == Location::Node;
        let Ok(node) = self.node.ctx() else {
            return Ok(()); // killed: nothing to announce to
        };
        // How many remote entries of `name` the far side must have: on the
        // host, the runtimes' and the node's; on the node, the host's own
        // at the root (the link's scope).
        let wanted = self
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|(r, _)| {
                r.config["name"].as_str() == Some(name)
                    && r.config.get("fails").is_none()
                    && match on_node {
                        true => r.at != Location::Host,
                        false => r.at != Location::Node && r.scope == "root",
                    }
            })
            .count();
        let far = if on_node { self.root.clone() } else { node };
        let imported = || {
            far.events()
                .subscriptions()
                .iter()
                .filter(|entry| entry.remote && entry.name == name)
                .count()
        };
        until(
            || imported() >= wanted,
            &format!("{name} of {} to reach the other node", row.label),
        )
        .await
    }

    /// Where every runtime's stderr is now: `reported` reads after it.
    fn mark(&self) {
        let marks = self
            .runtimes
            .iter()
            .map(|(name, runtime)| {
                let len = std::fs::metadata(&runtime.stderr)
                    .map(|m| m.len())
                    .unwrap_or(0);
                (name.clone(), len)
            })
            .collect();
        let reported_marks = (self.rust.reported().len(), self.node.reported().len());
        let mut state = self.state.lock().unwrap();
        state.marks = marks;
        state.reported_marks = reported_marks;
    }

    /// What the runtimes wrote on stderr since the case started, for a
    /// failure's record.
    fn stderr(&self) -> String {
        let marks = self.state.lock().unwrap().marks.clone();
        let mut out = String::new();
        for (name, runtime) in &self.runtimes {
            let text = std::fs::read(&runtime.stderr).unwrap_or_default();
            let from = (*marks.get(name).unwrap_or(&0) as usize).min(text.len());
            for line in String::from_utf8_lossy(&text[from..]).lines() {
                out.push_str(&format!("  {name}: {line}\n"));
            }
        }
        out
    }

    fn y_runtime(&self) -> String {
        runtime_name(Location::Y, &self.state.lock().unwrap().y)
    }

    async fn kill_y(&self) -> Result<(), String> {
        let name = self.y_runtime();
        let runtime = &self.runtimes[&name];
        let pid =
            std::fs::read_to_string(&runtime.pid).map_err(|e| format!("{name}'s pid: {e}"))?;
        let pid = pid.trim().to_owned();
        kill_now(&pid).map_err(|e| format!("kill {name}: {e}"))?;
        // The runtime's fiber stays up for a restart; its rows wait.
        let gone = || !alive(&pid);
        let rows: Vec<Row> = self
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|(row, _)| row.at == Location::Y)
            .map(|(row, _)| row.clone())
            .collect();
        let deadline = tokio::time::Instant::now() + COLD.limit();
        while !(gone()
            && rows.iter().all(|row| {
                !matches!(
                    self.row_state(&row.label, &row.scope),
                    Some((FiberState::Active, _))
                )
            }))
        {
            if tokio::time::Instant::now() > deadline {
                return Err(format!(
                    "{name} ({pid}) did not stop, or its rows still run"
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    async fn restart_y(&self) -> Result<(), String> {
        let name = self.y_runtime();
        let runtime = &self.runtimes[&name];
        tokio::time::timeout(COLD.limit(), runtime.view.restart())
            .await
            .map_err(|_| format!("restart {name}: did not finish within {:?}", COLD.limit()))?
            .map_err(|e| format!("restart {name}: {e}"))?;
        let rows: Vec<Row> = self
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|(row, _)| row.at == Location::Y)
            .map(|(row, _)| row.clone())
            .collect();
        for row in rows {
            self.settled(&row).await?;
        }
        Ok(())
    }
}

impl World for Host {
    fn load<'a>(&'a self, row: &'a Row) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match row.at {
                Location::X | Location::Y => {
                    let runtime = runtime_name(row.at, &self.state.lock().unwrap().y);
                    if !self.runtimes.contains_key(&runtime) {
                        return Err(format!("no runtime {runtime} here"));
                    }
                }
                Location::Host => {}
                Location::Node => {
                    self.state.lock().unwrap().rows.push((row.clone(), 0));
                    self.node.load(row).await?;
                    return self.announced(row).await;
                }
            }
            self.state.lock().unwrap().rows.push((row.clone(), 0));
            self.reconcile().await?;
            self.settled(row).await?;
            self.announced(row).await
        })
    }

    fn unload<'a>(&'a self, label: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let removed = {
                let mut state = self.state.lock().unwrap();
                let at = state.rows.iter().position(|(row, _)| row.label == label);
                at.map(|at| state.rows.remove(at).0)
            };
            let Some(row) = removed else {
                return Err(format!("no row {label}"));
            };
            if row.at == Location::Node {
                return self.node.unload(label).await;
            }
            self.reconcile().await?;
            until(
                || self.row_state(label, &row.scope).is_none(),
                &format!("{label} to unload"),
            )
            .await
        })
    }

    fn update<'a>(&'a self, label: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let row = {
                let mut state = self.state.lock().unwrap();
                state.generation += 1;
                let generation = state.generation;
                let entry = state
                    .rows
                    .iter_mut()
                    .find(|(row, _)| row.label == label)
                    .ok_or_else(|| format!("no row {label}"))?;
                entry.1 = generation;
                entry.0.clone()
            };
            if row.at == Location::Node {
                return self.node.update(&row).await;
            }
            // A config change restarts the row's fiber (the kernel's
            // `update`): it runs again once its snapshot moved past the one
            // it had, with the new config.
            let before = self.row_generation(&row.label, &row.scope);
            self.reconcile().await?;
            until(
                || {
                    self.row_generation(&row.label, &row.scope)
                        .is_some_and(|now| Some(now) > before)
                },
                &format!("{label} to start again with its new config"),
            )
            .await?;
            self.settled(&row).await
        })
    }

    fn emitter(&self, label: &str) -> Option<Arc<dyn HostDispatch>> {
        let state = self.state.lock().unwrap();
        let (row, _) = state.rows.iter().find(|(row, _)| row.label == label)?;
        if row.at == Location::Node {
            return self.node.emitter(label);
        }
        let ctx = self.anchors.0.lock().unwrap().get(&row.scope).cloned()?;
        ctx.get_as::<dyn HostDispatch>(host_key(&format!("emitter-{label}")))
    }

    fn kill<'a>(&'a self, target: Location) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match target {
                Location::Y => self.kill_y().await?,
                Location::Node => {
                    // The node's listeners leave the host's bus when the
                    // link's session ends, just after its calls in flight
                    // failed: wait for that, as for a runtime's rows.
                    let remote = || {
                        self.root
                            .events()
                            .subscriptions()
                            .iter()
                            .filter(|entry| entry.remote)
                            .count()
                    };
                    let on_node = self
                        .state
                        .lock()
                        .unwrap()
                        .rows
                        .iter()
                        .filter(|(row, _)| {
                            row.at == Location::Node && row.config.get("name").is_some()
                        })
                        .count();
                    let before = remote();
                    self.node.kill().await?;
                    until(
                        || remote() + on_node <= before,
                        "the node's listeners to leave the bus",
                    )
                    .await?;
                }
                at => return Err(format!("{} cannot be killed", at.as_str())),
            }
            self.state.lock().unwrap().killed.insert(target);
            Ok(())
        })
    }

    fn restart<'a>(&'a self, target: Location) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match target {
                Location::Y => self.restart_y().await?,
                Location::Node => self.node.restart().await?,
                at => return Err(format!("{} cannot be restarted", at.as_str())),
            }
            self.state.lock().unwrap().killed.remove(&target);
            Ok(())
        })
    }

    fn reset(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let (rows, killed) = {
                let mut state = self.state.lock().unwrap();
                state.generation = 0;
                (
                    std::mem::take(&mut state.rows),
                    std::mem::take(&mut state.killed),
                )
            };
            self.node.reset().await?;
            self.reconcile().await?;
            for (row, _) in rows.iter().filter(|(row, _)| row.at != Location::Node) {
                until(
                    || self.row_state(&row.label, &row.scope).is_none(),
                    &format!("{} to unload", row.label),
                )
                .await?;
            }
            // Gone from the other node too: a case counting the entries
            // the far side imported must not count the last case's.
            let stale = |ctx: &Ctx| {
                ctx.events()
                    .subscriptions()
                    .iter()
                    .any(|entry| entry.remote && entry.name.starts_with("t/"))
            };
            let node = self.node.ctx().ok();
            until(
                || !stale(&self.root) && !node.as_ref().is_some_and(stale),
                "the last case's listeners to leave both nodes",
            )
            .await?;
            if killed.contains(&Location::Y) {
                self.restart_y().await?;
            }
            self.mark();
            Ok(())
        })
    }

    fn dispatches(&self) -> Option<u64> {
        Some(self.rust.dispatches())
    }

    fn answered(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(self.rust.answered())
    }

    fn hold(&self, n: usize) -> Result<(), String> {
        self.rust.hold(n)
    }

    fn reported(&self) -> Option<Vec<String>> {
        let (marks, (host_mark, node_mark)) = {
            let state = self.state.lock().unwrap();
            (state.marks.clone(), state.reported_marks)
        };
        let mut lines = Vec::new();
        for (name, runtime) in &self.runtimes {
            let text = std::fs::read(&runtime.stderr).unwrap_or_default();
            let from = (*marks.get(name).unwrap_or(&0) as usize).min(text.len());
            lines.extend(
                String::from_utf8_lossy(&text[from..])
                    .lines()
                    .filter(|line| {
                        line.starts_with("rutis: listener of")
                            || line.starts_with("rutis: filter of")
                    })
                    .map(str::to_owned),
            );
        }
        // The host's and the node's sinks: what they reported in this case.
        lines.extend(self.rust.reported().into_iter().skip(host_mark));
        lines.extend(self.node.reported().into_iter().skip(node_mark));
        Some(lines)
    }

    fn description(&self, label: &str) -> Option<Json> {
        let state = self.state.lock().unwrap();
        let (row, _) = state.rows.iter().find(|(row, _)| row.label == label)?;
        let runtime = match row.at {
            Location::X | Location::Y => json!(runtime_name(row.at, &state.y)),
            _ => Json::Null,
        };
        let node = match row.at {
            Location::Node => json!("node"),
            _ => Json::Null,
        };
        // The chain of the row's scope, as its anchor there sees it: t1/s1
        // is [{id, group: tenant}, {id, group: session}].
        let chain: Vec<Json> = match row.scope.as_str() {
            "root" => Vec::new(),
            path => self
                .anchors
                .0
                .lock()
                .unwrap()
                .get(path)?
                .scope()
                .into_iter()
                .map(|scope| json!({ "id": scope.id, "group": scope.group }))
                .collect(),
        };
        let global = row.config["options"]["global"].as_bool().unwrap_or(false);
        Some(json!({
            "name": row.config["name"],
            "prefix": false,
            // The plugin's own name, the same in every language (the
            // Python fixture declares it).
            "plugin": "conformance-events",
            "row": label,
            "runtime": runtime,
            "node": node,
            "scope": chain,
            "global": global,
        }))
    }
}

/// Wait until `check` holds, at most [`COLD`]; a case's failure otherwise,
/// not the whole run's.
async fn until(check: impl Fn() -> bool, what: &str) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + COLD.limit();
    while !check() {
        if tokio::time::Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

// ── The suite ───────────────────────────────────────────────────

/// The languages `RUTIS_EVENTS_REQUIRE` names.
fn required() -> Vec<String> {
    std::env::var("RUTIS_EVENTS_REQUIRE")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// One test at a time: each starts every runtime, and the timings of the
/// cases (`settles`, the log's patience) assume a machine not running
/// another such host.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn conformance(x: &str) {
    let _one = ONE_AT_A_TIME.lock().await;
    let expected = events::expected();
    let required = required();
    let missing: Vec<&String> = required.iter().filter(|l| !available(l)).collect();
    assert!(
        missing.is_empty(),
        "RUTIS_EVENTS_REQUIRE names {missing:?}, which cannot run here (features, or the program on PATH)"
    );
    if !available(x) {
        eprintln!("{x} is not available here: skipped");
        return;
    }
    let started = std::time::Instant::now();
    let log = EventsLog::new();
    let host = Host::start(x, &log).await;
    let all_ys = events::y_languages(&expected, x);
    let ys: Vec<&str> = all_ys.iter().copied().filter(|l| available(l)).collect();
    let only = std::env::var("RUTIS_EVENTS_CASE").unwrap_or_default();
    let prefixes: Vec<&str> = only.split(',').map(str::trim).collect();
    for prefix in prefixes.iter().filter(|p| !p.is_empty()) {
        assert!(
            expected.cases.iter().any(|c| c.case.starts_with(prefix)),
            "RUTIS_EVENTS_CASE: no case starts with {prefix:?}"
        );
    }
    let coverage = Coverage::from_env();
    let mut tally = Tally::default();
    let mut placements = 0;
    for case in expected
        .cases
        .iter()
        .filter(|c| prefixes.iter().any(|prefix| c.case.starts_with(prefix)))
    {
        let started = std::time::Instant::now();
        let combos = events::placements(case, x, &ys, coverage).unwrap();
        if combos.is_empty() {
            // Not for X (its limits rule X out) is fine; a placement that
            // needs a language not here is a case that did not run.
            let anywhere = events::placements(case, x, &all_ys, coverage).unwrap();
            if !anywhere.is_empty() {
                tally.unrun(
                    case,
                    &format!(
                        "it ran in no placement: each needs a Y language not available here (available: {ys:?})"
                    ),
                );
            }
            continue;
        }
        for combo in combos {
            placements += 1;
            host.state.lock().unwrap().y = combo.y.clone();
            let result = events::run_keeping(&host, &log, case, &combo).await;
            if let Err(why) = &result {
                eprintln!(
                    "FAIL {} [{}]: {why}\n{}",
                    case.case,
                    combo.describe(),
                    host.stderr()
                );
            }
            if let Err(why) = events::finish(&host, &log).await {
                panic!("the world could not be reset after {}: {why}", case.case);
            }
            tally.record(case, &combo, &result);
        }
        // Where the time goes: the cases that wait.
        if started.elapsed() > Duration::from_secs(5) {
            eprintln!("X={x}: {} took {:?}", case.case, started.elapsed());
        }
    }
    println!(
        "events conformance, X={x}, {coverage:?}: {placements} placements in {:?}\n{tally}",
        started.elapsed()
    );
    host.root.shutdown().await.unwrap();
    assert_eq!(tally.failed(), 0, "X={x}:\n{tally}");
}

/// guarantee: events/one-bus (#464): every case of events.expected.json
/// with X a Node Cordis plugin.
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_cordis_plugin() {
    conformance("node").await;
}

/// guarantee: events/one-bus, X a Node leaf plugin.
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_node_leaf_plugin() {
    conformance("node-leaf").await;
}

/// guarantee: events/one-bus, X a Python plugin.
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_python_plugin() {
    conformance("python").await;
}

/// guarantee: events/one-bus, X a Go plugin.
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_go_plugin() {
    conformance("go").await;
}

/// guarantee: events/one-bus, X a Kotlin plugin (the SDK's jvm target).
#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_kotlin_plugin() {
    conformance("kotlin").await;
}

/// guarantee: events/one-bus, X a Swift plugin.
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_swift_plugin() {
    conformance("swift").await;
}

/// guarantee: events/one-bus, X a Bun plugin.
#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_bun_plugin() {
    conformance("bun").await;
}

// ── The Rust participant and the node (events-v2 API) ───────────

/// The Rust plugin `conformance-events` (location `host`, and `node` on
/// the second node), written against the events-v2 Rust API
/// (docs/design-events-protocol-2026-10-18.md §12.1), and the test hooks of
/// the host's bus.
mod rust_participant {
    include!("events_conformance/rust_participant.rs");
}
