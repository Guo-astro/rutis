//! A listener: accepts connections and lets through the upgrades its
//! admission accepts, each as a channel handed to where the admission said.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::channel::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};

use crate::websocket::connection::{self, Io};
use crate::websocket::{Limits, ListenerConfig};

/// What a dialer presents with its upgrade: a bearer token
/// (`Authorization: Bearer <token>`), or a TLS client certificate (DER),
/// at least one of them. An upgrade with neither is refused with 401
/// before any admission sees it.
#[derive(Debug, Clone, Copy)]
pub struct Upgrade<'a> {
    pub bearer: Option<&'a str>,
    pub certificate: Option<&'a [u8]>,
}

/// An upgrade let through: the session protocol it must have offered,
/// and where its channel goes once the upgrade completes.
pub struct Admitted {
    pub protocol: String,
    pub deliver: Box<dyn FnOnce(Channel) + Send>,
}

/// Why an upgrade is refused, as the dialer reads it: 403 (credentials
/// that are not accepted here; dialers retry slowly) or 503 (nothing takes
/// these credentials yet; dialers retry). The text goes into the response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Forbidden(String),
    Unavailable(String),
}

/// Decides, for each upgrade, whether it is let through and where its
/// channel goes. Called on the listener's threads: it must not block.
pub type Admit = Arc<dyn Fn(Upgrade<'_>) -> Result<Admitted, Refused> + Send + Sync>;

/// Why a dialer that offered none of this endpoint's session protocol is
/// refused, saying which end to upgrade (standard Q6.2.4) when the
/// versions it offered (`rutis.<n>`) are all newer (this endpoint) or all
/// older (the dialer) than this endpoint's.
fn protocol_refusal(ours: &str, offered: &[&str]) -> String {
    let version = |protocol: &str| protocol.strip_prefix("rutis.")?.parse::<u32>().ok();
    let theirs: Vec<(&str, u32)> = offered
        .iter()
        .filter_map(|protocol| Some((*protocol, version(protocol)?)))
        .collect();
    let mut reason = format!("this endpoint speaks {ours}");
    let Some(ours) = version(ours).filter(|_| !theirs.is_empty()) else {
        return reason;
    };
    let names: Vec<&str> = theirs.iter().map(|(name, _)| *name).collect();
    reason.push_str(&format!(", the dialer offered {}", names.join(", ")));
    if theirs.iter().all(|(_, n)| *n > ours) {
        reason.push_str(": upgrade this endpoint (Rust crates rutis-host / rutis-bridge)");
    } else if theirs.iter().all(|(_, n)| *n < ours) {
        reason.push_str(": upgrade the dialer");
    }
    reason
}

/// A running listener; dropping it stops listening (the channels it handed
/// over stay open).
pub struct Listener {
    addr: SocketAddr,
    task: tokio::task::AbortHandle,
}

impl Listener {
    /// The address it is bound to (to learn a picked port).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) fn start(
    config: &ListenerConfig,
    limits: &Limits,
    admit: Admit,
    runtime: &tokio::runtime::Handle,
) -> Result<Listener, String> {
    let limits = Arc::new(limits.clone());
    let std_listener = std::net::TcpListener::bind(config.bind).map_err(|error| {
        let hint = match error.kind() {
            std::io::ErrorKind::AddrInUse => {
                ": another program listens there: stop it, or change the address"
            }
            _ => "",
        };
        format!(
            "listener {}: cannot bind {}: {error}{hint}",
            config.name, config.bind
        )
    })?;
    std_listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let addr = std_listener
        .local_addr()
        .map_err(|error| error.to_string())?;
    let acceptor = match &config.tls {
        Some(tls) => Some(TlsAcceptor::from(
            crate::websocket::tls::server(tls)
                .map_err(|error| format!("listener {}: {error}", config.name))?,
        )),
        None => None,
    };
    let listener = {
        let _entered = runtime.enter();
        TcpListener::from_std(std_listener).map_err(|error| error.to_string())?
    };
    let path: Arc<str> = config.path.as_str().into();
    let handle = runtime.clone();
    let task = runtime.spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                continue;
            };
            let _ = tcp.set_nodelay(true);
            crate::websocket::dial::keepalive(&tcp);
            let (limits, admit, acceptor, path, handle2) = (
                limits.clone(),
                admit.clone(),
                acceptor.clone(),
                path.clone(),
                handle.clone(),
            );
            handle.spawn(async move {
                let limit = limits.handshake;
                let accepting = accept(tcp, acceptor, limits, admit, path, handle2);
                let _ = tokio::time::timeout(limit, accepting).await;
            });
        }
    });
    Ok(Listener {
        addr,
        task: task.abort_handle(),
    })
}

