//! Plugins of other languages as loader rows, each language in its own
//! runtime process: one on this machine (`rutis_bridge::runtime::LocalRuntime`,
//! the process started by the local transport and its session over a link),
//! or one elsewhere ([`RuntimePlugin::remote`], its session over a link to
//! it). Either way the rows are the same.
//!
//! The runtime is a rutis plugin the application mounts first, then the
//! loader, then a [`RuntimeRowsPlugin`] per runtime; every row depends on the
//! [`RuntimeRows`] service that plugin provides once the rows' declarations
//! are complete, so rows wait for their runtime and stop when its process
//! goes away. Each row is loaded and disposed on its own.
//!
//! - **Node** (feature `node`, [`RuntimeResolver::node`]): all rows load into
//!   one Cordis Context, so they resolve each other's services natively, as
//!   in dsh. A row names an npm package (resolved from the runtime's anchor
//!   `package.json`, `exports` honored), a subpath of one, or a file
//!   (`file://`, absolute). Its `isolate` and `inject` name Cordis services,
//!   so the resolver handles them itself (`Resolved::foreign_scope`) and
//!   forwards them. Its schemastery `Config` becomes the row's JSON Schema
//!   (`meta.volatile` → `x-volatile`), and a volatile-only change is handed
//!   to Cordis, which commits it in place and emits `loader/volatile-update`
//!   to the plugin, as dsh's loader does.
//! - **Python** (feature `python`, [`RuntimeResolver::modules`]): a row named
//!   `py:<module>` loads that module as a leaf plugin.
//! - **Go** (feature `go`, [`GoResolver`], [`GoRuntimes`]): a row named
//!   `go:<plugin>` runs the plugin in the Go binary that has it, one runtime
//!   per binary, started when a row uses it.
//! - **Runtimes that dial in** ([`RuntimeInstances`]): each connection an
//!   access point admits is an instance of an `instanced` group, whose rows
//!   `<runtime>:<plugin>` run on that connection.
//!
//! Services cross between the rows and the rest of rutis by name, as
//! `dyn HostDispatch` at `host_key(name)`:
//!
//! - a service the plugin injects gates the row in rutis, and the row leases
//!   it into its runtime while it runs, in a scope of the row's own, so the
//!   service sees which row calls it (`rutis_bridge::session::caller_row`).
//!   In a leaf runtime (Python) every injected name does; in Cordis the
//!   names the catalog shares, which are all of them unless it shares only
//!   those registered as shared ([`ServiceCatalog::share_registered_only`]),
//!   the others then left to Cordis;
//! - the services the plugin declares it provides are published from the
//!   row's own fiber, so Rust plugins and other runtimes' rows can inject
//!   them. So are the ones it provides without declaring them, which a
//!   runtime with `rows.announce` announces with the methods they have (a
//!   published Cordis plugin's, say); a Cordis row injecting a service
//!   another row of its runtime provides is served natively, and waits in
//!   rutis only until that service is there. A name another provider has
//!   already fails the later row, naming both.
//!
//! A shared name inside instances ([`ServiceCatalog::register_shared_instance`])
//! is each instance's own: a copy gates, leases and publishes it at
//! `host_key_in(name, instance)`, and in the runtime process it isolates
//! the name under its instance's label, so rows of other instances, which
//! share the process, do not see it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rutis::{BoxFuture, CordisError, Ctx, Effect, Listener, Plugin, PluginFactory, TypeKey};
use rutis_bridge::runtime::{
    row_projection_of, HostLease, Process, Projection, Runtime, RuntimeHandle,
};
use rutis_bridge::session::{host_key, CallerRow, HostDispatch};
use serde_json::{json, Map, Value};

use crate::resolver::{Build, ScopedFactory};
use crate::{
    volatile_key, Loader, LoaderError, Resolved, Resolver, ServiceCatalog, VolatileUpdate,
};

mod connections;
#[cfg(feature = "go")]
mod go;
mod rows;
pub(crate) use connections::{claimed, claimed_names};
pub use connections::{AcceptInstances, RuntimeConnection, RuntimeInstances};
#[cfg(feature = "go")]
pub use go::{
    read_go_manifest, GoBinaries, GoBinary, GoManifest, GoResolver, GoRuntimeInfo, GoRuntimeState,
    GoRuntimes, GoRuntimesHandle, GO_MARKER, PLUGIN_API,
};
pub use rows::{RowsSource, RuntimeRows, RuntimeRowsPlugin};

#[cfg(doc)]
use rutis_bridge::runtime::RuntimePlugin;

/// How row names map to plugins: the part of resolution that differs by
/// runtime. Everything else (asking the runtime what a plugin declares,
/// gating, staleness, the rows themselves) is shared.
enum Naming {
    /// npm package names, subpaths and files, resolved from the runtime's
    /// anchor (Node). A package says when it changed (`version`), so
    /// resolutions are cached.
    #[cfg(feature = "node")]
    Npm,
    /// `<prefix><module>`: names the runtime loads itself (Python modules;
    /// later Swift bundles or plugins compiled into a Go binary). Nothing
    /// says when one changed, so the runtime is asked every time.
    Modules { prefix: String },
}

