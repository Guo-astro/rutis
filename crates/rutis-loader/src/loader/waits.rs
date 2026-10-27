//! What a waiting row waits for, and why each is not there
//! ([`Loader::waits`]): what rutis-host's `waiting for …` lines say.

use std::collections::HashMap;
use std::sync::Arc;

use rutis::host::diagnostics::{DependencyStatus, PluginDiagnostics};
use rutis::host::{CtxHost, FiberState, PluginId, TypeKey};

use super::{EntryInfo, EntryStatus, Inner, Loader, Slot, State};
use crate::resolver::Resolved;

/// One service a waiting copy waits for ([`Loader::waits`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Wait {
    /// The service by name; a key that has none (a Rust type), described.
    pub service: String,
    pub reason: WaitReason,
}

/// Why a service a copy waits for is not there.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WaitReason {
    /// It is the copy's own runtime, `service` its name: the runtime has
    /// not released its rows (it is starting, or down).
    OwnRuntime,
    /// No row declares it. `like` is a service name close to it that a
    /// row declares or something provides (a misspelt name).
    NoProvider { like: Option<String> },
    /// No enabled row declares it, but rows that are disabled (themselves
    /// or by a group) were never resolved, so what they provide is not
    /// known: one of them may (`Loader::check` resolves them). `like` as
    /// for [`WaitReason::NoProvider`].
    NoEnabledProvider { like: Option<String> },
    /// rutis knows why, in a sentence (a Rust service of that name whose
    /// type is not marked `#[rutis::service]`).
    Explained(String),
    /// The row that provides it is disabled.
    ProviderDisabled { row: String },
    /// The row that provides it cannot run: it failed, or does not resolve.
    ProviderFailed { row: String, error: String },
    /// The row that provides it does not run: a group around it is not
    /// running, or it stopped in its instance.
    ProviderInactive { row: String },
    /// The row that provides it waits for its own runtime, `runtime`.
    ProviderRuntime { row: String, runtime: String },
    /// The row that provides it waits for `service`, which the waiting
    /// copy's row provides: they inject each other.
    Cycle { row: String, service: String },
    /// The row that provides it waits too, for `services`.
    ProviderWaiting { row: String, services: Vec<String> },
    /// The row that provides it is starting.
    ProviderStarting { row: String },
    /// The row that provides it runs, but has not provided it (or not
    /// where this copy looks).
    ProviderRunning { row: String },
    /// Anything else, as rutis says it (being removed, out of scope).
    Other(String),
}

impl Wait {
    pub fn new(service: impl Into<String>, reason: WaitReason) -> Self {
        Self {
            service: service.into(),
            reason,
        }
    }
}

/// A row as [`Loader::waits`] sees it: its status, its fiber, and the
/// services it declares.
struct Fact {
    status: EntryStatus,
    plugin: Option<PluginId>,
    provides: Vec<String>,
    /// Disabled (itself or by a group), and what it provides not known: it
    /// was never resolved.
    unknown_off: bool,
}

impl Loader {
    /// What the copy `entry` (one of [`Loader::entries`]) waits for while
    /// it is `Pending`, and why each is missing: no row declares it (with
    /// a declared name close to it), or the row that does is disabled,
    /// failed, waits for its own runtime, waits for a service of this row
    /// (they inject each other), and so on. Empty when it waits for
    /// nothing, or its fiber is gone. Read from a diagnostics snapshot:
    /// best effort. For several copies, [`Loader::waits_of`] reads it once.
    pub fn waits(&self, entry: &EntryInfo) -> Vec<Wait> {
        self.waits_of(&[entry]).pop().unwrap_or_default()
    }

