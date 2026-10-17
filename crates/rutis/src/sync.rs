//! The sync primitives the shuttle models swap (#179, `shuttle_tests.rs`).
//!
//! Built with `--cfg rutis_shuttle`, the two spots whose interleavings the
//! models explore use shuttle's primitives, so every access to them is a
//! point where shuttle may switch threads: a fiber driver's `alive` flag
//! (`post_join` against the driver's exit) and an effect record's state
//! (claiming its cleanups). Everything else keeps std's. A plain build is
//! unchanged: these are std's types.

#[cfg(not(rutis_shuttle))]
pub(crate) use std::sync::{atomic::AtomicBool, Mutex};

#[cfg(rutis_shuttle)]
pub(crate) use shuttle::sync::{atomic::AtomicBool, Mutex};

/// A point where shuttle may switch threads, in a window a model must be
/// able to stop in that no shuttle primitive opens (shuttle switches just
/// before an operation on its primitives, not after). Nothing in a plain
/// build.
#[inline(always)]
pub(crate) fn switch_point() {
    #[cfg(rutis_shuttle)]
    shuttle::thread::yield_now();
}
