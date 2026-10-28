//! rutis-loader: data-driven plugin management for rutis.
//!
//! The input is ordered patch layers (the desired state); the loader drives
//! the running fiber tree towards them. Imperative edits change one editable
//! layer and are handed to a persistence hook. The loader reads and writes no
//! files. Design: `docs/design-rutis-loader-2026-10-02.md`.

//!
//! The root has what assembling a loader takes; the rest is grouped by
//! topic: [`runtime`] (plugins of other languages), [`peer`] (rows from a
//! linked node), [`report`] (what the loader's methods return), [`edit`]
//! (imperative edits and persistence) and [`resolve`] (writing a
//! resolver). [`internal`] is for rutis-host and tests, with no stability
//! promise.

pub mod backoff;
mod catalog;
pub mod config;
pub mod edit;
mod error;
mod framework;
mod loader;
mod patch;
#[cfg(feature = "peer")]
pub mod peer;
mod persist;
mod resolver;
#[cfg(feature = "runtimes")]
pub mod runtime;
mod volatile;

/// The Rust host guide's examples compile against these crates (#667).
#[cfg(all(
    doctest,
    feature = "node",
    feature = "python",
    feature = "go",
    feature = "peer"
))]
#[doc = include_str!("../../../docs/guide/rust-host.md")]
pub struct RustHostGuide;

/// The English Rust host guide's examples compile against these crates.
#[cfg(all(
    doctest,
    feature = "node",
    feature = "python",
    feature = "go",
    feature = "peer"
))]
#[doc = include_str!("../../../docs/guide/rust-host.en.md")]
pub struct RustHostGuideEn;

/// The open-host guide's examples compile against these crates (#671).
#[cfg(all(doctest, feature = "node"))]
#[doc = include_str!("../../../docs/guide/open-host.md")]
pub struct OpenHostGuide;

/// The English open-host guide's examples compile against these crates.
#[cfg(all(doctest, feature = "node"))]
#[doc = include_str!("../../../docs/guide/open-host.en.md")]
pub struct OpenHostGuideEn;

pub use catalog::ServiceCatalog;
pub use error::LoaderError;
pub use framework::RowEvent;
pub use loader::{status, Loader, LoaderOptions, LoaderPlugin};
pub use patch::{Layer, Patch};
pub use resolver::{Builtins, Chain, Resolver};

/// What the loader's methods return: reports, row and entry states,
/// failures, change notices.
pub mod report {
    pub use crate::error::{Failure, Unavailable};
    pub use crate::framework::RowEventState;
    pub use crate::loader::{
        CheckReport, Clash, ConfigStatus, CycleLink, EntryInfo, EntryStatus, InstanceInfo, Isolate,
        LoaderChanged, PendingEditDropped, Provider, ReconcileReport, Requirement, RowCheck,
        RowInfo, RowStatus, Supply, Unknown, Wait, WaitReason,
    };
}

/// Writing a resolver: what a name resolves to and how a copy is built.
pub mod resolve {
    pub use crate::catalog::{ExprScope, Expressions};
    pub use crate::resolver::{Build, Declarations, Declared, Resolved, ScopedFactory};
}

/// What rutis-host and the tests use. No stability promise.
#[doc(hidden)]
pub mod internal {
    pub use crate::loader::followed_rows;
    pub use crate::patch::{apply_patches, Composed, ComposedRow, Owner, PatchWarning};
    #[cfg(feature = "peer")]
    pub use crate::peer::{node_schema, REMOVED_EVENTS};
    #[cfg(feature = "binary")]
    pub use crate::runtime::binary::read_binary_manifest;
    #[cfg(feature = "go")]
    pub use crate::runtime::go::PLUGIN_API;
    #[cfg(feature = "runtimes")]
    pub use crate::runtime::is_runtime_name;
    #[cfg(feature = "node")]
    pub use crate::runtime::resolve_entry;
    #[cfg(feature = "swift")]
    pub use crate::runtime::swift::SWIFT_PLUGIN_API;
}
