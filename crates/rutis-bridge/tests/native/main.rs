//! The bun and local-process integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod bun_channel;
mod bun_runtime;
mod concurrent_launch;
mod local_spawn;
mod local_stdio;
