//! Rows hosted on other nodes: `peer:<id>/<plugin>` loads `<plugin>` on the
//! node `<id>`, through its `host` (rutis-bridge), as a row of this loader.
//!
//! As for language runtimes, readiness has two stages. The link provides
//! `Peer#<id>`; then [`PeerRowsPlugin`] waits for the peer to offer
//! `plugins`, resolves the rows that were resolved without it again, and
//! only then provides [`PeerRows`], which every such row depends on. A row
//! of a peer that is offline or offers no host waits for it, with no
//! schema; it is not unresolved. The plugin's own dependencies are the
//! target node's business: they are not copied into this loader.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, InstanceId, PluginFactory, TypeKey,
};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Disposer, Error};
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::{settle, Value as RpcValue};
use rutis_bridge::{peer_key, LinkState, Peer};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::resolver::{Build, Resolved};
use crate::{Loader, LoaderError, Resolver};

const PREFIX: &str = "peer:";

/// The service rows of a peer depend on: the peer, once its host is
/// offered and the rows' declarations are complete.
pub struct PeerRows {
    peer: Arc<Peer>,
}

impl PeerRows {
    pub fn key(id: &PeerId) -> TypeKey {
        TypeKey::keyed_dynamic::<PeerRows>(id.to_string())
    }

    pub fn peer(&self) -> &Arc<Peer> {
        &self.peer
    }
}

/// The ready peers, for the resolver, which has no context of its own;
/// [`PeerRowsPlugin`]s keep it.
#[derive(Default)]
struct Directory {
    peers: Mutex<HashMap<PeerId, Arc<Peer>>>,
    /// Names resolved without their peer, to resolve again.
    stale: Mutex<HashSet<String>>,
    /// The running `rutis-bridge/peer` rows' links, by the row's fiber
    /// ([`PeerResolver::link_state`]), each with the token of the apply
    /// that registered it.
    links: Mutex<HashMap<InstanceId, (u64, rutis_bridge::PeerHandle)>>,
}

/// Resolves `peer:<id>/<plugin>` rows. Share one between the loader's
/// chain (`Chain::with_shared`) and the [`PeerRowsPlugin`]s.
#[derive(Default)]
pub struct PeerResolver {
    directory: Arc<Directory>,
}

impl PeerResolver {
    pub fn new() -> Self {
        Self::default()
    }

    fn split(name: &str) -> Option<(PeerId, &str)> {
        let rest = name.strip_prefix(PREFIX)?;
        let (peer, plugin) = rest.split_once('/')?;
        let peer = PeerId::new(peer).ok()?;
        (!plugin.is_empty()).then_some((peer, plugin))
    }

    fn offline(&self, name: &str, peer: &PeerId, plugin: &str, why: &str) -> Arc<Resolved> {
        self.directory.stale.lock().unwrap().insert(name.to_owned());
        Arc::new(Resolved {
            factory: Arc::new(PeerFactory::new(name, peer.clone(), plugin)),
            schema: None,
            meta: json!({ "source": "peer", "peer": peer.to_string(), "plugin": plugin, "schema": format!("unavailable: {why}") }),
            foreign_scope: true,
            scoped: None,
            declares: Some(runs_on(peer, why)),
        })
    }

    /// What the link of the `rutis-bridge/peer` row running as the fiber
    /// `row` is doing: `row` is the row's [`FiberView::instance`]
    /// (`EntryInfo::view`), for each copy inside instances too. `None`
    /// when that fiber is no running peer row of this resolver, or its
    /// link has not started yet. For a host to report the link's state
    /// beside the row's.
    ///
    /// [`FiberView::instance`]: rutis::host::FiberView::instance
    pub fn link_state(&self, row: InstanceId) -> Option<watch::Receiver<LinkState>> {
        let links = self.directory.links.lock().unwrap();
        links.get(&row).and_then(|(_, handle)| handle.link_state())
    }

