//! A bounded queue between a blocking side (the channel's caller) and an
//! async side (the connection task). Either end may close it, waking both.

use std::collections::VecDeque;

#[cfg(rutis_shuttle)]
use crate::sync::{Condvar, Mutex};
#[cfg(rutis_shuttle)]
use shuttle::sync::MutexGuard;
/// The lock and the wakeup of the pipe's queue, which the shuttle model
/// interleaves (#628). A plain build is std's.
#[cfg(not(rutis_shuttle))]
use std::sync::{Condvar, Mutex, MutexGuard};

use crate::channel::ChannelError;
use tokio::sync::Notify;

/// How a pipe ended, as its consumer sees it once drained.
#[derive(Clone, Debug)]
pub(crate) enum Ending {
    /// The far end finished normally.
    Finished,
    Failed(String),
    /// The far end rejected this end's identity (close code 4003).
    Rejected(String),
}

struct State {
    queue: VecDeque<Vec<u8>>,
    bytes: usize,
    ending: Option<Ending>,
    /// Threads waiting on this pipe, for the model (#628).
    #[cfg(all(test, rutis_shuttle))]
    waiting: usize,
}

pub(crate) struct Pipe {
    state: Mutex<State>,
    capacity: usize,
    /// Wakes blocking waiters.
    changed: Condvar,
    /// Wakes the async side.
    notify: Notify,
}

impl Pipe {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                bytes: 0,
                ending: None,
                #[cfg(all(test, rutis_shuttle))]
                waiting: 0,
            }),
            capacity,
            changed: Condvar::new(),
            notify: Notify::new(),
        }
    }

    fn has_room(&self, state: &State, size: usize) -> bool {
        // One message always fits, however large.
        state.queue.is_empty() || state.bytes + size <= self.capacity
    }

    /// Wait for a change, counted in `waiting` under test.
    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        #[cfg(all(test, rutis_shuttle))]
        let mut state = state;
        #[cfg(all(test, rutis_shuttle))]
        {
            state.waiting += 1;
        }
        #[allow(unused_mut)]
        let mut state = self.changed.wait(state).unwrap();
        #[cfg(all(test, rutis_shuttle))]
        {
            state.waiting -= 1;
        }
        state
    }

    /// How many threads wait on this pipe now.
    #[cfg(all(test, rutis_shuttle))]
    fn waiting(&self) -> usize {
        self.state.lock().unwrap().waiting
    }

    fn admit(&self, state: &mut State, message: Vec<u8>) {
        state.bytes += message.len();
        state.queue.push_back(message);
        self.changed.notify_all();
        self.notify.notify_waiters();
        self.notify.notify_one();
    }

    fn take(&self, state: &mut State) -> Option<Vec<u8>> {
        let message = state.queue.pop_front()?;
        state.bytes -= message.len();
        self.changed.notify_all();
        self.notify.notify_waiters();
        self.notify.notify_one();
        Some(message)
    }

    /// Queue `message`, blocking while the pipe is full.
    pub(crate) fn push_blocking(&self, message: &[u8]) -> Result<(), ChannelError> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(ending) = &state.ending {
                return Err(closed(ending));
            }
            if self.has_room(&state, message.len()) {
                self.admit(&mut state, message.to_vec());
                return Ok(());
            }
            state = self.wait(state);
        }
    }

    /// Queue `message`, waiting while the pipe is full. `false` once closed.
    pub(crate) async fn push(&self, message: Vec<u8>) -> bool {
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self.state.lock().unwrap();
                if state.ending.is_some() {
                    return false;
                }
                if self.has_room(&state, message.len()) {
                    self.admit(&mut state, message);
                    return true;
                }
            }
            notified.await;
        }
    }

    /// The next message, blocking while there is none. `Ok(None)` once
    /// drained after a normal end.
    pub(crate) fn pop_blocking(&self) -> Result<Option<Vec<u8>>, ChannelError> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(message) = self.take(&mut state) {
                return Ok(Some(message));
            }
            match &state.ending {
                Some(Ending::Finished) => return Ok(None),
                Some(ending) => return Err(closed(ending)),
                None => state = self.wait(state),
            }
        }
    }

    /// The next message, waiting while there is none; `None` once closed.
    /// Messages still queued at the close are dropped.
    pub(crate) async fn pop(&self) -> Option<Vec<u8>> {
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self.state.lock().unwrap();
                if state.ending.is_some() {
                    return None;
                }
                if let Some(message) = self.take(&mut state) {
                    return Some(message);
                }
            }
            notified.await;
        }
    }

    /// End the pipe; the first ending wins.
    pub(crate) fn close(&self, ending: Ending) {
        let mut state = self.state.lock().unwrap();
        state.ending.get_or_insert(ending);
        self.changed.notify_all();
        self.notify.notify_waiters();
        self.notify.notify_one();
    }
}

fn closed(ending: &Ending) -> ChannelError {
    match ending {
        Ending::Finished => ChannelError::Closed {
            reason: "closed".into(),
        },
        Ending::Failed(reason) => ChannelError::Closed {
            reason: reason.clone(),
        },
        Ending::Rejected(reason) => ChannelError::Rejected {
            reason: reason.clone(),
        },
    }
}

/// The shuttle model of the pipe's blocking side (#628).
#[cfg(all(test, rutis_shuttle))]
mod shuttle_tests;
