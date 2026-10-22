//! The Rust application of E2E scenario S7 (#191): a multi-tenant service
//! that embeds rutis as `docs/guide/rust-host.md` says (crates, not
//! rutis-host), with one instance subtree per open tenant.
//!
//! ```text
//! rutis-e2e-fixture-tenants [project]
//! ```
//!
//! In `project` (the current directory by default):
//!
//! - `app/rows.json`: the application's rows, a patch list (layer `app`):
//!   global rows, and the instanced group `tenant` whose rows run once per
//!   tenant;
//! - `state/edits.json`: the admin layer (layer `admin`), the loader's
//!   editable layer, stored by [`LayerFile`] after every edit and read at
//!   start. Each tenant has a global row `cfg-<tenant>` there (the Rust
//!   plugin `app/tenant-config`: `{ tenant, greeting, quota }`), disabled
//!   while the tenant is closed. It is all the application stores: at start
//!   it opens again every tenant whose row is enabled;
//! - `plugins/`: the Python modules (`py:<module>` rows) and the TS files the
//!   rows name; `package.json`: the Node runtime's anchor.
//!
//! The Rust rows: `app/tenant-config` provides the tenant's configuration
//! (refusing a quota of 0 when it starts, which no validation catches
//! earlier); inside a tenant, `app/settings` provides the instance service
//! `settings` from it, and `app/caller` uses the tenant's `greeter` and
//! `py_greeter` and is the application's door into the tenant: calls "from
//! the tenant" go through its context. Instance services (`settings`,
//! `greeter`, `py_greeter`) are declared per instance of `tenant`; `words`
//! is global.
//!
//! The Node runtime is `RUTIS_NODE_RUNTIME`; `RUTIS_PYTHON_PATH`, when set,
//! goes ahead on the Python runtime's path. With `RUTIS_TEST_INSPECT` set
//! the runtimes answer `inspect`; `RUTIS_TEST_ROWS_LATE_MS` releases the
//! runtimes' rows that much later (test support).
//!
//! Commands on stdin, one per line; each is answered with one JSON line
//! `{"answer": "<command>", …}` on stdout, with `"error"` when it failed.
//! Once started, its global rows too, it prints `{"answer": "ready",
//! "restored": [..], "rows": {..}}`.
//!
//! - `create <tenant> [greeting]`: add or enable `cfg-<tenant>`, then create
//!   the tenant's instance; answers once its door is open;
//! - `close <tenant>`: remove the instance, then disable `cfg-<tenant>`;
//! - `edit <tenant> <json>`: replace `cfg-<tenant>`'s config (an admin
//!   edit), all or nothing;
//! - `call <tenant> <service> <method> <json args>`: call the tenant's
//!   instance service `service` from inside the tenant;
//! - `peek <from> <of> <service>`: whether `of`'s instance service is
//!   visible from `from` (a tenant, or `-` for the application's root);
//! - `state <tenant>`: the tenant's state (config, rows, settings, answers),
//!   with what identifies the running copies apart (`identity`);
//! - `inspect`: what rutis, the loader and the runtimes hold;
//! - `stop`, or the end of stdin: shut down, exit 0 (2 when the shutdown
//!   did not finish).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::{
    BoxFuture, CordisError, Ctx, Effect, FiberState, InstanceId, Plugin, PluginFactory, TypeKey,
};
use rutis_bridge::runtime::{LocalRuntime, RuntimeHandle, Stdio};
use rutis_bridge::session::{
    host_key_in, settle, Error as CallError, HostDispatch, Reply, Value as CallValue,
};
use rutis_loader::{
    Build, Builtins, Chain, Edit, Editable, EntryStatus, Instance, InstanceResult, Layer, Loader,
    LoaderError, LoaderOptions, LoaderPlugin, NewEntry, Patch, Persist, PersistError,
    RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog, Version,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// How long the shutdown may take.
const SHUTDOWN: Duration = Duration::from_secs(10);
/// How long a tenant's door may take to open after its rows settled.
const DOOR: Duration = Duration::from_secs(60);
/// How long the global rows may take to start after `reconcile` returned.
const STARTED: Duration = Duration::from_secs(60);
/// The instanced group of a tenant's rows.
const TENANT: &str = "tenant";
/// The admin layer, which edits change.
const ADMIN: &str = "admin";

fn main() {
    let project = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the async runtime");
    let code = runtime.block_on(run(&project));
    drop(runtime);
    std::process::exit(code);
}

async fn run(project: &Path) -> i32 {
    let app = match App::start(project).await {
        Ok(app) => app,
        Err((root, error)) => {
            eprintln!("fixture-tenants: {error}");
            if let Some(root) = root {
                stop(&root).await;
            }
            return 1;
        }
    };
    // Commands from stdin; the channel closes with it.
    let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if commands.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = received.recv().await {
        let line = line.trim();
        let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
        let answer = match command {
            "" => continue,
            "stop" => break,
            "create" => app.create(rest).await,
            "close" => app.close(rest.trim()).await,
            "edit" => app.edit(rest).await,
            "call" => app.call(rest).await,
            "peek" => app.peek(rest),
            "state" => app.state(rest.trim()).await,
            "inspect" => app.inspect().await,
            other => Err(format!("unknown command {other:?}")),
        };
        let mut answer = match answer {
            Ok(Value::Object(fields)) => Value::Object(fields),
            Ok(other) => json!({ "ok": other }),
            Err(error) => json!({ "error": error }),
        };
        answer["answer"] = json!(command);
        say(answer);
    }
    stop(&app.root).await
}

/// One line on stdout: the `answer` field first, so the harness finds it.
fn say(mut answer: Value) {
    let name = answer
        .as_object_mut()
        .and_then(|fields| fields.remove("answer"))
        .unwrap_or(Value::Null);
    let rest = serde_json::to_string(&answer).unwrap_or_default();
    let rest = rest.strip_prefix('{').unwrap_or("}");
    let separator = if rest == "}" { "" } else { "," };
    println!("{{\"answer\":{name}{separator}{rest}");
}

/// Shut down; the exit code.
async fn stop(root: &Ctx) -> i32 {
    match tokio::time::timeout(SHUTDOWN, root.shutdown()).await {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("fixture-tenants: shutdown: {error}");
            2
        }
        Err(_) => {
            eprintln!("fixture-tenants: the shutdown did not finish within {SHUTDOWN:?}");
            2
        }
    }
}

