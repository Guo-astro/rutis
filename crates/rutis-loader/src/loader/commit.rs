//! Imperative edits: rewrite the editable layer, dry run, reconcile, roll
//! back, and persist through the pending queue.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use rutis::{CordisError, Ctx};

use crate::edit::{apply_edit, Edit};
use crate::error::Failure;
use crate::patch::Layer;
use crate::resolver::Build;
use crate::{LoaderError, PersistError};

use super::desired::{Desired, Row};
use super::{Failing, Inner, LoaderChanged, PendingEditDropped, Slot, State};

/// Where one copy would run: its instance (`None` outside instances), its
/// context without its own scope, and the instances enclosing it.
type Target = (Option<u64>, Option<Ctx>, Build);

/// Whether `row` is inside the group `top`.
fn below(desired: &Desired, row: &Row, top: &str) -> bool {
    let mut parent = row.parent.as_deref();
    while let Some(id) = parent {
        if id == top {
            return true;
        }
        parent = desired.row(id).and_then(|r| r.parent.as_deref());
    }
    false
}

/// The instances a row's copies run in under `desired`: `None` outside
/// instanced groups, else each instance of its instanced group.
fn copies(state: &State, desired: &Desired, row: &Row) -> Vec<Option<u64>> {
    let Some(group) = desired.instanced_group(row) else {
        return vec![None];
    };
    let mut numbers: Vec<u64> = state
        .instances
        .iter()
        .filter(|(_, instance)| instance.group == group.id)
        .map(|(n, _)| *n)
        .collect();
    numbers.sort_unstable();
    numbers.into_iter().map(Some).collect()
}

/// The context the copy of `leaf` in `scope` would run in, without its own
/// scope, when `top` (the leaf or a group above it) is applied afresh: the
/// running context above `top`, with the scope of `top` and of each group
/// between them applied.
fn dry_context(
    state: &State,
    desired: &Desired,
    leaf: &Row,
    top: &str,
    scope: Option<u64>,
    root: Option<&Ctx>,
) -> Result<Option<Ctx>, LoaderError> {
    let mut groups: Vec<&Row> = Vec::new();
    let mut current = scope;
    let mut row = leaf;
    let anchor = loop {
        let parent = if row.instanced {
            match current.and_then(|n| state.instances.get(&n)) {
                Some(instance) => instance.parent.clone(),
                None => return Ok(None),
            }
        } else {
            row.parent.clone().map(|id| Slot {
                row: id,
                scope: current,
            })
        };
        if row.id == top {
            break parent;
        }
        let Some(parent) = parent else {
            break None;
        };
        let Some(next) = desired.row(&parent.row) else {
            return Ok(None);
        };
        current = parent.scope;
        row = next;
        groups.push(row);
    };
    let base = match state.groups.get(&anchor) {
        Some(group) => group.ctx.clone(),
        None if anchor.as_ref().is_none_or(|a| a.scope.is_none()) => match root {
            Some(root) => root.clone(),
            None => return Ok(None),
        },
        None => return Ok(None),
    };
    let mut ctx = base;
    for group in groups.into_iter().rev() {
        ctx = group.scope.as_ref().map_err(Clone::clone)?.context(&ctx);
    }
    Ok(Some(ctx))
}

/// How many times a version conflict is resolved by replaying the pending
/// queue before giving up with [`LoaderError::Conflict`].
const CONFLICT_RETRIES: usize = 3;

