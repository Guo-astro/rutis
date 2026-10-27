use crate::runtime::rpc::{Connection, Dispatch, Reply, Value as RpcValue};
use crate::runtime::Error;
use crate::runtime::{CallerRow, HostDispatch, RuntimeSession};
use crate::transport::local::Stdio;
use rutis::host::CtxHost;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Receives changes of exported Cordis service slots. `handle` addresses the
/// object now in the slot (`None` when it is unavailable); `version` orders
/// changes, so an older notification must not override a newer one.
pub trait ServiceEvents: Send + Sync + 'static {
    fn changed(&self, name: &str, handle: Option<String>, version: u64);

    /// A row of this observer's announces the service `name`, which it did
    /// not declare, with the methods it has (`{ method: "async" }`, see
    /// [`Process::load_row_exporting`]). Called before each change of it
    /// that names a handle, since a new object may have other methods.
    /// Whether the observer follows it: one that does not leaves the
    /// service unexported (the default).
    fn announced(&self, name: &str, methods: &Value) -> bool {
        let _ = (name, methods);
        false
    }
}

/// One host-provided service: its Cordis name, the bound methods as
/// `{ method: "sync" | "async" }`, and the dispatcher that serves them.
pub struct Host {
    pub name: String,
    pub methods: Value,
    pub dispatch: Arc<dyn HostDispatch>,
}

type Slots = Arc<Mutex<HashMap<String, (Option<String>, u64)>>>;

/// How the runtime identifies the service `name` in the scope `label` (a
/// row's `isolate` label for it): `name` outside any scope, `name`, a NUL
/// and `label` inside one. Neither may contain NUL, and a label may not be
/// empty ([`check_scoped`]), so no two (name, label) pairs share an id. Export slots, host proxies and
/// `host:<id>` call targets are registered by it, so the same name in two
/// scopes (two instances, say) does not collide. A runtime takes labels
/// only with the `scopes` feature.
pub fn scoped_id(name: &str, label: Option<&str>) -> String {
    match label {
        None => name.to_owned(),
        Some(label) => format!("{name}\0{label}"),
    }
}

/// The scope label of row `key`'s own use of a service it would otherwise
/// use in the scope `label` ([`Process::lease_host_for`]). U+0001 sets it
/// apart from the labels rows are configured with; it is neither NUL nor
/// empty, so [`scoped_id`] takes it.
fn row_label(label: Option<&str>, key: &str) -> String {
    format!("{}\u{1}row:{key}", label.unwrap_or_default())
}

/// Refuse a name or label [`scoped_id`] cannot keep apart.
fn check_scoped(name: &str, label: Option<&str>) -> Result<(), Error> {
    if name.contains('\0') || label.is_some_and(|label| label.contains('\0')) {
        return Err(Error::Value(format!(
            "service {name:?} or its scope label contains NUL"
        )));
    }
    if label == Some("") {
        return Err(Error::Value(format!(
            "service {name:?} has an empty scope label"
        )));
    }
    Ok(())
}

/// A service a row exports: its name, and who follows its changes.
type Exported = (String, Arc<dyn ServiceEvents>);

/// A loaded row's exports: who follows them, and their ids (declared, then
/// announced).
struct RowExports {
    observer: Arc<dyn ServiceEvents>,
    ids: Vec<String>,
}

/// The service name of a slot id ([`scoped_id`]).
fn name_of(id: &str) -> &str {
    id.split_once('\0').map_or(id, |(name, _)| name)
}

/// The label `isolate` gives `name`, if it isolates it.
fn label_of<'a>(isolate: &'a [(String, String)], name: &str) -> Option<&'a str> {
    isolate
        .iter()
        .find(|(isolated, _)| isolated == name)
        .map(|(_, label)| label.as_str())
}

/// A host service registered with the Node side, and how many leases use
/// it. Hosts of a mount stay for the process's lifetime.
struct HostEntry {
    dispatch: Arc<dyn HostDispatch>,
    leases: usize,
    /// The row it was leased for alone ([`Process::lease_host_for`]), which
    /// its calls come from.
    row: Option<CallerRow>,
}

/// Records the newest handle per slot and forwards newer changes; serves
/// calls to host-provided services.
struct Imports {
    slots: Slots,
    events: Option<Arc<dyn ServiceEvents>>,
    /// Observers of the services rows export, by id ([`scoped_id`]), with
    /// the service's name.
    exported: Mutex<HashMap<String, Exported>>,
    /// The exports of each row loaded with an observer, by row key.
    rows: Mutex<HashMap<String, RowExports>>,
    hosts: Mutex<HashMap<String, HostEntry>>,
    /// The session's side of the logical event bus (`events.*`), set once
    /// the session is up, before `mount`.
    bus: std::sync::OnceLock<Arc<crate::events::SessionEvents>>,
    /// What to do when a row's plugin ends on its own (`rows.ended`), by
    /// row key (see [`Process::on_row_ended`]).
    ended: Mutex<HashMap<String, RowEnded>>,
    /// What to do when the runtime charges an error nobody handled to a
    /// row (`rows.failed`), by row key (see [`Process::on_row_failed`]).
    failed: Mutex<HashMap<String, RowFailed>>,
    /// The id of each row as its configuration names it, by row key
    /// ([`Process::name_row`]); a row not named is called by its key.
    names: Mutex<HashMap<String, String>>,
    /// The process these calls come from, set once it is up: a plugin
    /// definition passed in a call is that process's (see [`calling`]).
    process: std::sync::OnceLock<std::sync::Weak<Process>>,
}

