//! Plugins of other languages as loader rows, each language in its own
//! runtime process, the far end of a session (`rutis_bridge::Session`):
//! one on this machine (rutis-host's `runtimes.*`, the row named after the
//! runtime) or elsewhere (a connection row).
//!
//! Every row depends on the [`SessionRows`] of its session's row, provided
//! once the session is there and the rows' declarations are complete, so
//! rows wait for their runtime and stop when its process goes away. Each
//! row is loaded and disposed on its own.
//!
//! - [`SessionResolver`]: a row `<row>:<plugin>` runs `<plugin>` on the
//!   session of the row `row`: `py:<module>` loads a Python module as a
//!   leaf plugin, `bun:<module>` a Bun one. With [`SessionResolver::npm`],
//!   the names no runtime prefixes (an npm package, a subpath of one, a
//!   file) run in the Node runtime, where all rows load into one Cordis
//!   Context, so they resolve each other's services natively, as in dsh.
//!   Their `isolate` and `inject` name Cordis services, so the resolver
//!   handles them itself (`Resolved::foreign_scope`) and forwards them. A
//!   schemastery `Config` becomes the row's JSON Schema (`meta.volatile` →
//!   `x-volatile`), and a volatile-only change is handed to Cordis, which
//!   commits it in place and emits `loader/volatile-update` to the plugin,
//!   as dsh's loader does.
//! - **Go** (feature `go`, [`BinaryResolver`](binary::BinaryResolver)`<`[`Go`](go::Go)`>`,
//!   [`BinaryRuntimes`](binary::BinaryRuntimes)`<Go>`): a row named
//!   `go:<plugin>` runs the plugin in the Go binary that has it, one runtime
//!   per binary, started when a row uses it. What compiled languages share
//!   (feature `binary`, which theirs enable) is [`BinaryResolver`](binary::BinaryResolver) and
//!   [`BinaryRuntimes`](binary::BinaryRuntimes); a language names itself with a
//!   [`BinaryLanguage`](binary::BinaryLanguage).
//! - **Swift** (feature `swift`, `BinaryResolver<`[`Swift`](swift::Swift)`>`,
//!   `BinaryRuntimes<Swift>`): a
//!   row named `swift:<plugin>` runs the plugin in the Swift binary that has
//!   it, as Go does.
//!
//! Services cross between the rows and the rest of rutis by name, as
//! `dyn HostDispatch` at `host_key(name)`:
//!
//! - a service the plugin injects gates the row in rutis, and the row leases
//!   it into its runtime while it runs, in a scope of the row's own, so the
//!   service sees which row calls it (`rutis_bridge::session::caller_row`).
//!   In a leaf runtime (Python) every injected name does; in Cordis the
//!   names the catalog shares, which are all of them;
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, PluginFactory, TypeKey};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::{
    row_projection_of, ForeignPlugin, HostLease, PluginMounter, Process, Projection,
};
use rutis_bridge::session::Site;
use rutis_bridge::session::{host_key, CallerRow, HostDispatch};
use serde_json::{json, Map, Value};

use crate::resolver::Resolved;
use crate::resolver::{Build, ScopedFactory};
use crate::volatile::{volatile_key, VolatileUpdate};
use crate::{Loader, ServiceCatalog};

/// Runtimes started from a binary that carries its own manifest.
#[cfg(feature = "binary")]
pub mod binary;
mod discover;
/// Go runtimes: binaries built with the Go SDK.
#[cfg(feature = "go")]
pub mod go;
mod sessions;
/// Swift runtimes: binaries built with the Swift SDK.
#[cfg(feature = "swift")]
pub mod swift;
pub use sessions::{LoaderCatalog, SessionResolver, SessionRows};

use rutis::internal::CtxInternal;

