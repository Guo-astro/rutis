//! The sync primitives and the spawn stand-in the shuttle models swap (#179,
//! #622 item 1).
//!
//! Built with `--cfg rutis_shuttle`, the spots whose interleavings the models
//! explore use shuttle's primitives, so every access to them is a point where
//! shuttle may switch threads: a fiber driver's `alive` flag (`post_join`
//! against the driver's exit), an effect record's state (claiming its
//! cleanups), the admission, transition and effects locks, a fiber's flight
//! count and listener state, and a spawned task's result. Everything else
//! keeps std's. A plain build is unchanged: these are std's types and
//! [`spawn`] is the host runtime's.
//!
//! # The locks
//!
//! [`Mutex`] and [`RwLock`] follow the same seam: shuttle's behind the cfg,
//! std's otherwise. Only the locks whose critical sections the models
//! interleave are built with them:
//!
//! - `Shared.admission` (`ctx.rs`): mount admission. `spawn_fiber_inner` and
//!   `Ctx::register_mount_effect` check `is_closed()` and commit a child under
//!   it, while a root close sets `Shared.closing` and fixes the subtree under
//!   the same lock. Both sides touch [`AtomicBool`]s inside the section, so a
//!   std mutex would park the second shuttle thread for real while the first
//!   is stopped at a switch point (the deadlock model 7's note describes): a
//!   shuttle mutex parks it on the scheduler instead, which then runs the
//!   holder to the end of the section.
//! - a fiber's `transition` and `effects` (`fiber.rs`): the registration/close
//!   path nests them (admission → transition → effects), and both sides touch
//!   the flight count.
//! - the event bus's `transcoder` (`bus.rs`), the kernel's only `RwLock`; an
//!   emit reads it from a task a model drives, so it takes the same seam.
//!
//! The kernel's other `Mutex`es (`children`, a fiber's slots, the bus state)
//! keep std's: no two interleaved paths take them across a shuttle switch
//! point, and a std lock must be entered and left without yielding in
//! between.
//!
//! # The spawn stand-in
//!
//! [`spawn`] and [`TaskSet`] are the seam every task the kernel creates goes
//! through: a fiber's driver, the runner of `EffectRecord::drain` (and the
//! task of an async cleanup that runner runs), the emit chain and its
//! listener calls, and `run_parallel`'s set. In a plain build they are
//! `Handle::spawn` and `tokio::task::JoinSet`, exactly as before.
//!
//! Under `rutis_shuttle` a model installs a `SpawnScope`. While it is
//! installed, every spawn on that thread goes onto shuttle's async executor —
//! the runnable queue shuttle's scheduler drives — instead of the host
//! runtime, and `SpawnScope::run` is the test scheduler: it yields to
//! shuttle until every captured task has finished, including the ones whose
//! handle the kernel dropped (`ctx.effect`'s refused record, an emit's
//! listener call). Shuttle runs every task of one execution on the model's
//! operating-system thread, so the scope reaches the tasks a task spawns
//! without being passed along. With no scope installed (the other models
//! build a `Ctx` on a real handle to run registration and watchers) [`spawn`]
//! still calls the host runtime, so those models are unchanged.
//!
//! A model that installs a scope drives everything spawned under it. A fiber's
//! driver waits on its mailbox and only ends when the fiber closes, so a model
//! that mounts a fiber must write the driver out (as models 2 and 8 do)
//! instead of leaving it in the scope. The scope starts no runtime, picks no
//! order (shuttle's scheduler does), and turns no panic into a join error: a
//! task that panics fails the iteration, as a `thread::spawn` in the other
//! models does. One visible difference from the host runtime: shuttle's
//! executor may run another task inside `spawn` (the scheduler switches before
//! it adds the task), where `Handle::spawn` does not. A `std::thread_local!`
//! window that spans a spawn — the bus's `SPAWNING_EMIT` — therefore spans a
//! switch in a model, and a task that finishes there drops its guards inside
//! the window: `TailCleanup` sets `DROPPED_INLINE`, and the emit that is
//! spawning takes the flag for its own task (it read it as the runtime
//! shutting down) and skips recording its tail, so a later emit starts without
//! waiting (#1332). The bus's `state` is also a std lock whose critical section
//! reads entry state through this module's atomics — a switch point — so a task
//! that finishes there blocks in its guard's `Drop` on the lock the model thread
//! holds, which is a deadlock. Both are the stand-in's; the host runtime never
//! runs a task inside `Handle::spawn`, and a task on another thread waits for
//! the lock. The emit chain's queued shape therefore has no model; the list in
//! `fiber/shuttle_tests.rs` says what is modelled instead.
//!
//! # The deadline stand-in
//!
//! [`deadline`] is the timer an unload's `wait_events` bounds its wait with
//! (`shutdown_deadline()`). In a plain build it is the thread that has kept
//! that time (`rutis-unload-deadline`): `recv_timeout` on a channel the wait
//! closes when its calls finish, then a send on a oneshot when it times out.
//! Under `rutis_shuttle` no thread runs and no real time passes: the timer
//! registers its deadline on the model's `Clock`, and `Clock::advance` fires
//! it. The model is the scheduler: `Clock::wait_for_waiters` blocks until the
//! timers it is about to move are in place, `Clock::advance` moves virtual
//! time, and `Clock::now` says where it is. A wait that ends before its
//! deadline (the calls finished) drops its `Deadline`, which takes the
//! registration back, as dropping the channel end ends the plain thread. A
//! model that installs no clock gets no timer at all — `wait_events` waits for
//! the calls without a deadline, the path a plain build takes when the thread
//! cannot be spawned — which is what the models that only wait for calls that
//! finish (6, 15 and 16) need.
//!
//! Shuttle models no time, so the clock is the model's, not shuttle's: a
//! `std::thread` with `recv_timeout` would wait ten real seconds per iteration
//! and never let the model decide when the deadline arrives; a shuttle thread
//! that slept would only wait for the model's own `advance`.

