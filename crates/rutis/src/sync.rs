//! The sync primitives and the spawn stand-in the shuttle models swap (#179,
//! #622 item 1).
//!
//! Built with `--cfg rutis_shuttle`, the spots whose interleavings the models
//! explore use shuttle's primitives, so every access to them is a point where
//! shuttle may switch threads: a fiber driver's `alive` flag (`post_join`
//! against the driver's exit), an effect record's state (claiming its
//! cleanups) and a spawned task's result. Everything else keeps std's. A plain
//! build is unchanged: these are std's types and [`spawn`] is the host
//! runtime's.
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
//! switch in a model; a model that drives the emit chain must account for it.

#[cfg(not(rutis_shuttle))]
pub(crate) use host::{spawn, Task, TaskSet};

#[cfg(rutis_shuttle)]
pub(crate) use spawned::{spawn, Task, TaskSet};

#[cfg(all(test, rutis_shuttle))]
pub(crate) use spawned::SpawnScope;

#[cfg(not(rutis_shuttle))]
pub(crate) use std::sync::{atomic::AtomicBool, Mutex};

#[cfg(rutis_shuttle)]
pub(crate) use shuttle::sync::{atomic::AtomicBool, Mutex};

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

    use crate::error::{join_panic_error, Error};

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
