//! Services between nodes: `export` announces local services to a peer,
//! `import` registers a peer's services as local native ones.
//!
//! A service crosses as what rutis services of other languages already are:
//! `dyn HostDispatch` at `host_key(name)`. On the wire it is a record of one
//! function reference per method, with the methods' shape (`sync` or
//! `async`); `version` orders announcements, so a replacement or withdrawal
//! never loses to an older one. `plugin` names the plugin providing it:
//! its row's id where it has one, else its name.
//!
//! ```text
//! services.announce { name, service: { method: fn, … }, shape, version, plugin }
//! services.withdraw { name, version }
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::channel::PeerId;
use crate::session::rpc::{Connection, Reference, Reply, Value};
use crate::session::{host_key, Error, HostDispatch};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Disposer, Error as RutisError};
use serde_json::{json, Value as Json};

use crate::{peer_key, Offered, Peer, ServiceKeys};

/// `host_key(name)` for every name: the default mapping.
fn by_name() -> ServiceKeys {
    Arc::new(|name: &str| Some(host_key(name).erase()))
}

fn failed(label: &str, error: impl std::fmt::Display) -> RutisError {
    rutis::Error::from(ErrorKind::PluginFailed(format!("{label}: {error}").into()))
}

// ── export ──────────────────────────────────────────────────────────

/// Announces local services (`host_key(name)`) to `Peer#<peer>`, following
/// their replacement and withdrawal. Each name is a child plugin injecting
/// its service, so rutis gating drives the announcements.
pub struct ExportPlugin {
    label: String,
    names: Vec<String>,
    injects: [TypeKey; 1],
    keys: ServiceKeys,
}

impl ExportPlugin {
    pub fn new(peer: PeerId, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            label: format!("rutis-bridge/export#{peer}"),
            names: names.into_iter().map(Into::into).collect(),
            injects: [peer_key(&peer).erase()],
            keys: by_name(),
        }
    }

    /// Read each service at `keys(name)` instead of `host_key(name)` (a
    /// service inside an instance, say); a name without a key fails the
    /// export.
    pub fn with_keys(mut self, keys: ServiceKeys) -> Self {
        self.keys = keys;
        self
    }
}

impl DynPlugin for ExportPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let peer = ctx
                .get_key::<Peer>(self.injects[0].clone())
                .ok_or_else(|| failed(&self.label, "the peer is gone"))?;
            let keys = self
                .names
                .iter()
                .map(|name| {
                    (self.keys)(name)
                        .ok_or_else(|| failed(&self.label, format!("{name} is not a service here")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            for (name, key) in self.names.iter().zip(keys) {
                ctx.plugin(ExportOne {
                    label: format!("{}/{name}", self.label),
                    service: name.clone(),
                    injects: [key],
                    peer: peer.clone(),
                    shared: false,
                });
            }
            Ok(Effect::Done)
        })
    }
}

/// Announces one service while it is provided.
pub(crate) struct ExportOne {
    label: String,
    /// The exported service's name.
    service: String,
    injects: [TypeKey; 1],
    /// Its announcements and withdrawals take their versions from the
    /// session ([`Peer::next_version`]): an export restarted with another
    /// list goes on from where the last one ended.
    peer: Arc<Peer>,
    /// Only what a session shares: not a platform plugin's service, nor one
    /// that came over the peer's own session.
    shared: bool,
}

impl ExportOne {
    /// What a session shares of the service `name` (`host_key(name)`) with
    /// `peer`: announced while a plugin that is not a platform one provides
    /// it, unless it came over that same session.
    pub(crate) fn shared(name: &str, peer: Arc<Peer>) -> Self {
        Self {
            label: format!("rutis-bridge/session/export/{name}"),
            service: name.to_owned(),
            injects: [host_key(name).erase()],
            peer,
            shared: true,
        }
    }
}

