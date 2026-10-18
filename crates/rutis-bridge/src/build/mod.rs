//! What an application's `build.rs` calls (feature `exports`): [`Exports`]
//! lists existing Rust services to export by name to the rows of other
//! languages, and generates their dispatchers and TypeScript declarations
//! from the Rust source. The generated module is declared with
//! [`include_exports!`](crate::include_exports) and served through
//! [`crate::exports`].

mod crates;
mod exports;

pub use exports::Exports;
