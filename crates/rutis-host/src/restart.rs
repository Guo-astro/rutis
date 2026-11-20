//! A local runtime whose process ends on its own, as its `restart` says
//! (`docs/guide/rutis-host.md`, "When a runtime ends"): started again after
//! a backoff, up to a limit (`on-failure`), or the host told to stop
//! (`exit`). The numbers are fixed, not configurable. One loop
//! ([`supervise`]) for every runtime: the Node, Python and Bun runtimes
//! ([`Local`]) and each Go or Swift binary's ([`Compiled`]).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, ErrorKind, FiberView};
use rutis_bridge::runtime::{RuntimeHandle, RuntimeState};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::config::Restart;
#[cfg(not(all(test, rutis_shuttle)))]
use crate::errln;

/// The wait before a restart ([`supervise`]): tokio's in a plain build, the
/// model's clock in the model build (`cargo test` with `--cfg
/// rutis_shuttle`: real time is not an interleaving, `crate::shuttle`).
#[cfg(not(all(test, rutis_shuttle)))]
async fn sleep(wait: Duration) {
    tokio::time::sleep(wait).await;
}

#[cfg(all(test, rutis_shuttle))]
async fn sleep(wait: Duration) {
    crate::shuttle::sleep(wait).await;
}

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

/// How watching a runtime ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Supervised {
    /// `on-failure`: it was restarted [`LIMIT`] times within [`WINDOW`],
    /// and ended again; it stays down.
    GaveUp,
    /// `exit`: it ended, and why; the host is to stop.
    Exit(String),
    /// The host is stopping, or stopped the runtime.
    Closed,
}

/// A runtime as [`supervise`] watches it.
pub trait Watched: Send + Sync {
    /// Wait until its process ended on its own: why. Stopping it is no end.
    fn ended(&self) -> BoxFuture<'_, String>;

    /// After the wait: whether it is still down from that end, not started
    /// or stopped by something else meanwhile.
    fn still_ended(&self) -> bool {
        true
    }

    /// Start it again: `false` when it is closed, and there is nothing more
    /// to do.
    fn start_again(&self) -> BoxFuture<'_, bool>;
}

/// Watch the runtime `name` as `restart` says, until there is nothing more
/// to do. Each time its process ends on its own: `on-failure` starts it
/// again after `backoff`'s wait, printing `runtime <name>: restarting`, and
/// gives up with `runtime <name>: gave up after 5 restarts within 5
/// minutes`; `exit` returns at once. Rows come back by themselves once it
/// runs again. Stopping the runtime (or the host) is not an end: abort the
/// task watching it first.
pub async fn supervise(
    name: &str,
    runtime: &dyn Watched,
    restart: Restart,
    backoff: &mut Backoff,
) -> Supervised {
    #[cfg(all(test, rutis_shuttle))]
    let _ = name;
    let mut started = Instant::now();
    loop {
        let why = runtime.ended().await;
        match restart {
            Restart::No => return Supervised::Closed,
            Restart::Exit => return Supervised::Exit(why),
            Restart::OnFailure => {}
        }
        let Some(wait) = backoff.next(Instant::now(), started.elapsed()) else {
            #[cfg(not(all(test, rutis_shuttle)))]
            errln!(
                "runtime {name}: gave up after {LIMIT} restarts within {} minutes",
                WINDOW.as_secs() / 60
            );
            return Supervised::GaveUp;
        };
        sleep(wait).await;
        if !runtime.still_ended() {
            continue;
        }
        #[cfg(not(all(test, rutis_shuttle)))]
        errln!("runtime {name}: restarting");
        started = Instant::now();
        if !runtime.start_again().await {
            return Supervised::Closed;
        }
    }
}

/// A Node, Python or Bun runtime: its state, and its plugin
/// (`rutis-host/spawn`), which a restart starts again.
pub struct Local {
    pub handle: RuntimeHandle,
    pub fiber: FiberView,
}

impl Watched for Local {
    fn ended(&self) -> BoxFuture<'_, String> {
        Box::pin(async move {
            match self
                .handle
                .wait_for(|state| matches!(state, RuntimeState::Down(_)))
                .await
            {
                RuntimeState::Down(why) => why,
                _ => unreachable!("only a down runtime is waited for"),
            }
        })
    }

    fn start_again(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            // It runs again, or its start failed and it is down once more,
            // which counts as one more end.
            match self.fiber.restart().await {
                Err(error) => !matches!(*error, ErrorKind::Closed | ErrorKind::InactiveEffect),
                Ok(()) => true,
            }
        })
    }
}

