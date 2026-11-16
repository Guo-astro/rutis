//! Property test of the service registration slot (#631): random insert /
//! mark-removing / replace-mutable / finalize sequences are compared with a
//! plain map model.
//!
//! The model is one slot per `(TypeKey, scope)` holding the binding that was
//! inserted last: `insert_binding` refuses a key that holds a binding still
//! live and replaces one whose removal has started; `mark_removing_if` sets
//! the `removing` flag only while the slot still holds the binding it names;
//! `replace_mutable_if_current` commits a value only while that binding is
//! current, not removing, and mutable; `finalize_binding_if` removes the slot
//! only while it still holds the binding named, so a slot a new provide took
//! over keeps its new binding.
//!
//! After every operation every slot is compared: presence, binding identity,
//! the `removing` flag, the provider and generation, and the value. A
//! `PROPTEST_CASES` count (256 by default); a failure prints the seed and the
//! shrunk operations, rerun with `RUTIS_PROPTEST_SEED=<seed>` to replay it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};

use super::*;
use crate::key::ScopeId;
use crate::ErrorKind;

const KEYS: usize = 3;
const SCOPES: usize = 3;

fn type_key(key: u8) -> TypeKey {
    TypeKey::keyed_dynamic::<u64>(format!("prop-registry-{key}"))
}

fn scope_id(scope: u8) -> Option<ScopeId> {
    match scope {
        0 => None,
        n => Some(Arc::from(format!("prop-scope-{n}"))),
    }
}

fn binding(provider: u8, mutable: bool, value: u64) -> Binding {
    let stored = StoredValue::new(Arc::new(value));
    Binding {
        value: match mutable {
            true => ValueSlot::Mutable(Mutex::new(stored)),
            false => ValueSlot::Fixed(stored),
        },
        provider: Weak::new(),
        provider_id: PluginId(u64::from(provider) + 1),
        provider_gen: u64::from(provider),
        removing: std::sync::atomic::AtomicBool::new(false),
    }
}

fn stored_value(value: u64) -> StoredValue {
    StoredValue::new(Arc::new(value))
}

fn value_of(stored: &StoredValue) -> u64 {
    *stored.downcast::<u64>().expect("a u64 value")
}

/// One binding the test inserted, and what the model remembers of it.
struct Inserted {
    key: u8,
    scope: u8,
    provider: u8,
    mutable: bool,
    value: u64,
    removing: bool,
    handle: Arc<Binding>,
}

/// The map model: one slot per key and scope.
#[derive(Default)]
struct Model {
    slots: HashMap<(u8, u8), usize>,
    bindings: Vec<Inserted>,
}

impl Model {
    fn current(&self, key: u8, scope: u8) -> Option<usize> {
        self.slots.get(&(key, scope)).copied()
    }

    /// Whether `insert` would take the slot: none is free, one still live is
    /// refused, one whose removal started is replaced.
    fn insert_takes(&self, key: u8, scope: u8) -> bool {
        self.current(key, scope)
            .is_none_or(|index| self.bindings[index].removing)
    }
}

#[derive(Debug, Clone)]
enum Op {
    Insert {
        key: u8,
        scope: u8,
        provider: u8,
        mutable: bool,
    },
    Mark {
        target: u8,
    },
    Replace {
        target: u8,
        value: u64,
    },
    Finalize {
        target: u8,
    },
    Lookup {
        key: u8,
        scope: u8,
    },
}

fn operations() -> impl Strategy<Value = Vec<Op>> {
    let key = 0..KEYS as u8;
    let scope = 0..SCOPES as u8;
    let target = 0..64u8;
    let op = prop_oneof![
        5 => (key.clone(), scope.clone(), any::<u8>(), any::<bool>())
            .prop_map(|(key, scope, provider, mutable)| Op::Insert { key, scope, provider, mutable }),
        2 => target.clone().prop_map(|target| Op::Mark { target }),
        3 => (target.clone(), any::<u64>()).prop_map(|(target, value)| Op::Replace { target, value }),
        2 => target.clone().prop_map(|target| Op::Finalize { target }),
        1 => (key, scope).prop_map(|(key, scope)| Op::Lookup { key, scope }),
    ];
    prop::collection::vec(op, 0..40)
}

struct Harness {
    registry: Registry,
    model: Model,
    /// The value handed to the next inserted binding.
    next_value: u64,
}

impl Harness {
    fn new() -> Self {
        Self {
            registry: Registry::new(),
            model: Model::default(),
            next_value: 1,
        }
    }

    /// The binding a target names, when any: earlier insertions are kept
    /// even after their slot moved on, so `mark`, `replace` and `finalize`
    /// can name an old binding.
    fn target(&self, target: u8) -> Option<usize> {
        match self.model.bindings.is_empty() {
            true => None,
            false => Some(usize::from(target) % self.model.bindings.len()),
        }
    }

