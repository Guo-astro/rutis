//! The shuttle models' clock (#627): [`crate::restart::supervise`] waits
//! before a restart, and real time is not an interleaving. Built for the
//! model build only (`cargo test` with `--cfg rutis_shuttle`);
//! `crate::restart::sleep` asks this module for the wait there, and a model
//! installs its clock on the thread that supervises with
//! [`Clock::enter`], moving time itself with [`Clock::advance`].
//!
//! The clock is the model's, not shuttle's: a `tokio::time::sleep` would
//! wait real seconds per iteration and never let the model decide when a
//! wait ends, and a shuttle thread that slept would only wait for the
//! model's own advance. The pattern is the kernel's (`crates/rutis/src/
//! sync.rs`'s `Clock`, #622), kept here because the supervision is in
//! rutis-host.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::thread::ThreadId;
use std::time::Duration;

use shuttle::sync::{Condvar, Mutex};

/// The model's virtual clock: where time is, and the waits registered on
/// it. A wait ends when [`advance`](Self::advance) reaches it.
pub(crate) struct Clock {
    inner: Arc<Inner>,
}

impl Clock {
    /// A clock at time zero, with no waits.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                now: Mutex::new(Duration::ZERO),
                waits: Mutex::new(Vec::new()),
                requested: Mutex::new(Vec::new()),
                registered: Condvar::new(),
                next: Mutex::new(0),
            }),
        }
    }

    /// Registers this clock for the waits on this thread until the guard is
    /// dropped.
    pub(crate) fn enter(&self) -> Entered {
        Entered::install(self.inner.clone())
    }

    /// Moves time forward by `by` and fires every wait that is now due, in
    /// registration order; returns how many fired.
    pub(crate) fn advance(&self, by: Duration) -> usize {
        let now = {
            let mut now = self.inner.now.lock().unwrap();
            *now += by;
            *now
        };
        let due = {
            let mut waits = self.inner.waits.lock().unwrap();
            let mut due = Vec::new();
            let mut at = 0;
            while at < waits.len() {
                if waits[at].due <= now {
                    due.push(waits.remove(at));
                } else {
                    at += 1;
                }
            }
            due
        };
        let fired = due.len();
        // Outside the lock: a send wakes the wait it belongs to, which the
        // scheduler may run before this thread continues.
        for wait in due {
            let _ = wait.fire.send(());
        }
        fired
    }

    /// Blocks until `waits` waits are registered. The model calls it before
    /// moving time past a wait it must end; with no such wait it never
    /// returns, and shuttle reports the deadlock.
    pub(crate) fn wait_for_waiters(&self, waits: usize) {
        let mut pending = self.inner.waits.lock().unwrap();
        while pending.len() < waits {
            pending = self.inner.registered.wait(pending).unwrap();
        }
    }

    /// Every wait asked for so far, in order: what the model asserts the
    /// production loop requested.
    pub(crate) fn requested(&self) -> Vec<Duration> {
        self.inner.requested.lock().unwrap().clone()
    }
}

impl Clone for Clock {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct Inner {
    /// Virtual time since the clock was made.
    now: Mutex<Duration>,
    /// The waits registered and not yet fired or taken back.
    waits: Mutex<Vec<Wait>>,
    /// Every wait registered, kept after it fires, for
    /// [`Clock::requested`].
    requested: Mutex<Vec<Duration>>,
    /// Wakes `wait_for_waiters` when one is added.
    registered: Condvar,
    next: Mutex<u64>,
}

/// One registered wait. Its `fire` completes the wait's future; taken by
/// `advance`.
struct Wait {
    id: u64,
    due: Duration,
    fire: tokio::sync::oneshot::Sender<()>,
}

/// One registration, taken back when it is dropped: a wait that ends
/// before the clock reaches it (it cannot in the supervision models, but
/// a dropped future leaves one behind).
struct Registration {
    clock: Arc<Inner>,
    id: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.clock
            .waits
            .lock()
            .unwrap()
            .retain(|wait| wait.id != self.id);
    }
}

/// One wait's end. Dropping it (the future dropped first) takes the
/// registration back, so a later advance fires nothing for it.
pub(crate) struct WaitEnd {
    _registration: Registration,
    past: tokio::sync::oneshot::Receiver<()>,
}

impl WaitEnd {
    async fn passed(self) {
        let _ = self.past.await;
    }
}

/// Waits `limit` from now on the clock installed on this thread. The
/// supervision always installs one; without it the model would wait on no
/// clock, which no advance can end, so it is an error here rather than a
/// deadlock shuttle would report without saying why.
pub(crate) async fn sleep(limit: Duration) {
    let Some(clock) = current() else {
        panic!("a supervision model slept without installing its clock (Clock::enter)");
    };
    let id = {
        let mut next = clock.next.lock().unwrap();
        *next += 1;
        *next
    };
    let due = *clock.now.lock().unwrap() + limit;
    clock.requested.lock().unwrap().push(limit);
    let (late, past) = tokio::sync::oneshot::channel::<()>();
    clock.waits.lock().unwrap().push(Wait {
        id,
        due,
        fire: late,
    });
    clock.registered.notify_all();
    WaitEnd {
        _registration: Registration { clock, id },
        past,
    }
    .passed()
    .await;
}

/// The clock installed on this thread, innermost first. Keyed by the
/// operating-system thread rather than a `std::thread_local!`: shuttle runs
/// every task of one execution on the model's operating-system thread, and
/// the threads of one execution share thread locals.
fn current() -> Option<Arc<Inner>> {
    registry()
        .lock()
        .unwrap()
        .get(&std::thread::current().id())
        .and_then(|clocks| clocks.last().cloned())
}

fn registry() -> &'static StdMutex<HashMap<ThreadId, Vec<Arc<Inner>>>> {
    static REGISTRY: OnceLock<StdMutex<HashMap<ThreadId, Vec<Arc<Inner>>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// One clock installed on one thread; the clock underneath it comes back
/// on drop.
pub(crate) struct Entered {
    thread: ThreadId,
}

impl Entered {
    fn install(clock: Arc<Inner>) -> Self {
        let thread = std::thread::current().id();
        registry()
            .lock()
            .unwrap()
            .entry(thread)
            .or_default()
            .push(clock);
        Self { thread }
    }
}

impl Drop for Entered {
    fn drop(&mut self) {
        let mut all = registry().lock().unwrap();
        let empty = match all.get_mut(&self.thread) {
            Some(clocks) => {
                clocks.pop();
                clocks.is_empty()
            }
            None => false,
        };
        if empty {
            all.remove(&self.thread);
        }
    }
}
