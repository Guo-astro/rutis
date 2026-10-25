use crate::host::CtxHost;
use crate::internal::CtxInternal;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe, Location};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::ctx::{Ctx, Shared};
use crate::effect::{Disposer, Effect};
use crate::error::{
    panic_error, Error, ErrorKind, ServiceReadFailure, ServiceWriteError, ServiceWriteFailure,
};
use crate::fiber::{FiberInner, FiberState};
use crate::key::{ScopeId, TypeKey};
use crate::registry::{Binding, StoredValue};

/// A trusted interceptor can continue, replace a same-type value, or deny an
/// operation. The type alone cannot prove which instance created `Arc<T>`.
pub enum ServiceIntercept<T: ?Sized> {
    Continue,
    Replace(Arc<T>),
    Deny,
}

enum ErasedDecision {
    Continue,
    Replace(StoredValue),
    Deny,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HookKind {
    Read,
    Write,
}

type HookKey = (TypeKey, Option<ScopeId>);
type HookCall = dyn Fn(&StoredValue) -> ErasedDecision + Send + Sync;

struct Hook {
    call: Arc<HookCall>,
    owner: Weak<FiberInner>,
}

#[derive(Default)]
pub(crate) struct ServiceInterceptors {
    reads: Mutex<HashMap<HookKey, Vec<Arc<Hook>>>>,
    writes: Mutex<HashMap<HookKey, Vec<Arc<Hook>>>>,
    read_count: AtomicUsize,
    write_count: AtomicUsize,
}

impl ServiceInterceptors {
    fn count(&self, kind: HookKind) -> &AtomicUsize {
        match kind {
            HookKind::Read => &self.read_count,
            HookKind::Write => &self.write_count,
        }
    }

    fn has_any(&self, kind: HookKind) -> bool {
        self.count(kind).load(Ordering::SeqCst) != 0
    }

    fn table(&self, kind: HookKind) -> &Mutex<HashMap<HookKey, Vec<Arc<Hook>>>> {
        match kind {
            HookKind::Read => &self.reads,
            HookKind::Write => &self.writes,
        }
    }

    fn insert(&self, kind: HookKind, key: HookKey, hook: Arc<Hook>) {
        self.table(kind)
            .lock()
            .unwrap()
            .entry(key)
            .or_default()
            .push(hook);
        self.count(kind).fetch_add(1, Ordering::SeqCst);
    }

    fn remove(&self, kind: HookKind, key: &HookKey, hook: &Arc<Hook>) {
        let mut table = self.table(kind).lock().unwrap();
        if let Some(list) = table.get_mut(key) {
            let before = list.len();
            list.retain(|entry| !Arc::ptr_eq(entry, hook));
            self.count(kind)
                .fetch_sub(before - list.len(), Ordering::SeqCst);
            if list.is_empty() {
                table.remove(key);
            }
        }
        if table.capacity() > 64 && table.len() * 4 < table.capacity() {
            table.shrink_to_fit();
        }
    }

    fn select(&self, kind: HookKind, key: &HookKey, actor: &Ctx) -> (Vec<Arc<Hook>>, HookFlight) {
        let _admission = actor.shared().admission.lock().unwrap();
        let mut ancestry = Vec::new();
        let mut current = actor.weak_fiber().upgrade();
        while let Some(fiber) = current {
            current = fiber.parent_fiber.as_ref().and_then(Weak::upgrade);
            ancestry.push(fiber);
        }
        let mut owners = Vec::new();
        if let Some(actor) = ancestry.first() {
            owners.push(actor.clone());
        }
        let hooks = self
            .table(kind)
            .lock()
            .unwrap()
            .get(key)
            .into_iter()
            .flat_map(|list| list.iter())
            .filter_map(|hook| {
                let owner = hook.owner.upgrade()?;
                if !owner.alive.load(Ordering::SeqCst)
                    || owner.closing.load(Ordering::SeqCst)
                    || !ancestry.iter().any(|fiber| Arc::ptr_eq(fiber, &owner))
                {
                    return None;
                }
                owners.push(owner);
                Some(hook.clone())
            })
            .collect();
        (hooks, HookFlight::new(owners))
    }

