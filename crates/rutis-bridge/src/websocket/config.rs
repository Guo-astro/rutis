//! What a WebSocket listener listens on, whom a dialer trusts, and the
//! limits of both.

use std::net::SocketAddr;
use std::time::Duration;

/// One listener: an address, its upgrade path, and its TLS.
#[derive(Clone, Debug)]
pub struct ListenerConfig {
    /// Its name, in errors.
    pub name: String,
    /// Port 0 picks a free port ([`crate::websocket::Listener::local_addr`]).
    pub bind: SocketAddr,
    /// The upgrade path; `/rutis` by default.
    pub path: String,
    /// Required unless `bind` is a loopback address.
    pub tls: Option<ServerTls>,
}

impl ListenerConfig {
    pub fn new(name: impl Into<String>, bind: SocketAddr) -> Self {
        Self {
            name: name.into(),
            bind,
            path: "/rutis".into(),
            tls: None,
        }
    }

    /// An absolute path, and TLS unless bound to a loopback address (a
    /// TLS-terminating reverse proxy in front talks to a loopback
    /// listener).
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.path.starts_with('/') {
            return Err(format!(
                "listener {}: path {:?} must start with /",
                self.name, self.path
            ));
        }
        if self.tls.is_none() && !self.bind.ip().is_loopback() {
            return Err(format!(
                "listener {} binds {} without TLS: only loopback listeners may go without",
                self.name, self.bind
            ));
        }
        Ok(())
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    pub fn tls(mut self, tls: ServerTls) -> Self {
        self.tls = Some(tls);
        self
    }
}

/// A listener's certificate and key, and the CA its client certificates
/// must chain to, if it accepts client certificates.
#[derive(Clone)]
pub struct ServerTls {
    pub certificate_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub client_ca_pem: Option<Vec<u8>>,
}

impl std::fmt::Debug for ServerTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerTls")
            .field("client_certificates", &self.client_ca_pem.is_some())
            .finish_non_exhaustive()
    }
}

/// The roots a dialing side verifies server certificates against.
#[derive(Clone, Debug)]
pub struct Trust {
    /// The system's root certificates.
    pub system: bool,
    /// More CA certificates, PEM.
    pub ca_pem: Vec<Vec<u8>>,
}

impl Default for Trust {
    fn default() -> Self {
        Self {
            system: true,
            ca_pem: Vec::new(),
        }
    }
}

impl Trust {
    /// Only these CAs, not the system's.
    pub fn only(ca_pem: Vec<u8>) -> Self {
        Self {
            system: false,
            ca_pem: vec![ca_pem],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Limits {
    /// The largest message either way; 16 MiB by default. Over it, the
    /// channel closes with code 1009.
    pub max_message: usize,
    /// How often each side pings; 10 s by default.
    pub ping: Duration,
    /// Silence after which the far end counts as gone; 30 s by default.
    pub timeout: Duration,
    /// How long a connection may take to be established.
    pub handshake: Duration,
    /// Bytes buffered per direction before backpressure.
    pub buffer: usize,
}

impl Limits {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.max_message == 0 {
            return Err("the message size limit must be positive".into());
        }
        if self.timeout <= self.ping {
            return Err("the heartbeat timeout must exceed the ping interval".into());
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message: 16 * 1024 * 1024,
            ping: Duration::from_secs(10),
            timeout: Duration::from_secs(30),
            handshake: Duration::from_secs(10),
            buffer: 4 * 1024 * 1024,
        }
    }
}
