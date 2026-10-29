//! Module name → plugin factory.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use rutis::host::{BoxFuture, DynPlugin, ErrorKind, InstanceId, PluginFactory, TypeKey};
use rutis::internal::build_fn;
use rutis::{Error, Plugin};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::LoaderError;

/// What a name resolves to: [`Resolved::new`], then the `with_` methods.
#[non_exhaustive]
pub struct Resolved {
    pub factory: Arc<dyn PluginFactory<Value>>,
    /// JSON Schema of the config. The loader fills a copy of the config
    /// with its defaults and validates it against the schema before
    /// `validate_config` / `build` (config schema design §2.1), for the rows
    /// of every runtime but Cordis rows; the factory's own checks come
    /// after, and it gets the filled copy.
    pub schema: Option<Value>,
    /// Diagnostics: version, source path, hash and the like.
    pub meta: Value,
    /// The plugin handles its row's `isolate` and `inject` itself (they
    /// name services of another runtime, as for JavaScript rows): the loader
    /// neither resolves them through its catalog nor applies them, and the
    /// plugin reads them with `Loader::row`.
    pub foreign_scope: bool,
    /// Builds the factory for each copy from the instances it runs in
    /// ([`Builtins::register_with`]). Without it, every copy uses `factory`.
    pub scoped: Option<ScopedFactory>,
    /// What a row of it waits for in rutis and provides there, by service
    /// name, from the row's config ([`Loader::check`](crate::Loader::check)).
    /// Without it, nothing is known of either.
    pub declares: Option<Declarations>,
}

/// The services a row waits for in rutis and those it provides there, by
/// name, as its plugin declares them before it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    /// Waited for in rutis before the row starts (its plugin's `inject`
    /// that rutis gates; the row's own `inject` comes on top).
    pub requires: Vec<String>,
    /// Provided in rutis while the row runs.
    pub provides: Vec<String>,
    /// It may provide services `provides` does not list, announced when
    /// they are provided (a Cordis plugin whose package declares nothing):
    /// what it provides is known only once it runs.
    pub announces: bool,
}

/// A row's [`Declared`] from its config (the raw config, expressions
/// unevaluated), or why it is not known yet (what describes the plugin is
/// unreachable, say).
pub type Declarations = Arc<dyn Fn(&Value) -> Result<Declared, String> + Send + Sync>;

impl Resolved {
    /// `factory`, with no schema, no metadata, and the row's `isolate` and
    /// `inject` applied by the loader.
    pub fn new(factory: Arc<dyn PluginFactory<Value>>) -> Self {
        Resolved {
            factory,
            schema: None,
            meta: Value::Null,
            foreign_scope: false,
            scoped: None,
            declares: None,
        }
    }

    pub fn with_schema(mut self, schema: Option<Value>) -> Self {
        self.schema = schema;
        self
    }

    pub fn with_meta(mut self, meta: Value) -> Self {
        self.meta = meta;
        self
    }

    /// See [`Resolved::foreign_scope`].
    pub fn with_foreign_scope(mut self) -> Self {
        self.foreign_scope = true;
        self
    }

    pub fn with_scoped(mut self, scoped: ScopedFactory) -> Self {
        self.scoped = Some(scoped);
        self
    }

    /// What every row of it waits for and provides, whatever its config.
    pub fn with_declared(self, declared: Declared) -> Self {
        self.with_declarations(Arc::new(move |_: &Value| Ok(declared.clone())))
    }

    /// See [`Resolved::declares`].
    pub fn with_declarations(mut self, declares: Declarations) -> Self {
        self.declares = Some(declares);
        self
    }
}

/// Builds a copy's factory from the instances it runs in.
pub type ScopedFactory =
    Arc<dyn Fn(&Build) -> Result<Arc<dyn PluginFactory<Value>>, Error> + Send + Sync>;

pub(crate) type Values = Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>;

/// Where a copy runs: the instances of instanced groups enclosing it.
#[derive(Default, Clone)]
pub struct Build {
    /// Innermost first.
    pub(crate) chain: Vec<BuildLink>,
    /// The id of the row the copy is of, when known: the factory of a
    /// runtime that dials in names it in its config warnings.
    pub(crate) row: Option<String>,
}

#[derive(Clone)]
pub(crate) struct BuildLink {
    pub(crate) group: String,
    pub(crate) instance: InstanceId,
    pub(crate) values: Values,
}

impl Build {
    /// This build, for a copy of row `row`.
    pub(crate) fn for_row(mut self, row: &str) -> Self {
        self.row = Some(row.to_owned());
        self
    }

    /// The `ctx.instance()` of the enclosing instance of `group`: plugins in
    /// that instance can provide and read keys qualified with it.
    pub fn instance(&self, group: &str) -> Result<InstanceId, Error> {
        self.chain
            .iter()
            .find(|link| link.group == group)
            .map(|link| link.instance)
            .ok_or_else(|| {
                rutis::Error::from(ErrorKind::PluginFailed(
                    format!("not inside an instance of {group:?}").into(),
                ))
            })
    }

