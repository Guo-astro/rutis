#![allow(dead_code)]

pub mod runtimes;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, FiberStatusChanged,
    PluginFactory, PluginId, TypeKey,
};
use rutis::{Ctx, Error};
use rutis_loader::edit::{Edit, Editable, Persist, PersistError, Version};
use rutis_loader::report::EntryStatus;
use rutis_loader::resolve::{Build, Resolved, ScopedFactory};
use rutis_loader::{
    Builtins, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch, Resolver,
};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

pub type Log = Arc<Mutex<Vec<String>>>;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EchoConfig {
    #[serde(default)]
    pub label: String,
    /// Rejected by `validate_config`.
    #[serde(default)]
    pub invalid: bool,
    /// Fails in `apply`, after the dry run passed.
    #[serde(default)]
    pub fail_apply: bool,
}

/// A service the `provider` module provides and `consumer` depends on.
#[derive(Debug)]
pub struct Dep(pub String);

struct Echo {
    kind: &'static str,
    config: EchoConfig,
    log: Log,
    broken: Arc<AtomicBool>,
}

impl DynPlugin for Echo {
    fn name(&self) -> &str {
        self.kind
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let label = self.config.label.clone();
            if self.config.fail_apply || self.broken.load(Ordering::SeqCst) {
                self.log.lock().unwrap().push(format!("fail:{label}"));
                return Err(rutis::Error::from(ErrorKind::PluginFailed(
                    format!("{label} failed").into(),
                )));
            }
            match self.kind {
                "provider" => {
                    ctx.provide(Dep(label.clone()))?;
                }
                "consumer" => {
                    let dep = ctx.require::<Dep>()?;
                    self.log.lock().unwrap().push(format!("consume:{}", dep.0));
                }
                _ => {}
            }
            self.log.lock().unwrap().push(format!("apply:{label}"));
            let log = self.log.clone();
            Ok(Effect::Disposer(Box::new(move || {
                log.lock().unwrap().push(format!("cleanup:{label}"));
                Ok(())
            })))
        })
    }
}

struct EchoFactory {
    kind: &'static str,
    injects: Vec<TypeKey>,
    log: Log,
    broken: Arc<AtomicBool>,
}

impl PluginFactory<EchoConfig> for EchoFactory {
    fn name(&self) -> &str {
        self.kind
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn validate_config(&self, config: &EchoConfig) -> Result<(), Error> {
        if config.invalid {
            return Err(rutis::Error::from(ErrorKind::Validation {
                issues: vec!["invalid".into()],
            }));
        }
        Ok(())
    }

    fn build(&self, config: &EchoConfig) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Echo {
            kind: self.kind,
            config: config.clone(),
            log: self.log.clone(),
            broken: self.broken.clone(),
        }))
    }
}

/// Builtins `echo`, `echo2`, `provider`, `consumer` (needs `Dep`) and
/// `flaky` (fails while `broken` is set), plus `late`, which only resolves
/// while `late` is set.
pub struct Harness {
    pub log: Log,
    pub broken: Arc<AtomicBool>,
    pub late: Arc<AtomicBool>,
}

