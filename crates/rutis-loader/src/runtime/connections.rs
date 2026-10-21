//! Runtimes that dial in: each connection an access point admits is an
//! instance of one `instanced` group
//! (`docs/design-remote-runtime-dial-2026-10-14.md` §四).
//!
//! [`RuntimeInstances`] is both halves. As the access point's
//! [`Accept`] ([`RuntimeInstances::accept`]) it mounts a runtime on each
//! connection's session and creates an instance of the group with that
//! connection ([`RuntimeConnection`]) as the instance's value; the release
//! removes the instance. As a [`Resolver`] it resolves the rows
//! `<runtime>:<plugin>`, and each copy of such a row, through
//! [`Build::value`], runs on the connection of its own instance.
//!
//! - What a connection's plugin declares is read once per connection: from
//!   the `plugins` its `mount` reply lists, else with `rows.schema`. A
//!   connection without the plugin fails that row in its instance alone
//!   ([`Unavailable`]): other instances run it, and an edit adding the row
//!   is not refused because of it.
//! - The services a connection's plugins provide are its instance's own
//!   (`host_key_in(name, instance)`): two connections with the same plugin
//!   do not collide, and in each instance a row using the name gets its own
//!   connection's service. Rows outside the group cannot use them. So are
//!   the ones they provide without declaring them, from when a row
//!   announces one: the copies in the instance that gate on the name were
//!   built with the global key, and are rebuilt with the instance's
//!   ([`Loader::rekeying`]), as if it had been declared.
//!
//! The connection's process belongs to its instance: a row of it holds the
//! process directly and does not wait for a runtime service, and the
//! instance ends with the connection.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::{BoxFuture, CordisError, Ctx, InstanceId, Plugin, PluginFactory};
use rutis_bridge::runtime::{Mount, Process, RowSchema};
use rutis_bridge::session::host_key;
use rutis_bridge::{Accept, Accepted, PeerId, Release};
use serde_json::{json, Value};

use super::{unknown, JsFactory, RowRuntime};
use crate::resolver::{Build, ScopedFactory};
use crate::{Loader, LoaderError, Resolved, Resolver, ServiceCatalog, Unavailable};

/// What the rows of a runtime need from it (as for a runtime rutis mounts
/// itself): describing plugins, exporting their services, leasing host
/// services.
const ROW_FEATURES: [&str; 2] = ["rows.v2", "hosts"];

/// One runtime connection, the value of its instance ([`Build::value`]).
#[derive(Clone)]
pub struct RuntimeConnection {
    inner: Arc<Connected>,
}

struct Connected {
    runtime: String,
    id: PeerId,
    process: Arc<Process>,
    /// What each plugin a row named declares on this connection.
    described: Mutex<HashMap<String, Described>>,
    /// The names its rows provide without declaring them, as they announce
    /// them: this instance's own from then on, as declared ones are.
    announced: Mutex<BTreeSet<String>>,
    /// Rebuilds the copies whose keys a new announced name changes.
    rekeying: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
enum Described {
    Found(RowSchema),
    /// The connection does not contain the plugin.
    Missing,
    /// Asking the runtime failed: why.
    Failed(String),
}

impl RuntimeConnection {
    /// The runtime's name, the prefix of its rows.
    pub fn runtime(&self) -> &str {
        &self.inner.runtime
    }

    /// The connection id the access point admitted it as.
    pub fn id(&self) -> &PeerId {
        &self.inner.id
    }

    /// The runtime mounted on the connection's session.
    pub fn process(&self) -> &Arc<Process> {
        &self.inner.process
    }

    /// Whether a plugin of this connection that a row runs provides `name`:
    /// of the plugins described for rows, not of every plugin it lists, so a
    /// plugin no row runs claims no name; or a row of it announced `name`.
    fn provides(&self, name: &str) -> bool {
        if self.inner.announced.lock().unwrap().contains(name) {
            return true;
        }
        self.inner
            .described
            .lock()
            .unwrap()
            .values()
            .any(|described| match described {
                Described::Found(plugin) => plugin.provides.contains_key(name),
                _ => false,
            })
    }

