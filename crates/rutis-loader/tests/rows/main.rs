//! The `rows` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod bad_values;
mod check;
mod instance_services;
mod instances;
mod lifecycle;
mod loader;
mod migration_example;
mod patch_parity;
mod regressions;
mod reload;
mod scope;

mod config_rows;
mod edit_pipeline;
mod event_close_admission;
mod fault_containment;
mod fault_texts;
mod listener_scale;
mod loader_service;
mod marked_services;
mod optional_deps;
mod optional_deps_go;
mod patch_random_parity;
mod plugin_limits;
mod plugin_values;
mod row_events;
mod row_inject_gate;
mod session_rows;
mod sessions_open;
mod settled;
mod undeclared;
mod virtual_clock_contract;
mod waits;
