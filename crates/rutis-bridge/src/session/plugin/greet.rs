//! How this end opens a session on a channel: the handshake every session
//! of rutis-bridge starts with (a [`Session`](super::Session), a link).

use std::sync::Arc;
use std::time::Duration;

use crate::channel::{Channel, PeerId};
use crate::peer::Operations;
use crate::session::rpc::{Connection, Endpoint};
use crate::session::{Error, Handshake};

/// How this end opens a session: as `local`, expecting a far end (if it
/// names one), declaring and requiring capabilities.
pub(crate) struct Greet<'a> {
    pub(crate) local: PeerId,
    pub(crate) expected: Option<PeerId>,
    pub(crate) declare: &'a [String],
    pub(crate) require: &'a [String],
    pub(crate) timeout: Duration,
}

impl Greet<'_> {
    /// Open a session on `channel` and wait, at most `timeout`, for the far
    /// end to greet as expected, declaring what is required. On failure the
    /// session is closed, and so it is when this is dropped before it ends
    /// (its channel closed with `Close::Done`).
    pub(crate) async fn open(
        &self,
        channel: Channel,
    ) -> Result<(Connection, Arc<Operations>), Error> {
        let operations = Arc::new(Operations::default());
        let mut endpoint = Endpoint::rust(self.local.clone());
        endpoint.expected = self.expected.clone();
        // A rutis endpoint is a full framework node.
        endpoint.capabilities.push("node".into());
        endpoint.capabilities.extend(self.declare.iter().cloned());
        let session = Connection::open(channel, operations.clone(), endpoint)?;
        let closing = Closing(Some(session.clone()));
        let ready = tokio::time::timeout(self.timeout, session.ready()).await;
        match ready {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                session.close(error.clone());
                return Err(error);
            }
            Err(_) => {
                let error = Error::Transport("the far end did not greet in time".into());
                session.close(error.clone());
                return Err(error);
            }
        }
        // The contract is checked before use, not guessed from failing calls.
        let missing = self
            .require
            .iter()
            .find(|capability| !session.supports(capability));
        if let Some(missing) = missing {
            let far = session.greeting().map_or_else(
                || "the far end".to_owned(),
                |greeting| greeting.endpoint.to_string(),
            );
            let error = Error::Handshake(Handshake::Incompatible(format!(
                "{far} does not declare {missing}"
            )));
            session.close(error.clone());
            return Err(error);
        }
        closing.disarm();
        operations.attach(session.clone());
        Ok((session, operations))
    }
}

/// Closes a session whose handshake did not finish: dropped midway (its
/// caller gone), it ends with its channel.
struct Closing(Option<Connection>);

impl Closing {
    fn disarm(mut self) {
        self.0.take();
    }
}

impl Drop for Closing {
    fn drop(&mut self) {
        if let Some(session) = self.0.take() {
            session.close(Error::Transport("the handshake was abandoned".into()));
        }
    }
}
