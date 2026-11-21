//! Load, unload and restart requests racing on worker threads (#42), and a
//! sequential reference model of the fiber lifecycle (#622).
//!
//! Each racing case fires a pseudo-random mix of operations at one fiber
//! from several tasks and then checks invariants that must hold for every
//! interleaving: nothing hangs, generations never overlap, and every
//! cleanup a generation registered runs exactly once.
//!
//! The plugin registers its cleanup, then waits for `ctx.cancelled()`:
//! a generation ends because it is stopped, never on its own. A dispose
//! that starts a generation without cancelling it (Bug A, #619) leaves
//! that dispose waiting, which these tests catch. Restarts are issued from
//! their own tasks and joined after the fiber is disposed: awaiting one
//! directly would wait for the next generation's `apply`, which by design
//! runs until the generation is stopped.
//!
//! The mix comes from a seed: `RUTIS_SEED` if set, otherwise a random one.
//! Each test prints its seed, and every failure names it, so a failing
//! run can be replayed with `RUTIS_SEED=<seed> cargo test -p rutis --test
//! interleave`.
//!
//! The last test is sequential: random batches of mounts, dependency flips,
//! restarts, updates (including a failed apply, a panic, a config the dry
//! run refuses and a plugin that disposes itself) and disposals are compared
//! after every operation with a reference model of each fiber's state,
//! generation, apply and cleanup counts. `check()` was removed in 0.19
//! (#694); a dependency gate now flips only when a service appears or goes,
//! so `Provide` and `Withdraw` are those flips. `PROPTEST_CASES` (256 by
//! default) batches per seed; a failure prints the seed and the shrunk
//! batch, replayed with `RUTIS_PROPTEST_SEED=<seed>`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView, PluginFactory, TypeKey,
};
use rutis::{Ctx, Disposer, Error};
use rutis_test_support::{soon, Guard};

const ROUNDS: u64 = 100;
const TASKS: u64 = 6;
const OPS_PER_TASK: usize = 8;

/// The seed for this run: `RUTIS_SEED` if set, otherwise random. Printed,
/// so the test output of a failing run carries it.
fn seed() -> u64 {
    let seed = match std::env::var("RUTIS_SEED") {
        Ok(value) => value
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("RUTIS_SEED must be a number, got {value:?}")),
        Err(_) => {
            use std::hash::{BuildHasher, Hasher};
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish()
        }
    };
    println!("interleave seed: {seed} (replay with RUTIS_SEED={seed})");
    seed
}

/// A small deterministic generator, so a failing round can be replayed.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

/// What the generations of one fiber did.
#[derive(Default)]
struct Ledger {
    /// `apply` calls.
    applies: AtomicUsize,
    /// Generations between their apply and their cleanup.
    live: AtomicUsize,
    max_live: AtomicUsize,
    /// Cleanups registered and cleanups run.
    registered: AtomicUsize,
    cleaned: AtomicUsize,
}

impl Ledger {
    fn check(&self, seed: u64, round: u64) {
        assert_eq!(
            self.registered.load(Ordering::SeqCst),
            self.cleaned.load(Ordering::SeqCst),
            "seed {seed} round {round}: every registered cleanup runs exactly once"
        );
        assert_eq!(
            self.live.load(Ordering::SeqCst),
            0,
            "seed {seed} round {round}"
        );
        assert!(
            self.max_live.load(Ordering::SeqCst) <= 1,
            "seed {seed} round {round}: two generations were live at once"
        );
    }
}

/// Where a parent puts the newest child it started.
type ChildSlot = Arc<Mutex<Option<FiberView>>>;

/// Records its generations in a ledger; its apply registers a cooperative
/// cleanup (it waits for `ctx.cancelled()`) and then, after yielding a few
/// times so requests can arrive while it is Loading, waits for cancellation
/// itself. A generation ends because it is stopped.
struct Tracked {
    ledger: Arc<Ledger>,
    injects: Vec<TypeKey>,
    child: Option<(Arc<Ledger>, ChildSlot)>,
}

