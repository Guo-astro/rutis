//! Instances of instanced groups, created and removed on demand.
//!
//! An instanced group is not spawned by reconcile. Each
//! [`Loader::create_instance`] spawns one more group fiber where the
//! configuration tree puts it, and its rows load inside it as in any group.
//! Copies inside an instance are recorded under the instance's number
//! ([`super::Slot::scope`]), so edits reach every copy.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::Arc;

use rutis::{BoxFuture, CordisError, Ctx, FiberState, FiberView, PluginId};

use crate::LoaderError;

use super::{Inner, InstanceRecord, Loader, LoaderChanged, Slot, State};

/// The result of one row in a new instance.
#[derive(Debug, Clone)]
pub enum InstanceResult {
    Active,
    /// Waiting for services it depends on.
    Waiting,
    /// Resolution, validation, expression evaluation, or `apply` failed.
    Failed(LoaderError),
    /// The row, or a group above it inside the instance, is disabled.
    Skipped,
}

/// A created instance.
#[derive(Clone)]
pub struct Instance {
    /// The instance's group fiber.
    pub plugin: PluginId,
    pub view: FiberView,
    /// Each row inside the instance (nested instanced groups excluded), in
    /// tree order. Whether the instance is usable is the caller's call.
    pub report: Vec<(String, InstanceResult)>,
}

impl std::fmt::Debug for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instance")
            .field("plugin", &self.plugin)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// A pending [`Loader::create_instance`]; await it to create the instance.
pub struct CreateInstance<'a> {
    loader: &'a Loader,
    ctx: Ctx,
    group: String,
    values: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl CreateInstance<'_> {
    /// Attach a value for the factories of plugins in the instance
    /// ([`crate::Build::value`]); one per type, the last one wins.
    pub fn with<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.values.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }
}

impl<'a> IntoFuture for CreateInstance<'a> {
    type Output = Result<Instance, LoaderError>;
    type IntoFuture = BoxFuture<'a, Result<Instance, LoaderError>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(
            self.loader
                .clone()
                .spawn_instance(self.ctx, self.group, self.values),
        )
    }
}

impl Loader {
    /// Create an instance of the instanced group `group`. The parent is
    /// found from `ctx`: the instance (or group) of `group`'s configuration
    /// parent that `ctx`'s plugin belongs to, through any managed plugin
    /// inside it; for a top-level group, the loader's context or any
    /// context outside the loader's plugins.
    ///
    /// Waits until the rows inside settle. Does not take the operation lock,
    /// so a plugin can create instances from its own `apply`.
    pub fn create_instance(&self, ctx: &Ctx, group: &str) -> CreateInstance<'_> {
        CreateInstance {
            loader: self,
            ctx: ctx.clone(),
            group: group.to_owned(),
            values: HashMap::new(),
        }
    }

    async fn spawn_instance(
        self,
        ctx: Ctx,
        group: String,
        values: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    ) -> Result<Instance, LoaderError> {
        let inner = &self.inner;
        inner.check_open()?;
        let (number, view) = {
            let mut state = inner.state.lock().unwrap();
            let state = &mut *state;
            let root = state
                .groups
                .get(&None)
                .map(|g| g.ctx.clone())
                .ok_or(LoaderError::Closed)?;
            if root.root_view().map(|v| v.instance()) != ctx.root_view().map(|v| v.instance()) {
                return Err(LoaderError::InvalidEntry(
                    "the context is not in the loader's tree".into(),
                ));
            }
            let row = state
                .desired
                .row(&group)
                .ok_or_else(|| LoaderError::UnknownEntry(group.clone()))?;
            if let Some(invalid) = &row.invalid {
                return Err(invalid.clone());
            }
            if !row.instanced {
                return Err(LoaderError::InvalidEntry(format!(
                    "{group:?} is not an instanced group"
                )));
            }
            if !state.desired.wanted(row) {
                return Err(LoaderError::InvalidEntry(format!(
                    "{group:?} or a group above it is disabled"
                )));
            }
            let parent = parent_of(state, &ctx, row.parent.as_deref())?;
            let (parent_ctx, parent_token) = state
                .groups
                .get(&parent)
                .map(|g| (g.ctx.clone(), g.token))
                .ok_or_else(|| {
                    LoaderError::InvalidEntry(format!("the parent of {group:?} is not running"))
                })?;
            let index = state.desired.by_id[&group];
            state.next_token += 1;
            let number = state.next_token;
            let slot = Slot {
                row: group.clone(),
                scope: Some(number),
            };
            let view = inner
                .spawn_row(
                    state,
                    index,
                    &parent,
                    parent_token,
                    &parent_ctx,
                    slot.clone(),
                )
                .ok_or_else(|| {
                    state.rejected.remove(&slot).unwrap_or_else(|| {
                        LoaderError::InvalidEntry(format!("{group:?} cannot start"))
                    })
                })?;
            state.instances.insert(
                number,
                InstanceRecord {
                    group: group.clone(),
                    parent,
                    values: Arc::new(values),
                    plugin: view.id,
                    kernel: view.instance(),
                },
            );
            (number, view)
        };
        inner.emit(LoaderChanged::InstanceCreated {
            group: group.clone(),
            plugin: view.id,
        });
        inner.settle(Some(number)).await;
        if view.state().state == FiberState::Failed {
            let error = view
                .state()
                .error
                .map(|error| LoaderError::Rejected { id: group, error })
                .unwrap_or(LoaderError::Closed);
            let _ = self.remove_instance(view.id).await;
            return Err(error);
        }
        let report = report(&inner.state.lock().unwrap(), number);
        Ok(Instance {
            plugin: view.id,
            view,
            report,
        })
    }

    /// Remove the instance whose group fiber is `plugin`, with everything
    /// inside it. Closing the fiber some other way (`FiberView::shutdown`)
    /// removes the instance too.
    pub async fn remove_instance(&self, plugin: PluginId) -> Result<(), LoaderError> {
        let (view, disposals, events) = {
            let mut state = self.inner.state.lock().unwrap();
            let state = &mut *state;
            let number = state
                .instances
                .iter()
                .find(|(_, instance)| instance.plugin == plugin)
                .map(|(n, _)| *n)
                .ok_or_else(|| LoaderError::UnknownEntry(format!("instance {plugin:?}")))?;
            let removed = state.instances.remove(&number).unwrap();
            let slot = Slot {
                row: removed.group.clone(),
                scope: Some(number),
            };
            let view = state.running.remove(&slot).map(|running| {
                let key = Some(slot);
                if state.groups.get(&key).map(|g| g.token) == Some(running.token) {
                    state.groups.remove(&key);
                }
                Inner::forget(state, key, running.token);
                running.view
            });
            state.stopped.retain(|s, _| s.scope != Some(number));
            state.rejected.retain(|s, _| s.scope != Some(number));
            let (disposals, mut events) = Inner::prune_instances(state);
            events.insert(
                0,
                LoaderChanged::InstanceRemoved {
                    group: removed.group,
                    plugin: removed.plugin,
                },
            );
            (view, disposals, events)
        };
        if let Some(view) = view {
            let _ = view.dispose().await;
        }
        for disposal in disposals {
            disposal.await;
        }
        for event in events {
            self.inner.emit(event);
        }
        Ok(())
    }

    /// Restart one copy: the fiber `plugin`, or a copy inside an instance
    /// that disposed itself and had that fiber.
    pub async fn restart_instance(&self, plugin: PluginId) -> Result<(), LoaderError> {
        let unknown = || LoaderError::UnknownEntry(format!("plugin {plugin:?}"));
        let found = {
            let mut state = self.inner.state.lock().unwrap();
            let state = &mut *state;
            if let Some((slot, running)) = state.running.iter().find(|(_, r)| r.view.id == plugin) {
                Ok((slot.row.clone(), running.view.clone()))
            } else {
                let slot = state
                    .stopped
                    .iter()
                    .find(|(_, &old)| old == plugin)
                    .map(|(slot, _)| slot.clone())
                    .ok_or_else(unknown)?;
                state.stopped.remove(&slot);
                let row = state.desired.row(&slot.row).ok_or_else(unknown)?;
                let parent = row.parent.clone().map(|row| Slot {
                    row,
                    scope: slot.scope,
                });
                self.inner.spawn_children(state, &parent);
                Err(slot)
            }
        };
        match found {
            Ok((id, view)) => view
                .restart()
                .await
                .map_err(|error| LoaderError::Rejected { id, error }),
            Err(slot) => {
                self.inner.settle(slot.scope).await;
                Ok(())
            }
        }
    }
}

