//! `rutis-bridge/session`: the far end of one [`Channel`] as plugins of
//! this kernel (`docs/design-remote-session-2026-10-28.md` §二–§四).
//!
//! A session is two steps: [`Session::handshake`] greets the far end (as
//! the [`LocalId`] the host provides) and registers nothing; mounting the
//! [`Greeted`] session (`ctx.plugin(greeted)`) brings the far end in. Its
//! plugins become plugins `<id>:<plugin>` below the session, the services
//! of both kernels are seen on both sides, and the far end's listeners are
//! entries of this bus. [`Session::new`] is both steps in one plugin.
//!
//! The channel and the session live and end together: the far end leaving
//! unloads the session, and the session unloading (its connection plugin
//! unloaded, the host stopping) closes the channel with
//! [`Close::Done`](crate::Close::Done), after the far end's plugins here
//! unloaded.

mod greet;
mod services;

pub(crate) use greet::Greet;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, Key};
use rutis::{Ctx, Error as RutisError};

use crate::channel::{Channel, Close, PeerId};
use crate::peer::Operations;
use crate::session::rpc::Connection;
use crate::session::Error;
use crate::{peer_key, Peer};

/// The plugin name of every session, and of the root plugin that serves
/// `sessions`.
const NAME: &str = "rutis-bridge/session";

/// How long a far end may take to greet.
const HANDSHAKE: Duration = Duration::from_secs(10);

/// The id this end greets with in every session. The host provides it on
/// its root: `root.provide(LocalId(id))`. There is no default: a session
/// without one fails, saying `the host provides no id for its sessions`.
#[derive(Clone, Debug)]
pub struct LocalId(pub PeerId);

/// The key the session mounted under the connection row `row` provides
/// its [`SessionHandle`] at, once it is mounted: how rutis-loader finds the
/// session of a row `<row>:<plugin>`. A session outside any row registers
/// none, and so does a second session under a row that has one.
#[doc(hidden)]
pub fn session_key(row: &str) -> Key<SessionHandle> {
    Key::dynamic(row.to_owned())
}

fn failed(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> RutisError {
    RutisError::from(ErrorKind::PluginFailed(error.into()))
}

/// A session on one channel, handshake and mount in one plugin:
/// `ctx.plugin(Session::new(channel))`. Its [`FiberView`](rutis::host::FiberView)
/// ends when the session unloads: the far end left, it was unloaded, or the
/// handshake failed (the plugin fails with why).
pub struct Session {
    channel: Mutex<Option<Channel>>,
}

impl Session {
    pub fn new(channel: Channel) -> Self {
        Self {
            channel: Mutex::new(Some(channel)),
        }
    }

    /// Greet the far end on `channel` as the [`LocalId`] `ctx` sees, waiting
    /// at most 10 s for its greeting. Registers nothing: mount the result
    /// to bring the far end in. On failure the far end has this end's
    /// greeting, which says why where the protocol can (its version), and
    /// the channel is closed with [`Close::Done`].
    pub async fn handshake(ctx: &Ctx, channel: Channel) -> Result<Greeted, RutisError> {
        let Some(local) = ctx.get::<LocalId>() else {
            channel.closer.close(Close::Done);
            return Err(failed("the host provides no id for its sessions"));
        };
        let greet = Greet {
            local: local.0.clone(),
            expected: None,
            declare: &[],
            require: &[],
            timeout: HANDSHAKE,
        };
        let (connection, operations) = greet.open(channel).await.map_err(failed)?;
        let far = connection
            .greeting()
            .map(|greeting| greeting.endpoint.clone())
            .expect("a ready session has the far end's greeting");
        Ok(Greeted {
            shared: Arc::new(Shared {
                far,
                state: Mutex::new(None),
            }),
            start: Mutex::new(Some(Start {
                connection,
                operations,
            })),
        })
    }
}

impl DynPlugin for Session {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        NAME
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let channel = self.channel.lock().unwrap().take().ok_or_else(|| {
                failed("a session runs once: its channel is gone with the first run")
            })?;
            let greeted = Session::handshake(ctx, channel).await?;
            greeted.mount(ctx).await
        })
    }
}

