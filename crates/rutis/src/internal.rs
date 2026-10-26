//! What rutis's own crates (rutis-bridge, rutis-loader, rutis-dylib,
//! rutis-sdk, rutis-macros, rutis-host) use among themselves. No stability
//! promise: anything here may change in any release. If code outside this
//! repository needs something here, open an issue to move it to
//! [`crate::host`].
#![doc(hidden)]

use crate::Ctx;
use std::sync::Arc;

pub use crate::bus::{DispatchAttempt, EventBus};
pub use crate::error::{ServiceWriteError, ServiceWriteFailure};
pub use crate::events::kinds::{EventOptions, RESERVED_PREFIX};
pub use crate::intercept::{ServiceIntercept, ServiceWriter};
pub use crate::key::{EventKey, EventPattern};
pub use crate::plugin::build_fn;
/// What `#[rutis::service]` blocks and the bridge exchange.
pub mod service {
    pub use crate::service::*;
}

/// Plugin definitions another language handed over (rutis-bridge).
pub mod definitions {
    pub use crate::definition::{Source, WeakDefinition};
    use crate::events::Scope;
    use crate::host::BoxFuture;
    use crate::{Ctx, Definition};

    /// A definition built by `source`, belonging to `origin`'s generation;
    /// with no origin, to a process, ended with [`end`]. Copies mount inside
    /// `scope`, or the origin's scope chain when `scope` is `None`.
    pub fn from_source(
        origin: Option<&Ctx>,
        scope: Option<Vec<Scope>>,
        name: String,
        source: Box<dyn Source>,
    ) -> Definition {
        Definition::from_source(origin, scope, name, source)
    }

    /// How `definition` builds its copies.
    pub fn source(definition: &Definition) -> &dyn Source {
        definition.source()
    }

    /// End `definition` now: its copies end, then it has ended.
    pub fn end(definition: &Definition) -> BoxFuture<'static, ()> {
        crate::definition::end(definition)
    }
}
pub use rutis_macros::sdk_service as __sdk_service;

use crate::effect::{Disposer, Effect};
use crate::error::{Error, ServiceReadError};
use crate::key::TypeKey;

/// The event bus internals: remote entries, the wire values, raw dispatch.
pub mod events {
    pub use crate::events::remote::{
        BatchFilter, Candidate, Dispatch, EntryCall, FilterAnswer, RawTerminal, RemoteCall,
        RemoteEntry, RemoteListener, RemoteNext, RemoteSyncCall, RemoteSyncNext, TableChange,
        TableEntry,
    };
    pub use crate::events::wire::{Codec, JsonTranscoder, Payload, PayloadRef, Transcoder, Value};
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::Ctx {}
}

/// The [`crate::Ctx`] methods rutis's own crates use.
pub trait CtxInternal: sealed::Sealed {
    /// False when `key` names an instance this context is not inside (its
    /// fiber is neither that instance's fiber nor a descendant of it).
    fn in_instance_key(&self, key: &TypeKey) -> bool;

    /// 事件总线(全局唯一;事件分发不跨 isolate 过滤,D29)。
    fn events(&self) -> &EventBus;

    /// Make this context's fiber a scope of `group`: the loader calls it
    /// for each group instance before its rows start. Its id is the fiber's
    /// [`InstanceId`](crate::host::InstanceId). Marking it again with the same group is a no-op.
    fn mark_scope(&self, group: &str) -> Result<(), Error>;

    /// Make this context's fiber the row `key`: the descriptors
    /// ([`ListenerInfo::row`](crate::events::ListenerInfo::row)) of listeners registered
    /// in it and below it name the row. Marking it again with the same key
    /// is a no-op.
    fn mark_row(&self, key: &str) -> Result<(), Error>;

    /// 带显式 key 的读取(多实例,shaku Keyed 模式)。
    fn get_key<T: ?Sized + Send + Sync + 'static>(&self, key: impl Into<TypeKey>)
        -> Option<Arc<T>>;