impl Naming {
    /// The plugin a row name loads, or `None` when it is not this
    /// runtime's.
    fn entry(&self, runtime: &RuntimeHandle, name: &str) -> Option<PathBuf> {
        match self {
            // A remote runtime finds its plugins where it runs: the name
            // goes as it is, and it answers `rows.schema` for it. Files and
            // absolute paths are this machine's, so they are refused.
            #[cfg(feature = "node")]
            Naming::Npm if runtime.is_remote() => {
                let local = name.starts_with("file:") || Path::new(name).is_absolute();
                (!local && !runtime_prefixed(name)).then(|| PathBuf::from(name))
            }
            #[cfg(feature = "node")]
            Naming::Npm => resolve_entry(runtime.anchor(), name),
            Naming::Modules { prefix } => {
                let _ = runtime;
                name.strip_prefix(prefix.as_str())
                    .filter(|module| !module.is_empty())
                    .map(PathBuf::from)
            }
        }
    }

    /// Whether a resolution may be reused until its version changes.
    fn caches(&self) -> bool {
        match self {
            #[cfg(feature = "node")]
            Naming::Npm => true,
            Naming::Modules { .. } => false,
        }
    }

    /// What tells that the plugin at `entry` changed, if anything does.
    fn version(&self, entry: &Path) -> Option<Value> {
        match self {
            #[cfg(feature = "node")]
            Naming::Npm => package_version(entry),
            Naming::Modules { .. } => {
                let _ = entry;
                None
            }
        }
    }
}

