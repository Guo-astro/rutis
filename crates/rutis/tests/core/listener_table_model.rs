//! guarantee: events/order
//! Property test of the listener table (#631): random insert / remove /
//! prepend / prefix sequences are compared with one ordered-list model.
//!
//! The model is a list of registrations, each with its name, whether it is
//! a prefix entry, `prepend`, `once` and `global`, and its registration
//! number. Its call order is the one the protocol gives (spec §4.3): every
//! `prepend` entry, newest first, then every append, oldest first; an entry
//! is selected when its name is the dispatched one (an exact entry) or is
//! its prefix (a prefix entry). `once` entries leave the table when a call
//! of them starts; a disposed entry leaves it at once.
//!
//! Each case registers listeners through the public API, disposes some and
//! dispatches under random names through the four modes that call every
//! selected listener: `bail`, `serial`, `parallel`, and `emit`, which
//! dispatches over the list a name's emits share. `waterfall` is not here:
//! it wraps the listeners and stops at the first result. After every
//! operation the table `EventBus::subscriptions` reports — its order, each
//! entry's descriptor and its call count — and the listeners each dispatch
//! called, in order, are compared with the model.
//!
//! `PROPTEST_CASES` cases (256 by default); a failure prints the seed and
//! the shrunk operation list, rerun with `RUTIS_PROPTEST_SEED=<seed>` to
//! replay it (Q7.2).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use rutis::events::SyncCall;
use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::{Ctx, Disposer, Event};
use rutis_test_support::within;
use serde::{Deserialize, Serialize};

// ── the event and the names ─────────────────────────────────────

/// Dispatched under a name chosen at run time (`CtxInternal::named`).
#[derive(Serialize, Deserialize)]
struct Ev(u32);

impl Event for Ev {
    const NAME: &'static str = "";
    type Value = ();
}

/// Registrations: an exact entry takes a name from here, a prefix entry a
/// prefix from `PREFIXES`.
const NAMES: [&str; 6] = ["a", "ab", "abc", "b", "ba", "c"];
const PREFIXES: [&str; 4] = ["a", "ab", "b", "c"];

/// Dispatch names: every registration name, `abcd` (two prefixes match, no
/// exact entry) and `d` (nothing matches).
const DISPATCHED: [&str; 8] = ["a", "ab", "abc", "b", "ba", "c", "abcd", "d"];

// ── the model ───────────────────────────────────────────────────

/// One registration the model remembers.
#[derive(Debug, Clone)]
struct Registered {
    /// The table's id: this root registers nothing else, so the n-th
    /// registration is id n.
    id: u64,
    name: &'static str,
    prefix: bool,
    prepend: bool,
    once: bool,
    global: bool,
    live: bool,
    calls: u64,
}

/// The ordered table the test compares the listener table with.
#[derive(Debug, Default)]
struct Model {
    /// In registration order.
    entries: Vec<Registered>,
}

impl Model {
    /// The live entries in call order: `prepend` entries newest first, then
    /// the appends oldest first.
    fn in_order(&self) -> Vec<&Registered> {
        let live = || self.entries.iter().filter(|entry| entry.live);
        live()
            .filter(|entry| entry.prepend)
            .rev()
            .chain(live().filter(|entry| !entry.prepend))
            .collect()
    }

    /// The entries a dispatch under `name` calls, in order.
    fn selected(&self, name: &str) -> Vec<u64> {
        self.in_order()
            .into_iter()
            .filter(|entry| match entry.prefix {
                true => name.starts_with(entry.name),
                false => entry.name == name,
            })
            .map(|entry| entry.id)
            .collect()
    }

    fn live_ids(&self) -> Vec<u64> {
        self.in_order().iter().map(|entry| entry.id).collect()
    }

    fn entry(&mut self, id: u64) -> &mut Registered {
        self.entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .expect("a registered entry")
    }

    fn entry_by_id(&self, id: u64) -> &Registered {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .expect("a registered entry")
    }
}

// ── the operations ──────────────────────────────────────────────

/// An exact listener registered between two emits of one name.
#[derive(Debug, Clone)]
struct RegSpec {
    /// Under the dispatched name, not another one from `NAMES`.
    same: bool,
    /// Index into `NAMES`, when not `same`.
    other: usize,
    prepend: bool,
    once: bool,
    global: bool,
}

#[derive(Debug, Clone)]
enum Op {
    Register {
        /// Index into `NAMES` (an exact entry) or `PREFIXES` (a prefix one).
        name: usize,
        prefix: bool,
        prepend: bool,
        once: bool,
        global: bool,
    },
    /// Dispose the `pick`-th live entry (`pick % live`), nothing when none
    /// is live.
    Dispose { pick: usize },
    Dispatch {
        mode: usize,
        /// Index into `DISPATCHED`.
        name: usize,
    },
    /// `emit` under `name`, register exact listeners before the first
    /// emit's task starts, then `emit` again: the second emit calls them
    /// through the chains the first list gained (#980).
    EmitTwice {
        /// Index into `DISPATCHED`.
        name: usize,
        registers: Vec<RegSpec>,
    },
}

