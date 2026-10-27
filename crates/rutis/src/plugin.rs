use std::future::Future;

use crate::ctx::Ctx;
use crate::deps::Deps;
use crate::error::Error;
use crate::key::TypeKey;
use crate::{BoxFuture, Effect};

/// A plugin: what it depends on, and what it registers when they are ready.
///
/// ```
/// use std::sync::Arc;
/// use rutis::{Ctx, Plugin, Result};
///
/// struct Greeting(String);
///
/// struct Greeter;
///
/// impl Plugin for Greeter {
///     type Deps = ();
///
///     async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
///         ctx.provide(Greeting("hello".into()))?;
///         Ok(())
///     }
/// }
///
/// struct Printer;
///
/// impl Plugin for Printer {
///     type Deps = Arc<Greeting>;
///
///     async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> Result {
///         println!("{}", greeting.0);
///         Ok(())
///     }
/// }
/// ```
///
/// - `apply` runs once every required service in `Deps` is ready.
/// - What `apply` registers through `ctx` is withdrawn in reverse order when
///   the plugin stops; when `apply` fails, what it registered so far is
///   withdrawn the same way.
/// - When a required service is withdrawn or replaced, the plugin stops and
///   runs `apply` again once the service is ready.
///
/// Every `Plugin` is a [`DynPlugin`](crate::host::DynPlugin), so
/// [`Ctx::plugin`] mounts it.
pub trait Plugin: Send + Sync + 'static {
    /// What the plugin waits for and receives: `()`, `Arc<T>`,
    /// `Option<Arc<T>>`, or a tuple of up to eight of them.
    type Deps: Deps;

    /// Registers what the plugin provides, listens to and cleans up.
    fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> impl Future<Output = Result<(), Error>> + Send;

    /// The name in logs and diagnostics: the last segment of the type name.
    fn name(&self) -> &str {
        short_type_name::<Self>()
    }

    /// Checks the plugin's own configuration at load, before `apply`; an
    /// error leaves the plugin Failed.
    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// `my_crate::module::Weather<T>` → `Weather`.
fn short_type_name<T: ?Sized>() -> &'static str {
    let full = std::any::type_name::<T>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}

/// The object-safe form of a plugin: what the fiber stores and mounts.
///
/// Every [`Plugin`] is one. Implement it directly for a plugin whose
/// dependencies are only known at run time (by name, say): list them in
/// `injects` and read them with `require_at`. A boxed one is one too, so a
/// host can mount plugins chosen at run time.
pub trait DynPlugin: Send + Sync + 'static {
    /// 显示名(日志/诊断)。注册时同步调用；panic 向注册调用方传播。
    fn name(&self) -> &str;

    /// 依赖门控声明(支柱 2):全部就绪(存在 + provider Active + `check()` 通过)才启动。
    /// 注册时同步调用一次，panic 向注册调用方传播。
    fn injects(&self) -> Vec<TypeKey> {
        Vec::new()
    }

    /// 校验自持有 config(D12:validate-before-store,注册/装载期调用)。
    /// panic 在装载边界转换为 `PluginFailed` 并回滚已注册资源。
    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }

    /// 装配体:提供 0..n 服务、注册 0..n 监听、交回清理。
    /// 创建 Future 或 poll 时的 panic 均转换为 `PluginFailed` 并回滚。
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>>;

    /// Whether this plugin is part of rutis itself (a transport, a link, a
    /// runtime, the loader): it never reports a long wait or a failed
    /// start of its own (it has its own reports), and readable diagnostics
    /// leave it out. For rutis's own crates; no stability promise.
    #[doc(hidden)]
    fn platform(&self) -> bool {
        false
    }
}

impl<P: Plugin> DynPlugin for P {
    fn name(&self) -> &str {
        Plugin::name(self)
    }

    fn injects(&self) -> Vec<TypeKey> {
        crate::deps::declaration::<P::Deps>()
    }

    fn validate(&self) -> Result<(), Error> {
        Plugin::validate(self)
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let deps = P::Deps::resolve(ctx)?;
            Plugin::apply(self, ctx, deps).await?;
            Ok(Effect::Done)
        })
    }
}

/// A boxed plugin is a plugin, so a host can mount plugins chosen at run
/// time, such as a list built from configuration.
impl DynPlugin for Box<dyn DynPlugin> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn injects(&self) -> Vec<TypeKey> {
        (**self).injects()
    }

    fn validate(&self) -> Result<(), Error> {
        (**self).validate()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        (**self).apply(ctx)
    }

    fn platform(&self) -> bool {
        (**self).platform()
    }
}

/// 插件工厂(D32:配置热更新):每代从当前 config 构造插件实例。
///
/// `build` 必须是纯构造(无副作用或幂等)——`FiberView::update` 的 dry-run
/// 与实际装载各调用一次,两次产物不要求同一实例但要求等价。
///
/// 依赖声明与 [`DynPlugin::injects`] 同形:**静态**,spawn 时注册一次、终身
/// 不变(D32f)。按配置选依赖的标准形态是拆成多个插件、配置决定装哪个。
///
/// A typed build function (`Fn(&C) -> Result<P>` with `P: Plugin`) needs no
/// factory: `plugin_from`, `Builtins::register` and `export_plugin!(build:)`
/// take it and declare `P::Deps`.
pub trait PluginFactory<C: Send + Sync + 'static>: Send + Sync + 'static {
    /// 显示名(日志/诊断,fiber 创建时取用,不再随代变化)。
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// 依赖门控声明(静态,spawn 时注册一次;与 [`DynPlugin::injects`] 对称)。
    fn injects(&self) -> Vec<TypeKey> {
        Vec::new()
    }

    /// config 级校验(不构造实例;`update` 的 dry-run 第一步)。
    fn validate_config(&self, _config: &C) -> Result<(), Error> {
        Ok(())
    }

    /// 构造插件实例。失败 = config 无法产出可用实例(装载期走 `fail_load`)。
    fn build(&self, config: &C) -> Result<Box<dyn DynPlugin>, Error>;
}

/// The factory of a typed build function: its dependencies come from
/// `P::Deps`, its name from `P`.
pub(crate) struct BuildFn<B, P> {
    build: B,
    _plugin: std::marker::PhantomData<fn() -> P>,
}

impl<B, P> BuildFn<B, P> {
    pub(crate) fn new(build: B) -> Self {
        Self {
            build,
            _plugin: std::marker::PhantomData,
        }
    }
}

impl<C, B, P> PluginFactory<C> for BuildFn<B, P>
where
    C: Send + Sync + 'static,
    P: Plugin,
    B: Fn(&C) -> Result<P, Error> + Send + Sync + 'static,
{
    fn name(&self) -> &str {
        short_type_name::<P>()
    }

    fn injects(&self) -> Vec<TypeKey> {
        crate::deps::declaration::<P::Deps>()
    }

    fn build(&self, config: &C) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new((self.build)(config)?))
    }
}

/// [`BuildFn`] for other crates (rutis-loader's `Builtins::register`,
/// rutis-sdk's `export_plugin!`).
#[doc(hidden)]
pub fn build_fn<C, B, P>(build: B) -> impl PluginFactory<C>
where
    C: Send + Sync + 'static,
    P: Plugin,
    B: Fn(&C) -> Result<P, Error> + Send + Sync + 'static,
{
    BuildFn::new(build)
}
