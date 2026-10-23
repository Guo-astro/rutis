//! Wait helpers for the rutis tests (quality standard Q7.1, #231).
//!
//! A timeout here only guards against a hang ([`HANG_GUARD`]); it never
//! stands in for an event. Wait for what the test is about: a channel, a
//! watch, a line, a state. Poll ([`eventually`]) only where nothing can be
//! awaited, and then say on timeout what the state was
//! ([`eventually_with`]). Checking that something has *not* happened is
//! [`still_pending`] on the paused clock, in-process; across threads and
//! processes, see the rules in docs/design-test-clocks-2026-10-11.md §3.
//!
//! Every guard is scaled by [`time_scale`] (`RUTIS_TEST_TIME_SCALE`): a
//! loaded machine (CI runners sharing a host) waits longer, and a real hang
//! still fails, only later (docs/ci.md §11).

use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

/// How long a wait lasts before the test fails as hung, on an unloaded
/// machine: the helpers here scale it by [`time_scale`], and a wait of a
/// test's own takes [`scaled`]`(HANG_GUARD)`. It only guards against hangs,
/// and is never what a test measures. A wait that needs longer (several
/// runtimes starting cold) uses a multiple, with the reason next to it.
pub const HANG_GUARD: Duration = Duration::from_secs(10);

/// The environment variable [`time_scale`] reads.
pub const TIME_SCALE_VAR: &str = "RUTIS_TEST_TIME_SCALE";

/// How much longer than nominal every hang guard lasts on this machine:
/// `RUTIS_TEST_TIME_SCALE`, a number of at least 1 (CI sets it on the
/// self-hosted Linux runners, whose containers share a loaded host); 1 when
/// unset. Only guards scale: a test never measures with them, so a longer
/// guard changes nothing but how late a real hang fails. A value that is
/// not a number of at least 1 fails the test.
pub fn time_scale() -> f64 {
    static SCALE: OnceLock<f64> = OnceLock::new();
    *SCALE.get_or_init(|| match std::env::var(TIME_SCALE_VAR) {
        Err(_) => 1.0,
        Ok(value) if value.trim().is_empty() => 1.0,
        Ok(value) => match value.trim().parse::<f64>() {
            Ok(scale) if scale.is_finite() && scale >= 1.0 => scale,
            _ => panic!("{TIME_SCALE_VAR}={value:?}: expected a number of at least 1"),
        },
    })
}

/// `nominal`, a hang guard, scaled by [`time_scale`].
pub fn scaled(nominal: Duration) -> Duration {
    nominal.mul_f64(time_scale())
}

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
/// It only guards against hangs too. The field is nominal; a wait lasts
/// [`Guard::limit`].
#[derive(Clone, Copy, Debug)]
pub struct Guard(pub Duration);

impl Guard {
    /// How long a wait of this guard lasts here: the nominal guard,
    /// [`scaled`].
    pub fn limit(self) -> Duration {
        scaled(self.0)
    }

    /// [`within`] this guard.
    pub async fn within<F: Future>(self, what: &str, f: F) -> F::Output {
        let limit = self.limit();
        match tokio::time::timeout(limit, f).await {
            Ok(output) => output,
            Err(_) => panic!("timed out ({limit:?}): {what}"),
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
        let limit = self.limit();
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if let Some(value) = check() {
                return value;
            }
            if tokio::time::Instant::now() >= deadline {
                fail(limit, what, describe());
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

/// [`eventually`] on a plain thread, outside any runtime.
pub fn eventually_blocking<T>(mut check: impl FnMut() -> Option<T>, what: &str) -> T {
    let limit = Guard(HANG_GUARD).limit();
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = check() {
            return value;
        }
        if Instant::now() >= deadline {
            fail(limit, what, String::new());
        }
        std::thread::sleep(POLL);
    }
}

/// The next message of `rx`, within [`HANG_GUARD`].
pub fn recv_within<T>(what: &str, rx: &std::sync::mpsc::Receiver<T>) -> T {
    let limit = Guard(HANG_GUARD).limit();
    match rx.recv_timeout(limit) {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error} (hang guard {limit:?})"),
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
