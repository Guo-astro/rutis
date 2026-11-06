//! guarantee: bridge/value-tags
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
//! some for X (a language it needs is not in `RUTIS_TEST_RUNTIMES`).
//! `RUTIS_EVENTS_CASE=<prefix>[,<prefix>...]` runs only the cases whose id
//! starts with one of them (each must match a case).
//! The languages are those `RUTIS_TEST_RUNTIMES` lists
//! (`rutis_test_support::runtimes`): X not listed, the test is skipped; a
//! listed language whose toolchain is missing fails it.
//!
//! The suite runs one test at a time (each starts every runtime; a lock
//! keeps them from running side by side, whatever `--test-threads` says).
//! The runtimes are started through `sh` on Unix and through a small Node
//! script on Windows (`events_conformance/recorded.mjs`), which record
//! their pid and stderr.
//!
//! The `host` and `node` locations are the Rust plugin of
//! `events_conformance/rust_participant.rs`; `hold` and the
//! host's own failure reports are the host bus's test hooks
//! (`rutis_bridge::events::testing::Tap`).
#![cfg(feature = "runtimes")]

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::runtimes::{greet_as_main, launcher, repo, rows_stage, TestRuntime, COLD};
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, FiberView, PluginFactory,
    Snapshot,
};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error};
use rutis_bridge::events::testing::Tap;
use rutis_bridge::runtime::testing::events::{
    self, Coverage, EventsLog, EventsLogService, Location, Row, Tally, World,
};
use rutis_bridge::runtime::Launcher;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_loader::report::{EntryStatus, RowEventState};
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{
    Builtins, Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, RowEvent, ServiceCatalog,
};
use rutis_test_support::Guard;
use serde_json::{json, Value as Json};

// ── Languages ───────────────────────────────────────────────────

/// Whether `language` runs here: listed in `RUTIS_TEST_RUNTIMES` (Swift:
/// on macOS only).
fn available(language: &str) -> bool {
    use rutis_test_support::runtimes::enabled;
    match language {
        "node" | "node-leaf" => enabled("node"),
        "python" | "bun" | "go" | "kotlin" => enabled(language),
        "swift" => cfg!(target_os = "macos") && enabled("swift"),
        _ => false,
    }
}

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

#[cfg(target_os = "macos")]
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
fn runtime(language: &str, name: &str, dir: &Path) -> (TestRuntime, Modules) {
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
                launcher(rutis_test_support::runtimes::node(&package)),
                package.join("package.json"),
                modules,
            )
        }
        "python" => {
            let project = repo().join("python/rutis/tests/fixtures");
            let launcher = launcher(rutis_test_support::runtimes::python_runtime(&project));
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
        "go" => {
            let project = dir.join(format!("{name}-project"));
            std::fs::create_dir_all(&project).unwrap();
            // One Go plugin provides `emitter-<label>` without declaring it
            // (its sync method on the wire: `dispatchSync`).
            let modules = variants
                .iter()
                .map(|v| (*v, "conformance-events".to_owned()))
                .collect();
            (
                launcher(rutis_test_support::runtimes::binary(go_binary(), &project)),
                project,
                modules,
            )
        }
        // The Kotlin runtime (`Rutis.serve` on the jvm target), started by
        // the generic launcher; one plugin provides every emitter undeclared
        // (its sync method on the wire: `dispatchSync`), as in Go.
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
        #[cfg(target_os = "macos")]
        "swift" => {
            let project = dir.join(format!("{name}-project"));
            std::fs::create_dir_all(&project).unwrap();
            // As Go: one plugin provides `emitter-<label>` without declaring
            // it (its sync method on the wire: `dispatchSync`).
            let modules = variants
                .iter()
                .map(|v| (*v, "conformance-events".to_owned()))
                .collect();
            (
                launcher(rutis_test_support::runtimes::binary(
                    swift_binary(),
                    &project,
                )),
                project,
                modules,
            )
        }
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
            let launcher = launcher(rutis_test_support::runtimes::bun_runtime(&project));
            (launcher, project, modules)
        }
        other => panic!("no runtime language {other}"),
    };
    let recorded = recorded(
        launcher,
        &dir.join(format!("{name}.pid")),
        &dir.join(format!("{name}.stderr")),
    );
    (TestRuntime::new(name, recorded, &anchor), modules)
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

