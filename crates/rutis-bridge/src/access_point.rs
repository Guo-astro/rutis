//! An access point: one listener registration that admits many runtime
//! connections.
//!
//! Runtimes dial out to rutis. Each presents a credential and greets with
//! an id of its own; the [`Admission`] decides whom the access point admits
//! and as which connection id. Connections with different ids coexist. A
//! newer connection with the id of a current one takes over: the old one
//! is closed and released before the new one is accepted. What happens with
//! each connection (an instance, a runtime mounted on its session) is the
//! [`Accept`] given to the plugin. The access point provides no service per
//! connection, so the id a runtime chooses never claims a key of the
//! application.
//!
//! On its listener the access point registers as a link does, its name
//! standing for the far end: the transport routes a connection to it when
//! the admission verifies the credential. Static links and access points
//! share a listener as long as no credential is accepted by two of them
//! (such a connection is refused as ambiguous, 403) and no linked peer
//! there has the access point's name (the second registration fails).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rutis::host::{BoxFuture, DynPlugin, Effect, ErrorKind, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};
use tokio::sync::{broadcast, mpsc, watch};

use crate::channel::{Channel, ChannelError, Closer, PeerId, Receiver};
use crate::link::{Greet, Live};
use crate::runtime::RuntimeSession;
use crate::session::Error;
use crate::{
    admission_key, Accepting, Admission, Credential, Identity, Peer, Presented, Rejection, Retry,
};

/// Undoes what [`Accept::accept`] set up for a connection.
pub type Release = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// What an access point does with each connection it admits.
pub trait Accept: Send + Sync + 'static {
    /// Set up `connection`: an instance for it, a runtime on its session.
    /// The access point awaits the returned release when the connection
    /// ends or is taken over, before it accepts a newer connection with the
    /// same id. An error closes the connection.
    fn accept(&self, connection: Accepted) -> BoxFuture<'_, Result<Release, String>>;
}

/// A connection the access point admitted.
#[derive(Clone)]
pub struct Accepted {
    peer: Arc<Peer>,
}

impl Accepted {
    /// The connection id the admission gave it.
    pub fn id(&self) -> &PeerId {
        self.peer.id()
    }

    /// The far end as a peer: its session and the families it calls. Its
    /// generation is unique within the access point.
    pub fn peer(&self) -> &Arc<Peer> {
        &self.peer
    }

    /// The connection as a runtime session: provided as
    /// `RuntimeSession#<runtime>`, a [`RuntimePlugin::remote`](crate::runtime::RuntimePlugin::remote)
    /// runs its rows on it.
    pub fn runtime_session(&self) -> Arc<dyn RuntimeSession> {
        crate::runtime::peer_session(self.peer.clone())
    }
}

/// What happened at an access point, for observers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessEvent {
    /// The connection `generation` of `id` was admitted and accepted.
    Connected { id: PeerId, generation: u64 },
    /// A newer connection of `id` took over: the connection `generation`
    /// was closed and released.
    Replaced { id: PeerId, generation: u64 },
    /// The connection `generation` of `id` ended and was released.
    Disconnected {
        id: PeerId,
        generation: u64,
        reason: String,
    },
    /// The admission rejected a connection.
    Rejected { reason: String },
    /// A connection failed before it was accepted: its handshake, or the
    /// [`Accept`].
    Failed { reason: String },
}

#[derive(Clone, Debug)]
pub struct AccessPointConfig {
    /// The access point's name: what its registration on the listener is
    /// keyed by, as a link's is by its far end.
    pub name: PeerId,
    /// The admission's name (`Admission#<name>`).
    pub admission: String,
    /// The listener to register on.
    pub accepting: Accepting,
    /// How long a runtime may take to greet.
    pub handshake: Duration,
    /// Capabilities each runtime must declare (`runtime`).
    pub require: Vec<String>,
    /// Capabilities this end declares besides the session's own and `node`.
    pub declare: Vec<String>,
}

impl AccessPointConfig {
    pub fn new(name: PeerId, admission: &str, accepting: Accepting) -> Self {
        Self {
            name,
            admission: admission.into(),
            accepting,
            handshake: Retry::default().handshake,
            require: Vec::new(),
            declare: Vec::new(),
        }
    }

