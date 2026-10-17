//! A session frame from the far end: depth check, parse, value decoding,
//! and the frame and its tagged values written back and read again
//! (`rutis_bridge::fuzzing::session_frame`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| rutis_bridge::fuzzing::session_frame(data));