    /// The nearest value of type `T` given with `with` to an enclosing
    /// instance.
    pub fn value<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.chain.iter().find_map(|link| {
            link.values
                .get(&TypeId::of::<T>())
                .cloned()
                .and_then(|value| value.downcast::<T>().ok())
        })
    }

    /// The enclosing instances, innermost first: (group id, instance).
    pub fn instances(&self) -> impl Iterator<Item = (&str, InstanceId)> {
        self.chain
            .iter()
            .map(|link| (link.group.as_str(), link.instance))
    }
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("factory", &self.factory.name())
            .field("schema", &self.schema.is_some())
            .field("meta", &self.meta)
            .field("scoped", &self.scoped.is_some())
            .field("declares", &self.declares.is_some())
            .finish()
    }
}

pub trait Resolver: Send + Sync + 'static {
    /// Resolve a module name. Return [`LoaderError::NotFound`] for names this
    /// resolver does not handle, so a [`Chain`] can try the next one.
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>>;

    /// The runtimes whose rows it resolves, for [`Loader::status`](crate::Loader::status):
    /// none by default.
    #[cfg(feature = "runtimes")]
    fn runtimes(&self) -> Vec<rutis_bridge::runtime::RuntimeHandle> {
        Vec::new()
    }
}

/// Plugins compiled into the host, by exact name. Any name works, including
/// npm package names, so a Rust reimplementation can replace a JavaScript
/// plugin without touching configs.
#[derive(Default)]
pub struct Builtins {
    entries: HashMap<String, Arc<Resolved>>,
}

impl Builtins {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin built from its row's config: `build` makes the
    /// plugin from the config `C`, decoded from JSON (a `null` config, when
    /// the row has none, is decoded as `null` first, then as `{}`). The
    /// config's schema is generated from `C` (none for
    /// `serde_json::Value`); what the rows wait for comes from `P::Deps`.
    ///
    /// ```ignore
    /// builtins.register("weather", Weather::new); // fn new(config: &WeatherConfig) -> rutis::Result<Weather>
    /// ```
    pub fn register<C, P, B>(&mut self, name: impl Into<String>, build: B) -> &mut Self
    where
        C: DeserializeOwned + JsonSchema + Send + Sync + 'static,
        P: Plugin,
        B: Fn(&C) -> Result<P, Error> + Send + Sync + 'static,
    {
        let name = name.into();
        let factory = Named {
            name: name.clone(),
            factory: build_fn(build),
        };
        let schema = schema_of::<C>();
        let json = Json::<C, _>::new(factory, schema.is_some());
        self.insert(name, Arc::new(json), schema)
    }

    /// Register a factory that takes the raw JSON config: for a plugin
    /// whose dependencies are known only at run time (a
    /// [`DynPlugin`](rutis::host::DynPlugin) built from the config).
    pub fn register_raw(
        &mut self,
        name: impl Into<String>,
        factory: impl PluginFactory<Value>,
        schema: Option<Value>,
    ) -> &mut Self {
        self.insert(name, Arc::new(factory), schema)
    }

    /// [`register`](Self::register) for a plugin that depends on the
    /// instances it runs in: `build` makes each copy from where it runs
    /// ([`Build::instance`]) and its config. Outside instanced groups such
    /// a row fails when `build` does.
    pub fn register_with<C, P, B>(&mut self, name: impl Into<String>, build: B) -> &mut Self
    where
        C: DeserializeOwned + JsonSchema + Send + Sync + 'static,
        P: Plugin,
        B: Fn(&Build, &C) -> Result<P, Error> + Send + Sync + 'static,
    {
        let name = name.into();
        let build = Arc::new(build);
        let label = name.clone();
        let schema = schema_of::<C>();
        let has_schema = schema.is_some();
        let scoped: ScopedFactory = Arc::new(move |at: &Build| {
            let at = at.clone();
            let build = build.clone();
            let factory = Named {
                name: label.clone(),
                factory: build_fn(move |config: &C| build(&at, config)),
            };
            let factory: Arc<dyn PluginFactory<Value>> =
                Arc::new(Json::<C, _>::new(factory, has_schema));
            Ok(factory)
        });
        let factory = Arc::new(Unscoped { name: name.clone() });
        self.insert_resolved(name, factory, schema, Some(scoped))
    }

    fn insert(
        &mut self,
        name: impl Into<String>,
        factory: Arc<dyn PluginFactory<Value>>,
        schema: Option<Value>,
    ) -> &mut Self {
        self.insert_resolved(name, factory, schema, None)
    }