/// Whether `name` may name a runtime, and so prefix its rows
/// (`py:weather`, `bun:@acme/weather`, `<remote>:<module>`): two or more of
/// `a-z`, `0-9` and `-`, and not `file`, so that neither a `file:` URL nor a
/// one-letter Windows drive reads as a runtime's row.
pub fn is_runtime_name(name: &str) -> bool {
    name.len() >= 2
        && name != "file"
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `name` names a runtime before a colon: another runtime's row,
/// never an npm name, which has no colon.
#[cfg(feature = "node")]
fn runtime_prefixed(name: &str) -> bool {
    name.split_once(':')
        .is_some_and(|(prefix, _)| is_runtime_name(prefix))
}

pub struct RuntimeResolver {
    runtime: RuntimeHandle,
    naming: Naming,
    catalog: ServiceCatalog,
    resolved: Mutex<HashMap<String, Arc<Resolved>>>,
    /// Names whose resolution lacks the plugin's current declarations
    /// (resolved while the runtime was not running, or their package
    /// changed): [`RuntimeRowsPlugin`] resolves them again. A name leaves
    /// the set only once it is resolved with the runtime, so a refresh that
    /// is cut short is redone by the next one.
    offline: Mutex<HashSet<String>>,
}

impl RuntimeResolver {
    /// Rows of the Node runtime behind `runtime` ([`RuntimePlugin::handle`]):
    /// npm names and files.
    #[cfg(feature = "node")]
    pub fn node(runtime: RuntimeHandle) -> Self {
        Self::with_naming(runtime, Naming::Npm)
    }

    fn with_naming(runtime: RuntimeHandle, naming: Naming) -> Self {
        Self {
            runtime,
            naming,
            catalog: ServiceCatalog::default(),
            resolved: Mutex::new(HashMap::new()),
            offline: Mutex::new(HashSet::new()),
        }
    }

    /// Rows of a runtime that loads plugins by module name, such as a
    /// Python runtime (`rutis_bridge::runtime::LocalRuntime::python`): a row named
    /// `<runtime name>:<module>` (`py:weather.plugin`) loads `<module>`.
    pub fn modules(runtime: RuntimeHandle) -> Self {
        let prefix = format!("{}:", runtime.name());
        Self::with_naming(runtime, Naming::Modules { prefix })
    }

    pub(crate) fn runtime_name(&self) -> &str {
        self.runtime.name()
    }

    /// The catalog the loader uses (`LoaderOptions::catalog`): its shared
    /// names are the injected services rows wait for in rutis.
    pub fn with_catalog(mut self, catalog: &ServiceCatalog) -> Self {
        self.catalog = catalog.clone();
        self
    }

    /// Forget what is known about the row module `name`, for a package whose
    /// content changed without a new `version` (a linked package during
    /// development). Its next resolution asks the runtime again: at once
    /// with `Loader::reload`, or when the runtime restarts, through
    /// [`RuntimeRowsPlugin`].
    ///
    /// Node keeps the module code it has imported, so after a code change
    /// restart the runtime (`FiberView::restart` on its fiber): the new
    /// process imports the new code, and the invalidated rows are resolved
    /// again before they start.
    pub fn invalidate(&self, name: &str) {
        self.resolved.lock().unwrap().remove(name);
        self.offline.lock().unwrap().insert(name.to_owned());
    }

    /// [`RuntimeResolver::invalidate`] every row module resolved so far.
    pub fn invalidate_all(&self) {
        let names: Vec<String> = self
            .resolved
            .lock()
            .unwrap()
            .drain()
            .map(|(name, _)| name)
            .collect();
        self.offline.lock().unwrap().extend(names);
    }

    /// Names whose resolution is out of date: resolved without the runtime,
    /// or whose package version changed since. Their cached resolution is
    /// dropped, so the next resolution asks the runtime again.
    pub(crate) fn take_stale(&self) -> HashSet<String> {
        let mut stale = self.offline.lock().unwrap();
        self.resolved.lock().unwrap().retain(|name, found| {
            let current = Path::new(found.meta["entry"].as_str().unwrap_or_default());
            let recorded = found.meta.get("version").filter(|v| !v.is_null()).cloned();
            let fresh = self.naming.version(current) == recorded;
            if !fresh {
                stale.insert(name.clone());
            }
            fresh
        });
        stale.clone()
    }
}

impl Resolver for RuntimeResolver {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            let Some(entry) = self.naming.entry(&self.runtime, name) else {
                return Err(LoaderError::NotFound {
                    name: name.to_owned(),
                });
            };
            // A runtime that loads by module name is asked every time: a
            // reload must see the module's current declarations, and there
            // is no package version to tell that it changed.
            // Nothing here says when a remote runtime's plugin changed.
            let caches = self.naming.caches() && !self.runtime.is_remote();
            if caches {
                if let Some(found) = self.resolved.lock().unwrap().get(name) {
                    return Ok(found.clone());
                }
            }
            // The declarations need Node. Without a running runtime the row
            // still resolves (it waits for the runtime like any dependency),
            // and says why it has no schema; it is not cached, and
            // `RuntimeRowsPlugin` resolves it again once the runtime is up,
            // before the row may start.
            let Some(process) = self.runtime.ready().await else {
                self.offline.lock().unwrap().insert(name.to_owned());
                let module = JsModule {
                    name: name.to_owned(),
                    runtime: self.runtime.name().to_owned(),
                    entry: entry.clone(),
                    gated: Vec::new(),
                    provides: Map::new(),
                    catalog: self.catalog.clone(),
                };
                return Ok(Arc::new(Resolved {
                    factory: Arc::new(JsFactory::new(
                        name,
                        self.runtime.name(),
                        entry.clone(),
                        Vec::new(),
                        Map::new(),
                    )),
                    schema: None,
                    meta: json!({
                        "source": "runtime",
                        "entry": entry,
                        "schema": "unavailable: the Cordis runtime is not running",
                    }),
                    foreign_scope: true,
                    scoped: Some(module.scoped()),
                    declares: Some(unknown(format!(
                        "the {} runtime is not running",
                        self.runtime.name()
                    ))),
                }));
            };
            let described = process
                .describe_row(&entry)
                .await
                .map_err(|error| match error {
                    // The runtime looked and found no such plugin.
                    rutis_bridge::session::Error::Remote { name: kind, .. }
                        if kind == "NotFound" =>
                    {
                        LoaderError::NotFound {
                            name: name.to_owned(),
                        }
                    }
                    error => LoaderError::Resolve {
                        name: name.to_owned(),
                        message: error.to_string(),
                    },
                })?;
            // A leaf runtime has no dependency resolution of its own: every
            // service its plugins inject waits in rutis. In Cordis, only the
            // shared names do; the others resolve natively.
            let leaf = self.runtime.supports("leaf");
            let gated: Vec<String> = described
                .inject
                .iter()
                .filter(|name| leaf || self.catalog.is_shared(name))
                .cloned()
                .collect();
            self.offline.lock().unwrap().remove(name);
            let resolved = Arc::new(leaf_resolved(
                name,
                self.runtime.name(),
                entry.clone(),
                gated,
                &described,
                &self.catalog,
                json!({
                    "source": "runtime",
                    "entry": entry,
                    // The package's version where this side can read it,
                    // else what the runtime reported.
                    "version": (!self.runtime.is_remote())
                        .then(|| self.naming.version(&entry))
                        .flatten()
                        .or_else(|| described.version.clone().map(Value::String)),
                    "inject": described.inject,
                    "provides": described.provides,
                    // A Cordis row: its dependencies on the Cordis rows of
                    // this runtime resolve in Cordis.
                    "cordis": described.cordis,
                }),
            ));
            if caches {
                self.resolved
                    .lock()
                    .unwrap()
                    .insert(name.to_owned(), resolved.clone());
            }
            Ok(resolved)
        })
    }
}

/// Declarations not known, and why.
pub(crate) fn unknown(why: String) -> crate::Declarations {
    Arc::new(move |_: &Value| Err(why.clone()))
}

/// The resolution of a row `name` whose plugin `entry` runs in the runtime
/// named `runtime` and declares `described`; `gated` are the injected names
/// rutis gates it on.
pub(crate) fn leaf_resolved(
    name: &str,
    runtime: &str,
    entry: PathBuf,
    gated: Vec<String>,
    described: &rutis_bridge::runtime::RowSchema,
    catalog: &ServiceCatalog,
    meta: Value,
) -> Resolved {
    let declared = crate::Declared {
        requires: gated.clone(),
        provides: described.provides.keys().cloned().collect(),
        announces: described.announces,
    };
    let module = JsModule {
        name: name.to_owned(),
        runtime: runtime.to_owned(),
        entry: entry.clone(),
        gated: gated.clone(),
        provides: described.provides.clone(),
        catalog: catalog.clone(),
    };
    Resolved {
        factory: Arc::new(JsFactory::new(
            name,
            runtime,
            entry,
            gated,
            described.provides.clone(),
        )),
        schema: described.config.clone(),
        meta,
        foreign_scope: true,
        scoped: Some(module.scoped()),
        declares: Some(Arc::new(move |_: &Value| Ok(declared.clone()))),
    }
}

