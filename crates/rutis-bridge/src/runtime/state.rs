//! A language runtime's state, for code that is not one of its plugins (the
//! loader's status, the host's status lines): [`RuntimeStates`] is what
//! starts the runtime (rutis-host's `rutis-host/spawn`), [`RuntimeHandle`]
//! what watches it. The runtime itself is the far end of a session
//! ([`crate::Session`]) on the channel of its process.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rutis::Ctx;
use tokio::sync::{mpsc, watch};

use crate::runtime::Process;

/// What the runtime is doing, as seen through a [`RuntimeHandle`].
#[derive(Clone)]
pub enum RuntimeState {
    /// Not started yet, or stopped.
    Idle,
    /// Its process is starting, or its session greeting.
    Starting,
    Ready(Arc<Process>),
    /// It failed to start, or its process ended on its own.
    Down(String),
}

impl std::fmt::Debug for RuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeState::Idle => f.write_str("Idle"),
            RuntimeState::Starting => f.write_str("Starting"),
            RuntimeState::Ready(_) => f.write_str("Ready"),
            RuntimeState::Down(why) => f.debug_tuple("Down").field(why).finish(),
        }
    }
}

/// The state of one runtime, set by what starts it. Clones share it.
#[derive(Clone)]
pub struct RuntimeStates {
    name: String,
    anchor: PathBuf,
    state: Arc<watch::Sender<RuntimeState>>,
    starts: Arc<AtomicU64>,
    announcing: Arc<AtomicBool>,
    /// Every change, in order, for [`RuntimeStates::announce`]: the watch
    /// keeps only the latest, so a down and the next start in quick
    /// succession would reach it as one unchanged `Ready`.
    changes: Arc<Mutex<Option<mpsc::UnboundedSender<RuntimeState>>>>,
    processes: Option<crate::pipe::Processes>,
}

impl RuntimeStates {
    /// The runtime `name`, whose plugins resolve from `anchor` (a Node
    /// runtime's `package.json`, a project directory).
    pub fn new(name: impl Into<String>, anchor: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            anchor: anchor.into(),
            state: Arc::new(watch::channel(RuntimeState::Idle).0),
            starts: Arc::default(),
            announcing: Arc::default(),
            changes: Arc::default(),
            processes: None,
        }
    }

    /// The processes that run it, for [`RuntimeHandle::pid`].
    pub fn processes(mut self, processes: crate::pipe::Processes) -> Self {
        self.processes = Some(processes);
        self
    }

    /// Its process is starting: a new generation.
    pub fn starting(&self) {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.set(RuntimeState::Starting);
    }

    /// It runs plugins through `process`.
    pub fn ready(&self, process: Arc<Process>) {
        self.set(RuntimeState::Ready(process));
    }

    /// It could not start, or it ended on its own, and why.
    pub fn failed(&self, reason: impl Into<String>) {
        self.set(RuntimeState::Down(reason.into()));
    }

    /// It was stopped, or its start abandoned.
    pub fn stopped(&self) {
        self.set(RuntimeState::Idle);
    }

    fn set(&self, state: RuntimeState) {
        let changes = self.changes.lock().unwrap();
        if let Some(changes) = changes.as_ref() {
            let _ = changes.send(state.clone());
        }
        self.state.send_replace(state);
    }

    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle {
            name: self.name.clone(),
            anchor: self.anchor.clone(),
            state: self.state.subscribe(),
            starts: self.starts.clone(),
            processes: self.processes.clone(),
        }
    }

    /// Emit `rutis/runtime` on `ctx`'s bus at each change of the state from
    /// now on, for as long as the runtime exists (events protocol §8):
    /// `running` once it runs, `down` with why when it failed or ended (or
    /// `stopped` when it was stopped). Going down is also reported to the
    /// error sink ([`ErrorKind::RuntimeDown`](rutis::host::ErrorKind::RuntimeDown)).
    /// Once per runtime: later calls do nothing.
    pub fn announce(&self, ctx: &Ctx) {
        if self.announcing.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(tasks) = tokio::runtime::Handle::try_current() else {
            return;
        };
        // Every change from now on, after the state as it is now.
        let (sender, mut changes) = mpsc::unbounded_channel();
        let current = {
            let mut installed = self.changes.lock().unwrap();
            *installed = Some(sender);
            self.state.borrow().clone()
        };
        let ctx = rutis::host::CtxHost::root_ctx(ctx);
        let name = self.name.clone();
        tasks.spawn(async move {
            use crate::events::RuntimeEventState::{Down, Running};
            let mut running = false;
            // Each time it goes down, the error sink hears it once (on
            // stderr, by default: an embedding sees it, #678).
            let mut down = false;
            let mut state = current;
            loop {
                if !matches!(state, RuntimeState::Down(_)) {
                    down = false;
                }
                match state {
                    RuntimeState::Ready(_) if !running => {
                        running = true;
                        crate::events::emit_runtime(&ctx, &name, Running, None);
                    }
                    RuntimeState::Down(reason) => {
                        running = false;
                        // Not while the whole tree shuts down: ending
                        // then is expected.
                        if !down && !rutis::host::CtxHost::is_shutting_down(&ctx) {
                            down = true;
                            let report = rutis::host::ErrorKind::RuntimeDown {
                                runtime: name.clone(),
                                reason: reason.clone(),
                            };
                            rutis::host::CtxHost::error_sink(&ctx)(Arc::new(report));
                        }
                        crate::events::emit_runtime(&ctx, &name, Down, Some(reason));
                    }
                    RuntimeState::Idle if running => {
                        running = false;
                        crate::events::emit_runtime(&ctx, &name, Down, Some("stopped".into()));
                    }
                    _ => {}
                }
                match changes.recv().await {
                    Some(next) => state = next,
                    None => return,
                }
            }
        });
    }
}

