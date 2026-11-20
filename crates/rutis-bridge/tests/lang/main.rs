//! The `lang` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;
#[path = "../kotlin/mod.rs"]
mod kotlin;

mod dsh_baseline;
mod events_expected;
mod go_runtime;
mod kotlin_remote;
mod kotlin_runtime;
mod objects_expected;
mod python_runtime;
mod python_values;
mod runtime_conformance;
mod swift_remote;
mod swift_runtime;
mod tagged_values;
mod value_roundtrip;
