//! The Unix socket channel of the `Process` compatibility facade.
//!
//! Transports belong to the transport modules ([`crate::pipe`]);
//! this copy exists only because the facade still starts its own processes.
//! It goes once runtimes get their sessions through local + link (N2).
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use crate::channel::Channel;

use crate::pipe::unix::Shut;
use crate::runtime::Error;

/// A newline-framed channel on a connected Unix socket, one message per
/// line as the runtimes have always spoken: the local transport's framing,
/// with its size limit.
pub(crate) fn channel(stream: UnixStream) -> Result<Channel, Error> {
    let transport = |error: std::io::Error| Error::Transport(error.to_string());
    stream.set_nonblocking(false).map_err(transport)?;
    let reader = stream.try_clone().map_err(transport)?;
    let closer = Arc::new(Shut::new(stream.try_clone().map_err(transport)?));
    Ok(crate::pipe::lines::channel(reader, stream, closer))
}

#[cfg(test)]
mod tests {
    #[test]
    fn meets_the_channel_contract() {
        crate::channel::testing::contract(|| {
            let (a, b) = super::UnixStream::pair().unwrap();
            (super::channel(a).unwrap(), super::channel(b).unwrap())
        });
    }
}