impl DynPlugin for ExportOne {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let service = ctx
                .get_key::<dyn HostDispatch>(self.injects[0].clone())
                .ok_or_else(|| failed(&self.label, "the service is gone"))?;
            let shape = match service.methods() {
                Some(shape) => shape,
                // What cannot cross is not shared: only a named export fails.
                None if self.shared => return Ok(Effect::Done),
                None => {
                    return Err(failed(
                        &self.label,
                        "the service does not report its methods, so it cannot cross",
                    ))
                }
            };
            let record = record(&service, &shape).map_err(|error| failed(&self.label, error))?;
            let provider = provider(ctx, &self.injects[0]);
            if self.shared {
                let own = service.origin() == Some(self.peer.connection().tag());
                let platform = provider.as_ref().is_none_or(|provider| provider.platform);
                if own || platform {
                    return Ok(Effect::Done);
                }
            }
            let plugin = provider
                .map(|provider| provider.plugin)
                .unwrap_or_else(|| self.service.clone());
            let version = self.peer.next_version();
            let announcer = tokio::spawn(announce(
                self.peer.clone(),
                self.service.clone(),
                record,
                shape,
                version,
                plugin,
            ));
            let peer = self.peer.clone();
            let name = self.service.clone();
            ctx.effect(move || {
                async move {
                    announcer.abort();
                    let version = peer.next_version();
                    // Withdrawn there before this fiber ends. A peer that
                    // went away has nothing to withdraw.
                    let withdrawn = peer
                        .connection()
                        .invoke_async(
                            "",
                            "services.withdraw",
                            json!([{ "name": name, "version": version }]).into(),
                        )
                        .await;
                    if let Ok(withdrawn) = withdrawn {
                        let _ = crate::session::rpc::settle(withdrawn).await;
                    }
                    Ok(())
                }
            })?;
            Ok(Effect::Done)
        })
    }
}

/// A service as it crosses: one function per method of `shape`.
pub(crate) fn record(service: &Arc<dyn HostDispatch>, shape: &Json) -> Result<Value, Error> {
    let methods: BTreeMap<String, Json> = crate::session::decode(shape.clone())?;
    Ok(Value::Record(
        methods
            .keys()
            .map(|method| {
                let service = service.clone();
                let method_name = method.clone();
                let call = Value::callback(move |args| service.invoke(&method_name, args));
                (method.clone(), call)
            })
            .collect(),
    ))
}

/// Who provides the service at `key`, as `ctx` sees it.
pub(crate) struct Provider {
    /// The provider's row, else its name: what `plugin` says of it.
    pub(crate) plugin: String,
    /// Its row, if it has one.
    pub(crate) row: Option<String>,
    /// A platform plugin.
    pub(crate) platform: bool,
}

/// Who provides the service at `key` (outside any scope), if anyone does.
pub(crate) fn provider(ctx: &Ctx, key: &TypeKey) -> Option<Provider> {
    let diagnostics = ctx.diagnostics();
    let binding = diagnostics
        .bindings
        .iter()
        .find(|binding| &binding.key == key && binding.scope.is_none() && !binding.removing)?;
    let plugin = diagnostics
        .plugins
        .iter()
        .find(|plugin| plugin.id == binding.provider)?;
    Some(Provider {
        plugin: plugin.row.clone().unwrap_or_else(|| plugin.name.clone()),
        row: plugin.row.clone(),
        platform: plugin.platform,
    })
}

/// Announce whenever the far end offers `services`: at once if it does,
/// again for every new offer of them (an importer that started again).
pub(crate) async fn announce(
    peer: Arc<Peer>,
    name: String,
    service: Value,
    shape: Json,
    version: u64,
    plugin: String,
) {
    let mut offers = peer.offers();
    let mut announced_to = None;
    loop {
        let offered = offers.borrow_and_update().epoch("services");
        if offered.is_some() && offered != announced_to {
            let announced = peer
                .connection()
                .invoke_async(
                    "",
                    "services.announce",
                    Value::List(vec![Value::Record(BTreeMap::from([
                        ("name".to_owned(), Value::Data(json!(name))),
                        ("service".to_owned(), service.clone()),
                        ("shape".to_owned(), Value::Data(shape.clone())),
                        ("version".to_owned(), Value::Data(json!(version))),
                        ("plugin".to_owned(), Value::Data(json!(plugin))),
                    ]))]),
                )
                .await;
            // Settled: an importer may answer once it has provided it.
            let announced = match announced {
                Ok(reply) => crate::session::rpc::settle(reply).await,
                Err(error) => Err(error),
            };
            if let Err(error) = announced {
                eprintln!("{}: cannot offer {name}: {error}", peer.id());
            }
        }
        announced_to = offered;
        if offers.changed().await.is_err() {
            return;
        }
    }
}

// ── import ──────────────────────────────────────────────────────────

/// Registers the services `names` that `Peer#<peer>` announces as local
/// services (`host_key(name)`), replaced and withdrawn as it says. A name
/// already provided here is refused, naming both sides.
pub struct ImportPlugin {
    label: String,
    peer: PeerId,
    names: HashSet<String>,
    injects: [TypeKey; 1],
    keys: ServiceKeys,
}

impl ImportPlugin {
    pub fn new(peer: PeerId, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            label: format!("rutis-bridge/import#{peer}"),
            injects: [peer_key(&peer).erase()],
            names: names.into_iter().map(Into::into).collect(),
            peer,
            keys: by_name(),
        }
    }

    /// Provide each service at `keys(name)` instead of `host_key(name)` (a
    /// service inside an instance, say); a name without a key fails the
    /// import.
    pub fn with_keys(mut self, keys: ServiceKeys) -> Self {
        self.keys = keys;
        self
    }
}

