//! The channel a rutis interop session runs on
//! (`docs/design-protocol-channel-decoupling-2026-10-03.md`).
//!
//! A [`Channel`] is one ordered, reliable, duplex stream of messages: every
//! message sent arrives once, in order, with its boundaries kept. It moves
//! opaque bytes and knows nothing of protocol frames or their encoding.
//! Implementations, with their framing, limits and liveness:
//! [`crate::memory::pair`], [`crate::pipe::spawn`] and, with feature
//! `websocket`, `websocket::dial` and `websocket::listen`.

use std::sync::Arc;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod trace;

/// One established channel. The sender and receiver are used from one
/// thread each; the closer may be called from anywhere. Who is at the far
/// end is said by the far end itself, in the session's handshake.
pub struct Channel {
    pub sender: Box<dyn Sender>,
    pub receiver: Box<dyn Receiver>,
    pub closer: Arc<dyn Closer>,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel").finish_non_exhaustive()
    }
}

pub trait Sender: Send {
    /// Send one message; blocks under backpressure.
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError>;

    /// The largest message this sender takes, if it has a limit: a longer
    /// one ends the channel. A session checks what it would send against
    /// it first, so that a message over it fails only its own call (#675).
    fn max_message(&self) -> Option<usize> {
        None
    }
}

pub trait Receiver: Send {
    /// Block until the next message; `Ok(None)` when the far end finished
    /// normally.
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError>;
}

pub trait Closer: Send + Sync {
    /// Close the channel, saying why to the far end where the channel can.
    /// Idempotent (the first close counts); wakes threads blocked in
    /// `send` and `recv`.
    fn close(&self, why: Close);
}

/// Why a channel is closed. The far end learns which, where the channel
/// can tell it: dialers decide whether to retry by it
/// (`bridge/dial-out-rules`).
///
/// | `Close` | WebSocket | process pipe | memory |
/// | --- | --- | --- | --- |
/// | `Done` | close code 1000 | the child's end of the channel closed | the far end's `recv` gives `Ok(None)` |
/// | `Replaced` | close code 4002 | as `Done` | the far end's `recv` gives `Closed` |
/// | `Rejected(reason)` | close code 4003 with `reason` | as `Done` | the far end's `recv` gives `Rejected` |
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Close {
    /// A normal end.
    Done,
    /// A newer connection of the same far end took over.
    Replaced,
    /// The far end was refused after the channel was established: an
    /// authentication failure.
    Rejected(String),
}

/// An endpoint id: lowercase letters, digits and `-`, unique in a
/// deployment. The far end need not be a full framework node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId(String);

impl PeerId {
    pub fn new(id: impl Into<String>) -> Result<Self, InvalidPeerId> {
        let id = id.into();
        let valid = !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if valid {
            Ok(Self(id))
        } else {
            Err(InvalidPeerId(id))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid endpoint id {0:?}: use lowercase letters, digits and `-`")]
pub struct InvalidPeerId(pub String);

/// How an established channel ended. The reason is for diagnostics only:
/// no code branches on it. Why a transport ends a channel (a lost peer, a
/// message over its size limit, a failed heartbeat) is the transport's own
/// business.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChannelError {
    #[error("{reason}")]
    Closed { reason: String },
    /// The far end rejected this end's identity after the channel was
    /// established (a listener refusing the id this end greeted
    /// with): an authentication failure, which a session ends with as an
    /// identity mismatch. Only a transport that can say so reports it.
    #[error("{reason}")]
    Rejected { reason: String },
}

/// Why a channel could not be established. The category decides what a
/// link does next; `reason` is for diagnostics and carries no credentials.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// Temporary: refused, timed out, unavailable. Retry with backoff.
    #[error("{reason}")]
    Retryable { reason: String },
    /// Credentials, certificate, identity mapping or registration refused.
    /// Retry slowly and keep reporting.
    #[error("{reason}")]
    AuthRejected { reason: String },
    /// Subprotocol or a required transport capability does not match. Stop
    /// retrying until the configuration changes.
    #[error("{reason}")]
    Incompatible { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ids_are_lowercase_letters_digits_and_dashes() {
        assert!(PeerId::new("mac-2").is_ok());
        assert!(PeerId::new("").is_err());
        assert!(PeerId::new("Mac").is_err());
        assert!(PeerId::new("a/b").is_err());
        assert!(PeerId::new("a:1").is_err());
    }
}