    /// [`Loader::waits`] of each of `entries`, in order, from one snapshot
    /// of the rows and one of the diagnostics (a host following every
    /// waiting row reads them once per look).
    pub fn waits_of(&self, entries: &[&EntryInfo]) -> Vec<Vec<Wait>> {
        if entries.iter().all(|entry| entry.plugin.is_none()) {
            return vec![Vec::new(); entries.len()];
        }
        // Each entry's instance (`None` outside instances), and the rows as
        // each of those sees them.
        let (root, scopes, rows, records) = {
            let state = self.inner.state.lock().unwrap();
            let Some(root) = state.last_root.clone() else {
                return vec![Vec::new(); entries.len()];
            };
            let scopes: Vec<Option<u64>> = entries
                .iter()
                .map(|entry| {
                    entry.instance.as_ref().and_then(|instance| {
                        state
                            .instances
                            .iter()
                            .find(|(_, record)| record.plugin == instance.plugin)
                            .map(|(number, _)| *number)
                    })
                })
                .collect();
            let mut rows: HashMap<Option<u64>, Vec<Read>> = HashMap::new();
            for scope in &scopes {
                rows.entry(*scope)
                    .or_insert_with(|| read_rows(&state, *scope));
            }
            let records: HashMap<PluginId, String> = state
                .running
                .iter()
                .map(|(slot, running)| (running.view.id, slot.row.clone()))
                .collect();
            (root, scopes, rows, records)
        };
        let facts: HashMap<Option<u64>, Vec<(String, Fact)>> = rows
            .into_iter()
            .map(|(scope, rows)| (scope, facts(rows)))
            .collect();
        let diagnostics = root.diagnostics();
        let fibers: HashMap<PluginId, &PluginDiagnostics> = diagnostics
            .plugins
            .iter()
            .map(|fiber| (fiber.id, fiber))
            .collect();
        let missing = |plugin: PluginId| -> Vec<(TypeKey, DependencyStatus)> {
            fibers
                .get(&plugin)
                .map(|fiber| {
                    fiber
                        .injects
                        .iter()
                        .filter(|inject| inject.status != DependencyStatus::Ready)
                        .map(|inject| (inject.key.clone(), inject.status))
                        .collect()
                })
                .unwrap_or_default()
        };
        // Every name known to be provided or declared, for a misspelt one.
        let mut known: Vec<String> = facts
            .values()
            .flatten()
            .flat_map(|(_, fact)| fact.provides.iter().cloned())
            .collect();
        known.extend(
            diagnostics
                .bindings
                .iter()
                .filter_map(|binding| self.service_name(&binding.key)),
        );
        known.extend(self.inner.catalog.names());
        known.sort_unstable();
        known.dedup();

        entries
            .iter()
            .zip(scopes)
            .map(|(entry, scope)| {
                let Some(plugin) = entry.plugin else {
                    return Vec::new();
                };
                let facts = &facts[&scope];
                let fact_of = |id: &str| {
                    facts
                        .iter()
                        .find(|(row, _)| row == id)
                        .map(|(_, fact)| fact)
                };
                let own = fact_of(&entry.id)
                    .map(|fact| fact.provides.clone())
                    .unwrap_or_default();
                missing(plugin)
                    .into_iter()
                    .map(|(key, status)| {
                        let service = self.service_name(&key).unwrap_or_else(|| key.describe());
                        if own_runtime(&key) {
                            return Wait {
                                service,
                                reason: WaitReason::OwnRuntime,
                            };
                        }
                        let named = |key: &TypeKey| self.service_name(key);
                        let reason = match status {
                            DependencyStatus::Missing => {
                                if let Some(why) = unexported(&root, &service) {
                                    WaitReason::Explained(why)
                                } else {
                                    match facts.iter().find(|(id, fact)| {
                                        *id != entry.id && fact.provides.contains(&service)
                                    }) {
                                        Some((row, fact)) => {
                                            provider(row, fact, &own, &missing, named)
                                        }
                                        // Disabled rows never resolved may
                                        // provide it: not "no row".
                                        None if facts.iter().any(|(id, fact)| {
                                            *id != entry.id && fact.unknown_off
                                        }) =>
                                        {
                                            WaitReason::NoEnabledProvider {
                                                like: closest(&service, &known),
                                            }
                                        }
                                        None => WaitReason::NoProvider {
                                            like: closest(&service, &known),
                                        },
                                    }
                                }
                            }
                            DependencyStatus::ProviderInactive(_) => {
                                // The binding's provider, and its row.
                                let row = diagnostics
                                    .bindings
                                    .iter()
                                    .find(|binding| binding.key == key)
                                    .and_then(|binding| {
                                        row_of(binding.provider, &records, &fibers)
                                    });
                                match row.and_then(|row| fact_of(&row).map(|fact| (row, fact))) {
                                    Some((row, fact)) => {
                                        provider(&row, fact, &own, &missing, named)
                                    }
                                    None => WaitReason::Other(status.to_string()),
                                }
                            }
                            other => WaitReason::Other(other.to_string()),
                        };
                        Wait { service, reason }
                    })
                    .collect()
            })
            .collect()
    }

