//! `run`: a plugin of the far end run from here, as the remote plugin
//! `<id>:<plugin>` below the session, through the session's process and the
//! row contract (`rows.v2`) whatever the far end is.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberView, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};
use serde_json::Value as Json;
use tokio::sync::watch;

use super::failed;
use super::rows::{describe, Described};
use super::services::Target;
use crate::runtime::{row_projection_of, HostLease, Process, RowSchema};
use crate::session::{host_key, CallerRow, HostDispatch};

/// A plugin of the far end this end runs: dropping it stops the plugin.
/// [`RowHandle::loaded`] waits for its load.
pub struct RowHandle {
    task: tokio::task::AbortHandle,
    view: Arc<Mutex<Option<FiberView>>>,
    loaded: watch::Receiver<Option<Result<(), String>>>,
    tokio: tokio::runtime::Handle,
}

impl RowHandle {
    /// Wait until the plugin loaded there (`Ok`), or failed to: the far end
    /// does not contain it, or its load failed.
    pub async fn loaded(&self) -> Result<(), RutisError> {
        let mut loaded = self.loaded.clone();
        let result = match loaded.wait_for(Option::is_some).await {
            Ok(result) => result.clone().expect("waited for"),
            Err(_) => Err("the session ended before the plugin loaded".to_owned()),
        };
        result.map_err(failed)
    }
}

impl Drop for RowHandle {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(view) = self.view.lock().unwrap().take() {
            self.tokio.spawn(async move {
                let _ = view.dispose().await;
            });
        }
    }
}

/// What `run` needs of a mounted session.
pub(super) struct Running {
    /// Where its remote plugins are mounted.
    pub(super) ctx: Ctx,
    pub(super) far: String,
    pub(super) process: Arc<Process>,
    /// The far end is a rutis kernel, which resolves what its plugins
    /// inject itself.
    pub(super) node: bool,
}

/// Run `plugin` with `config` there.
pub(super) fn run(running: Running, plugin: &str, config: Json) -> RowHandle {
    let (result, loaded) = watch::channel(None);
    let view: Arc<Mutex<Option<FiberView>>> = Arc::default();
    let plugin = plugin.to_owned();
    let mounted = view.clone();
    let task = tokio::spawn(async move {
        let outcome = start(running, plugin, config, &mounted).await;
        let _ = result.send(Some(outcome));
    });
    RowHandle {
        task: task.abort_handle(),
        view,
        loaded,
        tokio: tokio::runtime::Handle::current(),
    }
}

async fn start(
    running: Running,
    plugin: String,
    config: Json,
    mounted: &Mutex<Option<FiberView>>,
) -> Result<(), String> {
    let far = running.far.clone();
    let schema = match describe(&running.process, &plugin).await {
        Described::Found(schema) => schema,
        Described::Missing => {
            return Err(format!(
                "runtime {far}: this connection does not contain plugin {plugin}"
            ))
        }
        Described::Failed(why) => {
            return Err(format!(
                "runtime {far}: this connection cannot describe plugin {plugin}: {why}"
            ))
        }
    };
    // A leaf runtime resolves nothing itself: what its plugin injects
    // waits here. A rutis kernel's plugin waits there.
    let injects = match !running.node && running.process.supports("leaf") {
        true => schema
            .inject
            .iter()
            .map(|name| host_key(name).erase())
            .collect(),
        false => Vec::new(),
    };
    let target = Target::new();
    let view = running.ctx.plugin(RunRow {
        name: format!("{far}:{plugin}"),
        plugin,
        config,
        schema,
        injects,
        process: running.process,
        target: target.clone(),
    });
    target.set_view(view.clone());
    *mounted.lock().unwrap() = Some(view.clone());
    (&view).await.map_err(|error| error.to_string())
}

/// Keys of rows run on far ends, unique in this process.
static RUNS: AtomicU64 = AtomicU64::new(0);

/// The remote plugin of a row run there.
struct RunRow {
    name: String,
    plugin: String,
    config: Json,
    schema: RowSchema,
    injects: Vec<TypeKey>,
    process: Arc<Process>,
    target: Target,
}

impl DynPlugin for RunRow {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            self.target.applied(ctx)?;
            let process = self.process.clone();
            let key = format!("{}#{}", self.plugin, RUNS.fetch_add(1, Ordering::Relaxed));
            // Ending or failing there ends or fails it here.
            let ending = ctx.clone();
            process.on_row_ended(&key, move || {
                let _ = ending.dispose_self();
            });
            let failing = self.target.clone();
            process.on_row_failed(&key, move |error| failing.fail(error.to_string()));
            process.name_row(&key, &self.name);
            self.load(ctx, process, key).await
        })
    }
}

impl RunRow {
    /// Load it there: what it injects is lent to it (on a leaf runtime),
    /// what it provides is projected from this fiber, as for a row of a
    /// runtime. A rutis kernel announces what it provides, under the key.
    async fn load(
        &self,
        ctx: &Ctx,
        process: Arc<Process>,
        key: String,
    ) -> Result<Effect, RutisError> {
        let caller = CallerRow::new(self.name.clone(), key.clone(), ctx.clone());
        let mut leases: Vec<HostLease> = Vec::new();
        for (name, inject) in self.schema.inject.iter().zip(&self.injects) {
            let Some(dispatch) = ctx.get_key::<dyn HostDispatch>(inject.clone()) else {
                continue;
            };
            if dispatch.origin() == Some(process.connection().tag()) {
                continue;
            }
            match process
                .lease_host_for(name, None, &caller, dispatch, None)
                .await
            {
                Ok(lease) => leases.push(lease),
                Err(error) => {
                    release(leases).await;
                    return Err(failed(error));
                }
            }
        }
        let projection = row_projection_of(
            &format!("row {}", self.name),
            &self.schema.provides,
            |name| host_key(name).erase(),
            |_| false,
        );
        projection.owned_by(caller);
        if let Err(error) = projection.attach(ctx, process.clone()) {
            release(leases).await;
            return Err(error);
        }
        let loaded = process
            .load_row_exporting(
                &key,
                Path::new(&self.plugin),
                self.config.clone(),
                &[],
                &[],
                &self.schema.provides,
                projection.clone(),
                &ctx.scope(),
            )
            .await;
        if let Err(error) = loaded {
            projection.close();
            release(leases).await;
            return Err(failed(error));
        }
        if let Some(refused) = projection.take_refused() {
            projection.withdraw().await;
            let _ = process.unload_row(&key).await;
            release(leases).await;
            return Err(failed(refused));
        }
        let failing = self.target.clone();
        projection.on_refused(move |why| failing.fail(why));
        Ok(Effect::AsyncDisposer(Box::new(move || {
            Box::pin(async move {
                // Its services go first, and their users stop, before the
                // plugin providing them is unloaded.
                projection.withdraw().await;
                let _ = process.unload_row(&key).await;
                release(leases).await;
                Ok(())
            })
        })))
    }
}

async fn release(leases: Vec<HostLease>) {
    for lease in leases {
        let _ = lease.release().await;
    }
}
