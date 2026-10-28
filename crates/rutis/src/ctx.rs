use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::bus::EventBus;
use crate::diagnostics::{
    DependencyDiagnostics, DependencyStatus, PluginDiagnostics, ResolvedDependency,
    RuntimeDiagnostics, ServiceAccess,
};
use crate::effect::{Disposer, Effect, EffectRecord};
use crate::error::{
    default_sink, Error, ErrorKind, ErrorSink, ServiceReadError, ServiceReadFailure,
};
use crate::fiber::{
    join_task, spawn_fiber, DisposeWaitError, FiberInner, FiberState, FiberView, Intent, PluginId,
    TransitionTask,
};
use crate::host::CtxHost;
use crate::intercept::{ServiceInterceptors, ServiceWriter};
use crate::internal::CtxInternal;
use crate::key::{InstanceId, Key, ScopeId, TypeKey};
use crate::plugin::{DynPlugin, Plugin, PluginFactory};
use crate::registry::{Binding, CheckFn, Registry, StoredValue, ValueSlot};

pub(crate) struct Shared {
    pub admission: Mutex<()>,
    pub handle: Handle,
    pub bus: EventBus,
    pub registry: Registry,
    pub interceptors: Arc<ServiceInterceptors>,
    pub error_sink: ErrorSink,
    pub next_plugin_id: AtomicU64,
    pub closing: AtomicBool,
    pub shutdown_task: Mutex<Option<Arc<TransitionTask>>>,
    /// How long a plugin waits for its services before the error sink is
    /// told which ones (`CtxHost::report_waiting_after`); `None`: never.
    pub waiting_report: Mutex<Option<std::time::Duration>>,
    /// What names keys in those reports (`CtxHost::name_keys_with`), held
    /// by whoever added them.
    pub key_namers: Mutex<Vec<std::sync::Weak<crate::host::KeyNamer>>>,
}

pub(crate) struct CtxInner {
    pub(crate) shared: Arc<Shared>,
    pub(crate) parent: Option<Ctx>,
    pub(crate) fiber: Weak<FiberInner>,
    pub(crate) isolate: Option<(TypeKey, ScopeId)>,
    pub(crate) instance: InstanceId,
    pub(crate) plugin_id: PluginId,
    pub(crate) closing: Arc<AtomicBool>,
    /// The load generation that handed this context to plugin code. Root and
    /// framework-owned contexts have no generation; isolated contexts inherit it.
    pub(crate) generation: Option<u64>,
    pub(crate) generation_token: Option<CancellationToken>,
}

/// A plugin's context. Cloning is cheap; [`plugin`](Ctx::plugin) and
/// [`isolate`](crate::host::CtxHost::isolate) return new contexts that share
/// the same root.
#[derive(Clone)]
pub struct Ctx(Arc<CtxInner>);

struct AccessOutcome {
    found: Option<(PluginId, u64, bool)>,
    declared: bool,
    out_of_scope: bool,
    strict: bool,
    failure: Option<ServiceReadFailure>,
}

struct EffectFactoryLease(Arc<FiberInner>);

impl Drop for EffectFactoryLease {
    fn drop(&mut self) {
        self.0.finish_event();
    }
}

impl Ctx {
    pub(crate) fn new_child(
        shared: Arc<Shared>,
        parent: &Ctx,
        fiber: Weak<FiberInner>,
        isolate: Option<(TypeKey, ScopeId)>,
        instance: InstanceId,
        plugin_id: PluginId,
        closing: Arc<AtomicBool>,
    ) -> Self {
        Self(Arc::new(CtxInner {
            shared,
            parent: Some(parent.clone()),
            fiber,
            isolate,
            instance,
            plugin_id,
            closing,
            generation: None,
            generation_token: None,
        }))
    }

    pub(crate) fn new_root(
        shared: Arc<Shared>,
        fiber: Weak<FiberInner>,
        instance: InstanceId,
        plugin_id: PluginId,
        closing: Arc<AtomicBool>,
    ) -> Self {
        Self(Arc::new(CtxInner {
            shared,
            parent: None,
            fiber,
            isolate: None,
            instance,
            plugin_id,
            closing,
            generation: None,
            generation_token: None,
        }))
    }

    pub(crate) fn weak_fiber(&self) -> Weak<FiberInner> {
        self.0.fiber.clone()
    }

    pub(crate) fn for_generation(&self, generation: u64) -> Self {
        self.for_generation_with_token(generation, self.cancellation_token())
    }

    fn for_generation_with_token(&self, generation: u64, token: CancellationToken) -> Self {
        Self(Arc::new(CtxInner {
            shared: self.0.shared.clone(),
            parent: self.0.parent.clone(),
            fiber: self.0.fiber.clone(),
            isolate: self.0.isolate.clone(),
            instance: self.0.instance,
            plugin_id: self.0.plugin_id,
            closing: self.0.closing.clone(),
            generation: Some(generation),
            generation_token: Some(token),
        }))
    }

    fn check_generation(&self, current: u64, state: FiberState) -> Result<(), Error> {
        if let Some(expected) = self.0.generation {
            if expected != current {
                return Err(ErrorKind::StaleGeneration { expected, current }.into());
            }
            if matches!(state, FiberState::Unloading | FiberState::Disposed) {
                return Err(ErrorKind::InactiveEffect.into());
            }
            if !matches!(state, FiberState::Loading | FiberState::Active) {
                return Err(ErrorKind::InactiveGeneration {
                    generation: expected,
                    state,
                }
                .into());
            }
        }
        Ok(())
    }

