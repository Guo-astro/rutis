//! What an application embedding rutis uses: creating and closing the
//! root, reading services by key, scopes and instances, mounting plugins
//! from configuration, fibers, diagnostics and the detailed error kinds.
//!
//! Plugin code does not need this module. The [`Ctx`] methods here come
//! from [`CtxHost`]; `use rutis::host::*;` brings them in scope.

use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

pub use crate::bus::EventSubscription;
pub use crate::effect::{Effect, EffectMeta, EffectPhase};
pub use crate::error::{
    default_sink, ErrorKind, ErrorSink, NextMisuse, ServiceReadError, ServiceReadFailure,
};
pub use crate::events::kinds::ListenerKind;
pub use crate::fiber::{
    shutdown_deadline, DisposeWaitError, FiberState, FiberStatusChanged, FiberView, PluginId,
    Snapshot, Stuck,
};
pub use crate::future::BoxFuture;
pub use crate::key::{InstanceId, Key, TypeKey};
pub use crate::plugin::{DynPlugin, PluginFactory};
pub use crate::service::CallError;

use crate::diagnostics::RuntimeDiagnostics;
use crate::effect::Disposer;
use crate::error::Error;
use crate::{Ctx, Event, Plugin};

/// The diagnostics snapshot [`CtxHost::diagnostics`] returns.
pub mod diagnostics {
    pub use crate::diagnostics::{
        BindingDiagnostics, DependencyDiagnostics, DependencyStatus, EventBacklog,
        PluginDiagnostics, ResolvedDependency, RuntimeDiagnostics, ServiceAccess, ServiceChange,
        ServiceChanged,
    };
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::Ctx {}
}

/// Names a key for messages ([`CtxHost::name_keys_with`]), when it knows it.
pub type KeyNamer = dyn Fn(&TypeKey) -> Option<String> + Send + Sync;

/// The [`Ctx`] methods of a host: `use rutis::host::*;` to call them.
pub trait CtxHost: sealed::Sealed {
    /// Identity of the fiber owning this context. Isolated contexts retain it.
    fn instance(&self) -> InstanceId;

    fn handle(&self) -> &Handle;

    /// 错误路由:插件运行期(apply 之外)的异步错误经此上报,不崩 root。
    /// 供外部插件在 turn 边界兜底(如 session 落盘失败),与框架内部
    /// 路由同一 sink——可观测,不静默。
    fn error_sink(&self) -> ErrorSink;

    /// Take completed, early-disposed effect errors owned by this fiber.
    /// `Disposer::dispose()` returns an error immediately without sending it
    /// to the error sink. Each error is returned here once; taken errors no
    /// longer appear in a later unload result or restart sink notification.
    /// Errors from effects still draining remain available to a subsequent
    /// call or to the unload that owns their completion.
    fn take_cleanup_errors(&self) -> Vec<Arc<ErrorKind>>;

    /// 自动路径:`Handle::try_current()` 失败返回明确错误,绝不隐式建 runtime(D8)。
    fn root() -> Result<Ctx, Error>
    where
        Self: Sized;

    /// 注入构造(优先路径,D8)。
    fn root_with(handle: Handle) -> Ctx
    where
        Self: Sized;

    /// 注入构造 + 自定义 ErrorSink。
    fn root_with_sink(handle: Handle, sink: ErrorSink) -> Ctx
    where
        Self: Sized;

    /// root fiber 句柄(root dispose 清子树 / root restart,§五 root_restart)。
    /// 最终 shutdown 并释放所有 `FiberView` 后返回 None。
    fn root_view(&self) -> Option<FiberView>;

    /// The root fiber's context of this context's tree: what lives as long
    /// as the root (and its event bus).
    fn root_ctx(&self) -> Ctx;

    /// The live fiber with this id anywhere under this context's root, for
    /// hosts that track plugins by [`PluginId`] (from diagnostics or events)
    /// and need to act on one.
    fn view(&self, id: crate::PluginId) -> Option<FiberView>;

    /// Whether this context's root has begun its final shutdown.
    ///
    /// Becomes true when [`Ctx::shutdown`] is called, before waiting for cleanup,
    /// and remains true after shutdown completes. All contexts under that root
    /// share this state; disposing or shutting down a child alone does not set it.
    /// Reads the same flag as [`RuntimeDiagnostics::shutting_down`] without
    /// collecting a diagnostic snapshot.
    fn is_shutting_down(&self) -> bool;

    /// After how long a plugin waiting for services nothing provides (its
    /// `Deps`, its `injects`) is reported: once each time it starts
    /// waiting, the error sink gets [`ErrorKind::StillWaiting`] naming them
    /// (by default a `printer: still waiting after 10s for …` line on
    /// stderr). rutis's own plugins (a link waiting for its peer) are not
    /// reported. 10 s by default; `None` reports nothing. For the
    /// whole tree of this context's root; a plugin already waiting keeps
    /// the setting it started waiting with.
    fn report_waiting_after(&self, after: Option<Duration>);

    /// Name keys in those reports with `namer`, for the keys it names (a
    /// loader names the services its rows wait for as their rows do); the
    /// first namer added that names a key wins, the others are described
    /// ([`TypeKey::describe`]). Kept while the caller holds `namer`.
    fn name_keys_with(&self, namer: &Arc<KeyNamer>);

    /// Read-only, best-effort snapshot of live fibers and services.
    ///
    /// This scans fibers and bindings under separate locks. Concurrent lifecycle
    /// changes may therefore mix states or generations from different moments,
    /// even within one plugin's state, dependency, and binding entries. The
    /// result is useful for diagnosis, not an atomic transaction or a change
    /// stream. Dependency checks are never called here: their status is the
    /// last result recorded by normal gate resolution, or `CheckPending` if
    /// that binding has not been checked yet. Reading does not call plugin
    /// `name`, `injects`, or other user code and does not advance a fiber.
    /// Its `Display` calls the key namers
    /// ([`name_keys_with`](Self::name_keys_with)) for the names it gives
    /// services; taking the snapshot does not.
    fn diagnostics(&self) -> RuntimeDiagnostics;

