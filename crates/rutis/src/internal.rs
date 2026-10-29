//! What rutis's own crates (rutis-bridge, rutis-loader, rutis-dylib,
//! rutis-sdk, rutis-macros, rutis-host) use among themselves. No stability
//! promise: anything here may change in any release. If code outside this
//! repository needs something here, open an issue to move it to
//! [`crate::host`].
#![doc(hidden)]

use std::sync::Arc;

pub use crate::bus::EventBus;
pub use crate::error::{ServiceWriteError, ServiceWriteFailure};
pub use crate::events::kinds::{EventOptions, RESERVED_PREFIX};
pub use crate::key::EventKey;
pub use crate::plugin::build_fn;
pub use crate::writer::ServiceWriter;
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
        RemoteEntry, RemoteListener, RemoteNext, RemoteSyncCall, TableChange, TableEntry,
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

    /// The event bus: one per root. Dispatch does not filter by isolate scope.
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

    /// The strict read a marked service's entry by name makes on every call
    /// ([`crate::service::Entry::resolve`]): [`CtxHost::require_at`](crate::host::CtxHost::require_at) by the
    /// providing context, of the binding `provision` provided only.
    fn require_provided_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        provision: &crate::service::Provision,
    ) -> Result<Arc<T>, ServiceReadError>;

    /// Register a provider-owned mutable binding and return a generation-bound
    /// writer. A write replaces the registered Arc, not existing Arc snapshots.
    fn provide_mut_at<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        value: Arc<T>,
    ) -> Result<(Disposer, ServiceWriter<T>), Error>;

    /// Registers a cleanup: `f` runs at once, and the cleanup it returns runs at
    /// unload, last registered first. Fails with `InactiveEffect` when the fiber
    /// is unloading or disposed.
    fn effect_raw(&self, f: impl FnOnce() -> Effect) -> Result<Disposer, Error>;

    /// Register a cleanup with a label: a shutdown that runs out of time
    /// names it in its Stuck hint.
    fn effect_named(
        &self,
        label: impl Into<String>,
        f: impl FnOnce() -> Effect,
    ) -> Result<Disposer, Error>;

    /// The five ways to send under `name` instead of the event's own:
    /// what a typed event dispatched under a name chosen at run time needs
    /// (plugin code uses a type that overrides `Event::name`).
    fn named(&self, name: impl Into<std::sync::Arc<str>>) -> crate::events::Filtered<'_>;
}
