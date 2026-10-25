//! What an application's `build.rs` calls (feature `exports`): [`Exports`]
//! writes the TypeScript declarations of the Rust services marked
//! `#[rutis::service]`, generated from the Rust source, on every build.

mod crates;
mod exports;

pub use exports::Exports;