#[cfg(not(rutis_shuttle))]
pub(crate) use host::{deadline, spawn, Task, TaskSet};

#[cfg(rutis_shuttle)]
pub(crate) use spawned::{spawn, Task, TaskSet};

#[cfg(rutis_shuttle)]
pub(crate) use timed::deadline;

#[cfg(all(test, rutis_shuttle))]
pub(crate) use spawned::SpawnScope;

#[cfg(all(test, rutis_shuttle))]
pub(crate) use timed::Clock;

#[cfg(not(rutis_shuttle))]
pub(crate) use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};

#[cfg(not(rutis_shuttle))]
pub(crate) use std::sync::{Mutex, RwLock};

#[cfg(rutis_shuttle)]
pub(crate) use shuttle::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};

#[cfg(rutis_shuttle)]
pub(crate) use shuttle::sync::{Mutex, RwLock};

/// A point where shuttle may switch threads, in a window a model must be
/// able to stop in that no shuttle primitive opens (shuttle switches just
/// before an operation on its primitives, not after). Nothing in a plain
/// build.
#[inline(always)]
pub(crate) fn switch_point() {
    #[cfg(rutis_shuttle)]
    shuttle::thread::yield_now();
}

#[cfg(not(rutis_shuttle))]
mod host {
    use std::future::Future;
    use std::time::Duration;

    use crate::error::{join_panic_error, Error};

    /// The timer `wait_events` bounds an unload with: a thread that waits on
    /// `finished` until the deadline passes, then sends on the oneshot. While
    /// this exists the thread keeps waiting; dropping it (the calls finished
    /// first) closes the channel, so `recv_timeout` returns `Disconnected` and
    /// the thread ends without firing.
    pub(crate) struct Deadline {
        /// The caller's end of the deadline thread's channel.
        _done: std::sync::mpsc::Sender<()>,
        /// Completes when the deadline passed.
        past: tokio::sync::oneshot::Receiver<()>,
    }

    impl Deadline {
        /// Whether the deadline passed before the wait was dropped. `Err`
        /// never happens here: the thread sends on the timeout and is ended by
        /// this value's drop otherwise. It disables the branch in
        /// `wait_events`'s `select!` anyway, as the plain build always did.
        pub(crate) async fn passed(self) -> Result<(), ()> {
            self.past.await.map_err(|_| ())
        }
    }

    /// Starts the deadline thread: `None` when it cannot be spawned (the
    /// unload then waits for its calls without a deadline).
    pub(crate) fn deadline(limit: Duration) -> Option<Deadline> {
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let (late, past) = tokio::sync::oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("rutis-unload-deadline".into())
            .spawn(move || {
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    finished.recv_timeout(limit)
                {
                    let _ = late.send(());
                }
            })
            .ok()?;
        Some(Deadline { _done: done, past })
    }

