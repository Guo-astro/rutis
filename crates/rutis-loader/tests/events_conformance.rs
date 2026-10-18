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
//! and fails if any case fails. `RUTIS_EVENTS_CASE=<prefix>` runs only the
//! cases whose id starts with it.
//!
//! The `host` and `node` locations, `viaHost`, `hold` and the host's own
//! failure reports need the events-v2 Rust API (#464, events/kernel and
//! events/host); until it lands they are built only with
//! `RUSTFLAGS="--cfg rutis_events_v2"`, and without it the cases using them
//! fail saying so.
#![cfg(all(unix, feature = "node", feature = "python"))]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::runtimes::{repo, COLD};
use rutis::{BoxFuture, CordisError, Ctx, Effect, FiberState, FiberView, Plugin, PluginFactory};
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

/// Whether `language` can run here: Bun and Go need their features and
/// their programs on PATH.
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
        _ => false,
    }
}

fn python() -> String {
    std::env::var("RUTIS_PYTHON").unwrap_or_else(|_| "python3".into())
}

#[cfg(feature = "go")]
/// The Go fixture runtime (`go/rutis/internal/fixtures/runtime`, serving
/// `conformance-events` among its test plugins), built once.
fn go_binary() -> &'static Path {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let out = tempfile::tempdir().unwrap().keep();
        let binary = out.join("fixtures");
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

/// The plugins of `conformance-events` a runtime loads: `""` for listener
/// rows, `"e"` and `"f"` for the emitter rows of those labels (each
/// declares only its own `emitter-<label>`: a declared service is exported
/// whether or not the row provides it).
type Modules = BTreeMap<&'static str, String>;

/// A runtime of `language` named `name`, started through `sh` so that its
/// pid and stderr are recorded under `dir` (`kill` and `reported`); and
/// the modules its rows load.
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
    generation: u64,
}