/// What a row module declares, from which each copy's factory is built
/// with the keys of the instances it runs in.
#[derive(Clone)]
struct JsModule {
    name: String,
    runtime: String,
    entry: PathBuf,
    /// The injected services that gate the row in rutis.
    gated: Vec<String>,
    provides: Map<String, Value>,
    catalog: ServiceCatalog,
}

impl JsModule {
    fn scoped(self) -> ScopedFactory {
        Arc::new(move |build: &Build| {
            let factory: Arc<dyn PluginFactory<Value>> = Arc::new(self.factory(build)?);
            Ok(factory)
        })
    }

    /// The factory of a copy running in `build`'s instances.
    fn factory(&self, build: &Build) -> Result<JsFactory, CordisError> {
        // A shared name resolves through the catalog (inside instances, to
        // the instance's key); any other name a leaf runtime gates on is
        // shared by name.
        let key = |name: &str| -> Result<TypeKey, CordisError> {
            match self.catalog.is_shared(name) {
                true => self.catalog.key_in(name, build).map_err(failed),
                false => Ok(host_key(name)),
            }
        };
        let keyed = |names: &mut dyn Iterator<Item = &String>| {
            names
                .map(|name| Ok((name.clone(), key(name)?)))
                .collect::<Result<Vec<_>, CordisError>>()
        };
        let scopes = self
            .catalog
            .instance_names(build)
            .into_iter()
            .filter(|(name, _)| self.catalog.is_shared(name))
            .map(|(name, instance)| (name, format!("rutis-loader/instance/{instance}")))
            .collect();
        // A service the row announces is keyed as a declared one would be.
        let (catalog, build) = (self.catalog.clone(), build.clone());
        let announced: KeyOf = Arc::new(move |name| match catalog.is_shared(name) {
            true => catalog
                .key_in(name, &build)
                .unwrap_or_else(|_| host_key(name)),
            false => host_key(name),
        });
        Ok(JsFactory::with_keys(
            &self.name,
            RowRuntime::Named(self.runtime.clone()),
            self.entry.clone(),
            keyed(&mut self.gated.iter())?,
            self.provides.clone(),
            keyed(&mut self.provides.keys())?,
            scopes,
        )
        .announcing(announced))
    }
}

/// Where a row's runtime comes from.
#[derive(Clone)]
pub(crate) enum RowRuntime {
    /// The runtime named so, through its [`RuntimeRows`]: rows wait for
    /// it and stop when it goes.
    Named(String),
    /// One runtime connection's process ([`RuntimeInstances`]): the row
    /// runs in that connection's instance, which ends with it.
    Connection(Arc<Process>),
}

/// The key a service a row announces is published under, by name.
pub(crate) type KeyOf = Arc<dyn Fn(&str) -> TypeKey + Send + Sync>;

struct JsFactory {
    name: String,
    runtime: RowRuntime,
    entry: PathBuf,
    /// The injected services that gate the row in rutis, with their keys.
    gated: Vec<(String, TypeKey)>,
    provides: Map<String, Value>,
    /// The key each provided service is published under.
    provided: Vec<(String, TypeKey)>,
    /// Shared names inside the copy's instances, with their instance's
    /// label: isolated in the runtime so other instances do not see them.
    scopes: Vec<(String, String)>,
    injects: Vec<TypeKey>,
    /// The key of each service the row announces.
    announced: KeyOf,
    /// Why a running row was refused a service it provided after loading,
    /// by its fiber: the restart that follows fails with it.
    refused: Refused,
}

/// See [`JsFactory::refused`].
type Refused = Arc<Mutex<HashMap<rutis::InstanceId, String>>>;

impl JsFactory {
    /// The factory of a copy outside instances.
    fn new(
        name: &str,
        runtime: &str,
        entry: PathBuf,
        gated: Vec<String>,
        provides: Map<String, Value>,
    ) -> Self {
        let gated = gated
            .into_iter()
            .map(|name| {
                let key = host_key(&name);
                (name, key)
            })
            .collect();
        let provided = provides
            .keys()
            .map(|name| (name.clone(), host_key(name)))
            .collect();
        Self::with_keys(
            name,
            RowRuntime::Named(runtime.to_owned()),
            entry,
            gated,
            provides,
            provided,
            Vec::new(),
        )
    }

    pub(crate) fn with_keys(
        name: &str,
        runtime: RowRuntime,
        entry: PathBuf,
        gated: Vec<(String, TypeKey)>,
        provides: Map<String, Value>,
        provided: Vec<(String, TypeKey)>,
        scopes: Vec<(String, String)>,
    ) -> Self {
        let rows = match &runtime {
            RowRuntime::Named(runtime) => Some(RuntimeRows::key(runtime)),
            RowRuntime::Connection(_) => None,
        };
        let injects = rows
            .into_iter()
            .chain(gated.iter().map(|(_, key)| key.clone()))
            .collect();
        Self {
            name: name.to_owned(),
            runtime,
            entry,
            gated,
            provides,
            provided,
            scopes,
            injects,
            announced: Arc::new(host_key),
            refused: Refused::default(),
        }
    }

