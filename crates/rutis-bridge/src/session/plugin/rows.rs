//! The row contract between two rutis kernels: each answers the other's
//! `mount` and `rows.*` as a runtime does, from the plugins it can load
//! (the [`PluginCatalog`] its host provides at [`catalog_key`]), and runs
//! the other's rows through a [`Process`] of its own, so `run` is the same
//! `rows.v2` whatever the far end is.
//!
//! A row loaded here for the far end is a plugin below the session, marked
//! as the row of its key: the services it provides reach the far end as
//! any service here does, with that key as their `plugin`, and the far end
//! gives them to its row.
//!
//! The two directions share the `rows` family: `rows.ended` and
//! `rows.failed` are this end's rows ending there; the rest are the far
//! end's rows here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView, Key, PluginFactory, TypeKey,
};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};
use serde_json::{json, Value as Json};

use crate::host::PluginCatalog;
use crate::peer::Offered;
use crate::runtime::{Process, Routed, RowSchema, RuntimeSession};
use crate::session::rpc::{Connection, Dispatch, Reply, Value};
use crate::session::{host_key, Error, HostDispatch};
use crate::Peer;

/// The key a host provides the plugins a far rutis kernel may run here at
/// (rutis-loader: its loader's catalog). Without one, a session answers
/// that it contains no plugin.
#[doc(hidden)]
pub fn catalog_key() -> Key<dyn PluginCatalog> {
    Key::dynamic("rutis-bridge/catalog")
}

/// The family of the targets a far end calls the services of its rows
/// here at: `row:<name>`, the service `name` as this kernel has it.
pub(super) const ROW_FAMILY: &str = "row";

/// What a session's row server says it speaks: the row contract, every
/// service a row injects waiting here.
const FEATURES: [&str; 3] = ["rows.v2", "hosts", "leaf"];

fn not_found(name: &str) -> Error {
    Error::Remote {
        name: "NotFound".into(),
        message: format!("no plugin {name} is installed here"),
        graph: None,
    }
}

/// The far end's rows here.
pub(super) struct Server {
    ctx: Ctx,
    session: Connection,
    loaded: Mutex<HashMap<String, FiberView>>,
}

impl Server {
    fn catalog(&self) -> Option<Arc<dyn PluginCatalog>> {
        self.ctx.get_at(&catalog_key())
    }

    fn mount(&self) -> Reply {
        // With a catalog, each plugin is asked for (`rows.schema`); without
        // one there is none.
        let plugins = match self.catalog() {
            Some(_) => Json::Null,
            None => json!({}),
        };
        Ok(Value::Data(json!({
            "services": {},
            "features": FEATURES,
            "plugins": plugins,
        })))
    }

    fn schema(self: &Arc<Self>, args: Vec<Json>) -> Reply {
        let name: String = crate::session::decode(args.into_iter().next().unwrap_or_default())?;
        let server = self.clone();
        Ok(Value::future(async move {
            let catalog = server.catalog().ok_or_else(|| not_found(&name))?;
            let found = catalog.find(&name).await.ok_or_else(|| not_found(&name))?;
            Ok(Value::Data(json!({
                "config": found.described.schema,
                "inject": [],
                "provides": {},
                "version": found.described.version,
                "announces": true,
            })))
        }))
    }

    fn load(self: &Arc<Self>, args: Vec<Json>) -> Reply {
        let mut args = args.into_iter();
        let key: String = crate::session::decode(args.next().unwrap_or_default())?;
        let name: String = crate::session::decode(args.next().unwrap_or_default())?;
        let config = args.next().unwrap_or(Json::Null);
        let server = self.clone();
        Ok(Value::future(async move {
            let catalog = server.catalog().ok_or_else(|| not_found(&name))?;
            let found = catalog.find(&name).await.ok_or_else(|| not_found(&name))?;
            let view = {
                let mut loaded = server.loaded.lock().unwrap();
                if loaded.contains_key(&key) {
                    return Err(Error::refused(format!("{key} is already loaded")));
                }
                let view = server.ctx.plugin_with(
                    Hosted {
                        factory: found.factory,
                        key: key.clone(),
                    },
                    config,
                );
                loaded.insert(key.clone(), view.clone());
                view
            };
            if let Err(error) = (&view).await {
                server.loaded.lock().unwrap().remove(&key);
                let _ = view.dispose().await;
                return Err(Error::Remote {
                    name: "LoadError".into(),
                    message: error.to_string(),
                    graph: None,
                });
            }
            tokio::spawn(server.clone().follow(key, view));
            Ok(Value::Undefined)
        }))
    }

