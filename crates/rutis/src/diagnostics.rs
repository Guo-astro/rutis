//! Read-only snapshots of fiber ownership and service resolution.
use std::sync::Arc;

use crate::{CordisError, FiberState, InstanceId, PluginId, ServiceReadFailure, TypeKey};

/// A read-only, best-effort view of live fibers and service bindings.
///
/// Fields are collected under separate locks, so concurrent lifecycle changes
/// can make entries reflect different moments. A retained snapshot does not
/// update itself; call [`crate::Ctx::diagnostics`] again for a fresh view.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RuntimeDiagnostics {
    pub shutting_down: bool,
    pub plugins: Vec<PluginDiagnostics>,
    pub bindings: Vec<BindingDiagnostics>,
    /// Event keys with `emit`s waiting or running, longest waiting first.
    pub event_backlogs: Vec<EventBacklog>,
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
    pub state: FiberState,
    pub generation: u64,
    pub error: Option<Arc<CordisError>>,
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
    /// The provider's scope chain on the logical bus ([`Ctx::scope`](crate::Ctx::scope)).
    pub chain: Vec<crate::Scope>,
    /// The row the provider belongs to ([`Ctx::row`](crate::Ctx::row)).
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

impl crate::Event for ServiceChanged {
    const NAME: &'static str = "rutis::ServiceChanged";
    type Value = ();
}