    /// Publish the services a row announces under `key(name)`.
    pub(crate) fn announcing(mut self, key: KeyOf) -> Self {
        self.announced = key;
        self
    }
}

impl PluginFactory<Value> for JsFactory {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn build(&self, config: &Value) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(JsRow {
            name: self.name.clone(),
            runtime: self.runtime.clone(),
            entry: self.entry.clone(),
            config: config.clone(),
            gated: self.gated.clone(),
            provides: self.provides.clone(),
            provided: self.provided.clone(),
            scopes: self.scopes.clone(),
            injects: self.injects.clone(),
            announced: self.announced.clone(),
            refused: self.refused.clone(),
        }))
    }
}

/// One generation of a JavaScript row: loaded on apply, disposed on cleanup.
struct JsRow {
    name: String,
    runtime: RowRuntime,
    entry: PathBuf,
    config: Value,
    gated: Vec<(String, TypeKey)>,
    provides: Map<String, Value>,
    provided: Vec<(String, TypeKey)>,
    scopes: Vec<(String, String)>,
    injects: Vec<TypeKey>,
    announced: KeyOf,
    refused: Refused,
}

fn failed(error: impl std::fmt::Display) -> CordisError {
    CordisError::PluginFailed(error.to_string().into())
}

impl Plugin for JsRow {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            // Restarted because it provided a service another provider has:
            // it fails as it would have had it provided it while loading.
            if let Some(refused) = self.refused.lock().unwrap().remove(&ctx.instance()) {
                return Err(failed(refused));
            }
            let (process, runtime) = match &self.runtime {
                RowRuntime::Named(name) => {
                    let runtime = ctx
                        .require_as::<RuntimeRows>(RuntimeRows::key(name))?
                        .runtime()
                        .clone();
                    (runtime.process().clone(), Some(runtime))
                }
                RowRuntime::Connection(process) => (process.clone(), None),
            };
            // Every lease taken is released: by the cleanup once the row
            // runs, here when it does not get that far. The kernel waits for
            // apply to finish, so this frame gets to release them; only a
            // panic in it leaves them raised, in a runtime whose state is
            // then unknown anyway.
            let mut leases: Vec<HostLease> = Vec::new();
            let started = self
                .start(ctx, &process, runtime.as_deref(), &mut leases)
                .await;
            let (key, projection) = match started {
                Ok(started) => started,
                Err(error) => {
                    release(leases).await;
                    return Err(error);
                }
            };
            Ok(Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    // The row's services go first, and their users stop,
                    // before the plugin that provides them is unloaded.
                    projection.withdraw().await;
                    let unloaded = process.unload_row(&key).await;
                    // After the unload: the plugin never sees a service it
                    // injects go away before it does.
                    release(leases).await;
                    match unloaded {
                        // The process is gone, and the row with it.
                        Ok(()) | Err(rutis_bridge::session::Error::Transport(_)) => Ok(()),
                        Err(e) => Err(failed(e)),
                    }
                })
            })))
        })
    }
}

