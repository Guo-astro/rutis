//! Channels between machines over WebSocket
//! (`docs/design-protocol-channel-decoupling-2026-10-03.md`, WebSocket
//! binding).
//!
//! [`dial`] connects to a listener; [`listen`] accepts connections, each
//! upgrade let through as its [`Admit`] decides. Their connections run on
//! threads of their own, so channels progress whatever the caller's
//! executor is doing.
//!
//! [`WebSocketPlugin`] provides `Transport#websocket`. It dials
//! `ws://` (loopback only) and `wss://` addresses once per call, and holds
//! the listeners links register on. Each connection carries one channel of
//! UTF-8 JSON text messages; the session protocol is its subprotocol.
//! Bearer tokens travel in the `Authorization` header, client certificates
//! in TLS; an [`Identity`](crate::Identity) maps either to the far
//! end's endpoint id. Both sides ping every 10 s and drop a far end silent
//! for 30 s. The transport runs on its own threads, so channels progress
//! whatever the caller's executor is doing.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::channel::{Channel, Close, ConnectError};
use crate::{
    transport_key, Credential, Dial, Presented, Refusal, Registered, Registration,
    RegistrationError, Registrations, Transport,
};
use rustls::RootCertStore;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::{Ctx, Error};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

mod config;
mod connection;
mod dial;
pub(crate) mod listen;
mod pipe;
mod tls;

pub use config::{Config, Limits, ListenerConfig, ServerTls, Trust};
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
    let task = threads().spawn(async move {
        dial::dial(
            &address,
            &protocol,
            credential.as_ref(),
            roots,
            &limits,
            threads(),
            None,
        )
        .await
        .map(|(channel, _)| channel)
    });
    AbortOnDrop(task).finish().await
}

/// Listen as `config` says (its `local` is not used here), letting through
/// the upgrades `admit` accepts. Fails if the address cannot be bound or
/// the TLS configuration is invalid.
pub fn listen(config: &ListenerConfig, limits: &Limits, admit: Admit) -> Result<Listener, String> {
    let hooks = Arc::new(listen::Hooks {
        admit,
        limits: limits.clone(),
        live: None,
        track: None,
    });
    listen::start(config, hooks, threads())
}

/// Provides `Transport#websocket`, configured with listeners, trust and
/// limits. Unloading closes its listeners, registrations and channels.
/// Clones share the running transport ([`WebSocketPlugin::transport`]).
#[derive(Clone)]
pub struct WebSocketPlugin {
    config: Config,
    started: Arc<Mutex<Option<Arc<WebSocketTransport>>>>,
}

impl WebSocketPlugin {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            config,
            started: Arc::default(),
        })
    }

    /// The running transport, while the plugin is applied.
    pub fn transport(&self) -> Option<Arc<WebSocketTransport>> {
        self.started.lock().unwrap().clone()
    }
}

impl DynPlugin for WebSocketPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "rutis-bridge/websocket"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let transport = WebSocketTransport::start(self.config.clone()).map_err(Error::msg)?;
            *self.started.lock().unwrap() = Some(transport.clone());
            let started = self.started.clone();
            let stopping = transport.clone();
            ctx.effect(move || {
                async move {
                    started.lock().unwrap().take();
                    stopping.stop();
                    // Let the close frames go out before the threads stop.
                    stopping.closed(std::time::Duration::from_secs(1)).await;
                    Ok(())
                }
            })?;
            ctx.provide_at(transport_key("websocket"), transport)?;
            Ok(Effect::Done)
        })
    }
}

/// What listeners and connections share.
pub(crate) struct Shared {
    pub registrations: Registrations,
    pub limits: Limits,
    open: Mutex<Vec<Weak<connection::Shared>>>,
    /// Connection tasks still running.
    pub live: std::sync::atomic::AtomicUsize,
    pub ended: tokio::sync::Notify,
}

impl Shared {
    pub(crate) fn track(&self, connection: Weak<connection::Shared>) {
        let mut open = self.open.lock().unwrap();
        open.retain(|connection| connection.strong_count() > 0);
        open.push(connection);
    }
}

pub(crate) fn ws_config(limits: &Limits) -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(limits.max_message))
        .max_frame_size(Some(limits.max_message))
}

/// The transport a [`WebSocketPlugin`] provides.
pub struct WebSocketTransport {
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    handle: tokio::runtime::Handle,
    shared: Arc<Shared>,
    roots: Arc<RootCertStore>,
    listeners: Mutex<HashMap<String, (crate::channel::PeerId, Listener)>>,
}