// ── The application ─────────────────────────────────────────────

struct App {
    root: Ctx,
    loader: Loader,
    node: RuntimeHandle,
    python: RuntimeHandle,
    /// The open tenants' instances.
    open: Mutex<BTreeMap<String, Instance>>,
    /// Every tenant's last instance id, open or not (for `peek`).
    seen: Mutex<HashMap<String, InstanceId>>,
    doors: Doors,
}

type Started = Result<App, (Option<Ctx>, String)>;

impl App {
    async fn start(project: &Path) -> Started {
        let base = read_patches(&project.join("app/rows.json")).map_err(|e| (None, e))?;
        let store = Arc::new(LayerFile {
            path: project.join("state/edits.json"),
        });
        let root = Ctx::root().map_err(|e| (None, e.to_string()))?;
        let failed = |root: &Ctx, error: String| (Some(root.clone()), error);

        let mut catalog = ServiceCatalog::new();
        catalog.register_shared("words");
        for name in ["settings", "greeter", "py_greeter"] {
            catalog.register_shared_instance(name, TENANT);
        }
        let node_package = std::env::var_os("RUTIS_NODE_RUNTIME")
            .map(PathBuf::from)
            .ok_or_else(|| failed(&root, "RUTIS_NODE_RUNTIME is not set".into()))?;
        // Only this program writes to stdout: its answers are whole lines.
        let node =
            LocalRuntime::node(node_package, project.join("package.json")).stdout(Stdio::Null);
        let mut python = LocalRuntime::python(project.join("plugins")).stdout(Stdio::Null);
        if let Some(source) = std::env::var_os("RUTIS_PYTHON_PATH") {
            python = python.python_path(source);
        }
        let (node_handle, python_handle) = (node.handle(), python.handle());
        let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
        let python_rows =
            Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
        root.plugin(node);
        root.plugin(python);

        let doors = Doors::default();
        let chain = Chain::new()
            .with(builtins(doors.clone()))
            .with_shared(python_rows.clone())
            .with_shared(node_rows.clone());
        let plugin = LoaderPlugin::new(
            chain,
            LoaderOptions {
                persist: store.clone(),
                catalog,
                ..LoaderOptions::default()
            },
        );
        let loader = plugin.handle();
        root.plugin(plugin)
            .await
            .map_err(|e| failed(&root, format!("the loader: {e}")))?;
        // `RUTIS_TEST_ROWS_LATE_MS`: the runtimes' rows are released that
        // much later, as on a loaded machine (test support, #568).
        let late = std::env::var("RUTIS_TEST_ROWS_LATE_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map(Duration::from_millis);
        match late {
            None => {
                root.plugin(RuntimeRowsPlugin::new(node_rows));
                root.plugin(RuntimeRowsPlugin::new(python_rows));
            }
            Some(late) => {
                let root = root.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(late).await;
                    root.plugin(RuntimeRowsPlugin::new(node_rows));
                    root.plugin(RuntimeRowsPlugin::new(python_rows));
                });
            }
        }