std::thread_local! {
    /// The process whose call is being served on this thread.
    static CALLING: std::cell::RefCell<Option<std::sync::Weak<Process>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` as serving a call of `process`.
fn with_calling<R>(process: Option<std::sync::Weak<Process>>, f: impl FnOnce() -> R) -> R {
    let previous = CALLING.with(|calling| calling.replace(process));
    let result = f();
    CALLING.with(|calling| calling.replace(previous));
    result
}

/// The process whose call into a host service is being served, read while
/// the call starts (as [`crate::session::caller_row`]).
pub(crate) fn calling() -> Option<Arc<Process>> {
    CALLING.with(|calling| calling.borrow().as_ref().and_then(std::sync::Weak::upgrade))
}

/// Run once when a row's plugin ends on its own.
type RowEnded = Box<dyn FnOnce() + Send>;

/// Run once, with why, when the runtime fails a running row: an error its
/// code left unhandled (#688).
type RowFailed = Box<dyn FnOnce(String) + Send>;

/// Everything one mount needs: the plugins loaded in order into one Cordis
/// Context, the exported services, and what the rutis side contributes.
#[derive(Default)]
pub struct Mount<'a> {
    /// `(entry, config)` per plugin, in load order.
    pub plugins: Vec<(&'a Path, Value)>,
    /// Exported services: `{ name: [member, ...] }`.
    pub services: Value,
    /// Follows changes of the exported service slots.
    pub observer: Option<Arc<dyn ServiceEvents>>,
    /// rutis services the plugins may depend on.
    pub hosts: Vec<Host>,
    /// The context the runtime's listeners join the logical event bus
    /// through (`docs/design-events-2026-10-18.md`): its bus, and, for the
    /// plugins of `plugins`, its scope. Without one the runtime's events
    /// stay in its process.
    pub ctx: Option<rutis::Ctx>,
    /// The runtime's name, as listener descriptors give it (`runtime`);
    /// `"runtime"` when not set.
    pub runtime: Option<String>,
    /// With no `plugins`: start an empty Context whose Cordis resolves from
    /// this file (a `package.json`), and load plugins later one by one with
    /// [`Process::load_row`].
    pub anchor: Option<&'a Path>,
    /// How to start the runtime process; `None` runs the Node runtime in
    /// the npm package (`node --import tsx <package>/src/runner.mjs`, feature
    /// `node`).
    pub launcher: Option<&'a Launcher>,
}

/// The command that starts a runtime process. It receives its channel and
/// then the first plugin (or the anchor) as its last two arguments: `fd:3`
/// when it takes an inherited socket ([`Launcher::inherit_fd`]), otherwise
/// a socket path to dial.
///
/// Build one with [`Launcher::new`] (or `Launcher::node` with the `node`
/// feature, `Launcher::python` with `python`) and its builders.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Launcher {
    pub program: std::ffi::OsString,
    pub args: Vec<std::ffi::OsString>,
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// The working directory; the application's by default.
    pub cwd: Option<std::path::PathBuf>,
    /// The process takes its channel as an inherited socket on fd 3
    /// (`fd:3`) instead of dialing a socket path.
    pub inherit_fd: bool,
    /// Standard input: none ([`Stdio::Null`]) unless set with
    /// [`Launcher::stdin`].
    pub stdin: Stdio,
    /// Standard output: this process's ([`Stdio::Inherit`]) unless set with
    /// [`Launcher::stdout`].
    pub stdout: Stdio,
    /// Standard error: this process's ([`Stdio::Inherit`]) unless set with
    /// [`Launcher::stderr`].
    pub stderr: Stdio,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            program: Default::default(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            inherit_fd: false,
            stdin: Stdio::Null,
            stdout: Stdio::Inherit,
            stderr: Stdio::Inherit,
        }
    }
}

impl Launcher {
    pub fn new(program: impl Into<std::ffi::OsString>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<std::ffi::OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(
        mut self,
        name: impl Into<std::ffi::OsString>,
        value: impl Into<std::ffi::OsString>,
    ) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    pub fn cwd(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// The process takes `fd:3`, an inherited socket, as its channel.
    pub fn inherit_fd(mut self) -> Self {
        self.inherit_fd = true;
        self
    }

    /// The process's standard input. By default it has none
    /// ([`Stdio::Null`]); [`Stdio::Inherit`] gives it this process's, for a
    /// plugin that reads the terminal (a terminal UI, Python's `input()`),
    /// which needs the real handle: it checks that it is a terminal and sets
    /// its modes itself. Give it to one process at most: processes reading
    /// the same terminal race for each key.
    ///
    /// The setting is the whole runtime process's: every plugin loaded into
    /// it can read the input. Run a plugin that needs the terminal in a
    /// runtime of its own ([`LocalRuntime::named`](crate::runtime::LocalRuntime::named)).
    ///
    /// On Unix, a runtime process normally runs in a process group of its
    /// own, so a Ctrl-C in the terminal reaches only the host, which unloads
    /// its plugins first. One that reads the terminal stays in the host's
    /// group (a background group reading it would be stopped), so a Ctrl-C
    /// reaches it too, and its plugins' cleanups may not run then.
    pub fn stdin(mut self, stdio: Stdio) -> Self {
        self.stdin = stdio;
        self
    }

    /// The process's standard output: this process's by default
    /// ([`Stdio::Inherit`]); [`Stdio::Null`] discards it, for example so
    /// that other runtimes do not draw over a terminal UI; [`Stdio::Lines`]
    /// passes it on a whole line at a time, so that it does not run into
    /// this process's own lines (or other runtimes'). For the whole
    /// runtime process, as [`Launcher::stdin`].
    pub fn stdout(mut self, stdio: Stdio) -> Self {
        self.stdout = stdio;
        self
    }

    /// The process's standard error: this process's by default
    /// ([`Stdio::Inherit`]); [`Stdio::Null`] discards it, [`Stdio::Lines`]
    /// passes it on a whole line at a time. For the whole runtime process,
    /// as [`Launcher::stdin`].
    pub fn stderr(mut self, stdio: Stdio) -> Self {
        self.stderr = stdio;
        self
    }

    /// The Node runtime of the npm package `node_package` (`node/rutis-runtime`,
    /// or a deployed `@arcships/rutis-runtime`): `node --import tsx
    /// src/runner.mjs` in the package, on an inherited socket when the
    /// package says it takes one (`rutisChannels` lists `"fd"`).
    #[cfg(feature = "node")]
    pub fn node(node_package: &std::path::Path) -> Self {
        let launcher = Launcher::new("node")
            .arg("--import")
            .arg("tsx")
            .arg(node_package.join("src/runner.mjs"))
            .cwd(node_package);
        match crate::runtime::spawn::node_inherits(node_package) {
            true => launcher.inherit_fd(),
            false => launcher,
        }
    }

    /// The Python runtime: `python3 -m rutis` (`python` on Windows; the
    /// `rutis` package installed where that interpreter finds it), with
    /// `project` ahead of the inherited `PYTHONPATH`, in `project`, on an
    /// inherited socket (on Windows, a loopback address).
    #[cfg(feature = "python")]
    pub fn python(project: &std::path::Path) -> Self {
        let inherited = std::env::var_os("PYTHONPATH").filter(|p| !p.is_empty());
        let path = search_path(project.into(), inherited.as_deref());
        Launcher::new(if cfg!(windows) { "python" } else { "python3" })
            .arg("-m")
            .arg("rutis")
            .env("PYTHONPATH", path)
            .env("PYTHONUNBUFFERED", "1")
            // A plugin imported again after an edit must not come from a
            // bytecode file written in the same second as the old source.
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .cwd(project)
            .inherit_fd()
    }

    /// A Go runtime: `binary`, built with the Go SDK (`go/rutis`), serving
    /// the plugins compiled into it; in `project`, on an inherited socket
    /// (on Windows, a loopback address).
    #[cfg(feature = "go")]
    pub fn go(binary: &std::path::Path, project: &std::path::Path) -> Self {
        Launcher::new(binary).cwd(project).inherit_fd()
    }

    /// A Swift runtime: `binary`, built with the Swift SDK (`swift/rutis`),
    /// serving the plugins compiled into it; in `project`, on an inherited
    /// socket. The same start as a Go binary's.
    #[cfg(feature = "swift")]
    pub fn swift(binary: &std::path::Path, project: &std::path::Path) -> Self {
        Launcher::new(binary).cwd(project).inherit_fd()
    }

    /// The Bun runtime of the npm package `package` (`bun/rutis-bun`, or a
    /// deployed `@arcships/rutis-bun`), run by the Bun executable `program`
    /// (`bun` on `PATH` when `None`), in `project`, on an inherited socket
    /// (on Windows, a loopback address). Auto-install and `.env` files are
    /// always off: plugins resolve only what the project has installed, and
    /// the environment is only what this process gives.
    #[cfg(feature = "bun")]
    pub fn bun(
        program: Option<&std::ffi::OsStr>,
        package: &std::path::Path,
        project: &std::path::Path,
    ) -> Self {
        Launcher::new(program.unwrap_or(std::ffi::OsStr::new("bun")))
            .arg("--no-install")
            .arg("--no-env-file")
            .arg(package.join("src/main.ts"))
            .cwd(project)
            .inherit_fd()
    }
}

/// `first` ahead of the search path `rest` (`PATH` syntax for the
/// platform: `:` or `;` between entries).
pub(crate) fn search_path(
    first: std::ffi::OsString,
    rest: Option<&std::ffi::OsStr>,
) -> std::ffi::OsString {
    let mut entries = vec![std::path::PathBuf::from(first)];
    entries.extend(rest.into_iter().flat_map(std::env::split_paths));
    std::env::join_paths(&entries).unwrap_or_else(|_| entries[0].clone().into_os_string())
}

impl Imports {
    /// Row `key` is unloaded or never loaded: nothing waits for it to
    /// end or fail any more.
    fn forget_row(&self, key: &str) {
        self.ended.lock().unwrap().remove(key);
        self.failed.lock().unwrap().remove(key);
    }

