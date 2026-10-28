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
mod rows;
mod run;
mod services;

pub use rows::catalog_key;
pub use run::RowHandle;

pub(crate) use greet::Greet;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, Key};
use rutis::{Ctx, Error as RutisError};

use crate::channel::{Channel, Close, PeerId};
use crate::peer::Operations;
use crate::runtime::{Mount, Process};
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
    /// the channel is closed with [`Close::Done`]; so it is when the
    /// handshake is dropped before it ends. Errors as a session's: another
    /// version or format is [`Handshake::Incompatible`](crate::session::Handshake),
    /// a far end refusing this end's id (4003) `IdentityMismatch`, no
    /// greeting in time `Transport("the far end did not greet in time")`.
    pub async fn handshake(ctx: &Ctx, channel: Channel) -> Result<Greeted, Error> {
        let Some(local) = ctx.get::<LocalId>() else {
            channel.closer.close(Close::Done);
            return Err(Error::refused(
                "the host provides no id for its sessions".into(),
            ));
        };
        let greet = Greet {
            local: local.0.clone(),
            expected: None,
            declare: &[],
            require: &[],
            timeout: HANDSHAKE,
        };
        let (connection, operations) = greet.open(channel).await?;
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
            let greeted = Session::handshake(ctx, channel).await.map_err(failed)?;
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
#[derive(Clone)]
struct Mounted {
    /// Where its remote plugins are mounted.
    ctx: Ctx,
    imports: Arc<services::Imports>,
    /// The far end is a rutis kernel.
    node: bool,
    /// What runs plugins there, once it is mounted: none for a far end
    /// that does not (a device app that only announces services).
    process: Option<Arc<Process>>,
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

    /// Run `plugin` there with `config`: once it loaded, it is the plugin
    /// `<id>:<plugin>` below the session. Dropping the handle stops it.
    /// Fails at once only when the session is not mounted (or has ended),
    /// or the far end runs no plugins; the far end not containing the
    /// plugin, or its load failing, is [`RowHandle::loaded`]'s.
    pub fn run(&self, plugin: &str, config: serde_json::Value) -> Result<RowHandle, RutisError> {
        let far = &self.shared.far;
        let mounted = self
            .shared
            .state
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| failed(format!("the session with {far} is not mounted")))?;
        let process = mounted
            .process
            .ok_or_else(|| failed(format!("{far} runs no plugins")))?;
        Ok(run::run(
            run::Running {
                ctx: mounted.ctx,
                far: far.to_string(),
                process,
                node: mounted.node,
            },
            plugin,
            config,
        ))
    }

    /// What runs plugins there: rutis-loader loads the rows `<row>:<plugin>`
    /// through it, as a runtime's.
    #[doc(hidden)]
    pub fn process(&self) -> Option<Arc<Process>> {
        self.shared
            .state
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|mounted| mounted.process.clone())
    }

    /// Mounted, with `process` running plugins there: provided at
    /// [`session_key`] of the row the session is under.
    fn ready(&self, ctx: &Ctx, process: Arc<Process>) -> Result<(), RutisError> {
        if let Some(mounted) = self.shared.state.lock().unwrap().as_mut() {
            mounted.imports.process(process.clone());
            mounted.process = Some(process);
        }
        let Some(row) = ctx.row() else {
            return Ok(());
        };
        let key = session_key(&row);
        if ctx.get_at(&key).is_some() {
            return Ok(());
        }
        ctx.provide_at(key, Arc::new(self.clone()))?;
        Ok(())
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
    let handle = SessionHandle {
        shared: shared.clone(),
    };
    let node = start.connection.supports("node");

    // What the far end provides, every far end: it announces services once
    // this end offers `services`.
    let imports = services::Imports::new(&scoped, far.clone(), start.connection.clone());
    let mut offered = vec![peer
        .register("services", Arc::new(services::Importing(imports.clone())))
        .map_err(failing)?];
    *shared.state.lock().unwrap() = Some(Mounted {
        ctx: scoped.clone(),
        imports,
        node,
        process: None,
    });
    let mut client = None;
    if start.connection.supports("runtime") {
        // A language runtime: this end's process on it, its listeners on
        // this bus.
        let mount = Mount {
            ctx: Some(scoped.clone()),
            runtime: Some(far.to_string()),
            ..Mount::default()
        };
        let process = Process::over(crate::runtime::peer_session(peer.clone()), mount)
            .await
            .map_err(failing)?;
        process.check_rows(far.as_str()).map_err(failing)?;
        handle.ready(ctx, process)?;
    } else if node {
        // A rutis kernel: each runs the other's rows; its listeners and
        // this bus's are entries of each other's bus, one hop, every name.
        let (rows, served) = rows::Rows::serve(&scoped, &peer).map_err(failing)?;
        offered.extend(served);
        scoped.plugin(crate::LinkEventsPlugin::new(far.clone(), ["*"], ["*"]));
        client = Some(tokio::spawn(client_once_served(
            ctx.clone(),
            scoped.clone(),
            handle.clone(),
            rows::Client {
                peer: peer.clone(),
                rows,
            },
        )));
    }
    // What this end provides, to every far end: announced to one that
    // offers `services`.
    let exporting = services::Exports::follow(&scoped, peer.clone())?;

    // The far end leaving unloads the session.
    let leaving = {
        let (ending, connection) = (ctx.clone(), start.connection.clone());
        tokio::spawn(async move {
            connection.closed().await;
            let _ = ending.dispose_self();
        })
    };
    let kept = Mutex::new(Some((start, peer, offered)));
    ctx.effect(move || async move {
        leaving.abort();
        if let Some(client) = client {
            client.abort();
        }
        let _ = exporting.dispose().await;
        shared.state.lock().unwrap().take();
        // The far end's plugins here have unloaded (children first): the
        // channel closes last.
        drop(kept.lock().unwrap().take());
        Ok(())
    })?;
    Ok(Effect::Done)
}

/// This end's process on a far rutis kernel, once that kernel serves rows:
/// a far end that never does (a Cordis node) runs nothing from here.
async fn client_once_served(ctx: Ctx, scoped: Ctx, handle: SessionHandle, client: rows::Client) {
    let mut offers = client.peer.offers();
    if offers
        .wait_for(|offers| offers.epoch("rows").is_some())
        .await
        .is_err()
    {
        return;
    }
    let far = handle.shared.far.to_string();
    let mount = Mount {
        ctx: Some(scoped),
        runtime: Some(far.clone()),
        ..Mount::default()
    };
    match Process::over_without_bus(Arc::new(client), mount).await {
        Ok(process) => {
            if let Err(error) = process
                .check_rows(&far)
                .map_err(failed)
                .and_then(|()| handle.ready(&ctx, process))
            {
                eprintln!("{far}: cannot run plugins there: {error}");
            }
        }
        // Ended meanwhile, or does not run plugins.
        Err(error) => {
            if ctx.cancellation_token().is_cancelled() {
                return;
            }
            eprintln!("{far}: cannot run plugins there: {error}");
        }
    }
}
