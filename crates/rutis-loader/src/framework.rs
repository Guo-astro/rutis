//! `rutis/row` (events protocol §8): every state change of a configured
//! row, any language's, on the logical bus at the row's scope. Plugins of
//! every language listen to it; only rutis emits it.

use std::sync::{Arc, Mutex};

use rutis::events::Scope;
use rutis::{CordisError, Ctx, Event, EventKey, FiberState, FiberStatusChanged, FiberView};
use serde::{Deserialize, Serialize};

/// A row's state, as `rutis/row` gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowState {
    /// Waiting for what it needs, or loading.
    Pending,
    Running,
    /// `error` says why.
    Failed,
    Stopped,
}

/// `rutis/row`: row `row` (plugin `plugin`, at `scope`) is now `state`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RowEvent {
    pub row: String,
    pub plugin: String,
    pub scope: Vec<Scope>,
    pub state: RowState,
    pub error: Option<String>,
}

impl Event for RowEvent {
    const NAME: &'static str = "rutis/row";
    type Value = ();
}

fn state_of(state: FiberState) -> RowState {
    match state {
        FiberState::Pending | FiberState::Loading => RowState::Pending,
        FiberState::Active => RowState::Running,
        FiberState::Failed => RowState::Failed,
        FiberState::Disposed | FiberState::Unloading => RowState::Stopped,
    }
}

/// Emit `rutis/row` at each state change of the row `row` running in
/// `view`, from `ctx` (where the row is), until it is disposed.
///
/// Each change is taken from the kernel's `FiberStatusChanged` of the
/// fiber, in `seq` order, none skipped: a row that runs and stops at once
/// still says `running` (a watch of the state would see only the latest).
/// What changed before the listener was on the table is read from the
/// state, and an older change delivered after it is not said.
pub(crate) fn follow(ctx: &Ctx, view: &FiberView, row: &str, plugin: &str) {
    let scope = ctx.scope();
    let emit = {
        let ctx = ctx.clone();
        let (row, plugin) = (row.to_owned(), plugin.to_owned());
        move |state: RowState, error: Option<String>| {
            let event = RowEvent {
                row: row.clone(),
                plugin: plugin.clone(),
                scope: scope.clone(),
                state,
                error,
            };
            let _ = ctx
                .events()
                .emit_framework(&ctx, &EventKey::<RowEvent>::of(), Arc::new(event));
        }
    };
    // Just spawned: pending, said before anything else of the row.
    emit(RowState::Pending, None);
    let follower = Arc::new(Follower {
        view: view.clone(),
        last: Mutex::new((0, RowState::Pending)),
        emit: Box::new(emit),
        listening: Mutex::default(),
    });
    let root = ctx.root_ctx();
    if let Ok(disposer) = root.events().on_sync(
        &root,
        &EventKey::<FiberStatusChanged>::of(),
        Listening(follower.clone()),
    ) {
        *follower.listening.lock().unwrap() = Some(disposer);
    }
    let (seq, now) = view.sequenced_state();
    follower.saw(seq, now.state);
}

type Emit = Box<dyn Fn(RowState, Option<String>) + Send + Sync>;

/// Follows one row's fiber.
struct Follower {
    view: FiberView,
    /// The `seq` of the latest change seen, and the state said last.
    last: Mutex<(u64, RowState)>,
    emit: Emit,
    /// Its registration: it leaves when the row is disposed.
    listening: Mutex<Option<rutis::Disposer>>,
}

impl Follower {
    fn saw(&self, seq: u64, fiber: FiberState) {
        let state = state_of(fiber);
        let mut last = self.last.lock().unwrap();
        if seq <= last.0 && seq != 0 {
            return;
        }
        last.0 = last.0.max(seq);
        if last.1 != state {
            last.1 = state;
            let error = (state == RowState::Failed)
                .then(|| self.view.state().error.map(|error| error.to_string()))
                .flatten();
            (self.emit)(state, error);
        }
        drop(last);
        if fiber == FiberState::Disposed {
            if let Some(disposer) = self.listening.lock().unwrap().take() {
                if let Ok(tasks) = tokio::runtime::Handle::try_current() {
                    tasks.spawn(async move {
                        let _ = disposer.dispose().await;
                    });
                }
            }
        }
    }
}

struct Listening(Arc<Follower>);

impl rutis::SyncListener<FiberStatusChanged> for Listening {
    fn call(
        &self,
        _: &Ctx,
        changed: &FiberStatusChanged,
        _: rutis::SyncCall<'_, ()>,
    ) -> Result<Option<()>, CordisError> {
        if changed.plugin_id == self.0.view.id {
            self.0.saw(changed.seq, changed.to);
        }
        Ok(None)
    }
}