    pub fn handshake(mut self, handshake: Duration) -> Self {
        self.handshake = handshake;
        self
    }

    /// Require each runtime to declare `capability`.
    pub fn require(mut self, capability: &str) -> Self {
        self.require.push(capability.into());
        self
    }

    /// Declare `capability` to each runtime.
    pub fn declare(mut self, capability: &str) -> Self {
        self.declare.push(capability.into());
        self
    }
}

/// The access point plugin: depends on `Admission#<name>`, registers on
/// the listener while applied, and hands each admitted connection to its
/// [`Accept`].
pub struct AccessPointPlugin {
    label: String,
    config: AccessPointConfig,
    injects: [TypeKey; 1],
    accept: Option<Arc<dyn Accept>>,
    events: broadcast::Sender<AccessEvent>,
}

impl AccessPointPlugin {
    pub fn new(config: AccessPointConfig) -> Self {
        Self {
            label: format!("rutis-bridge/access-point#{}", config.name),
            injects: [admission_key(&config.admission).erase()],
            accept: None,
            events: broadcast::channel(64).0,
            config,
        }
    }

    /// Hand each admitted connection to `accept`. Without one, connections
    /// are kept and observable, and nothing runs on them.
    pub fn accept(mut self, accept: impl Accept) -> Self {
        self.accept = Some(Arc::new(accept));
        self
    }

    /// What happens from now on.
    pub fn events(&self) -> broadcast::Receiver<AccessEvent> {
        self.events.subscribe()
    }
}