impl JsRow {
    /// Lease the shared services the plugin injects into `leases`, then
    /// load it into the runtime. Returns its key there and the projection
    /// of its services; on failure nothing of the load remains, and the
    /// caller releases the leases.
    async fn start(
        &self,
        ctx: &Ctx,
        process: &Arc<Process>,
        runtime: Option<&Runtime>,
        leases: &mut Vec<HostLease>,
    ) -> Result<(String, Arc<Projection>), CordisError> {
        let row = ctx
            .get::<Loader>()
            .and_then(|loader| loader.row(ctx.instance()));
        let (id, mut isolate, inject) = match row {
            Some(row) => (row.id, row.isolate, row.inject),
            None => (self.name.clone(), Vec::new(), Vec::new()),
        };
        // What its config isolates stays in the row's scope: a service it
        // provides there is not rutis's to publish.
        let private: HashSet<String> = isolate.iter().map(|(name, _)| name.clone()).collect();
        // Shared names inside the row's instances are isolated under the
        // instance's label, unless its config isolates them already.
        for (name, label) in &self.scopes {
            if !isolate.iter().any(|(isolated, _)| isolated == name) {
                isolate.push((name.clone(), label.clone()));
            }
        }
        let label_of = |isolate: &[(String, String)], name: &str| {
            isolate
                .iter()
                .find(|(isolated, _)| isolated == name)
                .map(|(_, label)| label.clone())
        };
        // The fiber identity and a generation number key the row on the
        // Cordis side: a fiber keeps its identity across restarts, and a
        // late `rows.ended` of an earlier generation must not end this one.
        static GENERATION: AtomicU64 = AtomicU64::new(0);
        let key = format!(
            "{}.{}",
            ctx.instance(),
            GENERATION.fetch_add(1, Ordering::Relaxed)
        );
        // Who calls the services it leases (`caller_row`).
        let caller = CallerRow::new(id.clone(), key.clone(), ctx.clone());
        // The shared services the plugin injects, registered in Cordis for
        // as long as the row runs, each in a scope of the row's own (inside
        // the one it isolates the name in), so their calls name the row.
        // One served by a row of this same process is already there
        // natively.
        for (name, key) in &self.gated {
            let dispatch = ctx.require_as::<dyn HostDispatch>(key.clone())?;
            if dispatch
                .origin()
                .is_some_and(|origin| origin == process.connection().tag())
            {
                continue;
            }
            let label = label_of(&isolate, name);
            let lease = process
                .lease_host_for(
                    name,
                    label.as_deref(),
                    &caller,
                    dispatch,
                    runtime.and_then(|runtime| runtime.host_methods(name)),
                )
                .await
                .map_err(failed)?;
            if let Some(own) = lease.label().filter(|own| Some(*own) != label.as_deref()) {
                isolate.retain(|(isolated, _)| isolated != name);
                isolate.push((name.clone(), own.to_owned()));
            }
            leases.push(lease);
        }
        // The row's services are published from this fiber, so they go when
        // it does: the declared ones, and those it announces.
        let provided = self.provided.clone();
        let announced = self.announced.clone();
        let projection = row_projection_of(
            &format!("row {id}"),
            &self.provides,
            move |name| {
                provided
                    .iter()
                    .find(|(provided, _)| provided == name)
                    .map(|(_, key)| key.clone())
                    .unwrap_or_else(|| announced(name))
            },
            move |name| private.contains(name),
        );
        projection.attach(ctx, process.clone())?;
        // A plugin that disposes itself there ends this fiber, as a Rust
        // plugin disposing itself: the loader's monitor takes it from there.
        let ending = ctx.clone();
        process.on_row_ended(&key, move || {
            // Already unloading or restarted: nothing left to end.
            let _ = ending.dispose_self();
        });
        if let Err(error) = process
            .load_row_exporting(
                &key,
                &self.entry,
                self.config.clone(),
                &isolate,
                &inject,
                &self.provides,
                projection.clone(),
            )
            .await
        {
            projection.close();
            return Err(failed(error));
        }
        // A service another provider had first: this row, the later one,
        // fails.
        if let Some(refused) = projection.take_refused() {
            projection.withdraw().await;
            let _ = process.unload_row(&key).await;
            return Err(failed(refused));
        }
        // Volatile-only changes go to Cordis, which commits them in place.
        let listening = ctx.events().on(
            ctx,
            &volatile_key(ctx),
            Forward {
                key: key.clone(),
                process: process.clone(),
            },
        );
        if let Err(error) = listening {
            // The plugin is loaded, but no cleanup will be registered for it:
            // undo the load here.
            projection.withdraw().await;
            let _ = process.unload_row(&key).await;
            return Err(error);
        }
        // One it provides afterwards fails it the same way: the fiber
        // restarts, and that load fails with why.
        let refused = self.refused.clone();
        let restarting = ctx.clone();
        let tasks = tokio::runtime::Handle::current();
        projection.on_refused(move |why| {
            let generation = restarting.cancellation_token();
            if generation.is_cancelled() {
                return; // already unloading or restarted
            }
            refused
                .lock()
                .unwrap()
                .entry(restarting.instance())
                .or_insert(why);
            let ctx = restarting.clone();
            let refused = refused.clone();
            tasks.spawn(async move {
                let instance = ctx.instance();
                let view = ctx
                    .diagnostics()
                    .plugins
                    .into_iter()
                    .find(|plugin| plugin.instance == instance)
                    .and_then(|plugin| ctx.view(plugin.id));
                match view {
                    Some(view) if !generation.is_cancelled() => {
                        let _ = view.restart().await;
                    }
                    // Gone or restarting anyway: nothing to fail.
                    _ => {
                        refused.lock().unwrap().remove(&instance);
                    }
                }
            });
        });
        Ok((key, projection))
    }
}

/// Give the leases back; a process that is gone has nothing left to
/// withdraw.
async fn release(leases: Vec<HostLease>) {
    for lease in leases {
        let _ = lease.release().await;
    }
}

struct Forward {
    key: String,
    process: Arc<Process>,
}

impl Listener<VolatileUpdate> for Forward {
    fn call<'a>(
        &'a self,
        _ctx: &'a Ctx,
        update: &'a VolatileUpdate,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        Box::pin(async move {
            self.process
                .update_row(&self.key, update.config.clone())
                .await
                .map_err(failed)?;
            Ok(None)
        })
    }
}

#[cfg(feature = "node")]
/// The `version` of the package a plugin file belongs to (the nearest
/// `package.json` above it), if it has one. A module name (a relative path)
/// has none.
fn package_version(entry: &Path) -> Option<Value> {
    if entry.is_relative() {
        return None;
    }
    let manifest = entry
        .ancestors()
        .skip(1)
        .map(|dir| dir.join("package.json"))
        .find(|path| path.exists())?;
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
    manifest.get("version").cloned()
}

#[cfg(test)]
mod stale_tests {
    use super::*;

