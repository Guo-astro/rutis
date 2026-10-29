//! rutis-loader: data-driven plugin management for rutis.
//!
//! The input is ordered patch layers (the desired state); the loader drives
//! the running fiber tree towards them. Imperative edits change one editable
//! layer and are handed to a persistence hook. The loader reads and writes no
//! files. Design: `docs/design-rutis-loader-2026-10-02.md`.

//!
//! The root has what assembling a loader takes; the rest is grouped by
//! topic: [`runtime`] (plugins of other languages), [`report`] (what the
//! loader's methods return), [`edit`] (imperative edits and persistence),
//! [`patch`] (composing layers of rows) and [`resolve`] (writing a
//! resolver).

mod catalog;
pub mod edit;
mod error;
mod framework;
mod loader;
pub mod patch;
mod persist;
mod resolver;
#[cfg(feature = "runtimes")]
pub mod runtime;
mod volatile;

pub use catalog::ServiceCatalog;
pub use error::LoaderError;
pub use framework::RowEvent;
#[doc(hidden)]
pub use loader::followed_rows;
pub use loader::{status, Loader, LoaderOptions, LoaderPlugin};
pub use patch::{Layer, Patch};
pub use resolver::{Builtins, Chain, Resolver};

/// What the loader's methods return: reports, row and entry states,
/// failures, change notices.
pub mod report {
    pub use crate::error::{Failure, Unavailable};
    pub use crate::framework::RowEventState;
    pub use crate::loader::{
        cannot_reload, CheckReport, Clash, ConfigStatus, CycleLink, EntryInfo, EntryStatus,
        InstanceInfo, Isolate, LoaderChanged, PendingEditDropped, Provider, ReconcileReport,
        Requirement, RowCheck, RowInfo, RowStatus, Supply, Unknown, Wait, WaitReason,
    };
}

/// Writing a resolver: what a name resolves to and how a copy is built.
pub mod resolve {
    pub use crate::catalog::{ExprScope, Expressions};
    pub use crate::resolver::{Build, Declarations, Declared, Resolved, ScopedFactory};
}