    /// A spawned task: the host runtime's handle.
    pub(crate) struct Task<T>(tokio::task::JoinHandle<T>);

    impl<T> Task<T> {
        /// Waits for the task; a panic or a cancellation is the error
        /// `join_panic_error` describes.
        pub(crate) async fn join(self) -> Result<T, Error> {
            self.0.await.map_err(join_panic_error)
        }
    }

    /// The tasks of one parallel dispatch (`run_parallel`).
    pub(crate) struct TaskSet<T> {
        inner: tokio::task::JoinSet<T>,
    }

    impl<T: Send + 'static> TaskSet<T> {
        pub(crate) fn new() -> Self {
            Self {
                inner: tokio::task::JoinSet::new(),
            }
        }

        pub(crate) fn spawn_on<F>(&mut self, future: F, handle: &tokio::runtime::Handle)
        where
            F: Future<Output = T> + Send + 'static,
        {
            self.inner.spawn_on(future, handle);
        }

        pub(crate) async fn join_next(&mut self) -> Option<Result<T, Error>> {
            self.inner
                .join_next()
                .await
                .map(|joined| joined.map_err(join_panic_error))
        }
    }

    /// Spawns on the host runtime, as the kernel did before the seam.
    pub(crate) fn spawn<F>(handle: &tokio::runtime::Handle, future: F) -> Task<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Task(handle.spawn(future))
    }
}

#[cfg(rutis_shuttle)]
mod spawned {
    use std::collections::{HashMap, VecDeque};
    use std::future::Future;
    use std::sync::{Arc, Mutex as StdMutex, OnceLock};
    use std::thread::ThreadId;

    use shuttle::sync::Mutex;
    use tokio::sync::Notify;

    use crate::error::{join_panic_error, Error};

    /// The tasks a model spawns, on shuttle's executor instead of the host
    /// runtime. Install it with [`enter`](Self::enter) around the spawns and
    /// drive them with [`run`](Self::run).
    #[cfg(test)]
    pub(crate) struct SpawnScope {
        inner: Arc<Scope>,
    }

    #[cfg(test)]
    impl SpawnScope {
        pub(crate) fn new() -> Self {
            Self {
                inner: Arc::new(Scope {
                    pending: Mutex::new(0),
                    wake: Notify::new(),
                }),
            }
        }

        /// Every spawn on this thread, and on every task it spawns, goes to
        /// this scope until the guard is dropped.
        pub(crate) fn enter(&self) -> Entered {
            Entered::install(self.inner.clone())
        }

        /// The test scheduler: yields to shuttle until every task spawned in
        /// this scope has finished, joined by the kernel or not. The scope is
        /// installed while it waits, so a task that spawns another stays in
        /// it.
        pub(crate) fn run(&self) {
            let _entered = self.enter();
            loop {
                let notified = self.inner.wake.notified();
                if *self.inner.pending.lock().unwrap() == 0 {
                    return;
                }
                shuttle::future::block_on(notified);
            }
        }
    }

    struct Scope {
        /// Tasks spawned in this scope that have not finished.
        pending: Mutex<usize>,
        /// Wakes the scheduler when one finishes.
        wake: Notify,
    }

    /// A task's result, filled by its executor task and taken by its join.
    struct Slot<T> {
        result: Mutex<Option<T>>,
        done: Notify,
    }

    /// A spawned task: the host runtime's handle, or the scope's slot.
    pub(crate) struct Task<T>(TaskKind<T>);

    enum TaskKind<T> {
        /// Spawned with no scope installed: the host runtime's.
        Host(tokio::task::JoinHandle<T>),
        /// Spawned in a scope: shuttle runs it.
        Scoped(Arc<Slot<T>>),
    }

    impl<T> Task<T> {
        /// Waits for the task. A task of the host runtime reports a panic or
        /// a cancellation as `join_panic_error` describes; a panicking task of
        /// a scope fails the iteration (the executor reports it), so a scoped
        /// join has no error of that kind.
        pub(crate) async fn join(self) -> Result<T, Error> {
            match self.0 {
                TaskKind::Host(handle) => handle.await.map_err(join_panic_error),
                TaskKind::Scoped(slot) => loop {
                    let notified = slot.done.notified();
                    {
                        let mut result = slot.result.lock().unwrap();
                        if let Some(value) = result.take() {
                            return Ok(value);
                        }
                    }
                    notified.await;
                },
            }
        }
    }

