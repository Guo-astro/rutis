use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use crate::diagnostics::{BindingDiagnostics, DependencyStatus};
use crate::error::{Error, ErrorKind};
use crate::fiber::{FiberInner, FiberState, Intent, PluginId};
use crate::key::{ScopeId, TypeKey};

pub(crate) type CheckFn = Arc<dyn Fn() -> bool + Send + Sync>;

/// Type-erased service value. The value is kept as an `Arc<T>` behind `dyn Any`,
/// so `T` may be unsized (trait-object registrations).
#[derive(Clone)]
pub(crate) struct StoredValue(Arc<Box<dyn Any + Send + Sync>>);

impl StoredValue {
    pub(crate) fn new<T: ?Sized + Send + Sync + 'static>(value: Arc<T>) -> Self {
        Self(Arc::new(Box::new(value) as Box<dyn Any + Send + Sync>))
    }

    pub(crate) fn downcast<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        // Box<dyn Any> 擦除的具体类型是内层 Arc<T>(不是 Box<Arc<T>>)
        (**self.0).downcast_ref::<Arc<T>>().cloned()
    }
}

/// A service binding. The registry stores `Arc<Binding>`, so every observer
/// (lookup, dependency resolution, removal) shares the same instance and sees
/// `removing` as soon as it is set.
pub(crate) struct Binding {
    pub value: ValueSlot,
    pub provider: Weak<FiberInner>,
    pub provider_id: PluginId,
    pub provider_gen: u64,
    pub check: Option<CheckFn>,
    pub check_status: Mutex<Option<DependencyStatus>>,
    /// Removal has started: strict reads fail at once. The binding stays until its
    /// consumers have drained, so the provider's subtree can still read it.
    pub removing: std::sync::atomic::AtomicBool,
}

/// Ordinary bindings remain immutable; only `provide_mut_as` pays a value lock.
pub(crate) enum ValueSlot {
    Fixed(StoredValue),
    Mutable(Mutex<StoredValue>),
}

impl ValueSlot {
    pub(crate) fn snapshot(&self) -> StoredValue {
        match self {
            Self::Fixed(value) => value.clone(),
            Self::Mutable(value) => value.lock().unwrap().clone(),
        }
    }

    fn replace_mutable(&self, value: StoredValue) -> Option<StoredValue> {
        match self {
            Self::Fixed(_) => None,
            Self::Mutable(slot) => {
                let old = std::mem::replace(&mut *slot.lock().unwrap(), value);
                Some(old)
            }
        }
    }
}

/// The service registry, with an index of the fibers that declare each key as
/// a dependency.
pub(crate) struct Registry {
    bindings: Mutex<HashMap<(TypeKey, Option<ScopeId>), Arc<Binding>>>,
    /// Injection index: `TypeKey` → the fibers that declare a dependency on it.
    inject_index: Mutex<HashMap<TypeKey, Vec<Weak<FiberInner>>>>,
}

/// Shrinks a map after entries were removed, so its capacity does not stay at
/// its peak. It shrinks when the capacity exceeds 64 slots and the length is
/// under a quarter of it; this leaves small tables alone and spreads the cost
/// over the times the threshold is crossed.
fn shrink_if_sparse<K: Eq + std::hash::Hash, V>(map: &mut HashMap<K, V>) {
    if map.capacity() > 64 && map.len() * 4 < map.capacity() {
        map.shrink_to_fit();
    }
}

