//! The `core` integration tests, one module per former test binary.

mod cleanup_errors;
mod config_cases;
mod config_update;
mod contract;
mod definitions;
mod evicted_apply;
mod failed_retry;
mod generation_registration;
mod host_control;
mod interleave;
mod lifecycle_diagnostics;
mod lifecycle_order;
mod load_window;
mod parity;
mod still_waiting;
mod strict_reads;
mod terminal_cleanup;
mod transient_release;
mod unload_deadline;
