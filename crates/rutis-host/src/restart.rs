//! A local runtime whose process ends on its own, as its `restart` says
//! (`docs/guide/rutis-host.md`, "When a runtime ends"): started again after
//! a backoff, up to a limit (`on-failure`), or the host told to stop
//! (`exit`). The numbers are fixed, not configurable. The Go runtimes are
//! supervised by their plugin instead ([`crate::host::Host::supervise`]),
//! with the same [`Backoff`].

use std::time::Duration;

use rutis::host::{ErrorKind, FiberView};
use rutis_bridge::runtime::{RuntimeHandle, RuntimeState};
use tokio::time::Instant;

use crate::config::Restart;
use crate::errln;

/// The backoff and its numbers: rutis-loader's, which the Go runtimes'
/// restarts use too.
pub use rutis_loader::backoff::{Backoff, FIRST, LIMIT, LONGEST, STABLE, WINDOW};

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

/// Watch the local runtime `name` (`handle`; `fiber`, its plugin) as
/// `restart` says, until there is nothing more to do. Each time its process
/// ends on its own: `on-failure` starts it again ([`FiberView::restart`])
/// after the [`Backoff`]'s wait, printing `runtime <name>: restarting`, and
/// gives up with `runtime <name>: gave up after 5 restarts within 5
/// minutes`; `exit` returns at once. Rows come back by themselves once it
/// runs again. Stopping the runtime (or the host) disposes it instead,
/// which is not an end: abort the task watching it first.
pub async fn supervise(
    name: &str,
    handle: &RuntimeHandle,
    fiber: &FiberView,
    restart: Restart,
) -> Supervised {
    let mut backoff = Backoff::default();
    let mut started = Instant::now();
    loop {
        let why = ended(handle).await;
        match restart {
            Restart::No => return Supervised::Closed,
            Restart::Exit => return Supervised::Exit(why),
            Restart::OnFailure => {}
        }
        let Some(wait) = backoff.next(Instant::now(), started.elapsed()) else {
            errln!(
                "runtime {name}: gave up after {LIMIT} restarts within {} minutes",
                WINDOW.as_secs() / 60
            );
            return Supervised::GaveUp;
        };
        tokio::time::sleep(wait).await;
        errln!("runtime {name}: restarting");
        started = Instant::now();
        // It runs again, or its start failed and it is down once more,
        // which counts as one more end.
        if let Err(error) = fiber.restart().await {
            if matches!(*error, ErrorKind::Closed | ErrorKind::InactiveEffect) {
                return Supervised::Closed;
            }
        }
    }
}

/// Wait until the runtime's process has ended on its own: why.
async fn ended(handle: &RuntimeHandle) -> String {
    loop {
        if let RuntimeState::Down(why) = handle.state() {
            return why;
        }
        // Looked at again now and then too: a change between the look and
        // the wait would otherwise be missed.
        tokio::select! {
            () = handle.changed() => {}
            () = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rutis::host::CtxHost;
    use rutis::host::{BoxFuture, DynPlugin, Effect};
    use rutis::{Ctx, Error};
    use rutis_bridge::runtime::RuntimePlugin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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
        let supervised = supervise("py", &handle, &fiber, Restart::OnFailure).await;
        assert_eq!(supervised, Supervised::GaveUp);
        assert_eq!(crashing.starts.load(Ordering::SeqCst), 1 + LIMIT);
        assert!(started.elapsed() >= Duration::from_secs(1 + 2 + 4 + 8 + 16));

        let crashing = Crashing::new();
        let handle = crashing.runtime.handle();
        let fiber = root.plugin(crashing.clone());
        let supervised = supervise("py", &handle, &fiber, Restart::Exit).await;
        assert_eq!(supervised, Supervised::Exit("crashed".into()));
        assert_eq!(crashing.starts.load(Ordering::SeqCst), 1);
        root.shutdown().await.unwrap();
    }

    /// A runtime that is down as soon as it starts.
    #[derive(Clone)]
    struct Crashing {
        runtime: RuntimePlugin,
        starts: Arc<AtomicUsize>,
    }

    impl Crashing {
        fn new() -> Self {
            Self {
                runtime: RuntimePlugin::session("py", "."),
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
