//! Helpers shared by the integration tests that check, on the paused
//! clock, that something is still waiting; they live in rutis-test-support
//! with the other wait helpers (#231). Not every test file uses every one.
#![allow(unused_imports)]

pub use rutis_test_support::{on_thread, still_pending};
