//! A link: the connection and session with one far end, kept up.
//!
//! The link dials the far end (and redials, with backoff by failure
//! category) or is accepted on a listener for it (and lets a newer
//! connection take over). Once the far end greets as the expected endpoint, the link
//! provides `Peer#<id>`; when the session ends it withdraws it, so what
//! depends on the peer stops natively. It knows nothing of rows, schemas
//! or what the families registered on the peer do.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::channel::{Channel, Closer, ConnectError, PeerId};
use crate::session::rpc::Connection;
use crate::session::{Error, Handshake};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};
use tokio::sync::{mpsc, watch};

use crate::peer::{Operations, Peer};
use crate::session::plugin::Greet;
use crate::{identity_key, peer_key, Accepting, Identity, Registered};

/// The session protocol links speak, as a WebSocket subprotocol.
pub fn protocol() -> String {
    format!("rutis.{}", crate::session::ENDPOINT_PROTOCOL)
}

/// One attempt to reach the far end, presenting the credential the link's
/// identity has for it (if any): each call reports one result and never
/// retries; the link owns the retry policy, decided by the
/// [`ConnectError`] category. Transitional (#695 S2): goes with links.
#[doc(hidden)]
pub type Dialer = Arc<
    dyn Fn(Option<crate::Credential>) -> BoxFuture<'static, Result<Channel, ConnectError>>
        + Send
        + Sync,
>;

/// How the link reaches its far end.
#[derive(Clone)]
pub enum Connect {
    /// Dial with this.
    Dial(Dialer),
    /// Accept the far end on this listener.
    Listen(Accepting),
}

impl std::fmt::Debug for Connect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Connect::Dial(_) => f.write_str("Dial"),
            Connect::Listen(accepting) => write!(f, "Listen({})", accepting.listener),
        }
    }
}

/// Reconnection, for a dialing link
/// (`docs/design-protocol-channel-decoupling-2026-10-03.md`).
#[derive(Clone, Debug)]
pub struct Retry {
    /// The first wait after a retryable failure; doubled each time.
    pub initial: Duration,
    pub max: Duration,
    /// ± this fraction of each wait, at random.
    pub jitter: f64,
    /// A session lasting this long resets the backoff.
    pub stable: Duration,
    /// The wait after an authentication failure.
    pub rejected: Duration,
    /// How long a far end may take to greet.
    pub handshake: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
            jitter: 0.2,
            stable: Duration::from_secs(60),
            rejected: Duration::from_secs(30),
            handshake: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LinkConfig {
    /// The far end.
    pub peer: PeerId,
    /// The identity's name (`Identity#<name>`): this endpoint, its
    /// credentials and the rules that verify the far end.
    pub identity: String,
    pub connect: Connect,
    pub retry: Retry,
    /// Capabilities the far end must declare, such as its contract
    /// (`runtime`, `node`); without them the link stops, as for an
    /// incompatible far end.
    pub require: Vec<String>,
    /// Capabilities this end declares besides the session's own and
    /// `node`, which every rutis link declares.
    pub declare: Vec<String>,
    /// Connect again after a session ends. A local runtime process is not
    /// restarted: its link stops when it ends.
    pub reconnect: bool,
}

impl LinkConfig {
    pub fn dial(peer: PeerId, identity: &str, dialer: Dialer) -> Self {
        Self {
            peer,
            identity: identity.into(),
            connect: Connect::Dial(dialer),
            retry: Retry::default(),
            require: Vec::new(),
            declare: Vec::new(),
            reconnect: true,
        }
    }

    pub fn listen(peer: PeerId, identity: &str, accepting: Accepting) -> Self {
        Self {
            peer,
            identity: identity.into(),
            connect: Connect::Listen(accepting),
            retry: Retry::default(),
            require: Vec::new(),
            declare: Vec::new(),
            reconnect: true,
        }
    }

    pub fn retry(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
    }

    /// Require the far end to declare `capability`.
    pub fn require(mut self, capability: &str) -> Self {
        self.require.push(capability.into());
        self
    }

    /// Declare `capability` to the far end.
    pub fn declare(mut self, capability: &str) -> Self {
        self.declare.push(capability.into());
        self
    }

