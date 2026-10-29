//! Channels to processes on the same machine: [`spawn`] starts a process
//! on an inherited socket (Unix) or a loopback address with a one-time
//! token (every platform) and connects it; the channel owns the process.
//! Unix sockets are framed the same way, by newline.
//!
//! It knows nothing of what runs in the process: a language runtime started
//! this way is a session's far end ([`crate::runtime::Launcher::start`],
//! [`crate::Session`]).

use std::sync::Arc;

use crate::channel::{Channel, ConnectError};

pub(crate) mod lines;

pub use lines::MAX_MESSAGE;

/// Frame a connected byte stream (two handles of one socket, and a closer
/// that wakes both) as a channel, one message per line of at most
/// [`MAX_MESSAGE`] bytes: what Unix socket channels are.
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
    spawn::start(spawn, processes).await
}

pub use spawn::{
    isolate_manifest_command, kill_processes, processes_ended, running_processes, Handover,
    ManifestChild, Processes, Spawn, Stdio, CHANNEL_FD, CHANNEL_TOKEN, HANDOVER_VARIABLE,
};

/// Dial a Unix socket: `unix:<path>`, or a bare path. Transitional
/// (#695 S2): for the node rows that link over a Unix socket, which go
/// with links.
#[doc(hidden)]
#[cfg(unix)]
pub async fn dial(address: &str) -> Result<Channel, ConnectError> {
    let path = match address.split_once(':') {
        Some(("unix", path)) => path,
        Some((scheme, _)) if !scheme.contains('/') => {
            return Err(ConnectError::Incompatible {
                reason: format!("cannot dial {scheme}: addresses on a Unix socket"),
            });
        }
        _ => address,
    };
    unix::dial(path).await
}

#[doc(hidden)]
#[cfg(not(unix))]
pub async fn dial(_address: &str) -> Result<Channel, ConnectError> {
    Err(ConnectError::Incompatible {
        reason: "Unix sockets are not available on this platform".into(),
    })
}
