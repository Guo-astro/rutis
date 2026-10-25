//! rutis: Cordis 核心范式的 Rust 惯用实现。
//!
//! A plugin is one trait, [`Plugin`]: what it depends on (`type Deps`) and
//! what it registers when they are ready (`async fn apply`). What it
//! registers through [`Ctx`] (services, listeners, cleanups, child plugins)
//! is withdrawn when it stops; when a service it depends on is replaced, it
//! starts again.
//!
//! ```
//! use std::sync::Arc;
//! use rutis::{Ctx, Plugin, Result};
//!
//! struct Greeting(String);
//!
//! struct Printer;
//!
//! impl Plugin for Printer {
//!     type Deps = Arc<Greeting>;
//!
//!     async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> Result {
//!         println!("{}", greeting.0);
//!         Ok(())
//!     }
//! }
//! ```
//!
//! The root holds what plugin authors use. Applications embedding rutis
//! also use [`host`]; the hidden `internal` module is for rutis's own
//! crates.
//!
//! 五支柱(设计文档 §一):
//! 1. 插件 = 装配单元(一次 `apply`,提供服务/监听/清理)
//! 2. fiber = 生命周期容器(六态状态机 + 依赖门控 + 级联卸载 + 恰好一次清理)
//! 3. 服务 = 类型键注册表 + isolate 作用域
//! 4. 事件总线 = 一张监听器表、五种分发(emit/parallel/serial/waterfall/bail),按名字与作用域
//! 5. 依赖驱动重载(provider 卸载 → 消费者驱逐并自动重载)

#![allow(clippy::type_complexity)]

mod bus;
mod ctx;
mod definition;
mod deps;
mod diagnostics;
mod effect;
mod error;
pub mod events;
mod fiber;
mod future;
pub mod host;
mod intercept;
pub mod internal;
mod key;
mod plugin;
mod registry;
mod service;
mod sync;

pub use ctx::Ctx;
pub use definition::Definition;
pub use deps::Deps;
pub use effect::Disposer;
pub use error::{Error, Result};
pub use events::Event;
pub use plugin::Plugin;

/// Serve a type's methods to the plugins of other languages by name.
///
/// On an inherent `impl` block: its public `&self` methods are callable
/// from rows of every language, as the service named after the type in
/// camelCase (`Calendar` is `calendar`) or `#[service(name = "...")]`,
/// wherever and as long as the type is provided ([`Ctx::provide`]). A
/// method marked `#[service(skip)]` is left out. See the Rust host guide,
/// "Provide Rust services to plugins".
pub use rutis_macros::service;

/// serde, for deriving the `Serialize` / `Deserialize` every [`Event`] needs.
pub use serde;

pub(crate) use future::BoxFuture;

// Crate-internal shorthands for the paths the kernel modules use; the
// public paths are the root above, `host` and `internal`.
#[allow(unused_imports)]
pub(crate) use diagnostics::{DependencyStatus, EventBacklog, ServiceChange, ServiceChanged};
#[allow(unused_imports)]
pub(crate) use effect::Effect;
#[allow(unused_imports)]
pub(crate) use error::{ErrorKind, ServiceReadFailure};
#[allow(unused_imports)]
pub(crate) use events::{Mode, Scope};
#[allow(unused_imports)]
pub(crate) use fiber::{FiberState, FiberView, PluginId};
#[allow(unused_imports)]
pub(crate) use key::{EventKey, InstanceId, TypeKey};
#[allow(unused_imports)]
pub(crate) use plugin::{DynPlugin, PluginFactory};

/// The development handbook's Rust examples compile against this crate.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/development-handbook.md")]
pub struct DevelopmentHandbook;

/// The Rust plugin guide's examples compile against this crate.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/guide/rust-plugin.md")]
pub struct RustPluginGuide;

/// The English Rust plugin guide's examples compile against this crate.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/guide/rust-plugin.en.md")]
pub struct RustPluginGuideEn;

/// The README's Rust examples compile against this crate.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
pub struct Readme;

/// The Chinese README's Rust examples compile against this crate.
#[cfg(doctest)]
#[doc = include_str!("../../../README.zh-CN.md")]
pub struct ReadmeZhCn;