    pub(crate) fn plugin_id(&self) -> PluginId {
        self.0.plugin_id
    }

    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.0.shared
    }

    /// True only while a live ancestor fiber owns this identity.
    pub(crate) fn in_instance(&self, id: InstanceId) -> bool {
        let mut current = self.0.fiber.upgrade();
        while let Some(fiber) = current {
            if fiber.instance == id {
                return true;
            }
            current = fiber.parent_fiber.as_ref().and_then(Weak::upgrade);
        }
        false
    }

    pub(crate) fn check_instance(&self, key: &TypeKey) -> Result<(), Error> {
        if let Some(id) = key.instance_id() {
            if !self.in_instance(id) {
                return Err(ErrorKind::InstanceOutOfScope { instance: id }.into());
            }
        }
        Ok(())
    }

    pub(crate) fn subtree_closing(&self) -> bool {
        let mut current = Some(self.clone());
        while let Some(ctx) = current {
            if ctx.0.closing.load(Ordering::SeqCst) {
                return true;
            }
            current = ctx.0.parent.clone();
        }
        false
    }

    pub(crate) fn registration_open(&self) -> Result<(), Error> {
        if self.0.shared.closing.load(Ordering::SeqCst) || self.subtree_closing() {
            Err(ErrorKind::Closed.into())
        } else {
            Ok(())
        }
    }

    /// Common precedence for public service and instance-event operations:
    /// permanent closure, inactive fiber, then key/instance validation.
    /// Registration itself repeats the state check under the admission lock.
    pub(crate) fn registration_preflight(&self) -> Result<(), Error> {
        self.registration_open()?;
        let fiber = self.0.fiber.upgrade().ok_or(ErrorKind::InactiveEffect)?;
        let tr = fiber.transition.lock().unwrap();
        self.check_generation(tr.generation, tr.state)?;
        if matches!(tr.state, FiberState::Unloading | FiberState::Disposed) {
            return Err(ErrorKind::InactiveEffect.into());
        }
        Ok(())
    }

    /// Non-instance event dispatch is not a registration. Preserve its
    /// historical observer behavior for an old context while its fiber is
    /// active again; instance dispatch still uses registration_preflight.
    pub(crate) fn dispatch_preflight(&self) -> Result<(), Error> {
        self.registration_open()?;
        let fiber = self.0.fiber.upgrade().ok_or(ErrorKind::InactiveEffect)?;
        if matches!(fiber.state(), FiberState::Unloading | FiberState::Disposed) {
            return Err(ErrorKind::InactiveEffect.into());
        }
        Ok(())
    }

    /// Mark this context's fiber as running the code of the library loaded
    /// as `generation` ([`crate::service::set_generation`]).
    pub(crate) fn set_code_generation(&self, generation: u64) {
        if let Some(fiber) = self.0.fiber.upgrade() {
            fiber.code_generation.store(generation, Ordering::SeqCst);
        }
    }

    /// The generation of the library whose code runs in this context's
    /// fiber or its nearest marked ancestor.
    pub(crate) fn code_generation(&self) -> Option<u64> {
        let mut current = self.0.fiber.upgrade();
        while let Some(fiber) = current {
            match fiber.code_generation.load(Ordering::SeqCst) {
                0 => current = fiber.parent_fiber.as_ref().and_then(Weak::upgrade),
                generation => return Some(generation),
            }
        }
        None
    }

    /// The name of the plugin owning this context.
    pub(crate) fn plugin_name(&self) -> String {
        self.0
            .fiber
            .upgrade()
            .map(|fiber| fiber.name.clone())
            .unwrap_or_default()
    }

    /// The scope for `key`: walks up the parent contexts and takes the nearest
    /// isolate of that key.
    pub(crate) fn scope_for(&self, key: &TypeKey) -> Option<ScopeId> {
        let mut current = Some(self.clone());
        while let Some(ctx) = current {
            if let Some((k, scope)) = &ctx.0.isolate {
                if k == key {
                    return Some(scope.clone());
                }
            }
            current = ctx.0.parent.clone();
        }
        None
    }

    /// Reads the service of type `T`, in the isolate scope found along the parent
    /// chain. A service whose provider is not active, or which is being removed,
    /// is hidden, except from the provider's own subtree (so its cleanup can
    /// still read it). It is also hidden when the reading fiber is itself
    /// unloading or disposed, as with an inactive context in Cordis; the
    /// provider's own subtree is again exempt.
    ///
    /// An optional read: it does not gate and creates no reload relation;
    /// diagnostics record it as an undeclared access. `T` may be a trait
    /// object. Services a plugin needs go in its `Deps`.
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.get_key::<T>(TypeKey::of::<T>())
    }

    fn require_inner<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: TypeKey,
        location: &'static std::panic::Location<'static>,
        provision: Option<&crate::service::Provision>,
        actor: Option<&Ctx>,
    ) -> Result<Arc<T>, ServiceReadError> {
        let scope = self.scope_for(&key);
        let caller = self.0.fiber.upgrade();
        let own_service = caller.as_ref().is_some_and(|fiber| {
            fiber
                .provided
                .lock()
                .unwrap()
                .iter()
                .any(|(provided_key, provided_scope)| {
                    provided_key == &key && provided_scope.as_ref() == scope.as_ref()
                })
        });
        let mut current = caller.clone();
        let mut declared_by_injects = false;
        while let Some(fiber) = current {
            if fiber.declared_injects.contains(&key)
                && fiber.ctx.scope_for(&key).as_ref() == scope.as_ref()
            {
                declared_by_injects = true;
                break;
            }
            current = fiber
                .parent_fiber
                .as_ref()
                .and_then(std::sync::Weak::upgrade);
        }
        let declared = own_service || declared_by_injects;
        let failure = if !self.in_instance_key(&key) {
            Some(ServiceReadFailure::OutOfScope)
        } else if !key.has_type::<T>() {
            Some(ServiceReadFailure::TypeMismatch)
        } else if caller.as_ref().is_none_or(|fiber| {
            matches!(fiber.state(), FiberState::Unloading | FiberState::Disposed)
        }) && !own_service
        {
            Some(ServiceReadFailure::Inactive)
        } else if !declared {
            Some(ServiceReadFailure::Undeclared)
        } else {
            None
        };
        if let Some(reason) = failure {
            if let Some(fiber) = caller.as_ref() {
                self.record_access(
                    fiber,
                    &key,
                    scope.as_ref(),
                    AccessOutcome {
                        found: None,
                        declared,
                        out_of_scope: reason == ServiceReadFailure::OutOfScope,
                        strict: true,
                        failure: Some(reason),
                    },
                );
            }
            return Err(self.read_error(key, reason, location));
        }

        let binding = self.0.shared.registry.lookup(&key, scope.as_ref());
        // A marked service's entry serves the binding it was provided as:
        // another one in the slot (provided again, by another build) is not
        // the value its code was chosen for.
        if let Some(provision) = provision {
            if !binding
                .as_ref()
                .is_some_and(|binding| provision.is(binding))
            {
                let reason = ServiceReadFailure::Unavailable(DependencyStatus::Missing);
                return Err(self.read_error(key, reason, location));
            }
        }
        // The cached status is read-only: this path never invokes user check().
        // Self-provided services retain the existing loading/cleanup exemption.
        if !own_service {
            let status = binding
                .as_ref()
                .map_or(DependencyStatus::Missing, |binding| {
                    Registry::binding_status(binding)
                });
            if status != DependencyStatus::Ready {
                let reason = ServiceReadFailure::Unavailable(status);
                if let Some(fiber) = caller.as_ref() {
                    self.record_access(
                        fiber,
                        &key,
                        scope.as_ref(),
                        AccessOutcome {
                            found: None,
                            declared,
                            out_of_scope: false,
                            strict: true,
                            failure: Some(reason),
                        },
                    );
                }
                return Err(self.read_error(key, reason, location));
            }
        }
        let (value, found) = self.read_binding_as::<T>(binding.as_ref());
        // During removal, a new provider may take the same slot before this
        // fiber's own provide record is cleared. Do not grant that replacement
        // the self-access exemption.
        if own_service && found.is_some_and(|(provider, _, _)| provider != self.0.plugin_id) {
            let reason = if caller.as_ref().is_none_or(|fiber| {
                matches!(fiber.state(), FiberState::Unloading | FiberState::Disposed)
            }) {
                Some(ServiceReadFailure::Inactive)
            } else if !declared_by_injects {
                Some(ServiceReadFailure::Undeclared)
            } else {
                let status = binding
                    .as_ref()
                    .map_or(DependencyStatus::Missing, |binding| {
                        Registry::binding_status(binding)
                    });
                (status != DependencyStatus::Ready)
                    .then_some(ServiceReadFailure::Unavailable(status))
            };
            if let Some(reason) = reason {
                if let Some(fiber) = caller.as_ref() {
                    self.record_access(
                        fiber,
                        &key,
                        scope.as_ref(),
                        AccessOutcome {
                            found: None,
                            declared: declared_by_injects,
                            out_of_scope: false,
                            strict: true,
                            failure: Some(reason),
                        },
                    );
                }
                return Err(self.read_error(key, reason, location));
            }
        }
        let mut reason = value.is_none().then(|| {
            if !own_service
                && caller.as_ref().is_none_or(|fiber| {
                    matches!(fiber.state(), FiberState::Unloading | FiberState::Disposed)
                })
            {
                return ServiceReadFailure::Inactive;
            }
            let status = binding
                .as_ref()
                .map_or(DependencyStatus::Missing, |binding| {
                    Registry::binding_status(binding)
                });
            // The binding may have changed state since the failed read.
            // Report the failed read, not a contradictory Ready.
            ServiceReadFailure::Unavailable(if status == DependencyStatus::Ready {
                DependencyStatus::Missing
            } else {
                status
            })
        });
        let result = match value {
            Some(value) => actor
                .unwrap_or(self)
                .apply_read_interceptors(&key, scope.as_ref(), value)
                .map_err(|failure| {
                    reason = Some(failure);
                    self.read_error(key.clone(), failure, location)
                }),
            None => Err(self.read_error(key.clone(), reason.unwrap(), location)),
        };
        if let Some(fiber) = caller.as_ref() {
            self.record_access(
                fiber,
                &key,
                scope.as_ref(),
                AccessOutcome {
                    found,
                    declared,
                    out_of_scope: false,
                    strict: true,
                    failure: reason,
                },
            );
        }
        result
    }

    fn read_error(
        &self,
        key: TypeKey,
        reason: ServiceReadFailure,
        location: &'static std::panic::Location<'static>,
    ) -> ServiceReadError {
        ServiceReadError {
            key,
            plugin_id: self.0.plugin_id,
            instance: self.instance(),
            location,
            reason,
        }
    }

    fn read_binding_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        binding: Option<&Arc<Binding>>,
    ) -> (Option<Arc<T>>, Option<(PluginId, u64, bool)>) {
        let Some(binding) = binding else {
            return (None, None);
        };
        let Some(provider) = binding.provider.upgrade() else {
            return (None, None);
        };
        let self_access = self.in_subtree_of(&provider);
        let found = Some((binding.provider_id, binding.provider_gen, !self_access));
        if !self_access {
            match self.0.fiber.upgrade() {
                None => return (None, found),
                Some(accessor)
                    if matches!(
                        accessor.state(),
                        FiberState::Unloading | FiberState::Disposed
                    ) =>
                {
                    return (None, found);
                }
                _ => {}
            }
            if provider.state() != FiberState::Active || binding.removing.load(Ordering::SeqCst) {
                return (None, found);
            }
        }
        (binding.value.snapshot().downcast::<T>(), found)
    }

    fn record_access(
        &self,
        fiber: &Arc<FiberInner>,
        key: &TypeKey,
        scope: Option<&ScopeId>,
        outcome: AccessOutcome,
    ) {
        let (provider, generation, external) = outcome
            .found
            .map(|(p, g, x)| (Some(p), Some(g), x))
            .unwrap_or((None, None, false));
        let access = ServiceAccess {
            key: key.clone(),
            scope: scope.map(|s| s.to_string()),
            provider,
            generation,
            declared: outcome.declared,
            external,
            out_of_scope: outcome.out_of_scope,
            strict: outcome.strict,
            failure: outcome.failure,
        };
        let mut accesses = fiber.accesses.lock().unwrap();
        if !accesses.contains(&access) {
            accesses.push(access);
        }
    }

    fn in_subtree_of(&self, other: &Arc<FiberInner>) -> bool {
        let mut current = self.0.fiber.upgrade();
        while let Some(fiber) = current {
            if Arc::ptr_eq(&fiber, other) {
                return true;
            }
            current = fiber.parent_fiber.as_ref().and_then(|w| w.upgrade());
        }
        false
    }

    /// Provides `value` under its own type until the plugin stops (or the
    /// returned [`Disposer`] withdraws it). A `#[rutis::service]` type is
    /// offered to other languages by its name too.
    pub fn provide<T: Send + Sync + 'static>(&self, value: T) -> Result<Disposer, Error> {
        self.provide_key::<T>(TypeKey::of::<T>(), Arc::new(value))
    }

    /// Provides `value` under the type `T`, typically a trait object:
    /// `ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))`.
    pub fn provide_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        value: Arc<T>,
    ) -> Result<Disposer, Error> {
        self.provide_key::<T>(TypeKey::of::<T>(), value)
    }

    /// Runs `f` now and registers the future it returns as a cleanup, run
    /// when the plugin stops (in reverse registration order). As Cordis's
    /// `ctx.effect`: when the plugin is already stopping, `f` does not run
    /// and the result is `InactiveEffect`, so nothing is acquired that
    /// would not be released.
    ///
    /// ```no_run
    /// # async fn demo(ctx: &rutis::Ctx) -> rutis::Result {
    /// ctx.effect(|| {
    ///     let stop = ctx.clone();
    ///     let task = tokio::spawn(async move { stop.cancelled().await });
    ///     async move {
    ///         let _ = task.await; // wait for it to exit at stop
    ///         Ok(())
    ///     }
    /// })?;
    /// # Ok(()) }
    /// ```
    ///
    /// Where it is called is kept: a shutdown that runs out of time names
    /// the cleanup by it (`the cleanup registered at src/lib.rs:12:9`).
    #[track_caller]
    pub fn effect<Fut>(&self, f: impl FnOnce() -> Fut) -> Result<Disposer, Error>
    where
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let origin = crate::effect::Origin::Effect(std::panic::Location::caller());
        let record = self.register_effect_named("anonymous".into(), origin, move || {
            let cleanup = f();
            Effect::AsyncDisposer(Box::new(move || Box::pin(cleanup)))
        })?;
        Ok(self.record_disposer(record))
    }

    fn provide_inner<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: TypeKey,
        value: Arc<T>,
        check: Option<CheckFn>,
        mutable: bool,
    ) -> Result<(Disposer, Arc<Binding>), Error> {
        self.registration_preflight()?;
        self.check_instance(&key)?;
        if !key.has_type::<T>() {
            return Err(ErrorKind::Validation {
                issues: vec![format!(
                    "service key {} expects value of type {}, got {}",
                    key.describe(),
                    key.type_name(),
                    std::any::type_name::<T>()
                )],
            }
            .into());
        }
        // A `#[rutis::service]` type is also served by name (R2), when it
        // is provided at the type itself (in an instance or not): a keyed
        // binding (`TypeKey::keyed`) is a second value of the type, which
        // would claim the same name, and a mutable binding is replaced by
        // its writer while the name would keep serving the first value.
        let marked = if mutable || key.name().is_some() {
            None
        } else {
            crate::service::marked::<T>(self)
        };
        let Some(entries) = marked else {
            let plain = !mutable && key.name().is_none();
            let provided = self.provide_native(key, value, check, mutable)?;
            // Said once it is provided: a refused provide serves nothing.
            if plain {
                if let Some(service) = crate::service::unserved::<T>(self) {
                    let note = Arc::new(ErrorKind::NotServedByName {
                        service: service.to_owned(),
                    });
                    let sink = self.error_sink();
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(note)));
                }
            }
            return Ok(provided);
        };
        // Both slots are checked before anything is registered: a provide
        // refused for either serves nothing, not even for a moment. The
        // name (R6) is checked by the exposer, where it would serve it.
        let scope = self.scope_for(&key);
        if self.slot_taken(&key) {
            return Err(ErrorKind::ServiceExists(format!(
                "{} in scope {}",
                key.describe(),
                scope.as_deref().unwrap_or("<default>")
            ))
            .into());
        }
        let provision = Arc::new(crate::service::Provision::new());
        let exposure = |entries| crate::service::Exposure {
            ctx: self,
            native: &key,
            scope: scope.as_deref(),
            entries,
            check: check.clone(),
            provision: provision.clone(),
        };
        crate::service::admit(&exposure(entries.clone()))?;
        // Then the native service, then the name: a row the name wakes
        // finds what it calls. Registered after it, the name is withdrawn
        // before it when the provider unloads (cleanups run last in, first
        // out), as by the disposer below: no call by name finds the name
        // served and the native service gone.
        let (native, binding) = self.provide_native(key.clone(), value, check.clone(), mutable)?;
        provision.fill(&binding);
        match crate::service::expose(exposure(entries)) {
            None => Ok((native, binding)),
            // One disposer for both, the name going first.
            Some(Ok(exposed)) => Ok((crate::service::chain(exposed, native), binding)),
            Some(Err(error)) => {
                // Only a race with another provide of the name, or the
                // provider unloading meanwhile, gets here: the native
                // service goes with the refusal, on this root's runtime
                // whether or not the caller runs on one.
                self.handle().spawn(native.dispose());
                Err(error)
            }
        }
    }

    /// Whether `key`'s slot, in the isolation scope this context gives it,
    /// holds a binding that is not being removed.
    pub(crate) fn slot_taken(&self, key: &TypeKey) -> bool {
        let scope = self.scope_for(key);
        self.0
            .shared
            .registry
            .lookup(key, scope.as_ref())
            .is_some_and(|existing| !existing.removing.load(Ordering::SeqCst))
    }

    fn provide_native<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: TypeKey,
        value: Arc<T>,
        check: Option<CheckFn>,
        mutable: bool,
    ) -> Result<(Disposer, Arc<Binding>), Error> {
        let scope = self.scope_for(&key);
        let event_key = key.clone();
        let event_scope = scope.clone();
        let shared = self.0.shared.clone();
        let label = format!("service provide: {}", key.describe());
        let inserted = Arc::new(Mutex::new(None));
        let inserted_slot = inserted.clone();
        let origin = crate::effect::Origin::Provide(key.clone());
        let disposer =
            self.register_internal_effect(label, origin, move |fiber, provider_gen, state| {
                // Admission and the fiber transition lock cover both the binding and
                // its cleanup record, so shutdown cannot miss a committed service.
                let binding = shared.registry.insert_binding(
                    key.clone(),
                    scope.clone(),
                    Binding {
                        value: if mutable {
                            ValueSlot::Mutable(Mutex::new(StoredValue::new(value)))
                        } else {
                            ValueSlot::Fixed(StoredValue::new(value))
                        },
                        provider: Arc::downgrade(fiber),
                        provider_id: fiber.id,
                        provider_gen,
                        check,
                        check_status: Mutex::new(None),
                        removing: AtomicBool::new(false),
                    },
                )?;
                *inserted_slot.lock().unwrap() = Some(binding);
                fiber
                    .provided
                    .lock()
                    .unwrap()
                    .push((key.clone(), scope.clone()));
                let cleanup_shared = shared.clone();
                let provider = Arc::downgrade(fiber);
                let pid = fiber.id;
                let evict_scope = scope.clone();
                let evict_key = key.clone();
                if state == FiberState::Active {
                    shared.registry.notify_key_changed(&key);
                }
                Ok(Effect::AsyncDisposer(Box::new(move || {
                    let shared = cleanup_shared.clone();
                    let provider = provider.clone();
                    let scope = evict_scope.clone();
                    Box::pin(async move {
                        evict_and_finalize(&shared, provider, pid, provider_gen, evict_key, scope)
                            .await
                    })
                })))
            })?;
        let binding = inserted
            .lock()
            .unwrap()
            .take()
            .expect("binding inserted before registration returns");
        self.emit_service_changed(
            &event_key,
            event_scope,
            &binding,
            crate::ServiceChange::Provided,
        );
        Ok((disposer, binding))
    }

    fn emit_service_changed(
        &self,
        key: &TypeKey,
        scope: Option<crate::key::ScopeId>,
        binding: &Binding,
        change: crate::ServiceChange,
    ) {
        let event = crate::ServiceChanged {
            key: key.clone(),
            scope: scope.as_ref().map(|s| s.to_string()),
            provider: binding.provider_id,
            generation: binding.provider_gen,
            change,
            chain: self.scope(),
            row: self.row(),
        };
        if let Err(error) = self.emit(event) {
            self.error_sink()(error.shared());
        }
    }

    /// Framework-owned registration: admission, state check, insertion and
    /// cleanup ownership commit at one synchronous point. `f` must not call
    /// user code or try to acquire the admission lock again.
    pub(crate) fn register_internal_effect_named(
        &self,
        label: String,
        f: impl FnOnce(&Arc<FiberInner>, u64, FiberState) -> Result<Effect, Error>,
    ) -> Result<Disposer, Error> {
        self.register_internal_effect(label, crate::effect::Origin::Other, f)
    }

    /// [`register_internal_effect_named`](Self::register_internal_effect_named),
    /// saying where the cleanup comes from.
    pub(crate) fn register_internal_effect(
        &self,
        label: String,
        origin: crate::effect::Origin,
        f: impl FnOnce(&Arc<FiberInner>, u64, FiberState) -> Result<Effect, Error>,
    ) -> Result<Disposer, Error> {
        let _admission = self.0.shared.admission.lock().unwrap();
        self.registration_open()?;
        let fiber = self.0.fiber.upgrade().ok_or(ErrorKind::InactiveEffect)?;
        let tr = fiber.transition.lock().unwrap();
        self.check_generation(tr.generation, tr.state)?;
        if matches!(tr.state, FiberState::Unloading | FiberState::Disposed) {
            return Err(ErrorKind::InactiveEffect.into());
        }
        let effect = f(&fiber, tr.generation, tr.state)?;
        let record = EffectRecord::from(effect, label, origin, self.0.fiber.clone());
        fiber.push_effect(record.clone());
        drop(tr);
        let handle = self.handle().clone();
        Ok(Disposer::new(Box::new(move || {
            let record = record.clone();
            let handle = handle.clone();
            Box::pin(async move { record.drain(&handle).await })
        })))
    }

    fn register_mount_effect(
        &self,
        child: &Arc<FiberInner>,
        make: impl FnOnce() -> Effect,
    ) -> Result<Arc<EffectRecord>, Error> {
        let _admission = self.0.shared.admission.lock().unwrap();
        self.registration_open()?;
        if child.closing.load(Ordering::SeqCst) || !child.alive.load(Ordering::SeqCst) {
            return Err(ErrorKind::Closed.into());
        }
        let parent = self.0.fiber.upgrade().ok_or(ErrorKind::InactiveEffect)?;
        let tr = parent.transition.lock().unwrap();
        self.check_generation(tr.generation, tr.state)?;
        if matches!(tr.state, FiberState::Unloading | FiberState::Disposed) {
            return Err(ErrorKind::InactiveEffect.into());
        }
        let record = EffectRecord::from(
            make(),
            format!("plugin mount: {} #{}", child.name, child.id.0),
            crate::effect::Origin::Mount,
            self.0.fiber.clone(),
        );
        parent.push_effect(record.clone());
        *child.mount.lock().unwrap() = Some(record.clone());
        Ok(record)
    }

    /// The disposer of a cleanup record: drains it early, once.
    pub(crate) fn record_disposer(&self, record: Arc<EffectRecord>) -> Disposer {
        let handle = self.handle().clone();
        Disposer::new(Box::new(move || {
            let record = record.clone();
            let handle = handle.clone();
            Box::pin(async move { record.drain(&handle).await })
        }))
    }

    /// Internal form of `effect`: returns the record itself, for callers that
    /// keep a reference to it (such as mount registration). The `Disposer`
    /// behaves as usual: dropping it does not run the cleanup, and the fiber's
    /// unload still runs it.
    pub(crate) fn register_effect_named(
        &self,
        label: String,
        origin: crate::effect::Origin,
        f: impl FnOnce() -> Effect,
    ) -> Result<Arc<EffectRecord>, Error> {
        let fiber = self.0.fiber.upgrade();
        if self.0.shared.closing.load(Ordering::SeqCst)
            || (fiber.is_none() && self.subtree_closing())
        {
            return Err(ErrorKind::Closed.into());
        }
        let fiber = fiber.ok_or(ErrorKind::InactiveEffect)?;
        let lease = {
            let _admission = self.0.shared.admission.lock().unwrap();
            let tr = fiber.transition.lock().unwrap();
            if self.0.shared.closing.load(Ordering::SeqCst)
                || (self.subtree_closing() && tr.state != FiberState::Loading)
            {
                return Err(ErrorKind::Closed.into());
            }
            self.check_generation(tr.generation, tr.state)?;
            if matches!(tr.state, FiberState::Unloading | FiberState::Disposed) {
                return Err(ErrorKind::InactiveEffect.into());
            }
            fiber.begin_event();
            EffectFactoryLease(fiber.clone())
        };
        // factory 锁外执行;但"状态检查 + effects 入队"必须在同一临界区
        //(transition → effects 嵌套,锁序无反向):否则检查通过后驱动恰好
        // 卸载取走 effects,新记录漏掉本轮清理而泄漏(评审 P1)
        let record = EffectRecord::from(f(), label, origin, self.0.fiber.clone());
        let handle = self.handle().clone();
        {
            let tr = fiber.transition.lock().unwrap();
            let stale = self.check_generation(tr.generation, tr.state).err();
            if stale.is_some() || matches!(tr.state, FiberState::Unloading | FiberState::Disposed) {
                drop(tr);
                // 生命周期已越过登记点:f() 可能已有副作用(如插入了监听器),
                // 立即排干该记录的清理并返失败
                let sink = self.error_sink();
                let drain_handle = handle.clone();
                handle.spawn(async move {
                    if let Err(e) = record.drain(&drain_handle).await {
                        sink(e);
                    }
                    drop(lease);
                });
                return Err(stale.unwrap_or(ErrorKind::InactiveEffect.into()));
            }
            fiber.push_effect(record.clone());
        }
        drop(lease);
        Ok(record)
    }

    /// Mounts a plugin and returns its [`FiberView`](crate::host::FiberView). The
    /// child belongs to this context's fiber: its dispose is registered as an
    /// effect of the parent, so it unloads with the parent.
    pub fn plugin(&self, p: impl DynPlugin) -> FiberView {
        // A copy of a plugin definition is recorded as it mounts, so it ends
        // with the definition's origin even while it waits for dependencies.
        let copy = crate::definition::copy_of(&p);
        let fiber = spawn_fiber(&self.0.shared, Some(self), Some(Arc::new(p)), false);
        let view = self.mount_fiber(fiber);
        if let Some(definition) = copy {
            definition.mounted(&view);
        }
        view
    }

    /// Record a plugin definition this context's generation made: false when
    /// the generation is already stopping (the definition ends at once).
    pub(crate) fn register_definition(&self, definition: Weak<crate::definition::Inner>) -> bool {
        let Some(fiber) = self.0.fiber.upgrade() else {
            return false;
        };
        let _admission = self.0.shared.admission.lock().unwrap();
        let tr = fiber.transition.lock().unwrap();
        if self.0.shared.closing.load(Ordering::SeqCst)
            || self.check_generation(tr.generation, tr.state).is_err()
            || matches!(tr.state, FiberState::Unloading | FiberState::Disposed)
        {
            return false;
        }
        fiber.definitions.lock().unwrap().push(definition);
        true
    }

    /// Finishes mounting: registers the cascading dispose and posts the first
    /// load. If the parent is no longer active, the child is disposed and never
    /// loads.
    fn mount_fiber(&self, fiber: std::sync::Arc<crate::fiber::FiberInner>) -> FiberView {
        let view = FiberView::from_inner(fiber.clone());
        if !fiber.alive.load(Ordering::SeqCst) {
            return view;
        }
        let child = view.clone();
        let sink = self.error_sink();
        let registered = self.register_mount_effect(&fiber, move || {
            Effect::AsyncDisposer(Box::new(move || {
                let child = child.clone();
                let sink = sink.clone();
                Box::pin(async move {
                    let closing_task = {
                        let _admission = child.inner.ctx.shared().admission.lock().unwrap();
                        child.inner.shutdown_inner.lock().unwrap().clone()
                    };
                    if let Some(task) = closing_task {
                        // Wait for the child's own cleanup, not its public
                        // shutdown result: the latter waits for this mount to
                        // detach and would form a parent/child cycle.
                        let _ = join_task(&task).await;
                        return Ok(());
                    }
                    // 级联 dispose:parent 卸载时子尚未处置,错误仅经 sink 可见;
                    // 子已 Disposed(调用方已从 dispose() 收到同一错误)则不再
                    // 重复上报——0.2.1 子终态退出也会 drain 本记录
                    let delivered = matches!(child.state().state, FiberState::Disposed);
                    if let Err(e) = child.dispose().await {
                        if !delivered {
                            sink(e);
                        }
                    }
                    Ok(())
                })
            }))
        });
        match registered {
            Ok(record) => {
                // Mount ownership was committed under the admission lock.
                drop(record);
                fiber.post(Intent::RefreshDeps);
            }
            Err(_) => {
                // parent 已失活:处置子 fiber,且不再触发装载(评审 #10:
                // 避免 Dispose 之后入队的重载意图无人处理)
                if fiber.closing.load(Ordering::SeqCst) {
                    // The subtree coordinator already owns the terminal intent.
                } else if self.0.shared.closing.load(Ordering::SeqCst) {
                    // Keep the orphan's terminal result available to its
                    // FiberView, even when root shutdown won the mount race.
                    drop(view.shutdown());
                } else {
                    fiber.post(Intent::Dispose);
                }
            }
        }
        view
    }

    /// Resolves when the current fiber generation is cancelled. Cancellation is
    /// cooperative: a plugin that never observes it can keep its unload, and so
    /// a `dispose`, waiting indefinitely.
    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + 'static {
        let token = self.cancellation_token();
        async move { token.cancelled().await }
    }
}