    /// A remote Node runtime takes npm names, not another runtime's rows.
    #[cfg(feature = "node")]
    #[test]
    fn runtime_prefixes_are_not_npm_names() {
        for name in [
            "py:weather",
            "bun:@acme/weather",
            "bun:./plugin.ts",
            "gpu-1:model",
        ] {
            assert!(runtime_prefixed(name), "{name}");
        }
        for name in [
            "@acme/weather",
            "weather/sub",
            "file:///p.mjs",
            "C:\\p.mjs",
            "./a:b.ts",
        ] {
            assert!(!runtime_prefixed(name), "{name}");
        }
    }

    fn cached(entry: &Path, version: Option<Value>) -> Arc<Resolved> {
        Arc::new(Resolved {
            factory: Arc::new(JsFactory::new(
                "x",
                "t",
                entry.to_owned(),
                Vec::new(),
                Map::new(),
            )),
            schema: None,
            meta: json!({ "entry": entry, "version": version }),
            foreign_scope: true,
            scoped: None,
            declares: None,
        })
    }

    #[cfg(feature = "node")]
    #[test]
    fn stale_rows_are_those_without_current_declarations() {
        let dir = tempfile::tempdir().unwrap();
        let versioned = dir.path().join("pkg/index.mjs");
        std::fs::create_dir_all(versioned.parent().unwrap()).unwrap();
        std::fs::write(
            dir.path().join("pkg/package.json"),
            r#"{ "version": "2.0.0" }"#,
        )
        .unwrap();
        let loose = Path::new("/nowhere/loose.mjs");
        let runtime = rutis_bridge::runtime::RuntimePlugin::session("t", dir.path());
        let resolver = RuntimeResolver::node(runtime.handle());
        {
            let mut resolved = resolver.resolved.lock().unwrap();
            // No package version, recorded as null: unchanged.
            resolved.insert("loose".into(), cached(loose, None));
            resolved.insert("same".into(), cached(&versioned, Some(json!("2.0.0"))));
            resolved.insert("older".into(), cached(&versioned, Some(json!("1.0.0"))));
        }
        resolver.offline.lock().unwrap().insert("offline".into());
        let stale = resolver.take_stale();
        assert_eq!(
            stale,
            HashSet::from(["older".to_owned(), "offline".to_owned()])
        );
        assert!(!resolver.resolved.lock().unwrap().contains_key("older"));
        // A refresh that never finished leaves them stale for the next one.
        assert_eq!(resolver.take_stale(), stale);
    }

    #[test]
    fn module_rows_have_no_version_and_are_never_cached() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = rutis_bridge::runtime::RuntimePlugin::session("py", dir.path());
        let naming = Naming::Modules {
            prefix: "py:".into(),
        };
        assert_eq!(
            naming.entry(&runtime.handle(), "py:weather.plugin"),
            Some(PathBuf::from("weather.plugin"))
        );
        assert_eq!(naming.entry(&runtime.handle(), "py:"), None);
        assert_eq!(naming.entry(&runtime.handle(), "weather"), None);
        assert!(!naming.caches());
        assert_eq!(naming.version(Path::new("weather.plugin")), None);
    }

    #[test]
    fn invalidated_rows_are_stale_until_resolved_again() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("p.mjs");
        let runtime = rutis_bridge::runtime::RuntimePlugin::session("t", dir.path());
        let resolver = RuntimeResolver::modules(runtime.handle());
        {
            let mut resolved = resolver.resolved.lock().unwrap();
            resolved.insert("a".into(), cached(&entry, None));
            resolved.insert("b".into(), cached(&entry, None));
        }
        assert!(resolver.take_stale().is_empty());
        resolver.invalidate("a");
        assert_eq!(resolver.take_stale(), HashSet::from(["a".to_owned()]));
        resolver.invalidate_all();
        assert_eq!(
            resolver.take_stale(),
            HashSet::from(["a".to_owned(), "b".to_owned()])
        );
        assert!(resolver.resolved.lock().unwrap().is_empty());
    }
}

#[cfg(feature = "node")]
mod npm {
    use super::*;

    // ── Node's package resolution ───────────────────────────────────

    fn package_dir(anchor: &Path, package: &str) -> Option<PathBuf> {
        let start = anchor.parent()?;
        start
            .ancestors()
            .filter(|dir| dir.file_name().is_none_or(|n| n != "node_modules"))
            .map(|dir| dir.join("node_modules").join(package))
            .find(|candidate| candidate.join("package.json").exists())
    }

    /// The `exports` target for `subpath` under the `node`/`import`/`default`
    /// conditions.
    fn export_target(exports: &Value, subpath: &str) -> Option<String> {
        fn pick(value: &Value) -> Option<String> {
            match value {
                Value::String(target) => Some(target.clone()),
                Value::Object(conditions) => ["node", "import", "default"]
                    .iter()
                    .find_map(|c| conditions.get(*c).and_then(pick)),
                Value::Array(options) => options.iter().find_map(pick),
                _ => None,
            }
        }
        match exports {
            Value::String(_) | Value::Array(_) if subpath == "." => pick(exports),
            Value::Object(map) if map.keys().any(|k| k.starts_with('.')) => {
                map.get(subpath).and_then(pick)
            }
            Value::Object(_) if subpath == "." => pick(exports),
            _ => None,
        }
    }