    /// Stop following the slots `ids` (a row's, unloaded): their observers,
    /// each with the service's name and the slot's last version, to tell
    /// them the services are gone. The slots go too: ids in an instance's
    /// scope are never exported again, and a row loaded again starts new
    /// slots; without this, every row ever unloaded left its slots here
    /// (#191).
    fn forget(
        &self,
        ids: impl IntoIterator<Item = String>,
    ) -> Vec<(String, Arc<dyn ServiceEvents>, u64)> {
        let observers: Vec<_> = {
            let mut exported = self.exported.lock().unwrap();
            ids.into_iter()
                .filter_map(|id| exported.remove(&id).map(|found| (id, found)))
                .collect()
        };
        let mut slots = self.slots.lock().unwrap();
        observers
            .into_iter()
            .map(|(id, (name, observer))| {
                let version = slots.remove(&id).map_or(0, |(_, version)| version);
                (name, observer, version)
            })
            .collect()
    }

    /// Row `row` announces the slot `id`, which it did not declare, with
    /// `methods`: whether its observer follows it from now on.
    fn announce(&self, row: &str, id: &str, methods: &Value) -> bool {
        let observer = {
            let rows = self.rows.lock().unwrap();
            match rows.get(row) {
                Some(exports) => exports.observer.clone(),
                // Not loaded with an observer, or unloaded meanwhile.
                None => return false,
            }
        };
        if let Some((_, current)) = self.exported.lock().unwrap().get(id) {
            // Another row's: the runtime exports a slot once.
            if !Arc::ptr_eq(current, &observer) {
                return false;
            }
        }
        let name = name_of(id);
        if !observer.announced(name, methods) {
            return false;
        }
        let mut rows = self.rows.lock().unwrap();
        let Some(exports) = rows.get_mut(row) else {
            return false;
        };
        let mut exported = self.exported.lock().unwrap();
        if !exported.contains_key(id) {
            exported.insert(id.to_owned(), (name.to_owned(), observer));
            exports.ids.push(id.to_owned());
        }
        true
    }

    /// The slot `id` now holds `handle`.
    fn update(&self, id: String, handle: Option<String>, version: u64) {
        // A withdrawal of a slot nobody follows or knows: a row's service
        // withdrawn after its row was forgotten (`forget_exports`), which
        // must not leave an entry behind for every row unloaded (#191).
        let followed = self.events.is_some() || self.exported.lock().unwrap().contains_key(&id);
        {
            let mut slots = self.slots.lock().unwrap();
            if handle.is_none() && !followed && !slots.contains_key(&id) {
                return;
            }
            let entry = slots.entry(id.clone()).or_insert((None, 0));
            if version <= entry.1 {
                return;
            }
            *entry = (handle.clone(), version);
        }
        if let Some(events) = &self.events {
            events.changed(&id, handle.clone(), version);
        }
        let exported = self.exported.lock().unwrap().get(&id).cloned();
        if let Some((name, observer)) = exported {
            observer.changed(&name, handle, version);
        }
    }
}
impl Dispatch for Imports {
    fn arrived(&self, _connection: &Connection, method: &str) {
        if let Some(events) = self.bus.get() {
            events.arrived(method);
        }
    }

    fn replied(&self, _connection: &Connection, method: &str) {
        if let Some(events) = self.bus.get() {
            events.replied(method);
        }
    }

    fn invoke(&self, connection: &Connection, target: &str, method: &str, args: RpcValue) -> Reply {
        if let Some(name) = target.strip_prefix("host:") {
            let (host, row, unleased) = {
                let hosts = self.hosts.lock().unwrap();
                let (host, row) = hosts
                    .get(name)
                    .map(|entry| (entry.dispatch.clone(), entry.row.clone()))
                    .ok_or_else(|| Error::Value(format!("no host service {name}")))?;
                // Through the registration rows share, of a service rows
                // lease for themselves (#676): the calling plugin does not
                // declare it, so a method that needs the calling row says
                // what to declare. A service registered for the whole
                // process (a mount's) is called so by design.
                let service = name_of(name);
                let leased = row.is_none()
                    && hosts.keys().any(|id| {
                        id.split_once('\0').is_some_and(|(other, label)| {
                            other == service && label.contains("\u{1}row:")
                        })
                    });
                (host, row, leased.then(|| service.to_owned()))
            };
            let process = self.process.get().cloned();
            return with_calling(process, || {
                crate::session::with_caller_row(row, || {
                    crate::session::with_unleased(unleased, || host.invoke(method, args))
                })
            });
        }
        if target.is_empty() && method.starts_with("events.") {
            return match self.bus.get() {
                Some(events) => events.invoke(connection, method, args),
                None => Err(Error::Value(format!(
                    "{method}: the session's events are not set up yet"
                ))),
            };
        }
        if target.is_empty() && method == "rows.ended" {
            let (key,): (String,) = crate::runtime::decode(args.json()?)?;
            // Its listeners leave the bus at once (events protocol §7.1).
            if let Some(events) = self.bus.get() {
                events.row_ended(&key);
            }
            let ended = self.ended.lock().unwrap().remove(&key);
            if let Some(ended) = ended {
                ended();
            }
            return Ok(RpcValue::Undefined);
        }
        if target.is_empty() && method == "rows.failed" {
            // `[key, why]`: an error the row's code left unhandled (#688).
            // The host fails the row; the runtime keeps running the others.
            let (key, why): (String, String) = crate::runtime::decode(args.json()?)?;
            let failed = self.failed.lock().unwrap().remove(&key);
            return match failed {
                Some(failed) => {
                    failed(why);
                    Ok(RpcValue::Undefined)
                }
                // Nothing fails rows here (an embedder without the
                // loader), or the row is gone: the runtime writes why.
                None => Err(Error::Value(format!(
                    "rows.failed: nothing fails row {key} here"
                ))),
            };
        }
        if !(target.is_empty() && method == "service") {
            return Err(Error::Value(
                "application has no exported service target".into(),
            ));
        }
        // `[id, handle, version]`, and for a service the row did not
        // declare, which a runtime announces only to a host whose mount
        // asked (`announce`), `{ row, methods }` after them.
        let mut args: Vec<Value> = crate::runtime::decode(args.json()?)?;
        let announced: Option<Announced> = match args.len() > 3 {
            true => Some(crate::runtime::decode(args.split_off(3).swap_remove(0))?),
            false => None,
        };
        let (name, handle, version): (String, Option<String>, u64) =
            crate::runtime::decode(Value::Array(args))?;
        if let (Some(announced), Some(object)) = (announced, &handle) {
            // Not followed (a service the row keeps to its own scope):
            // nothing here holds the object, so it goes back at once.
            if !self.announce(&announced.row, &name, &announced.methods) {
                let (connection, object) = (connection.clone(), object.clone());
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = connection
                            .invoke_async("", "release", json!([object]).into())
                            .await;
                    });
                }
                return Ok(RpcValue::Undefined);
            }
        }
        self.update(name, handle, version);
        Ok(RpcValue::Undefined)
    }
}

/// What a runtime says of a service a row provides without declaring it.
#[derive(serde::Deserialize)]
struct Announced {
    /// The row's key.
    row: String,
    /// `{ method: "sync" | "async" }`.
    #[serde(default)]
    methods: Value,
}

/// What the `mount` call sends.
struct MountRest {
    plugins: Vec<Value>,
    services: Value,
    provided: serde_json::Map<String, Value>,
    ctx: Option<rutis::Ctx>,
    runtime: Option<String>,
}

/// A mount's parts, once split from where the process comes from.
/// The runtime a mount runs on, for messages: its name, else `otherwise`.
fn far_name(mount: &Mount<'_>, otherwise: &str) -> String {
    mount
        .runtime
        .clone()
        .unwrap_or_else(|| otherwise.to_owned())
}

struct Started {
    plugins: Vec<(std::path::PathBuf, Value)>,
    services: Value,
    events: Option<Arc<dyn ServiceEvents>>,
    hosts: HashMap<String, HostEntry>,
    provided: serde_json::Map<String, Value>,
    ctx: Option<rutis::Ctx>,
    runtime: Option<String>,
}