impl crate::host::CtxHost for Ctx {
    fn instance(&self) -> InstanceId {
        self.0.instance
    }

    fn handle(&self) -> &Handle {
        &self.0.shared.handle
    }

    fn error_sink(&self) -> ErrorSink {
        self.0.shared.error_sink.clone()
    }

    fn take_cleanup_errors(&self) -> Vec<Arc<ErrorKind>> {
        self.0
            .fiber
            .upgrade()
            .map(|fiber| std::mem::take(&mut *fiber.drained_errors.lock().unwrap()))
            .unwrap_or_default()
    }

    fn root() -> Result<Ctx, Error> {
        let handle = Handle::try_current().map_err(|_| {
            ErrorKind::PluginFailed(
                "no tokio runtime in scope; construct inside #[tokio::test]/runtime, or use Ctx::root_with(handle)".into(),
            )
        })?;
        Ok(Self::root_with_sink(handle, default_sink()))
    }

    fn root_with(handle: Handle) -> Ctx {
        Self::root_with_sink(handle, default_sink())
    }

    fn root_with_sink(handle: Handle, sink: ErrorSink) -> Ctx {
        let shared = Arc::new(Shared {
            admission: Mutex::new(()),
            handle,
            bus: EventBus::new(),
            registry: Registry::new(),
            interceptors: Arc::new(ServiceInterceptors::default()),
            error_sink: sink,
            next_plugin_id: AtomicU64::new(1),
            closing: AtomicBool::new(false),
            shutdown_task: Mutex::new(None),
            waiting_report: Mutex::new(Some(crate::fiber::waiting::DEFAULT_REPORT_AFTER)),
            key_namers: Mutex::new(Vec::new()),
        });
        let root_fiber = spawn_fiber(&shared, None, None, true);
        root_fiber.ctx.clone()
    }

