//! The services of both kernels, seen on both sides of a session.
//!
//! Export: every service a plugin (not a platform one) provides by name
//! here (`host_key(name)`, outside any instance or isolated scope) is
//! announced to the far end, unless it came over this same session; one
//! child per name follows its provider ([`ExportOne`]).
//!
//! Import: what the far end announces is provided here by the remote
//! plugin `<id>:<plugin>` its `plugin` names, one per plugin; a name taken
//! here already fails that plugin, as the kernel refuses the later
//! provider. A plugin whose services are all withdrawn unloads. Services
//! of a row this end runs there ([`super::run`]) are that row's.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rutis::host::diagnostics::{ServiceChange, ServiceChanged};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberView, TypeKey};
use rutis::{Ctx, Disposer, Error as RutisError};

use crate::channel::PeerId;
use crate::peer::Peer;
use crate::runtime::Process;
use crate::services::{provider, Announcement, ExportOne};
use crate::session::rpc::{Connection, Reply, Value};
use crate::session::{host_key, Error};

/// The name of the host service `key` is, if it is one a session shares:
/// `host_key(name)` outside any instance.
fn shared_name(key: &TypeKey) -> Option<&str> {
    let name = key.name()?;
    (key.instance_id().is_none() && host_key(name).erase() == *key).then_some(name)
}

/// The names a session exports, each followed by an [`ExportOne`].
pub(super) struct Exports {
    ctx: Ctx,
    peer: Arc<Peer>,
    names: Mutex<HashSet<String>>,
}

impl Exports {
    /// Follow every shared service from now on, starting with those there
    /// are. Returns what stops following.
    pub(super) fn follow(ctx: &Ctx, peer: Arc<Peer>) -> Result<Disposer, RutisError> {
        let exports = Arc::new(Self {
            ctx: ctx.clone(),
            peer,
            names: Mutex::default(),
        });
        // Listening first, then what is provided already: a service
        // provided in between is seen twice, and followed once.
        let following = exports.clone();
        let listening = ctx.root_ctx().on::<ServiceChanged>(
            move |changed: Arc<ServiceChanged>, _call: rutis::events::Call<ServiceChanged>| {
                if changed.change == ServiceChange::Provided && changed.scope.is_none() {
                    following.export(&changed.key);
                }
                async { Ok(None) }
            },
        )?;
        for binding in ctx.diagnostics().bindings {
            if binding.scope.is_none() && !binding.removing {
                exports.export(&binding.key);
            }
        }
        Ok(listening)
    }

    fn export(&self, key: &TypeKey) {
        let Some(name) = shared_name(key) else {
            return;
        };
        if self.names.lock().unwrap().insert(name.to_owned()) {
            self.ctx.plugin(ExportOne::shared(name, self.peer.clone()));
        }
    }
}

/// A plugin of the far end here (`<id>:<plugin>`), or a row this end runs
/// there: what fails it.
#[derive(Clone)]
pub(super) struct Target {
    inner: Arc<TargetInner>,
}

struct TargetInner {
    /// Its fiber's context, once it applied.
    ctx: Mutex<Option<Ctx>>,
    view: Mutex<Option<FiberView>>,
    /// Why it failed: its next apply fails with it.
    refused: Mutex<Option<String>>,
    /// It failed: what the far end says of it is no longer provided.
    failed: Mutex<bool>,
}

