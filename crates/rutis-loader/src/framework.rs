//! `rutis/row` (events protocol §8): every state change of a configured
//! row, any language's, on the logical bus at the row's scope. Plugins of
//! every language listen to it; only rutis emits it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use rutis::events::Scope;
use rutis::host::CtxHost;
use rutis::host::{FiberState, FiberStatusChanged, FiberView, PluginId};
use rutis::internal::CtxInternal;
use rutis::internal::EventKey;
use rutis::{Ctx, Error, Event};
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
    /// The row's id as configured (the loader's row id, as a listener
    /// descriptor's `row`): the same in every copy of an instanced group,
    /// told apart by `scope`.
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
/// What changed before the follower was in `followers` is read from the
/// state, and an older change delivered after it is not said.
pub(crate) fn follow(
    followers: &Arc<Followers>,
    ctx: &Ctx,
    view: &FiberView,
    row: &str,
    plugin: &str,
) {
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
        followers: Arc::downgrade(followers),
    });
    // A dispatch selects its listeners before it runs them, so a change
    // emitted before this follower entered the map can still arrive after
    // it. `saw` drops that one: `last` is seeded below from
    // `view.sequenced_state()`, which is at least as new as anything the
    // earlier selection could carry.
    followers.insert(follower.clone());
    let (seq, now) = view.sequenced_state();
    follower.saw(seq, now.state);
}

/// The rows one loader follows, by the fiber each one runs in.
///
/// The loader registers one `FiberStatusChanged` listener for all of them
/// ([`Followers::listen`]); a per-row listener would make every change in
/// the process run every row's handler, which costs O(rows) per change.
#[derive(Default)]
pub(crate) struct Followers {
    rows: Mutex<HashMap<PluginId, Arc<Follower>>>,
    /// The registration, taken when the loader has stopped and its last
    /// follower has left.
    listener: Mutex<Listener>,
}

#[derive(Default)]
struct Listener {
    disposer: Option<rutis::Disposer>,
    /// The loader's fiber is going: rows still stop, and their changes must
    /// be said.
    stopped: bool,
}

impl Followers {
    /// Registers the dispatch on the root of `ctx`'s tree.
    ///
    /// It follows the loader's rows wherever they run — its tree, the
    /// instances it creates, the anchors below — through the map keyed by the
    /// change's fiber. It is registered on the root rather than on the
    /// loader's context because a row stopping with the loader starts
    /// delivery while the loader's own fiber is `closing`, and an entry whose
    /// owner is closing is skipped (`Entry::live`). It leaves with
    /// [`Followers::stop`] and the last follower (see [`Followers::remove`]),
    /// and called again while it is still there it keeps that one
    /// registration.
    pub(crate) fn listen(self: &Arc<Self>, ctx: &Ctx) -> Result<(), Error> {
        let mut listener = self.listener.lock().unwrap();
        // Live again either way: a registration found below is the one of
        // this run.
        listener.stopped = false;
        // A loader applied again (a restart) may come before the rows of the
        // previous run have said `Disposed`: their deliveries are spawned
        // tasks, and nothing in `restart` waits for them. The registration is
        // still there then — it is on the root, which is not closing, so it
        // still delivers — and it stays the one registration. Registering a
        // second one would deliver every change twice and leave the first
        // behind: `stop()` took it only for the last follower, which is no
        // longer the rule now that `stopped` is false again.
        if listener.disposer.is_some() {
            return Ok(());
        }
        let followers = self.clone();
        listener.disposer = Some(ctx.root_ctx().on_sync::<FiberStatusChanged>(
            move |changed, _| {
                followers.dispatch(&changed);
                Ok(None)
            },
        )?);
        Ok(())
    }

    /// The loader's fiber is being disposed. Its rows are still going: the
    /// listener stays until the last of them has left, so every `stopped` is
    /// still said.
    pub(crate) fn stop(&self) {
        let empty = self.rows.lock().unwrap().is_empty();
        let disposer = {
            let mut listener = self.listener.lock().unwrap();
            listener.stopped = true;
            if empty {
                listener.disposer.take()
            } else {
                None
            }
        };
        if let Some(disposer) = disposer {
            dispose_in_background(disposer);
        }
    }