impl DynPlugin for Tracked {
    fn name(&self) -> &str {
        "tracked"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.ledger.applies.fetch_add(1, Ordering::SeqCst);
            for _ in 0..3 {
                tokio::task::yield_now().await;
            }
            let ledger = self.ledger.clone();
            let live = ledger.live.fetch_add(1, Ordering::SeqCst) + 1;
            ledger.max_live.fetch_max(live, Ordering::SeqCst);
            let cleanup = ledger.clone();
            let stop = ctx.clone();
            let registered = ctx.effect(move || async move {
                stop.cancelled().await;
                cleanup.live.fetch_sub(1, Ordering::SeqCst);
                cleanup.cleaned.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            match registered {
                Ok(_) => {
                    ledger.registered.fetch_add(1, Ordering::SeqCst);
                }
                Err(error) => {
                    ledger.live.fetch_sub(1, Ordering::SeqCst);
                    return Err(error);
                }
            }
            if let Some((child_ledger, slot)) = &self.child {
                let child = ctx.plugin(Tracked {
                    ledger: child_ledger.clone(),
                    injects: vec![],
                    child: None,
                });
                *slot.lock().unwrap() = Some(child);
            }
            ctx.cancelled().await;
            Ok(Effect::Done)
        })
    }
}

/// Restarts issued from their own tasks, joined after the fiber is disposed:
/// a restart is handled only when the generation it starts has run, and a
/// cooperative generation runs until it is stopped.
type Restarts = Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>;

/// Spawn a restart of `view`, to be joined by [`join_restarts`].
async fn spawn_restart(view: FiberView, restarts: &Restarts) {
    restarts.lock().unwrap().push(tokio::spawn(async move {
        let _ = view.restart().await;
    }));
    // Let the restart reach the fiber before the task returns: the mix is
    // about requests arriving while a generation unloads and loads.
    tokio::task::yield_now().await;
}