    fn take_stale(&self, peer: &PeerId) -> HashSet<String> {
        let mut stale = self.directory.stale.lock().unwrap();
        let (mine, others): (HashSet<String>, HashSet<String>) = stale
            .drain()
            .partition(|name| Self::split(name).is_some_and(|(of, _)| &of == peer));
        *stale = others;
        mine
    }
}

impl Resolver for PeerResolver {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            let Some((peer_id, plugin)) = Self::split(name) else {
                return Err(LoaderError::NotFound {
                    name: name.to_owned(),
                });
            };
            let peer = self.directory.peers.lock().unwrap().get(&peer_id).cloned();
            let Some(peer) = peer else {
                return Ok(self.offline(name, &peer_id, plugin, "the peer is not connected"));
            };
            if !peer.offers().borrow().families.contains("plugins") {
                return Ok(self.offline(name, &peer_id, plugin, "the peer offers no host"));
            }
            let reply = peer
                .connection()
                .invoke_async("", "plugins.describe", json!([plugin]).into())
                .await;
            let described = match reply {
                Ok(described) => settle(described).await,
                Err(error) => Err(error),
            };
            let described = match described.and_then(RpcValue::json) {
                Ok(described) => described,
                Err(rutis_bridge::session::Error::Remote { name: kind, .. })
                    if kind == "NotFound" =>
                {
                    return Err(LoaderError::NotFound {
                        name: name.to_owned(),
                    })
                }
                // The session ended meanwhile: wait for the peer again.
                Err(rutis_bridge::session::Error::Transport(_)) => {
                    return Ok(self.offline(name, &peer_id, plugin, "the peer went away"))
                }
                Err(error) => {
                    return Err(LoaderError::Resolve {
                        name: name.to_owned(),
                        message: error.to_string(),
                    })
                }
            };
            Ok(Arc::new(Resolved {
                factory: Arc::new(PeerFactory::new(name, peer_id.clone(), plugin)),
                schema: described.get("schema").cloned().filter(|s| !s.is_null()),
                meta: json!({
                    "source": "peer",
                    "peer": peer_id.to_string(),
                    "plugin": plugin,
                    "version": described.get("version"),
                    "integrity": described.get("integrity"),
                }),
                foreign_scope: true,
                scoped: None,
                // The peer says what the plugin needs and provides there.
                declares: Some(runs_on(&peer_id, "what it declares is known there")),
            }))
        })
    }
}

/// Mount it after the link (or what provides `Peer#<id>`) and the loader,
/// with the resolver the loader uses.
pub struct PeerRowsPlugin {
    label: String,
    peer: PeerId,
    resolver: Arc<PeerResolver>,
    injects: [TypeKey; 2],
}

impl PeerRowsPlugin {
    pub fn new(peer: PeerId, resolver: Arc<PeerResolver>) -> Self {
        Self {
            label: format!("peer-rows#{peer}"),
            injects: [peer_key(&peer).erase(), TypeKey::of::<Loader>()],
            peer,
            resolver,
        }
    }
}

