//! Rows on the far end of a session: `<row>:<plugin>` runs `<plugin>`
//! there (`docs/design-remote-session-2026-10-28.md` §4.3). The session is
//! a connection row's, or a local runtime's (rutis-host's `runtimes.py` is
//! the row `py`, its process the far end): both are the same here.
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
//! resolver takes (`go:`, `swift:`, `peer:`) stays theirs. It resolves
//! every `<x>:<plugin>` whose `x` may name a runtime, whether or not a row
//! `x` is there yet (it may come in the same batch, or later): until its
//! session is there, the row waits for it. A local runtime without the
//! plugin leaves the name to the next resolver ([`LoaderError::NotFound`]),
//! as an unknown name. With [`SessionResolver::npm`],
//! the other names (npm names, `file:` URLs, absolute paths) run on the
//! Node runtime's session.
//!
//! [`Unavailable`]: crate::report::Unavailable

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, InstanceId, Key, TypeKey};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::Process;
use rutis_bridge::{session_key, SessionHandle};
use serde_json::{json, Map};

use rutis_bridge::runtime::RuntimeHandle;

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
    pub fn key(row: &str) -> Key<SessionRows> {
        Key::dynamic(row.to_owned())
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
    /// The row whose session runs the names no runtime prefixes (Node's).
    npm: Option<String>,
    /// The local runtimes, by row: what [`Loader::status`] says of them.
    runtimes: Mutex<Vec<RuntimeHandle>>,
    loader: Mutex<Option<WeakLoader>>,
    /// The connection rows whose [`SessionRowsPlugin`] is mounted, each
    /// with the loader context it is mounted under.
    mounted: Mutex<HashMap<String, InstanceId>>,
    /// Names resolved without their session, with the row they run on:
    /// resolved again before their rows may start. A name leaves only once
    /// resolved with a session.
    offline: Mutex<HashMap<String, String>>,
    /// Names resolved with a session, with the row and the session's
    /// connection tag: another session may run other code, so they are
    /// resolved again with it.
    described: Mutex<HashMap<String, (String, String)>>,
}

impl SessionResolver {
    /// `catalog` is the loader's (`LoaderOptions::catalog`): its shared
    /// names are the injected services rows wait for in rutis.
    pub fn new(catalog: &ServiceCatalog) -> Self {
        Self::with(catalog, None)
    }

    /// [`SessionResolver::new`], the names no runtime prefixes (npm names,
    /// subpaths of them, `file:` URLs, absolute paths) running on the
    /// session of the row `row` (rutis-host's Node runtime, `node`).
    pub fn npm(catalog: &ServiceCatalog, row: &str) -> Self {
        Self::with(catalog, Some(row.to_owned()))
    }

    fn with(catalog: &ServiceCatalog, npm: Option<String>) -> Self {
        Self {
            inner: Arc::new(Sessions {
                catalog: catalog.clone(),
                npm,
                runtimes: Mutex::default(),
                loader: Mutex::default(),
                mounted: Mutex::default(),
                offline: Mutex::default(),
                described: Mutex::default(),
            }),
        }
    }

    /// The local runtime `runtime` runs the rows of the session of the row
    /// named like it: [`Loader::status`] lists it, and a row waiting for
    /// it says the runtime's state.
    pub fn runtime(&self, runtime: RuntimeHandle) {
        let mut runtimes = self.inner.runtimes.lock().unwrap();
        runtimes.retain(|known| known.name() != runtime.name());
        runtimes.push(runtime);
    }

    /// The local runtime `name` no longer runs: the host stopped it.
    pub fn forget_runtime(&self, name: &str) {
        self.inner
            .runtimes
            .lock()
            .unwrap()
            .retain(|known| known.name() != name);
    }