impl Inner {
    /// Check a row would start: resolve, validate the config, build and
    /// validate the instance, for every copy (each instance it runs in).
    /// For a group, check every copy of every plugin below it the same way,
    /// in the contexts the group would give them; an error that copy (same
    /// row and instance) already fails with is left to reconcile. Nothing
    /// is spawned. With `resolved`, check that module
    /// instead of resolving the name.
    pub(super) async fn dry_run(
        &self,
        layers: &[Layer],
        id: &str,
        resolved: Option<Arc<crate::resolver::Resolved>>,
    ) -> Result<(), LoaderError> {
        // The plugins to check, and where each copy would run: its context
        // without the plugin's own scope, and the instances enclosing it.
        let (desired, checks, failing) = {
            let state = self.state.lock().unwrap();
            let root = state.groups.get(&None).map(|g| g.ctx.clone());
            let desired = self.build_desired(layers, root.as_ref());
            let Some(row) = desired.row(id) else {
                return Ok(());
            };
            if let Some(invalid) = &row.invalid {
                return Err(invalid.clone());
            }
            if let Err(e) = &row.disabled {
                return Err(e.clone());
            }
            if row.group {
                row.scope.as_ref().map_err(Clone::clone)?;
            }
            if !desired.wanted(row) {
                return Ok(());
            }
            let failing: Vec<(Option<u64>, Failure)> = Self::failures(&state)
                .into_iter()
                .map(|f| (f.scope, f.failure))
                .collect();
            let mut checks: Vec<(usize, Vec<Target>)> = Vec::new();
            for (index, leaf) in desired.rows.iter().enumerate() {
                if leaf.group
                    || !desired.wanted(leaf)
                    || (leaf.id != id && !below(&desired, leaf, id))
                {
                    continue;
                }
                let mut targets = Vec::new();
                for scope in copies(&state, &desired, leaf) {
                    let base = dry_context(&state, &desired, leaf, id, scope, root.as_ref())?;
                    targets.push((scope, base, Self::build_for(&state, scope)));
                }
                checks.push((index, targets));
            }
            (desired, checks, failing)
        };
        for (index, targets) in checks {
            let row = &desired.rows[index];
            // The edited row itself must start; a copy of a plugin below a
            // group only must not fail in a new way.
            let tolerated = |scope: Option<u64>, error: &LoaderError| {
                let failure = Failure {
                    id: row.id.clone(),
                    error: error.to_string(),
                };
                row.id != id && failing.contains(&(scope, failure))
            };
            let name = row.name.clone().unwrap_or_default();
            let module = match (&resolved, row.id == id) {
                (Some(resolved), true) => Ok(resolved.clone()),
                (_, true) => self.resolver.resolve(&name).await,
                (_, false) => {
                    let cached = self.state.lock().unwrap().resolved.get(&name).cloned();
                    match cached {
                        Some(result) => result,
                        None => self.resolver.resolve(&name).await,
                    }
                }
            };
            match module {
                Ok(module) => self.dry_run_copies(row, &module, targets, &tolerated)?,
                Err(error) if targets.iter().all(|(scope, ..)| tolerated(*scope, &error)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Check each copy of the plugin row `row` would start in its context.
    /// A copy failing with an error `tolerated` accepts for it does not
    /// count.
    fn dry_run_copies(
        &self,
        row: &Row,
        resolved: &crate::resolver::Resolved,
        targets: Vec<Target>,
        tolerated: &dyn Fn(Option<u64>, &LoaderError) -> bool,
    ) -> Result<(), LoaderError> {
        let rejected = |error: CordisError| LoaderError::Rejected {
            id: row.id.clone(),
            error: Arc::new(error),
        };
        for (copy, base, build) in targets {
            let check = |result: Result<(), LoaderError>| match result {
                Err(error) if !tolerated(copy, &error) => Err(error),
                _ => Ok(()),
            };
            // Evaluate where the plugin would run: its group, with its
            // isolates.
            let scope = if resolved.foreign_scope {
                super::desired::RowScope::default()
            } else {
                match &row.scope {
                    Ok(scope) => scope.clone(),
                    Err(error) => {
                        check(Err(error.clone()))?;
                        continue;
                    }
                }
            };
            let ctx = base.map(|ctx| scope.context(&ctx));
            let config = match self.eval().value(&row.config, ctx.as_ref()) {
                Ok(config) => config,
                Err(error) => {
                    check(Err(error))?;
                    continue;
                }
            };
            let checked = catch_unwind(AssertUnwindSafe(|| {
                let factory = match &resolved.scoped {
                    Some(scoped) => scoped(&build)?,
                    None => resolved.factory.clone(),
                };
                factory.validate_config(&config)?;
                factory.build(&config)?.validate()
            }));
            let result = match checked {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(rejected(error)),
                Err(_) => Err(rejected(CordisError::PluginFailed(
                    "panicked during the dry run".into(),
                ))),
            };
            check(result)?;
        }
        Ok(())
    }

    /// Apply one edit in memory: rewrite the editable layer, dry-run,
    /// reconcile, and roll back if rows newly fail. Nothing is persisted.
    pub(super) async fn commit(self: &Arc<Self>, edit: &Edit) -> Result<(), LoaderError> {
        let (layers, overlays, editable, before) = {
            let state = self.state.lock().unwrap();
            let editable = state.editable.ok_or(LoaderError::NoEditableLayer)?;
            (
                state.layers.clone(),
                state.overlays.clone(),
                editable,
                Self::failures(&state),
            )
        };
        // Overlays sit above the editable layer: their rows are not owned.
        let composed: Vec<Layer> = layers.iter().chain(&overlays).cloned().collect();
        let patches = apply_edit(&composed, editable, edit)?;
        let mut next = layers.clone();
        next[editable].patches = patches;
        if !matches!(
            edit,
            Edit::Remove { .. } | Edit::SetDisabled { disabled: true, .. }
        ) {
            if let Some(id) = edit.id() {
                let next_composed: Vec<Layer> = next.iter().chain(&overlays).cloned().collect();
                self.dry_run(&next_composed, id, None).await?;
            }
        }
        {
            let mut state = self.state.lock().unwrap();
            // A rename changes the module: resolve it afresh.
            if let Edit::Rename { name, .. } = edit {
                state.resolved.remove(name);
            }
            // A change to a row starts its stopped copies again.
            if let Some(id) = edit.id() {
                state.stopped.retain(|slot, _| slot.row != id);
            }
            state.layers = next;
        }
        let report = self.reconcile_inner().await;
        if report.new_failures.is_empty() {
            return Ok(());
        }
        self.state.lock().unwrap().layers = layers;
        self.reconcile_inner().await;
        // Compare with the state before the edit, not before the rollback.
        let rollback: Vec<Failure> = {
            let state = self.state.lock().unwrap();
            Failing::public(
                Self::failures(&state)
                    .iter()
                    .filter(|f| !before.contains(f)),
            )
        };
        if rollback.is_empty() {
            Err(LoaderError::ApplyFailed {
                failures: report.new_failures,
            })
        } else {
            Err(LoaderError::RollbackFailed {
                apply: report.new_failures,
                rollback,
            })
        }
    }

    /// Persist the pending queue; on a version conflict, reload the layer,
    /// replay the queue on it and try again. `current` is the queue index
    /// of the edit the caller is waiting for.
    pub(super) async fn persist_queue(
        self: &Arc<Self>,
        mut current: Option<usize>,
    ) -> Result<(), LoaderError> {
        let mut current_error: Option<LoaderError> = None;
        let finish = |error: Option<LoaderError>| error.map_or(Ok(()), Err);
        for attempt in 0..=CONFLICT_RETRIES {
            let (layer, version, edits, patches) = {
                let state = self.state.lock().unwrap();
                let Some(editable) = state.editable else {
                    return finish(current_error);
                };
                if state.pending.is_empty() {
                    return finish(current_error);
                }
                (
                    state.layers[editable].name.clone(),
                    state.version.clone(),
                    state.pending.clone(),
                    state.layers[editable].patches.clone(),
                )
            };
            match self.persist.save(&layer, &version, &edits, &patches).await {
                Ok(version) => {
                    let mut state = self.state.lock().unwrap();
                    state.version = version;
                    state.pending.clear();
                    return finish(current_error);
                }
                Err(PersistError::Failed(message)) => {
                    return Err(current_error.unwrap_or(LoaderError::PersistFailed(message)));
                }
                Err(PersistError::Conflict) if attempt == CONFLICT_RETRIES => {
                    return Err(current_error.unwrap_or(LoaderError::Conflict));
                }
                Err(PersistError::Conflict) => {
                    let (latest, version) = self
                        .persist
                        .load(&layer)
                        .await
                        .map_err(|e| LoaderError::PersistFailed(e.to_string()))?;
                    let queue = {
                        let mut state = self.state.lock().unwrap();
                        let editable = state.editable.unwrap();
                        state.layers[editable].patches = latest;
                        state.version = version;
                        std::mem::take(&mut state.pending)
                    };
                    self.reconcile_inner().await;
                    let mut replaced = None;
                    for (index, edit) in queue.into_iter().enumerate() {
                        match self.commit(&edit).await {
                            Ok(()) => {
                                let mut state = self.state.lock().unwrap();
                                if current == Some(index) {
                                    replaced = Some(state.pending.len());
                                }
                                state.pending.push(edit);
                            }
                            Err(error) if current == Some(index) => current_error = Some(error),
                            Err(error) => self.emit(PendingEditDropped { edit, error }),
                        }
                    }
                    current = replaced;
                }
            }
        }
        finish(current_error)
    }

    pub(super) async fn edit(self: &Arc<Self>, edit: Edit) -> Result<(), LoaderError> {
        let _op = self.op.lock().await;
        self.check_open()?;
        self.commit(&edit).await?;
        let index = {
            let mut state = self.state.lock().unwrap();
            state.pending.push(edit.clone());
            state.pending.len() - 1
        };
        let result = self.persist_queue(Some(index)).await;
        self.emit(LoaderChanged::Edited(edit));
        result
    }
}