    /// The name a key is known by: the catalog's name for it, or its own.
    fn service_name(&self, key: &TypeKey) -> Option<String> {
        self.inner
            .catalog
            .name_of(key)
            .or_else(|| key.name().map(str::to_owned))
    }
}

/// A row as read under the loader's lock: its id, status and fiber, its
/// resolution and raw config, whose declarations are asked after, and
/// whether it is enabled (itself and the groups around it).
type Read = (
    String,
    EntryStatus,
    Option<PluginId>,
    Option<Arc<Resolved>>,
    serde_json::Value,
    bool,
);

/// Each plugin row (for a row inside instances, its copy in instance
/// `scope`).
fn read_rows(state: &State, scope: Option<u64>) -> Vec<Read> {
    state
        .desired
        .rows
        .iter()
        .filter(|row| !row.group)
        .map(|row| {
            let copy = state.desired.instanced_group(row).and(scope);
            let info = Inner::info(state, row, copy);
            let running = state.running.get(&Slot {
                row: row.id.clone(),
                scope: copy,
            });
            let resolved = running
                .and_then(|running| running.resolved.clone())
                .or_else(|| {
                    row.name
                        .as_ref()
                        .and_then(|name| state.resolved.get(name))
                        .and_then(|resolved| resolved.as_ref().ok().cloned())
                });
            (
                row.id.clone(),
                info.status,
                info.plugin,
                resolved,
                row.config.clone(),
                state.desired.wanted(row),
            )
        })
        .collect()
}

/// Each row by id, in tree order, with what it declares it provides: asked
/// of its plugin outside the loader's lock.
fn facts(rows: Vec<Read>) -> Vec<(String, Fact)> {
    rows.into_iter()
        .map(|(id, status, plugin, resolved, config, wanted)| {
            let declared = resolved
                .and_then(|resolved| resolved.declares.as_ref().map(|declares| declares(&config)))
                .and_then(Result::ok);
            let fact = Fact {
                status,
                plugin,
                unknown_off: !wanted && declared.is_none(),
                provides: declared
                    .map(|declared| declared.provides)
                    .unwrap_or_default(),
            };
            (id, fact)
        })
        .collect()
}

/// Why the row `row`, which declares the service, does not provide it.
/// `own`: what the waiting row provides; `missing`: what a fiber waits for.
fn provider(
    row: &str,
    fact: &Fact,
    own: &[String],
    missing: &dyn Fn(PluginId) -> Vec<(TypeKey, DependencyStatus)>,
    name: impl Fn(&TypeKey) -> Option<String>,
) -> WaitReason {
    let row = row.to_owned();
    match &fact.status {
        EntryStatus::Disabled => WaitReason::ProviderDisabled { row },
        EntryStatus::Unresolved(error) => WaitReason::ProviderFailed {
            row,
            error: error.to_string(),
        },
        EntryStatus::Running(snapshot) => match snapshot.state {
            FiberState::Failed => WaitReason::ProviderFailed {
                row,
                error: snapshot
                    .error
                    .as_ref()
                    .map_or_else(|| "failed".to_owned(), ToString::to_string),
            },
            FiberState::Pending => {
                let waits = fact.plugin.map(missing).unwrap_or_default();
                if let Some((key, _)) = waits.iter().find(|(key, _)| own_runtime(key)) {
                    return WaitReason::ProviderRuntime {
                        row,
                        runtime: key.name().unwrap_or_default().to_owned(),
                    };
                }
                let services: Vec<String> = waits
                    .iter()
                    .map(|(key, _)| name(key).unwrap_or_else(|| key.describe()))
                    .collect();
                match services.iter().find(|service| own.contains(service)) {
                    Some(service) => WaitReason::Cycle {
                        row,
                        service: service.clone(),
                    },
                    None => WaitReason::ProviderWaiting { row, services },
                }
            }
            FiberState::Loading => WaitReason::ProviderStarting { row },
            FiberState::Active => WaitReason::ProviderRunning { row },
            _ => WaitReason::ProviderInactive { row },
        },
        _ => WaitReason::ProviderInactive { row },
    }
}

