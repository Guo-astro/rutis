//! The locks a process's exit state is built on, and the stand-ins the
//! shuttle models swap (#622's spawn/lock/timer doubles, #623).
//!
//! The exit state (`Exit` in `pipe/spawn.rs`) is read by the thread blocked on
//! the channel, waited on by a send holding the session's writer, and set by
//! the task that waits for the process: four histories of hangs and lost exit
//! statuses (#157, #243, #246, #252). Built with `--cfg rutis_shuttle`, its
//! mutex, condition variable and `closed` flag are shuttle's, so every access
//! to them is a point where a model may switch threads; a plain build is
//! unchanged: these are std's types.
//!
//! In `pipe/spawn.rs` only the exit state takes these. Its other locks — the
//! manifest list, a process's end signal, the started processes' list — are
//! not crossed by an interleaving a model needs, and a std lock entered and
//! left without yielding is what a plain build does (the same rule as
//! `crates/rutis/src/sync.rs`). A session's call, reference, future and
//! ordered-operation tables (`session/rpc.rs`), its wait graph
//! (`session/rpc/waits.rs`) and the line framing's writer (`pipe/lines.rs`)
//! take them too, for the same reason: a model interleaves there.

#[cfg(not(rutis_shuttle))]
pub(crate) use std::sync::{
    atomic::{AtomicBool as Flag, AtomicUsize as Count},
    Condvar, Mutex,
};

#[cfg(rutis_shuttle)]
pub(crate) use shuttle::sync::{
    atomic::{AtomicBool as Flag, AtomicUsize as Count},
    Condvar, Mutex,
};
