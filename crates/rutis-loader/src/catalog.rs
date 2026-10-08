//! Service names for configs and expressions.
//!
//! Configs name services by string (`isolate: { llm: true }`, `inject:
//! [llm]`); rutis keys them by type. The catalog maps one to the other, and
//! records how to test or read a service, since the kernel offers no lookup
//! by an erased key.

use std::collections::HashMap;
use std::sync::Arc;

use rutis::{Ctx, TypeKey};
use serde::Serialize;
use serde_json::Value;

use crate::resolver::Build;
use crate::LoaderError;

type Probe = Arc<dyn Fn(&Ctx, &TypeKey) -> bool + Send + Sync>;
type Read = Arc<dyn Fn(&Ctx, &TypeKey) -> Option<Value> + Send + Sync>;

/// Where a name's key comes from.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum NameKey {
    /// The same key everywhere.
    Global(TypeKey),
    /// `base` with the `InstanceId` of the nearest enclosing instance of
    /// the instanced group `group`.
    Instance { group: String, base: TypeKey },
}

impl NameKey {
    /// The key for a copy that runs in `build`'s instances.
    pub(crate) fn key(&self, name: &str, build: &Build) -> Result<TypeKey, LoaderError> {
        match self {
            NameKey::Global(key) => Ok(key.clone()),
            NameKey::Instance { group, base } => build
                .chain
                .iter()
                .find(|link| &link.group == group)
                .map(|link| base.clone().with_instance(link.instance))
                .ok_or_else(|| LoaderError::OutsideInstance {
                    name: name.to_owned(),
                    group: group.clone(),
                }),
        }
    }

    /// The instanced group the name belongs to, if any.
    pub(crate) fn group(&self) -> Option<&str> {
        match self {
            NameKey::Global(_) => None,
            NameKey::Instance { group, .. } => Some(group),
        }
    }

    fn base(&self) -> &TypeKey {
        match self {
            NameKey::Global(key) | NameKey::Instance { base: key, .. } => key,
        }
    }
}

#[derive(Clone)]
struct Service {
    key: NameKey,
    exists: Probe,
    /// Present for services expressions may read.
    read: Option<Read>,
}

/// Service name → key, plus whether expressions may read the value.
///
/// A name is global, or belongs to the instances of one instanced group
/// (`register_instance`): there it resolves to each instance's own key, and
/// it does not resolve outside them.
#[derive(Clone, Default)]
pub struct ServiceCatalog {
    services: HashMap<String, Service>,
    /// Every name not registered otherwise is shared by name.
    #[cfg_attr(not(any(feature = "runtimes", feature = "peer")), allow(dead_code))]
    by_name: bool,
}

fn probe<T: ?Sized + Send + Sync + 'static>() -> Probe {
    Arc::new(|ctx: &Ctx, key: &TypeKey| ctx.get_as::<T>(key.clone()).is_some())
}

fn reader<T: Serialize + Send + Sync + 'static>() -> Read {
    Arc::new(|ctx: &Ctx, key: &TypeKey| {
        ctx.get_as::<T>(key.clone())
            .and_then(|value| serde_json::to_value(&*value).ok())
    })
}

