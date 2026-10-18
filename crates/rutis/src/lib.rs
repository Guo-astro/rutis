//! rutis: Cordis 核心范式的 Rust 惯用实现。
//!
//! 五支柱(设计文档 §一):
//! 1. 插件 = 装配单元(一次 `apply`,提供服务/监听/清理)
//! 2. fiber = 生命周期容器(六态状态机 + 依赖门控 + 级联卸载 + 恰好一次清理)
//! 3. 服务 = 类型键注册表 + isolate 作用域
//! 4. 事件总线 = 四分发语义(emit/parallel/serial/waterfall)
//! 5. 依赖驱动重载(provider 卸载 → 消费者驱逐并自动重载)

#![allow(clippy::type_complexity)]

mod bus;
mod ctx;
mod diagnostics;
mod effect;
mod error;
pub mod events;
mod fiber;
mod intercept;
mod key;
mod plugin;
mod registry;
mod sync;
mod typed;

pub use bus::{DispatchAttempt, EventBus, EventSubscription, Filtered};
pub use ctx::Ctx;
pub use diagnostics::{
    BindingDiagnostics, DependencyDiagnostics, DependencyStatus, EventBacklog, PluginDiagnostics,
    ResolvedDependency, RuntimeDiagnostics, ServiceAccess, ServiceChange, ServiceChanged,
};
pub use effect::{Disposer, Effect, EffectMeta, EffectPhase};
pub use error::{
    CordisError, ErrorSink, NextMisuse, ServiceReadError, ServiceReadFailure, ServiceWriteError,
    ServiceWriteFailure,
};
pub use events::{
    Call, Event, EventOptions, Listener, ListenerInfo, ListenerKind, Mode, PatternListener, Scope,
    SyncCall, SyncListener, SyncPatternListener, Terminal,
};
pub use fiber::{DisposeWaitError, FiberState, FiberStatusChanged, FiberView, PluginId, Snapshot};
pub use intercept::{ServiceIntercept, ServiceWriter};
pub use key::{EventKey, EventPattern, InstanceId, Key, ServiceKey, TypeKey};
pub use plugin::{Plugin, PluginFactory};
pub use typed::{
    DepKey, Deps, Gate, Keyed, KeyedGate, Typed, TypedFactory, TypedPlugin, TypedPluginFactory,
};

/// serde, for deriving the `Serialize` / `Deserialize` every [`Event`] needs.
pub use serde;

/// dyn 兼容的 future 别名(与 `futures::future::BoxFuture` 同一定义,D1)。
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The development handbook's Rust examples compile against this crate.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/development-handbook.md")]
pub struct DevelopmentHandbook;
