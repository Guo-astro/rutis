//! Channels between machines over WebSocket
//! (`docs/design-protocol-channel-decoupling-2026-10-03.md`, WebSocket
//! binding).
//!
//! [`dial`] connects to a listener: `ws://` (loopback only) and `wss://`
//! addresses, once per call. [`listen`] accepts connections, each upgrade
//! let through as its [`Admit`] decides. Each connection carries one
//! channel of UTF-8 JSON text messages; the session protocol is its
//! subprotocol. Bearer tokens travel in the `Authorization` header, client
//! certificates in TLS. Both sides ping every 10 s and drop a far end
//! silent for 30 s. Connections run on threads of their own, so channels
//! progress whatever the caller's executor is doing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use crate::channel::{Channel, Close, ConnectError};
use crate::{Credential, Presented, Refusal, Registrations};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

mod config;
mod connection;
mod dial;
pub(crate) mod listen;
mod pipe;
mod tls;

pub use config::{Limits, ListenerConfig, ServerTls, Trust};
pub use listen::{Admit, Admitted, Listener, Refused, Upgrade};

/// The threads the connections of [`dial`] and [`listen`] run on, started
/// on first use.
fn threads() -> &'static tokio::runtime::Handle {
    static THREADS: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    THREADS
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("rutis-websocket")
                .enable_all()
                .build()
                .expect("the WebSocket threads start")
        })
        .handle()
}

/// Connection tasks still running, and a wake-up each time one ends.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static ENDED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Wait until every connection's task has ended, at most `limit`: what a
/// host that stops does, so that the close frames its channels sent go
/// out before the process exits.
pub async fn closed(limit: std::time::Duration) {
    let all_ended = async {
        loop {
            let ended = ENDED.notified();
            if LIVE.load(Ordering::SeqCst) == 0 {
                return;
            }
            ended.await;
        }
    };
    let _ = tokio::time::timeout(limit, all_ended).await;
}

/// Dial `address` (`wss://`, or `ws://` to a loopback address), offering
/// session protocol `protocol` (none when empty) and presenting
/// `credential`. The error's category says whether to retry
/// (`bridge/dial-out-rules`): from the failure's type or the listener's
/// status, never from text.
pub async fn dial(
    address: &str,
    protocol: &str,
    credential: Option<&Credential>,
    trust: &Trust,
    limits: &Limits,
) -> Result<Channel, ConnectError> {
    let roots = tls::roots(trust).map_err(|reason| ConnectError::Incompatible { reason })?;
    let (address, protocol, credential, limits) = (
        address.to_owned(),
        protocol.to_owned(),
        credential.cloned(),
        limits.clone(),
    );
    let mut task = AbortOnDrop(threads().spawn(async move {
        dial::dial(
            &address,
            &protocol,
            credential.as_ref(),
            roots,
            &limits,
            threads(),
        )
        .await
    }));
    (&mut task.0)
        .await
        .map_err(|error| ConnectError::Retryable {
            reason: format!("dial stopped: {error}"),
        })?
}

/// Listen as `config` says, letting through the upgrades `admit` accepts.
/// Fails if the configuration breaks a rule (an absolute path; TLS unless
/// bound to a loopback address, for a TLS-terminating reverse proxy in
/// front; a positive message limit; a heartbeat timeout longer than the
/// ping interval), or the address cannot be bound.
pub fn listen(config: &ListenerConfig, limits: &Limits, admit: Admit) -> Result<Listener, String> {
    config.validate()?;
    limits.validate()?;
    listen::start(config, limits, admit, threads())
}

/// Let through the upgrades `registrations` routes on `listener`: to the
/// registration of the far end the credentials verify, handed over if it
/// is still the same one. Transitional (#695 S2): goes with links and
/// access points.
#[doc(hidden)]
pub fn admit_registered(registrations: Registrations, listener: &str) -> Admit {
    let listener = listener.to_owned();
    Arc::new(move |upgrade: Upgrade<'_>| {
        let presented = match (upgrade.bearer, upgrade.certificate) {
            (Some(token), _) => Presented::Bearer(token),
            (None, Some(der)) => Presented::Certificate(der),
            (None, None) => return Err(Refused::Forbidden("not accepted here".into())),
        };
        let ticket = match registrations.route(&listener, presented) {
            Ok(ticket) => ticket,
            Err(Refusal::Unauthenticated) => {
                return Err(Refused::Forbidden("not accepted here".into()))
            }
            // Good credentials, nobody listening for them yet: retry.
            Err(Refusal::NotListening) => {
                return Err(Refused::Unavailable(
                    "not listening for this peer yet".into(),
                ))
            }
            Err(Refusal::Ambiguous) => {
                return Err(Refused::Forbidden("ambiguous credentials".into()))
            }
        };
        let registrations = registrations.clone();
        Ok(Admitted {
            protocol: ticket.protocol.clone(),
            deliver: Box::new(move |channel| {
                if let Err(channel) = registrations.hand_over(&ticket, channel) {
                    channel.closer.close(Close::Done);
                }
            }),
        })
    })
}

pub(crate) fn ws_config(limits: &Limits) -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(limits.max_message))
        .max_frame_size(Some(limits.max_message))
}

/// Aborts the task whose result is awaited if the awaiting stops.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
