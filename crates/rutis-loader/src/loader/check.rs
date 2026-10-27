//! A check of rows before they run: each resolves, passes the dry run, and
//! finds what it waits for in rutis provided by another row, a link's
//! imports or the application.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::patch::Layer;
use crate::resolver::{Declared, Resolved};
use crate::LoaderError;
use rutis::ConfigIssue;

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
    /// Rows that wait for each other's services (their `inject` makes a
    /// cycle): none of them starts. Each cycle as its links, in order.
    pub cycles: Vec<Vec<CycleLink>>,
    /// Services that several rows that start declare in one scope: only
    /// one of them can provide it, the others fail.
    pub clashes: Vec<Clash>,
}

/// One link of a cycle ([`CheckReport::cycles`]): `row` waits for
/// `service`, which `provider` provides.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CycleLink {
    pub row: String,
    pub service: String,
    pub provider: String,
}

/// A service several rows declare in one scope ([`CheckReport::clashes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Clash {
    pub service: String,
    /// In tree order.
    pub rows: Vec<String>,
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
    /// Extra keys in its config where its plugin's schema allows them
    /// (sorted by path): warnings, whether the row can run or not.
    pub config_warnings: Vec<ConfigIssue>,
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
        /// How its config was checked against its plugin's schema (a
        /// config that does not match fails the row instead).
        config: ConfigStatus,
    },
}

/// How a row's config was checked ([`RowStatus::Checked`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigStatus {
    /// It matches its plugin's schema (its extra keys:
    /// [`RowCheck::config_warnings`]).
    Checked,
    /// Its plugin has no schema.
    NoSchema,
    /// Its schema is known only when it runs (a remote runtime not
    /// connected, a peer that is not there): not checked.
    NotYet,
    /// A Cordis row: Cordis validates its config with its own `Config`.
    Cordis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Requirement {
    pub service: String,
    pub supply: Supply,
    /// When it is not known to be provided: a name close to it that a row
    /// declares or the application registered (a misspelt name).
    pub like: Option<String>,
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
    /// Only the disabled rows `disabled` declare it; the rows `announced`,
    /// which do not say all they provide, may provide it once they run.
    OnlyDisabled {
        disabled: Vec<String>,
        announced: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Provider {
    /// The row with this id (a `rutis-bridge/peer` row for what it imports).
    Row(String),
    /// The application, under the name it registered in the catalog.
    Host,
    /// A `#[rutis::service]` type of the application (its type name), which
    /// serves the name once it is provided: whether it is, is not known
    /// before the rows run.
    Marked(String),
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
            let mut warnings = Vec::new();
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
                            let dry = inner
                                .dry_run_checking(
                                    layers,
                                    &row.id,
                                    Some(resolved.clone()),
                                    Some(&mut warnings),
                                )
                                .await;
                            match dry {
                                Err(error) => RowStatus::Failed(error),
                                Ok(()) => {
                                    let config = match super::config_step::checkable(&resolved) {
                                        super::config_step::Checkable::Yes => ConfigStatus::Checked,
                                        super::config_step::Checkable::NoSchema => {
                                            ConfigStatus::NoSchema
                                        }
                                        super::config_step::Checkable::NotYet => {
                                            ConfigStatus::NotYet
                                        }
                                        super::config_step::Checkable::Cordis => {
                                            ConfigStatus::Cordis
                                        }
                                    };
                                    RowStatus::Checked {
                                        resolved,
                                        declared,
                                        requires: Vec::new(),
                                        config,
                                    }
                                }
                            }
                        }
                    }
                }
            };
            warnings.sort_by(|a, b| a.path.cmp(&b.path));
            rows.push(RowCheck {
                id: row.id.clone(),
                name,
                status,
                config_warnings: warnings,
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
        // Every name a row declares or the application registered, for a
        // misspelt one.
        let mut known: Vec<String> = providers
            .keys()
            .chain(disabled.keys())
            .map(|name| (*name).to_owned())
            .chain(catalog.names())
            .collect();
        known.sort_unstable();
        known.dedup();
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
                        } else if let Some(type_name) = catalog.marked_by_host(service) {
                            by.push(Provider::Marked(type_name.to_owned()));
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
                        let disabled: Vec<String> = disabled
                            .get(service)
                            .into_iter()
                            .flatten()
                            .map(|id| (*id).to_owned())
                            .collect();
                        let supply = match (by.is_empty(), others.is_empty()) {
                            (false, _) => Supply::Provided(by),
                            // Declared by disabled rows only: those are
                            // named, not the rows that declare nothing.
                            (true, _) if !announced.is_empty() && !disabled.is_empty() => {
                                Supply::OnlyDisabled {
                                    disabled,
                                    announced,
                                }
                            }
                            (true, _) if !announced.is_empty() => Supply::Announced(announced),
                            (true, false) => Supply::Unknown(others),
                            (true, true) => Supply::Missing { disabled },
                        };
                        let like = match supply {
                            Supply::Provided(_) => None,
                            _ => super::waits::closest(service, &known),
                        };
                        Requirement {
                            service: service.to_owned(),
                            supply,
                            like,
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

        // Several rows that start declaring one name in one scope; a row
        // waiting for a service nothing provides never starts.
        // (service, isolation label, instanced group) → rows.
        type Scope<'a> = (&'a str, Option<String>, Option<&'a str>);
        let mut declared_in: BTreeMap<Scope, Vec<&str>> = BTreeMap::new();
        for (id, wanted, declared) in &declared_by {
            let starts = rows.iter().any(|row| {
                &row.id == id
                    && matches!(&row.status, RowStatus::Checked { requires, .. }
                    if !requires.iter().any(|requirement| {
                        matches!(requirement.supply, Supply::Missing { .. })
                    }))
            });
            let (true, true, Ok(declared)) = (*wanted, starts, declared) else {
                continue;
            };
            let row = desired.row(id).expect("a row of the tree");
            let group = desired.instanced_group(row).map(|group| group.id.as_str());
            for name in &declared.provides {
                declared_in
                    .entry((name, isolated_in(&desired, row, name), group))
                    .or_default()
                    .push(id);
            }
        }
        let clashes = declared_in
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|((service, ..), ids)| Clash {
                service: service.to_owned(),
                rows: ids.into_iter().map(str::to_owned).collect(),
            })
            .collect();

        let cycles = cycles(&rows);
        CheckReport {
            rows,
            issues,
            cycles,
            clashes,
        }
    }
}

