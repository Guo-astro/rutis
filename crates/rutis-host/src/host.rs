//! A running host: the kernel, the local language runtimes the
//! configuration names, the transports and identity links use, and the
//! loader that runs the rows.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rutis::{BoxFuture, CordisError, Ctx, Effect, FiberView, Plugin, TypeKey};
use rutis_bridge::runtime::{LocalRuntime, RuntimeHandle, RuntimePlugin, RuntimeState};
use rutis_bridge::transport::local::LocalPlugin;
use rutis_bridge::transport::websocket::{
    Config, ListenerConfig, ServerTls, Trust, WebSocketPlugin,
};
use rutis_bridge::{Credential, IdentityPlugin, PeerId, StaticIdentity};
use rutis_loader::{
    register_peer_node, Builtins, Chain, GoBinaries, GoResolver, GoRuntimes, GoRuntimesHandle,
    Layer, Loader, LoaderOptions, LoaderPlugin, Patch, PeerResolver, RuntimeResolver, RuntimeRows,
    RuntimeRowsPlugin, ServiceCatalog,
};

use crate::config::{
    token, venv_python, BunRuntime, GoRuntime, GoStart, HostConfig, NodeRuntime, PythonRuntime,
    DEFAULT_PYTHON,
};

pub struct Host {
    /// The host runs while this lives.
    root: Ctx,
    pub loader: Loader,
    pub runtimes: Vec<(String, RuntimeHandle)>,
    /// The resolvers of the runtimes' rows, to invalidate after a change.
    pub resolvers: Vec<Arc<RuntimeResolver>>,
    /// The Go binaries and their runtimes, when the configuration has them.
    pub go: Option<GoHost>,
    /// The local runtimes' plugins, by name, to stop one
    /// ([`Host::stop_runtime`]).
    fibers: Vec<(String, FiberView)>,
    /// The `rutis-bridge/peer` rows' resolver, which also knows their links'
    /// states ([`PeerResolver::link_state`]).
    pub peers: Arc<PeerResolver>,
    /// Each configured listener (`listen`) and the address it is bound to,
    /// in the configuration's order: with port 0, the port picked.
    pub listening: Vec<(String, std::net::SocketAddr)>,
}

/// Registers rows compiled into a program ([`Embedding::builtins`]).
pub type RegisterBuiltins = Box<dyn FnOnce(&mut Builtins) + Send>;

/// What a Rust program embedding this host adds to what the configuration
/// names (the E2E fixture host, `tests/e2e/fixture-host`).
#[derive(Default)]
pub struct Embedding {
    /// Rows compiled into the program, next to `rutis-bridge/peer`.
    pub builtins: Option<RegisterBuiltins>,
    /// The identity links use instead of the one built from the
    /// configuration's peer rows: for a program that adds links later.
    pub identity: Option<IdentityPlugin>,
}

/// The Go side of a host.
pub struct GoHost {
    pub resolver: Arc<GoResolver>,
    pub runtimes: GoRuntimesHandle,
}

pub type Error = String;

