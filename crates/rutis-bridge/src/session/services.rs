//! A rutis service the far end of a session can call ([`HostDispatch`]).

use std::cell::RefCell;
use std::sync::Arc;

use rutis::TypeKey;
use serde_json::Value;

use crate::session::rpc::{Reply, Value as RpcValue};

/// A rutis service provided to the mounted Cordis plugins: the Node side
/// registers a proxy under `name` whose calls arrive here. While `invoke`
/// runs, [`caller_row`] names the runtime row that called, if a row did.
pub trait HostDispatch: Send + Sync + 'static {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply;

    /// The methods as `{ method: "sync" | "async" }`, when the service knows
    /// them; otherwise whoever registers it with a runtime supplies them.
    fn methods(&self) -> Option<Value> {
        None
    }

    /// The runtime session whose plugin serves this service, when it is
    /// one ([`RowService`](crate::runtime::RowService)), as its
    /// [`Connection::tag`](crate::session::Connection::tag): a row of that
    /// same session uses the plugin natively instead of through a proxy. A
    /// tag names one session of one runtime instance, wherever it runs.
    fn origin(&self) -> Option<&str> {
        None
    }
}

/// The key a host service named `name` is provided under, for example
/// `ctx.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe))`.
pub fn host_key(name: &str) -> TypeKey {
    TypeKey::keyed_dynamic::<dyn HostDispatch>(name.to_owned())
}

/// The key of the host service `name` inside the instance subtree
/// `instance`: seen only from that subtree, so each instance has its own.
pub fn host_key_in(name: &str, instance: rutis::InstanceId) -> TypeKey {
    host_key(name).with_instance(instance)
}

/// The row a call into a [`HostDispatch`] comes from ([`caller_row`]): a
/// plugin that rutis-loader runs as a row of a language runtime, whose
/// fiber here (its [`Ctx`](rutis::Ctx)) lives exactly as long as the row.
///
/// An effect registered on [`CallerRow::ctx`] is the row's own: it is
/// removed when the row unloads or restarts, while its runtime keeps
/// running, before the row's plugin is unloaded there.
#[derive(Clone)]
pub struct CallerRow(Arc<CallerRowInner>);

struct CallerRowInner {
    id: String,
    key: String,
    scope: Option<String>,
    ctx: rutis::Ctx,
}

impl CallerRow {
    /// The row `id` (its loader's id for it), loaded into its runtime as
    /// `key`, whose fiber here has `ctx`.
    pub fn new(id: impl Into<String>, key: impl Into<String>, ctx: rutis::Ctx) -> Self {
        Self(Arc::new(CallerRowInner {
            id: id.into(),
            key: key.into(),
            scope: None,
            ctx,
        }))
    }

    /// This row, using the service in the scope `scope` (see
    /// [`CallerRow::scope`]).
    pub fn in_scope(&self, scope: Option<&str>) -> Self {
        Self(Arc::new(CallerRowInner {
            id: self.0.id.clone(),
            key: self.0.key.clone(),
            scope: scope.map(str::to_owned),
            ctx: self.0.ctx.clone(),
        }))
    }

    /// The row's id, as its loader names it. Copies of one row in several
    /// instances of an instanced group share it; [`CallerRow::key`] and
    /// [`CallerRow::instance`] tell them apart.
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// The row's key in its runtime (`rows.load`): one per row and
    /// generation, so a restarted row has a new one.
    pub fn key(&self) -> &str {
        &self.0.key
    }

    /// The scope the row uses the service in: the label it isolates the
    /// service's name under (inside an instance, the instance's,
    /// `rutis-loader/instance/<id>`), or `None` outside any.
    pub fn scope(&self) -> Option<&str> {
        self.0.scope.as_deref()
    }

    /// The identity of the row's fiber (`ctx().instance()`), which it
    /// keeps across restarts.
    pub fn instance(&self) -> rutis::InstanceId {
        self.0.ctx.instance()
    }

    /// The row's fiber.
    pub fn ctx(&self) -> &rutis::Ctx {
        &self.0.ctx
    }
}

impl std::fmt::Debug for CallerRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallerRow")
            .field("id", &self.0.id)
            .field("key", &self.0.key)
            .field("scope", &self.0.scope)
            .field("instance", &self.0.ctx.instance())
            .finish()
    }
}

thread_local! {
    static CALLER_ROW: RefCell<Option<CallerRow>> = const { RefCell::new(None) };
}

/// The row whose call into a [`HostDispatch`] is being dispatched on this
/// thread: set while [`HostDispatch::invoke`] runs for a call a runtime
/// row made; `None` for a call made from Rust, from a peer node, or by a
/// runtime outside any row. Read it in `invoke` itself: a future that
/// `invoke` returns runs later and does not see it.
pub fn caller_row() -> Option<CallerRow> {
    CALLER_ROW.with(|row| row.borrow().clone())
}

/// Run `f` with `row` as [`caller_row`], the previous one restored after.
pub(crate) fn with_caller_row<R>(row: Option<CallerRow>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<CallerRow>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            CALLER_ROW.with(|row| *row.borrow_mut() = previous);
        }
    }
    let _restore = Restore(CALLER_ROW.with(|current| current.replace(row)));
    f()
}
