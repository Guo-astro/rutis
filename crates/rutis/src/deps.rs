//! What a [`Plugin`](crate::Plugin) depends on: one description, its
//! `type Deps`, gives both the gate declaration and the values `apply`
//! receives, so they cannot drift apart.

use crate::internal::CtxInternal;
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::error::{Error, ErrorKind, ServiceReadError, ServiceReadFailure};
use crate::key::TypeKey;

mod sealed {
    pub trait Sealed {}
}

/// What `type Deps` can be. Authors do not implement it.
///
/// | `type Deps =` | Gates on | Passed |
/// |---|---|---|
/// | `()` | nothing | `()` |
/// | `Arc<T>` | `T` | the service |
/// | `Option<Arc<T>>` | nothing | the service if visible at load |
/// | a tuple of up to eight | all members | each value |
///
/// `T` may be a trait object (`Arc<dyn Llm>`). An optional dependency is a
/// snapshot taken when `apply` starts: it does not gate, and its arrival or
/// departure does not reload the plugin. A required dependency withdrawn
/// after the gate opened but before `apply` reads it returns the plugin to
/// Pending. A plugin that depends on services by name implements
/// [`DynPlugin`](crate::host::DynPlugin) instead.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a plugin's dependency: `type Deps` lists services as `Arc<T>`",
    label = "not `()`, `Arc<T>`, `Option<Arc<T>>` or a tuple of them",
    note = "to depend on the service `{Self}`, write `Arc<{Self}>` (or `Option<Arc<{Self}>>` if the plugin can run without it)",
    note = "`type Deps` is `()`, `Arc<T>`, `Option<Arc<T>>`, or a tuple of up to eight of those"
)]
pub trait Deps: sealed::Sealed + Sized + Send + 'static {
    /// Appends the keys that gate the plugin.
    #[doc(hidden)]
    fn injects(out: &mut Vec<TypeKey>);

    /// Reads the values; called when `apply` starts, after the gate opened.
    #[doc(hidden)]
    fn resolve(ctx: &Ctx) -> Result<Self, Error>;
}

/// The gate declaration of `D`, each key once.
pub(crate) fn declaration<D: Deps>() -> Vec<TypeKey> {
    let mut all = Vec::new();
    D::injects(&mut all);
    let mut injects = Vec::with_capacity(all.len());
    for key in all {
        if !injects.contains(&key) {
            injects.push(key);
        }
    }
    injects
}

/// A required read: a dependency withdrawn after the gate opened is
/// reported as such, so the load returns to Pending.
fn require<T: ?Sized + Send + Sync + 'static>(ctx: &Ctx, key: TypeKey) -> Result<Arc<T>, Error> {
    ctx.require_key::<T>(key)
        .map_err(|error: ServiceReadError| match error.reason {
            ServiceReadFailure::Unavailable(_) => {
                Error::from(ErrorKind::InjectUnsatisfied(vec![error.key.describe()]))
            }
            _ => error.into(),
        })
}

impl sealed::Sealed for () {}
impl Deps for () {
    fn injects(_: &mut Vec<TypeKey>) {}

    fn resolve(_: &Ctx) -> Result<Self, Error> {
        Ok(())
    }
}

impl<T: ?Sized + Send + Sync + 'static> sealed::Sealed for Arc<T> {}
impl<T: ?Sized + Send + Sync + 'static> Deps for Arc<T> {
    fn injects(out: &mut Vec<TypeKey>) {
        out.push(TypeKey::of::<T>());
    }

    fn resolve(ctx: &Ctx) -> Result<Self, Error> {
        require(ctx, TypeKey::of::<T>())
    }
}

impl<T: ?Sized + Send + Sync + 'static> sealed::Sealed for Option<Arc<T>> {}
impl<T: ?Sized + Send + Sync + 'static> Deps for Option<Arc<T>> {
    fn injects(_: &mut Vec<TypeKey>) {}

    fn resolve(ctx: &Ctx) -> Result<Self, Error> {
        Ok(ctx.get_key::<T>(TypeKey::of::<T>()))
    }
}

macro_rules! tuple_deps {
    ($($name:ident),+) => {
        impl<$($name: Deps),+> sealed::Sealed for ($($name,)+) {}
        impl<$($name: Deps),+> Deps for ($($name,)+) {
            fn injects(out: &mut Vec<TypeKey>) {
                $($name::injects(out);)+
            }

            fn resolve(ctx: &Ctx) -> Result<Self, Error> {
                Ok(($($name::resolve(ctx)?,)+))
            }
        }
    };
}

tuple_deps!(A);
tuple_deps!(A, B);
tuple_deps!(A, B, C);
tuple_deps!(A, B, C, D);
tuple_deps!(A, B, C, D, E);
tuple_deps!(A, B, C, D, E, F);
tuple_deps!(A, B, C, D, E, F, G);
tuple_deps!(A, B, C, D, E, F, G, H);