/// A session whose far end greeted, not mounted yet: nothing of the far end
/// is registered. Mount it with `ctx.plugin(greeted)`; dropping it
/// unmounted closes the channel with [`Close::Done`].
pub struct Greeted {
    shared: Arc<Shared>,
    start: Mutex<Option<Start>>,
}

impl Greeted {
    /// The id the far end greeted with.
    pub fn id(&self) -> &str {
        self.shared.far.as_str()
    }

    /// The session's handle, before it is mounted: [`SessionHandle::run`]
    /// fails until it is.
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            shared: self.shared.clone(),
        }
    }

    async fn mount(&self, ctx: &Ctx) -> Result<Effect, RutisError> {
        let start = self
            .start
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| failed("a session runs once: it ended with its first run"))?;
        mount(self.shared.clone(), start, ctx).await
    }
}

impl DynPlugin for Greeted {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        NAME
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(self.mount(ctx))
    }
}

/// The greeted session, until it ends: dropping it closes the session and
/// its channel ([`Close::Done`]).
struct Start {
    connection: Connection,
    operations: Arc<Operations>,
}

impl Drop for Start {
    fn drop(&mut self) {
        self.connection
            .close(Error::Transport("the session ended".into()));
        self.operations.detach();
    }
}

/// What a session's handles share.
struct Shared {
    far: PeerId,
    state: Mutex<Option<Mounted>>,
}

/// A mounted session.
struct Mounted {
    #[allow(dead_code)]
    ctx: Ctx,
}

/// A mounted session, as code that is not one of its plugins uses it.
#[derive(Clone)]
pub struct SessionHandle {
    shared: Arc<Shared>,
}

impl SessionHandle {
    /// The id the far end greeted with.
    pub fn id(&self) -> &str {
        self.shared.far.as_str()
    }
}

/// Labels of the scopes sessions provide their peer in.
static SESSIONS: AtomicU64 = AtomicU64::new(0);

/// Bring the far end of `start` in, from `ctx`, the session's fiber.
async fn mount(shared: Arc<Shared>, start: Start, ctx: &Ctx) -> Result<Effect, RutisError> {
    let far = shared.far.clone();
    let peer = Arc::new(Peer::new(
        far.clone(),
        start.connection.clone(),
        1,
        start.operations.clone(),
    ));
    // The session's peer is its own: what runs on it is mounted below, in a
    // scope where the peer is this one.
    let label = format!("{NAME}/{}", SESSIONS.fetch_add(1, Ordering::Relaxed));
    let scoped = ctx.isolate(peer_key(&far), &label);
    scoped.provide_at(peer_key(&far), peer.clone())?;
    let failing = |error: Error| failed(error);

    // What the far end provides, every far end: it announces services once
    // this end offers `services`.
    let imports = services::Imports::new(&scoped, far.clone(), start.connection.clone());
    let importing = peer
        .register("services", Arc::new(services::Importing(imports.clone())))
        .map_err(failing)?;
    // What this end provides, to every far end: announced to one that
    // offers `services`.
    let exporting = services::Exports::follow(&scoped, peer.clone())?;
    // A rutis kernel's listeners are entries of this bus, and this bus's of
    // its: one hop, by every name.
    let node = start.connection.supports("node");
    if node {
        scoped.plugin(crate::LinkEventsPlugin::new(far.clone(), ["*"], ["*"]));
    }

    // The far end leaving unloads the session.
    let leaving = {
        let (ending, connection) = (ctx.clone(), start.connection.clone());
        tokio::spawn(async move {
            connection.closed().await;
            let _ = ending.dispose_self();
        })
    };
    *shared.state.lock().unwrap() = Some(Mounted { ctx: scoped });
    let kept = Mutex::new(Some((start, peer, importing, imports)));
    ctx.effect(move || async move {
        leaving.abort();
        shared.state.lock().unwrap().take();
        let _ = exporting.dispose().await;
        // The far end's plugins here have unloaded (children first): the
        // channel closes last.
        drop(kept.lock().unwrap().take());
        Ok(())
    })?;
    Ok(Effect::Done)
}
