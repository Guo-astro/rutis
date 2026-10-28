//! Channels to processes on the same machine: [`spawn`] starts a process
//! on a channel it owns; Unix sockets (Unix) are framed the same way.
//!
//! [`LocalPlugin`] provides `Transport#local`. It dials Unix sockets
//! (`unix:<path>`, or a bare path; Unix), framing messages by newline, and
//! starts processes: `spawn:<name>` starts the process registered as `name`
//! ([`LocalTransport::spawner`], a [`Spawn`]) on an inherited socket (Unix)
//! or a loopback address with a one-time token (every platform) and
//! connects it; the channel owns the process. Unloading the plugin closes
//! every channel it opened, and so ends the processes it started.
//!
//! It knows nothing of what runs in the process: a language runtime started
//! this way is composed on top ([`crate::runtime::LocalRuntime`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use crate::channel::{Channel, Close, ConnectError};
use crate::{transport_key, Dial, Transport};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::{Ctx, Error};

pub(crate) mod lines;

pub use lines::MAX_MESSAGE;

/// Frame a connected byte stream (two handles of one socket, and a closer
/// that wakes both) as a channel, one message per line of at most
/// [`MAX_MESSAGE`] bytes: what this transport's Unix channels are.
pub fn framed(
    read: impl std::io::Read + Send + 'static,
    write: impl std::io::Write + Send + 'static,
    closer: Arc<dyn crate::channel::Closer>,
) -> Channel {
    lines::channel(read, write, closer)
}
mod output;
pub(crate) mod spawn;
#[cfg(unix)]
pub(crate) mod unix;

pub use output::{output_ended, write_line, OutputStream, MAX_LINE};
/// Start `spawn` on a channel that owns the process: closing the channel,
/// or dropping all of it, ends the process, and its end says how the
/// process ended. The process is recorded in `processes`.
pub async fn spawn(spawn: &Spawn, processes: &Processes) -> Result<Channel, ConnectError> {
    spawn::start(spawn, spawn::TOKEN_WAIT, processes).await
}

pub use spawn::{
    isolate_manifest_command, kill_processes, processes_ended, running_processes, Handover,
    ManifestChild, Processes, Spawn, Stdio, CHANNEL_FD, CHANNEL_TOKEN, HANDOVER_VARIABLE,
};

/// Provides `Transport#local`.
#[derive(Default)]
pub struct LocalPlugin;

impl LocalPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl DynPlugin for LocalPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "rutis-bridge/local"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let transport = Arc::new(LocalTransport::default());
            let open = transport.clone();
            ctx.effect(move || async move {
                open.close_all();
                Ok(())
            })?;
            ctx.provide_at(transport_key("local"), transport)?;
            Ok(Effect::Done)
        })
    }
}

/// The transport a [`LocalPlugin`] provides.
#[derive(Default)]
pub struct LocalTransport {
    open: Mutex<Vec<Weak<dyn crate::channel::Closer>>>,
    spawners: Mutex<HashMap<String, Spawn>>,
    trace: Mutex<Option<crate::channel::trace::Sink>>,
    token_wait: Mutex<Option<std::time::Duration>>,
    processes: spawn::Processes,
}

impl LocalTransport {
    /// How long a connection to a started process's loopback listener may
    /// take to present its token before it is dropped: 10 s by default.
    /// Connections present at once each on their own, so a silent one never
    /// holds up the process's ([`Handover::Loopback`]).
    pub fn token_wait(&self, wait: std::time::Duration) {
        *self.token_wait.lock().unwrap() = Some(wait);
    }

    /// A transport recording the processes it starts in `processes`, which
    /// something that outlives it keeps ([`LocalTransport::processes`]).
    pub fn sharing(processes: spawn::Processes) -> Self {
        Self {
            processes,
            ..Self::default()
        }
    }

    /// The processes it started, to tell when they have ended.
    pub fn processes(&self) -> spawn::Processes {
        self.processes.clone()
    }

    /// Let `spawn:<name>` start `spawn`.
    pub fn spawner(&self, name: &str, spawn: Spawn) {
        self.spawners.lock().unwrap().insert(name.to_owned(), spawn);
    }

    /// Report every message crossing the channels it opens from now on
    /// (direction and length, never content) to `sink`.
    pub fn trace(&self, sink: crate::channel::trace::Sink) {
        *self.trace.lock().unwrap() = Some(sink);
    }

    fn track(&self, channel: &Channel) {
        let mut open = self.open.lock().unwrap();
        open.retain(|closer| closer.strong_count() > 0);
        open.push(Arc::downgrade(&channel.closer));
    }

    /// Close every channel it opened, ending the processes it started.
    pub fn close_all(&self) {
        for closer in std::mem::take(&mut *self.open.lock().unwrap()) {
            if let Some(closer) = closer.upgrade() {
                closer.close(Close::Done);
            }
        }
    }
}

impl Transport for LocalTransport {
    fn kind(&self) -> &str {
        "local"
    }

    fn dial<'a>(&'a self, dial: &'a Dial) -> BoxFuture<'a, Result<Channel, ConnectError>> {
        Box::pin(async move {
            let address = dial.address.as_str();
            let channel = match address.strip_prefix("spawn:") {
                Some(name) => self.spawn(name).await?,
                None => connect(address).await?,
            };
            let channel = match self.trace.lock().unwrap().clone() {
                Some(sink) => crate::channel::trace::trace(channel, address, sink),
                None => channel,
            };
            self.track(&channel);
            Ok(channel)
        })
    }
}

impl LocalTransport {
    async fn spawn(&self, name: &str) -> Result<Channel, ConnectError> {
        let spawn = self
            .spawners
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| ConnectError::Incompatible {
                reason: format!("nothing to spawn as {name}"),
            })?;
        let token_wait = self.token_wait.lock().unwrap().unwrap_or(spawn::TOKEN_WAIT);
        spawn::start(&spawn, token_wait, &self.processes).await
    }
}

#[cfg(unix)]
async fn connect(address: &str) -> Result<Channel, ConnectError> {
    let path = match address.split_once(':') {
        Some(("unix", path)) => path,
        Some((scheme, _)) if !scheme.contains('/') => {
            return Err(ConnectError::Incompatible {
                reason: format!("the local transport cannot dial {scheme}: addresses"),
            });
        }
        _ => address,
    };
    unix::dial(path).await
}

#[cfg(not(unix))]
async fn connect(_address: &str) -> Result<Channel, ConnectError> {
    Err(ConnectError::Incompatible {
        reason: "the local transport supports Unix sockets only on this platform".into(),
    })
}
