//! Read-only snapshots of fiber ownership and service resolution.
use std::sync::Arc;

use crate::error::ErrorKind;
use crate::{FiberState, InstanceId, PluginId, ServiceReadFailure, TypeKey};

/// A read-only, best-effort view of live fibers and service bindings.
///
/// Fields are collected under separate locks, so concurrent lifecycle changes
/// can make entries reflect different moments. A retained snapshot does not
/// update itself; call [`CtxHost::diagnostics`](crate::host::CtxHost::diagnostics) again for a fresh view.
///
/// Its `Display` is for people: one item a line, plugins by their row's or
/// their own name, services by the names reports give them
/// ([`CtxHost::name_keys_with`](crate::host::CtxHost::name_keys_with)), and
/// rutis's own plugins (transports, links, runtimes, the loader) left out;
/// `{:#}` lists those too. `Debug` has every field.
///
/// ```text
/// plugins:
///   weather: running
///   chat: waiting for calendar (service missing)
///   broken: failed: no API key
/// services:
///   forecast: provided by weather
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RuntimeDiagnostics {
    pub shutting_down: bool,
    pub plugins: Vec<PluginDiagnostics>,
    pub bindings: Vec<BindingDiagnostics>,
    /// Event keys with `emit`s waiting or running, longest waiting first.
    pub event_backlogs: Vec<EventBacklog>,
    pub(crate) key_names: KeyNames,
}

/// The key namers of the root when the snapshot was taken: asked only
/// when the snapshot is displayed, so reading one costs nothing more.
#[derive(Clone, Default)]
pub(crate) struct KeyNames(pub(crate) Vec<Arc<crate::host::KeyNamer>>);

impl KeyNames {
    fn get(&self, key: &TypeKey) -> String {
        self.0
            .iter()
            .find_map(|namer| namer(key))
            .unwrap_or_else(|| key.describe())
    }
}

impl std::fmt::Debug for KeyNames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("..")
    }
}

impl PluginDiagnostics {
    /// The name people know it by: its row's, else its plugin's.
    fn display_name(&self) -> &str {
        self.row.as_deref().unwrap_or(&self.name)
    }
}

impl std::fmt::Display for RuntimeDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let all = f.alternate();
        let shown = |plugin: &&PluginDiagnostics| all || !plugin.platform;
        if self.shutting_down {
            writeln!(f, "shutting down")?;
        }
        writeln!(f, "plugins:")?;
        for plugin in self.plugins.iter().filter(shown) {
            let state = match plugin.state {
                FiberState::Pending => {
                    let waits: Vec<String> = plugin
                        .injects
                        .iter()
                        .filter(|dep| dep.status != DependencyStatus::Ready)
                        .map(|dep| format!("{} ({})", self.key_names.get(&dep.key), dep.status))
                        .collect();
                    if waits.is_empty() {
                        "waiting".to_owned()
                    } else {
                        format!("waiting for {}", waits.join("; "))
                    }
                }
                FiberState::Loading => "starting".to_owned(),
                FiberState::Active => "running".to_owned(),
                FiberState::Unloading => "stopping".to_owned(),
                FiberState::Failed => match &plugin.error {
                    Some(error) => format!("failed: {error}"),
                    None => "failed".to_owned(),
                },
                FiberState::Disposed => "stopped".to_owned(),
            };
            writeln!(f, "  {}: {state}", plugin.display_name())?;
        }
        let provider = |id: PluginId| self.plugins.iter().find(|plugin| plugin.id == id);
        let services: Vec<String> = self
            .bindings
            .iter()
            .filter_map(|binding| {
                let by = match provider(binding.provider) {
                    // Provided on the root: by the host itself.
                    Some(plugin) if plugin.parent.is_none() => "the host",
                    Some(plugin) if all || !plugin.platform => plugin.display_name(),
                    Some(_) => return None,
                    None if all => "a plugin that is gone",
                    None => return None,
                };
                let mut line = format!("  {}: provided by {by}", self.key_names.get(&binding.key));
                if let Some(scope) = &binding.scope {
                    line.push_str(&format!(" in {scope}"));
                }
                if binding.removing {
                    line.push_str(" (being withdrawn)");
                }
                Some(line)
            })
            .collect();
        if !services.is_empty() {
            writeln!(f, "services:")?;
            for line in services {
                writeln!(f, "{line}")?;
            }
        }
        if !self.event_backlogs.is_empty() {
            writeln!(f, "events waiting:")?;
            for backlog in &self.event_backlogs {
                let oldest = std::time::Duration::from_millis(backlog.oldest.as_millis() as u64);
                writeln!(
                    f,
                    "  {}: {} not finished, oldest {oldest:?} ago",
                    backlog.name, backlog.pending
                )?;
            }
        }
        Ok(())
    }
}

