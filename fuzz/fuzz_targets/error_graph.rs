//! An error graph from the far end, read and printed as a host does
//! (`rutis_bridge::fuzzing::error_graph`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| rutis_bridge::fuzzing::error_graph(data));
