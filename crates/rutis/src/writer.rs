use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe, Location};
use std::sync::Arc;

use crate::ctx::{Ctx, Shared};
use crate::error::{panic_error, ServiceWriteError, ServiceWriteFailure};
use crate::fiber::FiberState;
use crate::host::CtxHost;
use crate::internal::CtxInternal;
use crate::key::{ScopeId, TypeKey};
use crate::registry::{Binding, StoredValue};

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
        let value = StoredValue::new(value);
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
