//! Runtime connections become instances (dial-out design §四): every
//! connection an access point admits is an instance of the `phone` group,
//! whose `phone:<plugin>` rows run on that connection. Runtimes here are
//! Rust stand-ins speaking the row contract over the memory transport.
#![cfg(feature = "runtimes")]

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use common::runtimes::{id, NoConfig};
use common::Scoped;
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, PluginFactory, TypeKey,
};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};
use rutis_bridge::session::{
    settle, Connection, Dispatch, Endpoint, Error, Format, HostDispatch, Reply, Value as RpcValue,
};
use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
use rutis_bridge::{
    protocol, AccessEvent, AccessPointConfig, AccessPointPlugin, AdmissionPlugin, Credential, Dial,
    Identity, SharedToken, StaticIdentity, Transport,
};
use rutis_loader::edit::{Editable, NewEntry, Version};
use rutis_loader::report::{EntryInfo, EntryStatus};
use rutis_loader::runtime::{RuntimeConnection, RuntimeInstances};
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis_test_support::{eventually, scaled, within, HANG_GUARD};
use serde_json::{json, Value};
use tokio::sync::broadcast;

type Log = Arc<Mutex<Vec<String>>>;

// ── A runtime ───────────────────────────────────────────────────

/// A runtime with `plugins`, each providing a service of its name whose
/// `whoami()` answers the runtime's id. It lists them in its `mount` reply
/// when `lists`; otherwise rutis asks for each (`rows.schema`). When
/// `undeclared`, its plugins declare nothing and it announces each one's
/// service as the row loads (`rows.announce`).
struct Runtime {
    name: String,
    plugins: Vec<&'static str>,
    lists: bool,
    undeclared: bool,
    /// Its connection drops while it loads a row: `rows.load` never answers.
    drops: bool,
    tokio: tokio::runtime::Handle,
    version: AtomicU64,
    /// The services each loaded row exports, by row key.
    exports: Mutex<HashMap<String, Vec<String>>>,
}

impl Runtime {
    fn schema(&self, plugin: &str) -> Value {
        match self.undeclared {
            true => json!({ "inject": [], "provides": {} }),
            // A config schema: the loader warns of a copy's extra keys.
            false => json!({
                "inject": [],
                "provides": { plugin: { "whoami": "async" } },
                "config": { "type": "object", "properties": { "label": { "type": "string" } } }
            }),
        }
    }

    /// Tell rutis what the slot `name` now holds; `row` for a service the
    /// row did not declare, with its methods.
    fn announce(&self, peer: &Connection, name: &str, handle: Option<String>, row: Option<&str>) {
        let version = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        let peer = peer.clone();
        let mut args = json!([name, handle, version]);
        if let (Some(row), Some(_)) = (row, &handle) {
            let announced = json!({ "row": row, "methods": { "whoami": "async" } });
            args.as_array_mut().unwrap().push(announced);
        }
        self.tokio.spawn(async move {
            let _ = peer.invoke_async("", "service", args.into()).await;
        });
    }
}

impl Dispatch for Runtime {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: RpcValue) -> Reply {
        let args = args.json().unwrap_or(Value::Null);
        match (target, method) {
            ("", "mount") => {
                let mut reply = json!({
                    "services": {},
                    "features": ["rows.v2", "hosts", "leaf", "rows.announce"]
                });
                if self.lists {
                    let plugins: serde_json::Map<String, Value> = self
                        .plugins
                        .iter()
                        .map(|plugin| (plugin.to_string(), self.schema(plugin)))
                        .collect();
                    reply["plugins"] = Value::Object(plugins);
                }
                Ok(reply.into())
            }
            ("", "rows.schema") => {
                let plugin = args[0].as_str().unwrap_or_default();
                match self.plugins.contains(&plugin) {
                    true => Ok(self.schema(plugin).into()),
                    false => Err(Error::Remote {
                        name: "NotFound".into(),
                        message: format!("no plugin {plugin}"),
                        graph: None,
                    }),
                }
            }
            ("", "rows.load") if self.drops => {
                peer.close(Error::Transport("the runtime's process ended".into()));
                Err(Error::Transport("the runtime's process ended".into()))
            }
            ("", "rows.load") => {
                let key = args[0].as_str().unwrap().to_owned();
                let names: Vec<String> = match self.undeclared {
                    // The plugin provides the service of its name.
                    true => vec![args[1].as_str().unwrap().to_owned()],
                    false => args[5]
                        .as_object()
                        .map(|exports| exports.keys().cloned().collect())
                        .unwrap_or_default(),
                };
                let row = self.undeclared.then_some(key.as_str());
                for name in &names {
                    self.announce(peer, name, Some(format!("{name}@{key}")), row);
                }
                self.exports.lock().unwrap().insert(key, names);
                Ok(RpcValue::Undefined)
            }
            ("", "rows.unload") => {
                let key = args[0].as_str().unwrap();
                for name in self.exports.lock().unwrap().remove(key).unwrap_or_default() {
                    self.announce(peer, &name, None, None);
                }
                Ok(RpcValue::Undefined)
            }
            // `release`, `hosts.provide`, `hosts.withdraw`.
            ("", _) => Ok(RpcValue::Undefined),
            (_, "whoami") => Ok(json!(self.name).into()),
            (target, method) => Err(Error::Value(format!("no {target}.{method}"))),
        }
    }
}

