//! Rows on the far end of a connection row's session: `<row>:<plugin>`
//! runs `<plugin>` there (`docs/design-remote-session-2026-10-28.md` §4.3).
//!
//! A connection row `gpu` mounts a `rutis-bridge/session` on its channel;
//! once mounted, the session provides its handle at
//! [`session_key`]`("gpu")`. A row `gpu:embedder` loads `embedder` through
//! the session's process, the `rows.v2` path a runtime's rows take, so it
//! keeps its `isolate`, `inject` and event scope.
//!
//! As for runtimes, two stages: the first resolution of a `gpu:…` row
//! mounts a [`SessionRowsPlugin`] under the loader, which waits for the
//! session, resolves again the rows resolved without it (or with an
//! earlier session), and only then provides [`SessionRows`]`#gpu`, which
//! every such row depends on. Without the session a row waits
//! (`waiting for gpu`); its session ending stops it, and it waits again. A
//! far end without the plugin fails the row alone, [`Unavailable`]
//! (`runtime gpu: this connection does not contain plugin embedder`).
//!
//! [`SessionResolver`] goes last in the loader's chain: a name another
//! resolver takes (a runtime named like the connection row, `peer:`, an
//! npm name) stays theirs, so existing texts do not change. It resolves
//! only `<x>:<plugin>` whose `x` may name a runtime and is the id of one of
//! the loader's rows; anything else is not found.
//!
//! [`Unavailable`]: crate::report::Unavailable

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, InstanceId, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error};
use rutis_bridge::runtime::Process;
use rutis_bridge::{session_key, SessionHandle};
use serde_json::{json, Map};

use super::discover::{self, Described};
use super::{
    is_runtime_name, leaf_resolved_on, unavailable_resolved, unknown, JsFactory, JsModule,
    RowRuntime,
};
use crate::loader::WeakLoader;
use crate::resolver::Resolved;
use crate::{Loader, LoaderError, Resolver, ServiceCatalog};

/// The service the rows of a connection row's session depend on: the
/// session's process, once the rows' declarations are complete.
pub struct SessionRows {
    process: Arc<Process>,
}

impl SessionRows {
    /// The key for the rows of the session of the connection row `row`.
    pub fn key(row: &str) -> TypeKey {
        TypeKey::keyed_dynamic::<SessionRows>(row.to_owned())
    }

    /// What runs the rows there.
    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }
}

/// Resolves `<row>:<plugin>`, the plugin run on the far end of the session
/// of the connection row `row`. Put it last in the loader's chain
/// (`Chain::with_shared`), and [`attach`](SessionResolver::attach) the
/// loader to it before the loader is mounted.
#[derive(Clone)]
pub struct SessionResolver {
    inner: Arc<Sessions>,
}

struct Sessions {
    catalog: ServiceCatalog,
    loader: Mutex<Option<WeakLoader>>,
    /// The connection rows whose [`SessionRowsPlugin`] is mounted, each
    /// with the loader context it is mounted under.
    mounted: Mutex<HashMap<String, InstanceId>>,
    /// Names resolved without their session: resolved again before their
    /// rows may start. A name leaves only once resolved with a session.
    offline: Mutex<HashSet<String>>,
    /// Names resolved with a session, by the session's connection tag:
    /// another session may run other code, so they are resolved again
    /// with it.
    described: Mutex<HashMap<String, String>>,
}

impl SessionResolver {
    /// `catalog` is the loader's (`LoaderOptions::catalog`): its shared
    /// names are the injected services rows wait for in rutis.
    pub fn new(catalog: &ServiceCatalog) -> Self {
        Self {
            inner: Arc::new(Sessions {
                catalog: catalog.clone(),
                loader: Mutex::default(),
                mounted: Mutex::default(),
                offline: Mutex::default(),
                described: Mutex::default(),
            }),
        }
    }

    /// The loader whose rows name the connections. Without it nothing
    /// resolves.
    pub fn attach(&self, loader: &Loader) {
        *self.inner.loader.lock().unwrap() = Some(loader.downgrade());
    }

    fn loader(&self) -> Option<Loader> {
        self.inner.loader.lock().unwrap().as_ref()?.upgrade()
    }

    /// `<row>:<plugin>`, when `row` may name a connection.
    fn split(name: &str) -> Option<(&str, &str)> {
        let (row, plugin) = name.split_once(':')?;
        (is_runtime_name(row) && !plugin.is_empty()).then_some((row, plugin))
    }

    /// Mount the [`SessionRowsPlugin`] of `row` under the loader, once per
    /// loader context.
    fn follow(&self, ctx: &Ctx, row: &str) {
        let mut mounted = self.inner.mounted.lock().unwrap();
        if mounted.get(row) == Some(&ctx.instance()) {
            return;
        }
        mounted.insert(row.to_owned(), ctx.instance());
        ctx.plugin(SessionRowsPlugin::new(row, self.clone()));
    }

    /// The names of `row`'s rows to resolve again before they start with
    /// the session whose connection is tagged `session`.
    fn take_stale(&self, row: &str, session: &str) -> HashSet<String> {
        let ours = |name: &str| Self::split(name).is_some_and(|(of, _)| of == row);
        let mut stale: HashSet<String> = self
            .inner
            .offline
            .lock()
            .unwrap()
            .iter()
            .filter(|name| ours(name))
            .cloned()
            .collect();
        for (name, by) in self.inner.described.lock().unwrap().iter() {
            if ours(name) && by != session {
                stale.insert(name.clone());
            }
        }
        stale
    }