impl Started {
    /// The parts, and the plugins as given (the first names what to start).
    fn from(mount: Mount<'_>) -> (Self, Vec<(&Path, Value)>) {
        let Mount {
            plugins,
            services,
            observer: events,
            hosts,
            ctx,
            runtime,
            anchor: _,
            launcher: _,
        } = mount;
        let provided = hosts
            .iter()
            .map(|host| (host.name.clone(), host.methods.clone()))
            .collect();
        let hosts = hosts
            .into_iter()
            .map(|host| {
                let entry = HostEntry {
                    dispatch: host.dispatch,
                    leases: 1,
                    row: None,
                };
                (host.name, entry)
            })
            .collect();
        let started = Self {
            plugins: plugins
                .iter()
                .map(|(entry, config)| (entry.to_path_buf(), config.clone()))
                .collect(),
            services,
            events,
            hosts,
            provided,
            ctx,
            runtime,
        };
        (started, plugins)
    }
}

/// Owns one native Cordis process and its generated service bindings.
pub struct Process {
    peer: Connection,
    imports: Arc<Imports>,
    runtime: tokio::runtime::Handle,
    /// The process this side started; `None` for an attached runtime.
    child: Option<crate::runtime::spawn::Child>,
    /// Reported by the runner when mounting.
    features: std::sync::OnceLock<Vec<String>>,
    /// What the runtime said of itself when mounted (see [`Process::about`]).
    about: std::sync::OnceLock<Value>,
    /// The plugins the runtime said it has when mounted, if it said (see
    /// [`Process::plugins`]).
    plugins: std::sync::OnceLock<Option<HashMap<String, RowSchema>>>,
    /// Serializes `hosts.provide` / `hosts.withdraw`, so a lease returns only
    /// once its service is registered on the Node side, and a withdrawal
    /// never overtakes the registration it undoes.
    host_changes: tokio::sync::Mutex<()>,
    _directory: Option<tempfile::TempDir>,
    /// For a runtime on a session something else owns (a link): what routes
    /// the runtime's calls here. The session is not this process's to close.
    routed: Option<Box<dyn std::any::Any + Send + Sync>>,
    /// The scope chain the process was mounted in: where a plugin definition
    /// it hands over outside any row's call may be mounted.
    scope: Vec<rutis::events::Scope>,
    /// The plugin definitions it handed over, by the value's identity, so
    /// one value is one definition.
    definitions: Mutex<HashMap<usize, rutis::internal::definitions::WeakDefinition>>,
    /// Those that belong to the process itself (handed over outside any
    /// row's call): they end when its session does.
    owned: Arc<Mutex<Vec<rutis::Definition>>>,
}

/// What a plugin module declares, for rutis-loader (see
/// [`Process::describe_row`]).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct RowSchema {
    /// The JSON Schema of the config, if the plugin declares one.
    pub config: Option<Value>,
    /// The services the plugin injects; Cordis has only required ones.
    #[serde(default)]
    pub inject: Vec<String>,
    /// The services it provides to rutis: `{ name: { method: "sync" | "async" } }`.
    #[serde(default)]
    pub provides: serde_json::Map<String, Value>,
    /// The plugin's version, when the runtime knows it (a package version,
    /// a module version, a VCS revision).
    #[serde(default)]
    pub version: Option<String>,
    /// The plugin may provide services `provides` does not list: the
    /// runtime announces them when they are provided (`rows.announce`), so
    /// what it provides is complete only once it runs. A Cordis plugin
    /// whose package declares nothing (a published one), say.
    #[serde(default)]
    pub announces: bool,
    /// A Cordis plugin, not a leaf one (`definePlugin`): it resolves what
    /// it injects from Cordis rows of its own runtime in Cordis. Only the
    /// Node runtime has them.
    #[serde(default)]
    pub cordis: bool,
}

/// What a plugin definition declares ([`Process::read_plugin`]): what a
/// module does ([`RowSchema`]), and its name.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PluginSchema {
    /// Its declared name, if it has one.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub schema: RowSchema,
}

/// One row's use of a host service (see [`Process::lease_host`]). Give it
/// back with [`HostLease::release`]: dropping it releases nothing, and the
/// service stays registered in the runtime.
#[must_use = "a lease is given back only by HostLease::release"]
pub struct HostLease {
    process: Arc<Process>,
    /// See [`scoped_id`].
    id: String,
    /// The scope label it was registered in.
    label: Option<String>,
    /// The shared registration a lease of one row's own takes too
    /// ([`Process::lease_host_for`]).
    shared: Option<Box<HostLease>>,
}

impl HostLease {
    /// The scope label the service is registered in: a row using it
    /// isolates its name under this label when loaded. For a lease of one
    /// row's own ([`Process::lease_host_for`]) it is that row's alone.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Give the lease back; the last one withdraws the service from Cordis.
    pub async fn release(self) -> Result<(), Error> {
        let released = self.process.release_host(&self.id).await;
        if let Some(shared) = self.shared {
            Box::pin(shared.release()).await?;
        }
        released
    }
}

impl Process {
    pub async fn launch(
        node_package: &Path,
        plugin: &Path,
        config: Value,
        services: Value,
    ) -> Result<Arc<Self>, Error> {
        Self::launch_observed(node_package, plugin, config, services, None).await
    }

    /// Launch and report every later change of the exported service slots.
    pub async fn launch_observed(
        node_package: &Path,
        plugin: &Path,
        config: Value,
        services: Value,
        events: Option<Arc<dyn ServiceEvents>>,
    ) -> Result<Arc<Self>, Error> {
        Self::launch_group(node_package, &[(plugin, config)], services, events).await
    }

    /// Launch a group of plugins in one Cordis Context, loaded in order, so
    /// dependencies between them resolve natively. `services` lists the
    /// exported service slots and their methods.
    pub async fn launch_group(
        node_package: &Path,
        plugins: &[(&Path, Value)],
        services: Value,
        events: Option<Arc<dyn ServiceEvents>>,
    ) -> Result<Arc<Self>, Error> {
        Self::launch_mount(node_package, plugins, services, events, Vec::new()).await
    }

    /// Launch a group and provide rutis services to it: each host is
    /// registered in the Cordis Context before the plugins load, so their
    /// dependencies on it resolve natively.
    pub async fn launch_mount(
        node_package: &Path,
        plugins: &[(&Path, Value)],
        services: Value,
        events: Option<Arc<dyn ServiceEvents>>,
        hosts: Vec<Host>,
    ) -> Result<Arc<Self>, Error> {
        Self::mount(
            node_package,
            Mount {
                plugins: plugins.to_vec(),
                services,
                observer: events,
                hosts,
                ctx: None,
                runtime: None,
                anchor: None,
                launcher: None,
            },
        )
        .await
    }

    /// Launch a mount: see [`Mount`].
    pub async fn mount(node_package: &Path, mount: Mount<'_>) -> Result<Arc<Self>, Error> {
        let (anchor, launcher) = (mount.anchor, mount.launcher);
        // Without a launcher, the Node runtime.
        let name = far_name(
            &mount,
            if launcher.is_none() {
                "node"
            } else {
                "runtime"
            },
        );
        let (started, plugins) = Started::from(mount);
        let plugin = match (plugins.first(), anchor) {
            (Some((plugin, _)), _) => *plugin,
            (None, Some(anchor)) => anchor,
            (None, None) => {
                return Err(Error::Value(
                    "a mount needs at least one plugin or an anchor".into(),
                ))
            }
        };
        let (command, connect) = crate::runtime::spawn::command(launcher, node_package)?;
        let spawned = crate::runtime::spawn::spawn(command, plugin, connect).await?;
        Self::start(
            spawned.channel,
            Some(spawned.child),
            spawned.directory,
            started,
            crate::runtime::rpc::Format::Compat,
            &name,
        )
        .await
    }