    fn root_view(&self) -> Option<FiberView> {
        let mut current = self.clone();
        while let Some(parent) = current.0.parent.clone() {
            current = parent;
        }
        current.0.fiber.upgrade().map(FiberView::from_inner)
    }

    fn root_ctx(&self) -> Ctx {
        let mut current = self.clone();
        while let Some(parent) = current.0.parent.clone() {
            current = parent;
        }
        current
    }

    fn view(&self, id: crate::PluginId) -> Option<FiberView> {
        let root = self.root_view()?;
        let mut pending = vec![root.inner];
        while let Some(fiber) = pending.pop() {
            if fiber.id == id {
                return Some(FiberView::from_inner(fiber));
            }
            pending.extend(
                fiber
                    .children
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(Weak::upgrade),
            );
        }
        None
    }

    fn is_shutting_down(&self) -> bool {
        self.0.shared.closing.load(Ordering::SeqCst)
    }

    fn report_waiting_after(&self, after: Option<std::time::Duration>) {
        *self.0.shared.waiting_report.lock().unwrap() = after;
    }

    fn name_keys_with(&self, namer: &Arc<crate::host::KeyNamer>) {
        let mut namers = self.0.shared.key_namers.lock().unwrap();
        namers.retain(|namer| namer.strong_count() > 0);
        namers.push(Arc::downgrade(namer));
    }