    /// A local runtime process: no reconnecting, and 30 s to greet
    /// (`LOCAL_RUNTIME_GREETING`).
    pub fn local_runtime(mut self) -> Self {
        self.reconnect = false;
        self.retry.handshake = LOCAL_RUNTIME_GREETING;
        self
    }
}

/// How long a local runtime process may take to greet, counted from its
/// spawn: the interpreter starting (a JVM, Python with its imports, Node or
/// Bun loading their modules) is part of it, and a healthy one on a busy or
/// slow machine (several runtimes starting at once at boot, a small board)
/// can take longer than the network's 10 s. A process that ends is noticed
/// at once, by its channel closing; this only bounds one that lives and says
/// nothing.
const LOCAL_RUNTIME_GREETING: Duration = Duration::from_secs(30);

/// Why the link is not ready, by what it means for retrying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Retryable,
    AuthRejected,
    Incompatible,
}

/// What a link is doing, for diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    /// Not applied.
    Idle,
    /// Dialing, or waiting for the far end to connect.
    Connecting,
    /// Session `generation` is ready; `Peer#<id>` is provided.
    Ready { generation: u64 },
    /// The last attempt failed; the next comes after `retry_in`.
    Waiting {
        failure: Failure,
        error: String,
        retry_in: Duration,
    },
    /// No more attempts until the link is restarted or reconfigured: the
    /// far end is incompatible (or the listener refused the registration,
    /// also `Incompatible`), or a link that does not reconnect (a local
    /// runtime's) ended, for `failure`.
    Stopped { failure: Failure, error: String },
}

/// The link plugin: depends on `Identity#<name>`, provides `Peer#<id>`
/// while a session is ready.
pub struct LinkPlugin {
    name: String,
    config: LinkConfig,
    injects: [TypeKey; 1],
    state: Arc<watch::Sender<LinkState>>,
}

impl LinkPlugin {
    pub fn new(config: LinkConfig) -> Self {
        Self {
            name: format!("link to {}", config.peer),
            injects: [identity_key(&config.identity).erase()],
            state: Arc::new(watch::channel(LinkState::Idle).0),
            config,
        }
    }

    pub fn state(&self) -> watch::Receiver<LinkState> {
        self.state.subscribe()
    }
}

impl DynPlugin for LinkPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let missing = |what: &str| {
                rutis::Error::from(ErrorKind::PluginFailed(
                    format!("{}: {what} is not provided", self.name).into(),
                ))
            };
            let identity = ctx
                .get_key::<dyn Identity>(self.injects[0].clone())
                .ok_or_else(|| missing("the identity"))?;
            let link = Link {
                ctx: ctx.clone(),
                config: self.config.clone(),
                identity,
                state: self.state.clone(),
                generation: 0,
            };
            // A listening link registers before its apply returns: once it
            // is applied, its far end is routed to it, also on a listener another
            // entry already registered on, where an unknown
            // credential is refused, not retried (#411).
            let task = match self.config.connect.clone() {
                Connect::Dial(dialer) => tokio::spawn(link.dialing(dialer)),
                Connect::Listen(accepting) => match link.register(&accepting) {
                    Some(listening) => tokio::spawn(link.listening(listening)),
                    None => tokio::spawn(async {}),
                },
            };
            let state = self.state.clone();
            ctx.effect(move || {
                async move {
                    // Dropping the task's state closes the session, revokes
                    // the registration and cancels any retry. Waited for:
                    // a dial in the middle of starting a process (fork and
                    // exec, which do not yield) finishes that first, and
                    // the process must have been recorded, and its channel
                    // closed, when the link has stopped.
                    task.abort();
                    let _ = task.await;
                    state.send_replace(LinkState::Idle);
                    Ok(())
                }
            })?;
            Ok(Effect::Done)
        })
    }
}

struct Link {
    ctx: Ctx,
    config: LinkConfig,
    identity: Arc<dyn Identity>,
    state: Arc<watch::Sender<LinkState>>,
    generation: u64,
}

