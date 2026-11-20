//! The `dial` integration tests, one module per former test binary.
//!
//! The Kotlin and Swift programs these tests start come from
//! `rutis-bridge/tests`, so the helpers keep their names at this crate's root
//! and the two test files that used to be the `kotlin` and `swift` binaries
//! are modules of their own (`kotlin_host`, `swift_host`).
#[path = "../common/mod.rs"]
mod common;
#[path = "../../../rutis-bridge/tests/kotlin/mod.rs"]
mod kotlin;
#[path = "../../../rutis-bridge/tests/swift/mod.rs"]
mod swift;

mod dial_conformance;
#[path = "kotlin.rs"]
mod kotlin_host;
mod lease_e2e;
mod swift_app;
#[path = "swift.rs"]
mod swift_host;
