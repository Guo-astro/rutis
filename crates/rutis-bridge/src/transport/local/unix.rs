use std::io::ErrorKind;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, PoisonError};

use crate::channel::{Channel, ChannelInfo, Closer, ConnectError};

use crate::transport::local::lines;

/// Shuts a Unix socket down, then lets go of its handle of it: the
/// channel's last handle closing is what always tells the far end
/// ([`lines`]).
pub(crate) struct Shut(Mutex<Option<UnixStream>>);

impl Shut {
    pub(crate) fn new(stream: UnixStream) -> Self {
        Self(Mutex::new(Some(stream)))
    }
}

impl Closer for Shut {
    fn close(&self, _reason: &str) {
        let stream = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(stream) = stream {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

pub(crate) async fn dial(path: &str) -> Result<Channel, ConnectError> {
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .and_then(|stream| stream.into_std())
        .and_then(|stream| {
            stream.set_nonblocking(false)?;
            Ok(stream)
        })
        .map_err(|error| classify(path, error))?;
    let split = || -> std::io::Result<_> { Ok((stream.try_clone()?, stream.try_clone()?)) };
    let (reader, closer) = split().map_err(|error| classify(path, error))?;
    Ok(lines::channel(
        reader,
        stream,
        Arc::new(Shut::new(closer)),
        ChannelInfo {
            transport: "unix",
            peer: None,
            label: path.to_owned(),
        },
    ))
}

/// The category comes from the error kind, never from its text.
fn classify(path: &str, error: std::io::Error) -> ConnectError {
    let reason = format!("{path}: {error}");
    match error.kind() {
        ErrorKind::PermissionDenied => ConnectError::AuthRejected { reason },
        _ => ConnectError::Retryable { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(stream: UnixStream) -> Channel {
        let reader = stream.try_clone().unwrap();
        let closer = Arc::new(Shut::new(stream.try_clone().unwrap()));
        lines::channel(reader, stream, closer, ChannelInfo::default())
    }

    #[test]
    fn meets_the_channel_contract() {
        crate::channel::testing::contract(|| {
            let (a, b) = UnixStream::pair().unwrap();
            (end(a), end(b))
        });
    }

    /// Q6.3.2: the size limit, at a size a test can reach quickly; the
    /// default is checked in `lines`.
    #[test]
    fn refuses_messages_over_its_limit() {
        let limited = |stream: UnixStream| {
            let reader = stream.try_clone().unwrap();
            let closer = Arc::new(Shut::new(stream.try_clone().unwrap()));
            lines::limited(reader, stream, closer, ChannelInfo::default(), 1024)
        };
        let (a, b) = UnixStream::pair().unwrap();
        crate::channel::testing::size_limit((limited(a), limited(b)), 1024);
    }
}