    fn diagnostics(&self) -> RuntimeDiagnostics {
        let mut plugins = Vec::new();
        if let Some(root) = self.root_view() {
            let mut pending = vec![root.inner];
            while let Some(fiber) = pending.pop() {
                pending.extend(
                    fiber
                        .children
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(Weak::upgrade),
                );
                let snapshot = fiber.state_snapshot();
                let injects = fiber
                    .declared_injects
                    .iter()
                    .map(|key| {
                        let scope = fiber.ctx.scope_for(key);
                        let status = if !fiber.ctx.in_instance_key(key) {
                            DependencyStatus::OutOfScope
                        } else {
                            self.0
                                .shared
                                .registry
                                .dependency_status(key, scope.as_ref())
                        };
                        DependencyDiagnostics {
                            key: key.clone(),
                            scope: scope.as_ref().map(|s| s.to_string()),
                            status,
                        }
                    })
                    .collect();
                let resolved_dependencies = fiber
                    .last_deps
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|deps| {
                        deps.iter()
                            .map(|(provider, generation, key, scope)| ResolvedDependency {
                                key: key.clone(),
                                scope: scope.as_ref().map(|s| s.to_string()),
                                provider: *provider,
                                generation: *generation,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                plugins.push(PluginDiagnostics {
                    id: fiber.id,
                    instance: fiber.instance,
                    parent: fiber
                        .parent_fiber
                        .as_ref()
                        .and_then(Weak::upgrade)
                        .map(|p| p.id),
                    name: fiber.name.clone(),
                    row: fiber.row.get().map(ToString::to_string),
                    platform: fiber.platform,
                    state: snapshot.state,
                    generation: snapshot.generation,
                    error: snapshot.error,
                    injects,
                    resolved_dependencies,
                    accesses: fiber.accesses.lock().unwrap().clone(),
                });
            }
        }
        RuntimeDiagnostics {
            shutting_down: self.is_shutting_down(),
            plugins,
            bindings: self.0.shared.registry.bindings_snapshot(),
            event_backlogs: self.events().backlogs(),
            // Asked for names only when it is displayed.
            key_names: crate::diagnostics::KeyNames(crate::fiber::waiting::namers(&self.0.shared)),
        }
    }

    fn shutdown(&self) -> crate::BoxFuture<'static, Result<(), Arc<ErrorKind>>> {
        let _admission = self.0.shared.admission.lock().unwrap();
        let task = {
            let mut slot = self.0.shared.shutdown_task.lock().unwrap();
            if let Some(task) = slot.as_ref() {
                task.clone()
            } else {
                let task = TransitionTask::new();
                *slot = Some(task.clone());
                self.0.shared.closing.store(true, Ordering::SeqCst);
                if let Some(root) = self.root_view() {
                    // Like subtree shutdown: descendants are cancelled before
                    // the root waits for its instance events to drain.
                    root.inner.cancel_current();
                    root.inner.cancel_descendants();
                    root.inner.post(Intent::Shutdown(task.clone()));
                } else {
                    task.complete(Some(Arc::new(ErrorKind::Closed)));
                }
                task
            }
        };
        Box::pin(async move { join_task(&task).await })
    }

    fn shutdown_with_timeout(
        &self,
        limit: Duration,
    ) -> crate::BoxFuture<'static, Result<(), DisposeWaitError>> {
        let root = self.root_view();
        let pending = self.shutdown();
        let Some(root) = root else {
            // No root view remains only after its driver has exited; the
            // cached shutdown result is already available without a deadline.
            return Box::pin(async move { pending.await.map_err(DisposeWaitError::Failed) });
        };
        Box::pin(async move {
            let started = Instant::now();
            match tokio::time::timeout(limit, pending).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(DisposeWaitError::Failed(error)),
                Err(_) => {
                    let snapshot = root.state();
                    Err(DisposeWaitError::TimedOut {
                        plugin_id: root.id,
                        generation: snapshot.generation,
                        state: snapshot.state,
                        elapsed: started.elapsed(),
                        stuck: crate::fiber::stuck_below(&root.inner),
                    })
                }
            }
        })
    }