impl DynPlugin for Anchor {
    fn name(&self) -> &str {
        "scope-anchor"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
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

    fn build(&self, config: &Json) -> Result<Box<dyn DynPlugin>, Error> {
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
    /// The last `rutis/row` state each row (its label, and whether it is at
    /// the root) was dispatched with, recorded by the kernel's test hook
    /// once that dispatch selected its listeners (#659). A step that loads,
    /// updates or restarts a row clears its record first.
    said: Said,
    _dir: tempfile::TempDir,
}

/// The last `rutis/row` state dispatched per row: by label and whether the
/// row is at the root (the scope is empty there), as `Host::row_state`
/// tells a row's copy apart.
#[derive(Clone, Default)]
struct Said(Arc<Mutex<HashMap<(String, bool), RowEventState>>>);

impl Said {
    fn key(row: &Row) -> (String, bool) {
        (row.label.clone(), row.scope == "root")
    }

    fn of(&self, row: &Row) -> Option<RowEventState> {
        self.0.lock().unwrap().get(&Self::key(row)).copied()
    }

    /// Before a step that makes the row run again: what it said before
    /// does not count.
    fn forget(&self, row: &Row) {
        self.0.lock().unwrap().remove(&Self::key(row));
    }
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
        // records them and holds the runtimes' dispatches.
        let tap = Tap::new();
        let print: rutis::host::ErrorSink =
            Arc::new(|error| match rutis_bridge::events::report_line(&error) {
                Some(line) => eprintln!("{line}"),
                None => rutis::host::default_sink()(error),
            });
        let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), tap.sink(print));
        tap.attach(&root).unwrap();
        // The loader emits a row's `rutis/row` from its fiber's status
        // changes, after the state is visible. The kernel's test hook runs
        // once a dispatch has selected its listeners, without adding an
        // entry to the bus (no case's table or routes change).
        let said = Said::default();
        {
            let said = said.clone();
            root.events().on_selected_for_testing(move |_, event| {
                if let Some(event) = event.downcast_ref::<RowEvent>() {
                    said.0
                        .lock()
                        .unwrap()
                        .insert((event.row.clone(), event.scope.is_empty()), event.state);
                }
            });
        }
        root.provide_at(
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
        builtins.register_raw(
            "scope-anchor",
            AnchorFactory(anchors.clone()),
            common::schema_of::<Json>(),
        );
        let rust = rust_participant::register(&mut builtins, log.clone(), tap);

        greet_as_main(&root);
        let sessions = Arc::new(SessionResolver::new(&catalog));
        let mut runtimes = BTreeMap::new();
        let names = std::iter::once(("rt-x".to_owned(), x.to_owned())).chain(
            events::runtime_languages(&expected)
                .into_iter()
                .filter(|l| available(l))
                .map(|l| (format!("rt-y-{l}"), l.to_owned())),
        );
        for (name, language) in names {
            let (local, modules) = runtime(&language, &name, dir.path());
            sessions.runtime(local.handle());
            let view = root.plugin(local);
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
            Chain::new().with(builtins).with_shared(sessions.clone()),
            LoaderOptions {
                catalog,
                ..LoaderOptions::default()
            },
        );
        let loader = plugin.handle();
        sessions.attach(&loader);
        root.plugin(plugin).await.unwrap();
        for name in runtimes.keys() {
            rows_stage(&root, &sessions, name).await;
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
            said,
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

    /// Wait until `row` runs (or failed as it should): its runtime first,
    /// then the row itself ([`row_settled`]).
    async fn settled(&self, row: &Row) -> Result<(), String> {
        if row.at == Location::Node {
            return Ok(());
        }
        let name = match row.at {
            Location::X | Location::Y => Some(runtime_name(row.at, &self.state.lock().unwrap().y)),
            _ => None,
        };
        let view = name
            .as_ref()
            .and_then(|name| self.runtimes.get(name))
            .map(|runtime| &runtime.view);
        // Its runtime first: rows run only on a runtime that is up, and a
        // runtime starts cold (a language VM, a compiled binary), which is
        // what takes the time on a loaded machine (#1003). Waiting for it
        // there is not taken out of the row's own window.
        if let (Some(view), Some(name)) = (view, name.as_ref()) {
            runtime_up(view, name, COLD, || self.runtimes_report()).await?;
        }
        row_settled(
            row,
            view.zip(name.as_deref()),
            || (self.row_state(&row.label, &row.scope), self.said.of(row)),
            COLD,
            || self.runtimes_report(),
        )
        .await
    }

    /// After `row` unloaded: its `stopped` has selected its listeners too,
    /// so no later step or case is in that dispatch. A row that never said
    /// anything has nothing to wait for.
    async fn said_stopped(&self, row: &Row) -> Result<(), String> {
        until(
            || matches!(self.said.of(row), None | Some(RowEventState::Stopped)),
            &format!("{}'s rutis/row stopped", row.label),
        )
        .await
    }

    /// The listener of `row` is on the other node's bus too, when it is at
    /// the root (one flat bus, every name): the session announces it after
    /// it registered, and the case goes on once it has, as it does once a
    /// row runs.
    async fn announced(&self, row: &Row) -> Result<(), String> {
        let Some(name) = row.config["name"].as_str() else {
            return Ok(());
        };
        if row.scope != "root"
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
        // at the root (the session's scope).
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

    /// What every runtime is doing and the end of what it wrote, for the
    /// summary a failure gives (#1003).
    fn runtimes_report(&self) -> String {
        let mut out = String::new();
        for (name, runtime) in &self.runtimes {
            out.push_str(&runtime_lines(name, &runtime.view.state(), &runtime.stderr));
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
        let rows: Vec<Row> = self
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|(row, _)| row.at == Location::Y)
            .map(|(row, _)| row.clone())
            .collect();
        for row in &rows {
            self.said.forget(row);
        }
        tokio::time::timeout(COLD.limit(), runtime.view.restart())
            .await
            .map_err(|_| format!("restart {name}: did not finish within {:?}", COLD.limit()))?
            .map_err(|e| format!("restart {name}: {e}"))?;
        for row in rows {
            self.settled(&row).await?;
        }
        Ok(())
    }
}

// ── Waiting for a row, and saying why it did not ────────────────

/// What a row shows at one moment: its fiber's state and error, and the
/// state of its last `rutis/row` ([`Said`]), `None` before any.
type Seen = (Option<(FiberState, Option<String>)>, Option<RowEventState>);

/// Wait for the runtime `name`'s row to come up, `guard` long: a runtime
/// starts cold (a language VM, a compiled binary, #238) and its rows run
/// only once it has greeted. Its fiber reaches [`FiberState::Active`] after
/// the runtime set its own `RuntimeState::Ready` (`TestRuntime`), so this
/// waits on the runtime's readiness, not on a fixed time: time spent here
/// is not taken out of the row's own window (#1003). A runtime that fails,
/// or does not come up within `guard`, is `report`'s failure to tell.
async fn runtime_up(
    view: &FiberView,
    name: &str,
    guard: Guard,
    report: impl Fn() -> String,
) -> Result<(), String> {
    let limit = guard.limit();
    let mut states = view.watch();
    let up = |snapshot: &Snapshot| {
        matches!(
            snapshot.state,
            FiberState::Active | FiberState::Failed | FiberState::Disposed
        )
    };
    let snapshot = match tokio::time::timeout(limit, states.wait_for(up)).await {
        Ok(Ok(snapshot)) => snapshot.clone(),
        // The runtime's fiber is gone: nothing will change its state again.
        Ok(Err(_)) => {
            return Err(format!(
                "runtime {name} went away before its rows could run\n{}",
                report()
            ))
        }
        Err(_) => {
            return Err(format!(
                "runtime {name} did not come up within {limit:?}\n{}",
                report()
            ))
        }
    };
    match snapshot.state {
        FiberState::Active => Ok(()),
        state => Err(format!(
            "runtime {name} is {state:?}: {}\n{}",
            snapshot
                .error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "no error".into()),
            report()
        )),
    }
}

/// Wait for `row` to settle, `guard` long: its fiber [`FiberState::Active`]
/// and its `rutis/row` `Running`, or `Failed` with `Failed` for a row that
/// fails on purpose. `saw` is what the row shows now, and `runtime` (with
/// its name) the runtime it waits on.
///
/// A runtime that fails loads nothing: the case hears of it at once, not at
/// the deadline. Both failures, and the deadline, say how far the row got
/// and what every runtime is doing, from `report` ([`row_failure`], #1003).
async fn row_settled(
    row: &Row,
    runtime: Option<(&FiberView, &str)>,
    saw: impl Fn() -> Seen,
    guard: Guard,
    report: impl Fn() -> String,
) -> Result<(), String> {
    let fails = row.config.get("fails").is_some();
    let limit = guard.limit();
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let (state, said) = saw();
        match &state {
            Some((FiberState::Active, _)) if !fails && said == Some(RowEventState::Running) => {
                return Ok(())
            }
            Some((FiberState::Failed, _)) if fails && said == Some(RowEventState::Failed) => {
                return Ok(())
            }
            // Failed as it should, its `failed` not out yet: wait.
            Some((FiberState::Failed, _)) if fails => {}
            // The row's own failure: the reason, and the runtimes, as the
            // deadline's failure gives them.
            Some((FiberState::Failed, _)) => {
                return Err(row_failure(row, "failed", &state, said, &report()))
            }
            _ => {}
        }
        if let Some((view, name)) = runtime {
            let state = view.state();
            if state.state == FiberState::Failed {
                return Err(format!(
                    "runtime {name} failed: {:?}\n{}",
                    state.error,
                    report()
                ));
            }
        }
        if tokio::time::Instant::now() > deadline {
            return Err(row_failure(
                row,
                &format!("did not start within {limit:?}"),
                &state,
                said,
                &report(),
            ));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Why `row` is not settled, `reason` after its label, in the shape a log
/// keeps (#1003): what it walks to, its error whole (it is the reason, so
/// not [`cut`]: a long one over several lines is still all there), its last
/// `rutis/row`, and then every runtime, a line each.
fn row_failure(
    row: &Row,
    reason: &str,
    state: &Option<(FiberState, Option<String>)>,
    said: Option<RowEventState>,
    report: &str,
) -> String {
    let mut out = format!("row {} {reason}\n", row.label);
    let (fiber, error) = match state {
        Some((state, error)) => (format!("{state:?}"), error.as_deref()),
        None => ("none".to_owned(), None),
    };
    out.push_str(&format!("  its fiber {fiber}\n"));
    if let Some(error) = error {
        for line in error.lines() {
            out.push_str(&format!("  its error: {line}\n"));
        }
    }
    let said = match said {
        Some(said) => format!("{said:?}"),
        None => "none".to_owned(),
    };
    out.push_str(&format!("  its last rutis/row {said}\n"));
    out.push_str(report);
    out
}

/// At most how long a line of a failure may be. A log keeps a failure's
/// lines only up to a width (the `(runtimes: …)` of #1003 ended at
/// `Some((Pending, No`), so a line here says one thing.
const WIDTH: usize = 80;

/// `text` at most [`WIDTH`] characters, cut with `…`.
fn cut(text: &str) -> String {
    match text.char_indices().nth(WIDTH) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// What runtime `name` is doing, then the end of what it wrote (`stderr`):
/// a line each, so that a log keeps them whole (#1003).
fn runtime_lines(name: &str, snapshot: &Snapshot, stderr: &Path) -> String {
    let mut out = format!(
        "  {name}: {}\n",
        cut(&format!("fiber {:?}", snapshot.state))
    );
    if let Some(error) = &snapshot.error {
        out.push_str(&format!(
            "  {name}: {}\n",
            cut(&format!("its error {error}"))
        ));
    }
    for line in tail(stderr, 5) {
        out.push_str(&format!("  {name}: | {line}\n"));
    }
    out
}

/// The last `n` lines of `stderr`, each at most [`WIDTH`] characters: the
/// end of what a runtime wrote, nothing while it wrote nothing.
fn tail(stderr: &Path, n: usize) -> Vec<String> {
    let text = std::fs::read(stderr).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|line| cut(line.trim_end()))
        .collect()
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
            self.said.forget(row);
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
            .await?;
            self.said_stopped(&row).await
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
            self.said.forget(&row);
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
        ctx.get_at::<dyn HostDispatch>(&host_key(&format!("emitter-{label}")))
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
                self.said_stopped(row).await?;
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

/// One test at a time: each starts every runtime, and the timings of the
/// cases (`settles`, the log's patience) assume a machine not running
/// another such host.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn conformance(x: &str) {
    let _one = ONE_AT_A_TIME.lock().await;
    let expected = events::expected();
    if !available(x) {
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
                        "it ran in no placement: each needs a Y language not in RUTIS_TEST_RUNTIMES (listed: {ys:?})"
                    ),
                );
            }
            continue;
        }
        for combo in combos {
            placements += 1;
            host.state.lock().unwrap().y = combo.y.clone();
            // A case that hangs fails here, naming itself and the
            // placement, instead of holding the job until the runner's own
            // timeout (which leaves the log with nothing to go on): no case
            // takes anywhere near a minute, and the guard only catches hangs.
            let what = format!("case {} [{}]", case.case, combo.describe());
            let result =
                rutis_test_support::Guard(rutis_test_support::scaled(Duration::from_secs(60)))
                    .within(&what, events::run_keeping(&host, &log, case, &combo))
                    .await;
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

/// guarantee: events/framework, events/one-bus (#464): every case of events.expected.json
/// with X a Node Cordis plugin.
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_cordis_plugin() {
    conformance("node").await;
}

/// guarantee: events/one-bus, X a Node leaf plugin., sdk/leaf-events
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
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_go_plugin() {
    conformance("go").await;
}

/// guarantee: events/one-bus, X a Kotlin plugin (the SDK's jvm target).
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_kotlin_plugin() {
    conformance("kotlin").await;
}

/// guarantee: events/one-bus, X a Swift plugin.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_swift_plugin() {
    conformance("swift").await;
}

/// guarantee: events/one-bus, X a Bun plugin.
#[tokio::test(flavor = "multi_thread")]
async fn events_with_x_a_bun_plugin() {
    conformance("bun").await;
}

// ── The waits of a case, and what a failure says (#1003) ────────

/// A runtime that takes its time to come up, as a language VM or a
/// compiled binary does on a loaded machine (#238, #1003).
struct SlowStart(Duration);

impl DynPlugin for SlowStart {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "a runtime starting cold"
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            tokio::time::sleep(self.0).await;
            Ok(Effect::Done)
        })
    }
}

