//! guarantee: bridge/marked-services
//! Property test of the marked block registry's generations (#631,
//! `bridge/marked-services c`, `MSV-4-1b`): random builds, supersedes and
//! name lookups are compared with a model of the registry.
//!
//! The model is a list of registrations, each with the type it is a block
//! of, the generation of the image that registered it (0 for the program's
//! own, `loading` for a loaded one), its name and whether the name was
//! given by the marker. From it the test derives, per the rules the kernel
//! documents:
//!
//! - `entries_of(type, Some(gen))`: the type's blocks of that image, in
//!   registration order; an image that registered none for the type (its
//!   build dropped the marker) never borrows another image's — the
//!   program's own (generation 0) blocks, or none. For the program itself
//!   (`None`) only generation 0's.
//! - `exported_as(name)`: the groups of type and generation in
//!   registration order, generations a newer load superseded left out, one
//!   name per group (the one a block gave it, else the first block's); the
//!   last group with that name answers, with its type's name.
//! - `name_of(type)`: the name a block gave it (the last one registered),
//!   else the first block's, over every generation (`is_marked` alike).
//!
//! `PROPTEST_CASES` cases (256 by default); a failure prints the seed and
//! the shrunk operation list, rerun with `RUTIS_PROPTEST_SEED=<seed>` to
//! replay it (Q7.2).

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use rutis::host::TypeKey;
use rutis::internal::service::{
    entries_of, exported_as, is_exported, is_marked, loading, name_of, register, supersede, Answer,
    CallError, Encoded, Entry, Generation, Provision, Wire,
};
use rutis::Ctx;

// ── the marked blocks ───────────────────────────────────────────

/// A type "rebuilt" by later images: a dylib rebuilt without a version
/// bump keeps its `TypeId`, which is what the registry tells apart by
/// generation.
struct RebuiltA;
struct RebuiltB;
struct RebuiltC;
/// A second type the marker gives `RebuiltA`'s name: which of the two a
/// lookup answers is the order rule.
struct RebuiltD;
/// Never registered: a type no build marked.
struct Unmarked;

fn a_id() -> TypeId {
    TypeId::of::<RebuiltA>()
}
fn b_id() -> TypeId {
    TypeId::of::<RebuiltB>()
}
fn c_id() -> TypeId {
    TypeId::of::<RebuiltC>()
}
fn d_id() -> TypeId {
    TypeId::of::<RebuiltD>()
}
fn unmarked_id() -> TypeId {
    TypeId::of::<Unmarked>()
}

fn no_methods() -> &'static [&'static str] {
    &[]
}

fn no_resolve(_: &Ctx, _: &TypeKey, _: &Provision) -> Result<Arc<dyn Any + Send + Sync>, String> {
    Err("unused".into())
}

fn no_invoke(
    _: &Arc<dyn Any + Send + Sync>,
    _: &str,
    _: Encoded,
    _: &'static dyn Wire,
) -> Result<Answer, CallError> {
    Err(CallError::new("unused"))
}

/// One marked block: the type, the name it is served by, and whether the
/// marker gave that name.
struct Block {
    entry: &'static Entry,
    ty: usize,
    name: &'static str,
    named: bool,
}

fn a_name() -> &'static str {
    "RebuiltA"
}
fn b_name() -> &'static str {
    "RebuiltB"
}
fn c_name() -> &'static str {
    "RebuiltC"
}
fn d_name() -> &'static str {
    "RebuiltD"
}

const fn entry(
    type_id: fn() -> TypeId,
    type_name: fn() -> &'static str,
    name: &'static str,
    named: bool,
) -> Entry {
    Entry::new(
        type_id, type_name, name, named, no_methods, no_resolve, no_invoke,
    )
}

static A_NAMED: Entry = entry(a_id, a_name, "generation-pt-a", true);
static A_DERIVED: Entry = entry(a_id, a_name, "RebuiltA", false);
static B_DERIVED: Entry = entry(b_id, b_name, "RebuiltB", false);
static B_NAMED: Entry = entry(b_id, b_name, "generation-pt-b", true);
static C_FIRST: Entry = entry(c_id, c_name, "RebuiltC", false);
static C_SECOND: Entry = entry(c_id, c_name, "RebuiltC", false);
static D_NAMED: Entry = entry(d_id, d_name, "RebuiltA", true);
static D_DERIVED: Entry = entry(d_id, d_name, "RebuiltD", false);

