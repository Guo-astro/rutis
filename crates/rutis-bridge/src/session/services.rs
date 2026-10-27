//! A rutis service the far end of a session can call ([`HostDispatch`]).

use std::cell::RefCell;
use std::sync::Arc;

use rutis::host::{CtxHost, Key};
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

    /// Who provides it, for messages naming it ("row prompts", "peer
    /// office"), when it says.
    fn provider(&self) -> Option<String> {
        None
    }

    /// The service as itself, for rutis's own code to recognize what it is
    /// (a [`RowService`](crate::runtime::RowService), whose plugin
    /// definitions [`crate::runtime::returned`] reads).
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// The key a host service named `name` is provided under, for example
/// `ctx.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe))`.
pub fn host_key(name: &str) -> Key<dyn HostDispatch> {
    Key::dynamic(name.to_owned())
}

/// The key of the host service `name` inside the instance subtree
/// `instance`: seen only from that subtree, so each instance has its own.
pub fn host_key_in(name: &str, instance: rutis::host::InstanceId) -> Key<dyn HostDispatch> {
    host_key(name).instance(instance)
}

/// Call `method` of the service provided by name as `name`, as `ctx` sees
/// it (`host_key(name)`), and wait for its result: a `#[rutis::service]`
/// type, a service a row of any language provides, a peer's, or a
/// hand-written [`HostDispatch`]. For hosts and tests:
///
/// ```ignore
/// let today: String = call_service(&root, "weather", "today", ("Oslo",)).await?;
/// ```
///
/// `args` are the call's arguments as a tuple (`("Oslo", 3)`, `("Oslo",)`
/// for one, `()` for none) or a list, encoded as a call from another
/// language would be ([`arg`](crate::session::arg)); the result is
/// decoded as `T` ([`decode_value`](crate::session::decode_value):
/// `serde_json::Value` for any, `()` to ignore it). A method the service
/// declares `sync` runs on a blocking thread; the others, and every method
/// of a service whose [`HostDispatch::methods`] says nothing (a
/// hand-written one returning `None`), are called as async: `invoke` runs
/// on the calling task and must not block. The call comes from no row
/// ([`caller_row`] is `None`), so a method taking the calling row's `&Ctx`
/// fails. Not provided, an unknown method or the method's own error: `Err`;
/// a sync method that panics fails the call with the panic's message.
pub async fn call_service<T>(
    ctx: &rutis::Ctx,
    name: &str,
    method: &str,
    args: impl serde::Serialize,
) -> Result<T, crate::session::Error>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    use crate::session::Error;
    let service = ctx
        .get_at::<dyn HostDispatch>(&host_key(name))
        .ok_or_else(|| Error::Remote {
            name: "ServiceNotFound".into(),
            message: format!("no service named {name} is provided"),
            graph: None,
        })?;
    let args = match crate::session::arg(&args)? {
        RpcValue::Data(Value::Null) => RpcValue::List(Vec::new()),
        args @ (RpcValue::Data(Value::Array(_)) | RpcValue::List(_)) => args,
        _ => {
            return Err(Error::Value(
                "the arguments of a call are a tuple or a list: (\"Oslo\",), or () for none".into(),
            ))
        }
    };
    let sync = service
        .methods()
        .and_then(|methods| methods.get(method)?.as_str().map(|kind| kind == "sync"))
        .unwrap_or(false);
    let reply = if sync {
        // A synchronous call waits for its answer: not on the async runtime.
        let called = method.to_owned();
        match tokio::task::spawn_blocking(move || service.invoke(&called, args)).await {
            Ok(reply) => reply,
            // As a panic in any call into a Rust service: its message.
            Err(error) if error.is_panic() => {
                Err(crate::session::rpc::panic_error(error.into_panic()))
            }
            Err(error) => Err(Error::Transport(format!("{name}.{method}: {error}"))),
        }
    } else {
        service.invoke(method, args)
    }?;
    crate::session::decode_value(crate::session::settle(reply).await?)
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
    pub fn instance(&self) -> rutis::host::InstanceId {
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
    /// The service a runtime called through its shared registration, which
    /// carries no row: one a row leases for itself does ([`unleased`]).
    static UNLEASED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The host service whose call is being dispatched on this thread when a
/// runtime made it through the registration rows share, not through a
/// row's own lease: code of a plugin that does not declare the service in
/// its `inject` (#676). Read while the call starts, as [`caller_row`].
pub(crate) fn unleased() -> Option<String> {
    UNLEASED.with(|name| name.borrow().clone())
}

/// Run `f` with `name` as [`unleased`], the previous one restored after.
pub(crate) fn with_unleased<R>(name: Option<String>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            UNLEASED.with(|name| *name.borrow_mut() = previous);
        }
    }
    let _restore = Restore(UNLEASED.with(|current| current.replace(name)));
    f()
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