    /// The tasks of one parallel dispatch (`run_parallel`). `join_next`
    /// returns them in the order they were spawned: the caller sorts its
    /// results by registration index anyway.
    pub(crate) struct TaskSet<T> {
        tasks: VecDeque<Task<T>>,
    }

    impl<T: Send + 'static> TaskSet<T> {
        pub(crate) fn new() -> Self {
            Self {
                tasks: VecDeque::new(),
            }
        }

        pub(crate) fn spawn_on<F>(&mut self, future: F, handle: &tokio::runtime::Handle)
        where
            F: Future<Output = T> + Send + 'static,
        {
            self.tasks.push_back(spawn(handle, future));
        }

        pub(crate) async fn join_next(&mut self) -> Option<Result<T, Error>> {
            let task = self.tasks.pop_front()?;
            Some(task.join().await)
        }
    }

    /// The runnable queue of the installed scope, or the host runtime when
    /// there is none.
    pub(crate) fn spawn<F>(handle: &tokio::runtime::Handle, future: F) -> Task<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let Some(scope) = current() else {
            return Task(TaskKind::Host(handle.spawn(future)));
        };
        *scope.pending.lock().unwrap() += 1;
        let slot = Arc::new(Slot {
            result: Mutex::new(None),
            done: Notify::new(),
        });
        let write = slot.clone();
        // The executor's handle is dropped on purpose: the scope counts the
        // task, so a spawn whose kernel handle is dropped still runs, and
        // `run` waits for it.
        drop(shuttle::future::spawn(async move {
            let _pending = Pending(scope);
            let result = future.await;
            *write.result.lock().unwrap() = Some(result);
            write.done.notify_waiters();
        }));
        Task(TaskKind::Scoped(slot))
    }

    /// The scope installed on this thread, innermost first. Shuttle runs every
    /// task of one execution on the model's operating-system thread, so one
    /// thread at a time is one execution, and a task a task spawns sees the
    /// same scope without it being passed along. (A `std::thread_local!` would
    /// not do: the shuttle threads of one execution share it.)
    fn current() -> Option<Arc<Scope>> {
        registry()
            .lock()
            .unwrap()
            .get(&std::thread::current().id())
            .and_then(|scopes| scopes.last().cloned())
    }

    fn registry() -> &'static StdMutex<HashMap<ThreadId, Vec<Arc<Scope>>>> {
        static REGISTRY: OnceLock<StdMutex<HashMap<ThreadId, Vec<Arc<Scope>>>>> = OnceLock::new();
        REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
    }

    /// One scope installed on one thread; the scope underneath it comes back
    /// on drop.
    #[cfg(test)]
    pub(crate) struct Entered {
        thread: ThreadId,
    }

    #[cfg(test)]
    impl Entered {
        fn install(scope: Arc<Scope>) -> Self {
            let thread = std::thread::current().id();
            registry()
                .lock()
                .unwrap()
                .entry(thread)
                .or_default()
                .push(scope);
            Self { thread }
        }
    }

    #[cfg(test)]
    impl Drop for Entered {
        fn drop(&mut self) {
            let mut all = registry().lock().unwrap();
            let empty = match all.get_mut(&self.thread) {
                Some(scopes) => {
                    scopes.pop();
                    scopes.is_empty()
                }
                None => false,
            };
            if empty {
                all.remove(&self.thread);
            }
        }
    }

    /// One scope task that has not finished; dropping it wakes the scheduler.
    struct Pending(Arc<Scope>);

    impl Drop for Pending {
        fn drop(&mut self) {
            let mut pending = self.0.pending.lock().unwrap();
            *pending -= 1;
            drop(pending);
            self.0.wake.notify_one();
        }
    }
}