/// A wait whose failure needs no summary of the runtimes (a test's own).
fn no_report() -> String {
    String::new()
}

/// The row `c` of a case, as a load of it gives one.
fn settled_row() -> Row {
    Row {
        label: "c".into(),
        at: Location::X,
        language: "node".into(),
        scope: "root".into(),
        config: json!({}),
    }
}

/// A runtime that starts slowly does not fail its row: the runtime is
/// waited for on its own, and the row's window starts after it (#1003). The
/// one window `settled` had before #1003 ran out: a loaded machine starts a
/// runtime in tens of seconds, and the row had not even been reached.
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_start_does_not_use_up_the_rows_guard() {
    // Both waits last `guard` here (times `RUTIS_TEST_TIME_SCALE` on a loaded
    // machine): the runtime takes four of those to come up, so the window a
    // row on its own gets runs out while it starts.
    let guard = Guard(Duration::from_millis(100));
    let window = guard.limit();
    let root = Ctx::root_with(tokio::runtime::Handle::current());
    let view = root.plugin(SlowStart(window * 4));
    let row = settled_row();
    let pending = || (Some((FiberState::Pending, None)), None);
    let alone = row_settled(&row, Some((&view, "rt-x")), pending, guard, no_report).await;
    assert_eq!(
        alone.unwrap_err().lines().next().unwrap(),
        format!("row c did not start within {window:?}")
    );
    // The runtime first (four windows, under its own guard), then the row in
    // its one window: the runtime's start is not taken out of the row's.
    runtime_up(&view, "rt-x", Guard(window * 8), no_report)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let runs = || {
        let up = started.elapsed() >= window / 5;
        let fiber = if up {
            FiberState::Active
        } else {
            FiberState::Pending
        };
        (Some((fiber, None)), up.then_some(RowEventState::Running))
    };
    row_settled(&row, Some((&view, "rt-x")), runs, guard, no_report)
        .await
        .unwrap();
}