/// One imported service: calls go to the far end, through one function
/// reference per method, or one object reference (as Cordis sends a
/// service object).
pub(crate) struct Remote {
    /// The session the service is on.
    session: Connection,
    target: Target,
    asynchronous: HashSet<String>,
    shape: Json,
    /// Its name, for messages.
    name: String,
    /// Who provides it, for messages ("peer office").
    provider: String,
    /// The session's tag, when the service is to be known as that
    /// session's ([`HostDispatch::origin`]).
    origin: Option<String>,
}

pub(crate) enum Target {
    Functions(BTreeMap<String, Reference>),
    Object(Reference),
}

enum Call {
    Function(Reference),
    Method(Reference, String),
}

impl HostDispatch for Remote {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let unknown = || {
            let methods = self
                .shape
                .as_object()
                .map(|methods| methods.keys().map(String::as_str).collect())
                .unwrap_or_default();
            Error::no_method(&self.name, method, methods)
        };
        let call = match &self.target {
            Target::Functions(methods) => {
                Call::Function(methods.get(method).ok_or_else(unknown)?.clone())
            }
            Target::Object(object) => {
                self.shape.get(method).ok_or_else(unknown)?;
                Call::Method(object.clone(), method.to_owned())
            }
        };
        // Called on behalf of another session (re-exported to a third
        // node, say): that session's call chain is rebased for this one, so
        // a call back reaches the caller that waits for it.
        let source =
            crate::session::rpc::caller().filter(|source| source.tag() != self.session.tag());
        if self.asynchronous.contains(method) {
            // The far end answers with its own future: wait for that too.
            let call = async move {
                let reply = match call {
                    Call::Function(reference) => reference.call_async(args).await?,
                    Call::Method(object, method) => object.call_method_async(&method, args).await?,
                };
                crate::session::rpc::settle(reply).await
            };
            return Ok(match source {
                Some(source) => Value::future(self.session.forward_async(&source, call)),
                None => Value::future(call),
            });
        }
        let call = || match call {
            Call::Function(reference) => reference.call(args),
            Call::Method(object, method) => object.call_method(&method, args),
        };
        match source {
            Some(source) => self.session.forward(&source, call),
            None => call(),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(self.shape.clone())
    }

    fn provider(&self) -> Option<String> {
        Some(self.provider.clone())
    }

    fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }
}

/// What a `services.announce` says.
pub(crate) struct Announcement {
    pub(crate) name: String,
    pub(crate) version: u64,
    pub(crate) plugin: String,
    shape: Json,
    target: Target,
    asynchronous: HashSet<String>,
}

impl Announcement {
    /// Its methods, `{ method: "sync" | "async" }`.
    pub(crate) fn shape(&self) -> Json {
        self.shape.clone()
    }

    /// Read a `services.announce`'s arguments.
    pub(crate) fn read(args: Value) -> Result<Self, Error> {
        let mut fields = match args.list()?.into_iter().next() {
            Some(Value::Record(fields)) => fields,
            _ => return Err(Error::refused("services.announce needs a record".into())),
        };
        let take = |fields: &mut BTreeMap<String, Value>, key: &str| {
            fields
                .remove(key)
                .ok_or_else(|| Error::refused(format!("services.announce lacks {key}")))
        };
        let name: String = crate::session::decode(take(&mut fields, "name")?.json()?)?;
        let version: u64 = crate::session::decode(take(&mut fields, "version")?.json()?)?;
        let plugin: String = crate::session::decode(take(&mut fields, "plugin")?.json()?)?;
        let shape = take(&mut fields, "shape")?.json()?;
        let kinds: BTreeMap<String, String> = crate::session::decode(shape.clone())?;
        let asynchronous = kinds
            .iter()
            .filter(|(_, kind)| *kind == "async")
            .map(|(method, _)| method.clone())
            .collect();
        let target = match take(&mut fields, "service")? {
            Value::Record(record) => Target::Functions(
                record
                    .into_iter()
                    .map(|(method, value)| value.reference().map(|reference| (method, reference)))
                    .collect::<Result<BTreeMap<_, _>, _>>()?,
            ),
            Value::Reference(object) if object.is_object() => Target::Object(object),
            _ => {
                return Err(Error::refused(
                    "a service crosses as a record of functions or an object".into(),
                ))
            }
        };
        Ok(Self {
            name,
            version,
            plugin,
            shape,
            target,
            asynchronous,
        })
    }

