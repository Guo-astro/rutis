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
        config.factory.build(&config.value)
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
            let token = self.loader.inner.next_token();
            self.loader.inner.attach(None, token, ctx);
            let weak = Arc::downgrade(&self.loader.inner);
            Ok(Effect::Disposer(Box::new(move || {
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
