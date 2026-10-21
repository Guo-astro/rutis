//! The Unix socket channel of the `Process` compatibility facade, and the
//! session constructors that took a socket before sessions ran on any
//! [`Channel`](crate::channel::Channel).
//!
//! Transports belong to the transport modules ([`crate::transport::local`]);
//! this copy exists only because the facade still starts its own processes.
//! It goes once runtimes get their sessions through local + link (N2).
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use crate::channel::{Channel, ChannelInfo};

use crate::runtime::rpc::{Connection, Dispatch};
use crate::runtime::Error;
use crate::transport::local::unix::Shut;

/// A newline-framed channel on a connected Unix socket, one message per
/// line as the runtimes have always spoken: the local transport's framing,
/// with its size limit.
pub(crate) fn channel(stream: UnixStream, label: &str) -> Result<Channel, Error> {
    let transport = |error: std::io::Error| Error::Transport(error.to_string());
    stream.set_nonblocking(false).map_err(transport)?;
    let reader = stream.try_clone().map_err(transport)?;
    let closer = Arc::new(Shut::new(stream.try_clone().map_err(transport)?));
    Ok(crate::transport::local::lines::channel(
        reader,
        stream,
        closer,
        ChannelInfo {
            transport: "unix",
            peer: None,
            label: label.to_owned(),
        },
    ))
}

impl Connection {
    /// A session on a connected Unix socket: [`Connection::open`] on its
    /// newline-framed channel.
    pub fn connect(stream: UnixStream, dispatch: Arc<dyn Dispatch>) -> Result<Self, Error> {
        Self::connect_with(
            stream,
            dispatch,
            Box::new(|| Error::Transport("peer disconnected".into())),
        )
    }

    /// Like [`Connection::connect`]; `disconnected` builds the error that
    /// ends the session when the peer goes away, for example with the exit
    /// status of its process. It runs on the reader thread and may block.
    pub fn connect_with(
        stream: UnixStream,
        dispatch: Arc<dyn Dispatch>,
        disconnected: Box<dyn FnOnce() -> Error + Send>,
    ) -> Result<Self, Error> {
        Self::open(
            crate::runtime::spawn::on_disconnect(channel(stream, "")?, disconnected),
            dispatch,
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn meets_the_channel_contract() {
        crate::channel::testing::contract(|| {
            let (a, b) = super::UnixStream::pair().unwrap();
            (
                super::channel(a, "").unwrap(),
                super::channel(b, "").unwrap(),
            )
        });
    }
}