    /// Every name [`RuntimeConnection::provides`].
    fn provided(&self) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = self
            .inner
            .described
            .lock()
            .unwrap()
            .values()
            .filter_map(|described| match described {
                Described::Found(plugin) => Some(plugin.provides.keys().cloned()),
                _ => None,
            })
            .flatten()
            .collect();
        names.extend(self.inner.announced.lock().unwrap().iter().cloned());
        names
    }

    /// A row of this connection announces `name`, which it did not
    /// declare: from now on it is the instance's own, and the copies that
    /// gate on it with the global key are rebuilt.
    fn announced(&self, name: &str) {
        let new = self.inner.announced.lock().unwrap().insert(name.to_owned());
        if new {
            (self.inner.rekeying)();
        }
    }

    /// Read what `plugin` declares on this connection, once.
    async fn describe(&self, plugin: &str) {
        if self.inner.described.lock().unwrap().contains_key(plugin) {
            return;
        }
        let described = match self.inner.process.plugins() {
            Some(plugins) => match plugins.get(plugin) {
                Some(found) => Described::Found(found.clone()),
                None => Described::Missing,
            },
            None => match self.inner.process.describe_row(Path::new(plugin)).await {
                Ok(found) => Described::Found(found),
                // The runtime looked and found no such plugin.
                Err(rutis_bridge::session::Error::Remote { name, .. }) if name == "NotFound" => {
                    Described::Missing
                }
                Err(error) => Described::Failed(error.to_string()),
            },
        };
        self.inner
            .described
            .lock()
            .unwrap()
            .entry(plugin.to_owned())
            .or_insert(described);
    }

    fn described(&self, plugin: &str) -> Option<Described> {
        self.inner.described.lock().unwrap().get(plugin).cloned()
    }
}

/// The instance enclosing a copy in `build` whose connection provides the
/// shared name `name`, innermost first: there `name` is that instance's own.
///
/// What a connection provides is read when its rows are resolved, and
/// grows as its rows announce names they do not declare. A row of the
/// runtime added to the group later, by a loader edit, claims its declared
/// names for copies built from then on; copies already running keep the
/// keys they were built with until they are rebuilt (rutis-host reads its
/// rows once, before any connection, so it does not meet this). An
/// announced name rebuilds the copies it changes the keys of itself
/// ([`RuntimeConnection::announced`]).
pub(crate) fn claimed(name: &str, build: &Build) -> Option<InstanceId> {
    connections(build)
        .find(|(connection, _)| connection.provides(name))
        .map(|(_, instance)| instance)
}

/// Every name the connections enclosing a copy in `build` provide, with
/// the instance it is the own of ([`claimed`]).
pub(crate) fn claimed_names(build: &Build) -> Vec<(String, InstanceId)> {
    let mut names: Vec<(String, InstanceId)> = Vec::new();
    for (connection, instance) in connections(build) {
        for name in connection.provided() {
            if !names.iter().any(|(known, _)| known == &name) {
                names.push((name, instance));
            }
        }
    }
    names
}

/// The runtime connections enclosing a copy in `build`, innermost first,
/// each with the instance it is the value of.
fn connections(build: &Build) -> impl Iterator<Item = (RuntimeConnection, InstanceId)> + '_ {
    build.chain.iter().filter_map(|link| {
        let value = link
            .values
            .get(&std::any::TypeId::of::<RuntimeConnection>())?;
        let connection = value.clone().downcast::<RuntimeConnection>().ok()?;
        Some(((*connection).clone(), link.instance))
    })
}

/// The runtime named `runtime` whose connections come in through an access
/// point, each an instance of the `instanced` group `group`: the resolver
/// of its rows `<runtime>:<plugin>`, and, with
/// [`RuntimeInstances::accept`], what the access point does with each
/// connection. Put it in the loader's chain (`Chain::with`) and give
/// `accept` to the access point (`AccessPointPlugin::accept`).
#[derive(Clone)]
pub struct RuntimeInstances {
    inner: Arc<Instances>,
}

struct Instances {
    runtime: String,
    group: String,
    prefix: String,
    catalog: ServiceCatalog,
    /// The connections with an instance, or about to have one.
    live: Mutex<Vec<RuntimeConnection>>,
    /// Every plugin a row named so far: each new connection describes
    /// them before its instance is created.
    plugins: Mutex<BTreeSet<String>>,
}