/// How long the runtime processes may take to exit once every cleanup
/// finished, however little of the deadline is left: longer than the
/// local transport gives a process whose channel closed before killing it.
pub const PROCESS_EXIT: Duration = Duration::from_secs(3);

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
        let root = Ctx::root().map_err(|error| error.to_string())?;
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
        match embedding.identity {
            Some(identity) => root.plugin(identity),
            None => root.plugin(IdentityPlugin::new("host", identity(config)?)),
        };

        let mut runtimes = Vec::new();
        let mut resolvers = Vec::new();
        let mut fibers = Vec::new();
        let peers = Arc::new(PeerResolver::new());
        let mut builtins = Builtins::new();
        register_peer_node(&mut builtins, peers.clone());
        if let Some(register) = embedding.builtins {
            register(&mut builtins);
        }
        let mut chain = Chain::new().with(builtins).with_shared(peers.clone());
        let go = match &config.runtimes.go {
            Some(go) => {
                let resolver = Arc::new(go_resolver(config, go, &catalog)?);
                chain = chain.with_shared(resolver.clone());
                Some((resolver, go))
            }
            None => None,
        };
        if let Some(py) = &config.runtimes.py {
            let runtime = python(py)?;
            let handle = runtime.handle();
            let rows = Arc::new(RuntimeResolver::modules(handle.clone()).with_catalog(&catalog));
            fibers.push(("py".to_owned(), root.plugin(runtime)));
            chain = chain.with_shared(rows.clone());
            runtimes.push(("py".to_owned(), handle));
            resolvers.push(rows);
        }
        if let Some(bun) = &config.runtimes.bun {
            let runtime = bun_runtime(bun)?;
            let handle = runtime.handle();
            let rows = Arc::new(RuntimeResolver::modules(handle.clone()).with_catalog(&catalog));
            fibers.push(("bun".to_owned(), root.plugin(runtime)));
            chain = chain.with_shared(rows.clone());
            runtimes.push(("bun".to_owned(), handle));
            resolvers.push(rows);
        }
        if let Some(node) = &config.runtimes.node {
            let runtime = node_runtime(node)?;
            let handle = runtime.handle();
            let rows = Arc::new(RuntimeResolver::node(handle.clone()).with_catalog(&catalog));
            fibers.push(("node".to_owned(), root.plugin(runtime)));
            chain = chain.with_shared(rows.clone());
            runtimes.push(("node".to_owned(), handle));
            resolvers.push(rows);
        }

        for remote in &config.runtimes.remote {
            let runtime = RuntimePlugin::remote(&remote.name);
            let handle = runtime.handle();
            let rows = match remote.language.as_str() {
                "python" | "py" | "go" => RuntimeResolver::modules(handle.clone()),
                "node" => RuntimeResolver::node(handle.clone()),
                other => {
                    return Err(format!(
                        "remote runtime {}: the language is python, go or node, not {other}",
                        remote.name
                    ))
                }
            };
            let rows = Arc::new(rows.with_catalog(&catalog));
            root.plugin(runtime);
            chain = chain.with_shared(rows.clone());
            resolvers.push(rows);
            // Not waited for at start: it runs when its link does.
        }

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
        let go = match go {
            Some((resolver, settings)) => {
                let mut plugin = GoRuntimes::new(resolver.clone(), &settings.project);
                plugin = match settings.start {
                    GoStart::Eager => plugin.eager(),
                    GoStart::OnDemand => plugin.idle(Some(std::time::Duration::from_secs(
                        settings.idle.unwrap_or(60),
                    ))),
                };
                let runtimes = plugin.handle();
                (&root.plugin(plugin))
                    .await
                    .map_err(|error| format!("the Go runtimes: {error}"))?;
                Some(GoHost { resolver, runtimes })
            }
            None => None,
        };
        Ok(Self {
            root,
            loader,
            runtimes,
            resolvers,
            go,
            fibers,
            peers,
            listening,
        })
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
            result?;
        }
        Ok(())
    }

    /// Load `rows` (replacing what was loaded before).
    pub async fn load(&self, rows: Vec<serde_json::Value>) -> Result<(), Error> {
        let patches: Vec<Patch> = serde_json::from_value(serde_json::json!([{ "insert": rows }]))
            .map_err(|error| format!("rows: {error}"))?;
        self.loader
            .reconcile(vec![Layer::new("rutis.json", patches)], None)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Check `rows` without running them ([`rutis_loader::Loader::check`]).
    pub async fn check(
        &self,
        rows: Vec<serde_json::Value>,
    ) -> Result<rutis_loader::CheckReport, Error> {
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
        self.root
            .shutdown_with_timeout(deadline)
            .await
            .map_err(|error| {
                Unfinished::Cleanups(match error {
                    rutis::DisposeWaitError::TimedOut { .. } => {
                        format!("{}s passed", deadline.as_secs_f64())
                    }
                    rutis::DisposeWaitError::Failed(error) => {
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
            eprintln!("rutis-host: processes {left:?} did not end when killed");
        }
    }

    /// What has not stopped yet: the plugins stopping (or still starting),
    /// and how many others wait for them to stop first.
    pub fn still_running(&self) -> (Vec<String>, usize) {
        use rutis::FiberState;
        let root = self.root.root_view().map(|root| root.id);
        let mut stopping = Vec::new();
        let mut waiting = 0;
        for plugin in self.root.diagnostics().plugins {
            if Some(plugin.id) == root {
                continue;
            }
            match plugin.state {
                FiberState::Disposed | FiberState::Failed => {}
                FiberState::Unloading => stopping.push(plugin.name),
                FiberState::Loading => stopping.push(format!("{} (still starting)", plugin.name)),
                _ => waiting += 1,
            }
        }
        (stopping, waiting)
    }

    /// The service `name` as rutis sees it (what a row provides).
    #[allow(dead_code)]
    pub fn service(&self, name: &str) -> Option<Arc<dyn rutis_bridge::session::HostDispatch>> {
        self.root.get_as::<dyn rutis_bridge::session::HostDispatch>(
            rutis_bridge::session::host_key(name),
        )
    }

    /// Forget what the runtimes' resolvers cached, so changed plugins are
    /// described again.
    pub fn invalidate(&self) {
        for rows in &self.resolvers {
            rows.invalidate_all();
        }
    }
}

/// The resolver of the Go binaries `go` names. A Go runtime may not take
/// another runtime's name.
fn go_resolver(
    config: &HostConfig,
    go: &GoRuntime,
    catalog: &ServiceCatalog,
) -> Result<GoResolver, Error> {
    go.validate()?;
    let mut binaries = GoBinaries::new();
    if let Some(dir) = &go.dir {
        binaries = binaries.dir(dir);
    }
    for binary in &go.binaries {
        binaries = binaries.file(binary);
    }
    if let Some((binary, name)) = &go.dev {
        binaries = binaries.file_named(binary, name);
    }
    let reserved = ["node".to_owned(), "py".to_owned(), "bun".to_owned()]
        .into_iter()
        .chain(
            config
                .runtimes
                .remote
                .iter()
                .map(|remote| remote.name.clone()),
        );
    Ok(GoResolver::new(binaries)
        .with_catalog(catalog)
        .reserve(reserved))
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

fn identity(config: &HostConfig) -> Result<StaticIdentity, Error> {
    let local = PeerId::new(config.id.clone()).map_err(|error| format!("id: {error}"))?;
    let mut identity = StaticIdentity::new(local);
    for (peer, dials) in config.peers() {
        let Some(token) = token(&peer) else { continue };
        let peer = PeerId::new(peer.clone()).map_err(|error| format!("peer {peer}: {error}"))?;
        identity = match dials {
            true => identity.present(peer, Credential::Bearer(token)),
            false => identity.accept_token(token, peer),
        };
    }
    Ok(identity)
}

/// The Node runtime of `node.project`, with the `@arcships/rutis-runtime`
/// package installed there (or given, or next to this program).
fn node_runtime(node: &NodeRuntime) -> Result<LocalRuntime, Error> {
    let anchor = node.project.join("package.json");
    if !anchor.exists() {
        return Err(format!(
            "the Node runtime needs a package.json in {}: run `npm init -y` there",
            node.project.display()
        ));
    }
    let candidates = [
        node.runtime.clone(),
        Some(node.project.join("node_modules/@arcships/rutis-runtime")),
        std::env::var_os("RUTIS_NODE_RUNTIME").map(PathBuf::from),
    ];
    let package = candidates
        .into_iter()
        .flatten()
        .find(|package| package.join("package.json").exists())
        .ok_or_else(|| {
            format!(
                "the Node runtime is not installed in {}: run `npm install @arcships/rutis-runtime` there",
                node.project.display()
            )
        })?;
    Ok(LocalRuntime::node(package, anchor))
}

/// The Bun runtime of `bun.project`, with the `@arcships/rutis-bun` package
/// installed there (or given), run by a Bun that answers.
fn bun_runtime(bun: &BunRuntime) -> Result<LocalRuntime, Error> {
    let candidates = [
        bun.runtime.clone(),
        Some(bun.project.join("node_modules/@arcships/rutis-bun")),
    ];
    let package = candidates
        .into_iter()
        .flatten()
        .find(|package| package.join("package.json").exists())
        .ok_or_else(|| {
            format!(
                "the Bun runtime is not installed in {}: run `bun add -d @arcships/rutis-bun` there",
                bun.project.display()
            )
        })?;
    let program = bun.program.clone().unwrap_or_else(|| PathBuf::from("bun"));
    let answers = std::process::Command::new(&program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !answers {
        return Err(format!(
            "the Bun runtime needs Bun: {} does not run (install it from https://bun.com, or set `program`)",
            program.display()
        ));
    }
    Ok(LocalRuntime::bun(package, &bun.project).interpreter(program))
}

/// The Python runtime of `py.project`, with an interpreter that has the
/// `rutis` package.
fn python(py: &PythonRuntime) -> Result<LocalRuntime, Error> {
    let interpreter = py
        .python
        .clone()
        .or_else(|| std::env::var_os("VIRTUAL_ENV").map(|venv| venv_python(Path::new(&venv))))
        .or_else(|| Some(venv_python(&py.project.join(".venv"))).filter(|venv| venv.exists()))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PYTHON));
    // A source checkout of the rutis package (tests, development of rutis).
    let source = std::env::var_os("RUTIS_PYTHON_PATH");
    let mut check = std::process::Command::new(&interpreter);
    check.args(["-c", "import rutis"]);
    if let Some(source) = &source {
        check.env("PYTHONPATH", source);
    }
    let found = check.output().map_err(|error| {
        format!(
            "the Python runtime cannot run its interpreter {} ({error}): install Python, \
             or set runtimes.py.python to an interpreter that has the rutis package",
            interpreter.display()
        )
    })?;
    if !found.status.success() {
        return Err(format!(
            "the Python runtime needs the rutis package in {}: run `{} -m pip install rutis` (or `uv add rutis`)",
            interpreter.display(),
            interpreter.display()
        ));
    }
    let mut runtime = LocalRuntime::python(&py.project).interpreter(interpreter);
    if let Some(source) = source {
        runtime = runtime.python_path(source);
    }
    Ok(runtime)
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

impl Plugin for Released {
    fn name(&self) -> &str {
        "runtime-rows-released"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
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
        let rows = host.root.get_as::<RuntimeRows>(RuntimeRows::key("py"));
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
                    project: PathBuf::from("."),
                    runtime: None,
                    program: None,
                }),
                remote: vec![RemoteRuntime {
                    name: remote.into(),
                    language: "python".into(),
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
        let go = host.go.as_ref().unwrap();
        assert_eq!(go.runtimes.runtimes()[0].name, "go-netkit");
    }

    /// A Go binary on another machine (here, listening on loopback): a peer
    /// row links to it, and its rows are `<runtime>:<plugin>`.
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