    fn scope(&self) -> Vec<crate::Scope> {
        let mut chain = Vec::new();
        let mut current = self.0.fiber.upgrade();
        while let Some(fiber) = current {
            if let Some(group) = fiber.scope_group.get() {
                chain.push(crate::Scope {
                    id: fiber.instance.to_string(),
                    group: group.to_string(),
                });
            }
            current = fiber.parent_fiber.as_ref().and_then(Weak::upgrade);
        }
        chain.reverse();
        chain
    }

    fn row(&self) -> Option<String> {
        let mut current = self.0.fiber.upgrade();
        while let Some(fiber) = current {
            if let Some(row) = fiber.row.get() {
                return Some(row.to_string());
            }
            current = fiber.parent_fiber.as_ref().and_then(Weak::upgrade);
        }
        None
    }

    fn isolate_label(&self, key: &TypeKey) -> Option<String> {
        self.scope_for(key).map(|scope| scope.to_string())
    }

    fn isolate(&self, key: impl Into<TypeKey>, label: &str) -> Ctx {
        let isolated = Ctx::new_child(
            self.0.shared.clone(),
            self,
            self.0.fiber.clone(),
            Some((key.into(), Arc::from(label))),
            self.instance(),
            self.0.plugin_id,
            self.0.closing.clone(),
        );
        match self.0.generation {
            Some(generation) => {
                isolated.for_generation_with_token(generation, self.cancellation_token())
            }
            None => isolated,
        }
    }

