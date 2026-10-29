//! Connect rutis to other processes, languages and machines.
//!
//! - [`runtime`]: plugins written in Node or Python, run by language
//!   runtimes on this machine ([`runtime::LocalRuntime`]); rutis-loader
//!   manages them as rows.
//! - [`Session`]: the far end of a [`Channel`] (a runtime, an app, a node)
//!   as plugins of this kernel.
//!   A far rutis kernel is a session's far end too: the services of both
//!   are seen on both sides, events are one bus, and each runs the other's
//!   plugins from its [`PluginCatalog`].
//! - [`events`]: the logical event bus across runtimes and nodes.
//! - What sessions run over: [`pipe`] (local processes),
//!   [`memory`] (in-process channels) and `websocket` (feature
//!   `websocket`), each giving a [`Channel`] directly ([`pipe::spawn`],
//!   [`memory::pair`], `websocket::dial`, `websocket::listen`).
//! - [`session`]: the protocol all of these speak, over any
//!   [`channel::Channel`].
//! - [`cordis`] (feature `cordis`): Cordis plugins mounted in a Rust
//!   application with Rust bindings generated at build time.
//!
//! Design: `docs/design-protocol-channel-decoupling-2026-10-03.md`,
//! `docs/design-remote-session-2026-10-28.md`.

pub mod channel;
#[cfg(feature = "cordis")]
pub mod cordis;
pub mod events;
pub mod exports;
pub mod memory;
pub mod pipe;
pub mod report;
pub mod runtime;
pub mod session;
#[cfg(feature = "websocket")]
pub mod websocket;

// The fuzz targets' entry points (`fuzz/`, #179): only in a build by
// cargo-fuzz, which sets `--cfg fuzzing`.
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzzing;

mod catalog;
mod identity;
mod marked;
mod peer;
mod services;

pub use catalog::{Installed, PluginCatalog, StaticCatalog};
pub use channel::{Channel, Close, ConnectError, PeerId};
pub use identity::fingerprint;
pub use session::plugin::{
    catalog_key, session_key, Greeted, LocalId, RowHandle, Session, SessionHandle,
};

/// The error another language threw, inside a rutis error: a row that
/// failed, a call to a plugin's service that failed. Gives its name, code,
/// data, stack and site (`docs/design-diagnostics-2026-10-26.md` §4.2), so
/// a host can say it in its own words by code instead of matching text.
/// `None` for errors rutis or Rust code raised. Takes a `rutis::Error`
/// (`&error`) or the `ErrorKind` a row's state keeps.
pub fn thrown(error: &rutis::host::ErrorKind) -> Option<&session::Error> {
    fn within<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a session::Error> {
        if let Some(remote) = error.downcast_ref::<session::Error>() {
            return matches!(remote, session::Error::Remote { .. }).then_some(remote);
        }
        if let Some(call) = error.downcast_ref::<rutis::host::CallError>() {
            if let Some(remote) = call
                .raw()
                .and_then(|raw| raw.downcast_ref::<session::Error>())
            {
                return matches!(remote, session::Error::Remote { .. }).then_some(remote);
            }
        }
        if let Some(kind) = error.downcast_ref::<rutis::host::ErrorKind>() {
            return of_kind(kind);
        }
        error.source().and_then(within)
    }
    fn of_kind(kind: &rutis::host::ErrorKind) -> Option<&session::Error> {
        match kind {
            rutis::host::ErrorKind::PluginFailed(source) => within(source.as_ref()),
            _ => None,
        }
    }
    of_kind(error)
}