    /// Routes one change to the follower of its fiber, if a row still runs
    /// there. The map is released before the follower runs: it removes
    /// itself when the fiber is disposed.
    fn dispatch(&self, changed: &FiberStatusChanged) {
        let follower = self.rows.lock().unwrap().get(&changed.plugin_id).cloned();
        if let Some(follower) = follower {
            follower.saw(changed.seq, changed.to);
        }
    }

    fn insert(&self, follower: Arc<Follower>) {
        self.rows.lock().unwrap().insert(follower.view.id, follower);
    }

    /// Takes the row out of the map; the last one out, after the loader has
    /// stopped, withdraws the listener.
    fn remove(&self, id: PluginId) {
        let empty = {
            let mut rows = self.rows.lock().unwrap();
            rows.remove(&id);
            rows.is_empty()
        };
        if !empty {
            return;
        }
        let disposer = {
            let mut listener = self.listener.lock().unwrap();
            if listener.stopped {
                listener.disposer.take()
            } else {
                None
            }
        };
        if let Some(disposer) = disposer {
            dispose_in_background(disposer);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.rows.lock().unwrap().len()
    }
}

/// Withdraws a registration without holding up the caller. Without a runtime
/// it is not withdrawn: the entry stays on the root until the root goes, one
/// entry per unmounted loader.
fn dispose_in_background(disposer: rutis::Disposer) {
    if let Ok(tasks) = tokio::runtime::Handle::try_current() {
        tasks.spawn(async move {
            let _ = disposer.dispose().await;
        });
    }
}

type Emit = Box<dyn Fn(RowState, Option<String>) + Send + Sync>;

/// Follows one row's fiber.
struct Follower {
    view: FiberView,
    /// The `seq` of the latest change seen, and the state said last.
    last: Mutex<(u64, RowState)>,
    emit: Emit,
    /// The loader's followers, weakly: the map holds the follower, so a
    /// strong reference here would keep both alive when a `Disposed` is
    /// never delivered.
    followers: Weak<Followers>,
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
            if let Some(followers) = self.followers.upgrade() {
                followers.remove(self.view.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rutis::host::{BoxFuture, DynPlugin, Effect};
    use rutis::internal::CtxInternal;

    fn status_listeners(root: &Ctx) -> usize {
        root.events()
            .subscriptions()
            .into_iter()
            .filter(|entry| entry.name == FiberStatusChanged::NAME)
            .count()
    }

    /// A row that becomes Active when the gate opens.
    struct GatedRow(Arc<tokio::sync::Notify>);

    impl DynPlugin for GatedRow {
        fn name(&self) -> &str {
            "row"
        }

        fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                self.0.notified().await;
                Ok(Effect::Done)
            })
        }
    }

    async fn mount_row() -> (Ctx, Arc<Followers>, FiberView, Arc<tokio::sync::Notify>) {
        let root = Ctx::root().unwrap();
        let followers = Arc::new(Followers::default());
        followers.listen(&root).unwrap();
        let gate = Arc::new(tokio::sync::Notify::new());
        let view = root.plugin(GatedRow(gate.clone()));
        (root, followers, view, gate)
    }

