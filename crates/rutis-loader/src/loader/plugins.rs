//! Kernel glue: the entry factory, group plugin and loader plugin.

use std::sync::{Arc, Mutex, Weak};

use rutis::host::{BoxFuture, DynPlugin, Effect, PluginFactory, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error};
use serde_json::Value;

use crate::resolver::{Resolved, Resolver};

use super::{Inner, Loader, LoaderOptions, Slot, State};

/// One generation's module, the factory it gives this copy, and the
/// evaluated config.
#[derive(Clone)]
pub(super) struct EntryConfig {
    pub(super) resolved: Arc<Resolved>,
    pub(super) factory: Arc<dyn PluginFactory<Value>>,
    pub(super) value: Value,
}

pub(super) struct EntryFactory {
    /// The loader: its config step fills and validates each config.
    pub(super) loader: Weak<Inner>,
    pub(super) name: String,
    pub(super) injects: Vec<TypeKey>,
    /// The row's id: its fiber is marked with it, so its listeners'
    /// descriptors name it (events protocol §4.2).
    pub(super) row: String,
}

impl PluginFactory<EntryConfig> for EntryFactory {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn validate_config(&self, config: &EntryConfig) -> Result<(), Error> {
        config.factory.validate_config(&self.filled(config)?)
    }

    fn build(&self, config: &EntryConfig) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(RowPlugin {
            row: self.row.clone(),
            inner: config.factory.build(&self.filled(config)?)?,
        }))
    }
}

impl EntryFactory {
    /// `config` through the loader's config step: the filled copy for the
    /// rows the loader fills, as it is for others.
    fn filled(&self, config: &EntryConfig) -> Result<Value, Error> {
        let Some(loader) = self.loader.upgrade() else {
            return Ok(config.value.clone());
        };
        let subject = super::config_step::Subject::of(&config.resolved);
        let purpose = super::config_step::Purpose::Run {
            row: &self.row,
            instance: None,
        };
        loader
            .config_step(&subject, &config.value, purpose)
            .map_err(rutis::ConfigError::into_error)
    }
}

/// A row's plugin, its fiber marked as the row (`Ctx::mark_row`).
struct RowPlugin {
    row: String,
    inner: Box<dyn DynPlugin>,
}

impl DynPlugin for RowPlugin {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.inner.injects()
    }

    fn validate(&self) -> Result<(), Error> {
        self.inner.validate()
    }

    fn platform(&self) -> bool {
        self.inner.platform()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.mark_row(&self.row)?;
            self.inner.apply(ctx).await
        })
    }
}

/// A group row: spawns its children in its own context, so disabling the
/// group unloads them through the kernel's cascade.
pub(super) struct GroupPlugin {
    pub(super) inner: Weak<Inner>,
    pub(super) slot: Slot,
    /// Keys from the row's `inject`.
    pub(super) injects: Vec<TypeKey>,
    /// The token of the spawn that created this plugin.
    pub(super) token: u64,
}

impl DynPlugin for GroupPlugin {
    fn name(&self) -> &str {
        &self.slot.row
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let Some(inner) = self.inner.upgrade() else {
                return Ok(Effect::Done);
            };
            let token = self.token;
            // An instance is a scope of the logical bus (events protocol
            // §3): its rows' listeners hear what is emitted in it.
            if self.slot.scope.is_some() {
                ctx.mark_scope(&self.slot.row)?;
            }
            inner.attach(Some(self.slot.clone()), token, ctx);
            let weak = self.inner.clone();
            let slot = self.slot.clone();
            Ok(Effect::Disposer(Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.detach(Some(slot), token);
                }
                Ok(())
            })))
        })
    }
}

/// Mounts a [`Loader`] and provides it as a service.
pub struct LoaderPlugin {
    loader: Loader,
}

impl LoaderPlugin {
    pub fn new(resolver: impl Resolver, options: LoaderOptions) -> Self {
        Self {
            loader: Loader {
                inner: Arc::new(Inner {
                    resolver: Arc::new(resolver),
                    persist: options.persist,
                    catalog: options.catalog,
                    expressions: options.expressions,
                    op: tokio::sync::Mutex::new(()),
                    #[cfg(feature = "runtimes")]
                    rekey_pending: Default::default(),
                    warned: Default::default(),
                    config_warnings: Mutex::new(super::config_step::standard_error()),
                    followers: Arc::default(),
                    state: Mutex::new(State::default()),
                }),
            },
        }
    }

    pub fn handle(&self) -> Loader {
        self.loader.clone()
    }
}

impl DynPlugin for LoaderPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "rutis-loader"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            // The service is released with the fiber.
            ctx.provide(self.loader.clone())?;
            // Copies of plugin definitions other languages hand over are
            // mounted through it, anywhere below.
            #[cfg(feature = "runtimes")]
            ctx.provide_as::<dyn rutis_bridge::runtime::PluginMounter>(Arc::new(
                crate::runtime::DefinitionMounter {
                    loader: self.loader.clone(),
                },
            ))?;
            // A marked Rust service served in an instance makes its name
            // that instance's own: the copies there gating on it are rebuilt.
            #[cfg(feature = "runtimes")]
            let rekeying = {
                let rekeying = self.loader.rekeying();
                rutis_bridge::exports::on_claim(&rekeying);
                rekeying
            };
            // `rutis/service` for the rows' services, Rust ones included
            // (events protocol §8).
            #[cfg(any(feature = "runtimes", feature = "peer"))]
            let bus = rutis_bridge::events::join_bus(ctx).map_err(Error::from)?;
            // A long wait's report names what rows wait for as they do: a
            // service by its name, a runtime as `runtime <name>` (#676).
            let namer: Arc<rutis::host::KeyNamer> = {
                let loader = Arc::downgrade(&self.loader.inner);
                Arc::new(move |key: &rutis::host::TypeKey| {
                    let inner = loader.upgrade()?;
                    super::waits::key_name(&inner.catalog, key)
                })
            };
            rutis::host::CtxHost::name_keys_with(ctx, &namer);
            // One listener for the loader's rows, registered on the root: it
            // outlives this fiber while the rows stop (`Followers`). Last of
            // the fallible steps, so a failure above leaves nothing behind on
            // the root; no row can be followed before `attach`.
            self.loader.inner.followers.listen(ctx)?;
            let token = self.loader.inner.next_token();
            self.loader.inner.attach(None, token, ctx);
            let weak = Arc::downgrade(&self.loader.inner);
            Ok(Effect::Disposer(Box::new(move || {
                drop(namer);
                #[cfg(any(feature = "runtimes", feature = "peer"))]
                drop(bus);
                #[cfg(feature = "runtimes")]
                drop(rekeying);
                if let Some(inner) = weak.upgrade() {
                    inner.detach(None, token);
                    // Instances live in the loader's tree: they go with it.
                    inner.state.lock().unwrap().instances.clear();
                    // Their rows still stop, and say so: the listener leaves
                    // with the last of them.
                    inner.followers.stop();
                }
                Ok(())
            })))
        })
    }
}