/// Wait for every restart spawned so far: a dispose cancels the generation
/// they waited on, so they all return.
async fn join_restarts(seed: u64, round: u64, restarts: &Restarts) {
    let handles = std::mem::take(&mut *restarts.lock().unwrap());
    tokio::time::timeout(Duration::from_secs(10), async {
        for handle in handles {
            let _ = handle.await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("seed {seed} round {round}: restarts hung"));
}

/// Runs `op` from `TASKS` tasks, each drawing `OPS_PER_TASK` op codes.
async fn storm<F>(seed: u64, round: u64, codes: u64, op: F)
where
    F: Fn(u64) -> BoxFuture<'static, ()> + Send + Sync + 'static,
{
    let op = Arc::new(op);
    let mut tasks = Vec::new();
    for task in 0..TASKS {
        let op = op.clone();
        tasks.push(tokio::spawn(async move {
            let mut rng = Lcg(seed.wrapping_add(round * 1_000 + task));
            for _ in 0..OPS_PER_TASK {
                op(rng.next(codes)).await;
            }
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        for task in tasks {
            task.await.expect("op task");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("seed {seed} round {round}: operations hung"));
}

/// guarantee: core/D5
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_settle_and_dispose_interleave() {
    let seed = seed();
    for round in 0..ROUNDS {
        let ctx = Ctx::root().unwrap();
        let ledger = Arc::new(Ledger::default());
        let view = ctx.plugin(Tracked {
            ledger: ledger.clone(),
            injects: vec![],
            child: None,
        });
        let target = view.clone();
        let restarts = Restarts::default();
        let spawned = restarts.clone();
        storm(seed, round, 8, move |code| {
            let view = target.clone();
            let restarts = spawned.clone();
            Box::pin(async move {
                // Results are not checked: a restart after dispose is
                // rejected, which is one of the interleavings.
                match code {
                    0..=4 => spawn_restart(view, &restarts).await,
                    5..=6 => drop(view.dispose().await),
                    _ => drop(view.state()),
                }
            })
        })
        .await;
        soon(view.dispose()).await.ok();
        assert_eq!(
            view.state().state,
            FiberState::Disposed,
            "seed {seed} round {round}"
        );
        join_restarts(seed, round, &restarts).await;
        ledger.check(seed, round);
        soon(ctx.shutdown()).await.unwrap();
    }
}

/// guarantee: core/D6, core/D7
#[tokio::test]
async fn a_dispose_right_after_mounting_ends_for_a_cooperative_apply() {
    let ctx = Ctx::root().unwrap();
    let ledger = Arc::new(Ledger::default());
    let view = ctx.plugin(Tracked {
        ledger: ledger.clone(),
        injects: vec![],
        child: None,
    });
    // No yield between mounting and disposing, so the queued first load
    // either sees the terminal registration and starts no generation, or
    // starts one the registration cancels: either way the cooperative apply
    // must return and the cleanup run once (#619). A generation the
    // registration does not cancel leaves the apply waiting forever.
    tokio::time::timeout(Duration::from_secs(10), view.dispose())
        .await
        .expect("the dispose returns")
        .expect("the dispose succeeds");
    assert_eq!(view.state().state, FiberState::Disposed);
    assert!(ledger.applies.load(Ordering::SeqCst) <= 1);
    ledger.check(0, 0);
    soon(ctx.shutdown()).await.unwrap();
}

#[derive(Debug)]
struct Dep;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dependency_churn_interleaves_with_restart() {
    let seed = seed();
    for round in 0..ROUNDS {
        let ctx = Ctx::root().unwrap();
        let ledger = Arc::new(Ledger::default());
        let view = ctx.plugin(Tracked {
            ledger: ledger.clone(),
            injects: vec![TypeKey::of::<Dep>()],
            child: None,
        });
        let provided = Arc::new(tokio::sync::Mutex::new(None));
        let (target, host) = (view.clone(), ctx.clone());
        let restarts = Restarts::default();
        let spawned = restarts.clone();
        storm(seed, round, 6, move |code| {
            let (view, ctx, provided) = (target.clone(), host.clone(), provided.clone());
            let restarts = spawned.clone();
            Box::pin(async move {
                match code {
                    0 | 1 => {
                        let mut slot = provided.lock().await;
                        if slot.is_none() {
                            *slot = Some(ctx.provide(Dep).unwrap());
                        }
                    }
                    2 | 3 => {
                        // Held until the service is gone, so the next
                        // provide does not find it still bound.
                        let mut slot = provided.lock().await;
                        if let Some(disposer) = slot.take() {
                            disposer.dispose().await.unwrap();
                        }
                    }
                    4 => spawn_restart(view, &restarts).await,
                    _ => drop(view.state()),
                }
            })
        })
        .await;
        // With the dependency present the consumer may be Loading, may have
        // returned Active after a cancellation, or may be unloading: with a
        // cooperative apply no one of those is fixed. The invariants below
        // are; the reference model compares the states themselves.
        if ctx.get::<Dep>().is_none() {
            ctx.provide(Dep).unwrap();
        }
        soon(view.dispose()).await.unwrap();
        assert_eq!(
            view.state().state,
            FiberState::Disposed,
            "seed {seed} round {round}"
        );
        join_restarts(seed, round, &restarts).await;
        ledger.check(seed, round);
        soon(ctx.shutdown()).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_restart_interleaves_with_child_operations() {
    let seed = seed();
    for round in 0..ROUNDS {
        let ctx = Ctx::root().unwrap();
        let parent_ledger = Arc::new(Ledger::default());
        let child_ledger = Arc::new(Ledger::default());
        let child_slot: ChildSlot = Arc::new(Mutex::new(None));
        let parent = ctx.plugin(Tracked {
            ledger: parent_ledger.clone(),
            injects: vec![],
            child: Some((child_ledger.clone(), child_slot.clone())),
        });
        // The parent's apply mounts the child before it waits for
        // cancellation; a child exists once that has run.
        soon(async {
            loop {
                if child_slot.lock().unwrap().is_some() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let (target, slot) = (parent.clone(), child_slot.clone());
        let restarts = Restarts::default();
        let spawned = restarts.clone();
        storm(seed, round, 6, move |code| {
            let parent = target.clone();
            // The newest child; an older one may already be gone.
            let child = slot.lock().unwrap().clone();
            let restarts = spawned.clone();
            Box::pin(async move {
                match (code, child) {
                    (0 | 1, _) => spawn_restart(parent, &restarts).await,
                    (2, Some(child)) => spawn_restart(child, &restarts).await,
                    (3, Some(child)) => drop(child.dispose().await),
                    (4, Some(child)) => drop(child.state()),
                    _ => drop(parent.state()),
                }
            })
        })
        .await;
        soon(parent.dispose()).await.unwrap();
        // Disposing the parent took every child generation with it.
        if let Some(child) = child_slot.lock().unwrap().take() {
            assert_eq!(
                child.state().state,
                FiberState::Disposed,
                "seed {seed} round {round}"
            );
        }
        join_restarts(seed, round, &restarts).await;
        parent_ledger.check(seed, round);
        child_ledger.check(seed, round);
        soon(ctx.shutdown()).await.unwrap();
    }
}

// ── the sequential reference model (#622 item 4) ───────────────

/// Two services the model's dependency gates watch. `check()` was removed
/// in 0.19 (#694), so a gate flips only when one of these appears or goes.
#[derive(Debug)]
struct K1;
#[derive(Debug)]
struct K2;

fn key(index: usize) -> TypeKey {
    match index {
        0 => TypeKey::of::<K1>(),
        _ => TypeKey::of::<K2>(),
    }
}

/// What `apply` does after it registers its cleanup. `Hold` is the
/// cooperative plugin: it runs until the generation is cancelled,
/// unless a restart set `finish` to let it return so the restart can be
/// awaited (a restarted generation would otherwise keep the restart
/// waiting forever).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Hold,
    Return,
    Fail,
    Panic,
    DisposeSelf,
    BadConfig,
}

impl Mode {
    fn from_code(code: u64) -> Self {
        match code {
            0 => Mode::Hold,
            1 => Mode::Return,
            2 => Mode::Fail,
            3 => Mode::Panic,
            4 => Mode::DisposeSelf,
            _ => Mode::BadConfig,
        }
    }

    fn code(self) -> u32 {
        match self {
            Mode::Hold => 0,
            Mode::Return => 1,
            Mode::Fail => 2,
            Mode::Panic => 3,
            Mode::DisposeSelf => 4,
            Mode::BadConfig => 9,
        }
    }
}

#[derive(Debug, Clone)]
enum Op {
    Mount(usize),
    Restart(usize),
    Update { fiber: usize, mode: Mode },
    Dispose(usize),
    Provide(usize),
    Withdraw(usize),
}

/// About 20 operations per case: the bounds let proptest shrink a failure
/// to the operations that matter.
fn operations() -> impl Strategy<Value = Vec<Op>> {
    let fiber = 0..MODEL_FIBERS;
    let key = 0..MODEL_KEYS;
    let mode = 0..6u64;
    let op = prop_oneof![
        3 => fiber.clone().prop_map(Op::Mount),
        4 => fiber.clone().prop_map(Op::Restart),
        3 => (fiber.clone(), mode).prop_map(|(fiber, mode)| Op::Update {
            fiber,
            mode: Mode::from_code(mode),
        }),
        3 => fiber.clone().prop_map(Op::Dispose),
        3 => key.clone().prop_map(Op::Provide),
        2 => key.prop_map(Op::Withdraw),
    ];
    prop::collection::vec(op, 12..=24)
}

/// What one fiber's generations did, shared by every instance the factory
/// builds: applies started, cleanups registered and cleanups run.
#[derive(Default)]
struct Counters {
    applies: AtomicUsize,
    registered: AtomicUsize,
    cleaned: AtomicUsize,
}

impl Counters {
    fn snapshot(&self) -> (usize, usize, usize) {
        (
            self.applies.load(Ordering::SeqCst),
            self.registered.load(Ordering::SeqCst),
            self.cleaned.load(Ordering::SeqCst),
        )
    }
}

/// The plugin every model fiber runs: it registers a cooperative cleanup
/// (waiting for `ctx.cancelled()`, already cancelled at unload), counts the
/// registration, then acts out `mode`.
struct Lifecycle {
    name: &'static str,
    mode: Mode,
    injects: Vec<TypeKey>,
    counters: Arc<Counters>,
    finish: Arc<AtomicBool>,
}

impl DynPlugin for Lifecycle {
    fn name(&self) -> &str {
        self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.counters.applies.fetch_add(1, Ordering::SeqCst);
            let counters = self.counters.clone();
            let stop = ctx.clone();
            let registered = ctx.effect(move || async move {
                stop.cancelled().await;
                counters.cleaned.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            match registered {
                Ok(_) => {
                    self.counters.registered.fetch_add(1, Ordering::SeqCst);
                }
                Err(error) => return Err(error),
            }
            match self.mode {
                Mode::Hold => {
                    if !self.finish.load(Ordering::SeqCst) {
                        ctx.cancelled().await;
                    }
                }
                Mode::Return => {}
                Mode::Fail => return Err(Error::msg("an apply the test failed")),
                Mode::Panic => panic!("an apply the test panicked"),
                Mode::DisposeSelf => ctx.dispose_self().expect("the generation is current"),
                Mode::BadConfig => unreachable!("the dry run refuses this config"),
            }
            Ok(Effect::Done)
        })
    }
}

/// Builds [`Lifecycle`] from the model's config word.
struct LifecycleFactory {
    name: &'static str,
    injects: Vec<TypeKey>,
    counters: Arc<Counters>,
    finish: Arc<AtomicBool>,
}

impl PluginFactory<u32> for LifecycleFactory {
    fn name(&self) -> &str {
        self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn validate_config(&self, config: &u32) -> Result<(), Error> {
        match *config {
            0..=4 => Ok(()),
            _ => Err(Error::msg("the model's configs are 0..=4")),
        }
    }

    fn build(&self, config: &u32) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Lifecycle {
            name: self.name,
            mode: Mode::from_code(u64::from(*config)),
            injects: self.injects.clone(),
            counters: self.counters.clone(),
            finish: self.finish.clone(),
        }))
    }
}

/// The model's operations are in-process; a hang is a model or kernel bug,
/// and a short guard keeps a shrinking counterexample quick.
const MODEL_GUARD: Guard = Guard(Duration::from_secs(2));

const MODEL_FIBERS: usize = 3;
const MODEL_KEYS: usize = 2;

/// Fibers A and B are built from a config (so `update` can change what the
/// next `apply` does); C is static. A has no dependencies, B waits for K1,
/// C for K1 and K2.
fn deps(fiber: usize) -> &'static [usize] {
    match fiber {
        0 => &[],
        1 => &[0],
        _ => &[0, 1],
    }
}

fn has_factory(fiber: usize) -> bool {
    fiber < 2
}

/// Every key of `fiber` provided.
fn full_mask(fiber: usize) -> u8 {
    (1u8 << deps(fiber).len()) - 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelState {
    Pending,
    Loading,
    Active,
    Failed,
    Disposed,
}

impl ModelState {
    fn as_fiber(self) -> FiberState {
        match self {
            ModelState::Pending => FiberState::Pending,
            ModelState::Loading => FiberState::Loading,
            ModelState::Active => FiberState::Active,
            ModelState::Failed => FiberState::Failed,
            ModelState::Disposed => FiberState::Disposed,
        }
    }

    fn live(self) -> bool {
        matches!(self, ModelState::Loading | ModelState::Active)
    }
}

#[derive(Debug)]
struct FiberModel {
    mounted: bool,
    state: ModelState,
    generation: u64,
    applies: usize,
    registered: usize,
    cleaned: usize,
    /// The config of the next load A or B builds; C stays `Hold`.
    mode: Mode,
    /// The dependency identities the last load bound (a mask of this fiber's
    /// own keys, all present while it ran). A failed fiber is retried only
    /// when this set changes; a service that goes and comes back from the
    /// same provider is the same identity, so it is not.
    last_deps: Option<u8>,
}

impl FiberModel {
    fn new() -> Self {
        Self {
            mounted: false,
            state: ModelState::Pending,
            generation: 0,
            applies: 0,
            registered: 0,
            cleaned: 0,
            mode: Mode::Hold,
            last_deps: None,
        }
    }
}

/// The fibers' lifecycle as the guarantees describe it, for operations run
/// one at a time: no operation overlaps another, so every transition is
/// decided by the state before it.
#[derive(Debug)]
struct LifecycleModel {
    fibers: [FiberModel; MODEL_FIBERS],
    provided: [bool; MODEL_KEYS],
}

impl LifecycleModel {
    fn new() -> Self {
        Self {
            fibers: [FiberModel::new(), FiberModel::new(), FiberModel::new()],
            provided: [false; MODEL_KEYS],
        }
    }

    fn ready(&self, fiber: usize) -> bool {
        self.dep_mask(fiber) == full_mask(fiber)
    }

    /// The keys of `fiber` that are provided now, one bit per key in the
    /// order of [`deps`].
    fn dep_mask(&self, fiber: usize) -> u8 {
        deps(fiber).iter().enumerate().fold(0, |mask, (bit, key)| {
            if self.provided[*key] {
                mask | (1 << bit)
            } else {
                mask
            }
        })
    }

    /// A generation starts: its number goes up, `apply` runs once and
    /// registers one cleanup. What the generation does next follows its
    /// mode; a failed apply and a self-dispose run their cleanup at once.
    fn load(&mut self, fiber: usize, finish: bool) {
        let mask = self.dep_mask(fiber);
        let fiber = &mut self.fibers[fiber];
        fiber.generation += 1;
        fiber.applies += 1;
        fiber.registered += 1;
        fiber.last_deps = Some(mask);
        fiber.state = match fiber.mode {
            Mode::Hold if finish => ModelState::Active,
            Mode::Hold => ModelState::Loading,
            Mode::Return => ModelState::Active,
            Mode::Fail | Mode::Panic => ModelState::Failed,
            Mode::DisposeSelf => ModelState::Disposed,
            Mode::BadConfig => unreachable!("the dry run refuses this config"),
        };
        if matches!(fiber.mode, Mode::Fail | Mode::Panic | Mode::DisposeSelf) {
            fiber.cleaned += 1;
        }
    }

    /// An unload for a restart, a dependency change or a dispose: a live
    /// generation's cleanup runs, then the fiber waits or is gone.
    fn unload(&mut self, fiber: usize) {
        let fiber = &mut self.fibers[fiber];
        if fiber.state.live() {
            fiber.cleaned += 1;
        }
        if fiber.state != ModelState::Disposed {
            fiber.state = ModelState::Pending;
            fiber.last_deps = None;
        }
    }

    /// A dependency key appeared or went. Only the consumers of that key
    /// reload; a failed fiber retries only when what it waits for changes.
    fn refresh(&mut self, fiber: usize) {
        if !self.fibers[fiber].mounted || self.fibers[fiber].state == ModelState::Disposed {
            return;
        }
        let ready = self.ready(fiber);
        let mask = self.dep_mask(fiber);
        match self.fibers[fiber].state {
            ModelState::Loading | ModelState::Active => {
                if ready && self.fibers[fiber].last_deps == Some(mask) {
                    return;
                }
                self.unload(fiber);
                if ready {
                    self.load(fiber, false);
                }
            }
            ModelState::Pending => {
                if ready {
                    self.load(fiber, false);
                }
            }
            // Failed is sticky: repeated notices for the same dependency set
            // do not retry it; a changed set does.
            ModelState::Failed => {
                if ready && self.fibers[fiber].last_deps != Some(mask) {
                    self.unload(fiber);
                    self.load(fiber, false);
                }
            }
            ModelState::Disposed => {}
        }
    }

    fn mount(&mut self, fiber: usize) {
        if self.fibers[fiber].mounted {
            return;
        }
        self.fibers[fiber].mounted = true;
        if self.ready(fiber) {
            self.load(fiber, false);
        }
    }

    /// The unload-and-load of a restart or update. `None` means there is no
    /// view to call (nothing was mounted).
    fn reload(&mut self, fiber: usize) -> bool {
        if self.fibers[fiber].state != ModelState::Pending {
            self.unload(fiber);
        }
        if self.ready(fiber) {
            self.load(fiber, true);
        }
        self.fibers[fiber].state != ModelState::Failed
    }

    fn restart(&mut self, fiber: usize) -> Option<bool> {
        if !self.fibers[fiber].mounted {
            return None;
        }
        if self.fibers[fiber].state == ModelState::Disposed {
            return Some(false);
        }
        Some(self.reload(fiber))
    }

    fn update(&mut self, fiber: usize, mode: Mode) -> Option<bool> {
        if !self.fibers[fiber].mounted {
            return None;
        }
        if self.fibers[fiber].state == ModelState::Disposed
            || !has_factory(fiber)
            || mode == Mode::BadConfig
        {
            return Some(false);
        }
        self.fibers[fiber].mode = mode;
        Some(self.reload(fiber))
    }

    fn provide(&mut self, key: usize) -> bool {
        if self.provided[key] {
            return false;
        }
        self.provided[key] = true;
        for fiber in 0..MODEL_FIBERS {
            if deps(fiber).contains(&key) {
                self.refresh(fiber);
            }
        }
        true
    }

    fn withdraw(&mut self, key: usize) {
        if !self.provided[key] {
            return;
        }
        self.provided[key] = false;
        for fiber in 0..MODEL_FIBERS {
            if deps(fiber).contains(&key) {
                self.refresh(fiber);
            }
        }
    }

    fn dispose(&mut self, fiber: usize) {
        if !self.fibers[fiber].mounted || self.fibers[fiber].state == ModelState::Disposed {
            return;
        }
        if self.fibers[fiber].state.live() {
            self.fibers[fiber].cleaned += 1;
        }
        self.fibers[fiber].state = ModelState::Disposed;
        self.fibers[fiber].last_deps = None;
    }
}

/// Drives the kernel and holds the model's counterpart of every handle.
struct Harness {
    root: Ctx,
    views: [Option<FiberView>; MODEL_FIBERS],
    counters: [Arc<Counters>; MODEL_FIBERS],
    provided: [Option<Disposer>; MODEL_KEYS],
    /// The flag `Hold` checks when a generation starts: set around a
    /// restart or update so the generation it starts returns and the
    /// restart can be awaited.
    finish: Arc<AtomicBool>,
    model: LifecycleModel,
}

impl Harness {
    fn new(root: Ctx) -> Self {
        Self {
            root,
            views: std::array::from_fn(|_| None),
            counters: std::array::from_fn(|_| Arc::new(Counters::default())),
            provided: std::array::from_fn(|_| None),
            finish: Arc::new(AtomicBool::new(false)),
            model: LifecycleModel::new(),
        }
    }

    fn factory(&self, fiber: usize) -> LifecycleFactory {
        LifecycleFactory {
            name: ["model-a", "model-b"][fiber],
            injects: deps(fiber).iter().map(|index| key(*index)).collect(),
            counters: self.counters[fiber].clone(),
            finish: self.finish.clone(),
        }
    }

    fn mount(&mut self, fiber: usize) -> bool {
        if self.views[fiber].is_some() {
            return false;
        }
        let view = match has_factory(fiber) {
            true => self.root.plugin_with(self.factory(fiber), 0u32),
            false => self.root.plugin(Lifecycle {
                name: "model-c",
                mode: Mode::Hold,
                injects: deps(fiber).iter().map(|index| key(*index)).collect(),
                counters: self.counters[fiber].clone(),
                finish: self.finish.clone(),
            }),
        };
        self.views[fiber] = Some(view);
        true
    }

    /// Wait until `fiber` reports the model's state and generation: a load,
    /// an unload and a dependency refresh all finish in their driver, so the
    /// snapshot is the operation's result.
    async fn wait(&self, fiber: usize, what: &str) {
        let Some(view) = &self.views[fiber] else {
            return;
        };
        let expected = &self.model.fibers[fiber];
        let state = expected.state.as_fiber();
        let generation = expected.generation;
        MODEL_GUARD
            .within(
                &format!("{what}: fiber {fiber} reaches {state:?} generation {generation}"),
                async {
                    let mut states = view.watch();
                    states
                        .wait_for(|snapshot| {
                            snapshot.state == state && snapshot.generation == generation
                        })
                        .await
                        .expect("the fiber's state channel is open");
                },
            )
            .await;
    }

    /// Compare every fiber's state, generation and counts, the live cleanup
    /// each generation keeps, and the services the root provides.
    fn check(&self, what: &str) -> Result<(), TestCaseError> {
        for fiber in 0..MODEL_FIBERS {
            let expected = &self.model.fibers[fiber];
            let Some(view) = &self.views[fiber] else {
                if expected.mounted {
                    return Err(TestCaseError::fail(format!(
                        "{what}: fiber {fiber} is in the model but has no view"
                    )));
                }
                continue;
            };
            if !expected.mounted {
                return Err(TestCaseError::fail(format!(
                    "{what}: fiber {fiber} has a view but is not in the model"
                )));
            }
            let snapshot = view.state();
            if snapshot.state != expected.state.as_fiber()
                || snapshot.generation != expected.generation
            {
                return Err(TestCaseError::fail(format!(
                    "{what}: fiber {fiber} is {:?} generation {}, the model {:?} generation {}",
                    snapshot.state, snapshot.generation, expected.state, expected.generation
                )));
            }
            let (applies, registered, cleaned) = self.counters[fiber].snapshot();
            let model = (expected.applies, expected.registered, expected.cleaned);
            if (applies, registered, cleaned) != model {
                return Err(TestCaseError::fail(format!(
                    "{what}: fiber {fiber} applied {applies}, registered {registered}, cleaned {cleaned}; the model {model:?}"
                )));
            }
            let live = usize::from(expected.state.live());
            if registered != cleaned + live {
                return Err(TestCaseError::fail(format!(
                    "{what}: fiber {fiber} registered {registered} cleanups, ran {cleaned}, and has {live} live generation(s)"
                )));
            }
        }
        for key_index in 0..MODEL_KEYS {
            let actual = match key_index {
                0 => self.root.get::<K1>().is_some(),
                _ => self.root.get::<K2>().is_some(),
            };
            if actual != self.model.provided[key_index] {
                return Err(TestCaseError::fail(format!(
                    "{what}: service {key_index} is provided={actual}, the model says {}",
                    self.model.provided[key_index]
                )));
            }
        }
        Ok(())
    }

    fn expect(result: bool, expected: bool, what: &str) -> Result<(), TestCaseError> {
        match result == expected {
            true => Ok(()),
            false => Err(TestCaseError::fail(format!(
                "{what}: the call returned {}, the model expects {}",
                if result { "Ok" } else { "an error" },
                if expected { "Ok" } else { "an error" }
            ))),
        }
    }

    async fn apply_op(&mut self, op: &Op, what: &str) -> Result<(), TestCaseError> {
        match op {
            Op::Mount(fiber) => {
                if self.mount(*fiber) {
                    self.model.mount(*fiber);
                    self.wait(*fiber, what).await;
                }
            }
            Op::Restart(fiber) => match self.views[*fiber].clone() {
                None => {
                    self.model.restart(*fiber);
                }
                Some(view) => {
                    let expected = self.model.restart(*fiber).expect("a mounted fiber");
                    self.finish.store(true, Ordering::SeqCst);
                    let result = MODEL_GUARD
                        .within(&format!("{what}: restart fiber {fiber}"), view.restart())
                        .await;
                    self.finish.store(false, Ordering::SeqCst);
                    Self::expect(result.is_ok(), expected, what)?;
                    self.wait(*fiber, what).await;
                }
            },
            Op::Update { fiber, mode } => match self.views[*fiber].clone() {
                None => {
                    self.model.update(*fiber, *mode);
                }
                Some(view) => {
                    let expected = self.model.update(*fiber, *mode).expect("a mounted fiber");
                    self.finish.store(true, Ordering::SeqCst);
                    let result = MODEL_GUARD
                        .within(
                            &format!("{what}: update fiber {fiber} to {mode:?}"),
                            view.update(mode.code()),
                        )
                        .await;
                    self.finish.store(false, Ordering::SeqCst);
                    Self::expect(result.is_ok(), expected, what)?;
                    // A self-dispose is queued by the new generation after
                    // `update` returned; joining the terminal task runs it.
                    if *mode == Mode::DisposeSelf
                        && self.model.fibers[*fiber].state == ModelState::Disposed
                    {
                        let joined = MODEL_GUARD
                            .within(
                                &format!("{what}: join the self-dispose of fiber {fiber}"),
                                view.dispose(),
                            )
                            .await;
                        Self::expect(joined.is_ok(), true, what)?;
                    }
                    self.wait(*fiber, what).await;
                }
            },
            Op::Dispose(fiber) => {
                if let Some(view) = self.views[*fiber].clone() {
                    self.model.dispose(*fiber);
                    let result = MODEL_GUARD
                        .within(&format!("{what}: dispose fiber {fiber}"), view.dispose())
                        .await;
                    Self::expect(result.is_ok(), true, what)?;
                    self.wait(*fiber, what).await;
                } else {
                    self.model.dispose(*fiber);
                }
            }
            Op::Provide(key_index) => {
                let expected = self.model.provide(*key_index);
                let result = match key_index {
                    0 => self.root.provide(K1),
                    _ => self.root.provide(K2),
                };
                let ok = result.is_ok();
                if let Ok(disposer) = result {
                    self.provided[*key_index] = Some(disposer);
                }
                Self::expect(ok, expected, what)?;
                for fiber in 0..MODEL_FIBERS {
                    if deps(fiber).contains(key_index) {
                        self.wait(fiber, what).await;
                    }
                }
            }
            Op::Withdraw(key_index) => {
                let result = match self.provided[*key_index].take() {
                    Some(disposer) => {
                        MODEL_GUARD
                            .within(
                                &format!("{what}: withdraw service {key_index}"),
                                disposer.dispose(),
                            )
                            .await
                    }
                    None => Ok(()),
                };
                self.model.withdraw(*key_index);
                Self::expect(result.is_ok(), true, what)?;
                for fiber in 0..MODEL_FIBERS {
                    if deps(fiber).contains(key_index) {
                        self.wait(fiber, what).await;
                    }
                }
            }
        }
        Ok(())
    }

    /// Dispose what is still mounted.
    async fn teardown(&mut self) -> Result<(), TestCaseError> {
        for fiber in 0..MODEL_FIBERS {
            if self.views[fiber].is_some() && self.model.fibers[fiber].state != ModelState::Disposed
            {
                self.apply_op(&Op::Dispose(fiber), "teardown").await?;
                self.check("teardown")?;
            }
        }
        self.check("teardown")
    }
}

async fn run(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), Arc::new(|_| {}));
    let mut harness = Harness::new(root.clone());
    for (index, op) in ops.iter().enumerate() {
        let what = format!("op {index} {op:?}");
        harness.apply_op(op, &what).await?;
        harness.check(&what)?;
    }
    harness.teardown().await?;
    MODEL_GUARD
        .within("the root shuts down", root.shutdown())
        .await
        .expect("the root shuts down");
    Ok(())
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

/// guarantee: core/D6, core/D7, core/D12, core/D14, core/D30, core/D32, core/features.errors.failed-retry
#[test]
fn the_fiber_lifecycle_matches_a_reference_model() {
    check(
        "mounts, dependency flips, restarts, updates and disposals",
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