/// Whether `name` may name a runtime, and so prefix its rows
/// (`py:weather`, `bun:@acme/weather`): two or more of
/// `a-z`, `0-9` and `-`, and not `file`, so that neither a `file:` URL nor a
/// one-letter Windows drive reads as a runtime's row.
#[doc(hidden)]
pub fn is_runtime_name(name: &str) -> bool {
    name.len() >= 2
        && name != "file"
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Declarations not known, and why.
pub(crate) fn unknown(why: String) -> crate::resolver::Declarations {
    Arc::new(move |_: &Value| Err(why.clone()))
}

/// The resolution of a row `name` whose runtime does not contain its plugin
/// `entry`: the row fails with `why`, alone, and an edit adding it is not
/// refused.
pub(crate) fn unavailable_resolved(
    name: &str,
    entry: &Path,
    why: crate::error::Unavailable,
) -> Resolved {
    Resolved {
        factory: Arc::new(UnavailableFactory {
            name: name.to_owned(),
            why: why.clone(),
        }),
        schema: None,
        meta: json!({ "source": "runtime", "entry": entry }),
        foreign_scope: true,
        scoped: None,
        declares: Some(unknown(why.0)),
    }
}

/// The factory of a row whose plugin its runtime does not contain.
struct UnavailableFactory {
    name: String,
    why: crate::error::Unavailable,
}

impl PluginFactory<Value> for UnavailableFactory {
    fn name(&self) -> &str {
        &self.name
    }

    fn validate_config(&self, _config: &Value) -> Result<(), Error> {
        Err(self.why.clone().into_error())
    }

    fn build(&self, _config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Err(self.why.clone().into_error())
    }
}

/// The resolution of a row `name` whose plugin `entry` runs on `runtime`
/// and declares `described`; `gated` are the injected names rutis gates it
/// on.
pub(crate) fn leaf_resolved_on(
    name: &str,
    runtime: RowRuntime,
    entry: PathBuf,
    gated: Vec<String>,
    described: &rutis_bridge::runtime::RowSchema,
    catalog: &ServiceCatalog,
    meta: Value,
) -> Resolved {
    let declared = crate::resolver::Declared {
        requires: gated.clone(),
        provides: described.provides.keys().cloned().collect(),
        announces: described.announces,
    };
    let module = JsModule {
        name: name.to_owned(),
        runtime: runtime.clone(),
        entry: entry.clone(),
        gated: gated.clone(),
        provides: described.provides.clone(),
        catalog: catalog.clone(),
    };
    Resolved {
        factory: Arc::new(JsFactory::on(
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
pub(crate) struct JsModule {
    pub(crate) name: String,
    pub(crate) runtime: RowRuntime,
    pub(crate) entry: PathBuf,
    /// The injected services that gate the row in rutis.
    pub(crate) gated: Vec<String>,
    pub(crate) provides: Map<String, Value>,
    pub(crate) catalog: ServiceCatalog,
}

impl JsModule {
    pub(crate) fn scoped(self) -> ScopedFactory {
        Arc::new(move |build: &Build| {
            let factory: Arc<dyn PluginFactory<Value>> = Arc::new(self.factory(build)?);
            Ok(factory)
        })
    }

    /// The factory of a copy running in `build`'s instances.
    fn factory(&self, build: &Build) -> Result<JsFactory, Error> {
        // A shared name resolves through the catalog (inside instances, to
        // the instance's key); any other name a leaf runtime gates on is
        // shared by name.
        let key = |name: &str| -> Result<TypeKey, Error> {
            match self.catalog.is_shared(name) {
                true => self.catalog.key_in(name, build).map_err(failed),
                false => Ok(host_key(name).erase()),
            }
        };
        let keyed = |names: &mut dyn Iterator<Item = &String>| {
            names
                .map(|name| Ok((name.clone(), key(name)?)))
                .collect::<Result<Vec<_>, Error>>()
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
                .unwrap_or_else(|_| host_key(name).erase()),
            false => host_key(name).erase(),
        });
        Ok(JsFactory::with_keys(
            &self.name,
            self.runtime.clone(),
            RowEntry::Module(self.entry.clone()),
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
    /// One process, for a copy of a plugin definition it handed over.
    Connection(Arc<Process>),
    /// The session of the row so named (a connection row's, a local
    /// runtime's), through its [`SessionRows`]: rows wait for it and stop
    /// when it goes.
    Session(String),
}

/// What a row loads: a module, or one copy of a plugin definition the
/// runtime handed over (`docs/design-plugin-values-2026-10-25.md` §5).
#[derive(Clone)]
pub(crate) enum RowEntry {
    Module(PathBuf),
    Definition(rutis_bridge::session::Reference),
}

/// The key a service a row announces is published under, by name.
pub(crate) type KeyOf = Arc<dyn Fn(&str) -> TypeKey + Send + Sync>;

struct JsFactory {
    name: String,
    runtime: RowRuntime,
    entry: RowEntry,
    /// The id its copies are named by when it is not a loader row (a copy
    /// of a definition, named after the row that handed it over).
    id: Option<String>,
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
type Refused = Arc<Refusals>;

/// Why rows were refused a service they provided after loading, by fiber.
/// A refusal is for the one restart it causes: only the apply that restart
/// runs fails with it. One recorded while another restart was already on
/// its way, or left once its restart has run (one that waited for a
/// dependency instead of applying), fails no later generation, which may
/// run when the clash is long gone; that one is refused again if it
/// provides the name while the clash is still there.
#[derive(Default)]
struct Refusals(Mutex<HashMap<rutis::host::InstanceId, (SessionError, bool)>>);

impl Refusals {
    /// Record why `instance` was refused, unless it was already.
    fn record(&self, instance: rutis::host::InstanceId, why: SessionError) {
        self.0
            .lock()
            .unwrap()
            .entry(instance)
            .or_insert((why, false));
    }

    /// The restart for `instance`'s refusal is about to run: its apply
    /// fails with it.
    fn arm(&self, instance: rutis::host::InstanceId) {
        if let Some((_, armed)) = self.0.lock().unwrap().get_mut(&instance) {
            *armed = true;
        }
    }

    /// Why this apply of `instance` fails: the refusal whose restart runs it.
    fn take(&self, instance: rutis::host::InstanceId) -> Option<SessionError> {
        let mut refusals = self.0.lock().unwrap();
        match refusals.get(&instance) {
            Some((_, true)) => refusals.remove(&instance).map(|(why, _)| why),
            _ => None,
        }
    }

    /// The refusal's restart has run, or will not: it fails nothing more.
    fn forget(&self, instance: rutis::host::InstanceId) {
        self.0.lock().unwrap().remove(&instance);
    }
}

impl JsFactory {
    /// The factory of a copy outside instances, its rows on `runtime`.
    pub(crate) fn on(
        name: &str,
        runtime: RowRuntime,
        entry: PathBuf,
        gated: Vec<String>,
        provides: Map<String, Value>,
    ) -> Self {
        let entry = RowEntry::Module(entry);
        let gated = gated
            .into_iter()
            .map(|name| {
                let key = host_key(&name).erase();
                (name, key)
            })
            .collect();
        let provided = provides
            .keys()
            .map(|name| (name.clone(), host_key(name).erase()))
            .collect();
        Self::with_keys(name, runtime, entry, gated, provides, provided, Vec::new())
    }

    pub(crate) fn with_keys(
        name: &str,
        runtime: RowRuntime,
        entry: RowEntry,
        gated: Vec<(String, TypeKey)>,
        provides: Map<String, Value>,
        provided: Vec<(String, TypeKey)>,
        scopes: Vec<(String, String)>,
    ) -> Self {
        let rows = match &runtime {
            RowRuntime::Connection(_) => None,
            RowRuntime::Session(row) => Some(SessionRows::key(row)),
        };
        let injects = rows
            .into_iter()
            .chain(gated.iter().map(|(_, key)| key.clone()))
            .collect();
        Self {
            name: name.to_owned(),
            runtime,
            entry,
            id: None,
            gated,
            provides,
            provided,
            scopes,
            injects,
            announced: Arc::new(|name: &str| host_key(name).erase()),
            refused: Refused::default(),
        }
    }

    /// Publish the services a row announces under `key(name)`.
    pub(crate) fn announcing(mut self, key: KeyOf) -> Self {
        self.announced = key;
        self
    }

    /// Name its copies `id` (they are not loader rows).
    pub(crate) fn named(mut self, id: Option<String>) -> Self {
        self.id = id;
        self
    }
}

impl PluginFactory<Value> for JsFactory {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(JsRow {
            name: self.name.clone(),
            runtime: self.runtime.clone(),
            entry: self.entry.clone(),
            id: self.id.clone(),
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
    entry: RowEntry,
    id: Option<String>,
    config: Value,
    gated: Vec<(String, TypeKey)>,
    provides: Map<String, Value>,
    provided: Vec<(String, TypeKey)>,
    scopes: Vec<(String, String)>,
    injects: Vec<TypeKey>,
    announced: KeyOf,
    refused: Refused,
}

/// The row failed with `error`, kept as it is: a plugin's error from the
/// runtime keeps its name, code, data and stack for whoever reads it
/// (`rutis_bridge::thrown`, #677).
fn failed(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
    rutis::Error::from(ErrorKind::PluginFailed(error.into()))
}

/// The row failed for a reason rutis states.
/// A row refused a service name another provider has: printed as the
/// row's state on its own (`b: forecast is already provided by a`).
fn refused(reason: String) -> Error {
    rutis::Error::from(ErrorKind::PluginFailed(Box::new(provide_error(reason))))
}

/// The error of a row refused a service name: `ProvideError`, printed as
/// its message alone.
fn provide_error(reason: String) -> SessionError {
    SessionError::Remote {
        name: "ProvideError".into(),
        message: reason,
        graph: None,
    }
}

/// An error rutis-bridge's sessions carry (a far end's, with its graph).
type SessionError = rutis_bridge::session::Error;

impl DynPlugin for JsRow {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            // Restarted because it provided a service another provider has:
            // it fails as it would have had it provided it while loading.
            // Or because its code left an error unhandled (#688): it fails
            // with that error.
            if let Some(error) = self.refused.take(ctx.instance()) {
                return Err(failed(error));
            }
            let process = match &self.runtime {
                RowRuntime::Connection(process) => process.clone(),
                RowRuntime::Session(row) => ctx
                    .require_key::<SessionRows>(SessionRows::key(row))?
                    .process()
                    .clone(),
            };
            // Every lease taken is released: by the cleanup once the row
            // runs, here when it does not get that far. The kernel waits for
            // apply to finish, so this frame gets to release them; only a
            // panic in it leaves them raised, in a runtime whose state is
            // then unknown anyway.
            let mut leases: Vec<HostLease> = Vec::new();
            let started = self.start(ctx, &process, &mut leases).await;
            let (key, projection) = match started {
                Ok(started) => started,
                Err(error) => {
                    release(leases).await;
                    // The runtime's process ended while the row loaded:
                    // not the row's failure, whichever row's code ended it.
                    // The row waits for its runtime, as the others in it do
                    // (#682). Only a named runtime's row, or a session's
                    // (its session's end withdraws `SessionRows`): nothing
                    // ends the wait of a row on a connection, which does not
                    // come back (a runtime connection's instance waits for
                    // this load before it can end with its connection), nor
                    // of a copy of a definition. They fail at once.
                    if let Some(ended) = process.connection().close_reason() {
                        if matches!(self.runtime, RowRuntime::Session(_)) {
                            return wait_for_the_runtime(ctx, ended).await;
                        }
                    }
                    return Err(error);
                }
            };
            let row = self.row_name(ctx);
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
                        Err(e) => Err(failed(e.with_site(&Site::new(row, "cleanup")))),
                    }
                })
            })))
        })
    }
}