    /// Run a mount on a runtime that is already connected: `channel` leads
    /// to a runtime process this side did not start (for example one that
    /// listens on a WebSocket and was dialed). [`Mount::launcher`] and
    /// [`Mount::anchor`] do not apply: that process was started with its own.
    /// It has no exit status here, and its end is the session's end.
    ///
    /// Network sessions use the endpoint [`Format`](crate::runtime::rpc::Format).
    pub async fn attach(
        channel: crate::channel::Channel,
        mount: Mount<'_>,
        format: crate::runtime::rpc::Format,
    ) -> Result<Arc<Self>, Error> {
        let name = far_name(&mount, "runtime");
        let (started, _) = Started::from(mount);
        Self::start(channel, None, None, started, format, &name).await
    }

    /// `name`: the runtime, for messages, when its greeting does not name
    /// it (the compat format).
    async fn start(
        channel: crate::channel::Channel,
        child: Option<crate::runtime::spawn::Child>,
        directory: Option<tempfile::TempDir>,
        started: Started,
        format: crate::runtime::rpc::Format,
        name: &str,
    ) -> Result<Arc<Self>, Error> {
        let (imports, rest) = Self::imports(started);
        let peer = Connection::open_with(channel, imports.clone(), format)?;
        peer.name_far_end(name);
        peer.ready().await?;
        Self::finish(peer, imports, child, directory, None, rest).await
    }

    /// What serves the runtime's calls into rutis, and the rest of the mount.
    fn imports(started: Started) -> (Arc<Imports>, MountRest) {
        let Started {
            plugins,
            services,
            events,
            hosts,
            provided,
            ctx,
            runtime,
        } = started;
        let plugins: Vec<Value> = plugins
            .iter()
            .map(|(entry, config)| json!({ "entry": entry, "config": config }))
            .collect();
        let imports = Arc::new(Imports {
            slots: Slots::default(),
            events,
            exported: Mutex::default(),
            rows: Mutex::default(),
            hosts: Mutex::new(hosts),
            bus: std::sync::OnceLock::new(),
            names: Mutex::default(),
            ended: Mutex::default(),
            failed: Mutex::default(),
            process: std::sync::OnceLock::new(),
        });
        let rest = MountRest {
            plugins,
            services,
            provided,
            ctx,
            runtime,
        };
        (imports, rest)
    }

    /// Run a mount on `session`, a runtime session something else owns:
    /// its calls into rutis are routed here while the process lives, and
    /// disposing or dropping the process leaves the session open.
    pub async fn over(
        session: Arc<dyn RuntimeSession>,
        mount: Mount<'_>,
    ) -> Result<Arc<Self>, Error> {
        let (started, _) = Started::from(mount);
        let (imports, rest) = Self::imports(started);
        let routed = session.route(imports.clone())?;
        Self::finish(
            session.connection(),
            imports,
            None,
            None,
            Some(routed),
            rest,
        )
        .await
    }

    async fn finish(
        peer: Connection,
        imports: Arc<Imports>,
        child: Option<crate::runtime::spawn::Child>,
        directory: Option<tempfile::TempDir>,
        routed: Option<Box<dyn std::any::Any + Send + Sync>>,
        rest: MountRest,
    ) -> Result<Arc<Self>, Error> {
        let MountRest {
            plugins,
            services,
            provided,
            ctx,
            runtime,
        } = rest;
        let scope = ctx.as_ref().map(rutis::Ctx::scope).unwrap_or_default();
        let process = Arc::new(Self {
            peer,
            imports,
            runtime: tokio::runtime::Handle::current(),
            child,
            features: std::sync::OnceLock::new(),
            about: std::sync::OnceLock::new(),
            plugins: std::sync::OnceLock::new(),
            host_changes: tokio::sync::Mutex::new(()),
            _directory: directory,
            routed,
            scope: scope.clone(),
            definitions: Mutex::default(),
            owned: Arc::default(),
        });
        let _ = process.imports.process.set(Arc::downgrade(&process));
        // The plugin definitions the process owns end with its session.
        let (connection, owned) = (process.peer.clone(), process.owned.clone());
        tokio::spawn(async move {
            connection.closed().await;
            let owned = std::mem::take(&mut *owned.lock().unwrap());
            for definition in owned {
                rutis::internal::definitions::end(&definition).await;
            }
        });
        // The session joins the logical bus before anything is mounted: the
        // runtime learns which names have listeners elsewhere first
        // (events protocol §4.5).
        let runtime = runtime.unwrap_or_else(|| "runtime".to_owned());
        let events = crate::events::SessionEvents::join(
            ctx.as_ref(),
            process.peer.clone(),
            runtime.clone(),
            scope.clone(),
        )?;
        let _ = process.imports.bus.set(events);
        let mounted = process
            .call_async(
                "",
                "mount",
                // `announce`: this side takes the services rows provide without
                // declaring them (`service` with `{ row, methods }`). `scope`:
                // the chain of the plugins of `plugins` (events protocol §3);
                // `runtime`: its name, as listener descriptors give it.
                json!({ "plugins": plugins, "services": services, "provided": provided, "announce": true, "scope": scope, "runtime": runtime }),
            )
            .await?;
        let slots: HashMap<String, (Option<String>, u64)> =
            crate::runtime::decode(mounted["services"].clone())?;
        for (name, (handle, version)) in slots {
            process.imports.update(name, handle, version);
        }
        // A runner older than 0.3.0 reports no features.
        let features: Vec<String> =
            crate::runtime::decode(mounted.get("features").cloned().unwrap_or(json!([])))?;
        let _ = process.features.set(features);
        let about: serde_json::Map<String, Value> = ["implementation", "engine"]
            .into_iter()
            .filter_map(|field| Some((field.to_owned(), mounted.get(field)?.clone())))
            .collect();
        let _ = process.about.set(Value::Object(about));
        // Optional: a runtime that does not list its plugins is asked for
        // each one (`rows.schema`).
        let plugins: Option<HashMap<String, RowSchema>> = match mounted.get("plugins") {
            None | Some(Value::Null) => None,
            Some(plugins) => Some(crate::runtime::decode(plugins.clone())?),
        };
        let _ = process.plugins.set(plugins);
        Ok(process)
    }

    /// Load one plugin into the Context as row `key` (rows mode, see
    /// [`Mount::anchor`]). `isolate` gives (service, label) pairs: rows
    /// naming the same label share that service's scope; `inject` lists
    /// extra services the row waits for. `scope` is the row's scope chain
    /// (`[]` at the root, [`rutis::Ctx::scope`] of the context that loads
    /// it): its listeners hear the events emitted there and below, and its
    /// emissions are emitted there (events protocol §3). Resolves once the
    /// plugin settled, its listeners on the bus.
    pub async fn load_row(
        &self,
        key: &str,
        entry: &Path,
        config: Value,
        isolate: &[(String, String)],
        inject: &[String],
        scope: &[rutis::events::Scope],
    ) -> Result<(), Error> {
        let id = self.row_loading(key, scope);
        let loaded = self
            .call_async(
                "",
                "rows.load",
                json!([key, entry, config, isolate, inject, {}, { "scope": scope, "id": id }]),
            )
            .await;
        if loaded.is_err() {
            // Never loaded, so it cannot end.
            self.imports.forget_row(key);
            self.row_gone(key);
        }
        loaded.map(|_| ())
    }

    /// Name row `key` `id`, the id its configuration gives it, before it
    /// is loaded: listener descriptions and the lines that report its
    /// failures call it so (events protocol §4.2). Keys are unique on the
    /// session; ids need not be (a group's instances copy its rows). A row
    /// not named is called by its key.
    pub fn name_row(&self, key: &str, id: &str) {
        self.imports
            .names
            .lock()
            .unwrap()
            .insert(key.to_owned(), id.to_owned());
    }

