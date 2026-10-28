//! Connect rutis to other processes, languages and machines.
//!
//! - [`runtime`]: plugins written in Node or Python, run by language
//!   runtimes on this machine ([`runtime::LocalRuntime`]) or elsewhere
//!   ([`runtime::RuntimePlugin::remote`]); rutis-loader manages them as
//!   rows.
//! - Links between rutis nodes ([`LinkPlugin`]) and what runs on them:
//!   services exported and imported, plugins hosted for a peer, events
//!   shared ([`PeerPlugin`] composes them).
//! - [`events`]: the logical event bus across runtimes and nodes.
//! - Access points ([`AccessPointPlugin`]): one listener admitting many
//!   runtimes that dial out to rutis, each by the id it greets with, as an
//!   [`Admission`] decides.
//! - What links run over: [`pipe`] (local processes and Unix sockets),
//!   [`memory`] (in-process channels) and `websocket` (feature
//!   `websocket`), each giving a [`Channel`] directly ([`pipe::spawn`],
//!   [`memory::pair`], `websocket::dial`, `websocket::listen`). An
//!   [`Identity`] holds credentials and the rules that map what a far end
//!   presents to its endpoint id.
//! - [`session`]: the protocol all of these speak, over any
//!   [`channel::Channel`].
//! - [`cordis`] (feature `cordis`): Cordis plugins mounted in a Rust
//!   application with Rust bindings generated at build time.
//!
//! Design: `docs/design-protocol-channel-decoupling-2026-10-03.md`,
//! `docs/design-remote-plugins-2026-10-03.md`.

#[cfg(feature = "exports")]
pub mod build;
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
#[cfg(feature = "testing")]
pub mod testing;
#[cfg(feature = "websocket")]
pub mod websocket;

// The fuzz targets' entry points (`fuzz/`, #179): only in a build by
// cargo-fuzz, which sets `--cfg fuzzing`.
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzzing;

mod access_point;
mod compose;
mod host;
mod identity;
mod link;
mod marked;
mod peer;
mod registration;
mod services;

pub use access_point::{
    Accept, Accepted, AccessEvent, AccessPointConfig, AccessPointPlugin, Release,
};
pub use channel::{Channel, Close, ConnectError, PeerId};
pub use compose::{Features, PeerHandle, PeerPlugin};
pub use events::LinkEventsPlugin;
pub use host::{Described, HostPlugin, Installed, PluginCatalog, ServiceKeys, StaticCatalog};
pub use identity::{
    admission_key, fingerprint, identity_key, Admission, AdmissionPlugin, Credential, Identity,
    IdentityPlugin, Presented, Rejection, Revocable, Revocations, SharedToken, StaticIdentity,
};
pub use link::{protocol, Connect, Dialer, Failure, LinkConfig, LinkPlugin, LinkState, Retry};
pub use peer::{family, peer_key, Handler, Offered, Offers, Peer};
pub use registration::{
    Accepting, Deliver, Refusal, Registered, Registration, RegistrationError, Registrations, Ticket,
};
pub use services::{ExportPlugin, ImportPlugin};

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
