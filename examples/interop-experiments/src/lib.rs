//! Bindings for the experiment plugins; see `tests/crash.rs` and
//! `src/bin/bench.rs`.
// build.rs generates the mounts on Unix only.
#[cfg(unix)]
rutis_bridge::include_mounts!();