impl DynPlugin for PeerRowsPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let peer = ctx.require_key::<Peer>(self.injects[0].clone())?;
            let loader = ctx.require::<Loader>()?;
            let directory = self.resolver.directory.clone();
            directory
                .peers
                .lock()
                .unwrap()
                .insert(self.peer.clone(), peer.clone());
            // In a task: refreshing takes the loader's lock, which the
            // reconcile that started this plugin may hold.
            let task = tokio::spawn(follow(
                ctx.clone(),
                peer,
                loader,
                self.resolver.clone(),
                self.peer.clone(),
            ));
            let id = self.peer.clone();
            ctx.effect(move || async move {
                task.abort();
                directory.peers.lock().unwrap().remove(&id);
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// Provide `PeerRows` while the peer offers a host: after resolving the
/// rows resolved without it, so no row starts on a placeholder.
async fn follow(
    ctx: Ctx,
    peer: Arc<Peer>,
    loader: Arc<Loader>,
    resolver: Arc<PeerResolver>,
    id: PeerId,
) {
    let mut offers = peer.offers();
    // What is provided, and for which offer of the host.
    let mut provided: Option<(Disposer, u64)> = None;
    loop {
        let hosting = offers.borrow_and_update().epoch("plugins");
        let current = provided.as_ref().map(|(_, epoch)| *epoch);
        if hosting != current {
            // The host went, or was opened anew (perhaps unseen in between,
            // with what it loaded gone): its rows stop; the peer and its
            // other features stay.
            if let Some((disposer, _)) = provided.take() {
                let _ = disposer.dispose().await;
            }
            if let Some(epoch) = hosting {
                let stale = resolver.take_stale(&id);
                for entry in loader.entries() {
                    let named = entry.options["name"].as_str().map(str::to_owned);
                    if named.is_some_and(|name| stale.contains(&name)) {
                        // A row that fails to resolve shows it in its status.
                        let _ = loader.refresh(&entry.id).await;
                    }
                }
                match ctx.provide_key(
                    PeerRows::key(&id),
                    Arc::new(PeerRows { peer: peer.clone() }),
                ) {
                    Ok(disposer) => provided = Some((disposer, epoch)),
                    Err(_) => return,
                }
            }
        }
        if offers.changed().await.is_err() {
            return;
        }
    }
}

struct PeerFactory {
    name: String,
    plugin: String,
    injects: [TypeKey; 1],
}

impl PeerFactory {
    fn new(name: &str, peer: PeerId, plugin: &str) -> Self {
        Self {
            name: name.to_owned(),
            injects: [PeerRows::key(&peer)],
            plugin: plugin.to_owned(),
        }
    }
}

impl PluginFactory<Value> for PeerFactory {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(PeerRow {
            name: self.name.clone(),
            plugin: self.plugin.clone(),
            config: config.clone(),
            injects: self.injects.clone(),
        }))
    }
}

/// One generation of a hosted row: loaded on apply, unloaded on cleanup.
struct PeerRow {
    name: String,
    plugin: String,
    config: Value,
    injects: [TypeKey; 1],
}

/// The hosted plugin failed with `error`, kept as it is (#677).
fn failed(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
    rutis::Error::from(ErrorKind::PluginFailed(error.into()))
}

/// The row failed for a reason rutis states.
fn refused(reason: impl Into<String>) -> Error {
    rutis::Error::from(ErrorKind::PluginFailed(reason.into().into()))
}

impl DynPlugin for PeerRow {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let rows = ctx.require_key::<PeerRows>(self.injects[0].clone())?;
            let session = rows.peer().connection().clone();
            // Unique, and new for every generation.
            let key = ctx.instance().to_string();
            let row = ctx
                .get::<Loader>()
                .and_then(|loader| loader.row(ctx.instance()));
            let (isolate, inject) = row.map(|row| (row.isolate, row.inject)).unwrap_or_default();
            let isolate: Vec<(String, String)> = isolate;
            let loaded = session
                .invoke_async(
                    "",
                    "plugins.load",
                    json!([key, self.plugin, self.config, isolate, inject]).into(),
                )
                .await
                .map_err(failed)?;
            settle(loaded).await.map_err(failed)?;
            Ok(Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    let unloaded = session
                        .invoke_async("", "plugins.unload", json!([key]).into())
                        .await;
                    let unloaded = match unloaded {
                        Ok(unloaded) => settle(unloaded).await,
                        Err(error) => Err(error),
                    };
                    match unloaded {
                        // A link that ended took the hosted plugin with it.
                        Ok(_) | Err(rutis_bridge::session::Error::Transport(_)) => Ok(()),
                        Err(error) => Err(failed(error)),
                    }
                })
            })))
        })
    }
}

// ── composition ─────────────────────────────────────────────────────

/// The plugins this loader can resolve, as what a host serves its peer.
/// Rows of other peers (`peer:…`) are not hosted: that would make this
/// node relay them.
pub struct LoaderCatalog {
    loader: Loader,
    /// The instances the hosted plugins run in, for factories built per
    /// instance.
    build: Build,
}