impl WebSocketTransport {
    /// Start the transport's threads and bind its listeners.
    pub fn start(config: Config) -> Result<Arc<Self>, String> {
        config.validate()?;
        // Before the runtime: returning with it, in an async context, would
        // panic as it is dropped.
        let roots = tls::roots(&config.trust)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("rutis-websocket")
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let handle = runtime.handle().clone();
        let shared = Arc::new(Shared {
            registrations: Registrations::new(),
            limits: config.limits.clone(),
            open: Mutex::default(),
            live: Default::default(),
            ended: Default::default(),
        });
        let mut listeners = HashMap::new();
        for listener in &config.listeners {
            let listening = listen::start(listener, Self::hooks(&shared, listener), &handle);
            match listening {
                Ok(listening) => {
                    listeners.insert(listener.name.clone(), (listener.local.clone(), listening));
                }
                Err(error) => {
                    drop(listeners);
                    runtime.shutdown_background();
                    return Err(error);
                }
            }
        }
        Ok(Arc::new(Self {
            runtime: Mutex::new(Some(runtime)),
            handle,
            shared,
            roots,
            listeners: Mutex::new(listeners),
        }))
    }

    /// How a listener of the transport lets upgrades through: to the
    /// registration of the far end the credentials verify, handed over if
    /// it is still the same one.
    fn hooks(shared: &Arc<Shared>, listener: &ListenerConfig) -> Arc<listen::Hooks> {
        let name = listener.name.clone();
        let registrations = shared.registrations.clone();
        let admit: Admit = Arc::new(move |upgrade: Upgrade<'_>| {
            let presented = match (upgrade.bearer, upgrade.certificate) {
                (Some(token), _) => Presented::Bearer(token),
                (None, Some(der)) => Presented::Certificate(der),
                (None, None) => return Err(Refused::Forbidden("not accepted here".into())),
            };
            let ticket = match registrations.route(&name, presented) {
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
        });
        let tracked = shared.clone();
        Arc::new(listen::Hooks {
            admit,
            limits: shared.limits.clone(),
            live: Some(shared.clone()),
            track: Some(Arc::new(move |connection| tracked.track(connection))),
        })
    }

    /// The address listener `name` is bound to (to learn a picked port).
    pub fn local_addr(&self, name: &str) -> Option<SocketAddr> {
        self.listeners
            .lock()
            .unwrap()
            .get(name)
            .map(|(_, listener)| listener.local_addr())
    }

    /// Wait until every connection task has finished, at most `limit`.
    pub async fn closed(&self, limit: std::time::Duration) {
        let all_ended = async {
            loop {
                let ended = self.shared.ended.notified();
                if self.shared.live.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    return;
                }
                ended.await;
            }
        };
        let _ = tokio::time::timeout(limit, all_ended).await;
    }

    /// Stop listening, revoke every registration and close every channel.
    pub fn stop(&self) {
        self.listeners.lock().unwrap().clear();
        self.shared.registrations.clear();
        for connection in std::mem::take(&mut *self.shared.open.lock().unwrap()) {
            connection::going_away(&connection);
        }
    }
}

impl Drop for WebSocketTransport {
    fn drop(&mut self) {
        self.stop();
        if let Some(runtime) = self.runtime.lock().unwrap().take() {
            runtime.shutdown_background();
        }
    }
}

/// Aborts the task whose result is awaited if the awaiting stops.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> AbortOnDrop<Result<T, ConnectError>> {
    /// The dial's result.
    async fn finish(mut self) -> Result<T, ConnectError> {
        (&mut self.0)
            .await
            .map_err(|error| ConnectError::Retryable {
                reason: format!("dial stopped: {error}"),
            })?
    }
}

impl Transport for WebSocketTransport {
    fn kind(&self) -> &str {
        "websocket"
    }

    fn dial<'a>(&'a self, request: &'a Dial) -> BoxFuture<'a, Result<Channel, ConnectError>> {
        let request = request.clone();
        let roots = self.roots.clone();
        let limits = self.shared.limits.clone();
        let handle = self.handle.clone();
        let live = connection::Live::new(self.shared.clone());
        let task = self.handle.spawn(async move {
            let credential = match (&request.identity, &request.peer) {
                (Some(identity), Some(peer)) => identity.credential(peer),
                _ => None,
            };
            dial::dial(
                &request.address,
                &request.protocol,
                credential.as_ref(),
                roots,
                &limits,
                &handle,
                Some(live),
            )
            .await
        });
        let task = AbortOnDrop(task);
        Box::pin(async move {
            let (channel, connection) = task.finish().await?;
            self.shared.track(connection);
            Ok(channel)
        })
    }

    fn register(&self, registration: Registration) -> Result<Registered, RegistrationError> {
        let local = self
            .listeners
            .lock()
            .unwrap()
            .get(&registration.listener)
            .map(|(local, _)| local.clone())
            .ok_or_else(|| {
                RegistrationError::NoListener(format!(
                    "no listener named {}",
                    registration.listener
                ))
            })?;
        self.shared.registrations.register(&local, registration)
    }
}
