//! `rutis/row` (events protocol §8): every state change of a configured
//! row, any language's, on the logical bus at the row's scope. Plugins of
//! every language listen to it; only rutis emits it.

use std::sync::Arc;

use rutis::events::Scope;
use rutis::{Ctx, Event, EventKey, FiberState, FiberView};
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
pub(crate) fn follow(ctx: &Ctx, view: &FiberView, row: &str, plugin: &str) {
    let Ok(tasks) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let mut watch = view.watch();
    let ctx = ctx.clone();
    let (row, plugin) = (row.to_owned(), plugin.to_owned());
    tasks.spawn(async move {
        let scope = ctx.scope();
        let mut last = None;
        loop {
            let (state, error) = {
                let snapshot = watch.borrow_and_update();
                (
                    state_of(snapshot.state),
                    snapshot.error.as_ref().map(|error| error.to_string()),
                )
            };
            if last != Some(state) {
                last = Some(state);
                let event = RowEvent {
                    row: row.clone(),
                    plugin: plugin.clone(),
                    scope: scope.clone(),
                    state,
                    error: (state == RowState::Failed).then_some(error).flatten(),
                };
                let _ = ctx.events().emit_framework(
                    &ctx,
                    &EventKey::<RowEvent>::of(),
                    Arc::new(event),
                );
            }
            if state == RowState::Stopped && watch.borrow().state == FiberState::Disposed {
                return;
            }
            if watch.changed().await.is_err() {
                return;
            }
        }
    });
}