/// The group context the new instance goes in: walk up from `ctx`'s
/// plugin through the loader's records to a group of row `parent`.
fn parent_of(state: &State, ctx: &Ctx, parent: Option<&str>) -> Result<Option<Slot>, LoaderError> {
    let mut current = state
        .running
        .iter()
        .find(|(_, r)| r.view.instance() == ctx.instance())
        .map(|(slot, _)| slot.clone());
    loop {
        match current {
            None if parent.is_none() => return Ok(None),
            None => {
                return Err(LoaderError::InvalidEntry(format!(
                    "the context is not inside a running {:?}",
                    parent.unwrap_or_default()
                )))
            }
            Some(slot) => {
                let running = &state.running[&slot];
                if running.group && Some(slot.row.as_str()) == parent {
                    return Ok(Some(slot));
                }
                current = running.parent.clone();
            }
        }
    }
}

/// Each row inside instance `number`, in tree order.
fn report(state: &State, number: u64) -> Vec<(String, InstanceResult)> {
    let Some(instance) = state.instances.get(&number) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in &state.desired.rows {
        let Some(group) = state.desired.instanced_group(row) else {
            continue;
        };
        if group.id != instance.group || row.id == instance.group {
            continue;
        }
        let slot = Slot {
            row: row.id.clone(),
            scope: Some(number),
        };
        let result = if let Some(invalid) = &row.invalid {
            InstanceResult::Failed(invalid.clone())
        } else if let Err(error) = &row.disabled {
            InstanceResult::Failed(error.clone())
        } else if !state.desired.wanted(row) {
            InstanceResult::Skipped
        } else if let Some(rejected) = state.rejected.get(&slot) {
            InstanceResult::Failed(rejected.clone())
        } else if let Some(running) = state.running.get(&slot) {
            let snapshot = running.view.state();
            match snapshot.state {
                FiberState::Active => InstanceResult::Active,
                FiberState::Failed => InstanceResult::Failed(LoaderError::Rejected {
                    id: row.id.clone(),
                    error: snapshot
                        .error
                        .unwrap_or_else(|| Arc::new(CordisError::PluginFailed("failed".into()))),
                }),
                _ => InstanceResult::Waiting,
            }
        } else if let Some(Err(error)) = row.name.as_ref().and_then(|n| state.resolved.get(n)) {
            InstanceResult::Failed(error.clone())
        } else {
            InstanceResult::Skipped
        };
        out.push((row.id.clone(), result));
    }
    out
}
