//! Wait helpers for the rutis tests (quality standard Q7.1, #231).
//!
//! A timeout here only guards against a hang ([`HANG_GUARD`]); it never
//! stands in for an event. Wait for what the test is about: a channel, a
//! watch, a line, a state. Poll ([`eventually`]) only where nothing can be
//! awaited, and then say on timeout what the state was
//! ([`eventually_with`]). Checking that something has *not* happened is
//! [`still_pending`] on the paused clock, in-process; across threads and
//! processes, see the rules in docs/design-test-clocks-2026-10-11.md §3.

use std::future::Future;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

/// How long a wait lasts before the test fails as hung. It only guards
/// against hangs, and is never what a test measures. A wait that needs
/// longer (several runtimes starting cold) uses a multiple, with the reason
/// next to it.
pub const HANG_GUARD: Duration = Duration::from_secs(10);

/// How often [`eventually`] looks again.
const POLL: Duration = Duration::from_millis(10);

/// `f`, unless it takes longer than [`HANG_GUARD`]: then the test fails,
/// naming `what`.
pub async fn within<F: Future>(what: &str, f: F) -> F::Output {
    Guard(HANG_GUARD).within(what, f).await
}

/// The first `Some` of `check`, looked at every 10 ms, within
/// [`HANG_GUARD`]; else the test fails, naming `what`.
pub async fn eventually<T>(check: impl FnMut() -> Option<T>, what: &str) -> T {
    Guard(HANG_GUARD).eventually(check, what).await
}

/// [`eventually`] for a condition: until `check()` is true.
pub async fn until(check: impl FnMut() -> bool, what: &str) {
    Guard(HANG_GUARD).until(check, what).await
}

/// [`eventually`], with `describe()` (the state at the time) in the failure.
pub async fn eventually_with<T>(
    check: impl FnMut() -> Option<T>,
    what: &str,
    describe: impl FnOnce() -> String,
) -> T {
    Guard(HANG_GUARD)
        .eventually_with(check, what, describe)
        .await
}

/// A longer hang guard, for the waits that need one: `Guard(HANG_GUARD *
/// 2)`, with the reason next to it (several runtimes starting cold, #238).
/// It only guards against hangs too.
#[derive(Clone, Copy, Debug)]
pub struct Guard(pub Duration);

impl Guard {
    /// [`within`] this guard.
    pub async fn within<F: Future>(self, what: &str, f: F) -> F::Output {
        match tokio::time::timeout(self.0, f).await {
            Ok(output) => output,
            Err(_) => panic!("timed out ({:?}): {what}", self.0),
        }
    }

    /// [`eventually`] within this guard.
    pub async fn eventually<T>(self, check: impl FnMut() -> Option<T>, what: &str) -> T {
        self.eventually_with(check, what, String::new).await
    }

    /// [`until`] within this guard.
    pub async fn until(self, mut check: impl FnMut() -> bool, what: &str) {
        self.eventually(|| check().then_some(()), what).await
    }

    /// [`eventually_with`] within this guard.
    pub async fn eventually_with<T>(
        self,
        mut check: impl FnMut() -> Option<T>,
        what: &str,
        describe: impl FnOnce() -> String,
    ) -> T {
        let deadline = tokio::time::Instant::now() + self.0;
        loop {
            if let Some(value) = check() {
                return value;
            }
            if tokio::time::Instant::now() >= deadline {
                fail(self.0, what, describe());
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

/// [`eventually`] on a plain thread, outside any runtime.
pub fn eventually_blocking<T>(mut check: impl FnMut() -> Option<T>, what: &str) -> T {
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if let Some(value) = check() {
            return value;
        }
        if Instant::now() >= deadline {
            fail(HANG_GUARD, what, String::new());
        }
        std::thread::sleep(POLL);
    }
}

/// The next message of `rx`, within [`HANG_GUARD`].
pub fn recv_within<T>(what: &str, rx: &std::sync::mpsc::Receiver<T>) -> T {
    match rx.recv_timeout(HANG_GUARD) {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error} (hang guard {HANG_GUARD:?})"),
    }
}

fn fail(guard: Duration, what: &str, state: String) -> ! {
    match state.is_empty() {
        true => panic!("timed out ({guard:?}): {what}"),
        false => panic!("timed out ({guard:?}): {what}\nstate: {state}"),
    }
}

/// Whether `f` is still unfinished after every other task has run as far
/// as it can. Only for tests on the paused clock (`start_paused`): the
/// runtime moves that clock forward only once every task is waiting, so
/// the timeout fires only after `f` had every chance to finish. Unlike a
/// short real-time wait, this does not get weaker on a faster machine.
pub async fn still_pending<F: Future + Unpin>(f: &mut F) -> bool {
    tokio::time::timeout(Duration::from_secs(3600), f)
        .await
        .is_err()
}

/// Runs blocking `f` on its own thread, inside this runtime. Not
/// `spawn_blocking`: while a blocking task runs, the paused clock does not
/// move on its own, and [`still_pending`] needs it to.
pub fn on_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> oneshot::Receiver<T> {
    let runtime = tokio::runtime::Handle::current();
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _runtime = runtime.enter();
        let _ = tx.send(f());
    });
    rx
}