/// Undoes what was set up on a session.
type Undo = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// A ready session and what is held for it. Dropping it closes the
/// session.
pub(crate) struct Live {
    pub(crate) session: Connection,
    pub(crate) closer: Arc<dyn Closer>,
    operations: Arc<Operations>,
    /// Undoes what was set up on the session (the provided peer). In a
    /// mutex only to make `Live` shareable while its holder awaits.
    release: Mutex<Option<Undo>>,
    /// tokio's clock, the same as std's unless paused: a test on the
    /// paused clock can check the reset after `stable` (#231).
    since: tokio::time::Instant,
}

impl Live {
    pub(crate) fn new(
        session: Connection,
        closer: Arc<dyn Closer>,
        operations: Arc<Operations>,
        release: Undo,
    ) -> Self {
        Self {
            session,
            closer,
            operations,
            release: Mutex::new(Some(release)),
            since: tokio::time::Instant::now(),
        }
    }

    /// Undo what was set up on the session (withdraw the peer), then close
    /// it.
    pub(crate) async fn end(self, reason: &str) {
        let release = self.release.lock().unwrap().take();
        if let Some(release) = release {
            release().await;
        }
        self.session.close(Error::Transport(reason.into()));
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.session.close(Error::Transport("link stopped".into()));
        self.operations.detach();
    }
}

fn classify(error: &Error) -> Failure {
    match error {
        Error::Handshake(Handshake::Incompatible(_)) => Failure::Incompatible,
        Error::Handshake(Handshake::IdentityMismatch(_)) => Failure::AuthRejected,
        _ => Failure::Retryable,
    }
}

impl Link {
    /// Open a session on `channel` and, once the far end greeted as the
    /// expected endpoint, provide the peer.
    async fn start(&mut self, channel: Channel) -> Result<Live, Error> {
        let closer = channel.closer.clone();
        let greet = Greet {
            local: self.identity.local().clone(),
            expected: Some(self.config.peer.clone()),
            declare: &self.config.declare,
            require: &self.config.require,
            timeout: self.config.retry.handshake,
        };
        let (session, operations) = greet.open(channel).await?;
        self.generation += 1;
        let peer = Peer::new(
            self.config.peer.clone(),
            session.clone(),
            self.generation,
            operations.clone(),
        );
        let provided = self
            .ctx
            .provide_at(peer_key(&self.config.peer), Arc::new(peer))
            .map_err(|error| Error::Transport(format!("cannot provide the peer: {error}")))?;
        self.state.send_replace(LinkState::Ready {
            generation: self.generation,
        });
        let release: Undo = Box::new(move || {
            Box::pin(async move {
                let _ = provided.dispose().await;
            })
        });
        Ok(Live::new(session, closer, operations, release))
    }

    async fn wait(&self, failure: Failure, error: String, retry_in: Duration) {
        self.state.send_replace(LinkState::Waiting {
            failure,
            error,
            retry_in,
        });
        tokio::time::sleep(retry_in).await;
    }

    async fn dialing(mut self, dialer: Dialer) {
        let retry = self.config.retry.clone();
        let mut backoff = Backoff::new(&retry);
        loop {
            self.state.send_replace(LinkState::Connecting);
            let (failure, error) = match dialer(self.identity.credential(&self.config.peer)).await {
                Err(error) => (
                    match error {
                        ConnectError::Retryable { .. } => Failure::Retryable,
                        ConnectError::AuthRejected { .. } => Failure::AuthRejected,
                        ConnectError::Incompatible { .. } => Failure::Incompatible,
                    },
                    error.to_string(),
                ),
                Ok(channel) => match self.start(channel).await {
                    Err(error) => (classify(&error), error.to_string()),
                    Ok(live) => {
                        live.session.closed().await;
                        if live.since.elapsed() >= retry.stable {
                            backoff.reset();
                        }
                        // Why it ended: for a local runtime, how its
                        // process ended; an identity the far end rejected
                        // after the greeting (a listener refusing this
                        // end's id) is an authentication failure.
                        let ended = live.session.close_reason();
                        let reason = ended.as_ref().map_or_else(
                            || "the connection ended".to_owned(),
                            |error| error.to_string(),
                        );
                        live.end("the connection ended").await;
                        (ended.as_ref().map_or(Failure::Retryable, classify), reason)
                    }
                },
            };
            if !self.config.reconnect {
                self.state
                    .send_replace(LinkState::Stopped { failure, error });
                return;
            }
            match failure {
                Failure::Incompatible => {
                    self.state
                        .send_replace(LinkState::Stopped { failure, error });
                    return;
                }
                Failure::AuthRejected => self.wait(failure, error, retry.rejected).await,
                Failure::Retryable => self.wait(failure, error, backoff.next()).await,
            }
        }
    }

