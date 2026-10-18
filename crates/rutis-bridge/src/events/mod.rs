//! The logical event bus across processes and nodes (#464,
//! `docs/design-events-2026-10-18.md`, protocol
//! `docs/design-events-protocol-2026-10-18.md`).
//!
//! All listeners on one host form one bus: the kernel's listener table
//! holds this host's Rust listeners, an entry for each listener of every
//! runtime process (registered by the runtime, `events.on`), and an entry
//! for each listener of a linked node that the link imports. A runtime
//! dispatches through the host whenever a name has listeners outside its
//! process (`events.dispatch`); the host invokes each entry elsewhere with
//! one `events.call`. Results are the same as if every listener were in one
//! process.
//!
//! - [`Args`]: an event by name with session values as arguments.
//! - [`LinkEventsPlugin`]: the events of a node link.
//! - [`ServiceEvent`], [`RuntimeEvent`]: framework events (`rutis/*`).

mod args;
mod errors;
mod framework;
mod hub;
mod link;

pub use args::{ArgValue, Args};
pub use framework::{RuntimeEvent, RuntimeEventState, ServiceEvent};
pub use link::LinkEventsPlugin;

pub(crate) use errors::to_session as to_session_error;
pub(crate) use framework::{runtime as emit_runtime, service as emit_service};
pub(crate) use hub::SessionEvents;