        let (edits, version) = store
            .load(ADMIN)
            .await
            .map_err(|e| failed(&root, e.to_string()))?;
        let report = loader
            .reconcile(
                vec![Layer::new("app", base), Layer::new(ADMIN, edits)],
                Some(Editable::new(ADMIN, version)),
            )
            .await
            .map_err(|e| failed(&root, e.to_string()))?;
        if !report.failures.is_empty() {
            let failures: Vec<String> = report.failures.iter().map(|f| f.to_string()).collect();
            return Err(failed(&root, failures.join("; ")));
        }
        let app = App {
            root: root.clone(),
            loader,
            node: node_handle,
            python: python_handle,
            open: Mutex::default(),
            seen: Mutex::default(),
            doors,
        };
        // The tenants open when the application stopped: their rows are
        // enabled in the admin layer.
        let mut restored = Vec::new();
        for entry in app.loader.entries() {
            let Some(tenant) = entry.id.strip_prefix("cfg-") else {
                continue;
            };
            if entry.instance.is_some() || matches!(entry.status, EntryStatus::Disabled) {
                continue;
            }
            app.open_tenant(tenant)
                .await
                .map_err(|e| failed(&root, format!("tenant {tenant}: {e}")))?;
            restored.push(tenant.to_owned());
        }
        started(&app.loader).await.map_err(|e| failed(&root, e))?;
        let rows: BTreeMap<String, String> = app
            .loader
            .entries()
            .into_iter()
            .filter(|entry| entry.instance.is_none())
            .map(|entry| (entry.id, status(&entry.status)))
            .collect();
        say(json!({
            "answer": "ready",
            "pid": std::process::id(),
            "restored": restored,
            "rows": rows,
        }));
        Ok(app)
    }

    /// `create <tenant> [greeting]`.
    async fn create(&self, args: &str) -> Result<Value, String> {
        let mut words = args.split_whitespace();
        let tenant = words.next().ok_or("create <tenant> [greeting]")?;
        let greeting = words.next().unwrap_or("Hello");
        if self.open.lock().unwrap().contains_key(tenant) {
            return Err(format!("tenant {tenant} is open"));
        }
        let id = format!("cfg-{tenant}");
        match self.loader.get(&id) {
            None => {
                let entry = NewEntry {
                    id: Some(id),
                    name: "app/tenant-config".into(),
                    config: json!({ "tenant": tenant, "greeting": greeting, "quota": 1 }),
                    ..NewEntry::default()
                };
                self.loader
                    .create(entry, None, None)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Some(entry) if matches!(entry.status, EntryStatus::Disabled) => {
                self.loader
                    .set_disabled(&id, false)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Some(_) => {}
        }
        let report = self.open_tenant(tenant).await?;
        Ok(json!({ "tenant": tenant, "report": report }))
    }

    /// Create `tenant`'s instance and wait for its door: the rows' results.
    async fn open_tenant(&self, tenant: &str) -> Result<Value, String> {
        let instance = self
            .loader
            .create_instance(&self.root, TENANT)
            .with(Tenant(tenant.to_owned()))
            .await
            .map_err(|e| e.to_string())?;
        let report: BTreeMap<String, String> = instance
            .report
            .iter()
            .map(|(row, result)| (row.clone(), result_of(result)))
            .collect();
        self.seen
            .lock()
            .unwrap()
            .insert(tenant.to_owned(), instance.view.instance());
        self.open
            .lock()
            .unwrap()
            .insert(tenant.to_owned(), instance);
        self.doors.wait(tenant).await?;
        Ok(json!(report))
    }

    /// `close <tenant>`.
    async fn close(&self, tenant: &str) -> Result<Value, String> {
        let instance = self
            .open
            .lock()
            .unwrap()
            .remove(tenant)
            .ok_or_else(|| format!("no open tenant {tenant}"))?;
        self.loader
            .remove_instance(instance.plugin)
            .await
            .map_err(|e| e.to_string())?;
        self.loader
            .set_disabled(&format!("cfg-{tenant}"), true)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({ "tenant": tenant }))
    }

    /// `edit <tenant> <json>`: the error's kind says how it was refused.
    async fn edit(&self, args: &str) -> Result<Value, String> {
        let (tenant, config) = args.split_once(' ').ok_or("edit <tenant> <json>")?;
        let config: Value = serde_json::from_str(config).map_err(|e| e.to_string())?;
        match self.loader.update(&format!("cfg-{tenant}"), config).await {
            Ok(()) => {
                if self.open.lock().unwrap().contains_key(tenant) {
                    self.doors.wait(tenant).await?;
                }
                Ok(json!({ "tenant": tenant }))
            }
            Err(error) => {
                let kind = match &error {
                    LoaderError::Rejected { .. } => "rejected",
                    LoaderError::ApplyFailed { .. } => "apply-failed",
                    LoaderError::RollbackFailed { .. } => "rollback-failed",
                    _ => "other",
                };
                if self.open.lock().unwrap().contains_key(tenant) {
                    self.doors.wait(tenant).await?;
                }
                Ok(json!({ "tenant": tenant, "error": error.to_string(), "kind": kind }))
            }
        }
    }

    /// `call <tenant> <service> <method> <json args>`.
    async fn call(&self, args: &str) -> Result<Value, String> {
        let mut parts = args.splitn(4, ' ');
        let (Some(tenant), Some(service), Some(method)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return Err("call <tenant> <service> <method> <json args>".into());
        };
        let args: Value = serde_json::from_str(parts.next().unwrap_or("[]"))
            .map_err(|e| format!("the arguments: {e}"))?;
        if !self.open.lock().unwrap().contains_key(tenant) {
            return Err(format!("no open tenant {tenant}"));
        }
        let door = self.doors.wait(tenant).await?;
        call(&door, service, method, args).await
    }

    /// `peek <from> <of> <service>`.
    fn peek(&self, args: &str) -> Result<Value, String> {
        let parts: Vec<&str> = args.split_whitespace().collect();
        let [from, of, service] = parts[..] else {
            return Err("peek <from> <of> <service>".into());
        };
        let instance = *self
            .seen
            .lock()
            .unwrap()
            .get(of)
            .ok_or_else(|| format!("no tenant {of} was ever open"))?;
        let ctx = match from {
            "-" => self.root.clone(),
            tenant => self
                .doors
                .get(tenant)
                .ok_or_else(|| format!("no open tenant {tenant}"))?
                .ctx
                .clone(),
        };
        let visible = ctx
            .get_as::<dyn HostDispatch>(host_key_in(service, instance))
            .is_some();
        Ok(json!({ "visible": visible }))
    }

    /// `state <tenant>`.
    async fn state(&self, tenant: &str) -> Result<Value, String> {
        let plugin = self
            .open
            .lock()
            .unwrap()
            .get(tenant)
            .map(|instance| instance.plugin)
            .ok_or_else(|| format!("no open tenant {tenant}"))?;
        let config = match self.loader.evaluated(&format!("cfg-{tenant}")) {
            Some(Ok(config)) => config,
            Some(Err(error)) => return Err(error.to_string()),
            None => return Err(format!("no row cfg-{tenant}")),
        };
        let mut rows = Vec::new();
        let mut plugins = BTreeMap::new();
        for entry in self.loader.entries() {
            if entry.instance.as_ref().map(|i| i.plugin) != Some(plugin) {
                continue;
            }
            rows.push(json!([entry.id, status(&entry.status)]));
            plugins.insert(entry.id, json!(entry.plugin.map(|p| p.0)));
        }
        let door = self.doors.wait(tenant).await?;
        let mut settings = serde_json::Map::new();
        for method in ["tenant", "greeting", "quota"] {
            settings.insert(
                method.into(),
                call(&door, "settings", method, json!([])).await?,
            );
        }
        let mut answers = serde_json::Map::new();
        let mut tokens = serde_json::Map::new();
        for service in ["greeter", "py_greeter"] {
            answers.insert(
                service.into(),
                call(&door, service, "hello", json!(["Ada"])).await?,
            );
            tokens.insert(
                service.into(),
                call(&door, service, "token", json!([])).await?,
            );
        }
        Ok(json!({
            "tenant": tenant,
            "config": config,
            "rows": rows,
            "settings": settings,
            "answers": answers,
            "identity": { "plugins": plugins, "tokens": tokens },
        }))
    }

    /// `inspect`.
    async fn inspect(&self) -> Result<Value, String> {
        let diagnostics = self.root.diagnostics();
        let parents: HashMap<u64, Option<u64>> = diagnostics
            .plugins
            .iter()
            .map(|p| (p.id.0, p.parent.map(|parent| parent.0)))
            .collect();
        let under = |mut id: Option<u64>, top: u64| {
            while let Some(current) = id {
                if current == top {
                    return true;
                }
                id = parents.get(&current).copied().flatten();
            }
            false
        };
        let entries = self.loader.entries();
        let mut tenants = serde_json::Map::new();
        let open: Vec<(String, rutis::PluginId, InstanceId)> = self
            .open
            .lock()
            .unwrap()
            .iter()
            .map(|(tenant, i)| (tenant.clone(), i.plugin, i.view.instance()))
            .collect();
        for (tenant, plugin, instance) in open {
            let plugins = diagnostics
                .plugins
                .iter()
                .filter(|p| under(Some(p.id.0), plugin.0))
                .count();
            let bindings = diagnostics
                .bindings
                .iter()
                .filter(|b| b.key.instance_id() == Some(instance))
                .count();
            // The language rows' keys in the runtimes start with their
            // fiber's instance id.
            let rows: Vec<String> = entries
                .iter()
                .filter(|e| e.instance.as_ref().map(|i| i.plugin) == Some(plugin))
                .filter_map(|e| e.view.as_ref().map(|v| format!("{}.", v.instance())))
                .collect();
            tenants.insert(
                tenant,
                json!({
                    "instance": instance.to_string(),
                    "label": format!("rutis-loader/instance/{instance}"),
                    "plugins": plugins,
                    "bindings": bindings,
                    "rows": rows,
                }),
            );
        }
        let mut runtimes = serde_json::Map::new();
        for (name, handle) in [("node", &self.node), ("py", &self.python)] {
            let held = match handle.ready().await {
                Some(process) => process
                    .call_async("", "inspect", Value::Null)
                    .await
                    .unwrap_or_else(|e| json!({ "error": e.to_string() })),
                None => json!({ "error": "not running" }),
            };
            runtimes.insert(name.into(), held);
        }
        Ok(json!({
            "pid": std::process::id(),
            "rutis": {
                "plugins": diagnostics.plugins.len(),
                "bindings": diagnostics.bindings.len(),
            },
            "loader": { "entries": entries.len() },
            "tenants": tenants,
            "runtimes": runtimes,
        }))
    }
}