    /// 最终关闭 root。重复或并发调用共享同一完成结果；现有 `dispose`
    /// 仍可 `restart`。关闭会拒绝新注册并等待准入关闭前完成挂载的子树
    /// 及其清理。创建已开始但挂载被关闭拒绝的子 fiber 不会进入 `apply`，
    /// 会独立关闭；其返回的 `FiberView::shutdown()` 可等待该 driver 退出。
    /// root 的完成结果不包含这种未挂载子 fiber。
    /// 不协作的代码可能使等待无限延长，可用 `shutdown_with_timeout` 限制等待。
    /// It has no time limit of its own: how long to wait is the host's
    /// choice (rutis-host waits 10 s).
    fn shutdown(&self) -> crate::BoxFuture<'static, Result<(), Arc<ErrorKind>>>;

    /// 限制等待最终关闭的时间。超时后关闭仍在后台进行，重复调用
    /// `shutdown()` 可继续 join 同一结果。
    ///
    /// When the time runs out, [`DisposeWaitError::TimedOut`] names the
    /// plugins still busy and with what ([`Stuck`]): a cleanup registered
    /// with `ctx.effect` by where it was registered, a service being
    /// withdrawn by its name, the cleanup a plugin's `apply` returned (a
    /// loader row's) by the plugin or row:
    /// `not finished after 3s: weather: still running the cleanup
    /// registered at src/weather.rs:12:9`.
    fn shutdown_with_timeout(
        &self,
        limit: Duration,
    ) -> crate::BoxFuture<'static, Result<(), DisposeWaitError>>;

    /// The chain of loader instances around this context, outermost first;
    /// `[]` at the root (protocol spec §3). It is a descriptor for filters;
    /// events are delivered by name on the root's one bus.
    fn scope(&self) -> Vec<crate::Scope>;

    /// The nearest marked row of this context's fiber or its ancestors.
    fn row(&self) -> Option<String>;

    /// The isolation label under which this context reads `key`, if any
    /// (the label of its nearest [`CtxHost::isolate`] for it).
    fn isolate_label(&self, key: &TypeKey) -> Option<String>;

    /// isolate 作用域(支柱 3):按 ServiceKey 隔离,同 label 合并(TS 语义,D21);
    /// 返回的 Ctx 保留原 fiber 所有权(D28)。
    fn isolate(&self, key: impl Into<TypeKey>, label: &str) -> Ctx;

    /// 工厂模式装载(D32:配置热更新):依赖门控声明在注册时取自工厂并固定,
    /// 每代装载用当前 config 构造实例;返回的 FiberView 可 `update(config)`。
    fn plugin_with<C: Send + Sync + 'static>(
        &self,
        factory: impl PluginFactory<C>,
        config: C,
    ) -> FiberView;

    /// Close the plugin's own fiber, as cordis's `ctx.fiber.dispose()`.
    /// Returns at once; the fiber unloads after the current apply or
    /// callback returns, so never wait for it from inside the plugin. The
    /// root cannot dispose itself this way (use [`Ctx::shutdown`]).
    ///
    /// Only the generation this context belongs to can dispose: a context
    /// kept past a restart, or used while a restart is under way, gets
    /// [`ErrorKind::InactiveEffect`] and the new generation keeps running.
    fn dispose_self(&self) -> Result<(), Error>;

    /// 当前 fiber 代的取消 token(D27:每代独立 token,卸载第②步取消)。
    fn cancellation_token(&self) -> CancellationToken;

    /// Provides `value` under `key`: a name (`Key::named`), a name known at
    /// run time (`Key::dynamic`), an instance, or a trait-object type. The
    /// key carries the value type, so a mismatch does not compile.
    fn provide_at<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: Key<T>,
        value: Arc<T>,
    ) -> Result<Disposer, Error>;

    /// An optional read under `key`: no gate, no reload relation.
    fn get_at<T: ?Sized + Send + Sync + 'static>(&self, key: &Key<T>) -> Option<Arc<T>>;

    /// A strict read of a declared service of type `T`: an error at the
    /// call site when it is not declared or not available.
    fn require<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>, ServiceReadError>;

    /// A strict read under `key`. A declaration on an ancestor fiber is
    /// usable only when that ancestor sees the same isolate scope.
    fn require_at<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: &Key<T>,
    ) -> Result<Arc<T>, ServiceReadError>;

    /// Mounts a plugin built from `config` by `build`, as
    /// `Builtins::register` does; the returned view can `update(config)`,
    /// which builds a new instance with the same function (so `build` only
    /// constructs). The dependencies are `P::Deps`.
    fn plugin_from<C, P, B>(&self, build: B, config: C) -> FiberView
    where
        C: Send + Sync + 'static,
        P: Plugin,
        B: Fn(&C) -> Result<P, Error> + Send + Sync + 'static;

    /// `waterfall`, synchronously: every listener on the chain must be
    /// synchronous (`on_sync`); `terminal` is what the last one's `next`
    /// returns.
    fn waterfall_sync<E, F>(
        &self,
        event: impl crate::events::IntoEvent<E>,
        terminal: F,
    ) -> Result<Option<E::Value>, Error>
    where
        E: Event,
        F: FnOnce(Arc<E>) -> Result<Option<E::Value>, Error>;

    /// The current registrations of the root's event bus, in registration
    /// order.
    fn subscriptions(&self) -> Vec<EventSubscription>;
}
