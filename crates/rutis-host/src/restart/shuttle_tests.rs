//! Shuttle models of the restart supervision (#627, #622's supervision
//! item): how a runtime that ends on its own is started again, and when it
//! is given up on.
//!
//! `rutis-host`'s `supervise` decides it for every local runtime: the Node,
//! Python and Bun runtimes through [`Local`], each Go and Swift runtime
//! through [`Compiled`]. The models run that loop and [`Backoff`]
//! themselves; what stands in for the rest is named at each model:
//!
//! - the process ending is the model's `Watched` stand-in, or a
//!   [`RuntimeStates`] the model puts down by hand;
//! - the wait is the model's clock ([`crate::shuttle::Clock`]): real time
//!   is not an interleaving, and a real wait of 1 s to 16 s per iteration
//!   would not let the model choose the order;
//! - the runtime's own process starting is `Starts::started`, as the
//!   host's spawn closure calls it.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-host --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! `RUTIS_SHUTTLE_ITERATIONS` sets the iterations of each scheduler: 1000
//! by default, as on pull requests; the nightly run (`stress.yml`) uses a
//! million. A failure prints its schedule; `RUTIS_SHUTTLE_REPLAY=<schedule>`
//! replays it, with the failing test's name as the filter.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use rutis::host::BoxFuture;
use rutis_bridge::runtime::RuntimeStates;
use shuttle::future::block_on;
use shuttle::sync::atomic::{AtomicUsize, Ordering};
use shuttle::thread;
use tokio::sync::watch;

use super::{supervise, Backoff, Compiled, Starts, Supervised, Watched, LIMIT};
use crate::config::Restart;
use crate::shuttle::Clock;

/// Iterations of each scheduler: `RUTIS_SHUTTLE_ITERATIONS`, or 1000.
fn iterations() -> usize {
    std::env::var("RUTIS_SHUTTLE_ITERATIONS")
        .ok()
        .map(|value| value.parse().expect("RUTIS_SHUTTLE_ITERATIONS is a number"))
        .unwrap_or(1000)
}

/// Explore `model` with shuttle's random scheduler and with PCT (which
/// finds bugs that need a few preemptions at the right places with better
/// odds than uniform choices), or replay `RUTIS_SHUTTLE_REPLAY`.
fn explore<F>(model: F)
where
    F: Fn() + Send + Sync + 'static,
{
    if let Ok(schedule) = std::env::var("RUTIS_SHUTTLE_REPLAY") {
        shuttle::replay(model, schedule.trim().trim_matches('"'));
        return;
    }
    let model = Arc::new(model);
    let iterations = iterations();
    let random = model.clone();
    shuttle::check_random(move || random(), iterations);
    shuttle::check_pct(move || model(), iterations, 3);
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// A runtime that ends whenever it starts: each end hands out the next
/// reason, and `start_again` counts the restarts — asserting the end it
/// restarts after was reported first.
struct Crashing {
    reasons: StdMutex<VecDeque<String>>,
    ends: AtomicUsize,
    starts: AtomicUsize,
    /// Set when a restart begins, with the ends and starts then: the model
    /// checks no restart ran ahead of the end it follows.
    order: StdMutex<Vec<(usize, usize)>>,
}

impl Crashing {
    fn new(reasons: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            reasons: StdMutex::new(reasons.into_iter().map(Into::into).collect()),
            ends: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            order: StdMutex::new(Vec::new()),
        }
    }

    fn ends(&self) -> usize {
        self.ends.load(Ordering::SeqCst)
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
}

impl Watched for Crashing {
    fn ended(&self) -> BoxFuture<'_, String> {
        self.ends.fetch_add(1, Ordering::SeqCst);
        let why = self
            .reasons
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "crashed".to_owned());
        Box::pin(async move { why })
    }

    fn start_again(&self) -> BoxFuture<'_, bool> {
        let (ends, starts) = (self.ends(), self.starts());
        assert!(
            ends > starts,
            "a restart began before the end of the process it restarts ({ends} ends, {starts} starts)"
        );
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.order.lock().unwrap().push((ends, starts + 1));
        Box::pin(async { true })
    }
}

/// The ends a runtime that keeps ending is given up on: 1 s, doubling to
/// 16 s, each restart only once its end was reported; after five restarts
/// the sixth end gives up, and nothing starts again.
///
/// Mutations this model catches: a `supervise` that starts before it waits
/// for the end (the assertion in `start_again`), a backoff that does not
/// double (the requested waits), one that gives up a round early or late
/// (the end and start counts), a give-up that still restarts (the zero
/// waiters after it), and a wait that ends in the wrong order (the clock
/// fires one waiter at a time).
#[test]
fn shuttle_a_runtime_that_keeps_ending_is_given_up_on_at_the_cap() {
    explore(|| {
        let clock = Clock::new();
        let crashing = Arc::new(Crashing::new([
            "exited with exit status: 17",
            "exited with signal: 9 (SIGKILL)",
            "exited with exit status: 1",
            "exited with exit status: 2",
            "exited with signal: 15 (SIGTERM)",
            "exited with exit status: 3",
        ]));
        let watched = crashing.clone();
        let supervised = thread::spawn({
            let clock = clock.clone();
            move || {
                let _entered = clock.enter();
                block_on(supervise(
                    "py",
                    &*watched,
                    Restart::OnFailure,
                    &mut Backoff::default(),
                ))
            }
        });

        // The wait before each restart: the model moves time past it, and
        // the supervision goes on to start the runtime again.
        for wait in [1, 2, 4, 8, 16] {
            clock.wait_for_waiters(1);
            assert_eq!(
                clock.advance(secs(wait)),
                1,
                "the wait of {wait} s did not fire"
            );
        }

        let supervised = supervised.join().unwrap();
        assert_eq!(supervised, Supervised::GaveUp);
        assert_eq!(crashing.ends(), 1 + LIMIT, "not every end was reported");
        assert_eq!(
            crashing.starts(),
            LIMIT,
            "the restarts do not match the cap"
        );
        assert_eq!(
            crashing.order.lock().unwrap().as_slice(),
            &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)],
            "a restart ran before the end it follows"
        );
        assert_eq!(
            clock.requested(),
            vec![secs(1), secs(2), secs(4), secs(8), secs(16)],
            "the waits do not double from 1 s to 16 s"
        );
        assert_eq!(
            clock.advance(secs(60)),
            0,
            "a wait was still registered after giving up"
        );
    });
}