/// Call `service.method(...args)` from inside a tenant (its door).
async fn call(door: &Door, service: &str, method: &str, args: Value) -> Result<Value, String> {
    let dispatch = door
        .ctx
        .get_as::<dyn HostDispatch>(host_key_in(service, door.instance))
        .ok_or_else(|| format!("{service} is not available in tenant {}", door.tenant))?;
    let reply = dispatch
        .invoke(method, CallValue::from(args))
        .map_err(|e| e.to_string())?;
    settle(reply)
        .await
        .and_then(CallValue::json)
        .map_err(|e| e.to_string())
}

/// Wait until every global row that runs has started (or failed).
/// `reconcile` returns once no row is in transition, but a row of a
/// runtime waits for the runtime's rows to be released
/// ([`RuntimeRowsPlugin`], once the runtime is up) and is `Pending` until
/// then: `ready` reports the rows as they run (#568).
async fn started(loader: &Loader) -> Result<(), String> {
    for entry in loader.entries() {
        let (EntryStatus::Running(_), None, Some(view)) =
            (&entry.status, &entry.instance, entry.view)
        else {
            continue;
        };
        let mut watch = view.watch();
        let starting =
            |state: &FiberState| matches!(state, FiberState::Pending | FiberState::Loading);
        let waited = tokio::time::timeout(
            STARTED,
            watch.wait_for(|snapshot| !starting(&snapshot.state)),
        )
        .await;
        match waited {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => return Err(format!("row {} ended while it started", entry.id)),
            Err(_) => {
                return Err(format!(
                    "row {} did not start within {STARTED:?}: {:?}",
                    entry.id,
                    view.state().state
                ))
            }
        }
    }
    Ok(())
}

