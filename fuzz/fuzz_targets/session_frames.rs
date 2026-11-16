//! A far end's frames in sequence, into a live session (#631): the greeting,
//! invocations and what they grant, calls and reads of references, releases,
//! cancellations and replies (`rutis_bridge::fuzzing::session_frames`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| rutis_bridge::fuzzing::session_frames(data));