    fn run(
        &self,
        kind: HookKind,
        key: HookKey,
        actor: &Ctx,
        mut value: StoredValue,
    ) -> Result<StoredValue, HookFailure> {
        if !self.has_any(kind) {
            return Ok(value);
        }
        let (hooks, _flight) = self.select(kind, &key, actor);
        if hooks.is_empty() {
            return Ok(value);
        }
        let _guard = ReentryGuard::enter(kind, key)?;
        for hook in hooks {
            match catch_unwind(AssertUnwindSafe(|| (hook.call)(&value))) {
                Ok(ErasedDecision::Continue) => {}
                Ok(ErasedDecision::Replace(replacement)) => value = replacement,
                Ok(ErasedDecision::Deny) => return Err(HookFailure::Denied),
                Err(_) => return Err(HookFailure::Panicked),
            }
        }
        Ok(value)
    }

    #[cfg(test)]
    pub(crate) fn table_counts(&self) -> (usize, usize) {
        (
            self.reads.lock().unwrap().len(),
            self.writes.lock().unwrap().len(),
        )
    }
}

struct HookFlight(Vec<Arc<FiberInner>>);

impl HookFlight {
    fn new(mut owners: Vec<Arc<FiberInner>>) -> Self {
        owners.sort_by_key(|fiber| fiber.id);
        owners.dedup_by_key(|fiber| fiber.id);
        for fiber in &owners {
            fiber.begin_event();
        }
        Self(owners)
    }
}

impl Drop for HookFlight {
    fn drop(&mut self) {
        for fiber in &self.0 {
            fiber.finish_event();
        }
    }
}

#[derive(Clone, Copy)]
enum HookFailure {
    Denied,
    Panicked,
    Reentrant,
}

thread_local! {
    static ACTIVE_HOOKS: RefCell<Vec<(HookKind, HookKey)>> = const { RefCell::new(Vec::new()) };
}

struct ReentryGuard;

impl ReentryGuard {
    fn enter(kind: HookKind, key: HookKey) -> Result<Self, HookFailure> {
        ACTIVE_HOOKS.with(|active| {
            let mut active = active.borrow_mut();
            if active.iter().any(|entry| entry.0 == kind && entry.1 == key) {
                return Err(HookFailure::Reentrant);
            }
            active.push((kind, key));
            Ok(Self)
        })
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        ACTIVE_HOOKS.with(|active| {
            active.borrow_mut().pop();
        });
    }
}

impl Ctx {
    /// [`CtxInternal::intercept_require_as`](crate::internal::CtxInternal::intercept_require_as).
    pub(crate) fn intercept_require_impl<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        hook: impl Fn(Arc<T>) -> ServiceIntercept<T> + Send + Sync + 'static,
    ) -> Result<Disposer, Error> {
        self.register_service_interceptor(key.into(), HookKind::Read, hook)
    }

