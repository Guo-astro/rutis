use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Weak};

use tokio::sync::Notify;

use crate::error::{aggregate_arcs, join_panic_error, panic_error, Error, ErrorKind};
use crate::fiber::FiberInner;
use crate::BoxFuture;

/// The cleanup a plugin hands back. A cleanup may fail; a closure that needs
/// the context captures an owned `Ctx` (it is not passed a `&Ctx`).
pub enum Effect {
    /// No cleanup.
    Done,
    /// A synchronous cleanup.
    Disposer(Box<dyn FnOnce() -> Result<(), Error> + Send>),
    /// An asynchronous cleanup. It runs in its own task; a panic becomes a
    /// `PluginFailed`.
    AsyncDisposer(Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Error>> + Send>),
    /// Several cleanups, run in reverse order of declaration (LIFO).
    Many(Vec<Effect>),
}

/// Whether an owned cleanup record is waiting or currently being drained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectPhase {
    Live,
    Draining,
}

/// Read-only cleanup ownership tree. Child phases follow their owning record;
/// the tree does not track which individual leaf is currently executing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EffectMeta {
    pub label: String,
    pub phase: EffectPhase,
    pub children: Vec<EffectMeta>,
}

impl EffectMeta {
    fn set_phase(&mut self, phase: EffectPhase) {
        self.phase = phase;
        for child in &mut self.children {
            child.set_phase(phase);
        }
    }
}

impl Effect {
    fn kind(&self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Disposer(_) => "disposer",
            Self::AsyncDisposer(_) => "async disposer",
            Self::Many(_) => "many",
        }
    }

    fn into_cleanups(self, label: String, out: &mut Vec<Cleanup>) -> EffectMeta {
        let mut children = Vec::new();
        match self {
            Effect::Done => {}
            Effect::Disposer(f) => out.push(Cleanup::Sync(f)),
            Effect::AsyncDisposer(f) => out.push(Cleanup::Async(f)),
            Effect::Many(v) => {
                for (index, effect) in v.into_iter().enumerate() {
                    let child_label = format!("{index}: {}", effect.kind());
                    children.push(effect.into_cleanups(child_label, out));
                }
            }
        }
        EffectMeta {
            label,
            phase: EffectPhase::Live,
            children,
        }
    }
}

pub(crate) enum Cleanup {
    Sync(Box<dyn FnOnce() -> Result<(), Error> + Send>),
    Async(Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Error>> + Send>),
}

/// Where a cleanup came from, for a shutdown that runs out of time to
/// say what it still waits for (#678).
pub(crate) enum Origin {
    /// `ctx.effect`, called here.
    Effect(&'static std::panic::Location<'static>),
    /// The withdrawal of a service the plugin provided.
    Provide(crate::TypeKey),
    /// The cleanup the plugin's `apply` returned.
    Apply,
    /// A child plugin's unload: the child says what it waits for.
    Mount,
    /// Anything else, by its label.
    Other,
}

/// A registered cleanup, kept apart from the code that registered it. The
/// cleanups run exactly once, strictly LIFO and one at a time; repeated
/// drains wait for the same result. One error is returned as it is, several
/// as an unflattened aggregate, and one failing cleanup does not stop the
/// rest. When done, the record removes itself from its owner fiber's
/// effects list, so short-lived records under a long-lived parent do not
/// accumulate.
pub(crate) struct EffectRecord {
    /// `crate::sync`: a shuttle mutex in the shuttle models (#179).
    st: crate::sync::Mutex<EffectState>,
    metadata: EffectMeta,
    origin: Origin,
    notify: Notify,
    owner: Option<Weak<FiberInner>>,
}

enum EffectState {
    Live(Vec<Cleanup>),
    Draining,
    Done(Option<Arc<ErrorKind>>),
}

impl EffectRecord {
    pub(crate) fn from(
        effect: Effect,
        label: String,
        origin: Origin,
        owner: Weak<FiberInner>,
    ) -> Arc<Self> {
        let mut cleanups = Vec::new();
        let metadata = effect.into_cleanups(label, &mut cleanups);
        Arc::new(Self {
            st: crate::sync::Mutex::new(EffectState::Live(cleanups)),
            metadata,
            origin,
            notify: Notify::new(),
            owner: Some(owner),
        })
    }

    /// Where it came from and its label, while its cleanups run.
    pub(crate) fn draining(&self) -> Option<(&Origin, &str)> {
        matches!(&*self.st.lock().unwrap(), EffectState::Draining)
            .then_some((&self.origin, self.metadata.label.as_str()))
    }

    pub(crate) fn snapshot(&self) -> Option<EffectMeta> {
        let phase = match &*self.st.lock().unwrap() {
            EffectState::Live(_) => EffectPhase::Live,
            EffectState::Draining => EffectPhase::Draining,
            EffectState::Done(_) => return None,
        };
        let mut metadata = self.metadata.clone();
        metadata.set_phase(phase);
        Some(metadata)
    }

    /// Runs the cleanups. The first caller moves them into a separate task;
    /// every caller, including one that retries after its future was
    /// dropped, waits for the same result (the same `Arc`). Dropping this
    /// future does not stop the cleanups, so the record never stays in
    /// `Draining`.
    pub(crate) async fn drain(
        self: &Arc<Self>,
        handle: &tokio::runtime::Handle,
    ) -> Result<(), Arc<ErrorKind>> {
        if let Some(mut cleanups) = self.claim() {
            let this = self.clone();
            let runner = handle.clone();
            handle.spawn(async move {
                // 清理全程 panic 兜底(评审 #7/#11):任何 panic 转为
                // PluginFailed 错误,任务必定写回 Done,不卡 Draining
                let err = run_cleanups(&mut cleanups, &runner).await;
                this.finish(err);
            });
        }
        self.join().await
    }