impl RuntimeInstances {
    /// The rows `<runtime>:<plugin>` of the runtime `runtime`, one instance
    /// of `group` per connection. `catalog` is the loader's
    /// (`LoaderOptions::catalog`): its shared names are the injected
    /// services the rows wait for in rutis.
    pub fn new(runtime: &str, group: &str, catalog: &ServiceCatalog) -> Self {
        Self {
            inner: Arc::new(Instances {
                runtime: runtime.to_owned(),
                group: group.to_owned(),
                prefix: format!("{runtime}:"),
                catalog: catalog.clone(),
                live: Mutex::default(),
                plugins: Mutex::default(),
            }),
        }
    }

    /// The runtime's name.
    pub fn runtime(&self) -> &str {
        &self.inner.runtime
    }

    /// The `instanced` group each connection is an instance of.
    pub fn group(&self) -> &str {
        &self.inner.group
    }

    /// What the access point does with each connection: mount the runtime
    /// on its session, then create the connection's instance of the group
    /// in `loader`, from `ctx` (the loader's context or one above it; the
    /// group is a top-level one). The release removes the instance.
    pub fn accept(&self, loader: Loader, ctx: Ctx) -> AcceptInstances {
        AcceptInstances {
            instances: self.clone(),
            loader,
            ctx,
        }
    }

    /// The factory of a copy running in `build`'s instances: on the
    /// connection of its instance, as the plugin is there.
    fn factory(
        &self,
        row: &str,
        plugin: &str,
        build: &Build,
    ) -> Result<Arc<dyn PluginFactory<Value>>, CordisError> {
        let inner = &self.inner;
        let found =
            connections(build).find(|(connection, _)| connection.runtime() == inner.runtime);
        let Some((connection, instance)) = found else {
            return Err(outside(row, &inner.group));
        };
        let described = match connection.described(plugin) {
            Some(Described::Found(described)) => described,
            Some(Described::Missing) => {
                return Err(Unavailable(format!(
                    "this connection does not contain plugin {plugin}"
                ))
                .into_error())
            }
            Some(Described::Failed(error)) => {
                return Err(Unavailable(format!(
                    "this connection cannot describe plugin {plugin}: {error}"
                ))
                .into_error())
            }
            None => {
                return Err(Unavailable(format!(
                    "plugin {plugin} was not described on this connection"
                ))
                .into_error())
            }
        };
        let process = connection.process().clone();
        // A leaf runtime has no dependency resolution of its own: every
        // service its plugins inject waits in rutis. In Cordis, only the
        // shared names do.
        let leaf = process.supports("leaf");
        let gated = described
            .inject
            .iter()
            .filter(|name| leaf || inner.catalog.is_shared(name))
            .map(|name| {
                let key = match inner.catalog.is_shared(name) {
                    true => inner.catalog.key_in(name, build).map_err(super::failed)?,
                    false => host_key(name),
                };
                Ok((name.clone(), key))
            })
            .collect::<Result<Vec<_>, CordisError>>()?;
        // Its services are its instance's own.
        let provided = described
            .provides
            .keys()
            .map(|name| {
                let key = host_key(name).with_instance(instance);
                (name.clone(), key)
            })
            .collect();
        // The process is the instance's alone: nothing in it needs a label
        // to keep instances apart.
        let factory: Arc<dyn PluginFactory<Value>> = Arc::new(
            JsFactory::with_keys(
                row,
                RowRuntime::Connection(process),
                PathBuf::from(plugin),
                gated,
                described.provides.clone(),
                provided,
                Vec::new(),
            )
            // So are the ones it announces, which from then on are the
            // instance's own for every copy there.
            .announcing(Arc::new(move |name| {
                connection.announced(name);
                host_key(name).with_instance(instance)
            })),
        );
        Ok(factory)
    }

    /// Describe `plugin` on every connection, for a row naming it.
    async fn describe_everywhere(&self, plugin: &str) {
        self.inner.plugins.lock().unwrap().insert(plugin.to_owned());
        let live = self.inner.live.lock().unwrap().clone();
        for connection in live {
            connection.describe(plugin).await;
        }
    }
}