/// `name` dialing in with `plugins`, its session ready.
async fn dial(main: &Main, name: &str, plugins: &[&'static str], lists: bool) -> Connection {
    dial_with(main, name, plugins, lists, false).await
}

/// [`dial`], its plugins providing their services without declaring them
/// when `undeclared`.
async fn dial_with(
    main: &Main,
    name: &str,
    plugins: &[&'static str],
    lists: bool,
    undeclared: bool,
) -> Connection {
    dial_runtime(main, name, plugins, lists, undeclared, false).await
}

/// [`dial_with`], its connection dropping while it loads a row when
/// `drops`.
async fn dial_runtime(
    main: &Main,
    name: &str,
    plugins: &[&'static str],
    lists: bool,
    undeclared: bool,
    drops: bool,
) -> Connection {
    let identity: Arc<dyn Identity> = Arc::new(
        StaticIdentity::new(id(name)).present(id("main"), Credential::Bearer("shared".into())),
    );
    let channel = main
        .transport
        .dial(
            &Dial::address("main-in")
                .peer(id("main"))
                .identity(identity)
                .protocol(protocol()),
        )
        .await
        .unwrap();
    let mut endpoint = Endpoint::rust(id(name)).expect(id("main"));
    endpoint.capabilities.push("runtime".into());
    let runtime = Runtime {
        name: name.into(),
        plugins: plugins.to_vec(),
        lists,
        undeclared,
        drops,
        tokio: tokio::runtime::Handle::current(),
        version: AtomicU64::new(0),
        exports: Mutex::default(),
    };
    let session =
        Connection::open_with(channel, Arc::new(runtime), Format::Endpoint(endpoint)).unwrap();
    within("the runtime's session", session.ready())
        .await
        .unwrap();
    session
}

// ── A row using the connection's service ────────────────────────

/// Calls `contacts.whoami()` in its instance and records what it heard.
struct Reader {
    key: TypeKey,
    connection: String,
    log: Log,
}

impl DynPlugin for Reader {
    fn name(&self) -> &str {
        "reader"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![self.key.clone()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let failed = |error: Error| {
                rutis::Error::from(ErrorKind::PluginFailed(error.to_string().into()))
            };
            let contacts = ctx.require_key::<dyn HostDispatch>(self.key.clone())?;
            let call = contacts
                .invoke("whoami", RpcValue::List(Vec::new()))
                .map_err(failed)?;
            let heard = settle(call)
                .await
                .and_then(RpcValue::json)
                .map_err(failed)?;
            let line = format!(
                "reader in {} hears {}",
                self.connection,
                heard.as_str().unwrap()
            );
            self.log.lock().unwrap().push(line);
            let (log, connection) = (self.log.clone(), self.connection.clone());
            Ok(Effect::Disposer(Box::new(move || {
                log.lock()
                    .unwrap()
                    .push(format!("reader in {connection} stops"));
                Ok(())
            })))
        })
    }
}

struct ReaderFactory {
    key: [TypeKey; 1],
    connection: String,
    log: Log,
}

impl PluginFactory<NoConfig> for ReaderFactory {
    fn name(&self) -> &str {
        "reader"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.key.to_vec()
    }

    fn build(&self, _: &NoConfig) -> Result<Box<dyn DynPlugin>, RutisError> {
        Ok(Box::new(Reader {
            key: self.key[0].clone(),
            connection: self.connection.clone(),
            log: self.log.clone(),
        }))
    }
}

/// The global `weather`: `whoami()` answers `global`.
struct GlobalWeather;

impl HostDispatch for GlobalWeather {
    fn invoke(&self, _method: &str, _args: RpcValue) -> Reply {
        Ok(RpcValue::Data(json!("global")))
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "whoami": "sync" }))
    }
}

