//! A check of rows before they run: each resolves, passes the dry run, and
//! finds what it waits for in rutis provided by another row, a link's
//! imports or the application.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::patch::Layer;
use crate::resolver::{Declared, Resolved};
use crate::LoaderError;

use super::Loader;

/// What [`Loader::check`] found, row by row in tree order (plugin rows
/// only: a group runs nothing itself).
#[derive(Debug)]
#[non_exhaustive]
pub struct CheckReport {
    pub rows: Vec<RowCheck>,
    /// What composing the layers skipped (rows without an id, duplicate
    /// ids) and its warnings.
    pub issues: Vec<String>,
}

impl CheckReport {
    /// The rows that cannot run: they fail, or wait for a service nothing
    /// provides.
    pub fn failing(&self) -> impl Iterator<Item = &RowCheck> {
        self.rows.iter().filter(|row| match &row.status {
            RowStatus::Failed(_) => true,
            RowStatus::Checked { requires, .. } => requires
                .iter()
                .any(|requirement| matches!(requirement.supply, Supply::Missing { .. })),
            RowStatus::Disabled => false,
        })
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct RowCheck {
    pub id: String,
    /// The module the row names.
    pub name: String,
    pub status: RowStatus,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum RowStatus {
    /// Disabled, itself or by a group above it: not checked.
    Disabled,
    /// It cannot start: the row is not valid, its module does not resolve,
    /// or its config or scope fails the dry run.
    Failed(LoaderError),
    /// It starts once each service in `requires` is there.
    Checked {
        resolved: Arc<Resolved>,
        /// What the plugin declares, or why that is not known
        /// ([`Resolved::declares`]).
        declared: Result<Declared, String>,
        /// What the row waits for in rutis (its plugin's and its own
        /// `inject`), by name, and who provides each.
        requires: Vec<Requirement>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Requirement {
    pub service: String,
    pub supply: Supply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Supply {
    /// These provide it (which one a copy uses, in which scope, is not
    /// checked).
    Provided(Vec<Provider>),
    /// Nothing provides it, and every row says what it provides: the row
    /// would wait for ever. `disabled` are disabled rows that provide it.
    Missing { disabled: Vec<String> },
    /// Nothing known provides it, but these rows may: what they provide is
    /// not known yet.
    Unknown(Vec<Unknown>),
    /// No row declares it, but these rows may provide it once they run:
    /// they provide services they do not declare ([`Declared::announces`],
    /// a published Cordis plugin, say). A Cordis row's dependency on
    /// another row of its runtime resolves in Cordis.
    Announced(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Provider {
    /// The row with this id (a `rutis-bridge/peer` row for what it imports).
    Row(String),
    /// The application, under the name it registered in the catalog.
    Host,
}

/// A row whose declarations are not known, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown {
    pub row: String,
    pub why: String,
}

impl Loader {
    /// Check `layers` without running anything of them (nor changing what
    /// runs): every plugin row resolves and passes the dry run, and every
    /// service it waits for in rutis is provided by another row (as its
    /// plugin declares; a `rutis-bridge/peer` row provides what it imports)
    /// or by the application (a name it registered in the catalog).
    ///
    /// Resolving asks runtimes to describe their plugins, which imports
    /// them there. What a copy uses inside isolated scopes and instances is
    /// not checked: a provider anywhere counts.
    pub async fn check(&self, layers: &[Layer]) -> CheckReport {
        let inner = &self.inner;
        let desired = {
            let state = inner.state.lock().unwrap();
            let root = state.groups.get(&None).map(|g| g.ctx.clone());
            inner.build_desired(layers, root.as_ref())
        };
        let mut issues = desired.issues.clone();
        issues.extend(desired.warnings.iter().map(|warning| {
            format!(
                "layer {:?}, patch {}: {}",
                warning.layer, warning.index, warning.message
            )
        }));

        // Each plugin row, and what each declares.
        let mut rows = Vec::new();
        let mut declared_by: Vec<(String, bool, Result<Declared, String>)> = Vec::new();
        for row in desired.rows.iter().filter(|row| !row.group) {
            let name = row.name.clone().unwrap_or_default();
            let wanted = desired.wanted(row);
            let status = if let Some(invalid) = &row.invalid {
                RowStatus::Failed(invalid.clone())
            } else if let Err(error) = &row.disabled {
                RowStatus::Failed(error.clone())
            } else {
                match inner.resolver.resolve(&name).await {
                    Err(error) if wanted => RowStatus::Failed(error),
                    Err(_) => RowStatus::Disabled,
                    Ok(resolved) => {
                        let declared = match &resolved.declares {
                            Some(declares) => declares(&row.config),
                            None => Err("its plugin does not say".to_owned()),
                        };
                        declared_by.push((row.id.clone(), wanted, declared.clone()));
                        if !wanted {
                            RowStatus::Disabled
                        } else {
                            match inner.dry_run(layers, &row.id, Some(resolved.clone())).await {
                                Err(error) => RowStatus::Failed(error),
                                Ok(()) => RowStatus::Checked {
                                    resolved,
                                    declared,
                                    requires: Vec::new(),
                                },
                            }
                        }
                    }
                }
            };
            rows.push(RowCheck {
                id: row.id.clone(),
                name,
                status,
            });
        }

        // Who provides each name: rows that run, rows that are disabled,
        // and rows whose declarations are not known.
        let mut providers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut disabled: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut unknown: Vec<Unknown> = Vec::new();
        let mut announcing: Vec<&str> = Vec::new();
        let starts = |id: &str| {
            rows.iter()
                .any(|row| row.id == id && matches!(row.status, RowStatus::Checked { .. }))
        };
        for (id, wanted, declared) in &declared_by {
            match declared {
                Ok(declared) => {
                    let into = match (wanted, starts(id)) {
                        (true, true) => &mut providers,
                        (true, false) => continue,
                        (false, _) => &mut disabled,
                    };
                    for name in &declared.provides {
                        into.entry(name).or_default().push(id);
                    }
                    if declared.announces && *wanted {
                        announcing.push(id);
                    }
                }
                Err(why) if *wanted && starts(id) => unknown.push(Unknown {
                    row: id.clone(),
                    why: why.clone(),
                }),
                Err(_) => {}
            }
        }

        let catalog = &inner.catalog;
        let supplies: Vec<Vec<Requirement>> = rows
            .iter()
            .map(|check| {
                let RowStatus::Checked {
                    resolved, declared, ..
                } = &check.status
                else {
                    return Vec::new();
                };
                let row = desired.row(&check.id).expect("a row of the tree");
                let mut names: Vec<&str> = declared
                    .as_ref()
                    .map(|declared| declared.requires.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                // The row's own `inject`: the loader gates it on each name,
                // unless its plugin handles them itself, where it waits in
                // rutis only for the names rutis shares (the others its
                // runtime resolves natively).
                names.extend(
                    row.raw_scope
                        .inject
                        .iter()
                        .filter(|name| !resolved.foreign_scope || catalog.shares(name))
                        .map(String::as_str),
                );
                names.sort_unstable();
                names.dedup();
                names
                    .into_iter()
                    .map(|service| {
                        let mut by: Vec<Provider> = providers
                            .get(service)
                            .into_iter()
                            .flatten()
                            .filter(|id| **id != check.id)
                            .map(|id| Provider::Row((*id).to_owned()))
                            .collect();
                        if catalog.registered_by_host(service) {
                            by.push(Provider::Host);
                        }
                        let others: Vec<Unknown> = unknown
                            .iter()
                            .filter(|source| source.row != check.id)
                            .cloned()
                            .collect();
                        let announced: Vec<String> = announcing
                            .iter()
                            .filter(|id| **id != check.id)
                            .map(|id| (*id).to_owned())
                            .collect();
                        let supply = match (by.is_empty(), others.is_empty()) {
                            (false, _) => Supply::Provided(by),
                            (true, _) if !announced.is_empty() => Supply::Announced(announced),
                            (true, false) => Supply::Unknown(others),
                            (true, true) => Supply::Missing {
                                disabled: disabled
                                    .get(service)
                                    .into_iter()
                                    .flatten()
                                    .map(|id| (*id).to_owned())
                                    .collect(),
                            },
                        };
                        Requirement {
                            service: service.to_owned(),
                            supply,
                        }
                    })
                    .collect()
            })
            .collect();
        for (check, supplied) in rows.iter_mut().zip(supplies) {
            if let RowStatus::Checked { requires, .. } = &mut check.status {
                *requires = supplied;
            }
        }
        CheckReport { rows, issues }
    }
}