impl Harness {
    pub fn new() -> Self {
        Self {
            log: Log::default(),
            broken: Arc::new(AtomicBool::new(false)),
            late: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn resolver(&self) -> Switch {
        let mut builtins = Builtins::new();
        for (name, kind, injects) in [
            ("echo", "echo", vec![]),
            ("echo2", "echo", vec![]),
            ("provider", "provider", vec![]),
            ("consumer", "consumer", vec![TypeKey::of::<Dep>()]),
            ("late", "echo", vec![]),
        ] {
            builtins.register_raw(
                name,
                json(EchoFactory {
                    kind,
                    injects,
                    log: self.log.clone(),
                    broken: Arc::new(AtomicBool::new(false)),
                }),
                schema_of::<EchoConfig>(),
            );
        }
        builtins.register_raw(
            "flaky",
            json(EchoFactory {
                kind: "flaky",
                injects: vec![],
                log: self.log.clone(),
                broken: self.broken.clone(),
            }),
            schema_of::<EchoConfig>(),
        );
        builtins.register::<Value, _, _>("plain", |_| Ok(Nothing));
        Switch {
            builtins,
            late: self.late.clone(),
        }
    }

    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

struct Nothing;

impl rutis::Plugin for Nothing {
    type Deps = ();

    fn name(&self) -> &str {
        "nothing"
    }

    async fn apply(&self, _ctx: &Ctx, (): ()) -> rutis::Result {
        Ok(())
    }
}

/// `factory` taking the row's JSON config, decoded as `Builtins::register`
/// decodes it (a `null` config as `null` first, then as `{}`), for
/// `Builtins::register_raw`: a factory that declares its dependencies in
/// `injects()` and checks its config in `validate_config`.
pub fn json<C, F>(factory: F) -> Json<C, F>
where
    C: DeserializeOwned + Send + Sync + 'static,
    F: PluginFactory<C>,
{
    Json {
        inner: factory,
        _config: PhantomData,
    }
}

/// The schema `Builtins::register` gives a config of type `C`.
pub fn schema_of<C: JsonSchema>() -> Option<Value> {
    serde_json::to_value(schemars::schema_for!(C)).ok()
}

/// See [`json`].
pub struct Json<C, F> {
    inner: F,
    _config: PhantomData<fn() -> C>,
}

fn decode<C: DeserializeOwned>(value: &Value) -> Result<C, Error> {
    let first = serde_json::from_value::<C>(value.clone());
    match (first, value) {
        (Ok(config), _) => Ok(config),
        (Err(_), Value::Null) => serde_json::from_value::<C>(Value::Object(Default::default()))
            .map_err(|e| {
                Error::from(ErrorKind::Validation {
                    issues: vec![e.to_string()],
                })
            }),
        (Err(e), _) => Err(Error::from(ErrorKind::Validation {
            issues: vec![e.to_string()],
        })),
    }
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
        self.inner.validate_config(&decode::<C>(config)?)
    }

    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        self.inner.build(&decode::<C>(config)?)
    }
}

/// Plugins whose factory is built for each copy from the instances it runs
/// in, with dependencies qualified by those instances (`injects()` naming
/// keys of `build.instance(..)`): what `Resolved::with_scoped` takes.
/// `Builtins::register_with` builds a `Plugin`, whose `Deps` name types
/// only, so these resolve here, in front of the `Builtins` in a `Chain`.
#[derive(Default)]
pub struct Scoped {
    entries: HashMap<String, Arc<Resolved>>,
}

impl Scoped {
    pub fn new() -> Self {
        Self::default()
    }

    /// `build` makes the factory of each copy from where it runs
    /// ([`Build::instance`]); outside instanced groups such a row fails.
    pub fn register<C, F, B>(&mut self, name: impl Into<String>, build: B) -> &mut Self
    where
        C: DeserializeOwned + JsonSchema + Send + Sync + 'static,
        F: PluginFactory<C>,
        B: Fn(&Build) -> Result<F, Error> + Send + Sync + 'static,
    {
        let name = name.into();
        let scoped: ScopedFactory = Arc::new(move |at: &Build| {
            let factory: Arc<dyn PluginFactory<Value>> = Arc::new(json::<C, F>(build(at)?));
            Ok(factory)
        });
        let resolved = Resolved::new(Arc::new(Unscoped { name: name.clone() }))
            .with_schema(schema_of::<C>())
            .with_meta(serde_json::json!({ "source": "builtin" }))
            .with_scoped(scoped);
        self.entries.insert(name, Arc::new(resolved));
        self
    }
}

impl Resolver for Scoped {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        let found = self.entries.get(name).cloned();
        Box::pin(async move {
            found.ok_or_else(|| LoaderError::NotFound {
                name: name.to_owned(),
            })
        })
    }
}

/// The factory of a [`Scoped`] plugin outside any instance.
struct Unscoped {
    name: String,
}

impl PluginFactory<Value> for Unscoped {
    fn name(&self) -> &str {
        &self.name
    }

    fn build(&self, _config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Err(Error::from(ErrorKind::PluginFailed(
            format!("{} runs only inside an instanced group", self.name).into(),
        )))
    }
}

pub struct Switch {
    builtins: Builtins,
    late: Arc<AtomicBool>,
}

impl Resolver for Switch {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        if name == "late" && !self.late.load(Ordering::SeqCst) {
            return Box::pin(async move {
                Err(LoaderError::NotFound {
                    name: name.to_owned(),
                })
            });
        }
        self.builtins.resolve(name)
    }
}

/// In-memory storage shared by several loaders, with version CAS.
#[derive(Clone, Default)]
pub struct MemStore {
    pub inner: Arc<Mutex<Stored>>,
}

#[derive(Default)]
pub struct Stored {
    pub patches: Vec<Patch>,
    pub version: u64,
    pub fail: bool,
    pub saves: Vec<Vec<Edit>>,
}

impl MemStore {
    pub fn version(&self) -> Version {
        Version(self.inner.lock().unwrap().version.to_string())
    }

    pub fn patches(&self) -> Vec<Patch> {
        self.inner.lock().unwrap().patches.clone()
    }

    pub fn set_fail(&self, fail: bool) {
        self.inner.lock().unwrap().fail = fail;
    }
}