    /// Tell the far end when row `key` ends or fails on its own.
    async fn follow(self: Arc<Self>, key: String, view: FiberView) {
        let mut states = view.watch();
        let mut failed = false;
        loop {
            let (state, error) = {
                let snapshot = states.borrow_and_update();
                (snapshot.state, snapshot.error.clone())
            };
            match state {
                FiberState::Disposed => {
                    // Not unloaded by the far end: it ended here, unless
                    // the session is going away, which ends the far end's
                    // rows as a disconnection does, to wait for the next.
                    let ended = self.loaded.lock().unwrap().remove(&key).is_some();
                    if ended && !self.ctx.cancellation_token().is_cancelled() {
                        let _ = self
                            .session
                            .invoke_async("", "rows.ended", json!([key]).into())
                            .await;
                    }
                    return;
                }
                FiberState::Failed if !failed => {
                    failed = true;
                    let why = error.map_or_else(|| "failed".to_owned(), |error| error.to_string());
                    let _ = self
                        .session
                        .invoke_async("", "rows.failed", json!([key, why]).into())
                        .await;
                }
                FiberState::Failed => {}
                _ => failed = false,
            }
            if states.changed().await.is_err() {
                return;
            }
        }
    }

    fn update(&self, args: Vec<Json>) -> Reply {
        let mut args = args.into_iter();
        let key: String = crate::session::decode(args.next().unwrap_or_default())?;
        let config = args.next().unwrap_or(Json::Null);
        let view = self
            .loaded
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::refused(format!("{key} is not loaded")))?;
        let updated = view.update(config);
        Ok(Value::future(async move {
            updated
                .await
                .map_err(|error| Error::refused(error.to_string()))?;
            Ok(Value::Undefined)
        }))
    }

    fn unload(&self, args: Vec<Json>) -> Reply {
        let key: String = crate::session::decode(args.into_iter().next().unwrap_or_default())?;
        let Some(view) = self.loaded.lock().unwrap().remove(&key) else {
            // Already gone: unloading is idempotent.
            return Ok(Value::Undefined);
        };
        Ok(Value::future(async move {
            view.dispose()
                .await
                .map_err(|error| Error::refused(error.to_string()))?;
            Ok(Value::Undefined)
        }))
    }

    /// A call of a far end's row into the service `name` its row here
    /// provides (or any service by that name here).
    fn call(&self, name: &str, method: &str, args: Value) -> Reply {
        let service = self
            .ctx
            .get_key::<dyn HostDispatch>(host_key(name).erase())
            .ok_or_else(|| {
                Error::binding(
                    crate::session::code::UNAVAILABLE,
                    format!("service {name} is not available"),
                    Some(json!({ "service": name })),
                )
            })?;
        service.invoke(method, args)
    }

    fn invoke(self: &Arc<Self>, method: &str, args: Value) -> Reply {
        let args = match args.json()? {
            Json::Array(args) => args,
            Json::Null => Vec::new(),
            other => vec![other],
        };
        match method {
            "mount" => self.mount(),
            "rows.schema" => self.schema(args),
            "rows.load" => self.load(args),
            "rows.update" => self.update(args),
            "rows.unload" => self.unload(args),
            // Services are seen on both sides already: a row here waits
            // for what it injects as any plugin here does.
            "hosts.provide" | "hosts.withdraw" => Ok(Value::Undefined),
            // `row:<name>` holds nothing to release.
            "release" => Ok(Value::Undefined),
            _ => Err(Error::refused(format!("{method} is not offered here"))),
        }
    }
}

/// A plugin of the catalog, run for the far end as the row `key`.
struct Hosted {
    factory: Arc<dyn PluginFactory<Json>>,
    key: String,
}

impl PluginFactory<Json> for Hosted {
    fn name(&self) -> &str {
        self.factory.name()
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.factory.injects()
    }

    fn validate_config(&self, config: &Json) -> Result<(), RutisError> {
        self.factory.validate_config(config)
    }