    /// Row `key` is being loaded in `scope`: its listeners may register
    /// from now on. Its id (`rows.load`'s `id`).
    fn row_loading(&self, key: &str, scope: &[rutis::events::Scope]) -> String {
        let id = self
            .imports
            .names
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_owned());
        if let Some(events) = self.imports.bus.get() {
            events.row_loading(key, scope.to_vec(), id.clone());
        }
        id
    }

    /// Row `key` is gone: its listeners leave the bus (events protocol
    /// §7.1).
    fn row_gone(&self, key: &str) {
        self.imports.names.lock().unwrap().remove(key);
        if let Some(events) = self.imports.bus.get() {
            events.row_ended(key);
        }
    }

    /// Load row `key` like [`Process::load_row`], exporting the services in
    /// `exports` (`{ name: { method: "sync" | "async" } }`) to `observer`:
    /// it hears every change of each service, read from the row's scope,
    /// until the row is unloaded. A service another row exports in the same
    /// scope is refused; a name the row isolates is exported in the scope
    /// of its label ([`scoped_id`]), which needs the runtime's `scopes`.
    ///
    /// A runtime with `rows.announce` also announces the services the row
    /// provides without declaring them, with the methods each has:
    /// `observer` follows each one it accepts
    /// ([`ServiceEvents::announced`]) like a declared one.
    #[allow(clippy::too_many_arguments)]
    pub async fn load_row_exporting(
        &self,
        key: &str,
        entry: &Path,
        config: Value,
        isolate: &[(String, String)],
        inject: &[String],
        exports: &serde_json::Map<String, Value>,
        observer: Arc<dyn ServiceEvents>,
        scope: &[rutis::events::Scope],
    ) -> Result<(), Error> {
        let entry = RpcValue::Data(json!(entry));
        let loaded = self
            .load_exporting(
                key, entry, config, isolate, inject, exports, observer, scope,
            )
            .await;
        if loaded.is_err() {
            // Never loaded, so it cannot end.
            self.imports.forget_row(key);
            self.row_gone(key);
        }
        loaded
    }

    /// [`Process::load_row_exporting`] for one copy of a plugin definition
    /// the runtime handed over (`definition`, a reference of this session):
    /// the definition goes back to the runtime as the row's entry
    /// (`docs/design-plugin-values-2026-10-25.md` §5).
    #[allow(clippy::too_many_arguments)]
    pub async fn load_definition_exporting(
        &self,
        key: &str,
        definition: &crate::session::Reference,
        config: Value,
        isolate: &[(String, String)],
        inject: &[String],
        exports: &serde_json::Map<String, Value>,
        observer: Arc<dyn ServiceEvents>,
        scope: &[rutis::events::Scope],
    ) -> Result<(), Error> {
        self.require_plugin_values()?;
        let entry = RpcValue::Reference(definition.clone());
        let loaded = self
            .load_exporting(
                key, entry, config, isolate, inject, exports, observer, scope,
            )
            .await;
        if loaded.is_err() {
            self.imports.forget_row(key);
            self.row_gone(key);
        }
        loaded
    }

    /// What the plugin definition `value` (a reference of this session)
    /// declares, read from the runtime that defined it (`plugins.read`).
    pub async fn read_plugin(
        &self,
        value: &crate::session::Reference,
    ) -> Result<PluginSchema, Error> {
        self.require_plugin_values()?;
        let read = crate::runtime::rpc::settle(
            self.peer
                .invoke_async(
                    "",
                    "plugins.read",
                    RpcValue::List(vec![RpcValue::Reference(value.clone())]),
                )
                .await?,
        )
        .await?;
        crate::runtime::decode(read.json()?)
    }

    fn require_plugin_values(&self) -> Result<(), Error> {
        match self.supports("plugin-values") {
            true => Ok(()),
            false => Err(Error::Value(
                "the runtime's SDK is too old to hand over plugin definitions \
                 (it lacks the feature plugin-values)"
                    .into(),
            )),
        }
    }

    /// The scope chain the process was mounted in.
    pub(crate) fn scope(&self) -> &[rutis::events::Scope] {
        &self.scope
    }

    /// The definition already made for the value whose identity is
    /// `identity`, if it is still held.
    pub(crate) fn known_definition(&self, identity: usize) -> Option<rutis::Definition> {
        let mut known = self.definitions.lock().unwrap();
        known.retain(|_, weak| weak.upgrade().is_some());
        known.get(&identity).and_then(|weak| weak.upgrade())
    }

    /// Remember `definition`, made for the value `identity`; one that
    /// belongs to the process itself ends with its session.
    pub(crate) fn remember_definition(
        &self,
        identity: usize,
        definition: &rutis::Definition,
        owned: bool,
    ) {
        self.definitions.lock().unwrap().insert(
            identity,
            rutis::internal::definitions::WeakDefinition::new(definition),
        );
        if owned {
            self.owned.lock().unwrap().push(definition.clone());
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn load_exporting(
        &self,
        key: &str,
        entry: RpcValue,
        config: Value,
        isolate: &[(String, String)],
        inject: &[String],
        exports: &serde_json::Map<String, Value>,
        observer: Arc<dyn ServiceEvents>,
        scope: &[rutis::events::Scope],
    ) -> Result<(), Error> {
        if !exports.is_empty() {
            self.require("rows.v2")?;
        }
        for name in exports.keys() {
            check_scoped(name, label_of(isolate, name))?;
        }
        let scoped: Vec<(String, String)> = exports
            .keys()
            .map(|name| (scoped_id(name, label_of(isolate, name)), name.clone()))
            .collect();
        if exports.keys().any(|name| label_of(isolate, name).is_some()) {
            self.require("scopes")?;
        }
        {
            // Registered before loading: the row's services may appear
            // while it starts.
            let mut exported = self.imports.exported.lock().unwrap();
            if let Some((id, _)) = scoped.iter().find(|(id, _)| exported.contains_key(id)) {
                return Err(Error::Value(format!("service {id} is already exported")));
            }
            for (id, name) in &scoped {
                exported.insert(id.clone(), (name.clone(), observer.clone()));
            }
        }
        let ids = scoped.into_iter().map(|(id, _)| id).collect();
        self.imports
            .rows
            .lock()
            .unwrap()
            .insert(key.to_owned(), RowExports { observer, ids });
        let id = self.row_loading(key, scope);
        let args = RpcValue::List(vec![
            RpcValue::Data(json!(key)),
            entry,
            RpcValue::Data(config),
            RpcValue::Data(json!(isolate)),
            RpcValue::Data(json!(inject)),
            RpcValue::Data(json!(exports)),
            RpcValue::Data(json!({ "scope": scope, "id": id })),
        ]);
        let loaded = async {
            crate::runtime::rpc::settle(self.peer.invoke_async("", "rows.load", args).await?).await
        }
        .await;
        if loaded.is_err() {
            self.forget_exports(key);
        }
        loaded.map(|_| ())
    }

    /// Stop following the services row `key` exported, withdrawing them
    /// first. The runtime withdraws them before it answers `rows.unload`,
    /// but its notification runs as a task of its own and may reach the
    /// observer only after this: without the withdrawal here, a late one
    /// finds no observer and the service stays projected.
    fn forget_exports(&self, key: &str) {
        let ids = self.imports.rows.lock().unwrap().remove(key);
        let ids = ids.into_iter().flat_map(|exports| exports.ids);
        for (name, observer, version) in self.imports.forget(ids) {
            observer.changed(&name, None, version);
        }
    }

    /// Give row `key` a new config. Cordis commits volatile values in place
    /// (`loader/volatile-update`, as dsh's loader does); any other change
    /// restarts the row with the config. An inactive row keeps it for its
    /// next activation.
    pub async fn update_row(&self, key: &str, config: Value) -> Result<(), Error> {
        self.call_async("", "rows.update", json!([key, config]))
            .await
            .map(|_| ())
    }

    /// Dispose row `key`.
    pub async fn unload_row(&self, key: &str) -> Result<(), Error> {
        // Withdrawn when `rows.unload` is sent (events protocol §7.1).
        self.row_gone(key);
        let unloaded = self.call_async("", "rows.unload", json!([key])).await;
        // After the call: the withdrawals of the row's services arrive
        // before its reply.
        self.forget_exports(key);
        self.imports.forget_row(key);
        unloaded.map(|_| ())
    }

    /// Run `ended` if the plugin of row `key` ends on its own in the runtime
    /// (a Cordis plugin disposing its own fiber): the runtime has unloaded
    /// what was left of the row by then. Register it before loading the
    /// row, since the plugin may end while it starts; it is dropped when
    /// the row is unloaded ([`Process::unload_row`]) or fails to load. A
    /// row that fails to load never ends this way: its load reports the
    /// error instead.
    ///
    /// The runtime ends such a row whether or not anything is registered
    /// for it: without `ended`, nobody hears it, a later `rows.update` of
    /// the row fails as for a row that is not loaded, and
    /// [`Process::unload_row`] still succeeds.
    pub fn on_row_ended(&self, key: &str, ended: impl FnOnce() + Send + 'static) {
        self.imports
            .ended
            .lock()
            .unwrap()
            .insert(key.to_owned(), Box::new(ended));
    }

    /// Run `failed` with why if the runtime fails row `key` while it runs:
    /// an error the row's code left unhandled (a promise rejection, an
    /// exception in a task), which the runtime traced to the row
    /// (`rows.failed`, #688). The runtime keeps running and keeps the row
    /// loaded: what to do with it is the host's (the loader restarts it,
    /// and the restart fails with why). Registered and dropped as
    /// [`Process::on_row_ended`] is: before the load, as the error may come
    /// right after it. A runtime fails a row this way only once it loaded:
    /// an error while it loads fails the load instead. With nothing
    /// registered the call fails, and the runtime writes why itself.
    pub fn on_row_failed(&self, key: &str, failed: impl FnOnce(String) + Send + 'static) {
        self.imports
            .failed
            .lock()
            .unwrap()
            .insert(key.to_owned(), Box::new(failed));
    }

    /// What the plugin at `entry` declares: its config schema (schemastery
    /// `Config` converted), the services it injects, and the services it
    /// provides to rutis.
    pub async fn describe_row(&self, entry: &Path) -> Result<RowSchema, Error> {
        self.require("rows.v2")?;
        crate::runtime::decode(self.call_async("", "rows.schema", json!([entry])).await?)
    }

    /// What the runtime reported of itself when mounted, for diagnostics:
    /// `implementation` (`{ name, version }`) and `engine` (`{ name,
    /// version }`, such as the Bun that runs it), each when it said.
    pub fn about(&self) -> &Value {
        static NOTHING: Value = Value::Null;
        self.about.get().unwrap_or(&NOTHING)
    }

    /// The plugins the runtime has, by the name a row gives them, each as
    /// [`Process::describe_row`] would describe it: when its `mount` reply
    /// listed them (`plugins`). `None` for a runtime that did not, whose
    /// plugins are described one by one.
    pub fn plugins(&self) -> Option<&HashMap<String, RowSchema>> {
        self.plugins.get().and_then(Option::as_ref)
    }

    /// Whether the Node runtime supports `feature` (reported when mounting).
    pub fn supports(&self, feature: &str) -> bool {
        self.features
            .get()
            .is_some_and(|features| features.iter().any(|f| f == feature))
    }

    fn require(&self, feature: &str) -> Result<(), Error> {
        match self.supports(feature) {
            true => Ok(()),
            false => Err(Error::Value(format!(
                "the Node runtime lacks `{feature}`: @arcships/rutis-runtime 0.3.0 or later is required"
            ))),
        }
    }

    /// Lease the host service `name` for a row: the first lease registers
    /// it with the Node side (methods from `methods`, else from
    /// [`HostDispatch::methods`]); the last one released withdraws it. A
    /// newer lease's dispatch replaces the older one's, since rows restart
    /// when their provider changes.
    pub async fn lease_host(
        self: &Arc<Self>,
        name: &str,
        dispatch: Arc<dyn HostDispatch>,
        methods: Option<Value>,
    ) -> Result<HostLease, Error> {
        self.lease_host_in(name, None, dispatch, methods).await
    }

    /// [`Process::lease_host`] in the scope `label` (the row's `isolate`
    /// label for `name`): only rows isolating `name` with that label see
    /// it, and the same name in other scopes is another service. A label
    /// needs the runtime's `scopes`.
    pub async fn lease_host_in(
        self: &Arc<Self>,
        name: &str,
        label: Option<&str>,
        dispatch: Arc<dyn HostDispatch>,
        methods: Option<Value>,
    ) -> Result<HostLease, Error> {
        self.lease(name, label, dispatch, methods, None).await
    }

    /// [`Process::lease_host_in`] for `row` alone: while the lease lasts,
    /// [`caller_row`](crate::session::caller_row) reports `row` (in the
    /// scope `label`) for the calls it makes to the service. The service
    /// is registered in a scope of the row's own, whose label
    /// ([`HostLease::label`]) the row isolates `name` under when it is
    /// loaded, instead of `label`. The lease also takes
    /// [`Process::lease_host_in`]'s shared registration in `label`, as
    /// before: code of other rows that looks the name up without declaring
    /// it (a Cordis `ctx.get`) still finds it there, and those calls name
    /// no row. This needs the runtime's `scopes` (every runtime rutis
    /// ships has it): in a runtime without it, the lease is only the
    /// shared one, and its calls name no row.
    pub async fn lease_host_for(
        self: &Arc<Self>,
        name: &str,
        label: Option<&str>,
        row: &CallerRow,
        dispatch: Arc<dyn HostDispatch>,
        methods: Option<Value>,
    ) -> Result<HostLease, Error> {
        let shared = self
            .lease_host_in(name, label, dispatch.clone(), methods.clone())
            .await?;
        if !self.supports("scopes") {
            return Ok(shared);
        }
        let own = row_label(label, row.key());
        let row = row.in_scope(label);
        match self
            .lease(name, Some(&own), dispatch, methods, Some(row))
            .await
        {
            Ok(mut lease) => {
                lease.shared = Some(Box::new(shared));
                Ok(lease)
            }
            Err(error) => {
                let _ = shared.release().await;
                Err(error)
            }
        }
    }

    async fn lease(
        self: &Arc<Self>,
        name: &str,
        label: Option<&str>,
        dispatch: Arc<dyn HostDispatch>,
        methods: Option<Value>,
        row: Option<CallerRow>,
    ) -> Result<HostLease, Error> {
        self.require("hosts")?;
        if label.is_some() {
            self.require("scopes")?;
        }
        check_scoped(name, label)?;
        let id = scoped_id(name, label);
        let _changing = self.host_changes.lock().await;
        let first = {
            let mut hosts = self.imports.hosts.lock().unwrap();
            match hosts.get_mut(&id) {
                Some(entry) => {
                    entry.leases = entry
                        .leases
                        .checked_add(1)
                        .ok_or_else(|| Error::Value(format!("host lease overflow for {id}")))?;
                    entry.dispatch = dispatch.clone();
                    entry.row = row;
                    false
                }
                None => {
                    hosts.insert(
                        id.clone(),
                        HostEntry {
                            dispatch: dispatch.clone(),
                            leases: 1,
                            row,
                        },
                    );
                    true
                }
            }
        };
        if first {
            let methods = methods
                .or_else(|| dispatch.methods())
                .unwrap_or_else(|| json!({}));
            let args = match label {
                None => json!([name, methods]),
                Some(label) => json!([name, methods, label]),
            };
            if let Err(error) = self.call_async("", "hosts.provide", args).await {
                self.imports.hosts.lock().unwrap().remove(&id);
                return Err(error);
            }
        }
        Ok(HostLease {
            process: self.clone(),
            id,
            label: label.map(str::to_owned),
            shared: None,
        })
    }

    async fn release_host(&self, id: &str) -> Result<(), Error> {
        let _changing = self.host_changes.lock().await;
        let last = {
            let mut hosts = self.imports.hosts.lock().unwrap();
            let Some(entry) = hosts.get_mut(id) else {
                return Ok(());
            };
            entry.leases -= 1;
            let last = entry.leases == 0;
            if last {
                hosts.remove(id);
            }
            last
        };
        match last {
            true => self
                .call_async("", "hosts.withdraw", json!([id]))
                .await
                .map(|_| ()),
            false => Ok(()),
        }
    }

    /// The handle of the object currently in an exported slot, if available.
    pub fn service(&self, name: &str) -> Option<String> {
        self.imports
            .slots
            .lock()
            .unwrap()
            .get(name)
            .and_then(|(handle, _)| handle.clone())
    }

    /// Tell the Cordis side that no Rust proxy uses `handle` any longer.
    /// Best effort: a closed session has already released everything.
    pub fn release(&self, handle: &str) {
        let peer = self.peer.clone();
        let handle = handle.to_owned();
        self.runtime.spawn(async move {
            let _ = peer
                .invoke_async("", "release", json!([handle]).into())
                .await;
        });
    }

    pub fn connection(&self) -> &Connection {
        &self.peer
    }

    pub fn call(&self, service: &str, method: &str, args: Value) -> Result<Value, Error> {
        self.peer.invoke(service, method, args.into())?.json()
    }

    /// Call a method with protocol arguments (keeps `undefined` distinct);
    /// the result may contain object references (see `decode_value`).
    pub fn invoke(
        &self,
        handle: &str,
        method: &str,
        args: Vec<RpcValue>,
    ) -> Result<RpcValue, Error> {
        self.peer.invoke(handle, method, RpcValue::List(args))
    }

    /// Read a declared property of the service object `handle` (live).
    pub fn get(&self, handle: &str, property: &str) -> Result<RpcValue, Error> {
        self.peer
            .invoke("", "get", json!([handle, property]).into())
    }

    /// Asynchronous form of [`Process::invoke`]: awaits a returned Promise.
    pub async fn invoke_async(
        &self,
        handle: &str,
        method: &str,
        args: Vec<RpcValue>,
    ) -> Result<RpcValue, Error> {
        crate::runtime::rpc::settle(
            self.peer
                .invoke_async(handle, method, RpcValue::List(args))
                .await?,
        )
        .await
    }

    pub async fn call_async(
        &self,
        service: &str,
        method: &str,
        args: Value,
    ) -> Result<Value, Error> {
        crate::runtime::rpc::settle(self.peer.invoke_async(service, method, args.into()).await?)
            .await?
            .json()
    }

    pub async fn dispose(&self) -> Result<(), Error> {
        // A session something else owns stays, and so does its runtime; its
        // rows were unloaded one by one.
        if self.routed.is_some() {
            return Ok(());
        }
        let result = self.call_async("", "dispose", Value::Null).await;
        self.peer
            .close(Error::Transport("plugin has been disposed".into()));
        let Some(child) = &self.child else {
            return result.map(|_| ());
        };
        let status = child.exited().await;
        result?;
        if status != "exited normally" {
            return Err(Error::Transport(format!("Cordis process {status}")));
        }
        Ok(())
    }

    /// How the Node process ended (for example `exited with signal: 9
    /// (SIGKILL)`), or `None` while it runs.
    pub fn exit_status(&self) -> Option<String> {
        self.child.as_ref().and_then(|child| child.status())
    }

    /// Resolves once the session with the Node process has ended, whether
    /// by disposal or because the process went away.
    pub async fn closed(&self) {
        self.peer.closed().await
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // A session something else owns stays; only the routing goes.
        if self.routed.is_none() {
            self.peer.close(Error::Transport("process dropped".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ignore;

    impl ServiceEvents for Ignore {
        fn changed(&self, _: &str, _: Option<String>, _: u64) {}
    }

    fn empty() -> Imports {
        Imports {
            slots: Slots::default(),
            events: None,
            exported: Mutex::default(),
            rows: Mutex::default(),
            hosts: Mutex::default(),
            bus: std::sync::OnceLock::new(),
            names: Mutex::default(),
            ended: Mutex::default(),
            failed: Mutex::default(),
            process: std::sync::OnceLock::new(),
        }
    }

    /// A row's slots go when it is forgotten, and a withdrawal arriving
    /// after that leaves nothing either: rows loaded and unloaded over and
    /// over (instances created and closed) do not pile up slots (#191).
    #[test]
    fn forgotten_slots_leave_nothing_behind() {
        let imports = empty();
        for instance in 0..100 {
            let label = format!("rutis-loader/instance/{instance}");
            let id = scoped_id("greeter", Some(&label));
            imports
                .exported
                .lock()
                .unwrap()
                .insert(id.clone(), ("greeter".into(), Arc::new(Ignore)));
            imports.update(id.clone(), Some(format!("{id}#1")), 2 * instance + 1);
            let forgotten = imports.forget([id.clone()]);
            assert_eq!(forgotten.len(), 1);
            assert_eq!(forgotten[0].2, 2 * instance + 1, "the slot's last version");
            // The runtime's withdrawal, late.
            imports.update(id, None, 2 * instance + 2);
        }
        assert!(imports.slots.lock().unwrap().is_empty());
        assert!(imports.exported.lock().unwrap().is_empty());
    }

    /// Accepts the announcements of `name` only, and records each.
    struct Announcements {
        name: &'static str,
        heard: Mutex<Vec<(String, Value)>>,
    }

    impl ServiceEvents for Announcements {
        fn changed(&self, _: &str, _: Option<String>, _: u64) {}

        fn announced(&self, name: &str, methods: &Value) -> bool {
            self.heard
                .lock()
                .unwrap()
                .push((name.to_owned(), methods.clone()));
            name == self.name
        }
    }

    /// A service a row announces is followed once its observer accepts it,
    /// and is forgotten with the row; one it declines, or one of a row
    /// loaded without an observer, is not followed at all.
    #[test]
    fn announced_slots_are_followed_when_accepted() {
        let imports = empty();
        let observer = Arc::new(Announcements {
            name: "store",
            heard: Mutex::default(),
        });
        imports.rows.lock().unwrap().insert(
            "row".into(),
            RowExports {
                observer: observer.clone(),
                ids: Vec::new(),
            },
        );
        let methods = json!({ "get": "async" });
        let scoped = scoped_id("store", Some("box"));
        assert!(imports.announce("row", &scoped, &methods));
        assert!(!imports.announce("row", "kept", &methods));
        assert!(!imports.announce("other", "store", &methods));
        assert_eq!(
            observer.heard.lock().unwrap()[..],
            [
                ("store".to_owned(), methods.clone()),
                ("kept".to_owned(), methods.clone())
            ]
        );
        assert_eq!(imports.exported.lock().unwrap()[&scoped].0, "store");
        assert_eq!(
            imports.rows.lock().unwrap()["row"].ids,
            std::slice::from_ref(&scoped)
        );
        // A new object of it is announced again: followed once.
        assert!(imports.announce("row", &scoped, &json!({ "get": "async", "set": "async" })));
        assert_eq!(imports.rows.lock().unwrap()["row"].ids.len(), 1);
        let ids = imports.rows.lock().unwrap().remove("row").unwrap().ids;
        assert_eq!(imports.forget(ids).len(), 1);
        assert!(imports.exported.lock().unwrap().is_empty());
    }

    /// A slot still followed keeps its withdrawal (and its version), and so
    /// does a mount's, which an observer follows as a whole.
    #[test]
    fn followed_slots_keep_their_withdrawals() {
        let imports = empty();
        imports
            .exported
            .lock()
            .unwrap()
            .insert("greeter".into(), ("greeter".into(), Arc::new(Ignore)));
        imports.update("greeter".into(), None, 3);
        // An older publication arriving late does not win.
        imports.update("greeter".into(), Some("greeter".into()), 2);
        assert_eq!(imports.slots.lock().unwrap()["greeter"], (None, 3));

        let mount = Imports {
            events: Some(Arc::new(Ignore)),
            ..empty()
        };
        mount.update("counter".into(), None, 1);
        assert_eq!(mount.slots.lock().unwrap()["counter"], (None, 1));
    }
}