// ── Harness ─────────────────────────────────────────────────────

/// `main`: a loader whose `phone` group has `phone:contacts` and a Rust
/// `reader` using `contacts`, and an access point admitting runtimes that
/// present `shared`, each an instance of `phone`.
struct Main {
    transport: Arc<MemoryTransport>,
    _root: Ctx,
    loader: Loader,
    log: Log,
    /// The config warnings the loader gave.
    warnings: Log,
    events: broadcast::Receiver<AccessEvent>,
}

async fn main() -> Main {
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));
    let root = Ctx::root().unwrap();
    (&root.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    (&root.plugin(AdmissionPlugin::new(
        "phone",
        SharedToken::new(id("main"), "shared"),
    )))
        .await
        .unwrap();

    // A global `weather`: a connection whose plugins include one no row
    // runs must not take the name over in its instance.
    root.provide_at(
        rutis_bridge::session::host_key("weather"),
        Arc::new(GlobalWeather),
    )
    .unwrap();

    let mut catalog = ServiceCatalog::new();
    catalog.share_by_name();
    let log = Log::default();
    let mut builtins = Scoped::new();
    {
        let (catalog, log) = (catalog.clone(), log.clone());
        builtins.register::<NoConfig, _, _>("weather-reader", move |build| {
            let key = catalog.key_in("weather", build).map_err(|error| {
                rutis::Error::from(ErrorKind::PluginFailed(error.to_string().into()))
            })?;
            let connection = build
                .value::<RuntimeConnection>()
                .map(|connection| connection.id().to_string())
                .unwrap_or_default();
            Ok(ReaderFactory {
                key: [key],
                connection,
                log: log.clone(),
            })
        });
    }
    {
        let (catalog, log) = (catalog.clone(), log.clone());
        builtins.register::<NoConfig, _, _>("reader", move |build| {
            let key = catalog.key_in("contacts", build).map_err(|error| {
                rutis::Error::from(ErrorKind::PluginFailed(error.to_string().into()))
            })?;
            let connection = build
                .value::<RuntimeConnection>()
                .map(|connection| connection.id().to_string())
                .unwrap_or_default();
            Ok(ReaderFactory {
                key: [key],
                connection,
                log: log.clone(),
            })
        });
    }
    let instances = RuntimeInstances::new("phone", "phone", &catalog);
    let plugin = LoaderPlugin::new(
        Chain::new().with(builtins).with(instances.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    let warnings = Log::default();
    {
        let warnings = warnings.clone();
        loader.on_config_warning(Arc::new(move |row: &str, issue: &rutis::ConfigIssue| {
            warnings
                .lock()
                .unwrap()
                .push(format!("{row}: config: {}", issue.message))
        }));
    }
    (&root.plugin(plugin)).await.unwrap();
    let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": [{
        "id": "phone", "group": true, "instanced": true, "config": [
            { "id": "contacts", "name": "phone:contacts", "config": { "labl": "x" } },
            { "id": "reader", "name": "reader" },
            { "id": "weather-reader", "name": "weather-reader" }
        ]
    }] }]))
    .unwrap();
    loader
        .reconcile(
            vec![Layer::new("rows", rows)],
            Some(Editable::new("rows", Version::default())),
        )
        .await
        .unwrap();

    let access = AccessPointPlugin::new(
        AccessPointConfig::new(id("phone"), "memory", "phone", "main-in")
            .require("runtime")
            .handshake(scaled(HANG_GUARD)),
    )
    .accept(instances.accept(loader.clone(), root.clone()));
    let events = access.events();
    (&root.plugin(access)).await.unwrap();
    Main {
        transport,
        _root: root,
        loader,
        log,
        warnings,
        events,
    }
}

impl Main {
    async fn next(&mut self) -> AccessEvent {
        within("an access point event", self.events.recv())
            .await
            .unwrap()
    }

    /// Wait for a connection of `who` to be accepted, or fail with why not.
    async fn connected(&mut self, who: &str) {
        match self.next().await {
            AccessEvent::Connected { id: connected, .. } if connected.as_str() == who => {}
            other => panic!("{who} not connected: {other:?}"),
        }
    }

    fn has(&self, line: &str) -> bool {
        self.log.lock().unwrap().iter().any(|l| l == line)
    }

