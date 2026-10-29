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

/// Open the channel this process was started on: the channel argument it
/// was given by a process that started it as [`spawn`] does (`fd:<n>`, or
/// `tcp:<host>:<port>` with the token in [`CHANNEL_TOKEN`]).
pub async fn given(channel: &str) -> Result<Channel, ConnectError> {
    spawn::given(channel).await
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

/// Wait until the process on `channel` sends its first message (its
/// greeting), for at most `limit` (`Duration::MAX`: for as long as it
/// takes, a process stopped at its first line for a debugger), then give
/// the channel back with that message first. A session's own handshake
/// waits 10 s from when it starts: a local process may take longer to
/// start (a cold interpreter, a debugger). Not greeted in time, ended
/// first, or the wait dropped: the channel is closed.
pub async fn greeted(channel: Channel, limit: std::time::Duration) -> Result<Channel, String> {
    let Channel {
        sender,
        mut receiver,
        closer,
    } = channel;
    let closing = closer.clone();
    // Closes the channel, waking the read, unless the greeting came.
    struct Abandon(Option<Arc<dyn crate::channel::Closer>>);
    impl Drop for Abandon {
        fn drop(&mut self) {
            if let Some(closer) = self.0.take() {
                closer.close(crate::channel::Close::Done);
            }
        }
    }
    let mut abandon = Abandon(Some(closing));
    let read = tokio::task::spawn_blocking(move || {
        let first = receiver.recv();
        (first, receiver)
    });
    let read = async {
        read.await
            .map_err(|_| "the runtime's channel failed".to_owned())
    };
    let (first, receiver) = match limit {
        std::time::Duration::MAX => read.await?,
        limit => tokio::time::timeout(limit, read)
            .await
            .map_err(|_| format!("the runtime did not greet within {}s", limit.as_secs_f64()))??,
    };
    let first = match first {
        Ok(Some(first)) => first,
        Ok(None) => return Err("the runtime ended before it greeted".into()),
        Err(error) => return Err(format!("the runtime ended before it greeted: {error}")),
    };
    abandon.0 = None;
    Ok(Channel {
        sender,
        receiver: Box::new(First {
            first: Some(first),
            rest: receiver,
        }),
        closer,
    })
}

/// A receiver whose first message was already read.
struct First {
    first: Option<Vec<u8>>,
    rest: Box<dyn crate::channel::Receiver>,
}

impl crate::channel::Receiver for First {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, crate::channel::ChannelError> {
        match self.first.take() {
            Some(first) => Ok(Some(first)),
            None => self.rest.recv(),
        }
    }
}