    /// A row resolved without its session: it waits for it.
    fn offline(&self, name: &str, row: &str, plugin: &str) -> Arc<Resolved> {
        let why = format!("{row} is not connected");
        let runtime = RowRuntime::Session(row.to_owned());
        let module = JsModule {
            name: name.to_owned(),
            runtime: runtime.clone(),
            entry: PathBuf::from(plugin),
            gated: Vec::new(),
            provides: Map::new(),
            catalog: self.inner.catalog.clone(),
        };
        Arc::new(Resolved {
            factory: Arc::new(JsFactory::on(
                name,
                runtime,
                PathBuf::from(plugin),
                Vec::new(),
                Map::new(),
            )),
            schema: None,
            meta: json!({
                "source": "runtime",
                "session": row,
                "entry": plugin,
                "schema": format!("unavailable: {why}"),
            }),
            foreign_scope: true,
            scoped: Some(module.scoped()),
            declares: Some(unknown(why)),
        })
    }
}

impl Resolver for SessionResolver {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            let not_found = || LoaderError::NotFound {
                name: name.to_owned(),
            };
            let Some((row, plugin)) = Self::split(name) else {
                return Err(not_found());
            };
            let Some(loader) = self.loader() else {
                return Err(not_found());
            };
            if loader.get(row).is_none() {
                return Err(not_found());
            }
            let ctx = loader.ctx();
            if let Some(ctx) = &ctx {
                self.follow(ctx, row);
            }
            // Marked first: a session arriving between reading it and
            // marking the name would leave the name unmarked.
            let marked_before = !self.inner.offline.lock().unwrap().insert(name.to_owned());
            let process = ctx
                .and_then(|ctx| ctx.get_at(&session_key(row)))
                .and_then(|handle: Arc<SessionHandle>| handle.process());
            let Some(process) = process else {
                return Ok(self.offline(name, row, plugin));
            };
            let tag = process.connection().tag().to_owned();
            let entry = PathBuf::from(plugin);
            let described = match discover::describe(&process, Path::new(plugin)).await {
                Described::Found(found) => found,
                Described::Missing => {
                    self.inner.offline.lock().unwrap().remove(name);
                    self.inner
                        .described
                        .lock()
                        .unwrap()
                        .insert(name.to_owned(), tag);
                    let why = discover::missing(row, plugin);
                    return Ok(Arc::new(unavailable_resolved(name, &entry, why)));
                }
                Described::Failed(message) => {
                    if !marked_before {
                        self.inner.offline.lock().unwrap().remove(name);
                    }
                    return Err(LoaderError::Resolve {
                        name: name.to_owned(),
                        message,
                    });
                }
            };
            // A leaf far end gates every service its plugins inject in
            // rutis; Cordis only the shared names.
            let leaf = process.supports("leaf");
            let gated: Vec<String> = described
                .inject
                .iter()
                .filter(|name| leaf || self.inner.catalog.is_shared(name))
                .cloned()
                .collect();
            self.inner.offline.lock().unwrap().remove(name);
            self.inner
                .described
                .lock()
                .unwrap()
                .insert(name.to_owned(), tag);
            Ok(Arc::new(leaf_resolved_on(
                name,
                RowRuntime::Session(row.to_owned()),
                entry,
                gated,
                &described,
                &self.inner.catalog,
                json!({
                    "source": "runtime",
                    "session": row,
                    "entry": plugin,
                    "version": described.version,
                    "inject": described.inject,
                    "provides": described.provides,
                    "cordis": described.cordis,
                }),
            )))
        })
    }
}

/// The second stage of a connection row's session: once the session is
/// there, resolve its rows again, then provide [`SessionRows`]. Mounted by
/// [`SessionResolver`] under the loader.
struct SessionRowsPlugin {
    row: String,
    name: String,
    resolver: SessionResolver,
    injects: [TypeKey; 2],
}

impl SessionRowsPlugin {
    fn new(row: &str, resolver: SessionResolver) -> Self {
        Self {
            injects: [session_key(row).erase(), TypeKey::of::<Loader>()],
            name: format!("rows of connection {row}"),
            row: row.to_owned(),
            resolver,
        }
    }
}

impl DynPlugin for SessionRowsPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let handle = ctx.require_at(&session_key(&self.row))?;
            let loader = ctx.require::<Loader>()?;
            // Provided only once the process is up (rutis-bridge).
            let Some(process) = handle.process() else {
                return Ok(Effect::Done);
            };
            let stale = self
                .resolver
                .take_stale(&self.row, process.connection().tag());
            let rows: Vec<String> = loader
                .entries()
                .into_iter()
                .filter(|entry| {
                    entry.options["name"]
                        .as_str()
                        .is_some_and(|name| stale.contains(name))
                })
                .map(|entry| entry.id)
                .collect();
            let (owner, key, row) = (ctx.clone(), SessionRows::key(&self.row), self.row.clone());
            // In a task: reloading takes the loader's lock, which a
            // reconcile that (re)started this plugin may hold.
            let refresh = tokio::spawn(async move {
                for id in rows {
                    // A row that fails to resolve says so in its status.
                    let _ = loader.refresh(&id).await;
                }
                if let Err(error) = owner.provide_key(key, Arc::new(SessionRows { process })) {
                    if !owner.cancellation_token().is_cancelled() {
                        eprintln!("connection {row}: cannot release its rows: {error}");
                    }
                }
            });
            let refresh = Mutex::new(Some(refresh));
            ctx.effect(move || async move {
                if let Some(refresh) = refresh.lock().unwrap().take() {
                    refresh.abort();
                }
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}