/// Each start of a compiled language's runtimes, by name: the latest
/// start's state and how many starts there were. The language's spawn
/// tells it ([`Starts::started`]); supervising hears of every runtime,
/// those that start later too ([`Starts::watch`]).
#[derive(Clone, Default)]
pub struct Starts(Arc<Mutex<StartsInner>>);

/// A runtime's latest start: its number, and its state.
type Start = (u64, RuntimeHandle);

type OnRuntime = Box<dyn Fn(&str, watch::Receiver<Start>) + Send + Sync>;

#[derive(Default)]
struct StartsInner {
    runtimes: HashMap<String, watch::Sender<Start>>,
    watching: Option<OnRuntime>,
}

impl Starts {
    /// The runtime `name` starts with the state `handle`.
    pub fn started(&self, name: &str, handle: RuntimeHandle) {
        let mut inner = self.0.lock().unwrap();
        if let Some(sender) = inner.runtimes.get(name) {
            sender.send_modify(|start| *start = (start.0 + 1, handle));
            return;
        }
        let (sender, receiver) = watch::channel((1, handle));
        inner.runtimes.insert(name.to_owned(), sender);
        if let Some(watching) = &inner.watching {
            watching(name, receiver);
        }
    }

    /// Tell `on` of each runtime started so far, and of each that starts
    /// for the first time from now on, until [`Starts::unwatch`].
    pub fn watch(&self, on: impl Fn(&str, watch::Receiver<Start>) + Send + Sync + 'static) {
        let mut inner = self.0.lock().unwrap();
        for (name, sender) in &inner.runtimes {
            on(name, sender.subscribe());
        }
        inner.watching = Some(Box::new(on));
    }

    /// Tell no one of runtimes that start from now on.
    pub fn unwatch(&self) {
        self.0.lock().unwrap().watching = None;
    }
}

/// A Go or Swift binary's runtime: each of its starts in turn, which a
/// restart replaces (`BinaryHost::restart_runtime`). An end is reported
/// once per start, so a restart that could not start it waits for its next
/// start. A runtime its session refused (an old SDK) did not end on its
/// own: it is not restarted. An idle stop is no end.
pub struct Compiled {
    starts: watch::Receiver<Start>,
    restart: Box<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    /// The start whose end was reported.
    reported: Mutex<Option<u64>>,
}

impl Compiled {
    /// The runtime whose starts `starts` follows, restarted by `restart`.
    pub fn new(
        starts: watch::Receiver<Start>,
        restart: impl Fn() -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            starts,
            restart: Box::new(restart),
            reported: Mutex::new(None),
        }
    }
}

impl Watched for Compiled {
    fn ended(&self) -> BoxFuture<'_, String> {
        Box::pin(async move {
            let mut starts = self.starts.clone();
            loop {
                let (start, handle) = starts.borrow_and_update().clone();
                if *self.reported.lock().unwrap() != Some(start) {
                    tokio::select! {
                        state = handle.wait_for(|state| matches!(state, RuntimeState::Down(_))) => {
                            *self.reported.lock().unwrap() = Some(start);
                            if let RuntimeState::Down(why) = state {
                                if !rutis_loader::runtime::binary::refused(&why) {
                                    return why;
                                }
                            }
                        }
                        changed = starts.changed() => {
                            if changed.is_err() {
                                std::future::pending::<()>().await;
                            }
                            continue;
                        }
                    }
                }
                if starts.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        })
    }

    fn still_ended(&self) -> bool {
        let (start, handle) = &*self.starts.borrow();
        *self.reported.lock().unwrap() == Some(*start)
            && matches!(handle.state(), RuntimeState::Down(_))
    }

    fn start_again(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            (self.restart)().await;
            true
        })
    }
}