#[cfg(rutis_shuttle)]
mod timed {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex, OnceLock};
    use std::thread::ThreadId;
    use std::time::Duration;

    use shuttle::sync::{Condvar, Mutex};

    /// The model's virtual clock (see the module docs): the deadline timer's
    /// stand-in. Time is where the model put it, and a deadline fires when
    /// [`advance`](Self::advance) reaches it. The model that drives a wait
    /// installs the clock on the thread that calls `wait_events`
    /// ([`enter`](Self::enter)), so a `Ctx` needs no clock field and a model
    /// that never waits pays nothing.
    #[cfg(test)]
    pub(crate) struct Clock {
        inner: Arc<Inner>,
    }

    #[cfg(test)]
    impl Clock {
        /// A clock at time zero, with no deadlines.
        pub(crate) fn new() -> Self {
            Self {
                inner: Arc::new(Inner {
                    now: Mutex::new(Duration::ZERO),
                    waits: Mutex::new(Vec::new()),
                    registered: Condvar::new(),
                    next: Mutex::new(0),
                }),
            }
        }

        /// Registers this clock for the `wait_events` calls on this thread
        /// until the guard is dropped. A shuttle thread keeps its own guard;
        /// a task shuttle runs on this thread's executor sees this one.
        pub(crate) fn enter(&self) -> ClockEntered {
            ClockEntered::install(self.inner.clone())
        }

        /// Where time is.
        pub(crate) fn now(&self) -> Duration {
            *self.inner.now.lock().unwrap()
        }

        /// Moves time forward by `by` and fires every deadline that is now
        /// due, in registration order; returns how many fired. A deadline
        /// registered at `t` with a limit of `l` fires at `t + l`.
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

        /// Blocks until `waits` deadlines are registered. The model's scheduler
        /// calls it before moving time past a deadline it must fire; with no
        /// such wait it never returns, and shuttle reports the deadlock.
        pub(crate) fn wait_for_waiters(&self, waits: usize) {
            let mut pending = self.inner.waits.lock().unwrap();
            while pending.len() < waits {
                pending = self.inner.registered.wait(pending).unwrap();
            }
        }
    }

    #[cfg(test)]
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
        /// The deadlines registered and not yet fired or taken back.
        waits: Mutex<Vec<Wait>>,
        /// Wakes `wait_for_waiters` when one is added.
        registered: Condvar,
        next: Mutex<u64>,
    }

    /// One registered deadline. A shuttle build without tests has no clock to
    /// install (`Clock` is the models' tool), so `deadline` never registers one
    /// there and nothing reads these two fields; the model-facing `advance`
    /// reads them in a test build.
    #[cfg_attr(not(test), allow(dead_code))]
    struct Wait {
        id: u64,
        due: Duration,
        /// Completes the wait's future; taken by `advance`.
        fire: tokio::sync::oneshot::Sender<()>,
    }

    /// One registration, taken back when it is dropped: the plain build's
    /// thread ending when its channel closes. Field order is the drop order.
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

    /// One wait's deadline. Dropping it (the calls finished first) takes the
    /// registration back, so a later advance fires nothing for it.
    pub(crate) struct Deadline {
        /// Dropped with the value, whichever way the wait ends.
        _registration: Registration,
        past: tokio::sync::oneshot::Receiver<()>,
    }

    impl Deadline {
        /// Whether the deadline passed before the wait was dropped (`Err`
        /// disables the branch in `wait_events`'s `select!`).
        pub(crate) async fn passed(self) -> Result<(), ()> {
            self.past.await.map_err(|_| ())
        }
    }

    /// Registers `limit` from now on the clock installed on this thread;
    /// `None` when there is none (the model waits for its calls without a
    /// deadline).
    pub(crate) fn deadline(limit: Duration) -> Option<Deadline> {
        let clock = current()?;
        let id = {
            let mut next = clock.next.lock().unwrap();
            *next += 1;
            *next
        };
        let due = *clock.now.lock().unwrap() + limit;
        let (late, past) = tokio::sync::oneshot::channel::<()>();
        clock.waits.lock().unwrap().push(Wait {
            id,
            due,
            fire: late,
        });
        clock.registered.notify_all();
        Some(Deadline {
            _registration: Registration { clock, id },
            past,
        })
    }

    /// The clock installed on this thread, innermost first. Keyed by the
    /// operating-system thread, as the spawn scope is, and for the same
    /// reason: shuttle's threads of one execution share a `std::thread_local!`.
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

    /// One clock installed on one thread; the clock underneath it comes back on
    /// drop.
    #[cfg(test)]
    pub(crate) struct ClockEntered {
        thread: ThreadId,
    }

    #[cfg(test)]
    impl ClockEntered {
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

    #[cfg(test)]
    impl Drop for ClockEntered {
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
}