impl LoaderCatalog {
    pub fn new(loader: Loader) -> Self {
        Self::in_instances(loader, Build::default())
    }

    /// The catalog of a host running in `build`'s instances: plugins whose
    /// factory depends on the instance get that instance's.
    pub fn in_instances(loader: Loader, build: Build) -> Self {
        Self { loader, build }
    }
}

impl rutis_bridge::PluginCatalog for LoaderCatalog {
    fn find<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<rutis_bridge::Installed>> {
        Box::pin(async move {
            if name.starts_with(PREFIX) {
                return None;
            }
            let resolved = self.loader.resolve(name).await.ok()?;
            let factory = match &resolved.scoped {
                Some(scoped) => scoped(&self.build).ok()?,
                None => resolved.factory.clone(),
            };
            Some(rutis_bridge::Installed {
                described: rutis_bridge::Described {
                    schema: resolved.schema.clone(),
                    version: resolved.meta["version"].as_str().map(str::to_owned),
                    integrity: resolved.meta["integrity"].as_str().map(str::to_owned),
                },
                factory,
            })
        })
    }
}

/// The config of a `rutis-bridge/peer` row.
#[derive(serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct NodeConfig {
    peer: String,
    transport: String,
    identity: String,
    #[serde(default)]
    dial: Option<String>,
    #[serde(default)]
    listen: Option<String>,
    #[serde(default)]
    require: Vec<String>,
    #[serde(default)]
    declare: Vec<String>,
    #[serde(default)]
    export: Vec<String>,
    #[serde(default)]
    import: Vec<String>,
    /// Host the plugins this loader resolves for the peer.
    #[serde(default)]
    host: bool,
    /// The peer is this runtime instance.
    #[serde(default)]
    runtime: Option<String>,
    /// Rows `peer:<peer>/…` of this loader run on the peer.
    #[serde(default)]
    rows: bool,
}

/// Why a peer config with `events` is refused: events are shared through
/// `export` and `import` since rutis 0.13 (events protocol §2.4, §9).
#[doc(hidden)]
pub const REMOVED_EVENTS: &str = "the peer config field events was removed in rutis 0.13: list \
     events: { out } in export and events: { in } in import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)";