/// A runtime that fails is the case's failure at once, with what it is and
/// what it wrote, not a row that quietly waits out its guard (#1003).
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_fails_is_named_at_once() {
    struct Refuses;

    impl DynPlugin for Refuses {
        fn platform(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "a runtime refusing the host"
        }

        fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move { Err(Error::from(ErrorKind::PluginFailed("refused".into()))) })
        }
    }

    let root = Ctx::root_with(tokio::runtime::Handle::current());
    let view = root.plugin(Refuses);
    let guard = Guard(Duration::from_secs(5));
    let why = runtime_up(&view, "rt-y-swift", guard, no_report)
        .await
        .unwrap_err();
    assert!(why.starts_with("runtime rt-y-swift is Failed"), "{why}");
    assert!(why.contains("refused"), "{why}");
}

/// A row that does not start says how far it got and what every runtime is
/// doing, in lines a log keeps whole: the `(runtimes: …)` of #1003 ended at
/// `Some((Pending, No`.
#[tokio::test(flavor = "multi_thread")]
async fn the_failure_names_every_runtime_and_its_last_words() {
    let dir = tempfile::tempdir().unwrap();
    let stderr = dir.path().join("rt-y-swift.stderr");
    std::fs::write(&stderr, format!("one\ntwo\n{}\n", "x".repeat(300))).unwrap();
    let root = Ctx::root_with(tokio::runtime::Handle::current());
    let view = root.plugin(SlowStart(Duration::from_secs(30)));
    let report = || {
        runtime_lines(
            "rt-y-swift",
            &Snapshot {
                generation: 0,
                state: FiberState::Pending,
                error: Some(std::sync::Arc::new(ErrorKind::PluginFailed(
                    "the runtime refused: ".repeat(20).into(),
                ))),
            },
            &stderr,
        )
    };
    let mut row = settled_row();
    row.at = Location::Y;
    let guard = Guard(Duration::from_millis(50));
    let why = row_settled(
        &row,
        Some((&view, "rt-y-swift")),
        || (Some((FiberState::Pending, None)), None),
        guard,
        report,
    )
    .await
    .unwrap_err();
    // The row, then what it saw, then every runtime: a line each.
    assert!(
        why.starts_with(format!("row c did not start within {:?}\n", guard.limit()).as_str()),
        "{why}"
    );
    assert!(why.contains("\n  its fiber Pending\n"), "{why}");
    assert!(why.contains("\n  its last rutis/row none\n"), "{why}");
    assert!(why.contains("\n  rt-y-swift: fiber Pending\n"), "{why}");
    assert!(
        why.contains("\n  rt-y-swift: its error the runtime refused"),
        "{why}"
    );
    assert!(why.contains("\n  rt-y-swift: | one\n"), "{why}");
    assert!(why.contains("\n  rt-y-swift: | two\n"), "{why}");
    // Every line within what a log keeps (the width of #1003's cut), the
    // long ones cut with `…`.
    for line in why.lines() {
        assert!(line.chars().count() <= 120, "{line}");
    }
    assert!(
        why.lines().filter(|line| line.ends_with('…')).count() == 2,
        "{why}"
    );
}