    fn insert(
        &mut self,
        key: u8,
        scope: u8,
        provider: u8,
        mutable: bool,
    ) -> Result<(), TestCaseError> {
        let expected = self.model.insert_takes(key, scope);
        let value = self.next_value;
        self.next_value += 1;
        let fresh = binding(provider, mutable, value);
        let result =
            self.registry
                .insert_binding(type_key(key), scope_id(scope), fresh, "prop service");
        match result {
            Ok(handle) => {
                if !expected {
                    return Err(TestCaseError::fail(format!(
                        "insert of key {key} scope {scope} took a slot the model says is held by a live binding"
                    )));
                }
                self.model.bindings.push(Inserted {
                    key,
                    scope,
                    provider,
                    mutable,
                    value,
                    removing: false,
                    handle,
                });
                self.model
                    .slots
                    .insert((key, scope), self.model.bindings.len() - 1);
            }
            Err(error) => {
                if expected {
                    return Err(TestCaseError::fail(format!(
                        "insert of key {key} scope {scope} failed on a free slot: {error}"
                    )));
                }
                if !matches!(error.kind(), Some(ErrorKind::ServiceExists(_))) {
                    return Err(TestCaseError::fail(format!(
                        "insert over a live binding reported {error:?}, not ServiceExists"
                    )));
                }
            }
        }
        Ok(())
    }

    fn mark(&mut self, target: u8) {
        let Some(index) = self.target(target) else {
            return;
        };
        let (key, scope) = {
            let binding = &self.model.bindings[index];
            (binding.key, binding.scope)
        };
        let handle = self.model.bindings[index].handle.clone();
        self.registry
            .mark_removing_if(&type_key(key), scope_id(scope).as_ref(), &handle);
        if self.model.current(key, scope) == Some(index) {
            self.model.bindings[index].removing = true;
        }
    }

