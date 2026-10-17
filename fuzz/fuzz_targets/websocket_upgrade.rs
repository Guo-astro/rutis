//! A WebSocket upgrade request read by the listening side: tungstenite's
//! handshake and the listener's reading of path, bearer token and session
//! protocols (`rutis_bridge::fuzzing::websocket_upgrade`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| rutis_bridge::fuzzing::websocket_upgrade(data));
