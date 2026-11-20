//! The `runtimes` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod instance_runtimes;
mod multilang;
mod python_rows;
mod runtime_exit;
mod slow_runtime;
mod status;