/// The six blocks the cases register: two of `RebuiltA` (one named by the
/// marker), two of `RebuiltB` (one named), two of `RebuiltC` (both with the
/// same derived name), two of `RebuiltD` (one named with `RebuiltA`'s name,
/// so a lookup has two types to choose between).
static BLOCKS: [Block; 8] = [
    Block {
        entry: &A_NAMED,
        ty: 0,
        name: "generation-pt-a",
        named: true,
    },
    Block {
        entry: &A_DERIVED,
        ty: 0,
        name: "RebuiltA",
        named: false,
    },
    Block {
        entry: &B_DERIVED,
        ty: 1,
        name: "RebuiltB",
        named: false,
    },
    Block {
        entry: &B_NAMED,
        ty: 1,
        name: "generation-pt-b",
        named: true,
    },
    Block {
        entry: &C_FIRST,
        ty: 2,
        name: "RebuiltC",
        named: false,
    },
    Block {
        entry: &C_SECOND,
        ty: 2,
        name: "RebuiltC",
        named: false,
    },
    Block {
        entry: &D_NAMED,
        ty: 3,
        name: "RebuiltA",
        named: true,
    },
    Block {
        entry: &D_DERIVED,
        ty: 3,
        name: "RebuiltD",
        named: false,
    },
];

/// A type the cases look at: its name, and how to get its `TypeId`.
struct Kind {
    name: &'static str,
    id: fn() -> TypeId,
}

/// Every type the cases look at; the last one is never registered.
static TYPES: [Kind; 5] = [
    Kind {
        name: "RebuiltA",
        id: a_id,
    },
    Kind {
        name: "RebuiltB",
        id: b_id,
    },
    Kind {
        name: "RebuiltC",
        id: c_id,
    },
    Kind {
        name: "RebuiltD",
        id: d_id,
    },
    Kind {
        name: "Unmarked",
        id: unmarked_id,
    },
];

/// The names the cases look up: every block's, and one nothing registers.
const NAMES: [&str; 7] = [
    "generation-pt-a",
    "RebuiltA",
    "RebuiltB",
    "generation-pt-b",
    "RebuiltC",
    "RebuiltD",
    "generation-pt-none",
];

// ── the model ───────────────────────────────────────────────────

/// One registration: a block of a type, in an image's generation.
#[derive(Debug, Clone)]
struct Row {
    ty: usize,
    generation: u64,
    name: &'static str,
    named: bool,
}

/// The registry the test compares with, over every case of the run: the
/// registry itself keeps every registration a case made.
#[derive(Debug, Default)]
struct Model {
    rows: Vec<Row>,
    superseded: HashSet<u64>,
    /// Every generation the cases made, for the lookups of earlier ones
    /// (a `Generation` comes only from `loading`).
    generations: Vec<Generation>,
    /// How many checks have run, to walk the names in turn.
    checks: usize,
}

impl Model {
    fn registered(&mut self, ty: usize, generation: u64, block: &Block) {
        self.rows.push(Row {
            ty,
            generation,
            name: block.name,
            named: block.named,
        });
    }

    fn superseded(&mut self, generation: u64) {
        if generation != 0 {
            self.superseded.insert(generation);
        }
    }

    /// A type's blocks in one generation, in registration order.
    fn of(&self, ty: usize, generation: u64) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|row| row.ty == ty && row.generation == generation)
            .collect()
    }

    /// The names `entries_of` gives `ty` for `generation`: that image's
    /// blocks, else the program's own when the image registered none.
    fn served(&self, ty: usize, generation: Option<u64>) -> Vec<&'static str> {
        let own = match generation {
            Some(generation) if generation != 0 => {
                let own = self.of(ty, generation);
                if own.is_empty() {
                    self.of(ty, 0)
                } else {
                    own
                }
            }
            _ => self.of(ty, 0),
        };
        own.iter().map(|row| row.name).collect()
    }

    /// The (type, generation) groups: the index of the first block of each,
    /// and the name it is exported as, superseded generations left out.
    fn groups(&self) -> HashMap<(usize, u64), (usize, &'static str)> {
        let mut groups: HashMap<(usize, u64), (usize, &'static str)> = HashMap::new();
        for (index, row) in self.rows.iter().enumerate() {
            if self.superseded.contains(&row.generation) {
                continue;
            }
            let exported = groups.entry((row.ty, row.generation));
            match exported {
                // The one a block gave it (they agree), else the first one's.
                std::collections::hash_map::Entry::Occupied(mut known) => {
                    if row.named {
                        known.get_mut().1 = row.name;
                    }
                }
                std::collections::hash_map::Entry::Vacant(new) => {
                    new.insert((index, row.name));
                }
            }
        }
        groups
    }

    /// The type name `exported_as(name)` answers: the last group, in the
    /// order of its first block, exported as `name`.
    fn exported_as(&self, name: &str) -> Option<&'static str> {
        self.groups()
            .into_iter()
            .filter(|((_, _), (_, exported))| *exported == name)
            .max_by_key(|((_, _), (index, _))| *index)
            .map(|((ty, _), _)| TYPES[ty].name)
    }

    /// `name_of`: the name a block gave the type, else the first block's.
    fn name_of(&self, ty: usize) -> Option<&'static str> {
        let mut fallback = None;
        let mut named = None;
        for row in self.rows.iter().filter(|row| row.ty == ty) {
            fallback.get_or_insert(row.name);
            if row.named {
                named = Some(row.name);
            }
        }
        named.or(fallback)
    }
}

