//! A calendar: plain Rust services and serde types, in a crate that knows
//! nothing of rutis-bridge or of other languages. The application that
//! depends on it exports [`Calendar`] and [`Notes`] to its rows by naming
//! them in its `build.rs` (see `examples/rust-services`).

mod calendar;
pub mod model;

pub use calendar::{Calendar, Notes};
pub use model::*;