impl Target {
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(TargetInner {
                ctx: Mutex::default(),
                view: Mutex::default(),
                refused: Mutex::default(),
                failed: Mutex::new(false),
            }),
        }
    }

    /// Its fiber.
    pub(super) fn set_view(&self, view: FiberView) {
        *self.inner.view.lock().unwrap() = Some(view);
    }

    /// Its fiber applies with `ctx`: fails if it was refused.
    pub(super) fn applied(&self, ctx: &Ctx) -> Result<(), RutisError> {
        if let Some(refused) = self.inner.refused.lock().unwrap().take() {
            return Err(RutisError::from(ErrorKind::PluginFailed(refused.into())));
        }
        *self.inner.ctx.lock().unwrap() = Some(ctx.clone());
        Ok(())
    }

    /// Fail its fiber with `why`: it restarts and fails so.
    pub(super) fn fail(&self, why: String) {
        *self.inner.failed.lock().unwrap() = true;
        *self.inner.refused.lock().unwrap() = Some(why);
        self.inner.ctx.lock().unwrap().take();
        if let Some(view) = self.inner.view.lock().unwrap().clone() {
            tokio::spawn(async move {
                let _ = view.restart().await;
            });
        }
    }

    fn failed(&self) -> bool {
        *self.inner.failed.lock().unwrap()
    }

    fn ctx(&self) -> Option<Ctx> {
        self.inner.ctx.lock().unwrap().clone()
    }

    async fn dispose(&self) {
        let view = self.inner.view.lock().unwrap().clone();
        if let Some(view) = view {
            let _ = view.dispose().await;
        }
    }
}

/// A plugin of the far end, here: what it provides is provided from its
/// fiber.
struct RemotePlugin {
    name: String,
    target: Target,
}

impl DynPlugin for RemotePlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            self.target.applied(ctx)?;
            Ok(Effect::Done)
        })
    }
}

/// The newest the far end said of a name: provided at `version` by
/// `plugin`, or, with no `provided`, withdrawn at it.
struct Imported {
    version: u64,
    plugin: String,
    provided: Option<Disposer>,
    /// Given to the row of this end's process that `plugin` is.
    routed: bool,
}

#[derive(Default)]
struct ImportState {
    services: HashMap<String, Imported>,
    /// The remote plugins, by the far end's name for them.
    plugins: HashMap<String, Target>,
    /// Every row of this end's process whose services came: a late
    /// announcement of one that is gone is not another plugin.
    rows: HashSet<String>,
}

/// What the far end provides, provided here.
pub(super) struct Imports {
    ctx: Ctx,
    far: PeerId,
    session: Connection,
    state: tokio::sync::Mutex<ImportState>,
    /// What runs this end's rows there, once it does.
    process: Mutex<Option<Arc<Process>>>,
}

impl Imports {
    pub(super) fn new(ctx: &Ctx, far: PeerId, session: Connection) -> Arc<Self> {
        Arc::new(Self {
            ctx: ctx.clone(),
            far,
            session,
            state: tokio::sync::Mutex::default(),
            process: Mutex::default(),
        })
    }

    /// This end's rows there run on `process`.
    pub(super) fn process(&self, process: Arc<Process>) {
        *self.process.lock().unwrap() = Some(process);
    }

    /// Who provides `name` here, in a refusal.
    fn holder(&self, name: &str) -> String {
        let key = host_key(name);
        match provider(&self.ctx, &key.erase()) {
            Some(provider) => match provider.row {
                Some(row) => format!("row {row}"),
                None => match self.ctx.get_at(&key).and_then(|service| service.provider()) {
                    Some(by) => by,
                    None => format!("plugin {}", provider.plugin),
                },
            },
            None => "another plugin".to_owned(),
        }
    }