// ── the operations ──────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Op {
    /// A loaded image (a new generation) registers these blocks.
    Build { blocks: Vec<usize> },
    /// The program itself (generation 0) registers these blocks.
    Program { blocks: Vec<usize> },
    /// A newer load of the plugin replaces the `pick`-th generation this
    /// case made (`pick % made`).
    Supersede { pick: usize },
}

fn operations() -> impl Strategy<Value = Vec<Op>> {
    let blocks = prop::collection::vec(0..BLOCKS.len(), 0..=2);
    let op = prop_oneof![
        6 => blocks.clone().prop_map(|blocks| Op::Build { blocks }),
        2 => blocks.prop_map(|blocks| Op::Program { blocks }),
        2 => (0..8usize).prop_map(|pick| Op::Supersede { pick }),
    ];
    prop::collection::vec(op, 1..4)
}

// ── the run ─────────────────────────────────────────────────────

/// The model over the whole run (the registry keeps every case's blocks).
static MODEL: Mutex<Option<Model>> = Mutex::new(None);

fn run(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let mut guard = MODEL.lock().unwrap();
    let model = guard.get_or_insert_with(Model::default);
    // The generations this case made, to supersede one of them.
    let mut made: Vec<Generation> = Vec::new();
    for op in ops {
        match op {
            Op::Build { blocks } => {
                let ((), generation) = loading(|| {
                    blocks
                        .iter()
                        .for_each(|index| register(BLOCKS[*index].entry))
                });
                for index in &blocks {
                    let block = &BLOCKS[*index];
                    model.registered(block.ty, generation.id(), block);
                }
                model.generations.push(generation);
                made.push(generation);
            }
            Op::Program { blocks } => {
                for index in &blocks {
                    register(BLOCKS[*index].entry);
                }
                for index in &blocks {
                    let block = &BLOCKS[*index];
                    model.registered(block.ty, 0, block);
                }
            }
            Op::Supersede { pick } => {
                if !made.is_empty() {
                    let generation = made[pick % made.len()];
                    supersede([generation]);
                    model.superseded(generation.id());
                }
            }
        }
        check(model, &made)?;
    }
    Ok(())
}

/// Compare the registry with the model after every operation: the program
/// itself, the generation this case made last, and one the model made
/// earlier.
fn check(model: &mut Model, made: &[Generation]) -> Result<(), TestCaseError> {
    // Only some generations: every one the model knows would make each
    // check scan the whole registry.
    let generations = std::iter::once(None)
        .chain(made.last().copied().map(Some))
        .chain(
            model
                .generations
                .get(model.generations.len() / 2)
                .copied()
                .map(Some),
        )
        .collect::<Vec<_>>();
    for (index, ty) in TYPES.iter().enumerate() {
        let (type_name, type_id) = (ty.name, (ty.id)());
        for generation in &generations {
            let actual: Vec<&'static str> = entries_of(type_id, *generation)
                .iter()
                .map(|entry| entry.name())
                .collect();
            let expected = model.served(index, generation.map(Generation::id));
            prop_assert_eq!(
                &actual,
                &expected,
                "entries_of({}, {:?})",
                type_name,
                generation
            );
        }
        prop_assert_eq!(
            name_of(type_id),
            model.name_of(index),
            "name_of({})",
            type_name
        );
        prop_assert_eq!(
            is_marked(type_id),
            model.name_of(index).is_some(),
            "is_marked({})",
            type_name
        );
    }
    // One name per check, in turn: `exported_as` groups the whole registry,
    // which over a run of a thousand cases is the check's cost.
    let name = NAMES[model.checks % NAMES.len()];
    let expected = model.exported_as(name);
    let actual = exported_as(name);
    let exported = is_exported(name);
    model.checks += 1;
    prop_assert_eq!(actual, expected, "exported_as({:?})", name);
    prop_assert_eq!(exported, expected.is_some(), "is_exported({:?})", name);
    Ok(())
}

/// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED` or a
/// fresh one; on failure, panic with the seed and the shrunk input.
fn check_with<S: Strategy>(
    name: &str,
    strategy: S,
    test: impl Fn(S::Value) -> Result<(), TestCaseError>,
) where
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
fn each_builds_blocks_and_the_newest_names_match_the_registry_model() {
    check_with(
        "random builds, supersedes and name lookups",
        operations(),
        run,
    );
}