impl Persist for MemStore {
    fn load<'a>(
        &'a self,
        _layer: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let stored = self.inner.lock().unwrap();
            Ok((stored.patches.clone(), Version(stored.version.to_string())))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        expected: &'a Version,
        edits: &'a [Edit],
        patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move {
            let mut stored = self.inner.lock().unwrap();
            if stored.fail {
                return Err(PersistError::Failed("disk full".into()));
            }
            if expected.0 != stored.version.to_string() {
                return Err(PersistError::Conflict);
            }
            stored.patches = patches.to_vec();
            stored.version += 1;
            stored.saves.push(edits.to_vec());
            Ok(Version(stored.version.to_string()))
        })
    }
}

pub fn patches(value: Value) -> Vec<Patch> {
    serde_json::from_value(value).unwrap()
}

/// Mount a loader on a fresh root.
pub async fn mount(resolver: Switch, store: MemStore) -> (Ctx, Loader) {
    mount_with(
        resolver,
        LoaderOptions {
            persist: Arc::new(store),
            ..LoaderOptions::default()
        },
    )
    .await
}

pub async fn mount_with(resolver: Switch, options: LoaderOptions) -> (Ctx, Loader) {
    let root = Ctx::root().unwrap();
    let plugin = LoaderPlugin::new(resolver, options);
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    (root, loader)
}

/// Layers `base` (given) and an editable `user` layer read from `store`.
pub async fn reconcile(loader: &Loader, base: Vec<Patch>, store: &MemStore) {
    let report = loader
        .reconcile(
            vec![
                Layer::new("base", base),
                Layer::new("user", store.patches()),
            ],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    assert!(report.new_failures.is_empty(), "{report:?}");
}

pub fn state(loader: &Loader, id: &str) -> Option<FiberState> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

/// The state changes of every fiber under a root (`FiberStatusChanged`).
///
/// One fiber's changes arrive in the order it made them: the fiber's own
/// drive task (`drive` in crates/rutis/src/fiber.rs) changes its state under
/// its transition lock, numbering each change (`seq`, from 1) and queueing
/// it (`set_state`, D24), then emits the queue outside the lock
/// (`flush_status`), on one event key, dispatched in emission order (D31).
/// That is not assumed here but checked: [`Transitions::loads`] asserts that
/// each fiber's changes arrived numbered 1, 2, 3, … with none missing (a
/// listener registered too late would miss the first). So once a change of
/// a fiber is here, every earlier change of it is too.
///
/// What it is for: a row must not start before what it injects. The rows
/// of these tests use the injected service in their apply, so a start
/// without it fails (Failed is neither Active nor Disposed, and
/// [`Transitions::loads`] then waits until the hang guard), and a start
/// that is undone by a restart is counted.
#[derive(Clone, Default)]
pub struct Transitions(Arc<Mutex<Vec<(PluginId, u64, FiberState)>>>);

impl Transitions {
    pub fn on(root: &Ctx) -> Self {
        let transitions = Self::default();
        let seen = transitions.clone();
        root.on::<FiberStatusChanged>(move |e, _| {
            seen.0.lock().unwrap().push((e.plugin_id, e.seq, e.to));
            std::future::ready(Ok(None))
        })
        .unwrap();
        transitions
    }

    /// The states `plugin` went to, in order.
    pub fn of(&self, plugin: PluginId) -> Vec<FiberState> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _, _)| *id == plugin)
            .map(|(_, _, state)| *state)
            .collect()
    }

    /// How often the fibers `plugins` (one row's, say) loaded, counted once
    /// each of them is active or disposed: a start before that is counted.
    pub async fn loads(&self, plugins: &[PluginId]) -> usize {
        let mut unique: Vec<PluginId> = Vec::new();
        for plugin in plugins {
            if !unique.contains(plugin) {
                unique.push(*plugin);
            }
        }
        let plugins = &unique;
        rutis_test_support::until(
            || {
                plugins.iter().all(|plugin| {
                    let states = self.of(*plugin);
                    states.contains(&FiberState::Active) || states.contains(&FiberState::Disposed)
                })
            },
            "the fibers to settle",
        )
        .await;
        let seen = self.0.lock().unwrap().clone();
        for plugin in plugins {
            let numbers: Vec<u64> = seen
                .iter()
                .filter(|(id, _, _)| id == plugin)
                .map(|(_, seq, _)| *seq)
                .collect();
            let expected: Vec<u64> = (1..=numbers.len() as u64).collect();
            assert_eq!(
                numbers, expected,
                "{plugin:?}: changes missed or out of order"
            );
        }
        seen.iter()
            .filter(|(id, _, state)| plugins.contains(id) && *state == FiberState::Loading)
            .count()
    }
}