    fn insert_resolved(
        &mut self,
        name: impl Into<String>,
        factory: Arc<dyn PluginFactory<Value>>,
        schema: Option<Value>,
        scoped: Option<ScopedFactory>,
    ) -> &mut Self {
        let name = name.into();
        let meta = serde_json::json!({ "source": "builtin" });
        self.entries.insert(
            name,
            Arc::new(Resolved {
                factory,
                schema,
                meta,
                foreign_scope: false,
                scoped,
                declares: None,
            }),
        );
        self
    }

    /// Say what rows of the registered plugin `name` wait for and provide,
    /// from their config ([`Resolved::declares`]). Does nothing for a name
    /// not registered.
    pub fn declare(&mut self, name: &str, declares: Declarations) -> &mut Self {
        if let Some(entry) = self.entries.get_mut(name) {
            *entry = Arc::new(Resolved {
                factory: entry.factory.clone(),
                schema: entry.schema.clone(),
                meta: entry.meta.clone(),
                foreign_scope: entry.foreign_scope,
                scoped: entry.scoped.clone(),
                declares: Some(declares),
            });
        }
        self
    }
}

impl Resolver for Builtins {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        let found = self.entries.get(name).cloned();
        Box::pin(async move {
            found.ok_or_else(|| LoaderError::NotFound {
                name: name.to_owned(),
            })
        })
    }
}

/// Try resolvers in order; the first that does not answer `NotFound` wins.
#[derive(Default)]
pub struct Chain {
    resolvers: Vec<Arc<dyn Resolver>>,
}

impl Chain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, resolver: impl Resolver) -> Self {
        self.resolvers.push(Arc::new(resolver));
        self
    }

    /// Add a resolver that something else holds too, such as an
    /// `RuntimeResolver` shared with its `RuntimeRowsPlugin`.
    pub fn with_shared(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolvers.push(resolver);
        self
    }
}

impl Resolver for Chain {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            for resolver in &self.resolvers {
                match resolver.resolve(name).await {
                    Err(LoaderError::NotFound { .. }) => continue,
                    other => return other,
                }
            }
            Err(LoaderError::NotFound {
                name: name.to_owned(),
            })
        })
    }

    #[cfg(feature = "runtimes")]
    fn runtimes(&self) -> Vec<rutis_bridge::runtime::RuntimeHandle> {
        self.resolvers
            .iter()
            .flat_map(|resolver| resolver.runtimes())
            .collect()
    }
}

/// Adapts a `PluginFactory<C>` to JSON config.
struct Json<C, F> {
    inner: F,
    /// `C` has a schema the loader validates against first.
    schema: bool,
    _config: PhantomData<fn() -> C>,
}

impl<C, F> Json<C, F> {
    /// `schema`: `C` has a schema the loader validates against first.
    fn new(inner: F, schema: bool) -> Self {
        Self {
            inner,
            schema,
            _config: PhantomData,
        }
    }
}

/// `value` as `C`. With a schema the loader validated it first, and a
/// decoder error is one sentence; without one it is written by its kind
/// (config schema design §3.3).
fn decode<C: DeserializeOwned>(value: &Value, schema: bool) -> Result<C, Error> {
    rutis::config::decode(value, schema).map_err(rutis::ConfigError::into_error)
}

impl<C, F> PluginFactory<Value> for Json<C, F>
where
    C: DeserializeOwned + Send + Sync + 'static,
    F: PluginFactory<C>,
{
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.inner.injects()
    }

    fn validate_config(&self, config: &Value) -> Result<(), Error> {
        self.inner
            .validate_config(&decode::<C>(config, self.schema)?)
    }

    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        self.inner.build(&decode::<C>(config, self.schema)?)
    }
}

/// The factory of a `register_with` plugin outside any instance.
struct Unscoped {
    name: String,
}

impl PluginFactory<Value> for Unscoped {
    fn name(&self) -> &str {
        &self.name
    }

    fn build(&self, _config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Err(rutis::Error::from(ErrorKind::PluginFailed(
            format!("{} runs only inside an instanced group", self.name).into(),
        )))
    }
}

/// A factory shown under the name it was registered with.
struct Named<F> {
    name: String,
    factory: F,
}

impl<C, F> PluginFactory<C> for Named<F>
where
    C: Send + Sync + 'static,
    F: PluginFactory<C>,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.factory.injects()
    }

    fn validate_config(&self, config: &C) -> Result<(), Error> {
        self.factory.validate_config(config)
    }

    fn build(&self, config: &C) -> Result<Box<dyn DynPlugin>, Error> {
        self.factory.build(config)
    }
}

/// The schema of `C`; none for a raw JSON config.
fn schema_of<C: JsonSchema + 'static>() -> Option<Value> {
    if TypeId::of::<C>() == TypeId::of::<Value>() {
        return None;
    }
    serde_json::to_value(schemars::schema_for!(C)).ok()
}
