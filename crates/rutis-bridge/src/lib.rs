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
//!   `websocket`). Each gives a [`Channel`] directly ([`pipe::spawn`],
//!   [`memory::pair`], `websocket::dial`, `websocket::listen`) and has a
//!   plugin that provides a [`Transport`] under [`transport_key`]; links
//!   depend on that and never on a concrete transport. An [`Identity`]
//!   holds credentials and the rules that map what a far end presents to
//!   its endpoint id.
//! - [`session`]: the protocol all of these speak, over any
//!   [`channel::Channel`].
//! - [`cordis`] (feature `cordis`): Cordis plugins mounted in a Rust
//!   application with Rust bindings generated at build time.
//!
//! Design: `docs/design-protocol-channel-decoupling-2026-10-03.md`,
//! `docs/design-remote-plugins-2026-10-03.md`.

use std::sync::Arc;

use rutis::host::BoxFuture;

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
    IdentityPlugin, Presented, Rejection, SharedToken, StaticIdentity,
};
pub use link::{protocol, Connect, Failure, LinkConfig, LinkPlugin, LinkState, Retry};
pub use peer::{family, peer_key, Handler, Offered, Offers, Peer};
pub use registration::{
    Deliver, Refusal, Registered, Registration, RegistrationError, Registrations, Ticket,
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

/// One kind of carrier, as its plugin provides it.
pub trait Transport: Send + Sync + 'static {
    /// `"local"`, `"memory"`, `"websocket"`, …: the key it is provided under.
    fn kind(&self) -> &str;

    /// Establish one channel. Each call reports one result and never
    /// retries: whoever dials owns the retry policy, decided by the
    /// [`ConnectError`] category.
    fn dial<'a>(&'a self, dial: &'a Dial) -> BoxFuture<'a, Result<Channel, ConnectError>>;

    /// Accept the channels of one far end on a listener this transport
    /// holds. The registration lasts until the returned handle is dropped.
    /// Transports without listeners refuse.
    fn register(&self, registration: Registration) -> Result<Registered, RegistrationError> {
        let _ = registration;
        Err(RegistrationError::NoListener(format!(
            "the {} transport does not listen",
            self.kind()
        )))
    }
}

/// What to dial.
#[derive(Clone, Default)]
pub struct Dial {
    /// In the transport's own syntax (`unix:/path`, `wss://host/rutis`, …).
    pub address: String,
    /// The endpoint expected at the far end; bound to the channel once the
    /// transport has verified it (for TLS, the server certificate).
    pub peer: Option<PeerId>,
    /// Credentials to present.
    pub identity: Option<Arc<dyn Identity>>,
    /// The session protocol to speak (`rutis.2`), for transports that
    /// negotiate one (a WebSocket subprotocol); a mismatch is
    /// [`ConnectError::Incompatible`].
    pub protocol: String,
}

impl Dial {
    pub fn address(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            ..Self::default()
        }
    }

    pub fn peer(mut self, peer: PeerId) -> Self {
        self.peer = Some(peer);
        self
    }

    pub fn identity(mut self, identity: Arc<dyn Identity>) -> Self {
        self.identity = Some(identity);
        self
    }

    pub fn protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocol = protocol.into();
        self
    }
}

/// The key a transport of `kind` is provided under (`Transport#local`).
pub fn transport_key(kind: &str) -> rutis::host::Key<dyn Transport> {
    rutis::host::Key::dynamic(kind.to_owned())
}