fn operations() -> impl Strategy<Value = Vec<Op>> {
    let exact = 0..NAMES.len();
    let prefix = 0..PREFIXES.len();
    let pick = 0..16usize;
    let mode = 0..4usize;
    let dispatched = 0..DISPATCHED.len();
    let late = prop::collection::vec(
        (
            any::<bool>(),
            0..NAMES.len(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(same, other, prepend, once, global)| RegSpec {
                same,
                other,
                prepend,
                once,
                global,
            }),
        1..4,
    );
    let op = prop_oneof![
        4 => (exact, prefix, any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>())
            .prop_map(|(exact, prefix, quoted, prepend, once, global)| Op::Register {
                name: match quoted {
                    true => prefix,
                    false => exact,
                },
                prefix: quoted,
                prepend,
                once,
                global,
            }),
        2 => pick.prop_map(|pick| Op::Dispose { pick }),
        3 => (mode, dispatched.clone()).prop_map(|(mode, name)| Op::Dispatch { mode, name }),
        2 => (dispatched, late).prop_map(|(name, registers)| Op::EmitTwice { name, registers }),
    ];
    prop::collection::vec(op, 0..32)
}

// ── running it against the kernel ───────────────────────────────

/// What the listener table and the model are compared on: the descriptor
/// and call count of each live entry, in table order.
type Shown = (u64, String, bool, bool, bool, bool, u64);

struct Harness {
    root: Ctx,
    /// The ids of the listeners each dispatch called, in order.
    calls: Arc<Mutex<Vec<u64>>>,
    model: Model,
    disposers: HashMap<u64, Disposer>,
    next: u64,
}

impl Harness {
    fn new(root: Ctx) -> Self {
        Self {
            root,
            calls: Arc::default(),
            model: Model::default(),
            disposers: HashMap::new(),
            next: 0,
        }
    }

    /// Register one synchronous listener that records its id when called.
    fn register(
        &mut self,
        name: &'static str,
        prefix: bool,
        prepend: bool,
        once: bool,
        global: bool,
    ) -> Result<(), TestCaseError> {
        let id = self.next + 1;
        let mut listen = self.root.listen::<Ev>();
        listen = match prefix {
            true => listen.prefix(name),
            false => listen.name(name),
        };
        if prepend {
            listen = listen.prepend();
        }
        if once {
            listen = listen.once();
        }
        if global {
            listen = listen.global();
        }
        let calls = self.calls.clone();
        let disposer = listen
            .on_sync(move |_: Arc<Ev>, _: SyncCall<'_, Ev>| {
                calls.lock().unwrap().push(id);
                Ok(None)
            })
            .map_err(|error| TestCaseError::fail(format!("registering {name}: {error}")))?;
        self.next = id;
        self.disposers.insert(id, disposer);
        self.model.entries.push(Registered {
            id,
            name,
            prefix,
            prepend,
            once,
            global,
            live: true,
            calls: 0,
        });
        Ok(())
    }

    async fn dispose(&mut self, pick: usize) -> Result<(), TestCaseError> {
        let live = self.model.live_ids();
        if live.is_empty() {
            return Ok(());
        }
        let id = live[pick % live.len()];
        let disposer = self.disposers.remove(&id).expect("a live disposer");
        disposer
            .dispose()
            .await
            .map_err(|error| TestCaseError::fail(format!("disposing {id}: {error}")))?;
        self.model.entry(id).live = false;
        Ok(())
    }

    /// Dispatch under `name` in `mode`, and compare the calls with the
    /// model: the ids called, in order, and the table afterwards.
    async fn dispatch(&mut self, mode: usize, name: &'static str) -> Result<(), TestCaseError> {
        let expected = self.model.selected(name);
        self.calls.lock().unwrap().clear();
        let outcome = match mode {
            0 => self.root.named(name).bail(Ev(0)).map(drop),
            1 => self.root.named(name).serial(Ev(0)).await.map(drop),
            2 => self.root.named(name).parallel(Ev(0)).await,
            _ => {
                let emitted = self.root.named(name).emit(Ev(0));
                self.calls_started().await;
                emitted
            }
        };
        outcome.map_err(|error| TestCaseError::fail(format!("dispatching {name}: {error}")))?;
        self.calls_match(name, &expected)
    }

    /// `emit` under `name`, register exact listeners, then `emit` again
    /// before the first emit's task started: the second emit's list is the
    /// first one's plus the chains the registrations appended, so it calls
    /// the new listeners in registration order (#980).
    async fn emit_twice(
        &mut self,
        name: &'static str,
        registers: &[RegSpec],
    ) -> Result<(), TestCaseError> {
        // The first emit calls the listeners selected when it was accepted.
        let first = self.model.selected(name);
        self.calls.lock().unwrap().clear();
        let accepted = self.root.named(name).emit(Ev(0));
        for spec in registers {
            let other = NAMES[spec.other];
            let name = match spec.same {
                true => name,
                false => other,
            };
            self.register(name, false, spec.prepend, spec.once, spec.global)?;
        }
        // The second emit selects the same list, with the registrations
        // since appended to it. Its calls wait for the first emit's; a
        // `once` entry the first one calls is out of the table by then.
        let second = self.model.selected(name);
        let queued = self.root.named(name).emit(Ev(0));
        self.calls_started().await;
        accepted.map_err(|error| TestCaseError::fail(format!("dispatching {name}: {error}")))?;
        queued.map_err(|error| TestCaseError::fail(format!("dispatching {name}: {error}")))?;
        let claimed: Vec<u64> = first
            .iter()
            .copied()
            .filter(|id| self.model.entry_by_id(*id).once)
            .collect();
        let mut expected = first;
        expected.extend(second.iter().copied().filter(|id| !claimed.contains(id)));
        self.calls_match(name, &expected)
    }

    /// An emit starts its calls in a task of its own; its backlog entry is
    /// dropped once every call of it started.
    async fn calls_started(&self) {
        within("the emit's calls started", async {
            while !self.root.diagnostics().event_backlogs.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    /// Compare the ids called with `expected`, then count the calls in the
    /// model: a `once` entry leaves the table when its call starts.
    fn calls_match(&mut self, what: &str, expected: &[u64]) -> Result<(), TestCaseError> {
        let called = std::mem::take(&mut *self.calls.lock().unwrap());
        if called != expected {
            return Err(TestCaseError::fail(format!(
                "a dispatch under {what} called {called:?}, the model {expected:?}"
            )));
        }
        for id in expected {
            let entry = self.model.entry(*id);
            entry.calls += 1;
            if entry.once {
                entry.live = false;
            }
        }
        Ok(())
    }

    /// The table `subscriptions()` reports, and the model's, in table order.
    fn shown(&self) -> (Vec<Shown>, Vec<Shown>) {
        let actual = self
            .root
            .events()
            .subscriptions()
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.name.clone(),
                    entry.prefix,
                    entry.prepend,
                    entry.once,
                    entry.global,
                    entry.calls,
                )
            })
            .collect();
        let expected = self
            .model
            .in_order()
            .into_iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.name.to_owned(),
                    entry.prefix,
                    entry.prepend,
                    entry.once,
                    entry.global,
                    entry.calls,
                )
            })
            .collect();
        (actual, expected)
    }

    fn check_table(&self, what: &str) -> Result<(), TestCaseError> {
        let (actual, expected) = self.shown();
        match actual == expected {
            true => Ok(()),
            false => Err(TestCaseError::fail(format!(
                "after {what}: the table is {actual:?}, the model {expected:?}"
            ))),
        }
    }

    async fn apply(&mut self, ops: &[Op]) -> Result<(), TestCaseError> {
        for (index, op) in ops.iter().enumerate() {
            let what = format!("op {index} {op:?}");
            match op {
                Op::Register {
                    name,
                    prefix,
                    prepend,
                    once,
                    global,
                } => {
                    let name = match prefix {
                        true => PREFIXES[*name],
                        false => NAMES[*name],
                    };
                    self.register(name, *prefix, *prepend, *once, *global)?;
                }
                Op::Dispose { pick } => self.dispose(*pick).await?,
                Op::Dispatch { mode, name } => self.dispatch(*mode, DISPATCHED[*name]).await?,
                Op::EmitTwice { name, registers } => {
                    self.emit_twice(DISPATCHED[*name], registers).await?
                }
            }
            self.check_table(&what)?;
        }
        Ok(())
    }

    /// Let go of every registration, so nothing outlives the case.
    async fn release(&mut self) {
        for (_, disposer) in std::mem::take(&mut self.disposers) {
            let _ = disposer.dispose().await;
        }
    }
}

async fn run(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let root = Ctx::root().expect("a root context");
    let mut harness = Harness::new(root.clone());
    let outcome = harness.apply(&ops).await;
    harness.release().await;
    root.shutdown().await.expect("shutdown");
    outcome
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
fn the_listener_table_matches_an_ordered_model() {
    check(
        "random inserts, removals, prepends and prefixes",
        operations(),
        |ops| {
            // A runtime per case: each case starts from a fresh root and
            // leaves nothing behind for the next one.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(run(ops))
        },
    );
}
