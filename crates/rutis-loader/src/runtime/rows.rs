//! The second stage of a runtime: rows start only once their declarations
//! are complete.
//!
//! A row resolved before its runtime was running has no schema and none of
//! the dependencies its plugin declares, since only the runtime can read
//! them. Were rows to start as soon as the runtime does, such a row could
//! run before a service it injects is there. So the runtime provides
//! `Runtime` first; this plugin then resolves those rows again and
//! only afterwards provides [`RuntimeRows`], which every row depends
//! on. Rows waiting for it do not start while they are reloaded.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error};
use rutis_bridge::runtime::Runtime;

use super::RuntimeResolver;
use crate::Loader;

/// Where a [`RuntimeRowsPlugin`] learns which rows of its runtime to
/// resolve again before they may start.
pub trait RowsSource: Send + Sync + 'static {
    /// The runtime's name.
    fn runtime_name(&self) -> &str;
    /// The row names whose resolution lacks the runtime's current
    /// declarations. A name is returned again until it is resolved anew.
    fn take_stale(&self) -> HashSet<String>;
}

impl RowsSource for RuntimeResolver {
    fn runtime_name(&self) -> &str {
        RuntimeResolver::runtime_name(self)
    }

    fn take_stale(&self) -> HashSet<String> {
        RuntimeResolver::take_stale(self)
    }
}

/// The service rows of a runtime depend on: the runtime, once the
/// rows' declarations are complete.
pub struct RuntimeRows {
    runtime: Arc<Runtime>,
}

impl RuntimeRows {
    /// The key for the rows of the runtime named `name`.
    pub fn key(name: &str) -> TypeKey {
        TypeKey::keyed_dynamic::<RuntimeRows>(name.to_owned())
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }
}

/// Mount it after the runtime and the loader, with the resolver the loader
/// uses (`Chain::with_shared`).
pub struct RuntimeRowsPlugin {
    /// The runtime's name.
    runtime: String,
    /// Its plugin name: `rows of runtime <runtime>`.
    name: String,
    resolver: Arc<dyn RowsSource>,
    injects: [TypeKey; 2],
}

impl RuntimeRowsPlugin {
    pub fn new(resolver: Arc<impl RowsSource>) -> Self {
        let name = resolver.runtime_name().to_owned();
        Self {
            injects: [Runtime::key(&name).erase(), TypeKey::of::<Loader>()],
            name: format!("rows of runtime {name}"),
            runtime: name,
            resolver,
        }
    }
}

impl DynPlugin for RuntimeRowsPlugin {
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
            let runtime = ctx.require_at::<Runtime>(&Runtime::key(&self.runtime))?;
            let key = RuntimeRows::key(&self.runtime);
            let loader = ctx.require::<Loader>()?;
            let stale = self.resolver.take_stale();
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
            let owner = ctx.clone();
            let runtime_name = self.runtime.clone();
            // In a task, not in apply: reloading takes the loader's lock,
            // which a reconcile that (re)started this plugin may hold.
            let refresh = tokio::spawn(async move {
                for id in rows {
                    // A row that fails to resolve reports it in its status;
                    // the others still start.
                    if let Err(error) = loader.refresh(&id).await {
                        eprintln!("{id}: cannot load: {error}");
                    }
                }
                if let Err(error) = owner.provide_key(key, Arc::new(RuntimeRows { runtime })) {
                    // Disposed meanwhile: nothing to provide to.
                    if !owner.cancellation_token().is_cancelled() {
                        eprintln!("runtime {runtime_name}: cannot release its rows: {error}");
                    }
                }
            });
            let refresh = Arc::new(Mutex::new(Some(refresh)));
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
