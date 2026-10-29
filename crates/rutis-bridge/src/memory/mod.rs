//! Channels within one process, for tests and in-process links:
//! bounded, [`CAPACITY`] messages per direction.
//!
//! [`pair`] makes two connected channels.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use crate::channel::{Channel, ChannelError, Close, Closer, ConnectError, Receiver, Sender};
use crate::{Presented, Refusal, Registrations};

/// Messages buffered per direction before `send` blocks.
pub const CAPACITY: usize = 64;

#[derive(Default)]
struct State {
    queue: VecDeque<Vec<u8>>,
    /// The sending end finished; the receiver drains, then sees the end.
    finished: bool,
    /// Closed by either end, and why: no more sends, receivers stop.
    closed: Option<Close>,
    /// Threads waiting on this direction, for tests that need "it blocks"
    /// as a state to wait for rather than a pause (#231).
    #[cfg(test)]
    waiting: usize,
}

#[derive(Default)]
struct Direction {
    state: Mutex<State>,
    changed: Condvar,
}

impl Direction {
    fn update(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.state.lock().unwrap());
        self.changed.notify_all();
    }

    /// Wait for a change, counted in `waiting` under test.
    fn wait<'a>(
        &self,
        state: std::sync::MutexGuard<'a, State>,
    ) -> std::sync::MutexGuard<'a, State> {
        #[cfg(test)]
        let state = {
            let mut state = state;
            state.waiting += 1;
            state
        };
        #[allow(unused_mut)]
        let mut state = self.changed.wait(state).unwrap();
        #[cfg(test)]
        {
            state.waiting -= 1;
        }
        state
    }

    /// How many threads wait on this direction now.
    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.state.lock().unwrap().waiting
    }
}

struct MemorySender(Arc<Direction>);
impl Sender for MemorySender {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if let Some(why) = &state.closed {
                return Err(ended(why));
            }
            if state.queue.len() < CAPACITY {
                state.queue.push_back(message.to_vec());
                self.0.changed.notify_all();
                return Ok(());
            }
            state = self.0.wait(state);
        }
    }
}
impl Drop for MemorySender {
    fn drop(&mut self) {
        self.0.update(|state| state.finished = true);
    }
}

/// What a send or receive on a closed channel reports, `why` it closed.
fn ended(why: &Close) -> ChannelError {
    match why {
        Close::Done => ChannelError::Closed {
            reason: "the channel is closed".into(),
        },
        Close::Replaced => ChannelError::Closed {
            reason: "replaced by a new connection".into(),
        },
        Close::Rejected(reason) => ChannelError::Rejected {
            reason: format!("rejected: {reason}"),
        },
    }
}

struct MemoryReceiver {
    incoming: Arc<Direction>,
    /// Set when this end closed itself: its own receive reports the close.
    own: Arc<Mutex<bool>>,
}
impl Receiver for MemoryReceiver {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        let mut state = self.incoming.state.lock().unwrap();
        loop {
            if *self.own.lock().unwrap() {
                return Err(ChannelError::Closed {
                    reason: "closed here".into(),
                });
            }
            if let Some(message) = state.queue.pop_front() {
                self.incoming.changed.notify_all();
                return Ok(Some(message));
            }
            match &state.closed {
                Some(Close::Done) => return Ok(None),
                Some(why) => return Err(ended(why)),
                None if state.finished => return Ok(None),
                None => {}
            }
            state = self.incoming.wait(state);
        }
    }
}
impl Drop for MemoryReceiver {
    fn drop(&mut self) {
        self.incoming.update(|state| {
            state.closed.get_or_insert(Close::Done);
        });
    }
}

struct MemoryCloser {
    incoming: Arc<Direction>,
    outgoing: Arc<Direction>,
    own: Arc<Mutex<bool>>,
}
impl Closer for MemoryCloser {
    fn close(&self, why: Close) {
        *self.own.lock().unwrap() = true;
        for direction in [&self.incoming, &self.outgoing] {
            direction.update(|state| {
                state.closed.get_or_insert_with(|| why.clone());
            });
        }
    }
}

fn end(incoming: &Arc<Direction>, outgoing: &Arc<Direction>) -> Channel {
    let own = Arc::new(Mutex::new(false));
    Channel {
        sender: Box::new(MemorySender(outgoing.clone())),
        receiver: Box::new(MemoryReceiver {
            incoming: incoming.clone(),
            own: own.clone(),
        }),
        closer: Arc::new(MemoryCloser {
            incoming: incoming.clone(),
            outgoing: outgoing.clone(),
            own,
        }),
    }
}

/// Two channels joined end to end: what one sends, the other receives.
pub fn pair() -> (Channel, Channel) {
    let (a, b) = (Arc::default(), Arc::default());
    (end(&a, &b), end(&b, &a))
}

/// [`pair`], with what the first channel sends into and what it receives
/// from, to see who is blocked.
#[cfg(test)]
fn pair_watched() -> (Channel, Channel, Arc<Direction>, Arc<Direction>) {
    let (a, b): (Arc<Direction>, Arc<Direction>) = (Arc::default(), Arc::default());
    (end(&a, &b), end(&b, &a), b, a)
}

/// Dial `listener` of `registrations` in this process, presenting the
/// bearer token the link's identity gives: routed as a network connection
/// is, to the registration whose identity verifies it. Transitional
/// (#695 S2): goes with links.
#[doc(hidden)]
pub fn dialer(registrations: Registrations, listener: &str) -> crate::Dialer {
    let listener = listener.to_owned();
    std::sync::Arc::new(move |credential: Option<crate::Credential>| {
        let result = dial_registered(&registrations, &listener, credential);
        Box::pin(async move { result })
    })
}

fn dial_registered(
    registrations: &Registrations,
    listener: &str,
    credential: Option<crate::Credential>,
) -> Result<Channel, ConnectError> {
    let Some(crate::Credential::Bearer(token)) = credential else {
        return Err(ConnectError::AuthRejected {
            reason: format!("memory:{listener}: credentials required"),
        });
    };
    let ticket = registrations
        .route(listener, Presented::Bearer(&token))
        .map_err(|refusal| match refusal {
            Refusal::NotListening => ConnectError::Retryable {
                reason: format!("memory:{listener}: not listening for this peer yet"),
            },
            Refusal::Unauthenticated => ConnectError::AuthRejected {
                reason: format!("memory:{listener}: not accepted here"),
            },
            Refusal::Ambiguous => ConnectError::AuthRejected {
                reason: format!("memory:{listener}: ambiguous credentials"),
            },
        })?;
    if ticket.protocol != crate::protocol() {
        return Err(ConnectError::Incompatible {
            reason: format!("memory:{listener} speaks {}", ticket.protocol),
        });
    }
    let (dialed, accepted) = pair();
    match registrations.hand_over(&ticket, accepted) {
        Ok(()) => Ok(dialed),
        // Revoked since it was routed: nobody listens for it now, as when
        // it was never registered.
        Err(_) => Err(ConnectError::Retryable {
            reason: format!("memory:{listener}: registration revoked"),
        }),
    }
}

#[cfg(test)]
mod tests;
