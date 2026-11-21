//! The `session` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod boundary_rules;
mod call_service;
mod caller_rows;
mod cancellation;
mod error_shape;
mod grant_rollback;
mod group_mount;
mod host_services;
mod live_objects;
mod marked_services;
mod memory_session;
mod mount_host_service;
mod mount_scope;
mod multihop;
mod projection_lifecycle;
mod row_services;
mod rpc_callbacks;
mod sdk_protocol;
mod service_projection;
mod session_future_release;
mod session_plugin;
mod session_tags;
