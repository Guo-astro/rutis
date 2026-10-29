//! Services across a session: what this end shares is announced to the
//! far end, what the far end announces is provided here
//! (`session::plugin::services`).
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

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use crate::session::rpc::{Connection, Reference, Reply, Value};
use crate::session::{host_key, Error, HostDispatch};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};
use serde_json::{json, Value as Json};

use crate::peer::Peer;

fn failed(label: &str, error: impl std::fmt::Display) -> RutisError {
    rutis::Error::from(ErrorKind::PluginFailed(format!("{label}: {error}").into()))
}

/// Announces one service while it is provided.
pub(crate) struct ExportOne {
    label: String,
    /// The exported service's name.
    service: String,
    injects: [TypeKey; 1],
    /// Its announcements and withdrawals take their versions from the
    /// session ([`Peer::next_version`]).
    peer: Arc<Peer>,
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
            // What cannot cross is not shared.
            let Some(shape) = service.methods() else {
                return Ok(Effect::Done);
            };
            let record = record(&service, &shape).map_err(|error| failed(&self.label, error))?;
            let provider = provider(ctx, &self.injects[0]);
            // Not a platform plugin's service, nor one that came over this
            // same session.
            let own = service.origin() == Some(self.peer.connection().tag());
            let platform = provider.as_ref().is_none_or(|provider| provider.platform);
            if own || platform {
                return Ok(Effect::Done);
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
