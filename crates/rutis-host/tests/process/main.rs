//! The `process` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod busy_runtime;
mod launcher;
mod nodes;
mod reload;
mod runtime_exit;
mod runtimes;
mod signals;