impl DynPlugin for AccessPointPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let failed = |what: String| {
                rutis::Error::from(ErrorKind::PluginFailed(
                    format!("{}: {what}", self.label).into(),
                ))
            };
            let admission = ctx
                .get_key::<dyn Admission>(self.injects[0].clone())
                .ok_or_else(|| failed("the admission is not provided".into()))?;
            let (sender, mut delivered) = mpsc::unbounded_channel::<Channel>();
            let registered = self
                .config
                .accepting
                .register(
                    self.config.name.clone(),
                    Arc::new(Routing {
                        admission: admission.clone(),
                        name: self.config.name.clone(),
                    }),
                    crate::protocol(),
                    Box::new(move |channel| {
                        if let Err(refused) = sender.send(channel) {
                            refused.0.closer.close(crate::channel::Close::Done);
                        }
                    }),
                )
                .map_err(|error| failed(error.to_string()))?;
            let (stop, stopping) = watch::channel(false);
            let shared = Arc::new(Shared {
                config: self.config.clone(),
                admission,
                accept: self.accept.clone(),
                events: self.events.clone(),
                generation: AtomicU64::new(0),
                slots: Mutex::default(),
                stopping,
            });
            let mut stopped = shared.stopping.clone();
            let task = tokio::spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                // Counted here, in the order of delivery: the connection
                // tasks may first run in any order.
                let mut arrival = 0;
                loop {
                    tokio::select! {
                        channel = delivered.recv() => {
                            let Some(channel) = channel else { break };
                            arrival += 1;
                            connections.spawn(shared.clone().connection(channel, arrival));
                        }
                        Some(_) = connections.join_next() => {}
                        _ = stopped.wait_for(|stopped| *stopped) => break,
                    }
                }
                // Not aborted: each connection releases what its accept set
                // up, also one stopped half-way through a takeover.
                while connections.join_next().await.is_some() {}
            });
            ctx.effect(move || async move {
                drop(registered);
                let _ = stop.send(true);
                let _ = task.await;
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// The admission as the identity the registration routes by: a credential
/// it verifies proves the access point's name.
struct Routing {
    admission: Arc<dyn Admission>,
    name: PeerId,
}

impl Identity for Routing {
    fn local(&self) -> &PeerId {
        self.admission.local()
    }

    fn credential(&self, _: &PeerId) -> Option<Credential> {
        None
    }

    fn verify(&self, presented: Presented<'_>) -> Option<PeerId> {
        self.admission.verify(presented).then(|| self.name.clone())
    }
}

/// Why an accepted connection ends.
enum Ending {
    Closed,
    Stopped,
}

/// The current connection of one id.
struct Current {
    /// When it was delivered: of two connections of an id, the one that
    /// arrived later is current, whichever greeted first.
    arrival: u64,
    generation: u64,
    live: Live,
}

/// Where the connection of one id is kept. Locked from the arrival of a
/// newer connection until it is accepted, so takeovers of one id run one
/// at a time and other ids are not held up.
type Slot = Arc<tokio::sync::Mutex<Option<Current>>>;

struct Shared {
    config: AccessPointConfig,
    admission: Arc<dyn Admission>,
    accept: Option<Arc<dyn Accept>>,
    events: broadcast::Sender<AccessEvent>,
    generation: AtomicU64,
    slots: Mutex<HashMap<PeerId, Slot>>,
    /// Set when the access point stops: connections end and release.
    stopping: watch::Receiver<bool>,
}

impl Shared {
    fn emit(&self, event: AccessEvent) {
        // Nobody observing is fine.
        let _ = self.events.send(event);
    }

    /// Admit, accept and keep one delivered connection until it ends.
    async fn connection(self: Arc<Self>, channel: Channel, arrival: u64) {
        let mut stopping = self.stopping.clone();
        let Channel {
            sender,
            receiver,
            closer,
        } = channel;
        // Routed by its credential alone: its id comes with its hello.
        let verdict = Arc::new(OnceLock::new());
        let channel = Channel {
            sender,
            receiver: Box::new(Admitting {
                inner: receiver,
                closer: closer.clone(),
                admission: self.admission.clone(),
                verdict: Some(verdict.clone()),
            }),
            closer: closer.clone(),
        };
        let greet = Greet {
            local: self.admission.local().clone(),
            expected: None,
            declare: &self.config.declare,
            require: &self.config.require,
            compat: false,
            timeout: self.config.handshake,
        };
        // A connection still greeting when the access point stops has
        // nothing to release.
        let opened = tokio::select! {
            opened = greet.open(channel) => opened,
            _ = stopping.wait_for(|stopped| *stopped) => return,
        };
        if let Some(Err(rejection)) = verdict.get() {
            return self.emit(AccessEvent::Rejected {
                reason: rejection.to_string(),
            });
        }
        let (session, operations) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                return self.emit(AccessEvent::Failed {
                    reason: error.to_string(),
                })
            }
        };
        // Ready means it greeted in the endpoint format, so was admitted.
        let Some(Ok(id)) = verdict.get().cloned() else {
            let reason = "the far end greeted without being admitted";
            session.close(Error::Transport(reason.into()));
            return self.emit(AccessEvent::Failed {
                reason: reason.into(),
            });
        };

        let slot = self.slot(&id);
        let mut current = slot.lock().await;
        let stopped = *self.stopping.borrow();
        let superseded = if stopped {
            Some("the access point stopped".to_owned())
        } else {
            current
                .as_ref()
                .filter(|current| current.arrival > arrival)
                .map(|_| format!("{id}: a newer connection is current"))
        };
        if let Some(reason) = superseded {
            drop(current);
            match stopped {
                true => closer.close(crate::channel::Close::Done),
                false => closer.close(crate::channel::Close::Replaced),
            }
            session.close(Error::Transport(reason.clone()));
            operations.detach();
            self.forget(&id, slot);
            return self.emit(AccessEvent::Failed { reason });
        }
        // A newer connection takes over: the old one is closed and
        // released before this one is accepted.
        if let Some(old) = current.take() {
            old.live.closer.close(crate::channel::Close::Replaced);
            old.live.end("replaced by a new connection").await;
            self.emit(AccessEvent::Replaced {
                id: id.clone(),
                generation: old.generation,
            });
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let peer = Arc::new(Peer::new(
            id.clone(),
            session.clone(),
            generation,
            operations.clone(),
        ));
        let release: Release = match &self.accept {
            None => Box::new(|| Box::pin(async {})),
            Some(accept) => match accept.accept(Accepted { peer }).await {
                Ok(release) => release,
                Err(error) => {
                    drop(current);
                    let reason = format!("{id}: {error}");
                    session.close(Error::Transport(reason.clone()));
                    operations.detach();
                    self.forget(&id, slot);
                    return self.emit(AccessEvent::Failed { reason });
                }
            },
        };
        *current = Some(Current {
            arrival,
            generation,
            live: Live::new(session.clone(), closer, operations, release),
        });
        drop(current);
        self.emit(AccessEvent::Connected {
            id: id.clone(),
            generation,
        });

        let ending = tokio::select! {
            _ = session.closed() => Ending::Closed,
            _ = stopping.wait_for(|stopped| *stopped) => Ending::Stopped,
        };
        let mut current = slot.lock().await;
        // Unless a newer connection took over, and released this one.
        if current
            .as_ref()
            .is_some_and(|current| current.generation == generation)
        {
            let ended = current.take().expect("checked");
            let reason = match ending {
                Ending::Stopped => "the access point stopped".to_owned(),
                Ending::Closed => session.close_reason().map_or_else(
                    || "the connection ended".to_owned(),
                    |error| error.to_string(),
                ),
            };
            ended.live.end(&reason).await;
            self.emit(AccessEvent::Disconnected {
                id: id.clone(),
                generation,
                reason,
            });
        }
        drop(current);
        self.forget(&id, slot);
    }

    fn slot(&self, id: &PeerId) -> Slot {
        self.slots
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone()
    }

    /// Drop the slot of `id` if it is empty and nobody else holds it.
    fn forget(&self, id: &PeerId, slot: Slot) {
        let mut slots = self.slots.lock().unwrap();
        // Slots are cloned only under this lock: the count is exact.
        let unused =
            Arc::strong_count(&slot) == 2 && slot.try_lock().is_ok_and(|current| current.is_none());
        if unused {
            slots.remove(id);
        }
    }
}

/// The receiver of a connection not admitted yet: the far end's hello is
/// admitted before the session reads it, so a rejected one never reaches
/// the session, and the channel closes as rejected.
struct Admitting {
    inner: Box<dyn Receiver>,
    closer: Arc<dyn Closer>,
    admission: Arc<dyn Admission>,
    /// Until the first message.
    verdict: Option<Arc<OnceLock<Result<PeerId, Rejection>>>>,
}

impl Receiver for Admitting {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        let message = self.inner.recv()?;
        let (Some(verdict), Some(first)) = (self.verdict.take(), &message) else {
            return Ok(message);
        };
        // Anything but an endpoint-format hello naming a valid endpoint is
        // the session's to refuse (as incompatible).
        let Some(reported) = reported(first) else {
            return Ok(message);
        };
        let admitted = self.admission.admit(&reported);
        let rejected = admitted.as_ref().err().cloned();
        let _ = verdict.set(admitted);
        if let Some(rejection) = rejected {
            self.closer
                .close(crate::channel::Close::Rejected(rejection.0.clone()));
            return Err(ChannelError::Closed {
                reason: format!("rejected: {rejection}"),
            });
        }
        Ok(message)
    }
}