    fn plugin_with<C: Send + Sync + 'static>(
        &self,
        factory: impl PluginFactory<C>,
        config: C,
    ) -> FiberView {
        let fiber =
            crate::fiber::spawn_factory_fiber(&self.0.shared, Some(self), factory, config, false);
        self.mount_fiber(fiber)
    }

    fn dispose_self(&self) -> Result<(), Error> {
        let fiber = self.0.fiber.upgrade().ok_or(ErrorKind::InactiveEffect)?;
        if fiber.is_root {
            return Err(ErrorKind::Validation {
                issues: vec!["the root disposes through shutdown".into()],
            }
            .into());
        }
        // Registration happens in the call; the join future is not needed.
        FiberView::from_inner(fiber).register_dispose(|tr| {
            let stale = self.0.generation.is_some_and(|g| g != tr.generation);
            // A restart cancels the generation's token before it unloads.
            if stale
                || self.cancellation_token().is_cancelled()
                || matches!(tr.state, FiberState::Unloading | FiberState::Disposed)
            {
                return Err(ErrorKind::InactiveEffect.into());
            }
            Ok(())
        })?;
        Ok(())
    }

    fn cancellation_token(&self) -> CancellationToken {
        if let Some(token) = &self.0.generation_token {
            return token.clone();
        }
        match self.0.fiber.upgrade() {
            Some(fiber) => fiber.current_token(),
            // fiber 已析构 ≡ 代已结束:返回预取消 token,cancelled() 不永等(评审 P2)
            None => {
                let token = CancellationToken::new();
                token.cancel();
                token
            }
        }
    }

    fn provide_at<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: Key<T>,
        value: Arc<T>,
    ) -> Result<Disposer, Error> {
        self.provide_key::<T>(key.erase(), value)
    }

    fn get_at<T: ?Sized + Send + Sync + 'static>(&self, key: &Key<T>) -> Option<Arc<T>> {
        self.get_key::<T>(key.erase())
    }

    #[track_caller]
    fn require<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>, ServiceReadError> {
        self.require_key::<T>(TypeKey::of::<T>())
    }

    #[track_caller]
    fn require_at<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: &Key<T>,
    ) -> Result<Arc<T>, ServiceReadError> {
        self.require_key::<T>(key.erase())
    }

    fn plugin_from<C, P, B>(&self, build: B, config: C) -> FiberView
    where
        C: Send + Sync + 'static,
        P: Plugin,
        B: Fn(&C) -> Result<P, Error> + Send + Sync + 'static,
    {
        self.plugin_with(crate::plugin::BuildFn::new(build), config)
    }

    fn subscriptions(&self) -> Vec<crate::bus::EventSubscription> {
        self.events().subscriptions()
    }
}

impl crate::internal::CtxInternal for Ctx {
    fn in_instance_key(&self, key: &TypeKey) -> bool {
        key.instance_id().is_none_or(|id| self.in_instance(id))
    }

    fn events(&self) -> &EventBus {
        &self.0.shared.bus
    }

    fn mark_scope(&self, group: &str) -> Result<(), Error> {
        let fiber = self.0.fiber.upgrade().ok_or(ErrorKind::InactiveEffect)?;
        if fiber.is_root {
            return Err(ErrorKind::Validation {
                issues: vec!["the root is not a scope of a group".into()],
            }
            .into());
        }
        let marked = fiber.scope_group.get_or_init(|| Arc::from(group));
        if &**marked != group {
            return Err(ErrorKind::Validation {
                issues: vec![format!(
                    "this fiber is already a scope of group {marked}, not {group}"
                )],
            }
            .into());
        }
        Ok(())
    }

    fn mark_row(&self, key: &str) -> Result<(), Error> {
        let fiber = self.0.fiber.upgrade().ok_or(ErrorKind::InactiveEffect)?;
        let marked = fiber.row.get_or_init(|| Arc::from(key));
        if &**marked != key {
            return Err(ErrorKind::Validation {
                issues: vec![format!("this fiber is already the row {marked}, not {key}")],
            }
            .into());
        }
        Ok(())
    }

    fn get_key<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
    ) -> Option<Arc<T>> {
        let key = key.into();
        let scope = self.scope_for(&key);
        let out_of_scope = !self.in_instance_key(&key);
        let (result, found) = if out_of_scope {
            (None, None)
        } else {
            let binding = self.0.shared.registry.lookup(&key, scope.as_ref());
            self.read_binding_as::<T>(binding.as_ref())
        };
        if let Some(fiber) = self.0.fiber.upgrade() {
            if fiber.state() == FiberState::Loading {
                self.record_access(
                    &fiber,
                    &key,
                    scope.as_ref(),
                    AccessOutcome {
                        found,
                        declared: fiber.declared_injects.contains(&key),
                        out_of_scope,
                        strict: false,
                        failure: None,
                    },
                );
            }
        }
        result
    }

    #[track_caller]
    fn require_key<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
    ) -> Result<Arc<T>, ServiceReadError> {
        self.require_inner(key.into(), std::panic::Location::caller(), None, None)
    }

    #[track_caller]
    fn require_provided_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        provision: &crate::service::Provision,
        actor: Option<&Ctx>,
    ) -> Result<Arc<T>, ServiceReadError> {
        self.require_inner(
            key.into(),
            std::panic::Location::caller(),
            Some(provision),
            actor,
        )
    }

    fn provide_key<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        value: Arc<T>,
    ) -> Result<Disposer, Error> {
        self.provide_inner(key.into(), value, None, false)
            .map(|(disposer, _)| disposer)
    }

    fn provide_checked<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        value: Arc<T>,
        check: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Disposer, Error> {
        self.provide_inner(key.into(), value, Some(Arc::new(check)), false)
            .map(|(disposer, _)| disposer)
    }

    fn provide_mut_at<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        value: Arc<T>,
    ) -> Result<(Disposer, ServiceWriter<T>), Error> {
        let key = key.into();
        let scope = self.scope_for(&key);
        let (disposer, binding) = self.provide_inner(key.clone(), value, None, true)?;
        let writer = ServiceWriter::new(key, scope, binding, self.0.shared.clone());
        Ok((disposer, writer))
    }

    fn effect_raw(&self, f: impl FnOnce() -> Effect) -> Result<Disposer, Error> {
        self.effect_named("anonymous", f)
    }

    fn effect_named(
        &self,
        label: impl Into<String>,
        f: impl FnOnce() -> Effect,
    ) -> Result<Disposer, Error> {
        let record = self.register_effect_named(label.into(), crate::effect::Origin::Other, f)?;
        Ok(self.record_disposer(record))
    }

    fn refresh(&self) {
        self.0.shared.registry.refresh_all();
    }

    fn try_filter(
        &self,
        keep: impl Fn(&crate::events::ListenerInfo) -> Result<bool, Error> + Send + Sync + 'static,
    ) -> crate::events::Filtered<'_> {
        crate::events::Filtered::new(self, Arc::new(keep))
    }

    fn named(&self, name: impl Into<Arc<str>>) -> crate::events::Filtered<'_> {
        crate::events::Filtered::named(self, name.into())
    }

    fn intercept_require_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        hook: impl Fn(Arc<T>) -> crate::intercept::ServiceIntercept<T> + Send + Sync + 'static,
    ) -> Result<Disposer, Error> {
        self.intercept_require_impl(key, hook)
    }

    fn intercept_set_as<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        hook: impl Fn(Arc<T>) -> crate::intercept::ServiceIntercept<T> + Send + Sync + 'static,
    ) -> Result<Disposer, Error> {
        self.intercept_set_impl(key, hook)
    }

    fn try_filter_named(
        &self,
        name: impl Into<Arc<str>>,
        keep: impl Fn(&crate::events::ListenerInfo) -> Result<bool, Error> + Send + Sync + 'static,
    ) -> crate::events::Filtered<'_> {
        crate::events::Filtered::filtered_named(self, name.into(), Arc::new(keep))
    }
}