fn status(status: &EntryStatus) -> String {
    match status {
        EntryStatus::Running(snapshot) => format!("{:?}", snapshot.state),
        EntryStatus::Disabled => "Disabled".into(),
        EntryStatus::Inactive => "Inactive".into(),
        EntryStatus::Unresolved(error) => format!("Unresolved: {error}"),
        EntryStatus::Stopped => "Stopped".into(),
        other => format!("{other:?}"),
    }
}

fn result_of(result: &InstanceResult) -> String {
    match result {
        InstanceResult::Active => "Active".into(),
        InstanceResult::Waiting => "Waiting".into(),
        InstanceResult::Failed(error) => format!("Failed: {error}"),
        InstanceResult::Skipped => "Skipped".into(),
    }
}

fn read_patches(path: &Path) -> Result<Vec<Patch>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

// ── The admin layer on disk ─────────────────────────────────────

/// The admin layer as a JSON file: written whole to a file beside it, then
/// renamed over it; its version is a hash of its content, so a save finding
/// other content is a conflict.
struct LayerFile {
    path: PathBuf,
}

impl LayerFile {
    fn read(&self) -> Result<Option<String>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", self.path.display())),
        }
    }
}

/// FNV-1a of the content; `none` for no file.
fn version_of(text: Option<&str>) -> Version {
    Version(match text {
        None => "none".into(),
        Some(text) => {
            let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
            format!("{hash:016x}")
        }
    })
}