    async fn heard(&self, line: &str) {
        eventually(|| self.has(line).then_some(()), line).await
    }

    /// The copies of row `id`, one per instance.
    fn copies(&self, id: &str) -> Vec<EntryInfo> {
        self.loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == id && entry.instance.is_some())
            .collect()
    }

    /// The copies of row `id` running, and the errors of those that cannot.
    fn states(&self, id: &str) -> (usize, Vec<String>) {
        let mut running = 0;
        let mut failed = Vec::new();
        for copy in self.copies(id) {
            match copy.status {
                EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active => {
                    running += 1
                }
                EntryStatus::Unresolved(error) => failed.push(error.to_string()),
                other => failed.push(format!("{other:?}")),
            }
        }
        (running, failed)
    }
}

// ── Tests ───────────────────────────────────────────────────────

/// Two connections with the same plugin: two instances, and in each, the
/// row using `contacts` hears its own connection. One lists its plugins in
/// the `mount` reply, the other is asked for each.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn connections_with_the_same_plugin_each_get_their_own_instance() {
    let mut main = main().await;
    let _pixel = dial(&main, "pixel", &["contacts"], true).await;
    main.connected("pixel").await;
    let _tablet = dial(&main, "tablet", &["contacts"], false).await;
    main.connected("tablet").await;
    main.heard("reader in pixel hears pixel").await;
    main.heard("reader in tablet hears tablet").await;
    assert_eq!(main.states("contacts"), (2, Vec::new()));
    assert_eq!(main.states("reader"), (2, Vec::new()));
    // Each instance's own: nothing is provided under the global name.
    assert!(main
        ._root
        .get_at(&rutis_bridge::session::host_key("contacts"))
        .is_none());
}

/// A copy's extra config key, its schema known per connection, is warned
/// about once per row and instance, named after the row (config schema
/// design §3.2); the copies run.
/// guarantee: sdk/config-schema
#[tokio::test(flavor = "multi_thread")]
async fn extra_config_keys_are_warned_about_per_instance() {
    let mut main = main().await;
    let _pixel = dial(&main, "pixel", &["contacts"], true).await;
    main.connected("pixel").await;
    let _tablet = dial(&main, "tablet", &["contacts"], false).await;
    main.connected("tablet").await;
    main.heard("reader in pixel hears pixel").await;
    main.heard("reader in tablet hears tablet").await;
    assert_eq!(main.states("contacts"), (2, Vec::new()));
    assert_eq!(
        *main.warnings.lock().unwrap(),
        [
            "contacts: config: labl is not a config key (did you mean label?)",
            "contacts: config: labl is not a config key (did you mean label?)",
        ]
    );
}

/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_disconnection_closes_only_its_instance() {
    let mut main = main().await;
    let pixel = dial(&main, "pixel", &["contacts"], true).await;
    main.connected("pixel").await;
    let _tablet = dial(&main, "tablet", &["contacts"], true).await;
    main.connected("tablet").await;
    main.heard("reader in pixel hears pixel").await;
    main.heard("reader in tablet hears tablet").await;

    pixel.close(Error::Transport("gone".into()));
    match main.next().await {
        AccessEvent::Disconnected { id: gone, .. } => assert_eq!(gone, id("pixel")),
        other => panic!("{other:?}"),
    }
    assert!(main.has("reader in pixel stops"));
    assert!(!main.has("reader in tablet stops"));
    assert_eq!(main.states("contacts"), (1, Vec::new()));
    assert_eq!(main.states("reader"), (1, Vec::new()));
}

/// A connection that drops while its instance's rows load: nothing waits
/// for it to come back (a connection does not): the instance goes at once
/// with the connection, which the access point reports as ended; the next
/// connection gets an instance as before.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_connection_dropping_while_its_rows_load_ends_its_instance_at_once() {
    let mut main = main().await;
    let _pixel = dial_runtime(&main, "pixel", &["contacts"], true, false, true).await;
    main.connected("pixel").await;
    match main.next().await {
        AccessEvent::Disconnected { id: gone, .. } => assert_eq!(gone, id("pixel")),
        other => panic!("{other:?}"),
    }
    assert!(main.copies("contacts").is_empty());
    assert!(main.copies("reader").is_empty());

    let _tablet = dial(&main, "tablet", &["contacts"], true).await;
    main.connected("tablet").await;
    main.heard("reader in tablet hears tablet").await;
}

