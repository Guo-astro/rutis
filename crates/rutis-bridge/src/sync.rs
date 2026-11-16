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
//! Only the exit state takes these. The other locks in `pipe/spawn.rs` — the
//! manifest list, a process's end signal, the started processes' list — are
//! not crossed by an interleaving a model needs, and a std lock entered and
//! left without yielding is what a plain build does (the same rule as
//! `crates/rutis/src/sync.rs`).

#[cfg(not(rutis_shuttle))]
pub(crate) use std::sync::{atomic::AtomicBool as Flag, Condvar, Mutex};

#[cfg(rutis_shuttle)]
pub(crate) use shuttle::sync::{atomic::AtomicBool as Flag, Condvar, Mutex};