/// How the process behind `process` went, once its session ended: its
/// channel's reason (`the process exited with code 1`, say).
pub fn ended_status(process: &Process) -> String {
    match (process.exit_status(), process.connection().close_reason()) {
        (Some(status), _) => format!("runtime process {status}"),
        (None, Some(reason)) => format!("runtime ended: {reason}"),
        (None, None) => "runtime disconnected".to_owned(),
    }
}

/// Observes a runtime from code that is not a plugin (a loader resolver,
/// the host's status).
#[derive(Clone)]
pub struct RuntimeHandle {
    name: String,
    anchor: PathBuf,
    state: watch::Receiver<RuntimeState>,
    starts: Arc<AtomicU64>,
    processes: Option<crate::pipe::Processes>,
}

impl RuntimeHandle {
    /// The process id of the runtime's process, while it runs.
    pub fn pid(&self) -> Option<u32> {
        match self.state() {
            RuntimeState::Ready(_) | RuntimeState::Starting => self
                .processes
                .as_ref()
                .and_then(|processes| processes.pid()),
            _ => None,
        }
    }

    /// The runtime's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where its plugins resolve from.
    pub fn anchor(&self) -> &Path {
        &self.anchor
    }

    pub fn state(&self) -> RuntimeState {
        self.state.borrow().clone()
    }

    /// How many times the runtime has started, counted as it happens: a
    /// runtime seen down twice with a count between says it came back and
    /// went down again, however short it ran.
    pub fn starts(&self) -> u64 {
        self.starts.load(Ordering::SeqCst)
    }

    /// Resolves at the next change of [`RuntimeHandle::state`]. A change
    /// between a look at the state and this call is not seen: to wait for a
    /// state, use [`RuntimeHandle::wait_for`].
    pub async fn changed(&self) {
        let mut state = self.state.clone();
        state.borrow_and_update();
        let _ = state.changed().await;
    }

    /// Waits until the state is one `until` accepts, and gives it. The
    /// state as it is now counts, so nothing between an earlier look and
    /// this wait is missed. A runtime whose [`RuntimeStates`] are gone no
    /// longer changes: unless its last state is accepted, this does not
    /// resolve.
    pub async fn wait_for(&self, mut until: impl FnMut(&RuntimeState) -> bool) -> RuntimeState {
        let mut state = self.state.clone();
        // Cloned out at once: the borrow holds the state's lock.
        let found = state
            .wait_for(|now| until(now))
            .await
            .map(|now| now.clone());
        match found {
            Ok(now) => now,
            Err(_) => std::future::pending().await,
        }
    }

    /// The state as a receiver of its own: a loop waiting on it next to
    /// something else marks what it looked at (`borrow_and_update`), so a
    /// change right after the look still wakes it.
    pub fn subscribe(&self) -> watch::Receiver<RuntimeState> {
        self.state.clone()
    }

    /// Whether the running runtime reported `feature` (`rows.v2`, `hosts`,
    /// `leaf`, …); `false` while none runs.
    pub fn supports(&self, feature: &str) -> bool {
        match &*self.state.borrow() {
            RuntimeState::Ready(process) => process.supports(feature),
            _ => false,
        }
    }

    /// The running process. Waits while it is starting; `None` when it is
    /// not running.
    pub async fn ready(&self) -> Option<Arc<Process>> {
        let mut state = self.state.clone();
        let settled = state
            .wait_for(|state| !matches!(state, RuntimeState::Starting))
            .await
            .ok()?;
        match &*settled {
            RuntimeState::Ready(process) => Some(process.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A state reached after a look at the state but before the wait is the
    /// one the wait gives. Missed, a loop waiting for its runtime to start
    /// woke only when the process later ended and took that for a failed
    /// start (multilang.rs::a_dead_python_process_stops_only_its_rows on CI).
    #[tokio::test]
    async fn a_state_reached_before_the_wait_is_not_missed() {
        let runtime = RuntimeStates::new("py", ".");
        let handle = runtime.handle();
        runtime.starting();
        assert!(matches!(handle.state(), RuntimeState::Starting));
        runtime.failed("crashed");
        let state = tokio::time::timeout(
            Duration::from_secs(5),
            handle.wait_for(|state| matches!(state, RuntimeState::Down(_))),
        )
        .await
        .expect("the state it was waiting for had already come");
        assert!(matches!(state, RuntimeState::Down(reason) if reason == "crashed"));

        // A loop of its own marks what it looked at: a change right after
        // the look still wakes it.
        let mut states = handle.subscribe();
        assert!(matches!(*states.borrow_and_update(), RuntimeState::Down(_)));
        runtime.starting();
        tokio::time::timeout(Duration::from_secs(5), states.changed())
            .await
            .expect("a change after the look wakes the wait")
            .unwrap();
        assert!(matches!(
            *states.borrow_and_update(),
            RuntimeState::Starting
        ));
    }
}