/// The same rule for one key's entry list: a list left with few entries after
/// a peak does not keep its peak capacity. The threshold is the one of
/// `shrink_if_sparse`; after shrinking, a list has to grow fourfold before it
/// shrinks again, so alternating inserts and removals do not turn quadratic.
fn shrink_vec_if_sparse<T>(list: &mut Vec<T>) {
    if list.capacity() > 64 && list.len() * 4 < list.capacity() {
        list.shrink_to_fit();
    }
}
impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            bindings: Mutex::new(HashMap::new()),
            inject_index: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn table_counts(&self) -> (usize, usize) {
        (
            self.bindings.lock().unwrap().len(),
            self.inject_index.lock().unwrap().len(),
        )
    }

    /// Allocated slots of the same two tables, for the capacity checks.
    #[cfg(test)]
    pub(crate) fn table_capacities(&self) -> (usize, usize) {
        (
            self.bindings.lock().unwrap().capacity(),
            self.inject_index.lock().unwrap().capacity(),
        )
    }

    /// The largest allocated length of one key's consumer list.
    #[cfg(test)]
    pub(crate) fn max_list_capacity(&self) -> usize {
        let index = self.inject_index.lock().unwrap();
        index.values().map(Vec::capacity).max().unwrap_or(0)
    }

    pub(crate) fn insert_binding(
        &self,
        key: TypeKey,
        scope: Option<ScopeId>,
        binding: Binding,
    ) -> Result<Arc<Binding>, Error> {
        let mut bindings = self.bindings.lock().unwrap();
        let entry = (key.clone(), scope.clone());
        if let Some(existing) = bindings.get(&entry) {
            if !existing.removing.load(std::sync::atomic::Ordering::SeqCst) {
                let scope_desc = entry.1.as_deref().unwrap_or("<default>");
                return Err(ErrorKind::ServiceExists(format!(
                    "{} in scope {scope_desc}",
                    key.describe()
                ))
                .into());
            }
            // 摘除进行中的绑定可被同键新 provide 替换(TS dispose 同步释放
            // 注册表槽位;旧绑定由驱逐方按 Arc 身份 finalize,不误伤新绑定)
        }
        let stored = Arc::new(binding);
        bindings.insert(entry, stored.clone());
        Ok(stored)
    }

    pub(crate) fn lookup(&self, key: &TypeKey, scope: Option<&ScopeId>) -> Option<Arc<Binding>> {
        let bindings = self.bindings.lock().unwrap();
        bindings.get(&(key.clone(), scope.cloned())).cloned()
    }

    /// Commit a mutable value only while this exact binding still occupies
    /// its key/scope slot. The caller holds the provider transition lock.
    ///
    /// Returns the old value on success, or the *candidate* value on failure
    /// so the caller can drop it outside any framework lock.
    pub(crate) fn replace_mutable_if_current(
        &self,
        key: &TypeKey,
        scope: Option<&ScopeId>,
        expected: &Arc<Binding>,
        value: StoredValue,
    ) -> Result<StoredValue, StoredValue> {
        let bindings = self.bindings.lock().unwrap();
        let current = bindings.get(&(key.clone(), scope.cloned()));
        let Some(current) = current else {
            return Err(value);
        };
        if !Arc::ptr_eq(current, expected)
            || current.removing.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(value);
        }
        match &current.value {
            ValueSlot::Mutable(_) => {
                // Safety: Mutable slot always returns Some.
                Ok(current.value.replace_mutable(value).unwrap())
            }
            ValueSlot::Fixed(_) => Err(value),
        }
    }

    /// Linearize service removal with mutable value commits.
    pub(crate) fn mark_removing_if(
        &self,
        key: &TypeKey,
        scope: Option<&ScopeId>,
        expected: &Arc<Binding>,
    ) {
        let bindings = self.bindings.lock().unwrap();
        if bindings
            .get(&(key.clone(), scope.cloned()))
            .is_some_and(|current| Arc::ptr_eq(current, expected))
        {
            expected
                .removing
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub(crate) fn bindings_snapshot(&self) -> Vec<BindingDiagnostics> {
        self.bindings
            .lock()
            .unwrap()
            .iter()
            .map(|((key, scope), binding)| BindingDiagnostics {
                key: key.clone(),
                scope: scope.as_ref().map(|s| s.to_string()),
                provider: binding.provider_id,
                generation: binding.provider_gen,
                removing: binding.removing.load(std::sync::atomic::Ordering::SeqCst),
            })
            .collect()
    }

    pub(crate) fn dependency_status(
        &self,
        key: &TypeKey,
        scope: Option<&ScopeId>,
    ) -> DependencyStatus {
        let Some(binding) = self.lookup(key, scope) else {
            return DependencyStatus::Missing;
        };
        Self::binding_status(&binding)
    }

    /// Cached status of one binding snapshot; never invokes its check callback.
    pub(crate) fn binding_status(binding: &Binding) -> DependencyStatus {
        if binding.removing.load(std::sync::atomic::Ordering::SeqCst) {
            return DependencyStatus::Removing;
        }
        let Some(provider) = binding.provider.upgrade() else {
            return DependencyStatus::Missing;
        };
        let state = provider.state();
        if state != FiberState::Active {
            return DependencyStatus::ProviderInactive(state);
        }
        if binding.check.is_some() {
            return binding
                .check_status
                .lock()
                .unwrap()
                .unwrap_or(DependencyStatus::CheckPending);
        }
        DependencyStatus::Ready
    }

    /// Removes the binding for good; until now the provider's cleanup could still
    /// read it. Removes it only if the slot still holds `expected`: a slot that a
    /// new provide took over during removal is left alone, as TypeScript's
    /// dispose frees the slot at once. Returns whether it removed the binding.
    pub(crate) fn finalize_binding_if(
        &self,
        key: TypeKey,
        scope: Option<ScopeId>,
        expected: &Arc<Binding>,
    ) -> bool {
        let mut bindings = self.bindings.lock().unwrap();
        let still_old = bindings
            .get(&(key.clone(), scope.clone()))
            .is_some_and(|b| Arc::ptr_eq(b, expected));
        if still_old {
            bindings.remove(&(key, scope));
            shrink_if_sparse(&mut bindings);
        }
        still_old
    }

    /// The current consumers of the dependency `quad` (provider, generation, key,
    /// scope). Each fiber's `last_deps` is the only record: a fiber that declares
    /// an injection of `key` and whose `last_deps` contains `quad` is bound to the
    /// binding. This matches the load and unload windows exactly: `last_deps` is
    /// set before a load's apply, cleared when the fiber unloads, and after a
    /// failed load holds the dependencies of the failed generation. A failed fiber
    /// counts as a consumer: removal checks it again, and it stays Failed if the
    /// dependency is missing. Because `quad` includes the scope, a consumer of one
    /// scope's binding is not a consumer of the same provider's binding of that
    /// key in another scope.
    pub(crate) fn consumers_of(
        &self,
        key: &TypeKey,
        quad: (PluginId, u64, TypeKey, Option<ScopeId>),
    ) -> Vec<Arc<FiberInner>> {
        let index = self.inject_index.lock().unwrap();
        let Some(list) = index.get(key) else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|weak| {
                let fiber = weak.upgrade()?;
                let bound = fiber
                    .last_deps
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|deps| deps.contains(&quad));
                bound.then_some(fiber)
            })
            .collect()
    }

    pub(crate) fn register_inject(&self, key: TypeKey, fiber: &Arc<FiberInner>) {
        self.inject_index
            .lock()
            .unwrap()
            .entry(key)
            .or_default()
            .push(Arc::downgrade(fiber));
    }

    /// Removes the dependency declarations of a fiber that has finished: a fiber
    /// whose driver has exited takes no further part in gated resolution or
    /// re-check notifications. Keys left with no entries are removed, so keyed
    /// declarations (one qualified name per instance) do not accumulate under a
    /// long-lived root.
    pub(crate) fn unregister_injects(&self, fiber: &Arc<FiberInner>, keys: &[TypeKey]) {
        let weak = Arc::downgrade(fiber);
        let mut index = self.inject_index.lock().unwrap();
        for key in keys {
            let mut empty = false;
            if let Some(list) = index.get_mut(key) {
                list.retain(|w| !Weak::ptr_eq(w, &weak));
                empty = list.is_empty();
                shrink_vec_if_sparse(list);
            }
            if empty {
                index.remove(key);
            }
        }
        shrink_if_sparse(&mut index);
    }

    /// Asks every fiber that injects `key` to re-check its dependencies.
    pub(crate) fn notify_key_changed(&self, key: &TypeKey) {
        let fibers: Vec<Arc<FiberInner>> = {
            let index = self.inject_index.lock().unwrap();
            index
                .get(key)
                .map(|list| list.iter().filter_map(|w| w.upgrade()).collect())
                .unwrap_or_default()
        };
        for fiber in fibers {
            fiber.post(Intent::RefreshDeps);
        }
    }

    /// Asks every fiber that declares dependencies to re-check them (`refresh`,
    /// when the result of a `check()` predicate changes).
    pub(crate) fn refresh_all(&self) {
        let mut seen: HashSet<PluginId> = HashSet::new();
        let index = self.inject_index.lock().unwrap();
        for list in index.values() {
            for weak in list {
                if let Some(fiber) = weak.upgrade() {
                    if seen.insert(fiber.id) {
                        fiber.post(Intent::RefreshDeps);
                    }
                }
            }
        }
    }

    /// Gated resolution: the binding exists, is not being removed, its provider is
    /// active and its `check()` passes (a panicking check does not pass). Returns
    /// the provider and its generation.
    pub(crate) fn resolve_dep(
        &self,
        key: &TypeKey,
        scope: Option<&ScopeId>,
    ) -> Option<(PluginId, u64)> {
        let binding = self.lookup(key, scope)?;
        if binding.removing.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let provider = binding.provider.upgrade()?;
        if provider.state() != FiberState::Active {
            return None;
        }
        if let Some(check) = &binding.check {
            // check() 是用户回调:panic 视为不就绪(TS 语义:记日志并删除,
            // fiber.ts:695-698;评审 #6——不得杀调用方所在的驱动任务)
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check()));
            let passed = result.as_ref().copied().unwrap_or(false);
            *binding.check_status.lock().unwrap() = Some(match result {
                Ok(true) => DependencyStatus::Ready,
                Ok(false) => DependencyStatus::CheckRejected,
                Err(_) => DependencyStatus::CheckPanicked,
            });
            if !passed {
                return None;
            }
        }
        Some((binding.provider_id, binding.provider_gen))
    }
}

#[cfg(test)]
mod transient_tests;