    /// Every change of a followed row is said once and in order, and a change
    /// older than the state `follow` seeded is not said: that one covers an
    /// emission selected before the follower entered the map, delivered after
    /// `follow` read the state.
    #[tokio::test]
    async fn changes_are_said_once_in_order() {
        let (root, followers, view, gate) = mount_row().await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        root.on::<RowEvent>(move |event, _| {
            sink.lock().unwrap().push(event.state);
            std::future::ready(Ok(None))
        })
        .unwrap();
        follow(&followers, &root, &view, "row", "row");
        assert_eq!(followers.len(), 1);
        gate.notify_one();
        rutis_test_support::eventually(
            || {
                (seen.lock().unwrap().as_slice() == [RowState::Pending, RowState::Running])
                    .then_some(())
            },
            "pending then running",
        )
        .await;
        let follower = followers
            .rows
            .lock()
            .unwrap()
            .get(&view.id)
            .cloned()
            .unwrap();
        follower.saw(1, FiberState::Loading);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [RowState::Pending, RowState::Running]
        );
        view.dispose().await.unwrap();
        rutis_test_support::eventually(
            || {
                (seen.lock().unwrap().as_slice()
                    == [RowState::Pending, RowState::Running, RowState::Stopped])
                .then_some(())
            },
            "stopped",
        )
        .await;
        rutis_test_support::eventually(
            || (followers.len() == 0).then_some(()),
            "the disposed row left the map",
        )
        .await;
    }

    /// A row that runs and stops at once still says `running`: the states are
    /// the kernel's changes, not a watch of the latest one
    /// ([`follow`]'s contract).
    #[tokio::test]
    async fn a_row_that_ends_at_once_still_says_running() {
        let (root, followers, view, gate) = mount_row().await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        root.on::<RowEvent>(move |event, _| {
            sink.lock().unwrap().push(event.state);
            std::future::ready(Ok(None))
        })
        .unwrap();
        follow(&followers, &root, &view, "row", "row");
        gate.notify_one();
        rutis_test_support::eventually(
            || (view.state().state == FiberState::Active).then_some(()),
            "the row runs",
        )
        .await;
        // Disposed before the change above was delivered: both are still said.
        view.dispose().await.unwrap();
        rutis_test_support::eventually(
            || {
                (seen.lock().unwrap().as_slice()
                    == [RowState::Pending, RowState::Running, RowState::Stopped])
                .then_some(())
            },
            "pending, running, stopped",
        )
        .await;
        rutis_test_support::eventually(
            || (followers.len() == 0).then_some(()),
            "the row left the map",
        )
        .await;
    }
    /// guarantee: events/framework (rutis/row)
    ///
    /// A loader applied again before its rows' `Disposed` was delivered keeps
    /// the one registration: `stop()` leaves it for the rows still going, and
    /// the second `listen()` registers no other. The rows still say
    /// `stopped`, and `stop()` with an empty map withdraws it.
    #[tokio::test]
    async fn a_second_listen_keeps_the_registration() {
        let root = Ctx::root().unwrap();
        let before = status_listeners(&root);
        let followers = Arc::new(Followers::default());
        followers.listen(&root).unwrap();
        let gate = Arc::new(tokio::sync::Notify::new());
        let view = root.plugin(GatedRow(gate.clone()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        root.on::<RowEvent>(move |event, _| {
            sink.lock().unwrap().push(event.state);
            std::future::ready(Ok(None))
        })
        .unwrap();
        follow(&followers, &root, &view, "row", "row");
        gate.notify_one();
        rutis_test_support::eventually(
            || (view.state().state == FiberState::Active).then_some(()),
            "the row runs",
        )
        .await;
        assert_eq!(status_listeners(&root), before + 1);
        // The loader's fiber goes while the row still runs: the registration
        // stays, for the row to say its `stopped`.
        followers.stop();
        assert_eq!(status_listeners(&root), before + 1);
        // Applied again before the row's `Disposed` was delivered.
        followers.listen(&root).unwrap();
        assert_eq!(
            status_listeners(&root),
            before + 1,
            "the second listen registers nothing"
        );
        view.dispose().await.unwrap();
        rutis_test_support::eventually(
            || {
                (seen.lock().unwrap().as_slice()
                    == [RowState::Pending, RowState::Running, RowState::Stopped])
                .then_some(())
            },
            "pending, running, stopped",
        )
        .await;
        assert_eq!(status_listeners(&root), before + 1);
        rutis_test_support::eventually(
            || (followers.len() == 0).then_some(()),
            "the row left the map",
        )
        .await;
        // The row leaving did not withdraw it: the second `listen` reset
        // `stopped`.
        assert_eq!(status_listeners(&root), before + 1);
        followers.stop();
        rutis_test_support::eventually(
            || (status_listeners(&root) == before).then_some(()),
            "the registration left with the empty map",
        )
        .await;
    }
}