/// A row that failed for real (not a `fails` row) is the case's failure at
/// once: the reason, whole, and every runtime, as the deadline's failure
/// gives them (#1003).
#[tokio::test(flavor = "multi_thread")]
async fn a_row_that_failed_says_why_and_what_every_runtime_is_doing() {
    let dir = tempfile::tempdir().unwrap();
    let stderr = dir.path().join("rt-x.stderr");
    std::fs::write(&stderr, "loading t/keeps\n").unwrap();
    let root = Ctx::root_with(tokio::runtime::Handle::current());
    let view = root.plugin(SlowStart(Duration::from_secs(30)));
    let row = settled_row();
    let report = || {
        runtime_lines(
            "rt-x",
            &Snapshot {
                generation: 0,
                state: FiberState::Active,
                error: None,
            },
            &stderr,
        )
    };
    let started = std::time::Instant::now();
    let why = row_settled(
        &row,
        Some((&view, "rt-x")),
        || {
            (
                Some((FiberState::Failed, Some("no such plugin t/keeps".into()))),
                None,
            )
        },
        Guard(Duration::from_secs(30)),
        report,
    )
    .await
    .unwrap_err();
    // Not at the guard: the failure is there and is the case's.
    assert!(started.elapsed() < Duration::from_secs(5), "{why}");
    // The row, its reason, then every runtime: a line each.
    assert_eq!(why.lines().next().unwrap(), "row c failed");
    assert!(why.contains("\n  its fiber Failed\n"), "{why}");
    assert!(
        why.contains("\n  its error: no such plugin t/keeps\n"),
        "{why}"
    );
    assert!(why.contains("\n  its last rutis/row none\n"), "{why}");
    assert!(why.contains("\n  rt-x: fiber Active\n"), "{why}");
    assert!(why.contains("\n  rt-x: | loading t/keeps\n"), "{why}");
}