/// The `emit`s of one event key that were accepted and have not finished.
/// An emit stays here until every listener call it started has finished;
/// `emit` itself never waits for them.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct EventBacklog {
    pub name: String,
    /// Accepted dispatches not finished, including the one running.
    pub pending: usize,
    /// How long ago the oldest of them was emitted.
    pub oldest: std::time::Duration,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PluginDiagnostics {
    pub id: PluginId,
    pub instance: InstanceId,
    pub parent: Option<PluginId>,
    pub name: String,
    /// The loader row it is (`Ctx::mark_row`), once it has started.
    pub row: Option<String>,
    /// Whether it is part of rutis itself (a transport, a link, a runtime,
    /// the loader) or the root: the readable `Display` leaves it out.
    pub platform: bool,
    pub state: FiberState,
    pub generation: u64,
    pub error: Option<Arc<ErrorKind>>,
    pub injects: Vec<DependencyDiagnostics>,
    pub resolved_dependencies: Vec<ResolvedDependency>,
    pub accesses: Vec<ServiceAccess>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DependencyDiagnostics {
    pub key: TypeKey,
    pub scope: Option<String>,
    pub status: DependencyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DependencyStatus {
    OutOfScope,
    Missing,
    Removing,
    ProviderInactive(FiberState),
    CheckPending,
    CheckRejected,
    CheckPanicked,
    Ready,
}

impl std::fmt::Display for DependencyStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfScope => f.write_str("instance out of scope"),
            Self::Missing => f.write_str("service missing"),
            Self::Removing => f.write_str("service being removed"),
            Self::ProviderInactive(state) => write!(f, "provider inactive ({state:?})"),
            Self::CheckPending => f.write_str("check pending"),
            Self::CheckRejected => f.write_str("check rejected"),
            Self::CheckPanicked => f.write_str("check panicked"),
            Self::Ready => f.write_str("ready"),
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedDependency {
    pub key: TypeKey,
    pub scope: Option<String>,
    pub provider: PluginId,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServiceAccess {
    pub key: TypeKey,
    pub scope: Option<String>,
    pub provider: Option<PluginId>,
    pub generation: Option<u64>,
    pub declared: bool,
    pub external: bool,
    pub out_of_scope: bool,
    /// Whether this access used `require` / `require_as`.
    pub strict: bool,
    /// Rejection reason for a strict read; absent on success and optional reads.
    pub failure: Option<ServiceReadFailure>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BindingDiagnostics {
    pub key: TypeKey,
    pub scope: Option<String>,
    pub provider: PluginId,
    pub generation: u64,
    pub removing: bool,
}

/// A service binding was registered or finally removed. Emitted on the bus
/// of the provider's context, after the registry change and outside every
/// lock. A registered binding becomes visible once its provider is active;
/// removal is reported after consumers were evicted.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServiceChanged {
    pub key: TypeKey,
    pub scope: Option<String>,
    pub provider: PluginId,
    pub generation: u64,
    pub change: ServiceChange,
    /// The provider's scope chain on the logical bus ([`CtxHost::scope`](crate::host::CtxHost::scope)).
    pub chain: Vec<crate::Scope>,
    /// The row the provider belongs to ([`CtxHost::row`](crate::host::CtxHost::row)).
    pub row: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ServiceChange {
    Provided,
    Removed,
}

/// Serialized with its key described as text. It is a process-local
/// diagnostic (its key is a Rust type): decoding one fails.
impl serde::Serialize for ServiceChanged {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("ServiceChanged", 7)?;
        s.serialize_field("key", &self.key.describe())?;
        s.serialize_field("scope", &self.scope)?;
        s.serialize_field("provider", &self.provider)?;
        s.serialize_field("generation", &self.generation)?;
        s.serialize_field("change", &self.change)?;
        s.serialize_field("chain", &self.chain)?;
        s.serialize_field("row", &self.row)?;
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for ServiceChanged {
    fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "rutis::ServiceChanged names a Rust type key and cannot be decoded; \
             listen to rutis/service instead",
        ))
    }
}

#[diagnostic::do_not_recommend]
impl crate::Event for ServiceChanged {
    const NAME: &'static str = "rutis::ServiceChanged";
    type Value = ();
}
