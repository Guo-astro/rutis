//! guarantee: bridge/error-report-bounded
//! An error graph from the far end, read and printed as a host does, and its
//! `data` read within time and size limits (#631,
//! `bridge/error-code-and-data b`; `rutis_bridge::fuzzing::error_graph`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| rutis_bridge::fuzzing::error_graph(data));
