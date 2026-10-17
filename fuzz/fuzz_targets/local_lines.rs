//! The local channels' line framing with a message limit, reading a stream
//! that arrives in pieces (`rutis_bridge::fuzzing::local_lines`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| rutis_bridge::fuzzing::local_lines(data));