struct Host {
    root: Ctx,
    loader: Loader,
    expected: events::Expected,
    anchors: Anchors,
    runtimes: BTreeMap<String, Runtime>,
    state: Mutex<State>,
    #[cfg(rutis_events_v2)]
    rust: rust_participant::Hooks,
    #[cfg(rutis_events_v2)]
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
        let root = Ctx::root().unwrap();
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
        #[cfg(rutis_events_v2)]
        let rust = rust_participant::register(&mut builtins, log.clone());

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
        #[cfg(rutis_events_v2)]
        let node = rust_participant::Node::start(&root, log.clone()).await;
        let host = Host {
            root,
            loader,
            expected,
            anchors,
            runtimes,
            state: Mutex::default(),
            #[cfg(rutis_events_v2)]
            rust,
            #[cfg(rutis_events_v2)]
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
            Some(json!({ "id": row.label, "name": name, "config": config }))
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

    async fn settled(&self, row: &Row) -> Result<(), String> {
        if row.at == Location::Node {
            return Ok(());
        }
        let fails = row.config.get("fails").is_some();
        let deadline = tokio::time::Instant::now() + COLD.0;
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
                return Err(format!("row {} did not start: {state:?}", row.label));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
        self.state.lock().unwrap().marks = marks;
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
        let status = std::process::Command::new("kill")
            .args(["-9", pid.trim()])
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("kill {name} ({})", pid.trim()));
        }
        // The runtime's fiber stays up for a restart; its rows wait.
        let pid = pid.trim().to_owned();
        let gone = || {
            !std::process::Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let rows: Vec<Row> = self
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|(row, _)| row.at == Location::Y)
            .map(|(row, _)| row.clone())
            .collect();
        let deadline = tokio::time::Instant::now() + COLD.0;
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
        runtime
            .view
            .restart()
            .await
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
                #[cfg(not(rutis_events_v2))]
                Location::Host | Location::Node => {
                    return Err(format!(
                        "{} at {}: the Rust participant needs the events-v2 Rust API (build with --cfg rutis_events_v2)",
                        row.label,
                        row.at.as_str()
                    ))
                }
                #[cfg(rutis_events_v2)]
                Location::Host => {}
                #[cfg(rutis_events_v2)]
                Location::Node => {
                    self.state.lock().unwrap().rows.push((row.clone(), 0));
                    return self.node.load(row).await;
                }
            }
            self.state.lock().unwrap().rows.push((row.clone(), 0));
            self.reconcile().await?;
            self.settled(row).await
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
            #[cfg(rutis_events_v2)]
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
            #[cfg(rutis_events_v2)]
            if row.at == Location::Node {
                return self.node.update(&row).await;
            }
            self.reconcile().await?;
            // Updated in place or restarted: wait until it runs again.
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.settled(&row).await
        })
    }

    fn emitter(&self, label: &str) -> Option<Arc<dyn HostDispatch>> {
        let state = self.state.lock().unwrap();
        let (row, _) = state.rows.iter().find(|(row, _)| row.label == label)?;
        #[cfg(rutis_events_v2)]
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
                #[cfg(rutis_events_v2)]
                Location::Node => self.node.kill().await?,
                _ => return Err("killing the node link needs the events-v2 Rust API".into()),
            }
            self.state.lock().unwrap().killed.insert(target);
            Ok(())
        })
    }

    fn restart<'a>(&'a self, target: Location) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match target {
                Location::Y => self.restart_y().await?,
                #[cfg(rutis_events_v2)]
                Location::Node => self.node.restart().await?,
                _ => return Err("restarting the node link needs the events-v2 Rust API".into()),
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
            #[cfg(rutis_events_v2)]
            self.node.reset().await?;
            self.reconcile().await?;
            for (row, _) in rows.iter().filter(|(row, _)| row.at != Location::Node) {
                until(
                    || self.row_state(&row.label, &row.scope).is_none(),
                    &format!("{} to unload", row.label),
                )
                .await?;
            }
            if killed.contains(&Location::Y) {
                self.restart_y().await?;
            }
            self.mark();
            Ok(())
        })
    }

    #[cfg(rutis_events_v2)]
    fn dispatches(&self) -> Option<u64> {
        Some(self.rust.dispatches())
    }

    #[cfg(rutis_events_v2)]
    fn hold(&self, n: usize) -> Result<(), String> {
        self.rust.hold(n)
    }

    fn reported(&self) -> Option<Vec<String>> {
        let marks = self.state.lock().unwrap().marks.clone();
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
        #[cfg(rutis_events_v2)]
        lines.extend(self.rust.reported());
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
        // The groups of the scope path: t1/s1 is [tenant, session].
        let mut groups = Vec::new();
        let mut path = String::new();
        for part in row.scope.split('/').filter(|p| *p != "root") {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            groups.push(json!(self.expected.scopes[&path]));
        }
        Some(json!({ "runtime": runtime, "node": node, "scope": groups }))
    }
}

/// Wait until `check` holds, at most [`COLD`]; a case's failure otherwise,
/// not the whole run's.
async fn until(check: impl Fn() -> bool, what: &str) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + COLD.0;
    while !check() {
        if tokio::time::Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

// ── The suite ───────────────────────────────────────────────────

async fn conformance(x: &str) {
    if !available(x) {
        eprintln!("{x} is not available here: skipped");
        return;
    }
    let log = EventsLog::new();
    let host = Host::start(x, &log).await;
    let expected = events::expected();
    let ys: Vec<&str> = events::y_languages(&expected, x)
        .into_iter()
        .filter(|l| available(l))
        .collect();
    let only = std::env::var("RUTIS_EVENTS_CASE").unwrap_or_default();
    let coverage = Coverage::from_env();
    let mut tally = Tally::default();
    for case in expected.cases.iter().filter(|c| c.case.starts_with(&only)) {
        for combo in events::placements(case, x, &ys, coverage).unwrap() {
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
    }
    println!("events conformance, X={x}:\n{tally}");
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
/// the host's bus. Built with `--cfg rutis_events_v2` until that API is on
/// the branch.
#[cfg(rutis_events_v2)]
mod rust_participant {
    include!("events_conformance/rust_participant.rs");
}