    /// [`CtxInternal::intercept_set_as`](crate::internal::CtxInternal::intercept_set_as).
    pub(crate) fn intercept_set_impl<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: impl Into<TypeKey>,
        hook: impl Fn(Arc<T>) -> ServiceIntercept<T> + Send + Sync + 'static,
    ) -> Result<Disposer, Error> {
        self.register_service_interceptor(key.into(), HookKind::Write, hook)
    }

    fn register_service_interceptor<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: TypeKey,
        kind: HookKind,
        callback: impl Fn(Arc<T>) -> ServiceIntercept<T> + Send + Sync + 'static,
    ) -> Result<Disposer, Error> {
        self.registration_preflight()?;
        self.check_instance(&key)?;
        if !key.has_type::<T>() {
            return Err(ErrorKind::Validation {
                issues: vec![format!(
                    "interceptor type does not match {}",
                    key.describe()
                )],
            }
            .into());
        }
        let scoped_key = (key.clone(), self.scope_for(&key));
        let call = move |value: &StoredValue| {
            let typed = value
                .downcast::<T>()
                .expect("interceptor key type checked at registration");
            match callback(typed) {
                ServiceIntercept::Continue => ErasedDecision::Continue,
                ServiceIntercept::Replace(value) => {
                    ErasedDecision::Replace(StoredValue::new(value))
                }
                ServiceIntercept::Deny => ErasedDecision::Deny,
            }
        };
        let hook = Arc::new(Hook {
            call: Arc::new(call),
            owner: self.weak_fiber(),
        });
        let shared = self.shared().clone();
        let label = match kind {
            HookKind::Read => format!("service read interceptor: {}", key.describe()),
            HookKind::Write => format!("service write interceptor: {}", key.describe()),
        };
        self.register_internal_effect_named(label, move |_, _, _| {
            shared
                .interceptors
                .insert(kind, scoped_key.clone(), hook.clone());
            Ok(Effect::AsyncDisposer(Box::new(move || {
                {
                    let _admission = shared.admission.lock().unwrap();
                    shared.interceptors.remove(kind, &scoped_key, &hook);
                }
                Box::pin(async move {
                    if let Some(owner) = hook.owner.upgrade() {
                        owner.wait_events().await;
                    }
                    Ok(())
                })
            })))
        })
    }

    pub(crate) fn apply_read_interceptors<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: &TypeKey,
        scope: Option<&ScopeId>,
        value: Arc<T>,
    ) -> Result<Arc<T>, ServiceReadFailure> {
        if !self.shared().interceptors.has_any(HookKind::Read) {
            return Ok(value);
        }
        let stored = self
            .shared()
            .interceptors
            .run(
                HookKind::Read,
                (key.clone(), scope.cloned()),
                self,
                StoredValue::new(value),
            )
            .map_err(|failure| match failure {
                HookFailure::Denied => ServiceReadFailure::InterceptDenied,
                HookFailure::Panicked => ServiceReadFailure::InterceptPanicked,
                HookFailure::Reentrant => ServiceReadFailure::InterceptReentrant,
            })?;
        Ok(stored
            .downcast::<T>()
            .expect("interceptor preserves value type"))
    }
}

/// Generation-bound authority to update one mutable service binding.
pub struct ServiceWriter<T: ?Sized + Send + Sync + 'static> {
    key: TypeKey,
    scope: Option<ScopeId>,
    binding: Arc<Binding>,
    shared: Arc<Shared>,
    _marker: PhantomData<fn() -> Arc<T>>,
}