    fn replace(&mut self, target: u8, value: u64) -> Result<(), TestCaseError> {
        let Some(index) = self.target(target) else {
            return Ok(());
        };
        let (key, scope) = {
            let binding = &self.model.bindings[index];
            (binding.key, binding.scope)
        };
        let handle = self.model.bindings[index].handle.clone();
        let expected = self.model.current(key, scope) == Some(index)
            && !self.model.bindings[index].removing
            && self.model.bindings[index].mutable;
        let result = self.registry.replace_mutable_if_current(
            &type_key(key),
            scope_id(scope).as_ref(),
            &handle,
            stored_value(value),
        );
        match result {
            Ok(old) => {
                if !expected {
                    return Err(TestCaseError::fail(format!(
                        "replace committed on binding {index} ({key}, {scope}), which the model says is not current, mutable or live"
                    )));
                }
                let old = value_of(&old);
                if old != self.model.bindings[index].value {
                    return Err(TestCaseError::fail(format!(
                        "replace of binding {index} returned {old}, the model {}",
                        self.model.bindings[index].value
                    )));
                }
                self.model.bindings[index].value = value;
            }
            Err(candidate) => {
                if expected {
                    return Err(TestCaseError::fail(format!(
                        "replace of the current mutable binding {index} was refused"
                    )));
                }
                let candidate = value_of(&candidate);
                if candidate != value {
                    return Err(TestCaseError::fail(format!(
                        "a refused replace returned value {candidate}, not the candidate {value}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn finalize(&mut self, target: u8) -> Result<(), TestCaseError> {
        let Some(index) = self.target(target) else {
            return Ok(());
        };
        let (key, scope) = {
            let binding = &self.model.bindings[index];
            (binding.key, binding.scope)
        };
        let handle = self.model.bindings[index].handle.clone();
        let expected = self.model.current(key, scope) == Some(index);
        let removed = self
            .registry
            .finalize_binding_if(type_key(key), scope_id(scope), &handle);
        if removed != expected {
            return Err(TestCaseError::fail(format!(
                "finalize of binding {index} ({key}, {scope}) returned {removed}, the model {expected}"
            )));
        }
        if expected {
            self.model.slots.remove(&(key, scope));
        }
        Ok(())
    }

    /// Compare every slot with the model: presence, identity, removal flag,
    /// provider and value.
    fn check_slot(&self, key: u8, scope: u8) -> Result<(), TestCaseError> {
        let actual = self
            .registry
            .lookup(&type_key(key), scope_id(scope).as_ref());
        let expected = self.model.current(key, scope);
        match (actual.as_ref(), expected) {
            (None, None) => Ok(()),
            (Some(actual), Some(index)) => {
                let model = &self.model.bindings[index];
                if !Arc::ptr_eq(actual, &model.handle) {
                    return Err(TestCaseError::fail(format!(
                        "slot ({key}, {scope}) holds another binding than the model's {index}"
                    )));
                }
                if actual.removing.load(std::sync::atomic::Ordering::SeqCst) != model.removing {
                    return Err(TestCaseError::fail(format!(
                        "slot ({key}, {scope}) removing is {}, the model {}",
                        actual.removing.load(std::sync::atomic::Ordering::SeqCst),
                        model.removing
                    )));
                }
                if actual.provider_id != PluginId(u64::from(model.provider) + 1)
                    || actual.provider_gen != u64::from(model.provider)
                {
                    return Err(TestCaseError::fail(format!(
                        "slot ({key}, {scope}) provider is {:?}/{} the model {}/{}",
                        actual.provider_id, actual.provider_gen, model.provider, model.provider
                    )));
                }
                let value = value_of(&actual.value.snapshot());
                if value != model.value {
                    return Err(TestCaseError::fail(format!(
                        "slot ({key}, {scope}) value is {value}, the model {}",
                        model.value
                    )));
                }
                Ok(())
            }
            (actual, expected) => Err(TestCaseError::fail(format!(
                "slot ({key}, {scope}) is {}, the model {}",
                match actual {
                    Some(_) => "held",
                    None => "free",
                },
                match expected {
                    Some(_) => "held",
                    None => "free",
                }
            ))),
        }
    }

    fn check(&self, what: &str) -> Result<(), TestCaseError> {
        for key in 0..KEYS as u8 {
            for scope in 0..SCOPES as u8 {
                if let Err(error) = self.check_slot(key, scope) {
                    return Err(TestCaseError::fail(format!("after {what}: {error}")));
                }
            }
        }
        let (bindings, _) = self.registry.table_counts();
        if bindings != self.model.slots.len() {
            return Err(TestCaseError::fail(format!(
                "after {what}: the table holds {bindings} bindings, the model {}",
                self.model.slots.len()
            )));
        }
        let snapshot = self.registry.bindings_snapshot();
        if snapshot.len() != self.model.slots.len() {
            return Err(TestCaseError::fail(format!(
                "after {what}: diagnostics list {} bindings, the model {}",
                snapshot.len(),
                self.model.slots.len()
            )));
        }
        for key in 0..KEYS as u8 {
            for scope in 0..SCOPES as u8 {
                let expected = match self.model.current(key, scope) {
                    Some(index) if self.model.bindings[index].removing => {
                        crate::DependencyStatus::Removing
                    }
                    // The model's bindings name a provider that is gone, so a
                    // binding that is not being removed is missing too.
                    _ => crate::DependencyStatus::Missing,
                };
                let actual = self
                    .registry
                    .dependency_status(&type_key(key), scope_id(scope).as_ref());
                if actual != expected {
                    return Err(TestCaseError::fail(format!(
                        "after {what}: the status of slot ({key}, {scope}) is {actual:?}, the model {expected:?}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn apply(&mut self, ops: &[Op]) -> Result<(), TestCaseError> {
        for (index, op) in ops.iter().enumerate() {
            let what = format!("op {index} {op:?}");
            match op {
                Op::Insert {
                    key,
                    scope,
                    provider,
                    mutable,
                } => self.insert(*key, *scope, *provider, *mutable)?,
                Op::Mark { target } => self.mark(*target),
                Op::Replace { target, value } => self.replace(*target, *value)?,
                Op::Finalize { target } => self.finalize(*target)?,
                Op::Lookup { key, scope } => self.check_slot(*key, *scope)?,
            }
            self.check(&what)?;
        }
        Ok(())
    }
}

/// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED` or a
/// fresh one; on failure, panic with the seed and the shrunk input.
fn check<S: Strategy>(name: &str, strategy: S, test: impl Fn(S::Value) -> Result<(), TestCaseError>)
where
    S::Value: std::fmt::Debug,
{
    let seed: u64 = match std::env::var("RUTIS_PROPTEST_SEED") {
        Ok(seed) => seed.parse().expect("RUTIS_PROPTEST_SEED is a u64"),
        Err(_) => {
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            );
            hasher.finish()
        }
    };
    let mut bytes = [0u8; 32];
    for chunk in bytes.chunks_mut(8) {
        chunk.copy_from_slice(&seed.to_le_bytes());
    }
    // `Config::default()` reads PROPTEST_CASES (256 when unset).
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let cases = config.cases;
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    let started = std::time::Instant::now();
    if let Err(error) = runner.run(&strategy, test) {
        panic!("{name}: {error}\nreplay with RUTIS_PROPTEST_SEED={seed}");
    }
    eprintln!(
        "{name}: {cases} cases in {:?} (seed {seed})",
        started.elapsed()
    );
}

#[test]
fn the_registration_slot_matches_a_map_model() {
    check(
        "random insert, mark, replace and finalize",
        operations(),
        |ops| {
            let mut harness = Harness::new();
            harness.apply(&ops)
        },
    );
}
