//! When to start a runtime again after its process ended on its own: the
//! one backoff of every runtime restart, rutis-host's local runtimes'
//! (`restart: "on-failure"`) and the compiled-program runtimes'
//! (`RestartPolicy::OnFailure`). The numbers are fixed, not configurable.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

/// The wait before the first restart; each one after doubles it.
pub const FIRST: Duration = Duration::from_secs(1);

/// The longest wait before a restart.
pub const LONGEST: Duration = Duration::from_secs(30);

/// A runtime that ran this long before it ended starts over from
/// [`FIRST`].
pub const STABLE: Duration = Duration::from_secs(60);

/// At most [`LIMIT`] restarts within this long; one more end gives up.
pub const WINDOW: Duration = Duration::from_secs(5 * 60);

/// How many restarts [`WINDOW`] allows.
pub const LIMIT: usize = 5;

/// When to start a runtime again after it ended.
#[derive(Debug)]
pub struct Backoff {
    /// The next wait.
    wait: Duration,
    /// When it was restarted, within the last [`WINDOW`].
    restarts: VecDeque<Instant>,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            wait: FIRST,
            restarts: VecDeque::new(),
        }
    }
}

impl Backoff {
    /// The runtime ended at `now`, `ran` after it last started: how long to
    /// wait before starting it again, or `None` to give up, when it was
    /// restarted [`LIMIT`] times within the last [`WINDOW`].
    pub fn next(&mut self, now: Instant, ran: Duration) -> Option<Duration> {
        if ran >= STABLE {
            self.wait = FIRST;
        }
        while self
            .restarts
            .front()
            .is_some_and(|restart| now.duration_since(*restart) >= WINDOW)
        {
            self.restarts.pop_front();
        }
        if self.restarts.len() >= LIMIT {
            return None;
        }
        let wait = self.wait;
        self.wait = (self.wait * 2).min(LONGEST);
        self.restarts.push_back(now + wait);
        Some(wait)
    }
}
