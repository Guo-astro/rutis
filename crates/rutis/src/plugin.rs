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
    /// Display name, for logs and diagnostics. Called synchronously at
    /// registration; a panic propagates to the caller that registers it.
    fn name(&self) -> &str;

    /// The dependencies that gate the plugin: it starts only when every one
    /// is ready (registered, its provider Active, and its readiness check,
    /// if any, passing). Called once, synchronously, at registration; a
    /// panic propagates to the caller that registers it.
    fn injects(&self) -> Vec<TypeKey> {
        Vec::new()
    }

    /// Validates the plugin's own config before it is applied. Called on
    /// each load (and in a config update's dry run). An error or a panic
    /// fails the load: a panic becomes a `PluginFailed`, and what was
    /// registered is rolled back.
    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }

    /// The plugin body: provides services, registers listeners and returns
    /// its cleanup. A panic while creating or polling the future becomes a
    /// `PluginFailed` and the load is rolled back.
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

/// A plugin factory, for config updates: each load builds a fresh plugin
/// instance from the current config.
///
/// `build` must be pure construction (no side effects, or idempotent ones):
/// `FiberView::update` calls it once for a dry run and again for the actual
/// load. The two instances need not be the same, but must be equivalent.
///
/// Dependencies are declared like [`DynPlugin::injects`]: **static**,
/// registered once at spawn and never changed. To pick dependencies by
/// config, split the plugin into several and let the config decide which
/// one to mount.
///
/// A typed build function (`Fn(&C) -> Result<P>` with `P: Plugin`) needs no
/// factory where rutis-loader's `Builtins::register` and
/// `export_plugin!(build:)` take it: they declare `P::Deps`.
pub trait PluginFactory<C: Send + Sync + 'static>: Send + Sync + 'static {
    /// Display name, for logs and diagnostics. Read once when the fiber is
    /// created; it does not change across loads.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// The dependencies that gate the plugin, as in [`DynPlugin::injects`]:
    /// static, registered once at spawn.
    fn injects(&self) -> Vec<TypeKey> {
        Vec::new()
    }

    /// Validates a config without building an instance. The first step of
    /// `update`'s dry run.
    fn validate_config(&self, _config: &C) -> Result<(), Error> {
        Ok(())
    }

    /// Builds the plugin instance. An error means the config cannot produce
    /// a usable instance; during a load it fails the load.
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