    async fn announce(self: Arc<Self>, announced: Announcement) -> Result<(), Error> {
        let mut state = self.state.lock().await;
        let (name, version, plugin) = (
            announced.name.clone(),
            announced.version,
            announced.plugin.clone(),
        );
        if state
            .services
            .get(&name)
            .is_some_and(|current| current.version >= version)
        {
            return Ok(());
        }
        let previous = state.services.remove(&name);
        let process = self.process.lock().unwrap().clone();
        // A row of this end's: its projection takes it.
        if let Some(process) = process.clone().filter(|process| process.has_row(&plugin)) {
            if let Some(provided) = previous.and_then(|previous| previous.provided) {
                let _ = provided.dispose().await;
            }
            state.rows.insert(plugin.clone());
            let handle = format!("{}:{name}", super::rows::ROW_FAMILY);
            let routed =
                process.announce_row_service(&plugin, &name, &announced.shape(), handle, version);
            state.services.insert(
                name,
                Imported {
                    version,
                    plugin,
                    provided: None,
                    routed,
                },
            );
            return Ok(());
        }
        // A replacement: the old one goes first, as the kernel has one
        // provider per name.
        if let Some(previous) = previous {
            if let Some(provided) = previous.provided {
                let _ = provided.dispose().await;
            }
            if previous.routed {
                if let Some(process) = &process {
                    process.withdraw_row_service(&name, version);
                }
            }
        }
        let entry = Imported {
            version,
            plugin: plugin.clone(),
            provided: None,
            routed: false,
        };
        let target = match state.plugins.get(&plugin) {
            // A row of this end's that is gone: nothing of it comes back.
            _ if state.rows.contains(&plugin) => None,
            Some(target) => Some(target.clone()),
            None => {
                let target = Target::new();
                let view = self.ctx.plugin(RemotePlugin {
                    name: format!("{}:{plugin}", self.far),
                    target: target.clone(),
                });
                target.set_view(view.clone());
                state.plugins.insert(plugin.clone(), target.clone());
                let _ = view.await;
                Some(target)
            }
        };
        let provided = match target.filter(|target| !target.failed()) {
            None => None,
            Some(target) => match target.ctx() {
                None => None,
                Some(ctx) => {
                    let service = announced.service(
                        self.session.clone(),
                        format!("row {}:{plugin}", self.far),
                        Some(self.session.tag().to_owned()),
                    );
                    match ctx.provide_at(host_key(&name), Arc::new(service)) {
                        Ok(provided) => Some(provided),
                        Err(error) => {
                            let why = match &*error {
                                ErrorKind::ServiceExists(_) => format!(
                                    "{}:{plugin}: {name} is provided here by {}: not registered from {}",
                                    self.far,
                                    self.holder(&name),
                                    self.far
                                ),
                                other => format!("{}:{plugin}: {name}: {other}", self.far),
                            };
                            target.fail(why);
                            None
                        }
                    }
                }
            },
        };
        state.services.insert(name, Imported { provided, ..entry });
        Ok(())
    }

    async fn withdraw(self: Arc<Self>, name: String, version: u64) -> Result<(), Error> {
        let mut state = self.state.lock().await;
        let plugin = match state.services.get_mut(&name) {
            Some(current) if current.version >= version => return Ok(()),
            Some(current) => {
                current.version = version;
                if let Some(provided) = current.provided.take() {
                    let _ = provided.dispose().await;
                }
                if std::mem::take(&mut current.routed) {
                    if let Some(process) = self.process.lock().unwrap().clone() {
                        process.withdraw_row_service(&name, version);
                    }
                }
                current.plugin.clone()
            }
            // Withdrawn at `version` before it was announced: an older
            // announcement is stale.
            None => {
                state.services.insert(
                    name,
                    Imported {
                        version,
                        plugin: String::new(),
                        provided: None,
                        routed: false,
                    },
                );
                return Ok(());
            }
        };
        // A remote plugin providing nothing any more has ended there.
        let providing = state
            .services
            .values()
            .any(|service| service.plugin == plugin && service.provided.is_some());
        if !providing {
            if let Some(target) = state.plugins.remove(&plugin) {
                drop(state);
                target.dispose().await;
            }
        }
        Ok(())
    }
}

/// `services.*` from the far end.
pub(super) struct Importing(pub(super) Arc<Imports>);

impl crate::peer::Handler for Importing {
    fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
        let imports = self.0.clone();
        match method {
            "services.announce" => {
                let announced = Announcement::read(args)?;
                Ok(Value::future(async move {
                    imports.announce(announced).await?;
                    Ok(Value::Undefined)
                }))
            }
            "services.withdraw" => {
                let fields = args
                    .list()?
                    .into_iter()
                    .next()
                    .ok_or_else(|| Error::refused("services.withdraw needs its name".into()))?
                    .json()?;
                let name: String = crate::session::decode(fields["name"].clone())?;
                let version: u64 = crate::session::decode(fields["version"].clone())?;
                Ok(Value::future(async move {
                    imports.withdraw(name, version).await?;
                    Ok(Value::Undefined)
                }))
            }
            _ => Err(Error::refused(format!("{method} is not offered here"))),
        }
    }
}
