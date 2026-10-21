//! Kernel glue: the entry factory, group plugin and loader plugin.

use std::sync::{Arc, Mutex, Weak};

use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin, PluginFactory, TypeKey};
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

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn validate_config(&self, config: &EntryConfig) -> Result<(), CordisError> {
        config.factory.validate_config(&config.value)
    }

    fn build(&self, config: &EntryConfig) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(RowPlugin {
            row: self.row.clone(),
            inner: config.factory.build(&config.value)?,
        }))
    }
}

/// A row's plugin, its fiber marked as the row (`Ctx::mark_row`).
struct RowPlugin {
    row: String,
    inner: Box<dyn Plugin>,
}

impl Plugin for RowPlugin {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn injects(&self) -> &[TypeKey] {
        self.inner.injects()
    }

    fn validate(&self) -> Result<(), CordisError> {
        self.inner.validate()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
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

impl Plugin for GroupPlugin {
    fn name(&self) -> &str {
        &self.slot.row
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
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
            // An instance serves its group's exported Rust services.
            #[cfg(any(feature = "runtimes", feature = "peer"))]
            {
                let instanced = inner
                    .state
                    .lock()
                    .unwrap()
                    .desired
                    .row(&self.slot.row)
                    .is_some_and(|row| row.instanced);
                if instanced {
                    for export in inner.catalog.exports_in(Some(&self.slot.row)) {
                        ctx.plugin(export.plugin(Some(ctx.instance())));
                    }
                }
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
                    state: Mutex::new(State::default()),
                }),
            },
        }
    }

    pub fn handle(&self) -> Loader {
        self.loader.clone()
    }
}

impl Plugin for LoaderPlugin {
    fn name(&self) -> &str {
        "rutis-loader"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            // The service is released with the fiber.
            ctx.provide(self.loader.clone())?;
            // Exported Rust services outside instances, served from here.
            #[cfg(any(feature = "runtimes", feature = "peer"))]
            for export in self.loader.inner.catalog.exports_in(None) {
                ctx.plugin(export.plugin(None));
            }
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
            let bus = rutis_bridge::events::join_bus(ctx).map_err(CordisError::from)?;
            let token = self.loader.inner.next_token();
            self.loader.inner.attach(None, token, ctx);
            let weak = Arc::downgrade(&self.loader.inner);
            Ok(Effect::Disposer(Box::new(move || {
                #[cfg(any(feature = "runtimes", feature = "peer"))]
                drop(bus);
                #[cfg(feature = "runtimes")]
                drop(rekeying);
                if let Some(inner) = weak.upgrade() {
                    inner.detach(None, token);
                    // Instances live in the loader's tree: they go with it.
                    inner.state.lock().unwrap().instances.clear();
                }
                Ok(())
            })))
        })
    }
}
