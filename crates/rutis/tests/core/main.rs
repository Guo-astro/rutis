//! The `core` integration tests, one module per former test binary.

mod contract;
mod parity;
mod interleave;
mod load_window;
mod evicted_apply;
mod lifecycle_order;
mod terminal_cleanup;
mod failed_retry;
mod lifecycle_diagnostics;
mod generation_registration;
mod transient_release;
mod still_waiting;
mod cleanup_errors;
mod config_update;
mod config_cases;
mod host_control;
mod unload_deadline;
mod strict_reads;
mod definitions;