impl JsRow {
    /// The row's name in messages: the loader's id for it, the row that
    /// handed over a copied definition, or the plugin's name.
    fn row_name(&self, ctx: &Ctx) -> String {
        if let Some(id) = &self.id {
            return id.clone();
        }
        ctx.get::<Loader>()
            .and_then(|loader| loader.row(ctx.instance()))
            .map(|row| row.id)
            .unwrap_or_else(|| self.name.clone())
    }

    /// Lease the shared services the plugin injects into `leases`, then
    /// load it into the runtime. Returns its key there and the projection
    /// of its services; on failure nothing of the load remains, and the
    /// caller releases the leases.
    async fn start(
        &self,
        ctx: &Ctx,
        process: &Arc<Process>,
        leases: &mut Vec<HostLease>,
    ) -> Result<(String, Arc<Projection>), Error> {
        let row = match &self.id {
            // A copy of a definition: named after the row that handed it
            // over, with nothing a row's config isolates or injects.
            Some(_) => None,
            None => ctx
                .get::<Loader>()
                .and_then(|loader| loader.row(ctx.instance())),
        };
        let (id, mut isolate, inject) = match row {
            Some(row) => (row.id, row.isolate, row.inject),
            None => (
                self.id.clone().unwrap_or_else(|| self.name.clone()),
                Vec::new(),
                Vec::new(),
            ),
        };
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
            let dispatch = ctx.require_key::<dyn HostDispatch>(key.clone())?;
            if dispatch
                .origin()
                .is_some_and(|origin| origin == process.connection().tag())
            {
                continue;
            }
            let label = label_of(&isolate, name);
            let lease = process
                .lease_host_for(name, label.as_deref(), &caller, dispatch, None)
                .await
                .map_err(failed)?;
            if let Some(own) = lease.label().filter(|own| Some(*own) != label.as_deref()) {
                isolate.retain(|(isolated, _)| isolated != name);
                isolate.push((name.clone(), own.to_owned()));
            }
            leases.push(lease);
        }
        // The row's services are published from this fiber, so they go when
        // it does: the declared ones, and those it announces. A name the row
        // isolates is published in its isolate (the loader runs the fiber
        // there), where rows of the same isolate see it.
        let provided = self.provided.clone();
        let announced = self.announced.clone();
        let projection = row_projection_of(&format!("row {id}"), &self.provides, move |name| {
            provided
                .iter()
                .find(|(provided, _)| provided == name)
                .map(|(_, key)| key.clone())
                .unwrap_or_else(|| announced(name))
        });
        // A definition one of its services returns belongs to this row.
        projection.owned_by(caller.clone());
        projection.attach(ctx, process.clone())?;
        // A plugin that disposes itself there ends this fiber, as a Rust
        // plugin disposing itself: the loader's monitor takes it from there.
        let ending = ctx.clone();
        process.on_row_ended(&key, move || {
            // Already unloading or restarted: nothing left to end.
            let _ = ending.dispose_self();
        });
        // So does an error its code leaves unhandled, which the runtime
        // traced to it (#688): the runtime and its other rows go on.
        // Registered before the load, as an error may come right after it
        // (the load itself fails for one while it loads). One that comes
        // while this apply still finishes restarts the fiber all the same:
        // the kernel's restart cancels the apply running, whose cleanup
        // unloads the runtime's row, before the next apply fails.
        let tasks = tokio::runtime::Handle::current();
        let (refusals, restarting) = (self.refused.clone(), ctx.clone());
        process.on_row_failed(&key, move |why| {
            fail_running(&restarting, &refusals, &tasks, why)
        });
        // Its listeners' descriptions and failure reports name it by its id.
        process.name_row(&key, &id);
        let loaded = match &self.entry {
            RowEntry::Module(entry) => {
                process
                    .load_row_exporting(
                        &key,
                        entry,
                        self.config.clone(),
                        &isolate,
                        &inject,
                        &self.provides,
                        projection.clone(),
                        &ctx.scope(),
                    )
                    .await
            }
            RowEntry::Definition(definition) => {
                process
                    .load_definition_exporting(
                        &key,
                        definition,
                        self.config.clone(),
                        &isolate,
                        &inject,
                        &self.provides,
                        projection.clone(),
                        &ctx.scope(),
                    )
                    .await
            }
        };
        if let Err(error) = loaded {
            projection.close();
            return Err(failed(
                error.with_site(&Site::new(self.row_name(ctx), "apply")),
            ));
        }
        // A service another provider had first: this row, the later one,
        // fails.
        if let Some(refused_reason) = projection.take_refused() {
            projection.withdraw().await;
            let _ = process.unload_row(&key).await;
            return Err(refused(refused_reason));
        }
        // Volatile-only changes go to Cordis, which commits them in place.
        let forward = Arc::new(Forward {
            key: key.clone(),
            process: process.clone(),
        });
        let listening =
            ctx.listen::<VolatileUpdate>()
                .name(volatile_key(ctx))
                .on(move |update, _| {
                    let forward = forward.clone();
                    async move { forward.apply(&update).await }
                });
        if let Err(error) = listening {
            // The plugin is loaded, but no cleanup will be registered for it:
            // undo the load here.
            projection.withdraw().await;
            let _ = process.unload_row(&key).await;
            return Err(error);
        }
        // One it provides afterwards fails it the same way: the fiber
        // restarts, and that load fails with why.
        let tasks = tokio::runtime::Handle::current();
        let (refused, restarting) = (self.refused.clone(), ctx.clone());
        projection
            .on_refused(move |why| fail_running(&restarting, &refused, &tasks, provide_error(why)));
        Ok((key, projection))
    }
}

