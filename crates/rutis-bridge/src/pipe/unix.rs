use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, PoisonError};

use crate::channel::{Close, Closer};

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
    fn close(&self, _why: Close) {
        let stream = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(stream) = stream {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Channel;
    use crate::pipe::lines;
    use std::sync::Arc;

    fn end(stream: UnixStream) -> Channel {
        let reader = stream.try_clone().unwrap();
        let closer = Arc::new(Shut::new(stream.try_clone().unwrap()));
        lines::channel(reader, stream, closer)
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
            lines::limited(reader, stream, closer, 1024)
        };
        let (a, b) = UnixStream::pair().unwrap();
        crate::channel::testing::size_limit((limited(a), limited(b)), 1024);
    }
}