    /// The service it announces, on `session`, provided by `provider`.
    pub(crate) fn service(
        self,
        session: Connection,
        provider: String,
        origin: Option<String>,
    ) -> Remote {
        Remote {
            session,
            target: self.target,
            asynchronous: self.asynchronous,
            shape: self.shape,
            name: self.name,
            provider,
            origin,
        }
    }
}

/// The newest the peer said of a name: provided at `version`, or, with no
/// `provided`, withdrawn at it. A withdrawal is kept so an older
/// announcement arriving after it (calls are dispatched concurrently) does
/// not bring the service back.
struct Imported {
    version: u64,
    provided: Option<Disposer>,
}

struct Importer {
    ctx: Ctx,
    session: Connection,
    peer: PeerId,
    /// The imported names, with the key each is provided under here.
    names: HashMap<String, TypeKey>,
    services: Mutex<HashMap<String, Imported>>,
}

impl Importer {
    fn announce(&self, args: Value) -> Reply {
        let announced = Announcement::read(args)?;
        let (name, version) = (announced.name.clone(), announced.version);
        let Some(key) = self.names.get(&name).cloned() else {
            // Not imported here: the reference is dropped, which releases it.
            return Ok(Value::Undefined);
        };
        let mut services = self.services.lock().unwrap();
        if services
            .get(&name)
            .is_some_and(|current| current.version >= version)
        {
            return Ok(Value::Undefined);
        }
        let previous = services.remove(&name);
        let replacing = previous
            .as_ref()
            .is_some_and(|previous| previous.provided.is_some());
        if !replacing && self.ctx.get_key::<dyn HostDispatch>(key.clone()).is_some() {
            return Err(Error::refused(format!(
                "{name} is already provided here: the import from {} is refused",
                self.peer
            )));
        }
        let provided = self
            .ctx
            .provide_key::<dyn HostDispatch>(
                key,
                Arc::new(announced.service(
                    self.session.clone(),
                    format!("peer {}", self.peer),
                    None,
                )),
            )
            .map_err(|error| Error::refused(error.to_string()))?;
        services.insert(
            name,
            Imported {
                version,
                provided: Some(provided),
            },
        );
        drop(services);
        if let Some(mut previous) = previous {
            if let Some(provided) = previous.provided.take() {
                tokio::spawn(async move {
                    let _ = provided.dispose().await;
                });
            }
        }
        Ok(Value::Undefined)
    }

    fn withdraw(&self, args: Value) -> Reply {
        let fields = args
            .list()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::refused("services.withdraw needs its name".into()))?
            .json()?;
        let name: String = crate::session::decode(fields["name"].clone())?;
        let version: u64 = crate::session::decode(fields["version"].clone())?;
        if !self.names.contains_key(&name) {
            return Ok(Value::Undefined);
        }
        let removed = {
            let mut services = self.services.lock().unwrap();
            match services.get(&name) {
                Some(current) if current.version >= version => None,
                // Withdrawn at `version`, even if not (yet) announced: an
                // older announcement is stale.
                _ => services.insert(
                    name,
                    Imported {
                        version,
                        provided: None,
                    },
                ),
            }
        };
        if let Some(mut removed) = removed {
            if let Some(provided) = removed.provided.take() {
                return Ok(Value::future(async move {
                    provided
                        .dispose()
                        .await
                        .map_err(|error| Error::refused(error.to_string()))?;
                    Ok(Value::Undefined)
                }));
            }
        }
        Ok(Value::Undefined)
    }
}

impl crate::Handler for Importer {
    fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
        match method {
            "services.announce" => self.announce(args),
            "services.withdraw" => self.withdraw(args),
            _ => Err(Error::refused(format!("{method} is not offered here"))),
        }
    }
}

impl DynPlugin for ImportPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let peer = ctx
                .get_key::<Peer>(self.injects[0].clone())
                .ok_or_else(|| failed(&self.label, "the peer is gone"))?;
            let names = self
                .names
                .iter()
                .map(|name| {
                    let key = (self.keys)(name).ok_or_else(|| {
                        failed(&self.label, format!("{name} is not a service here"))
                    })?;
                    Ok((name.clone(), key))
                })
                .collect::<Result<HashMap<_, _>, RutisError>>()?;
            let importer = Arc::new(Importer {
                ctx: ctx.clone(),
                session: peer.connection().clone(),
                peer: self.peer.clone(),
                names,
                services: Mutex::default(),
            });
            let offered: Offered = peer
                .register("services", importer.clone())
                .map_err(|error| failed(&self.label, error))?;
            ctx.effect(move || {
                async move {
                    // Unregistering stops announcements; the imported
                    // services were provided from this fiber and go with it.
                    drop(offered);
                    importer.services.lock().unwrap().clear();
                    Ok(())
                }
            })?;
            Ok(Effect::Done)
        })
    }
}