/// The scope `row` provides `name` in: the label of the nearest `isolate`
/// of it, on the row or a group around it; `None` outside any.
fn isolated_in(
    desired: &super::desired::Desired,
    row: &super::desired::Row,
    name: &str,
) -> Option<String> {
    let mut current = Some(row);
    while let Some(row) = current {
        if let Some((_, label)) = row
            .raw_scope
            .isolate
            .iter()
            .find(|(isolated, _)| isolated == name)
        {
            return Some(label.clone());
        }
        current = row.parent.as_deref().and_then(|id| desired.row(id));
    }
    None
}

/// The cycles of rows waiting for each other: a row waits for a row when
/// it requires a service only rows provide, that one among them. One cycle
/// per set of rows that reach each other, from its first row in tree order.
fn cycles(rows: &[RowCheck]) -> Vec<Vec<CycleLink>> {
    // row → (service, provider), in tree order.
    let mut edges: BTreeMap<usize, Vec<(String, usize)>> = BTreeMap::new();
    let index = |id: &str| rows.iter().position(|row| row.id == id);
    for (from, row) in rows.iter().enumerate() {
        let RowStatus::Checked { requires, .. } = &row.status else {
            continue;
        };
        for requirement in requires {
            let Supply::Provided(by) = &requirement.supply else {
                continue;
            };
            // Only a service one row alone provides: with another provider
            // the row may start from that one.
            let [Provider::Row(id)] = by.as_slice() else {
                continue;
            };
            if let Some(to) = index(id) {
                edges
                    .entry(from)
                    .or_default()
                    .push((requirement.service.clone(), to));
            }
        }
    }
    // Which rows each row reaches.
    let reach = |start: usize| -> Vec<bool> {
        let mut seen = vec![false; rows.len()];
        let mut stack = vec![start];
        while let Some(at) = stack.pop() {
            for (_, to) in edges.get(&at).into_iter().flatten() {
                if !seen[*to] {
                    seen[*to] = true;
                    stack.push(*to);
                }
            }
        }
        seen
    };
    let reached: Vec<Vec<bool>> = (0..rows.len()).map(reach).collect();
    let mut placed = vec![false; rows.len()];
    let mut out = Vec::new();
    for start in 0..rows.len() {
        if placed[start] || !reached[start][start] {
            continue;
        }
        // The rows of its cycle set: those it reaches that reach it.
        let members: Vec<bool> = (0..rows.len())
            .map(|other| reached[start][other] && reached[other][start])
            .collect();
        for (other, member) in members.iter().enumerate() {
            if *member {
                placed[other] = true;
            }
        }
        // A path back to `start` within the set, shortest first.
        let mut back: Vec<Option<(usize, String)>> = vec![None; rows.len()];
        let mut queue = std::collections::VecDeque::from([start]);
        let mut found = false;
        'search: while let Some(at) = queue.pop_front() {
            for (service, to) in edges.get(&at).into_iter().flatten() {
                if !members[*to] {
                    continue;
                }
                if *to == start {
                    back[start] = Some((at, service.clone()));
                    found = true;
                    break 'search;
                }
                if back[*to].is_none() {
                    back[*to] = Some((at, service.clone()));
                    queue.push_back(*to);
                }
            }
        }
        if !found {
            continue;
        }
        let mut links = Vec::new();
        let mut to = start;
        while let Some((from, service)) = back[to].take() {
            links.push(CycleLink {
                row: rows[from].id.clone(),
                service,
                provider: rows[to].id.clone(),
            });
            to = from;
            if to == start {
                break;
            }
        }
        links.reverse();
        out.push(links);
    }
    out
}