impl<T: ?Sized + Send + Sync + 'static> ServiceWriter<T> {
    pub(crate) fn new(
        key: TypeKey,
        scope: Option<ScopeId>,
        binding: Arc<Binding>,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
            key,
            scope,
            binding,
            shared,
            _marker: PhantomData,
        }
    }

    /// Replace the value owned by the provider fiber. Existing `Arc<T>`
    /// snapshots keep their old value; future reads see the new one.
    #[track_caller]
    pub fn set(&self, caller: &Ctx, value: Arc<T>) -> Result<(), ServiceWriteError> {
        let location = Location::caller();
        let error = |reason| ServiceWriteError {
            key: self.key.clone(),
            provider: self.binding.provider_id,
            generation: self.binding.provider_gen,
            location,
            reason,
        };
        if !Arc::ptr_eq(caller.shared(), &self.shared) {
            return Err(error(ServiceWriteFailure::WrongOwner));
        }
        let provider = self
            .binding
            .provider
            .upgrade()
            .ok_or_else(|| error(ServiceWriteFailure::Stale))?;
        let actor = caller
            .weak_fiber()
            .upgrade()
            .ok_or_else(|| error(ServiceWriteFailure::WrongOwner))?;
        if !Arc::ptr_eq(&actor, &provider)
            || caller.scope_for(&self.key).as_ref() != self.scope.as_ref()
            || !caller.in_instance_key(&self.key)
        {
            return Err(error(ServiceWriteFailure::WrongOwner));
        }
        caller
            .registration_preflight()
            .map_err(|_| error(ServiceWriteFailure::Stale))?;
        let value = self
            .shared
            .interceptors
            .run(
                HookKind::Write,
                (self.key.clone(), self.scope.clone()),
                caller,
                StoredValue::new(value),
            )
            .map_err(|failure| {
                error(match failure {
                    HookFailure::Denied => ServiceWriteFailure::InterceptDenied,
                    HookFailure::Panicked => ServiceWriteFailure::InterceptPanicked,
                    HookFailure::Reentrant => ServiceWriteFailure::InterceptReentrant,
                })
            })?;
        // Locked section: admission → transition → registry bindings.
        // On success we return the old value; on failure we return the
        // *candidate* value so it is dropped outside every framework lock.
        // This prevents Drop reentry deadlocks (PR #55 P1).
        let (owned, result) = (|| -> (StoredValue, Result<(), ServiceWriteError>) {
            let _admission = self.shared.admission.lock().unwrap();
            if caller.registration_open().is_err() {
                return (value, Err(error(ServiceWriteFailure::Stale)));
            }
            let transition = provider.transition.lock().unwrap();
            if transition.generation != self.binding.provider_gen
                || !matches!(transition.state, FiberState::Loading | FiberState::Active)
            {
                return (value, Err(error(ServiceWriteFailure::Stale)));
            }
            match self.shared.registry.replace_mutable_if_current(
                &self.key,
                self.scope.as_ref(),
                &self.binding,
                value,
            ) {
                Ok(old) => (old, Ok(())),
                Err(candidate) => (candidate, Err(error(ServiceWriteFailure::Stale))),
            }
        })();
        // Drop the owned value (old on success, candidate on failure) outside
        // every lock. The last Arc may run user Drop code — panic is caught
        // and reported via the error sink.
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(owned))) {
            let sink = caller.error_sink();
            let error = panic_error(panic).shared();
            let _ = catch_unwind(AssertUnwindSafe(|| sink(error)));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoxFuture, DynPlugin};

    struct HookPlugin;

    impl DynPlugin for HookPlugin {
        fn name(&self) -> &str {
            "hook-churn"
        }

        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                let key = TypeKey::instance::<u64>(ctx.instance());
                ctx.intercept_require_as::<u64>(key.clone(), |_| ServiceIntercept::Continue)?;
                ctx.intercept_set_as::<u64>(key, |_| ServiceIntercept::Continue)?;
                Ok(Effect::Done)
            })
        }
    }

    #[tokio::test]
    async fn repeated_hook_disposal_reclaims_tables() {
        let root = Ctx::root().unwrap();
        let key = TypeKey::of::<u64>();
        for _ in 0..1000 {
            let read = root
                .intercept_require_as::<u64>(key.clone(), |_| ServiceIntercept::Continue)
                .unwrap();
            let write = root
                .intercept_set_as::<u64>(key.clone(), |_| ServiceIntercept::Continue)
                .unwrap();
            assert_eq!(root.shared().interceptors.table_counts(), (1, 1));
            read.dispose().await.unwrap();
            write.dispose().await.unwrap();
            assert_eq!(root.shared().interceptors.table_counts(), (0, 0));
        }
    }

    #[tokio::test]
    async fn thousand_child_shutdowns_reclaim_instance_hook_keys() {
        let root = Ctx::root().unwrap();
        for _ in 0..1000 {
            let child = root.plugin(HookPlugin);
            (&child).await.unwrap();
            assert_eq!(root.shared().interceptors.table_counts(), (1, 1));
            child.shutdown().await.unwrap();
            assert_eq!(root.shared().interceptors.table_counts(), (0, 0));
        }
    }
}