impl Persist for LayerFile {
    fn load<'a>(
        &'a self,
        _layer: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let unreadable = |message: String| LoaderError::Resolve {
                name: self.path.display().to_string(),
                message,
            };
            let text = self.read().map_err(unreadable)?;
            let patches = match &text {
                None => Vec::new(),
                Some(text) => serde_json::from_str(text).map_err(|e| unreadable(e.to_string()))?,
            };
            Ok((patches, version_of(text.as_deref())))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        expected: &'a Version,
        _edits: &'a [Edit],
        patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move {
            let current = self.read().map_err(PersistError::Failed)?;
            if version_of(current.as_deref()) != *expected {
                return Err(PersistError::Conflict);
            }
            let text = serde_json::to_string_pretty(patches)
                .map_err(|e| PersistError::Failed(e.to_string()))?;
            let failed =
                |e: std::io::Error| PersistError::Failed(format!("{}: {e}", self.path.display()));
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir).map_err(failed)?;
            }
            let part = self.path.with_extension("json.part");
            std::fs::write(&part, &text).map_err(failed)?;
            std::fs::rename(&part, &self.path).map_err(failed)?;
            Ok(version_of(Some(&text)))
        })
    }
}

// ── The Rust rows ───────────────────────────────────────────────

/// The tenant an instance is for (`create_instance(..).with`).
struct Tenant(String);