/// The row whose fiber is `plugin` or an ancestor of it.
pub(super) fn row_of(
    plugin: PluginId,
    records: &HashMap<PluginId, String>,
    fibers: &HashMap<PluginId, &PluginDiagnostics>,
) -> Option<String> {
    let mut current = Some(plugin);
    while let Some(id) = current {
        if let Some(row) = records.get(&id) {
            return Some(row.clone());
        }
        current = fibers.get(&id).and_then(|fiber| fiber.parent);
    }
    None
}

/// How messages name `key`, when the loader knows it: a row's runtime as
/// `runtime <name>`, a service by its catalog name or the name it crosses
/// between languages under; `None` for other keys (Rust types).
pub(crate) fn key_name(catalog: &crate::ServiceCatalog, key: &TypeKey) -> Option<String> {
    if own_runtime(key) {
        return key.name().map(|name| format!("runtime {name}"));
    }
    if let Some(name) = catalog.name_of(key) {
        return Some(name);
    }
    #[cfg(any(feature = "runtimes", feature = "peer"))]
    if key.type_id() == std::any::TypeId::of::<dyn rutis_bridge::session::HostDispatch>() {
        return key.name().map(str::to_owned);
    }
    None
}

/// Whether `key` is a row's own runtime ([`RuntimeRows`](crate::runtime::RuntimeRows)).
pub(super) fn own_runtime(key: &TypeKey) -> bool {
    #[cfg(feature = "runtimes")]
    return key.type_id() == std::any::TypeId::of::<crate::runtime::RuntimeRows>();
    #[cfg(not(feature = "runtimes"))]
    {
        let _ = key;
        false
    }
}

/// Why `name` is missing, when rutis knows more (see
/// `rutis_bridge::exports::unexported`).
fn unexported(root: &rutis::Ctx, name: &str) -> Option<String> {
    #[cfg(any(feature = "runtimes", feature = "peer"))]
    return rutis_bridge::exports::unexported(root, name);
    #[cfg(not(any(feature = "runtimes", feature = "peer")))]
    {
        let _ = (root, name);
        None
    }
}

/// The name of `known` closest to `name` (the rule shared with config
/// keys, config schema design §6.1).
pub(crate) use rutis::internal::config::closest;
#[cfg(test)]
use rutis::internal::config::distance;

#[cfg(test)]
mod tests {
    use super::{closest, distance};

    #[test]
    fn a_misspelt_name_finds_the_one_it_meant() {
        let known = ["llm", "weather", "calendar"].map(str::to_owned);
        assert_eq!(closest("lmm", &known).as_deref(), Some("llm"));
        assert_eq!(closest("waether", &known).as_deref(), Some("weather"));
        assert_eq!(closest("calender", &known).as_deref(), Some("calendar"));
        assert_eq!(closest("store", &known), None);
        assert_eq!(closest("llm", &known), None);
        // Too short to guess.
        assert_eq!(closest("ll", &known), None);
        assert_eq!(distance("ab", "ba"), 1);
    }

    /// Reports name a row's runtime and a service as rows do, and leave
    /// Rust types to be described.
    #[cfg(feature = "runtimes")]
    #[test]
    fn keys_are_named_as_rows_name_them() {
        use super::key_name;
        let mut catalog = crate::ServiceCatalog::new();
        catalog.register::<u32>("counter");
        let runtime = crate::runtime::RuntimeRows::key("py");
        assert_eq!(key_name(&catalog, &runtime).as_deref(), Some("runtime py"));
        let llm = rutis_bridge::session::host_key("llm").erase();
        assert_eq!(key_name(&catalog, &llm).as_deref(), Some("llm"));
        let counter = rutis::host::TypeKey::of::<u32>();
        assert_eq!(key_name(&catalog, &counter).as_deref(), Some("counter"));
        let other = rutis::host::TypeKey::of::<String>();
        assert_eq!(key_name(&catalog, &other), None);
    }
}