/// The supervision's shuttle models (#627), built only with
/// `--cfg rutis_shuttle`.
#[cfg(all(test, rutis_shuttle))]
mod shuttle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rutis::host::CtxHost;
    use rutis::host::{BoxFuture, DynPlugin, Effect};
    use rutis::{Ctx, Error};
    use rutis_bridge::runtime::RuntimeStates;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// The numbers: 1 s, doubling up to 30 s; five restarts within five
    /// minutes, the sixth end gives up; a runtime that ran a minute starts
    /// over from 1 s.
    /// guarantee: host/runtime-restart
    #[test]
    fn the_backoff_is_fixed() {
        assert_eq!(
            (FIRST, LONGEST, STABLE, WINDOW, LIMIT),
            (
                Duration::from_secs(1),
                Duration::from_secs(30),
                Duration::from_secs(60),
                Duration::from_secs(300),
                5
            )
        );
        assert!(LONGEST.checked_mul(2).is_some(), "the step doubles safely");
        let start = Instant::now();
        let secs = Duration::from_secs;
        // Ending at once each time: 1, 2, 4, 8, 16 s, then giving up.
        let mut backoff = Backoff::default();
        let mut now = start;
        let mut waits = Vec::new();
        while let Some(wait) = backoff.next(now, Duration::ZERO) {
            waits.push(wait.as_secs());
            now += wait;
        }
        assert_eq!(waits, [1, 2, 4, 8, 16]);

        // Ending every 59 s, past the window: the wait doubles to 30 s and
        // stays there, and the window never holds five.
        let mut backoff = Backoff::default();
        let mut now = start;
        let mut waits = Vec::new();
        for _ in 0..8 {
            let wait = backoff.next(now, secs(59)).expect("restarted");
            waits.push(wait.as_secs());
            now += wait + secs(59);
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);

        // A minute of running starts over from 1 s.
        let mut backoff = Backoff::default();
        assert_eq!(backoff.next(start, Duration::ZERO), Some(secs(1)));
        assert_eq!(backoff.next(start + secs(2), Duration::ZERO), Some(secs(2)));
        assert_eq!(backoff.next(start + secs(70), secs(60)), Some(secs(1)));
    }

    proptest! {
        /// guarantee: host/runtime-restart
        ///
        /// For any sequence of endings and running times: the wait is 1 s,
        /// doubling to 30 s and staying there; a runtime that ran 60 s
        /// starts over from 1 s; and once five restarts are still within
        /// five minutes the next end gives up. The deadlines are kept in
        /// plain `Duration`s here, not in the implementation's deque, so
        /// the window arithmetic is checked against a second account.
        #[test]
        fn any_sequence_of_ends_follows_the_backoff(
            ends in prop::collection::vec((0u64..=500, 0u64..=120), 1..=12),
        ) {
            let start = Instant::now();
            let mut backoff = Backoff::default();
            let mut now = Duration::ZERO;
            let mut expected = FIRST;
            let mut restarts: Vec<Duration> = Vec::new();
            let mut waits: Vec<Duration> = Vec::new();
            for (extra, ran) in ends {
                let ran = Duration::from_secs(ran);
                if ran >= STABLE {
                    expected = FIRST;
                }
                restarts.retain(|deadline| now.saturating_sub(*deadline) < WINDOW);
                let got = backoff.next(start + now, ran);
                if restarts.len() >= LIMIT {
                    prop_assert_eq!(got, None);
                    break;
                }
                prop_assert_eq!(got, Some(expected));
                waits.push(expected);
                restarts.push(now + expected);
                expected = (expected * 2).min(LONGEST);
                now += waits.last().copied().unwrap() + Duration::from_secs(extra);
            }
            prop_assert!(waits.iter().all(|wait| (FIRST..=LONGEST).contains(wait)));
        }
    }

    /// A runtime that ends whenever it starts, as one whose plugin crashes
    /// it at once: `on-failure` restarts it five times, then gives up;
    /// `exit` says why at once, and restarts nothing.
    /// guarantee: host/runtime-restart
    #[tokio::test(start_paused = true)]
    async fn a_runtime_that_keeps_ending_is_given_up_on() {
        let root = Ctx::root().unwrap();
        let crashing = Crashing::new();
        let handle = crashing.runtime.handle();
        let fiber = root.plugin(crashing.clone());
        let started = Instant::now();
        let local = Local { handle, fiber };
        let supervised = supervise("py", &local, Restart::OnFailure, &mut Backoff::default()).await;
        assert_eq!(supervised, Supervised::GaveUp);
        assert_eq!(crashing.starts.load(Ordering::SeqCst), 1 + LIMIT);
        assert!(started.elapsed() >= Duration::from_secs(1 + 2 + 4 + 8 + 16));

        let crashing = Crashing::new();
        let handle = crashing.runtime.handle();
        let fiber = root.plugin(crashing.clone());
        let local = Local { handle, fiber };
        let supervised = supervise("py", &local, Restart::Exit, &mut Backoff::default()).await;
        assert_eq!(supervised, Supervised::Exit("crashed".into()));
        assert_eq!(crashing.starts.load(Ordering::SeqCst), 1);
        root.shutdown().await.unwrap();
    }

    /// A runtime that cannot start at all (its interpreter is not there) is
    /// started again on every wait of the backoff, then given up on: a
    /// start that fails is an end as any other, each round (#622).
    /// guarantee: host/runtime-restart
    #[tokio::test(start_paused = true)]
    async fn a_runtime_that_cannot_start_is_retried_each_round_then_given_up() {
        struct Missing {
            starts: AtomicUsize,
        }

        impl Watched for Missing {
            fn ended(&self) -> BoxFuture<'_, String> {
                Box::pin(async { "cannot start it: No such file or directory".to_owned() })
            }

            fn start_again(&self) -> BoxFuture<'_, bool> {
                self.starts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { true })
            }
        }

        let missing = Missing {
            starts: AtomicUsize::new(0),
        };
        let started = Instant::now();
        let supervised =
            supervise("py", &missing, Restart::OnFailure, &mut Backoff::default()).await;
        assert_eq!(supervised, Supervised::GaveUp);
        assert_eq!(missing.starts.load(Ordering::SeqCst), LIMIT);
        assert!(started.elapsed() >= Duration::from_secs(1 + 2 + 4 + 8 + 16));
    }

    /// A Go or Swift runtime: its starts, and the starts its restarts
    /// make, each down at once when `crashing`.
    struct Binary {
        starts: Starts,
        restarts: Arc<AtomicUsize>,
        /// Every start's state, kept so no handle closes.
        states: Arc<std::sync::Mutex<Vec<RuntimeStates>>>,
    }

    impl Binary {
        fn new() -> Self {
            Self {
                starts: Starts::default(),
                restarts: Arc::default(),
                states: Arc::default(),
            }
        }

        /// A start of `go-a`, as its spawn makes it.
        fn start(&self) -> RuntimeStates {
            start(&self.starts, &self.states)
        }

        /// The runtime as supervising sees it: each restart is a start,
        /// down at once when `crashing`.
        fn watched(&self, crashing: bool) -> Compiled {
            let receiver = Arc::new(std::sync::Mutex::new(None));
            let seen = receiver.clone();
            self.starts
                .watch(move |_, starts| *seen.lock().unwrap() = Some(starts));
            let starts = receiver.lock().unwrap().take().expect("go-a started");
            let (all, states, restarts) = (
                self.starts.clone(),
                self.states.clone(),
                self.restarts.clone(),
            );
            Compiled::new(starts, move || {
                restarts.fetch_add(1, Ordering::SeqCst);
                if crashing {
                    start(&all, &states).failed("crashed");
                }
                Box::pin(async {})
            })
        }

        fn restarts(&self) -> usize {
            self.restarts.load(Ordering::SeqCst)
        }
    }

    fn start(starts: &Starts, states: &std::sync::Mutex<Vec<RuntimeStates>>) -> RuntimeStates {
        let runtime = RuntimeStates::new("go-a", ".");
        starts.started("go-a", runtime.handle());
        runtime.starting();
        states.lock().unwrap().push(runtime.clone());
        runtime
    }

    /// Long enough for any wait of the backoff.
    const LONG: Duration = Duration::from_secs(600);

    /// A Go or Swift runtime that ends whenever it starts is restarted five
    /// times, then given up on, like a local one; one that ended before
    /// supervising began is handled then.
    /// guarantee: host/runtime-restart
    #[tokio::test(start_paused = true)]
    async fn a_binary_runtime_that_keeps_ending_is_given_up_on() {
        let binary = Binary::new();
        binary.start().failed("crashed");
        let watched = binary.watched(true);
        let mut backoff = Backoff::default();
        let supervised = supervise("go-a", &watched, Restart::OnFailure, &mut backoff).await;
        assert_eq!(supervised, Supervised::GaveUp);
        assert_eq!(binary.restarts(), LIMIT);

        let binary = Binary::new();
        binary.start().failed("crashed");
        let watched = binary.watched(true);
        let supervised = supervise("go-a", &watched, Restart::Exit, &mut Backoff::default()).await;
        assert_eq!(supervised, Supervised::Exit("crashed".into()));
        assert_eq!(binary.restarts(), 0);
    }

    /// Not ends: an idle stop, and a session refused (an old SDK), which
    /// starting again would only repeat. A restart that could not start it
    /// waits for its next start; one started by something else during the
    /// wait is not restarted.
    /// guarantee: host/runtime-restart
    #[tokio::test(start_paused = true)]
    async fn a_binary_runtime_restarts_only_after_its_own_end() {
        let quiet = |binary: &Binary, crashing| {
            let watched = binary.watched(crashing);
            async move {
                let mut backoff = Backoff::default();
                let supervised = supervise("go-a", &watched, Restart::OnFailure, &mut backoff);
                tokio::time::timeout(LONG, supervised).await.is_err()
            }
        };

        let binary = Binary::new();
        let first = binary.start();
        first.stopped();
        assert!(quiet(&binary, true).await);
        assert_eq!(binary.restarts(), 0);

        let binary = Binary::new();
        binary
            .start()
            .failed("incompatible session: plugin API 9 is newer than 2");
        assert!(quiet(&binary, true).await);
        assert_eq!(binary.restarts(), 0);

        // Restarted once; that start failed before a new one began.
        let binary = Binary::new();
        binary.start().failed("crashed");
        assert!(quiet(&binary, false).await);
        assert_eq!(binary.restarts(), 1);

        // Started again by hand (a new build) within the first second.
        let binary = Binary::new();
        binary.start().failed("crashed");
        let watched = binary.watched(true);
        let supervised = tokio::spawn(async move {
            let mut backoff = Backoff::default();
            let supervised = supervise("go-a", &watched, Restart::OnFailure, &mut backoff);
            tokio::time::timeout(LONG, supervised).await.is_err()
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        binary.start();
        assert!(supervised.await.unwrap());
        assert_eq!(binary.restarts(), 0);
    }

    /// What one supervision made of a crash sequence: how it ended, the ends
    /// it saw, and how often it restarted the runtime.
    #[derive(Debug, PartialEq, Eq)]
    struct Outcome {
        supervised: Supervised,
        ends: Vec<String>,
        restarts: usize,
    }

    /// A `Watched` that records each end it hands to the supervision, and
    /// otherwise runs the runtime it wraps.
    struct Logged<W> {
        inner: W,
        ends: Arc<Mutex<Vec<String>>>,
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

    /// A local runtime that ends with the next reason whenever it starts.
    #[derive(Clone)]
    struct Failing {
        runtime: RuntimeStates,
        reasons: Arc<Mutex<VecDeque<String>>>,
        starts: Arc<AtomicUsize>,
    }

    impl Failing {
        fn new(reasons: &[&str]) -> Self {
            Self {
                runtime: RuntimeStates::new("py", "."),
                reasons: Arc::new(Mutex::new(
                    reasons.iter().map(|why| (*why).to_owned()).collect(),
                )),
                starts: Arc::default(),
            }
        }
    }

    impl DynPlugin for Failing {
        fn name(&self) -> &str {
            "failing-runtime"
        }

        fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.runtime.starting();
            let why = self
                .reasons
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "crashed".to_owned());
            self.runtime.failed(why);
            Box::pin(async { Ok(Effect::Done) })
        }
    }

    /// One crash sequence through [`Local`], the Node, Python and Bun
    /// runtimes' `Watched`.
    async fn local_outcome(root: &Ctx, reasons: &[&str], restart: Restart) -> Outcome {
        let failing = Failing::new(reasons);
        let starts = failing.starts.clone();
        let handle = failing.runtime.handle();
        let fiber = root.plugin(failing);
        let ends = Arc::new(Mutex::new(Vec::new()));
        let watched = Logged {
            inner: Local { handle, fiber },
            ends: ends.clone(),
        };
        let supervised = supervise("py", &watched, restart, &mut Backoff::default()).await;
        let ends = ends.lock().unwrap().clone();
        Outcome {
            supervised,
            ends,
            restarts: starts.load(Ordering::SeqCst).saturating_sub(1),
        }
    }

    /// One start of the compiled runtimes' `go-a`, ending at once with the
    /// next reason.
    fn start_binary(
        starts: &Starts,
        states: &Arc<Mutex<Vec<RuntimeStates>>>,
        reasons: &Arc<Mutex<VecDeque<String>>>,
    ) {
        let runtime = RuntimeStates::new("go-a", ".");
        runtime.starting();
        starts.started("go-a", runtime.handle());
        let why = reasons
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "crashed".to_owned());
        runtime.failed(why);
        states.lock().unwrap().push(runtime);
    }

    /// One crash sequence through [`Compiled`], each Go and Swift runtime's
    /// `Watched`.
    async fn compiled_outcome(reasons: &[&str], restart: Restart) -> Outcome {
        let starts = Starts::default();
        let states: Arc<Mutex<Vec<RuntimeStates>>> = Arc::default();
        let restarts = Arc::new(AtomicUsize::new(0));
        let reasons: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(
            reasons.iter().map(|why| (*why).to_owned()).collect(),
        ));
        start_binary(&starts, &states, &reasons);

        let seen: Arc<Mutex<Option<tokio::sync::watch::Receiver<Start>>>> = Arc::default();
        let capture = seen.clone();
        starts.watch(move |_, receiver| *capture.lock().unwrap() = Some(receiver));
        let receiver = seen.lock().unwrap().take().expect("go-a started");
        let compiled = Compiled::new(receiver, {
            let starts = starts.clone();
            let states = states.clone();
            let reasons = reasons.clone();
            let restarts = restarts.clone();
            move || {
                restarts.fetch_add(1, Ordering::SeqCst);
                start_binary(&starts, &states, &reasons);
                Box::pin(async {})
            }
        });
        let ends = Arc::new(Mutex::new(Vec::new()));
        let watched = Logged {
            inner: compiled,
            ends: ends.clone(),
        };
        let supervised = supervise("go-a", &watched, restart, &mut Backoff::default()).await;
        let ends = ends.lock().unwrap().clone();
        Outcome {
            supervised,
            ends,
            restarts: restarts.load(Ordering::SeqCst),
        }
    }

    /// The same crash sequence through both `Watched` implementations the
    /// host's supervision drives — [`Local`] (Node, Python, Bun) and
    /// [`Compiled`] (each Go and Swift runtime) — restarts, gives up and
    /// reports the same ends, whatever the process ended with (an exit
    /// status or a signal) and however fast the exits follow each other
    /// (#627).
    /// guarantee: host/runtime-restart
    #[tokio::test(start_paused = true)]
    async fn the_two_supervisions_handle_the_same_crash_sequence_alike() {
        const STATUS: &str = "exited with exit status: 17";
        const SIGNAL: &str = "exited with signal: 9 (SIGKILL)";
        const OTHER: &str = "exited with exit status: 2";
        let root = Ctx::root().unwrap();
        let sequences: &[&[&str]] = &[
            &[STATUS, STATUS, STATUS, STATUS, STATUS, STATUS],
            &[SIGNAL, SIGNAL, SIGNAL, SIGNAL, SIGNAL, SIGNAL],
            // Rapid exits, an exit status and a signal in turn.
            &[STATUS, SIGNAL, OTHER, STATUS, SIGNAL, OTHER],
        ];
        for reasons in sequences {
            for restart in [Restart::OnFailure, Restart::Exit, Restart::No] {
                let local = local_outcome(&root, reasons, restart).await;
                let compiled = compiled_outcome(reasons, restart).await;
                assert_eq!(
                    local, compiled,
                    "the two supervisions disagree on {reasons:?} with {restart:?}"
                );
            }
        }
        root.shutdown().await.unwrap();
    }

    /// A runtime that is down as soon as it starts.
    #[derive(Clone)]
    struct Crashing {
        runtime: RuntimeStates,
        starts: Arc<AtomicUsize>,
    }

    impl Crashing {
        fn new() -> Self {
            Self {
                runtime: RuntimeStates::new("py", "."),
                starts: Arc::default(),
            }
        }
    }

    impl DynPlugin for Crashing {
        fn name(&self) -> &str {
            "crashing-runtime"
        }

        fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.runtime.starting();
            self.runtime.failed("crashed");
            Box::pin(async { Ok(Effect::Done) })
        }
    }
}