/// Fail the running row of `restarting` with `why`: its fiber restarts,
/// and that apply fails with why ([`Refusals`]). A row a service clash
/// refused fails so, and one whose code left an error unhandled (#688).
fn fail_running(
    restarting: &Ctx,
    refused: &Refused,
    tasks: &tokio::runtime::Handle,
    why: SessionError,
) {
    let generation = restarting.cancellation_token();
    if generation.is_cancelled() {
        return; // already unloading or restarted
    }
    refused.record(restarting.instance(), why);
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
        // Gone or restarting anyway: nothing to fail.
        if let Some(view) = view.filter(|_| !generation.is_cancelled()) {
            refused.arm(instance);
            let _ = view.restart().await;
        }
        refused.forget(instance);
    });
}

/// Mounts the copies of plugin definitions other languages hand over: each
/// is a row of the definition's runtime, keyed for the instance it mounts
/// in (`docs/design-plugin-values-2026-10-25.md` §6.3).
pub(crate) struct DefinitionMounter {
    pub(crate) loader: Loader,
}

impl PluginMounter for DefinitionMounter {
    fn mount(
        &self,
        at: &Ctx,
        plugin: &ForeignPlugin,
        config: Value,
    ) -> Result<Box<dyn DynPlugin>, Error> {
        let build = self.loader.build_at(at);
        let catalog = self.loader.catalog().clone();
        let process = plugin.process();
        let schema = plugin.schema();
        // As for a row of that runtime: a leaf runtime gates everything a
        // plugin injects in rutis; in Cordis only the shared names.
        let leaf = process.supports("leaf");
        let key = |name: &str| -> Result<TypeKey, Error> {
            match catalog.is_shared(name) {
                true => catalog.key_in(name, &build).map_err(failed),
                false => Ok(host_key(name).erase()),
            }
        };
        let gated = schema
            .inject
            .iter()
            .filter(|name| leaf || catalog.is_shared(name))
            .map(|name| Ok((name.clone(), key(name)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        let provided = schema
            .provides
            .keys()
            .map(|name| Ok((name.clone(), key(name)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        let scopes = catalog
            .instance_names(&build)
            .into_iter()
            .filter(|(name, _)| catalog.is_shared(name))
            .map(|(name, instance)| (name, format!("rutis-loader/instance/{instance}")))
            .collect();
        let announced: KeyOf = Arc::new(move |name| match catalog.is_shared(name) {
            true => catalog
                .key_in(name, &build)
                .unwrap_or_else(|_| host_key(name).erase()),
            false => host_key(name).erase(),
        });
        let id = plugin.row().map(str::to_owned);
        JsFactory::with_keys(
            plugin.name(),
            RowRuntime::Connection(process.clone()),
            RowEntry::Definition(plugin.reference().clone()),
            gated,
            schema.provides.clone(),
            provided,
            scopes,
        )
        .announcing(announced)
        .named(id.or_else(|| Some(plugin.name().to_owned())))
        .build(&config)
    }
}

/// How long a row whose runtime ended while it loaded waits for that end to
/// reach it (the runtime withdrawing its rows' service, which ends this
/// generation) before it fails, saying the runtime ended.
const RUNTIME_GONE: std::time::Duration = std::time::Duration::from_secs(30);

/// End a generation whose runtime's session ended (`ended`) while it loaded:
/// once the runtime's end reaches the row, which then waits for the
/// runtime, with nothing of it registered.
async fn wait_for_the_runtime(
    ctx: &Ctx,
    ended: rutis_bridge::session::Error,
) -> Result<Effect, Error> {
    tokio::select! {
        () = ctx.cancelled() => Ok(Effect::Done),
        () = tokio::time::sleep(RUNTIME_GONE) => Err(failed(format!(
            "its runtime ended while it loaded: {ended}"
        ))),
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

impl Forward {
    async fn apply(&self, update: &VolatileUpdate) -> Result<Option<()>, Error> {
        self.process
            .update_row(&self.key, update.config.clone())
            .await
            .map_err(failed)?;
        Ok(None)
    }
}