/// The endpoint an endpoint-format hello names, if it names a valid one.
fn reported(message: &[u8]) -> Option<PeerId> {
    let hello: serde_json::Value = serde_json::from_slice(message).ok()?;
    let endpoint_format = hello.get("op")? == "hello"
        && hello.get("version")?.as_u64()? == u64::from(crate::session::ENDPOINT_PROTOCOL);
    if !endpoint_format {
        return None;
    }
    PeerId::new(hello.get("endpoint")?.as_str()?).ok()
}

#[cfg(test)]
mod tests {
    use super::reported;
    use crate::channel::PeerId;

    #[test]
    fn only_an_endpoint_format_hello_reports_an_id() {
        let hello = |text: &str| reported(text.as_bytes());
        assert_eq!(
            hello(r#"{"op":"hello","version":3,"endpoint":"pixel"}"#),
            Some(PeerId::new("pixel").unwrap())
        );
        assert_eq!(hello(r#"{"op":"hello","version":3}"#), None);
        assert_eq!(hello(r#"{"op":"hello","version":3,"endpoint":7}"#), None);
        assert_eq!(
            hello(r#"{"op":"hello","version":3,"endpoint":"Pixel 7"}"#),
            None
        );
        assert_eq!(hello(r#"{"op":"hello","version":2}"#), None);
        assert_eq!(hello(r#"{"op":"invoke","version":3}"#), None);
        assert_eq!(hello("not json"), None);
    }
}
