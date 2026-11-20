//! The `platform` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;
#[path = "../kotlin/mod.rs"]
mod kotlin;
#[path = "../swift/mod.rs"]
mod swift;

mod embedder_exit;
mod host_gone;
mod local_loopback;
mod local_runtime;
mod process_exit;
mod session_matrix;
mod websocket;
mod websocket_channel;
mod websocket_cross;
