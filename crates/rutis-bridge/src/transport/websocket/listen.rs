//! A listener: accepts connections, authenticates them, routes each to
//! the registration of the far end it proves to be, and hands it over only
//! if that registration is still the same one.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::channel::{ChannelInfo, PeerId};
use crate::{Presented, Refusal, Ticket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};

use crate::transport::websocket::connection::{self, Io};
use crate::transport::websocket::{ListenerConfig, Shared};

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

/// A running listener.
pub(crate) struct Listening {
    pub local: PeerId,
    pub addr: SocketAddr,
    task: tokio::task::AbortHandle,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) fn start(
    config: &ListenerConfig,
    shared: Arc<Shared>,
    runtime: &tokio::runtime::Handle,
) -> Result<Listening, String> {
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
            crate::transport::websocket::tls::server(tls)
                .map_err(|error| format!("listener {}: {error}", config.name))?,
        )),
        None => None,
    };
    let listener = {
        let _entered = runtime.enter();
        TcpListener::from_std(std_listener).map_err(|error| error.to_string())?
    };
    let name: Arc<str> = config.name.as_str().into();
    let path: Arc<str> = config.path.as_str().into();
    let handle = runtime.clone();
    let task = runtime.spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                continue;
            };
            let _ = tcp.set_nodelay(true);
            crate::transport::websocket::dial::keepalive(&tcp);
            let (shared, acceptor, name, path, handle2) = (
                shared.clone(),
                acceptor.clone(),
                name.clone(),
                path.clone(),
                handle.clone(),
            );
            handle.spawn(async move {
                let limit = shared.limits.handshake;
                let _ =
                    tokio::time::timeout(limit, accept(tcp, acceptor, shared, name, path, handle2))
                        .await;
            });
        }
    });
    Ok(Listening {
        local: config.local.clone(),
        addr,
        task: task.abort_handle(),
    })
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
    shared: Arc<Shared>,
    name: Arc<str>,
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
    let routed: Arc<Mutex<Option<Ticket>>> = Arc::default();
    let route = routed.clone();
    let registrations = shared.registrations.clone();
    // tungstenite fixes this callback's signature, error response included.
    #[allow(clippy::result_large_err)]
    let check = move |request: &Request, mut response: Response| {
        if request.uri().path() != &*path {
            return Err(refuse(StatusCode::NOT_FOUND, "no rutis endpoint here"));
        }
        let bearer = request
            .headers()
            .get("Authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        let presented = match (bearer, &certificate) {
            (Some(token), _) => Presented::Bearer(token),
            (None, Some(der)) => Presented::Certificate(der),
            (None, None) => {
                return Err(refuse(StatusCode::UNAUTHORIZED, "credentials required"));
            }
        };
        let ticket = match registrations.route(&name, presented) {
            Ok(ticket) => ticket,
            Err(Refusal::Unauthenticated) => {
                return Err(refuse(StatusCode::FORBIDDEN, "not accepted here"));
            }
            // Good credentials, nobody listening for them yet: retry.
            Err(Refusal::NotListening) => {
                return Err(refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "not listening for this peer yet",
                ));
            }
            Err(Refusal::Ambiguous) => {
                return Err(refuse(StatusCode::FORBIDDEN, "ambiguous credentials"));
            }
        };
        let offered: Vec<&str> = request
            .headers()
            .get_all("Sec-WebSocket-Protocol")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .collect();
        if !offered.contains(&ticket.protocol.as_str()) {
            return Err(refuse(
                StatusCode::BAD_REQUEST,
                &protocol_refusal(&ticket.protocol, &offered),
            ));
        }
        let protocol = HeaderValue::from_str(&ticket.protocol)
            .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, "invalid protocol"))?;
        response
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", protocol);
        *route.lock().unwrap() = Some(ticket);
        Ok(response)
    };
    let Ok(socket) = tokio_tungstenite::accept_hdr_async_with_config(
        io,
        check,
        Some(crate::transport::websocket::ws_config(&shared.limits)),
    )
    .await
    else {
        return;
    };
    let Some(ticket) = routed.lock().unwrap().take() else {
        return;
    };
    let channel = connection::channel(
        socket,
        ChannelInfo {
            transport: "websocket",
            peer: Some(ticket.peer.clone()),
            label: ticket.listener.clone(),
        },
        &shared.limits,
        &runtime,
        connection::Live::new(shared.clone()),
    );
    shared.track(&channel);
    if let Err(channel) = shared.registrations.hand_over(&ticket, channel) {
        channel.closer.close("registration revoked");
    }
}