/// Withdraws a service: (1) marks this binding as being removed, so strict
/// reads fail at once while the provider's cleanup can still read it;
/// (2) cancels its consumers and has each re-check its dependencies, joining
/// them (a consumer whose driver has exited completes at once, so the join
/// does not hang); (3) once they have drained, removes the binding and
/// reports the removal, only if the slot still holds this binding: a slot a
/// new provide took over in the meantime is left alone, as TypeScript's
/// dispose frees the slot at once; (4) removes the entry from the provider's
/// `provided` list, so repeated provide/dispose on a long-lived root does
/// not accumulate.
async fn evict_and_finalize(
    shared: &Arc<Shared>,
    provider: Weak<FiberInner>,
    pid: crate::PluginId,
    provider_gen: u64,
    key: TypeKey,
    scope: Option<crate::key::ScopeId>,
) -> Result<(), Error> {
    let old = shared
        .registry
        .lookup(&key, scope.as_ref())
        .filter(|b| b.provider_id == pid && b.provider_gen == provider_gen);
    if let Some(binding) = &old {
        shared
            .registry
            .mark_removing_if(&key, scope.as_ref(), binding);
    }
    let consumers: Vec<Arc<FiberInner>> = shared
        .registry
        .consumers_of(&key, (pid, provider_gen, key.clone(), scope.clone()));
    let mut tasks = Vec::new();
    for fiber in &consumers {
        if fiber.id == pid {
            continue;
        }
        fiber.cancel_current();
        if fiber.closing.load(Ordering::SeqCst) {
            let task = {
                let _admission = shared.admission.lock().unwrap();
                fiber.shutdown_inner.lock().unwrap().clone()
            };
            if let Some(task) = task {
                tasks.push(task);
            }
            continue;
        }
        let task = TransitionTask::new();
        // post_join 返回 false 时任务已在 post 内即刻完成(评审 #3)
        fiber.post_join(task.clone(), Intent::RefreshDepsJoin);
        tasks.push(task);
    }
    // 其它注入该键的 fiber(Pending 者)也重查(不取消:可能是等值合并)
    shared.registry.notify_key_changed(&key);
    for task in tasks {
        let _ = join_task(&task).await;
    }
    // 清理期自访问结束,最终摘除(仅当槽位未被替换)
    if let Some(binding) = old {
        let removed = shared
            .registry
            .finalize_binding_if(key.clone(), scope.clone(), &binding);
        if removed {
            if let Some(fiber) = provider.upgrade() {
                fiber.ctx.emit_service_changed(
                    &key,
                    scope.clone(),
                    &binding,
                    crate::ServiceChange::Removed,
                );
            }
        }
    }
    // ④摘除 provider 的 provided 记账:仅移除本键本作用域的一条(同键
    // 新 provide 的条目保留;fiber 卸载整表清空后此处自然 no-op)
    if let Some(fiber) = provider.upgrade() {
        let mut provided = fiber.provided.lock().unwrap();
        if let Some(pos) = provided.iter().position(|(k, s)| *k == key && *s == scope) {
            provided.swap_remove(pos);
        }
        // 稀疏即收缩(0.2.5):长寿 root 的瞬态记账容量不随历史 provide 滞留
        //(常驻键使表永不为空,按浪费率收缩)。
        if provided.capacity() > 64 && provided.len() * 4 < provided.capacity() {
            provided.shrink_to_fit();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ApplyProbe(Arc<AtomicUsize>);

    impl DynPlugin for ApplyProbe {
        fn name(&self) -> &str {
            "apply-probe"
        }

        fn apply<'a>(&'a self, _ctx: &'a Ctx) -> crate::BoxFuture<'a, Result<Effect, Error>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Effect::Done) })
        }
    }

    /// Outside a runtime, `Ctx::root()` reports that there is none and does
    /// not make one up: afterwards this thread still has no runtime.
    ///
    /// guarantee: core/D8
    #[test]
    fn root_without_a_runtime_is_an_explicit_error() {
        assert!(
            Handle::try_current().is_err(),
            "test runs outside a runtime"
        );
        let error = match Ctx::root() {
            Ok(_) => panic!("Ctx::root() outside a runtime must fail"),
            Err(error) => error,
        };
        assert!(
            matches!(&*error, ErrorKind::PluginFailed(_)),
            "unexpected error: {error:?}"
        );
        let message = error.to_string();
        assert!(message.contains("no tokio runtime"), "{message}");
        assert!(message.contains("Ctx::root_with"), "{message}");
        assert!(
            Handle::try_current().is_err(),
            "Ctx::root() must not enter or create a runtime"
        );
    }

    #[tokio::test]
    async fn child_created_before_root_closes_but_mounted_afterward_never_applies() {
        let ctx = Ctx::root().unwrap();
        let root_view = ctx.root_view().unwrap();
        let applies = Arc::new(AtomicUsize::new(0));
        // Split the public plugin path at its two admission points to force
        // the otherwise narrow spawn/mount race without timing sleeps.
        let fiber = spawn_fiber(
            ctx.shared(),
            Some(&ctx),
            Some(Arc::new(ApplyProbe(applies.clone()))),
            false,
        );
        let root_shutdown = ctx.shutdown();
        let child = ctx.mount_fiber(fiber);
        root_shutdown.await.unwrap();
        child.shutdown().await.unwrap();
        assert_eq!(applies.load(Ordering::SeqCst), 0);
        assert_eq!(child.state().state, FiberState::Disposed);
        assert!(child.inner.effects.lock().unwrap().is_empty());
        assert!(child.inner.driver.lock().unwrap().is_none());
        assert!(ctx.shared().registry.bindings_snapshot().is_empty());
        assert!(root_view.inner.children.lock().unwrap().is_empty());
    }
}