/// A tenant's configuration: the config of its row `cfg-<tenant>`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TenantConfig {
    tenant: String,
    greeting: String,
    quota: u32,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoConfig {}

/// Where a tenant's configuration is provided.
fn config_key(tenant: &str) -> TypeKey {
    TypeKey::keyed_dynamic::<TenantConfig>(format!("tenant/{tenant}"))
}

fn failed(error: impl std::fmt::Display) -> CordisError {
    CordisError::PluginFailed(error.to_string().into())
}

fn builtins(doors: Doors) -> Builtins {
    let mut builtins = Builtins::new();
    builtins.register::<TenantConfig, _>("app/tenant-config", ConfigFactory);
    builtins.register_with::<NoConfig, _, _>("app/settings", |build: &Build| {
        let tenant = tenant_of(build)?;
        Ok(RowFactory {
            name: "app/settings",
            injects: vec![config_key(&tenant)],
            row: Arc::new(Row::Settings {
                tenant,
                instance: build.instance(TENANT)?,
            }),
        })
    });
    builtins.register_with::<NoConfig, _, _>("app/caller", move |build: &Build| {
        let instance = build.instance(TENANT)?;
        Ok(RowFactory {
            name: "app/caller",
            injects: ["settings", "greeter", "py_greeter"]
                .iter()
                .map(|name| host_key_in(name, instance))
                .collect(),
            row: Arc::new(Row::Caller {
                tenant: tenant_of(build)?,
                instance,
                doors: doors.clone(),
            }),
        })
    });
    builtins
}

fn tenant_of(build: &Build) -> Result<String, CordisError> {
    build
        .value::<Tenant>()
        .map(|tenant| tenant.0.clone())
        .ok_or_else(|| failed("an instance of `tenant` is created with its Tenant"))
}

/// `app/tenant-config`.
struct ConfigFactory;

impl PluginFactory<TenantConfig> for ConfigFactory {
    fn name(&self) -> &str {
        "app/tenant-config"
    }

    fn build(&self, config: &TenantConfig) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(ConfigRow(config.clone())))
    }
}

struct ConfigRow(TenantConfig);

impl Plugin for ConfigRow {
    fn name(&self) -> &str {
        "app/tenant-config"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            // Checked as it starts, not by the config's validation: an edit
            // to 0 passes the dry run and fails here (it is rolled back).
            if self.0.quota == 0 {
                return Err(failed(format!(
                    "tenant {} needs a quota of at least 1",
                    self.0.tenant
                )));
            }
            ctx.provide_as::<TenantConfig>(config_key(&self.0.tenant), Arc::new(self.0.clone()))?;
            Ok(Effect::Done)
        })
    }
}

/// What a row inside a tenant does.
enum Row {
    /// Provide `settings` from the tenant's configuration.
    Settings {
        tenant: String,
        instance: InstanceId,
    },
    /// Open the tenant's door.
    Caller {
        tenant: String,
        instance: InstanceId,
        doors: Doors,
    },
}