impl NodeConfig {
    fn parse(config: &Value) -> Result<Self, Error> {
        if config.get("events").is_some() {
            return Err(refused(REMOVED_EVENTS));
        }
        let config: Self = serde_json::from_value(config.clone())
            .map_err(|error| refused(format!("invalid peer config: {error}")))?;
        let peer = PeerId::new(&config.peer).map_err(failed)?;
        match (&config.dial, &config.listen) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => {
                return Err(refused(format!(
                    "the link to {peer} needs one of dial and listen"
                )))
            }
        }
        // `*` only ends an item: a prefix of event names (events protocol
        // §9).
        if let Some(item) = config
            .export
            .iter()
            .chain(&config.import)
            .find(|item| item.is_empty() || item[..item.len() - 1].contains('*'))
        {
            return Err(refused(format!(
                "invalid export or import item {item:?}: a name, or a prefix of event names \
                 ending in *"
            )));
        }
        Ok(config)
    }

    fn link(&self) -> rutis_bridge::LinkConfig {
        let peer = PeerId::new(&self.peer).expect("validated");
        let mut link = match (&self.dial, &self.listen) {
            (Some(address), _) => {
                rutis_bridge::LinkConfig::dial(peer, &self.transport, &self.identity, address)
            }
            (_, Some(listener)) => {
                rutis_bridge::LinkConfig::listen(peer, &self.transport, &self.identity, listener)
            }
            _ => unreachable!("validated"),
        };
        link.require = self.require.clone();
        link.declare = self.declare.clone();
        link
    }

    /// The features, for a link running in `build`'s instances: names
    /// map to that instance's services.
    fn features(&self, loader: Option<&Loader>, build: &Build) -> rutis_bridge::Features {
        rutis_bridge::Features {
            export: self.export.clone(),
            import: self.import.clone(),
            host: (self.host)
                .then(|| loader.cloned())
                .flatten()
                .map(|loader| {
                    Arc::new(LoaderCatalog::in_instances(loader, build.clone()))
                        as Arc<dyn rutis_bridge::PluginCatalog>
                }),
            // The peer's rows name services as this loader's catalog does.
            host_services: (self.host)
                .then(|| loader.cloned())
                .flatten()
                .map(|loader| {
                    let build = build.clone();
                    Arc::new(move |name: &str| loader.service_key_in(name, &build).ok())
                        as rutis_bridge::ServiceKeys
                }),
            services: loader.cloned().map(|loader| {
                let build = build.clone();
                Arc::new(move |name: &str| loader.shared_key_in(name, &build).ok())
                    as rutis_bridge::ServiceKeys
            }),
            runtime: self.runtime.clone(),
        }
    }

    /// Refuse what a link in `build`'s instances cannot do: a service
    /// exported or imported by a name that has no key there, and, inside
    /// instances, features that publish keys the rest of the tree uses
    /// (`rows`, `runtime`): every instance's link would publish the same.
    fn check(&self, loader: Option<&Loader>, build: &Build) -> Result<(), Error> {
        if build.instances().next().is_some() {
            if self.rows {
                return Err(refused(
                    "a link inside instances cannot carry rows: put the `rows` link outside instanced groups",
                ));
            }
            if self.runtime.is_some() {
                return Err(refused(
                    "a link inside instances cannot be a runtime: put the `runtime` link outside instanced groups",
                ));
            }
        }
        if let Some(loader) = loader {
            // Prefixes select events only.
            for name in self.export.iter().chain(&self.import) {
                if name.ends_with('*') {
                    continue;
                }
                loader.shared_key_in(name, build).map_err(failed)?;
            }
        }
        Ok(())
    }
}

/// The schema of a `rutis-bridge/peer` row. `export` and `import` select
/// services by name and events by name or `*` prefix. What is exported and
/// imported is volatile: changing it restarts only that feature and keeps
/// the session.
#[doc(hidden)]
pub fn node_schema() -> Value {
    let names = json!({ "type": "array", "items": { "type": "string" }, "x-volatile": true });
    json!({
        "type": "object",
        "required": ["peer", "transport", "identity"],
        "properties": {
            "peer": { "type": "string", "description": "the far end's endpoint id" },
            "transport": { "type": "string", "description": "Transport#<kind>" },
            "identity": { "type": "string", "description": "Identity#<name>" },
            "dial": { "type": "string" },
            "listen": { "type": "string" },
            "require": { "type": "array", "items": { "type": "string" } },
            "declare": { "type": "array", "items": { "type": "string" } },
            "export": names,
            "import": names,
            "host": { "type": "boolean" },
            "runtime": { "type": "string" },
            "rows": { "type": "boolean" }
        }
    })
}

/// The `rutis-bridge/peer` row: a link and its features, composed; with
/// `rows`, this loader's `peer:<peer>/…` rows run on the peer. Register it
/// with [`register_peer_node`].
///
/// Inside instances, each instance has its own link: its `Peer#<peer>` is
/// isolated under the instance's label, and what it exports, imports and
/// hosts is that instance's.
struct NodeFactory {
    resolver: Arc<PeerResolver>,
    build: Build,
}

impl PluginFactory<Value> for NodeFactory {
    fn name(&self) -> &str {
        "rutis-bridge/peer"
    }

    fn validate_config(&self, config: &Value) -> Result<(), Error> {
        NodeConfig::parse(config).map(|_| ())
    }

    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Node {
            config: NodeConfig::parse(config)?,
            resolver: self.resolver.clone(),
            build: self.build.clone(),
        }))
    }
}

/// Declarations of a row that runs on `peer`, not known here, and why.
fn runs_on(peer: &PeerId, why: &str) -> crate::resolver::Declarations {
    let why = format!("it runs on {peer}: {why}");
    Arc::new(move |_: &Value| Err(why.clone()))
}

