//! Channels within one process, for tests and in-process sessions:
//! bounded, [`CAPACITY`] messages per direction.
//!
//! [`pair`] makes two connected channels.

use std::collections::VecDeque;
use std::sync::Arc;

#[cfg(rutis_shuttle)]
use crate::sync::{Condvar, Mutex};
#[cfg(rutis_shuttle)]
use shuttle::sync::MutexGuard;
/// The lock and the wakeup of a direction, which the shuttle model
/// interleaves (#628). A plain build is std's.
#[cfg(not(rutis_shuttle))]
use std::sync::{Condvar, Mutex, MutexGuard};

use crate::channel::{Channel, ChannelError, Close, Closer, Receiver, Sender};

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

    /// Set the close reason; the waiters wake only once every direction has
    /// it ([`MemoryCloser::close`]).
    fn mark_closed(&self, why: &Close) {
        self.state
            .lock()
            .unwrap()
            .closed
            .get_or_insert_with(|| why.clone());
    }

    /// Wake every thread waiting on this direction.
    fn wake(&self) {
        self.changed.notify_all();
    }

    /// Wait for a change, counted in `waiting` under test.
    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
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

/// What the two ends of one pair share: why it closed, decided once by the
/// first end to close (#624).
#[derive(Default)]
struct Pair {
    /// The first close's reason. Both directions end with it, so a far end
    /// reads the rejection even when this end's own teardown (`Done`) closes
    /// at the same time.
    reason: Mutex<Option<Close>>,
    /// A test's step after a close has taken its reason on both directions
    /// and before either is woken (#624).
    #[cfg(test)]
    between: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

struct MemoryCloser {
    incoming: Arc<Direction>,
    outgoing: Arc<Direction>,
    own: Arc<Mutex<bool>>,
    pair: Arc<Pair>,
}
impl Closer for MemoryCloser {
    fn close(&self, why: Close) {
        // One reason for the pair, decided before either direction closes: a
        // far end's rejection (4003) must not be lost to this end's own
        // teardown (`Done`) racing it (#624).
        let why = {
            let mut reason = self.pair.reason.lock().unwrap();
            reason.get_or_insert(why).clone()
        };
        *self.own.lock().unwrap() = true;
        // Both directions take the reason before either wakes a waiter: this
        // end's reader wakes first otherwise, drops its sender, and the far
        // end woken by that reads a plain end (`Ok(None)`) in the gap before
        // the outgoing direction is closed (#1295).
        for direction in [&self.outgoing, &self.incoming] {
            direction.mark_closed(&why);
        }
        #[cfg(test)]
        {
            let between = self.pair.between.lock().unwrap().take();
            if let Some(between) = between {
                between();
            }
        }
        for direction in [&self.outgoing, &self.incoming] {
            direction.wake();
        }
    }
}

fn end(incoming: &Arc<Direction>, outgoing: &Arc<Direction>, pair: &Arc<Pair>) -> Channel {
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
            pair: pair.clone(),
        }),
    }
}

/// Two channels joined end to end: what one sends, the other receives.
pub fn pair() -> (Channel, Channel) {
    let (a, b) = (Arc::default(), Arc::default());
    let pair = Arc::default();
    (end(&a, &b, &pair), end(&b, &a, &pair))
}

/// [`pair`], with what the first channel sends into and what it receives
/// from, to see who is blocked.
#[cfg(test)]
fn pair_watched() -> (Channel, Channel, Arc<Direction>, Arc<Direction>) {
    let (a, b): (Arc<Direction>, Arc<Direction>) = (Arc::default(), Arc::default());
    let pair = Arc::default();
    (end(&a, &b, &pair), end(&b, &a, &pair), b, a)
}

#[cfg(test)]
mod tests;

/// The shuttle model of a channel's close, send and receive (#628).
#[cfg(all(test, rutis_shuttle))]
mod shuttle_tests;