    fn build(&self, config: &Json) -> Result<Box<dyn DynPlugin>, RutisError> {
        Ok(Box::new(Marked {
            inner: self.factory.build(config)?,
            key: self.key.clone(),
        }))
    }
}

/// A plugin marked as the row `key`.
struct Marked {
    inner: Box<dyn DynPlugin>,
    key: String,
}

impl DynPlugin for Marked {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.inner.injects()
    }

    fn validate(&self) -> Result<(), RutisError> {
        self.inner.validate()
    }

    fn platform(&self) -> bool {
        self.inner.platform()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            ctx.mark_row(&self.key)?;
            self.inner.apply(ctx).await
        })
    }
}

/// The `rows`, `mount` and `hosts` families: the far end's rows here, and
/// the end of this end's rows there.
pub(super) struct Rows {
    server: Arc<Server>,
    /// What serves this end's process on the far end.
    client: Mutex<Option<Arc<dyn Dispatch>>>,
}

impl Rows {
    /// Serve the row contract on `peer`, loading rows from `ctx`.
    pub(super) fn serve(ctx: &Ctx, peer: &Peer) -> Result<(Arc<Self>, Vec<Offered>), Error> {
        let rows = Arc::new(Self {
            server: Arc::new(Server {
                ctx: ctx.clone(),
                session: peer.connection().clone(),
                loaded: Mutex::default(),
            }),
            client: Mutex::default(),
        });
        let handler: Arc<dyn crate::Handler> = Arc::new(Serving(rows.clone()));
        let offered = ["mount", "hosts", "rows", ROW_FAMILY, "release"]
            .into_iter()
            .map(|family| peer.register(family, handler.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((rows, offered))
    }
}

struct Serving(Arc<Rows>);

impl crate::Handler for Serving {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        if let Some(name) = target
            .strip_prefix(ROW_FAMILY)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            return self.0.server.call(name, method, args);
        }
        match method {
            "rows.ended" | "rows.failed" => {
                let client = self.0.client.lock().unwrap().clone();
                match client {
                    Some(client) => client.invoke(peer, target, method, args),
                    None => Err(Error::refused(format!(
                        "{method}: no row runs there from here"
                    ))),
                }
            }
            _ => self.0.server.invoke(method, args),
        }
    }
}

/// This end's process on a far rutis kernel: its calls into rutis are
/// `host` and `service`, and the end of its rows (through [`Rows`]).
pub(super) struct Client {
    pub(super) peer: Arc<Peer>,
    pub(super) rows: Arc<Rows>,
}

/// Undoes [`Client::route`].
struct Routing {
    _offered: Vec<Offered>,
    rows: Arc<Rows>,
}

impl Drop for Routing {
    fn drop(&mut self) {
        self.rows.client.lock().unwrap().take();
    }
}

impl RuntimeSession for Client {
    fn connection(&self) -> Connection {
        self.peer.connection().clone()
    }

    fn route(
        &self,
        dispatch: Arc<dyn Dispatch>,
    ) -> Result<Box<dyn std::any::Any + Send + Sync>, Error> {
        let handler: Arc<dyn crate::Handler> = Arc::new(Routed(dispatch.clone()));
        let offered = ["host", "service"]
            .into_iter()
            .map(|family| self.peer.register(family, handler.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        *self.rows.client.lock().unwrap() = Some(dispatch);
        Ok(Box::new(Routing {
            _offered: offered,
            rows: self.rows.clone(),
        }))
    }
}

/// What a plugin declares on a session's process.
#[derive(Clone)]
pub(super) enum Described {
    Found(RowSchema),
    /// The far end does not contain it.
    Missing,
    /// Asking failed: why.
    Failed(String),
}

/// What `plugin` declares on `process`: from the plugins its `mount` reply
/// listed, else by asking (`rows.schema`).
pub(super) async fn describe(process: &Process, plugin: &str) -> Described {
    if let Some(plugins) = process.plugins() {
        return match plugins.get(plugin) {
            Some(found) => Described::Found(found.clone()),
            None => Described::Missing,
        };
    }
    match process.describe_row(std::path::Path::new(plugin)).await {
        Ok(found) => Described::Found(found),
        Err(Error::Remote { name, .. }) if name == "NotFound" => Described::Missing,
        Err(error) => Described::Failed(error.to_string()),
    }
}