/// Register the `rutis-bridge/peer` row type: share `resolver` with the
/// loader's chain for its `peer:` rows.
pub fn register_peer_node(builtins: &mut crate::Builtins, resolver: Arc<PeerResolver>) {
    let factory = Arc::new(NodeFactory {
        resolver: resolver.clone(),
        build: Build::default(),
    });
    let scoped: crate::resolver::ScopedFactory = Arc::new(move |build: &Build| {
        let factory: Arc<dyn PluginFactory<Value>> = Arc::new(NodeFactory {
            resolver: resolver.clone(),
            build: build.clone(),
        });
        Ok(factory)
    });
    builtins.register_raw_with("rutis-bridge/peer", factory, scoped, Some(node_schema()));
    // A link provides what it imports; what the peer itself offers is
    // known only once it is connected, and is not a service of this node
    // until imported.
    builtins.declare(
        "rutis-bridge/peer",
        Arc::new(|config: &Value| {
            let provides = config["import"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect();
            Ok(crate::resolver::Declared {
                requires: Vec::new(),
                provides,
                announces: false,
            })
        }),
    );
}

/// Tokens of [`Directory::links`] entries.
static NEXT_LINK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Node {
    config: NodeConfig,
    resolver: Arc<PeerResolver>,
    /// The instances the row's copy runs in.
    build: Build,
}

struct Reconfigure {
    handle: rutis_bridge::PeerHandle,
    loader: Option<Loader>,
    build: Build,
}

impl Reconfigure {
    async fn apply(&self, update: &crate::volatile::VolatileUpdate) -> Result<Option<()>, Error> {
        let config = NodeConfig::parse(&update.config)?;
        config.check(self.loader.as_ref(), &self.build)?;
        self.handle
            .set(config.features(self.loader.as_ref(), &self.build))
            .await
            .map_err(failed)?;
        Ok(None)
    }
}

impl DynPlugin for Node {
    fn name(&self) -> &str {
        "rutis-bridge/peer"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let loader = ctx.get::<Loader>().map(|loader| (*loader).clone());
            self.config.check(loader.as_ref(), &self.build)?;
            let composed = rutis_bridge::PeerPlugin::new(
                self.config.link(),
                self.config.features(loader.as_ref(), &self.build),
            )
            .map_err(failed)?;
            let handle = composed.handle();
            // The host reads the link's state by the row's fiber.
            let row = ctx.instance();
            let token = NEXT_LINK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let links = self.resolver.directory.clone();
            links
                .links
                .lock()
                .unwrap()
                .insert(row, (token, handle.clone()));
            ctx.effect(move || async move {
                let mut links = links.links.lock().unwrap();
                if links.get(&row).is_some_and(|(mine, _)| *mine == token) {
                    links.remove(&row);
                }
                Ok(())
            })?;
            // Inside instances, the link is the instance's own: its peer
            // is kept apart from other instances' links to the same peer.
            let scope = match self.build.instances().next() {
                Some((_, instance)) => {
                    let peer = PeerId::new(&self.config.peer).map_err(failed)?;
                    ctx.isolate(
                        peer_key(&peer),
                        &format!("rutis-loader/instance/{instance}"),
                    )
                }
                None => ctx.clone(),
            };
            scope.plugin(composed);
            if self.config.rows {
                let peer = PeerId::new(&self.config.peer).map_err(failed)?;
                ctx.plugin(PeerRowsPlugin::new(peer, self.resolver.clone()));
            }
            let reconfigure = Arc::new(Reconfigure {
                handle,
                loader,
                build: self.build.clone(),
            });
            ctx.listen::<crate::volatile::VolatileUpdate>()
                .name(crate::volatile::volatile_key(ctx))
                .on(move |update, _| {
                    let reconfigure = reconfigure.clone();
                    async move { reconfigure.apply(&update).await }
                })?;
            Ok(Effect::Done)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rutis::host::FiberState;
    use rutis_bridge::memory::{MemoryPlugin, MemoryTransport};
    use rutis_bridge::{
        Credential, Described, HostPlugin, IdentityPlugin, LinkConfig, LinkPlugin, Retry,
        StaticCatalog, StaticIdentity,
    };
    use rutis_test_support::eventually;

    use super::*;

    fn id(name: &str) -> PeerId {
        PeerId::new(name).unwrap()
    }

    /// Installed on mac: provides `GreetingService`.
    struct Greeter;
    impl PluginFactory<Value> for Greeter {
        fn build(&self, _: &Value) -> Result<Box<dyn DynPlugin>, Error> {
            Ok(Box::new(Greeting))
        }
    }
    struct Greeting;
    impl DynPlugin for Greeting {
        fn name(&self) -> &str {
            "greeter"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                ctx.provide(GreetingService)?;
                Ok(Effect::Done)
            })
        }
    }
    struct GreetingService;

    /// Provides `PeerRows` the way [`follow`] does: from its context, with
    /// the disposer kept to withdraw it.
    struct ProvidesRows {
        peer: Arc<Peer>,
        ctx: Arc<Mutex<Option<Ctx>>>,
        provided: Arc<Mutex<Option<Disposer>>>,
    }
    impl ProvidesRows {
        fn provide(ctx: &Ctx, peer: &Arc<Peer>) -> Disposer {
            ctx.provide_key::<PeerRows>(
                PeerRows::key(&id("mac")),
                Arc::new(PeerRows { peer: peer.clone() }),
            )
            .unwrap()
        }
    }
    impl DynPlugin for ProvidesRows {
        fn name(&self) -> &str {
            "provides-rows"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                *self.provided.lock().unwrap() = Some(Self::provide(ctx, &self.peer));
                *self.ctx.lock().unwrap() = Some(ctx.clone());
                Ok(Effect::Done)
            })
        }
    }

    type Withdrawal = Mutex<Option<std::thread::JoinHandle<Result<(), Arc<ErrorKind>>>>>;

    /// A hosted row whose first generation sees `PeerRows` withdrawn after
    /// its gate opened, before its apply runs.
    struct WithdrawnBeforeApply {
        row: PeerRow,
        main: Ctx,
        provided: Arc<Mutex<Option<Disposer>>>,
        withdrawal: Arc<Withdrawal>,
    }
    impl DynPlugin for WithdrawnBeforeApply {
        fn name(&self) -> &str {
            self.row.name()
        }
        fn injects(&self) -> Vec<TypeKey> {
            self.row.injects()
        }
        fn validate(&self) -> Result<(), Error> {
            if let Some(disposer) = self.provided.lock().unwrap().take() {
                // From another thread: this worker is blocked here.
                let runtime = tokio::runtime::Handle::current();
                let withdrawal = std::thread::spawn(move || runtime.block_on(disposer.dispose()));
                *self.withdrawal.lock().unwrap() = Some(withdrawal);
                // The removal is marked as soon as the withdrawal starts.
                let start = std::time::Instant::now();
                while self
                    .main
                    .get_key::<PeerRows>(PeerRows::key(&id("mac")))
                    .is_some()
                {
                    assert!(start.elapsed() < Duration::from_secs(10), "never withdrawn");
                    std::thread::yield_now();
                }
            }
            Ok(())
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            self.row.apply(ctx)
        }
    }

    /// `PeerRows` withdrawn after a row's apply was scheduled and before it
    /// ran: the row waits for it again instead of failing, and loads when it
    /// is provided again (#438).
    ///
    /// guarantee: core/typed.untyped-require
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_row_whose_peer_rows_go_before_its_apply_waits_and_loads_when_back() {
        let transport = Arc::new(MemoryTransport::default());
        transport.endpoint("main-in", id("main"));
        let quick = Retry {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(200),
            jitter: 0.0,
            ..Retry::default()
        };
        let main = Ctx::root().unwrap();
        (&main.plugin(MemoryPlugin::with_transport(transport.clone())))
            .await
            .unwrap();
        main.plugin(IdentityPlugin::new(
            "main",
            StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
        ));
        main.plugin(LinkPlugin::new(
            LinkConfig::listen(id("mac"), "memory", "main", "main-in").retry(quick.clone()),
        ));
        let mac = Ctx::root().unwrap();
        (&mac.plugin(MemoryPlugin::with_transport(transport)))
            .await
            .unwrap();
        mac.plugin(IdentityPlugin::new(
            "mac",
            StaticIdentity::new(id("mac"))
                .present(id("main"), Credential::Bearer("mac-token".into())),
        ));
        mac.plugin(LinkPlugin::new(
            LinkConfig::dial(id("main"), "memory", "mac", "main-in").retry(quick),
        ));
        let catalog = StaticCatalog::new().with("greeter", Described::default(), Greeter);
        mac.plugin(HostPlugin::new(id("main"), Arc::new(catalog)));

        let peer = eventually(
            || {
                let peer = main.get_at::<Peer>(&peer_key(&id("mac")))?;
                let hosting = peer.offers().borrow().families.contains("plugins");
                hosting.then_some(peer)
            },
            "the host offered",
        )
        .await;
        let rows_ctx = Arc::new(Mutex::new(None));
        let provided = Arc::new(Mutex::new(None));
        (&main.plugin(ProvidesRows {
            peer: peer.clone(),
            ctx: rows_ctx.clone(),
            provided: provided.clone(),
        }))
            .await
            .unwrap();

        let withdrawal = Arc::new(Withdrawal::default());
        let factory = PeerFactory::new("peer:mac/greeter", id("mac"), "greeter");
        let row = main.plugin(WithdrawnBeforeApply {
            row: PeerRow {
                name: factory.name.clone(),
                plugin: factory.plugin.clone(),
                config: json!({}),
                injects: factory.injects.clone(),
            },
            main: main.clone(),
            provided,
            withdrawal: withdrawal.clone(),
        });
        let withdrawn = eventually(|| withdrawal.lock().unwrap().take(), "the withdrawal").await;
        tokio::task::spawn_blocking(move || withdrawn.join().unwrap())
            .await
            .unwrap()
            .unwrap();
        let waiting = (&row).await;
        assert!(waiting.is_ok(), "{waiting:?}");
        let state = row.state();
        assert_eq!(state.state, FiberState::Pending, "{:?}", state.error);
        assert!(mac.get::<GreetingService>().is_none());

        // Provided again, as `follow` does: the row loads.
        let rows_ctx = rows_ctx.lock().unwrap().clone().unwrap();
        let _provided = ProvidesRows::provide(&rows_ctx, &peer);
        eventually(|| mac.get::<GreetingService>(), "the hosted row").await;
        eventually(
            || (row.state().state == FiberState::Active).then_some(()),
            "the row active",
        )
        .await;
    }

    /// guarantee: events/node-config
    ///
    /// `events: { out, in }` is gone (events protocol §2.4): writing it is
    /// refused, naming the replacement; `export` / `import` take `*`
    /// prefixes, which select events only, and nothing else with a `*`.
    #[test]
    fn link_config_shares_events_through_export_and_import() {
        let base = json!({ "peer": "mac", "transport": "memory", "identity": "main", "dial": "x" });
        let with = |field: &str, value: Value| {
            let mut config = base.clone();
            config[field] = value;
            NodeConfig::parse(&config)
        };
        let refused = with("events", json!({ "out": ["tick"] })).err().unwrap();
        assert!(refused.to_string().contains(REMOVED_EVENTS), "{refused}");
        let config = with("export", json!(["clock", "session/*", "*"])).unwrap();
        let features = config.features(None, &Build::default());
        assert_eq!(features.export, ["clock", "session/*", "*"]);
        let refused = with("import", json!(["a*b"])).err().unwrap();
        assert!(
            refused.to_string().contains("a prefix of event names"),
            "{refused}"
        );
    }
}
