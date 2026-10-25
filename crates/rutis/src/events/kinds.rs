//! Listener registration options and descriptions: what hosts and rutis's
//! own crates see of the listener table.

use serde::{Deserialize, Serialize};

/// The reserved namespace of framework events (protocol spec §8). Plugins
/// listen to them; only rutis emits them.
pub const RESERVED_PREFIX: &str = "rutis/";

/// Whether a listener is known to be synchronous (protocol spec §4.2
/// `kind`). Rust `on` listeners are `Async`, `on_sync` ones `Sync`; `Any`
/// only comes from remote listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenerKind {
    Sync,
    Async,
    Any,
}

/// Registration options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EventOptions {
    /// Take a position before every existing listener.
    pub prepend: bool,
    /// Called at most once: the first dispatch that selects it claims it.
    pub once: bool,
    /// Custom filters skip it.
    pub global: bool,
}

impl EventOptions {
    pub const fn prepend(mut self, prepend: bool) -> Self {
        self.prepend = prepend;
        self
    }

    pub const fn once(mut self, once: bool) -> Self {
        self.once = once;
        self
    }

    pub const fn global(mut self, global: bool) -> Self {
        self.global = global;
        self
    }
}