/// The factory of a row inside a tenant.
struct RowFactory {
    name: &'static str,
    injects: Vec<TypeKey>,
    row: Arc<Row>,
}

impl PluginFactory<NoConfig> for RowFactory {
    fn name(&self) -> &str {
        self.name
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn build(&self, _: &NoConfig) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(TenantRow {
            name: self.name,
            injects: self.injects.clone(),
            row: self.row.clone(),
        }))
    }
}

struct TenantRow {
    name: &'static str,
    injects: Vec<TypeKey>,
    row: Arc<Row>,
}

impl Plugin for TenantRow {
    fn name(&self) -> &str {
        self.name
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            match &*self.row {
                Row::Settings { tenant, instance } => {
                    let config = ctx
                        .require_as::<TenantConfig>(config_key(tenant))
                        .map_err(failed)?;
                    ctx.provide_as::<dyn HostDispatch>(
                        host_key_in("settings", *instance),
                        Arc::new(Settings((*config).clone())),
                    )?;
                    Ok(Effect::Done)
                }
                Row::Caller {
                    tenant,
                    instance,
                    doors,
                } => {
                    let serial = doors.open(Door {
                        tenant: tenant.clone(),
                        instance: *instance,
                        ctx: ctx.clone(),
                    });
                    let (doors, tenant) = (doors.clone(), tenant.clone());
                    Ok(Effect::Disposer(Box::new(move || {
                        doors.close(&tenant, serial);
                        Ok(())
                    })))
                }
            }
        })
    }
}

/// The instance service `settings`.
struct Settings(TenantConfig);

impl HostDispatch for Settings {
    fn invoke(&self, method: &str, _args: CallValue) -> Reply {
        match method {
            "tenant" => Ok(json!(self.0.tenant).into()),
            "greeting" => Ok(json!(self.0.greeting).into()),
            "quota" => Ok(json!(self.0.quota).into()),
            other => Err(CallError::Value(format!("settings has no method {other}"))),
        }
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "tenant": "sync", "greeting": "sync", "quota": "sync" }))
    }
}

// ── Doors into tenants ──────────────────────────────────────────

/// A tenant's door: the context of its `caller` row, inside its instance.
#[derive(Clone)]
struct Door {
    tenant: String,
    instance: InstanceId,
    ctx: Ctx,
}

/// The open doors, by tenant.
#[derive(Clone, Default)]
struct Doors(Arc<DoorsInner>);

#[derive(Default)]
struct DoorsInner {
    open: Mutex<HashMap<String, (u64, Door)>>,
    changed: tokio::sync::Notify,
    serial: AtomicU64,
}

impl Doors {
    fn open(&self, door: Door) -> u64 {
        let serial = self.0.serial.fetch_add(1, Ordering::Relaxed);
        self.0
            .open
            .lock()
            .unwrap()
            .insert(door.tenant.clone(), (serial, door));
        self.0.changed.notify_waiters();
        serial
    }

    /// Close `tenant`'s door, if it is still the one opened as `serial`.
    fn close(&self, tenant: &str, serial: u64) {
        let mut open = self.0.open.lock().unwrap();
        if open.get(tenant).is_some_and(|(s, _)| *s == serial) {
            open.remove(tenant);
        }
    }

    fn get(&self, tenant: &str) -> Option<Door> {
        self.0
            .open
            .lock()
            .unwrap()
            .get(tenant)
            .map(|(_, door)| door.clone())
    }

    /// `tenant`'s door, once open (its rows may still be starting).
    async fn wait(&self, tenant: &str) -> Result<Door, String> {
        let waiting = async {
            loop {
                // Registered before the check: a door opened in between
                // wakes it.
                let changed = self.0.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if let Some(door) = self.get(tenant) {
                    return door;
                }
                changed.await;
            }
        };
        tokio::time::timeout(DOOR, waiting)
            .await
            .map_err(|_| format!("tenant {tenant}'s door did not open within {DOOR:?}"))
    }
}
