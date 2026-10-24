//! Test hooks of a host's logical bus (feature `testing`): what the events
//! conformance suite (`runtime::testing::events`) asks of the host itself.
//!
//! - [`Tap::dispatches`]: how many `events.dispatch` requests the host
//!   received from its runtimes (`viaHost`);
//! - [`Tap::answered`]: wait until the host has written its reply to each
//!   of them (`answered`);
//! - [`Tap::hold`]: keep the next `n` of them until all `n` arrived, so they
//!   run at the same time (`hold`);
//! - [`Tap::reported`]: the lines the host printed for event failures its
//!   error sink received (events protocol §5.6, `reported`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rutis::host::ErrorSink;
use rutis::Ctx;

use super::hub::Hub;
use crate::session::Error;

/// How long a held dispatch waits for the others before it runs anyway.
const HOLD: Duration = Duration::from_secs(10);

/// The test hooks of one host's bus. Make it before the root
/// ([`Tap::sink`] is the root's error sink), then [`Tap::attach`] it.
#[derive(Clone, Default)]
pub struct Tap(Arc<TapState>);

#[derive(Default)]
pub(crate) struct TapState {
    dispatches: AtomicU64,
    /// The dispatches whose reply was written.
    replies: AtomicU64,
    /// Woken at each reply.
    replied: tokio::sync::Notify,
    hold: Mutex<Hold>,
    released: Condvar,
    reported: Mutex<Vec<String>>,
    /// The hub tapped: it lives while the tap does.
    hub: Mutex<Option<Arc<Hub>>>,
}

#[derive(Default)]
struct Hold {
    /// How many dispatches the hold waits for; `0`: none is held.
    want: usize,
    arrived: usize,
    /// Advances when a hold is released.
    round: u64,
}

impl Tap {
    pub fn new() -> Tap {
        Tap::default()
    }

    /// An error sink that records the §5.6 line of each event failure and
    /// passes every error on to `then`.
    pub fn sink(&self, then: ErrorSink) -> ErrorSink {
        let state = self.0.clone();
        Arc::new(move |error| {
            if let Some(line) = super::report_line(&error) {
                state.reported.lock().unwrap().push(line);
            }
            then(error)
        })
    }

    /// Tap the bus of `ctx` (its root's).
    pub fn attach(&self, ctx: &Ctx) -> Result<(), Error> {
        let hub = Hub::of(ctx)?;
        hub.tap(self.0.clone());
        *self.0.hub.lock().unwrap() = Some(hub);
        Ok(())
    }

    /// The `events.dispatch` requests received so far: each is counted as
    /// its session reads it, so one a runtime sent before answering a call
    /// is counted once that answer is.
    pub fn dispatches(&self) -> u64 {
        self.0.dispatches.load(Ordering::SeqCst)
    }

    /// Wait until the host has written its reply to every `events.dispatch`
    /// it received so far: a frame the suite has sent to a runtime after
    /// this returns reaches it after those replies, so an emitter that
    /// takes its replies in frame order (events protocol §5.5) has every
    /// earlier emit through the host answered when that frame starts
    /// another. Fails after `within`.
    pub async fn answered(&self, within: Duration) -> Result<(), String> {
        let wanted = self.dispatches();
        let waited = tokio::time::timeout(within, async {
            loop {
                let replied = self.0.replied.notified();
                if self.0.replies.load(Ordering::SeqCst) >= wanted {
                    return;
                }
                replied.await;
            }
        });
        waited.await.map_err(|_| {
            format!(
                "the host answered {} of {wanted} events.dispatch within {within:?}",
                self.0.replies.load(Ordering::SeqCst)
            )
        })
    }

    /// Hold the next `n` dispatches until all `n` arrived.
    pub fn hold(&self, n: usize) {
        let mut hold = self.0.hold.lock().unwrap();
        hold.want = n;
        hold.arrived = 0;
    }

    /// The lines of the event failures reported so far.
    pub fn reported(&self) -> Vec<String> {
        self.0.reported.lock().unwrap().clone()
    }
}

impl TapState {
    /// An `events.dispatch` was read: counted on the session's reader,
    /// before its next frame, so a reply the runtime sent after it (the
    /// emitter's) finds it counted.
    pub(crate) fn received(&self) {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
    }

    /// The reply to an `events.dispatch` was written.
    pub(crate) fn replied(&self) {
        self.replies.fetch_add(1, Ordering::SeqCst);
        self.replied.notify_waiters();
    }

    /// An `events.dispatch` is about to run: held if a hold waits for it
    /// (the thread blocks until the hold is complete).
    pub(crate) fn starting(&self) {
        let mut hold = self.hold.lock().unwrap();
        if hold.want == 0 {
            return;
        }
        hold.arrived += 1;
        if hold.arrived >= hold.want {
            hold.want = 0;
            hold.arrived = 0;
            hold.round += 1;
            self.released.notify_all();
            return;
        }
        let round = hold.round;
        let wait = |hold| {
            self.released
                .wait_timeout_while(hold, HOLD, |hold: &mut Hold| hold.round == round)
                .unwrap()
                .0
        };
        // On a worker of a multi-threaded runtime, its other tasks move to
        // another thread meanwhile.
        let multi = tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        });
        let mut hold = if multi {
            tokio::task::block_in_place(|| wait(hold))
        } else {
            wait(hold)
        };
        if hold.round == round {
            // Timed out: the others never came; run anyway.
            hold.want = 0;
            hold.arrived = 0;
            hold.round += 1;
            self.released.notify_all();
        }
    }
}