/// A row of the runtime used outside its group.
fn outside(row: &str, group: &str) -> CordisError {
    CordisError::PluginFailed(
        format!(
            "{row} runs only inside the {group} group, in the instance of a runtime connection"
        )
        .into(),
    )
}

impl Resolver for RuntimeInstances {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            let plugin = match name.strip_prefix(self.inner.prefix.as_str()) {
                Some(plugin) if !plugin.is_empty() => plugin.to_owned(),
                _ => {
                    return Err(LoaderError::NotFound {
                        name: name.to_owned(),
                    })
                }
            };
            // A row added or reloaded is described on the connections
            // there are; a connection that comes later describes it itself.
            self.describe_everywhere(&plugin).await;
            let scoped: ScopedFactory = {
                let instances = self.clone();
                let (row, plugin) = (name.to_owned(), plugin.clone());
                Arc::new(move |build: &Build| instances.factory(&row, &plugin, build))
            };
            Ok(Arc::new(Resolved {
                factory: Arc::new(Outside {
                    name: name.to_owned(),
                    group: self.inner.group.clone(),
                }),
                schema: None,
                meta: json!({
                    "source": "runtime",
                    "runtime": self.inner.runtime,
                    "entry": plugin,
                    "instances": self.inner.group,
                }),
                foreign_scope: true,
                scoped: Some(scoped),
                declares: Some(unknown(format!(
                    "what {name} declares depends on the connection of each {} instance",
                    self.inner.group
                ))),
            }))
        })
    }
}

/// The factory of a row of the runtime outside its group's instances.
struct Outside {
    name: String,
    group: String,
}

impl PluginFactory<Value> for Outside {
    fn name(&self) -> &str {
        &self.name
    }

    fn build(&self, _config: &Value) -> Result<Box<dyn Plugin>, CordisError> {
        Err(outside(&self.name, &self.group))
    }
}

/// What an access point does with each runtime connection
/// ([`RuntimeInstances::accept`]).
pub struct AcceptInstances {
    instances: RuntimeInstances,
    loader: Loader,
    ctx: Ctx,
}

impl Accept for AcceptInstances {
    fn accept(&self, accepted: Accepted) -> BoxFuture<'_, Result<Release, String>> {
        Box::pin(async move {
            let inner = &self.instances.inner;
            let runtime = &inner.runtime;
            let process = Process::over(accepted.runtime_session(), Mount::default())
                .await
                .map_err(|error| format!("mounting the {runtime} runtime: {error}"))?;
            let missing: Vec<&str> = ROW_FEATURES
                .iter()
                .copied()
                .filter(|feature| !process.supports(feature))
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "the {runtime} runtime lacks {}: a runtime speaking the row contract is required",
                    missing.join(" and ")
                ));
            }
            let connection = RuntimeConnection {
                inner: Arc::new(Connected {
                    runtime: runtime.clone(),
                    id: accepted.id().clone(),
                    process,
                    described: Mutex::default(),
                    announced: Mutex::default(),
                    rekeying: self.loader.rekeying(),
                }),
            };
            // Live first, then the plugins named so far: a row added
            // meanwhile is described here or by its resolution.
            inner.live.lock().unwrap().push(connection.clone());
            let plugins = inner.plugins.lock().unwrap().clone();
            for plugin in &plugins {
                connection.describe(plugin).await;
            }
            let forget = {
                let instances = self.instances.clone();
                let connection = connection.clone();
                move || {
                    instances
                        .inner
                        .live
                        .lock()
                        .unwrap()
                        .retain(|live| !Arc::ptr_eq(&live.inner, &connection.inner));
                }
            };
            // Not under the loader's operation lock, and from the access
            // point's own task: a reconcile running meanwhile is not waited
            // for, nor does it wait for this.
            let created = self
                .loader
                .create_instance(&self.ctx, &inner.group)
                .with(connection)
                .await;
            let instance = match created {
                Ok(instance) => instance,
                Err(error) => {
                    forget();
                    return Err(format!("its {} instance: {error}", inner.group));
                }
            };
            let loader = self.loader.clone();
            let release: Release = Box::new(move || {
                Box::pin(async move {
                    forget();
                    // Gone already when the loader or the parent went.
                    let _ = loader.remove_instance(instance.plugin).await;
                })
            });
            Ok(release)
        })
    }
}