    /// Release the rows of `row`'s session as soon as it is there, before
    /// any of them is resolved: for a local runtime the host waits for
    /// (`rutis-host`'s `runtimes_ready`). Once the loader runs.
    pub fn follow_row(&self, row: &str) {
        if let Some(ctx) = self.loader().and_then(|loader| loader.ctx()) {
            self.follow(&ctx, row);
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

    /// The row `name` runs on and the plugin it runs there:
    /// `<row>:<plugin>`, when `row` may name a runtime; else the npm row's.
    fn split<'n>(&'n self, name: &'n str) -> Option<(&'n str, &'n str)> {
        if let Some((row, plugin)) = name.split_once(':') {
            if is_runtime_name(row) {
                return (!plugin.is_empty()).then_some((row, plugin));
            }
        }
        let npm = self.inner.npm.as_deref()?;
        (!name.is_empty()).then_some((npm, name))
    }

    /// Mount the [`SessionRowsPlugin`] of `row` under the loader, once per
    /// loader context.
    fn follow(&self, ctx: &Ctx, row: &str) {
        let mut mounted = self.inner.mounted.lock().unwrap();
        if mounted.get(row) == Some(&ctx.instance()) {
            return;
        }
        mounted.insert(row.to_owned(), ctx.instance());
        ctx.plugin(SessionRowsPlugin::new(
            row,
            Arc::new(Stale {
                resolver: self.clone(),
                row: row.to_owned(),
            }),
        ));
    }

    /// The names of `row`'s rows to resolve again before they start with
    /// the session whose connection is tagged `session`.
    fn take_stale(&self, row: &str, session: &str) -> HashSet<String> {
        let mut stale: HashSet<String> = self
            .inner
            .offline
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, of)| *of == row)
            .map(|(name, _)| name.clone())
            .collect();
        for (name, (of, by)) in self.inner.described.lock().unwrap().iter() {
            if of == row && by != session {
                stale.insert(name.clone());
            }
        }
        stale
    }

    /// Whether `row` is a local runtime's.
    fn is_local(&self, row: &str) -> bool {
        self.inner
            .runtimes
            .lock()
            .unwrap()
            .iter()
            .any(|runtime| runtime.name() == row)
    }

    /// A row resolved without its session: it waits for it.
    fn offline(&self, name: &str, row: &str, plugin: &str) -> Arc<Resolved> {
        let why = match self.is_local(row) {
            true => format!("the {row} runtime is not running"),
            false => format!("{row} is not connected"),
        };
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
    fn runtimes(&self) -> Vec<RuntimeHandle> {
        self.inner.runtimes.lock().unwrap().clone()
    }

    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            let not_found = || LoaderError::NotFound {
                name: name.to_owned(),
            };
            let Some((row, plugin)) = self.split(name) else {
                return Err(not_found());
            };
            // Whether a row `row` exists is not asked: it may come in the
            // same batch as this one, or later. Until its session is there,
            // this row waits for it (§4.3).
            let ctx = self.loader().and_then(|loader| loader.ctx());
            if let Some(ctx) = &ctx {
                self.follow(ctx, row);
            }
            // Marked first: a session arriving between reading it and
            // marking the name would leave the name unmarked.
            let marked_before = self
                .inner
                .offline
                .lock()
                .unwrap()
                .insert(name.to_owned(), row.to_owned())
                .is_some();
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
                // A local runtime without the plugin: no plugin by this
                // name (installed where the runtime finds it, it would be).
                Described::Missing if self.is_local(row) => {
                    if !marked_before {
                        self.inner.offline.lock().unwrap().remove(name);
                    }
                    return Err(not_found());
                }
                Described::Missing => {
                    self.inner.offline.lock().unwrap().remove(name);
                    self.inner
                        .described
                        .lock()
                        .unwrap()
                        .insert(name.to_owned(), (row.to_owned(), tag));
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
                .insert(name.to_owned(), (row.to_owned(), tag));
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

/// Where a [`SessionRowsPlugin`] learns which rows of its session to
/// resolve again before they may start.
pub(crate) trait RowsSource: Send + Sync + 'static {
    /// The row names whose resolution lacks the declarations of the session
    /// whose connection is tagged `session`. A name is returned again until
    /// it is resolved anew.
    fn take_stale(&self, session: &str) -> HashSet<String>;
}

/// The rows [`SessionResolver`] resolved to one row's session.
struct Stale {
    resolver: SessionResolver,
    row: String,
}

impl RowsSource for Stale {
    fn take_stale(&self, session: &str) -> HashSet<String> {
        self.resolver.take_stale(&self.row, session)
    }
}

/// The second stage of a row's session: once the session is there, resolve
/// its rows again, then provide [`SessionRows`]. Mounted under the loader
/// (by [`SessionResolver`], or a compiled language's runtimes).
pub(crate) struct SessionRowsPlugin {
    row: String,
    name: String,
    source: Arc<dyn RowsSource>,
    injects: [TypeKey; 2],
}

impl SessionRowsPlugin {
    pub(crate) fn new(row: &str, source: Arc<dyn RowsSource>) -> Self {
        Self {
            injects: [session_key(row).erase(), TypeKey::of::<Loader>()],
            name: format!("rows of {row}"),
            row: row.to_owned(),
            source,
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
            let stale = self.source.take_stale(process.connection().tag());
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
                if let Err(error) = owner.provide_at(key, Arc::new(SessionRows { process })) {
                    if !owner.cancellation_token().is_cancelled() {
                        eprintln!("{row}: cannot release its rows: {error}");
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

/// The plugins this loader can resolve, as what a far rutis kernel may run
/// here through a session (`rutis-bridge/session` answers its `run` from
/// it): provided at [`rutis_bridge::catalog_key`] by the loader.
pub struct LoaderCatalog {
    loader: Loader,
}

impl LoaderCatalog {
    pub fn new(loader: Loader) -> Self {
        Self { loader }
    }
}

impl rutis_bridge::PluginCatalog for LoaderCatalog {
    fn find<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<rutis_bridge::Installed>> {
        Box::pin(async move {
            let resolved = self.loader.resolve(name).await.ok()?;
            let factory = match &resolved.scoped {
                Some(scoped) => scoped(&crate::resolver::Build::default()).ok()?,
                None => resolved.factory.clone(),
            };
            Some(rutis_bridge::Installed {
                schema: resolved.schema.clone(),
                version: resolved.meta["version"].as_str().map(str::to_owned),
                factory,
            })
        })
    }
}