    /// The cleanups, to the one caller that finds the record live: checked
    /// and taken under one lock, so concurrent drains run them once.
    pub(crate) fn claim(&self) -> Option<Vec<Cleanup>> {
        let mut st = self.st.lock().unwrap();
        match &mut *st {
            EffectState::Live(_) => {
                let live = std::mem::replace(&mut *st, EffectState::Draining);
                match live {
                    EffectState::Live(cleanups) => Some(cleanups),
                    _ => unreachable!(),
                }
            }
            EffectState::Draining | EffectState::Done(_) => None,
        }
    }

    /// Record the cleanups' result and wake every join.
    pub(crate) fn finish(self: &Arc<Self>, err: Option<Arc<ErrorKind>>) {
        *self.st.lock().unwrap() = EffectState::Done(err.clone());
        // 自摘与寄存(0.2.1)先于 notify:join 返回时宿主列表已无本记录。
        // 仅当记录仍在宿主 effects 列表(提前 drain,收集者不是 fiber
        // 级卸载)才寄存错误并自摘;drain_effects 整表收走的记录由其
        // join 直接收集,不重复寄存。锁内判定与 mem::take 互斥,无竞态。
        if let Some(owner) = self.owner.as_ref().and_then(Weak::upgrade) {
            let mut effects = owner.effects.lock().unwrap();
            let present = effects.iter().any(|r| Arc::ptr_eq(r, self));
            if present {
                effects.retain(|r| !Arc::ptr_eq(r, self));
                if effects.capacity() > 64 && effects.len() * 4 < effects.capacity() {
                    effects.shrink_to_fit();
                }
                if let Some(e) = &err {
                    // Keep the effects lock until the error is queued.
                    // drain_effects takes the whole effects list under
                    // this lock, then consumes drained_errors: it must
                    // see either this record or its queued error.
                    owner.drained_errors.lock().unwrap().push(e.clone());
                }
            }
            drop(effects);
            let weak = Arc::downgrade(self);
            let mut index = owner.effect_index.lock().unwrap();
            index.retain(|entry| !Weak::ptr_eq(entry, &weak) && entry.strong_count() > 0);
            if index.capacity() > 64 && index.len() * 4 < index.capacity() {
                index.shrink_to_fit();
            }
        }
        self.notify.notify_waiters();
    }

    /// Wait for the cleanups' result. The wakeup is registered before the
    /// state is read, so a `finish` between the two is not missed.
    pub(crate) async fn join(&self) -> Result<(), Arc<ErrorKind>> {
        loop {
            let notified = self.notify.notified();
            {
                let st = self.st.lock().unwrap();
                if let EffectState::Done(e) = &*st {
                    return match e.clone() {
                        Some(e) => Err(e),
                        None => Ok(()),
                    };
                }
            }
            // The window `notified` covers: `finish` may run here.
            crate::sync::switch_point();
            notified.await;
        }
    }
}

/// Runs the cleanups strictly LIFO, one at a time. A panic in the closure
/// call (`f()`) or while polling its future becomes a `PluginFailed`; one
/// failing cleanup does not stop the rest.
pub(crate) async fn run_cleanups(
    cleanups: &mut Vec<Cleanup>,
    handle: &tokio::runtime::Handle,
) -> Option<Arc<ErrorKind>> {
    let mut errors: Vec<Error> = Vec::new();
    while let Some(cleanup) = cleanups.pop() {
        let result = match cleanup {
            Cleanup::Sync(f) => match std::panic::catch_unwind(AssertUnwindSafe(f)) {
                Ok(r) => r,
                Err(p) => Err(panic_error(p)),
            },
            Cleanup::Async(f) => {
                // 闭包调用本身也在任务边界内(评审 #7)
                let fut = match std::panic::catch_unwind(AssertUnwindSafe(f)) {
                    Ok(fut) => fut,
                    Err(p) => {
                        errors.push(panic_error(p));
                        continue;
                    }
                };
                let joined = handle.spawn(fut);
                match joined.await {
                    Ok(r) => r,
                    // 取消不是 panic:into_panic 在取消场景会二次 panic(评审 P2);
                    // 无论哪种,EffectRecord 都最终进 Done,不卡 Draining
                    Err(join_err) => Err(join_panic_error(join_err)),
                }
            }
        };
        if let Err(e) = result {
            errors.push(e);
        }
    }
    aggregate_arcs(errors.into_iter().map(Error::shared).collect())
}

/// A handle to release a cleanup early. It is not meant to be put back into
/// an `Effect`. Dropping it does not run the cleanup; the fiber's unload
/// still runs it.
pub struct Disposer {
    run: Option<Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> + Send>>,
}

impl Disposer {
    pub(crate) fn new(
        run: Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> + Send>,
    ) -> Self {
        Self { run: Some(run) }
    }

    /// Runs the cleanup now and waits for it to finish. If it is already
    /// running or done, waits for the same result.
    pub fn dispose(mut self) -> BoxFuture<'static, Result<(), Arc<ErrorKind>>> {
        match self.run.take() {
            Some(run) => run(),
            None => Box::pin(async { Ok(()) }),
        }
    }
}

impl std::fmt::Debug for Disposer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Disposer")
    }
}