    /// The module file a row name loads, or `None` when it is not a resolvable
    /// JavaScript plugin name.
    pub fn resolve_entry(anchor: &Path, name: &str) -> Option<PathBuf> {
        if name.starts_with("file:") {
            // A URL: percent-decoded, query and fragment dropped, as Node's
            // fileURLToPath; a remote host is not a local file.
            return url::Url::parse(name).ok()?.to_file_path().ok();
        }
        // As in Node: `/…` is absolute on Windows too (on the current drive).
        if name.starts_with('/') || Path::new(name).is_absolute() {
            return Some(PathBuf::from(name));
        }
        let mut parts = name.splitn(if name.starts_with('@') { 3 } else { 2 }, '/');
        let package = if name.starts_with('@') {
            format!("{}/{}", parts.next()?, parts.next()?)
        } else {
            parts.next()?.to_owned()
        };
        if package.is_empty() || package.contains(':') {
            return None;
        }
        let subpath = match parts.next() {
            Some(rest) => format!("./{rest}"),
            None => ".".to_owned(),
        };
        let dir = package_dir(anchor, &package)?;
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("package.json")).ok()?).ok()?;
        let relative = match manifest.get("exports") {
            Some(exports) => export_target(exports, &subpath)?,
            None if subpath == "." => manifest
                .get("module")
                .or_else(|| manifest.get("main"))
                .and_then(Value::as_str)
                .unwrap_or("index.js")
                .to_owned(),
            None => subpath,
        };
        Some(dir.join(relative.trim_start_matches("./")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn resolves_like_node() {
            let dir = tempfile::tempdir().unwrap();
            let anchor = dir.path().join("app/package.json");
            let modules = dir.path().join("node_modules");
            let write = |path: PathBuf, text: &str| {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            };
            write(anchor.clone(), "{}");
            write(
                modules.join("@s/a/package.json"),
                r#"{ "exports": { ".": { "require": "./c.cjs", "import": "./lib/index.js" }, "./tools": "./lib/tools.js" } }"#,
            );
            write(modules.join("b/package.json"), r#"{ "main": "main.js" }"#);
            write(
                modules.join("c/package.json"),
                r#"{ "exports": "./only.mjs" }"#,
            );
            assert_eq!(
                resolve_entry(&anchor, "@s/a"),
                Some(modules.join("@s/a/lib/index.js"))
            );
            assert_eq!(
                resolve_entry(&anchor, "@s/a/tools"),
                Some(modules.join("@s/a/lib/tools.js"))
            );
            assert_eq!(resolve_entry(&anchor, "@s/a/hidden"), None, "not exported");
            assert_eq!(resolve_entry(&anchor, "b"), Some(modules.join("b/main.js")));
            assert_eq!(
                resolve_entry(&anchor, "c"),
                Some(modules.join("c/only.mjs"))
            );
            assert_eq!(resolve_entry(&anchor, "missing"), None);
            assert_eq!(resolve_entry(&anchor, "dylib:x"), None);
            assert_eq!(
                resolve_entry(&anchor, "/abs/p.mjs"),
                Some(PathBuf::from("/abs/p.mjs"))
            );
        }

        #[test]
        #[cfg(unix)]
        fn file_urls_are_decoded() {
            let cases = [
                ("file:///abs/p.mjs", "/abs/p.mjs"),
                ("file:///my%20plugins/p.mjs", "/my plugins/p.mjs"),
                ("file:///%E6%8F%92%E4%BB%B6/p.mjs", "/插件/p.mjs"),
                ("file:///a%23b/p.mjs", "/a#b/p.mjs"),
                ("file:///100%25/p.mjs", "/100%/p.mjs"),
                ("file:///p.mjs#fragment", "/p.mjs"),
                ("file:///p.mjs?v=2", "/p.mjs"),
                ("file://localhost/p.mjs", "/p.mjs"),
            ];
            let anchor = Path::new("/nowhere/package.json");
            for (url, path) in cases {
                assert_eq!(
                    resolve_entry(anchor, url),
                    Some(PathBuf::from(path)),
                    "{url}"
                );
            }
            assert_eq!(resolve_entry(anchor, "file://server/share/p.mjs"), None);
        }

        #[test]
        #[cfg(windows)]
        fn file_urls_are_decoded() {
            let cases = [
                ("file:///C:/abs/p.mjs", r"C:\abs\p.mjs"),
                ("file:///C:/my%20plugins/p.mjs", r"C:\my plugins\p.mjs"),
                ("file:///C:/%E6%8F%92%E4%BB%B6/p.mjs", r"C:\插件\p.mjs"),
                ("file:///C:/p.mjs#fragment", r"C:\p.mjs"),
                ("file:///C:/p.mjs?v=2", r"C:\p.mjs"),
                ("file://server/share/p.mjs", r"\\server\share\p.mjs"),
            ];
            let anchor = Path::new(r"C:\nowhere\package.json");
            for (url, path) in cases {
                assert_eq!(
                    resolve_entry(anchor, url),
                    Some(PathBuf::from(path)),
                    "{url}"
                );
            }
        }
    }
}
#[cfg(feature = "node")]
pub use npm::resolve_entry;