    /// Strict keyed read. A declaration on an ancestor fiber is usable only
    /// when that ancestor sees the same isolate scope as this context.
    fn require_key<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
    ) -> Result<Arc<T>, ServiceReadError>;

    /// The strict read a marked service's entry by name makes on every call
    /// ([`crate::service::Entry::resolve`]): [`CtxHost::require_at`](crate::host::CtxHost::require_at) by the
    /// providing context, of the binding `provision` provided only, with
    /// read interceptors selected for `actor` (the calling row; this
    /// context when there is none), as for a Rust reader in its place.
    fn require_provided_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        provision: &crate::service::Provision,
        actor: Option<&Ctx>,
    ) -> Result<Arc<T>, ServiceReadError>;

    /// trait 对象 / 共享实例注册入口。
    fn provide_key<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        value: Arc<T>,
    ) -> Result<Disposer, Error>;

    /// 带 `check()` 谓词的注册(§四:check 门控保留)。
    ///
    /// `check` runs whenever a consumer's gate is evaluated, so at least
    /// twice per load: at the gate and again once the consumer's
    /// dependencies are recorded, before its plugin callbacks run.
    fn provide_checked<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        value: Arc<T>,
        check: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Disposer, Error>;

    /// Register a provider-owned mutable binding and return a generation-bound
    /// writer. A write replaces the registered Arc, not existing Arc snapshots.
    fn provide_mut_at<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        value: Arc<T>,
    ) -> Result<(Disposer, ServiceWriter<T>), Error>;

    /// 注册清理效应(D23):`f` 立即执行,返回的清理在卸载时 LIFO 执行。
    /// fiber 已 Disposed/Unloading 时返回 `InactiveEffect`(§四:重入报错)。
    fn effect_raw(&self, f: impl FnOnce() -> Effect) -> Result<Disposer, Error>;

    /// Register a cleanup with a label visible through [`FiberView::effects`](crate::host::FiberView::effects).
    fn effect_named(
        &self,
        label: impl Into<String>,
        f: impl FnOnce() -> Effect,
    ) -> Result<Disposer, Error>;

    /// 触发依赖重查(check() 谓词结果变更等场景)。
    fn refresh(&self);

    /// `ctx.filter` with a predicate that can fail: its error fails the
    /// dispatch before any call (in `emit`: reported, nothing delivered).
    fn try_filter(
        &self,
        keep: impl Fn(&crate::events::ListenerInfo) -> Result<bool, Error> + Send + Sync + 'static,
    ) -> crate::events::Filtered<'_>;

    /// The five ways to send under `name` instead of the event's own:
    /// what a typed event dispatched under a name chosen at run time needs
    /// (plugin code uses a type that overrides `Event::name`).
    fn named(&self, name: impl Into<std::sync::Arc<str>>) -> crate::events::Filtered<'_>;

    /// Intercept only strict reads under `key`, after the existing
    /// declaration, visibility and readiness checks. `get_key` stays an
    /// unhooked locator.
    fn intercept_require_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        hook: impl Fn(Arc<T>) -> ServiceIntercept<T> + Send + Sync + 'static,
    ) -> Result<Disposer, Error>;

    /// Intercept a provider's update of a binding made with
    /// [`provide_mut_at`](Self::provide_mut_at).
    fn intercept_set_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        hook: impl Fn(Arc<T>) -> ServiceIntercept<T> + Send + Sync + 'static,
    ) -> Result<Disposer, Error>;

    /// [`try_filter`](Self::try_filter) and [`named`](Self::named)
    /// together: under `name`, to the listeners `keep` accepts (a filtered
    /// dispatch of an event with no name of its own, such as rutis-bridge's
    /// `Args`).
    fn try_filter_named(
        &self,
        name: impl Into<std::sync::Arc<str>>,
        keep: impl Fn(&crate::events::ListenerInfo) -> Result<bool, Error> + Send + Sync + 'static,
    ) -> crate::events::Filtered<'_>;
}