/// A `Watched` that records the ends it hands to the supervision, and
/// otherwise runs the runtime it wraps.
struct Logged<W: Watched> {
    inner: W,
    ends: Arc<StdMutex<Vec<String>>>,
}

impl<W: Watched> Watched for Logged<W> {
    fn ended(&self) -> BoxFuture<'_, String> {
        let ends = self.ends.clone();
        Box::pin(async move {
            let why = self.inner.ended().await;
            ends.lock().unwrap().push(why.clone());
            why
        })
    }

    fn still_ended(&self) -> bool {
        self.inner.still_ended()
    }

    fn start_again(&self) -> BoxFuture<'_, bool> {
        self.inner.start_again()
    }
}

/// The starts of runtime `go-a`, one at a time, and the end each one
/// reports: the second generation is started by hand before the host
/// supervises, and the first generation's unreported end is dropped rather
/// than restarted.
///
/// The first generation ended before supervising started; a start by hand
/// (a new build) made a second. [`Compiled::ended`] reports the current
/// start only, so generation 1's end is not reported, and generation 2 is
/// not restarted for it. Generation 2 then ends, and each restart starts a
/// generation that ends at once, up to the cap.
///
/// Mutations this model catches: an `ended` that reads the first start it
/// saw rather than the current one (generation 1's reason appears in the
/// ends), one that reports an end twice (another end, so the cap comes
/// early or the generations do not match), and a restart closure that
/// starts nothing (the supervision waits for a start no one makes, and
/// shuttle reports the deadlock).
#[test]
fn shuttle_an_end_the_next_start_took_over_is_not_restarted() {
    explore(|| {
        let starts = Starts::default();
        let states: Arc<StdMutex<Vec<RuntimeStates>>> = Arc::default();

        // Generation 1: ended before the host supervised.
        let first = RuntimeStates::new("go-a", ".");
        first.starting();
        starts.started("go-a", first.handle());
        first.failed("exited with exit status: 17");
        states.lock().unwrap().push(first);

        // Generation 2: started by hand (a new build), not ended yet.
        let second = RuntimeStates::new("go-a", ".");
        second.starting();
        starts.started("go-a", second.handle());
        states.lock().unwrap().push(second.clone());

        let seen: Arc<StdMutex<Option<watch::Receiver<super::Start>>>> = Arc::default();
        let capture = seen.clone();
        starts.watch(move |_, receiver| *capture.lock().unwrap() = Some(receiver));
        let receiver = seen.lock().unwrap().take().expect("go-a started");

        let restarts = Arc::new(AtomicUsize::new(0));
        let compiled = Compiled::new(receiver, {
            let starts = starts.clone();
            let states = states.clone();
            let restarts = restarts.clone();
            move || {
                restarts.fetch_add(1, Ordering::SeqCst);
                let next = RuntimeStates::new("go-a", ".");
                next.starting();
                starts.started("go-a", next.handle());
                next.failed("exited with exit status: 1");
                states.lock().unwrap().push(next);
                Box::pin(async {})
            }
        });
        let ends = Arc::new(StdMutex::new(Vec::new()));
        let watched = Logged {
            inner: compiled,
            ends: ends.clone(),
        };

        let clock = Clock::new();
        let supervised = thread::spawn({
            let clock = clock.clone();
            move || {
                let _entered = clock.enter();
                block_on(supervise(
                    "go-a",
                    &watched,
                    Restart::OnFailure,
                    &mut Backoff::default(),
                ))
            }
        });

        // Generation 2 ends, though generation 1's end was never reported.
        second.failed("exited with signal: 9 (SIGKILL)");

        for (round, wait) in [1, 2, 4, 8, 16].into_iter().enumerate() {
            clock.wait_for_waiters(1);
            assert_eq!(
                clock.advance(secs(wait)),
                1,
                "the wait of {wait} s did not fire"
            );
            // The restart started a generation that ended at once.
            let started = round + 3;
            let has_started = || states.lock().unwrap().len() >= started;
            while !has_started() {
                thread::yield_now();
            }
        }

        let supervised = supervised.join().unwrap();
        assert_eq!(supervised, Supervised::GaveUp);
        assert_eq!(restarts.load(Ordering::SeqCst), LIMIT);
        assert_eq!(
            ends.lock().unwrap().as_slice(),
            &[
                "exited with signal: 9 (SIGKILL)",
                "exited with exit status: 1",
                "exited with exit status: 1",
                "exited with exit status: 1",
                "exited with exit status: 1",
                "exited with exit status: 1",
            ],
            "an end was reported twice, or the taken-over end was reported"
        );
        assert_eq!(
            states.lock().unwrap().len(),
            2 + LIMIT,
            "a restart started more or fewer generations than the cap"
        );
    });
}
