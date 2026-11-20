//! The `events` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod event_hub;
mod fault_links;
mod local_soak;
mod trace_validation;
mod websocket_soak;