impl ServiceCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, name: impl Into<String>, key: NameKey, exists: Probe, read: Option<Read>) {
        self.services
            .insert(name.into(), Service { key, exists, read });
    }

    /// Name the service `T` registered under `TypeKey::of::<T>()`.
    pub fn register<T: ?Sized + Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
    ) -> &mut Self {
        self.register_keyed::<T>(name, TypeKey::of::<T>())
    }

    /// Name a service registered under an explicit key (named, dynamic or
    /// instance keys).
    pub fn register_keyed<T: ?Sized + Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
        key: TypeKey,
    ) -> &mut Self {
        self.insert(name, NameKey::Global(key), probe::<T>(), None);
        self
    }

    /// Name the service `T` inside each instance of the instanced group
    /// `group`: a row resolves it to `TypeKey::instance::<T>(id)` of the
    /// nearest enclosing instance of `group`. Rows outside such instances
    /// cannot use it.
    pub fn register_instance<T: ?Sized + Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
        group: impl Into<String>,
    ) -> &mut Self {
        self.register_instance_keyed::<T>(name, group, TypeKey::of::<T>())
    }

    /// [`ServiceCatalog::register_instance`] with an explicit key (named or
    /// dynamic), qualified with the instance.
    pub fn register_instance_keyed<T: ?Sized + Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
        group: impl Into<String>,
        key: TypeKey,
    ) -> &mut Self {
        let key = NameKey::Instance {
            group: group.into(),
            base: key,
        };
        self.insert(name, key, probe::<T>(), None);
        self
    }

    /// Name a service whose value expressions may read, serialized to JSON.
    /// Meant for small host-owned values such as startup parameters.
    pub fn readable<T: Serialize + Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
    ) -> &mut Self {
        self.readable_keyed::<T>(name, TypeKey::of::<T>())
    }

    pub fn readable_keyed<T: Serialize + Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
        key: TypeKey,
    ) -> &mut Self {
        self.insert(
            name,
            NameKey::Global(key),
            probe::<T>(),
            Some(reader::<T>()),
        );
        self
    }

    /// Name a service shared across languages: a `dyn HostDispatch` at
    /// `host_key(name)`, provided by Rust or by a row of any runtime. Rows
    /// that inject a shared name wait for it in rutis; other names a
    /// JavaScript row injects are left to Cordis.
    #[cfg(all(unix, feature = "runtimes"))]
    pub fn register_shared(&mut self, name: impl Into<String>) -> &mut Self {
        let name = name.into();
        let key = rutis_bridge::session::host_key(&name);
        self.register_keyed::<dyn rutis_bridge::session::HostDispatch>(name, key)
    }

    /// A shared service ([`ServiceCatalog::register_shared`]) inside each
    /// instance of the instanced group `group`: at `host_key_in(name, id)`
    /// of the nearest enclosing instance of `group`, provided by Rust or by
    /// a row of any runtime in that instance.
    #[cfg(any(feature = "runtimes", feature = "peer"))]
    pub fn register_shared_instance(
        &mut self,
        name: impl Into<String>,
        group: impl Into<String>,
    ) -> &mut Self {
        let name = name.into();
        let key = rutis_bridge::session::host_key(&name);
        self.register_instance_keyed::<dyn rutis_bridge::session::HostDispatch>(name, group, key)
    }

    /// Share every name not registered otherwise, as
    /// [`ServiceCatalog::register_shared`] would: a host whose services all
    /// cross between languages and nodes by name (rutis-host).
    #[cfg(any(feature = "runtimes", feature = "peer"))]
    pub fn share_by_name(&mut self) -> &mut Self {
        self.by_name = true;
        self
    }

    /// Whether `name` is shared across languages by name, globally or
    /// inside instances.
    #[cfg(any(feature = "runtimes", feature = "peer"))]
    pub fn is_shared(&self, name: &str) -> bool {
        self.name_key(name)
            .is_some_and(|key| key.base() == &rutis_bridge::session::host_key(name))
    }

    /// The key of a global name; `None` for unknown names and for names
    /// inside instances (see [`ServiceCatalog::key_in`]).
    pub fn key(&self, name: &str) -> Option<TypeKey> {
        match self.name_key(name)? {
            NameKey::Global(key) => Some(key),
            NameKey::Instance { .. } => None,
        }
    }

    /// The key of `name` for a copy running in `build`'s instances.
    pub fn key_in(&self, name: &str, build: &Build) -> Result<TypeKey, LoaderError> {
        self.name_key(name)
            .ok_or_else(|| LoaderError::UnknownService(vec![name.to_owned()]))?
            .key(name, build)
    }

    /// The instance names usable in `build`'s instances, with the instance
    /// each resolves to: what a copy there isolates by instance.
    pub(crate) fn instance_names(&self, build: &Build) -> Vec<(String, rutis::InstanceId)> {
        let mut names: Vec<(String, rutis::InstanceId)> = self
            .services
            .iter()
            .filter_map(|(name, service)| {
                let group = service.key.group()?;
                let link = build.chain.iter().find(|link| link.group == group)?;
                Some((name.clone(), link.instance))
            })
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        names
    }

    pub(crate) fn name_key(&self, name: &str) -> Option<NameKey> {
        match self.services.get(name) {
            Some(service) => Some(service.key.clone()),
            None => self.shared_key(name).map(NameKey::Global),
        }
    }

    #[cfg(any(feature = "runtimes", feature = "peer"))]
    fn shared_key(&self, name: &str) -> Option<TypeKey> {
        self.by_name.then(|| rutis_bridge::session::host_key(name))
    }

    #[cfg(not(any(feature = "runtimes", feature = "peer")))]
    fn shared_key(&self, _: &str) -> Option<TypeKey> {
        None
    }

    /// Where the key of each of `names` comes from, or every unknown name.
    pub(crate) fn name_keys<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Vec<(String, NameKey)>, LoaderError> {
        let mut found = Vec::new();
        let mut unknown = Vec::new();
        for name in names {
            match self.name_key(name) {
                Some(key) => found.push((name.to_owned(), key)),
                None => unknown.push(name.to_owned()),
            }
        }
        if unknown.is_empty() {
            Ok(found)
        } else {
            Err(LoaderError::UnknownService(unknown))
        }
    }
}