/// A connection without the plugin fails that row in its instance only;
/// a row added later that only one connection has is not refused for it.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_connection_without_the_plugin_fails_that_row_in_its_instance_only() {
    let mut main = main().await;
    let _pixel = dial(&main, "pixel", &["contacts", "extra"], false).await;
    main.connected("pixel").await;
    // One that says it has no such plugin, one that is asked and has none.
    let _watch = dial(&main, "watch", &[], true).await;
    main.connected("watch").await;
    let _clock = dial(&main, "clock", &[], false).await;
    main.connected("clock").await;
    main.heard("reader in pixel hears pixel").await;
    let missing = "this connection does not contain plugin contacts";
    let (running, failed) = main.states("contacts");
    assert_eq!(running, 1, "{failed:?}");
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(
        failed.iter().all(|error| error.contains(missing)),
        "{failed:?}"
    );

    main.loader
        .create(
            NewEntry {
                id: Some("extra".into()),
                name: "phone:extra".into(),
                ..NewEntry::default()
            },
            Some("phone"),
            None,
        )
        .await
        .expect("an edit is not refused for a connection without the plugin");
    let (running, failed) = main.states("extra");
    assert_eq!(running, 1, "{failed:?}");
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(
        failed
            .iter()
            .all(|error| error.contains("this connection does not contain plugin extra")),
        "{failed:?}"
    );
}

/// A newer connection with the same id replaces the instance: the old one
/// is removed before the new one is created.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn the_same_id_replaces_the_instance() {
    let mut main = main().await;
    let first = dial(&main, "pixel", &["contacts"], true).await;
    main.connected("pixel").await;
    main.heard("reader in pixel hears pixel").await;
    let before = main.copies("reader")[0].instance.clone().unwrap().plugin;

    let _second = dial(&main, "pixel", &["contacts"], false).await;
    assert!(matches!(main.next().await, AccessEvent::Replaced { .. }));
    main.connected("pixel").await;
    within("the old session's end", first.closed()).await;
    assert!(main.has("reader in pixel stops"));
    // The new instance's reader hears the new connection.
    let heard = || {
        let log = main.log.lock().unwrap();
        log.iter()
            .filter(|line| *line == "reader in pixel hears pixel")
            .count()
    };
    eventually(|| (heard() == 2).then_some(()), "the new reader").await;
    let copies = main.copies("reader");
    assert_eq!(copies.len(), 1);
    assert_ne!(copies[0].instance.clone().unwrap().plugin, before);
    assert_eq!(main.states("reader"), (1, Vec::new()));
}

/// Rows of the runtime outside its group run nowhere, and say where to
/// put them.
#[tokio::test(flavor = "multi_thread")]
async fn rows_of_the_runtime_run_only_in_its_group() {
    let main = main().await;
    let error = main
        .loader
        .create(
            NewEntry {
                id: Some("stray".into()),
                name: "phone:contacts".into(),
                ..NewEntry::default()
            },
            None,
            None,
        )
        .await
        .err()
        .expect("refused");
    assert_eq!(
        error.to_string(),
        "entry \"stray\" rejected: phone:contacts runs only inside the phone group, \
         in the instance of a runtime connection"
    );
}

/// A connection lists a plugin no row runs (`weather`): it claims no name,
/// so in its instance a row using `weather` gets the global one.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_listed_plugin_no_row_runs_claims_no_name() {
    let mut main = main().await;
    let _pixel = dial(&main, "pixel", &["contacts", "weather"], true).await;
    main.connected("pixel").await;
    main.heard("reader in pixel hears pixel").await;
    main.heard("reader in pixel hears global").await;
}

/// Plugins that provide `contacts` without declaring it: the name is still
/// each instance's own, from when the row announces it, and the row using
/// it there, built before that with the global key, is rebuilt with the
/// instance's and hears its own connection, as with a declared one.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn undeclared_services_are_each_instances_own() {
    let mut main = main().await;
    let _pixel = dial_with(&main, "pixel", &["contacts"], true, true).await;
    main.connected("pixel").await;
    let _tablet = dial_with(&main, "tablet", &["contacts"], false, true).await;
    main.connected("tablet").await;
    main.heard("reader in pixel hears pixel").await;
    main.heard("reader in tablet hears tablet").await;
    assert_eq!(main.states("contacts"), (2, Vec::new()));
    assert_eq!(main.states("reader"), (2, Vec::new()));
    assert!(main
        ._root
        .get_at(&rutis_bridge::session::host_key("contacts"))
        .is_none());
    // A name no row of the connection announces stays global.
    main.heard("reader in pixel hears global").await;
}
