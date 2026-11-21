//! The `lang` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;
#[path = "../kotlin/mod.rs"]
mod kotlin;

mod dsh_baseline;
mod event_hub;
mod events_expected;
mod fault_links;
mod go_runtime;
mod kotlin_remote;
mod kotlin_runtime;
mod local_soak;
mod objects_expected;
mod python_runtime;
mod python_values;
mod runtime_conformance;
mod swift_remote;
mod swift_runtime;
mod tagged_values;
mod trace_validation;
mod value_roundtrip;
mod websocket_soak;
