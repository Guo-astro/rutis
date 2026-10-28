//! The TypeScript declarations and Python types of the Rust services marked
//! `#[rutis::service]`, generated from the Rust source: [`Exports`], called
//! from an application's `build.rs` on every build, and by `rutis-host types`.

mod crates;
mod exports;

pub use exports::Exports;