    /// Register on the listener for the far end. If the listener refuses,
    /// the link is stopped (as `Incompatible`) and there is nothing to run.
    fn register(&self, accepting: &Accepting) -> Option<Listening> {
        let (sender, accepted) = mpsc::unbounded_channel::<Channel>();
        let registered = accepting.register(
            self.config.peer.clone(),
            self.identity.clone(),
            protocol(),
            Box::new(move |channel| {
                if let Err(refused) = sender.send(channel) {
                    refused.0.closer.close(crate::channel::Close::Done);
                }
            }),
        );
        match registered {
            Ok(registered) => {
                self.state.send_replace(LinkState::Connecting);
                Some(Listening {
                    _registered: registered,
                    accepted,
                })
            }
            Err(error) => {
                self.state.send_replace(LinkState::Stopped {
                    failure: Failure::Incompatible,
                    error: error.to_string(),
                });
                None
            }
        }
    }

    /// Accept the far end's connections delivered to the registration, a
    /// newer one taking over, until the link is stopped (the task aborted,
    /// which revokes the registration).
    async fn listening(mut self, listening: Listening) {
        let Listening {
            _registered,
            mut accepted,
        } = listening;
        let mut current: Option<Live> = None;
        loop {
            let ended = async {
                match &current {
                    Some(live) => live.session.closed().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                channel = accepted.recv() => {
                    let Some(channel) = channel else { return };
                    // A newer connection of the far end takes over: the old
                    // session ends, and its peer is withdrawn, first.
                    if let Some(old) = current.take() {
                        old.closer.close(crate::channel::Close::Replaced);
                        old.end("replaced by a new connection").await;
                    }
                    match self.start(channel).await {
                        Ok(live) => current = Some(live),
                        Err(error) => {
                            self.state.send_replace(LinkState::Waiting {
                                failure: classify(&error),
                                error: error.to_string(),
                                retry_in: Duration::ZERO,
                            });
                        }
                    }
                }
                () = ended => {
                    if let Some(old) = current.take() {
                        old.end("the connection ended").await;
                    }
                    self.state.send_replace(LinkState::Connecting);
                }
            }
        }
    }
}

/// A listening link's registration and the connections handed over to it.
struct Listening {
    /// Revoked when dropped.
    _registered: Registered,
    accepted: mpsc::UnboundedReceiver<Channel>,
}

/// Exponential backoff with jitter.
struct Backoff {
    initial: Duration,
    max: Duration,
    jitter: f64,
    next: Duration,
    seed: Mutex<u64>,
}

impl Backoff {
    fn new(retry: &Retry) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9_7f4a_7c15, |elapsed| elapsed.as_nanos() as u64)
            | 1;
        Self {
            initial: retry.initial,
            max: retry.max,
            jitter: retry.jitter,
            next: retry.initial,
            seed: Mutex::new(seed),
        }
    }

    fn reset(&mut self) {
        self.next = self.initial;
    }

    /// The next wait: the current step ± jitter; the step doubles up to max.
    fn next(&mut self) -> Duration {
        let base = self.next;
        self.next = (self.next * 2).min(self.max);
        let unit = {
            // xorshift64: no cryptographic need, only spreading reconnects.
            let mut seed = self.seed.lock().unwrap();
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            (*seed >> 11) as f64 / (1u64 << 53) as f64
        };
        base.mul_f64(1.0 + self.jitter * (2.0 * unit - 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_its_cap_within_jitter_and_resets() {
        let retry = Retry::default();
        let mut backoff = Backoff::new(&retry);
        let steps: Vec<f64> = (0..10).map(|_| backoff.next().as_secs_f64()).collect();
        let bases = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0, 30.0];
        for (step, base) in steps.iter().zip(bases) {
            assert!(
                (base * 0.8..=base * 1.2).contains(step),
                "{step} is not within 20% of {base}"
            );
        }
        backoff.reset();
        assert!(backoff.next() <= Duration::from_millis(600));
    }
}