/// What a dialer's upgrade request presents, read from its headers.
pub(crate) struct UpgradeRequest<'a> {
    /// The token of an `Authorization: Bearer <token>` header.
    pub bearer: Option<&'a str>,
    /// The session protocols offered: every `Sec-WebSocket-Protocol`
    /// header, split at commas.
    pub offered: Vec<&'a str>,
}

/// Read an upgrade request to `path`; one to another path is refused.
#[allow(clippy::result_large_err)] // tungstenite's error response
pub(crate) fn read_upgrade<'a>(
    request: &'a Request,
    path: &str,
) -> Result<UpgradeRequest<'a>, ErrorResponse> {
    if request.uri().path() != path {
        return Err(refuse(StatusCode::NOT_FOUND, "no rutis endpoint here"));
    }
    let bearer = request
        .headers()
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let offered = request
        .headers()
        .get_all("Sec-WebSocket-Protocol")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    Ok(UpgradeRequest { bearer, offered })
}

/// The `Sec-WebSocket-Protocol` response header for this endpoint's session
/// protocol `ours`, if the dialer offered it; else the refusal saying which
/// end to upgrade.
#[allow(clippy::result_large_err)] // tungstenite's error response
pub(crate) fn agree_protocol(ours: &str, offered: &[&str]) -> Result<HeaderValue, ErrorResponse> {
    if !offered.contains(&ours) {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            &protocol_refusal(ours, offered),
        ));
    }
    HeaderValue::from_str(ours)
        .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, "invalid protocol"))
}

fn refuse(status: StatusCode, reason: &str) -> ErrorResponse {
    let mut response = ErrorResponse::new(Some(reason.to_owned()));
    *response.status_mut() = status;
    response
}

/// How long, and for how many bytes, a refused TLS connection waits for
/// the dialer to close after the alert.
const LINGER: Duration = Duration::from_secs(2);
const LINGER_BYTES: usize = 64 * 1024;

/// Closes a connection whose TLS handshake failed so that the dialer reads
/// the alert (already written by the acceptor) and can tell an
/// authentication failure from a network one.
///
/// Under TLS 1.3 the dialer's handshake finishes before this side checks
/// its certificate, so the dialer has already sent its upgrade request.
/// Dropping a socket with that request unread resets the connection, and
/// on Windows the reset reaches the dialer before the alert, which it then
/// never reads (#300). So: send FIN after the alert, then read and discard
/// until the dialer closes, bounded in time and bytes.
async fn linger(mut tcp: TcpStream) {
    let _ = tcp.shutdown().await;
    let _ = tokio::time::timeout(LINGER, async {
        let mut buffer = [0u8; 4096];
        let mut left = LINGER_BYTES;
        while left > 0 {
            match tcp.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => left = left.saturating_sub(read),
            }
        }
    })
    .await;
}

async fn accept(
    tcp: TcpStream,
    acceptor: Option<TlsAcceptor>,
    limits: Arc<Limits>,
    admit: Admit,
    path: Arc<str>,
    runtime: tokio::runtime::Handle,
) {
    let (io, certificate): (Box<dyn Io>, Option<Vec<u8>>) = match acceptor {
        Some(acceptor) => match acceptor.accept(tcp).into_fallible().await {
            Ok(tls) => {
                let certificate = tls
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|chain| chain.first())
                    .map(|leaf| leaf.as_ref().to_vec());
                (Box::new(tls), certificate)
            }
            Err((_, tcp)) => return linger(tcp).await,
        },
        None => (Box::new(tcp), None),
    };
    type Deliver = Box<dyn FnOnce(Channel) + Send>;
    let admitted: Arc<Mutex<Option<Deliver>>> = Arc::default();
    let route = admitted.clone();
    // tungstenite fixes this callback's signature, error response included.
    #[allow(clippy::result_large_err)]
    let check = move |request: &Request, mut response: Response| {
        let upgrade = read_upgrade(request, &path)?;
        if upgrade.bearer.is_none() && certificate.is_none() {
            return Err(refuse(StatusCode::UNAUTHORIZED, "credentials required"));
        }
        let presented = Upgrade {
            bearer: upgrade.bearer,
            certificate: certificate.as_deref(),
        };
        let Admitted { protocol, deliver } = admit(presented).map_err(|refused| match refused {
            Refused::Forbidden(reason) => refuse(StatusCode::FORBIDDEN, &reason),
            Refused::Unavailable(reason) => refuse(StatusCode::SERVICE_UNAVAILABLE, &reason),
        })?;
        let protocol = agree_protocol(&protocol, &upgrade.offered)?;
        response
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", protocol);
        *route.lock().unwrap() = Some(deliver);
        Ok(response)
    };
    let Ok(socket) = tokio_tungstenite::accept_hdr_async_with_config(
        io,
        check,
        Some(crate::websocket::ws_config(&limits)),
    )
    .await
    else {
        return;
    };
    let Some(deliver) = admitted.lock().unwrap().take() else {
        return;
    };
    deliver(connection::channel(socket, &limits, &runtime));
}