/// What an expression may see: services by catalog name, through the
/// context of the row being evaluated (so `isolate` applies), and for a
/// copy inside instances, those instances' services.
pub struct ExprScope<'a> {
    ctx: Option<&'a Ctx>,
    catalog: &'a ServiceCatalog,
    build: Option<&'a Build>,
}

impl<'a> ExprScope<'a> {
    /// A scope over `ctx` (`None`: no service is available), outside
    /// instances. The loader builds these itself; this is public so
    /// evaluators can be tested.
    pub fn new(ctx: Option<&'a Ctx>, catalog: &'a ServiceCatalog) -> Self {
        Self {
            ctx,
            catalog,
            build: None,
        }
    }

    /// A scope over `ctx` for a copy running in `build`'s instances.
    pub fn in_instances(
        ctx: Option<&'a Ctx>,
        catalog: &'a ServiceCatalog,
        build: &'a Build,
    ) -> Self {
        Self {
            ctx,
            catalog,
            build: Some(build),
        }
    }

    fn service(&self, name: &str) -> Result<(&'a Service, TypeKey), LoaderError> {
        let service = self
            .catalog
            .services
            .get(name)
            .ok_or_else(|| LoaderError::UnknownService(vec![name.to_owned()]))?;
        let outside = Build::default();
        let key = service.key.key(name, self.build.unwrap_or(&outside))?;
        Ok((service, key))
    }

    /// Whether the named service is currently available. Any catalog name
    /// may be tested; an unknown name, or an instance name outside its
    /// instances, is an error, not `false`.
    pub fn has(&self, name: &str) -> Result<bool, LoaderError> {
        let (service, key) = self.service(name)?;
        Ok(self.ctx.is_some_and(|ctx| (service.exists)(ctx, &key)))
    }

    /// The value of a readable service, `None` while it is absent. Services
    /// not registered as readable cannot be read.
    pub fn read(&self, name: &str) -> Result<Option<Value>, LoaderError> {
        let (service, key) = self.service(name)?;
        let read = service
            .read
            .as_ref()
            .ok_or_else(|| LoaderError::NotReadable(name.to_owned()))?;
        Ok(self.ctx.and_then(|ctx| read(ctx, &key)))
    }
}

/// Evaluates expression nodes (`{ "__jsExpr": "<source>" }`) in rows.
/// The loader ships no implementation; dsh's lives in rutis-dsh.
///
/// Called with the loader's state lock held: it must not call back into the
/// loader.
pub trait Expressions: Send + Sync + 'static {
    fn evaluate(&self, expr: &str, scope: &ExprScope<'_>) -> Result<Value, LoaderError>;
}