/// The reason of a failure is whole, however long and however many lines:
/// it is what went wrong, so it is not cut (`cut` is for the report's lines,
/// #1003).
#[tokio::test(flavor = "multi_thread")]
async fn a_long_reason_over_several_lines_is_not_cut() {
    let long = "the runtime refused the module: ".repeat(6);
    assert!(long.chars().count() > WIDTH * 2, "{long}");
    let reason = format!("first line\n{long}\nlast line");
    let seen = reason.clone();
    let why = row_settled(
        &settled_row(),
        None,
        || (Some((FiberState::Failed, Some(seen.clone()))), None),
        Guard(Duration::from_secs(30)),
        no_report,
    )
    .await
    .unwrap_err();
    assert_eq!(
        why,
        format!(
            "row c failed\n  its fiber Failed\n  its error: first line\n  its error: {long}\n  its error: last line\n  its last rutis/row none\n"
        )
    );
}

// ── The Rust participant and the node (events-v2 API) ───────────

/// The Rust plugin `conformance-events` (location `host`, and `node` on
/// the second node), written against the events-v2 Rust API
/// (docs/design-events-protocol-2026-10-18.md §12.1), and the test hooks of
/// the host's bus.
mod rust_participant {
    include!("events_conformance/rust_participant.rs");
}
