//! The `core` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod author_api;
mod cleanup_errors;
mod compile_errors;
mod config_cases;
mod config_update;
mod contract;
mod definitions;
mod dispatch_chain_probe;
mod embedding_output;
mod event_backlog;
mod event_order;
mod events;
mod evicted_apply;
mod failed_retry;
mod generation_registration;
mod host_control;
mod instance_subtrees;
mod interleave;
mod lifecycle_diagnostics;
mod lifecycle_order;
mod listener_table_model;
mod load_window;
mod marked_generations;
mod parity;
mod service_emits;
mod service_writer;
mod still_waiting;
mod strict_reads;
mod terminal_cleanup;
mod transient_release;
mod typed_dispatch;
mod typed_plugin;
mod unload_deadline;
mod waterfall_sync;
