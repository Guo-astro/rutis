//! Typed dependencies (#50): one description gives the gate and the reads.

use std::future::IntoFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{Call, Listener};
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, ErrorSink, FiberState, FiberStatusChanged,
    FiberView, Key, TypeKey,
};
use rutis::{Ctx, Deps, Error, Plugin};
use rutis_test_support::within;
use tokio::sync::Notify;

#[derive(Debug)]
struct Llm(u32);
#[derive(Debug)]
struct Logger;
#[derive(Debug)]
struct Ready;

async fn soon<F: std::future::IntoFuture>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), f)
        .await
        .expect("timed out")
}

async fn reach(view: &FiberView, state: FiberState) {
    let mut rx = view.watch();
    soon(rx.wait_for(|snapshot| snapshot.state == state))
        .await
        .expect("fiber dropped");
}

/// What each generation received.
type Seen = Arc<Mutex<Vec<u32>>>;

/// Requires `Llm`, takes `Logger` when present, waits for `Ready`.
struct Chat(Seen);

impl Plugin for Chat {
    // `Ready` only gates: it is taken and left unused.
    type Deps = (Arc<Llm>, Option<Arc<Logger>>, Arc<Ready>);

    fn name(&self) -> &str {
        "chat"
    }

    async fn apply(&self, _: &Ctx, (llm, logger, _): Self::Deps) -> rutis::Result {
        self.0
            .lock()
            .unwrap()
            .push(llm.0 + if logger.is_some() { 100 } else { 0 });
        Ok(())
    }
}

#[tokio::test]
async fn required_and_gate_dependencies_gate_optional_ones_do_not() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let plugin = Chat(seen.clone());
    assert_eq!(
        plugin.injects(),
        [TypeKey::of::<Llm>(), TypeKey::of::<Ready>()]
    );
    let view = ctx.plugin(plugin);
    ctx.provide(Llm(1)).unwrap();
    soon(&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Pending); // waits for Ready
    ctx.provide(Ready).unwrap();
    soon(&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Active);
    assert_eq!(*seen.lock().unwrap(), [1]);
}

#[tokio::test]
async fn an_optional_dependency_is_passed_when_present() {
    let ctx = Ctx::root().unwrap();
    ctx.provide(Llm(1)).unwrap();
    ctx.provide(Logger).unwrap();
    ctx.provide(Ready).unwrap();
    let seen = Seen::default();
    let view = ctx.plugin(Chat(seen.clone()));
    soon(&view).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), [101]);
}

#[tokio::test]
async fn losing_a_required_dependency_evicts_and_the_next_one_is_passed() {
    let ctx = Ctx::root().unwrap();
    ctx.provide(Ready).unwrap();
    let first = ctx.provide(Llm(1)).unwrap();
    let seen = Seen::default();
    let view = ctx.plugin(Chat(seen.clone()));
    soon(&view).await.unwrap();

    first.dispose().await.unwrap();
    reach(&view, FiberState::Pending).await;
    ctx.provide(Llm(2)).unwrap();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [1, 2]);
}

#[test]
fn a_key_named_twice_gates_once() {
    struct Twice;
    impl Plugin for Twice {
        type Deps = (Arc<Llm>, Option<Arc<Logger>>, Arc<Llm>, Arc<Llm>);
        fn name(&self) -> &str {
            "twice"
        }
        async fn apply(&self, _: &Ctx, _: Self::Deps) -> rutis::Result {
            Ok(())
        }
    }
    assert_eq!(Twice.injects(), [TypeKey::of::<Llm>()]);
}

// ── services behind trait objects ───────────────────────────────

trait Model: Send + Sync {
    fn id(&self) -> u32;
}

impl Model for Llm {
    fn id(&self) -> u32 {
        self.0
    }
}

trait Tracer: Send + Sync {}

struct UsesModel(Seen);

impl Plugin for UsesModel {
    type Deps = (Arc<dyn Model>, Option<Arc<dyn Tracer>>);

    fn name(&self) -> &str {
        "uses-model"
    }

    async fn apply(&self, _: &Ctx, (model, tracer): Self::Deps) -> rutis::Result {
        assert!(tracer.is_none());
        self.0.lock().unwrap().push(model.id());
        Ok(())
    }
}

#[tokio::test]
async fn trait_object_services_are_dependencies() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let view = ctx.plugin(UsesModel(seen.clone()));
    let model: Arc<dyn Model> = Arc::new(Llm(5));
    ctx.provide_as::<dyn Model>(model).unwrap();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [5]);
}

// ── keys chosen when mounting ───────────────────────────────────

/// Requires the `Llm` under `primary`, takes the one under `backup` when
/// present, waits for the `Ready` under `ready`: a plugin that depends by
/// name lists the keys in `injects` and reads them with `require_at`.
struct Pair {
    seen: Seen,
    primary: Key<Llm>,
    backup: Key<Llm>,
    ready: Key<Ready>,
}

impl DynPlugin for Pair {
    fn name(&self) -> &str {
        "pair"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![self.primary.erase(), self.ready.erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let primary = ctx.require_at(&self.primary)?;
            let backup = ctx.get_at(&self.backup);
            let mut seen = self.seen.lock().unwrap();
            seen.push(primary.0);
            seen.extend(backup.map(|backup| backup.0));
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn named_keys_are_chosen_when_mounting() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let plugin = Pair {
        seen: seen.clone(),
        primary: Key::named("primary"),
        backup: Key::dynamic(format!("backup-{}", 1)),
        ready: Key::named("ready"),
    };
    assert_eq!(
        plugin.injects(),
        [
            TypeKey::keyed::<Llm>("primary"),
            TypeKey::keyed::<Ready>("ready")
        ]
    );
    let view = ctx.plugin(plugin);
    // Services under other keys do not open the gate.
    ctx.provide(Llm(9)).unwrap();
    ctx.provide(Ready).unwrap();
    soon(&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Pending);

    ctx.provide_at(Key::<Llm>::named("primary"), Arc::new(Llm(1)))
        .unwrap();
    ctx.provide_at(Key::<Llm>::named("backup-1"), Arc::new(Llm(2)))
        .unwrap();
    ctx.provide_at(Key::<Ready>::named("ready"), Arc::new(Ready))
        .unwrap();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [1, 2]);
}

/// Hands out the context of a fiber of its own (a new instance).
struct Capture(Arc<Mutex<Option<Ctx>>>);

impl DynPlugin for Capture {
    fn name(&self) -> &str {
        "capture"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        *self.0.lock().unwrap() = Some(ctx.clone());
        Box::pin(async { Ok(Effect::Done) })
    }
}

async fn instance(parent: &Ctx) -> Ctx {
    let slot = Arc::new(Mutex::new(None));
    let view = parent.plugin(Capture(slot.clone()));
    soon(&view).await.unwrap();
    let ctx = slot.lock().unwrap().take().unwrap();
    ctx
}

/// Requires one `Llm`.
struct One(Seen);

impl Plugin for One {
    type Deps = Arc<Llm>;

    fn name(&self) -> &str {
        "one"
    }

    async fn apply(&self, _: &Ctx, llm: Arc<Llm>) -> rutis::Result {
        self.0.lock().unwrap().push(llm.0);
        Ok(())
    }
}

/// [`One`] with the key of its `Llm` chosen when mounting.
struct OneAt {
    key: Key<Llm>,
    seen: Seen,
}

impl DynPlugin for OneAt {
    fn name(&self) -> &str {
        "one"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![self.key.erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let llm = ctx.require_at(&self.key)?;
            self.seen.lock().unwrap().push(llm.0);
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn instance_keys_resolve_inside_their_instance_only() {
    let root = Ctx::root().unwrap();
    let a = instance(&root).await;
    let b = instance(&root).await;
    let key = Key::<Llm>::of().instance(a.instance());
    a.provide_at(key.clone(), Arc::new(Llm(10))).unwrap();

    let seen = Seen::default();
    let inside = a.plugin(OneAt {
        key: key.clone(),
        seen: seen.clone(),
    });
    let outside = b.plugin(OneAt {
        key,
        seen: seen.clone(),
    });
    soon(&inside).await.unwrap();
    soon(&outside).await.unwrap();
    assert_eq!(inside.state().state, FiberState::Active);
    assert_eq!(outside.state().state, FiberState::Pending);
    assert_eq!(*seen.lock().unwrap(), [10]);
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn isolated_scopes_pass_their_own_service() {
    let ctx = Ctx::root().unwrap();
    let scope = ctx.isolate(TypeKey::of::<Llm>(), "a");
    scope.provide(Llm(1)).unwrap();
    ctx.provide(Llm(2)).unwrap();
    let seen = Seen::default();
    let scoped = scope.plugin(One(seen.clone()));
    soon(&scoped).await.unwrap();
    let shared = ctx.plugin(One(seen.clone()));
    soon(&shared).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), [1, 2]);
}

// ── factories ───────────────────────────────────────────────────

struct Offset(u32, Seen);

impl Plugin for Offset {
    // `Ready` only gates: it is taken and left unused.
    type Deps = (Arc<Llm>, Arc<Ready>);

    fn name(&self) -> &str {
        "offset"
    }

    async fn apply(&self, _: &Ctx, (llm, _): Self::Deps) -> rutis::Result {
        self.1.lock().unwrap().push(llm.0 + self.0);
        Ok(())
    }
}

#[tokio::test]
async fn a_typed_factory_declares_from_the_same_description() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    // Builds `Offset` plugins from a configured offset, as rutis-loader's
    // `Builtins::register` mounts a typed build function.
    let build_seen = seen.clone();
    let view = ctx.plugin_with(
        rutis::internal::build_fn(move |offset: &u32| Ok(Offset(*offset, build_seen.clone()))),
        10u32,
    );
    ctx.provide(Llm(1)).unwrap();
    soon(&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Pending); // the gate is declared too
    ctx.provide(Ready).unwrap();
    reach(&view, FiberState::Active).await;
    soon(view.update(20u32)).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), [11, 21]);
}

// ── a dependency lost between the gate and the read ─────────────

/// Records every state the fiber enters.
fn record_states(states: Arc<Mutex<Vec<FiberState>>>) -> impl Listener<FiberStatusChanged> {
    move |event: Arc<FiberStatusChanged>, _: Call<FiberStatusChanged>| {
        states.lock().unwrap().push(event.to);
        std::future::ready(Ok(None))
    }
}

/// The first generation waits in `apply` until its `Llm` is withdrawn,
/// reads it as a typed plugin does, then waits for `hold` (if any) before
/// reporting the read.
struct LosesItsDependency {
    generations: AtomicUsize,
    read: Arc<Notify>,
    hold: Option<Arc<Notify>>,
    seen: Seen,
    injects: Vec<TypeKey>,
}

impl LosesItsDependency {
    fn new(hold: Option<Arc<Notify>>, seen: Seen) -> Self {
        Self {
            generations: AtomicUsize::new(0),
            read: Arc::new(Notify::new()),
            hold,
            seen,
            injects: vec![TypeKey::of::<Llm>()],
        }
    }
}

impl DynPlugin for LosesItsDependency {
    fn name(&self) -> &str {
        "loses-its-dependency"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let first = self.generations.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                // Withdrawing the provider cancels this generation.
                ctx.cancelled().await;
            }
            let read = <Arc<Llm> as Deps>::resolve(ctx);
            if first {
                assert!(matches!(
                    &read,
                    Err(error) if matches!(&**error, ErrorKind::InjectUnsatisfied(_))
                ));
                self.read.notify_one();
                if let Some(hold) = &self.hold {
                    hold.notified().await;
                }
            }
            self.seen.lock().unwrap().push(read?.0);
            Ok(Effect::Done)
        })
    }
}

async fn lose_and_restore(hold: bool) -> (Vec<u32>, Vec<FiberState>) {
    lose_and_restore_in(Ctx::root().unwrap(), hold).await
}

async fn lose_and_restore_in(ctx: Ctx, hold: bool) -> (Vec<u32>, Vec<FiberState>) {
    let states = Arc::new(Mutex::new(Vec::new()));
    let first = ctx.provide(Llm(1)).unwrap();
    let seen = Seen::default();
    let release = hold.then(|| Arc::new(Notify::new()));
    let plugin = LosesItsDependency::new(release.clone(), seen.clone());
    let read = plugin.read.clone();
    let view = ctx.plugin(plugin);
    reach(&view, FiberState::Loading).await;
    ctx.on::<FiberStatusChanged>(record_states(states.clone()))
        .unwrap();

    // The gate was open; the dependency goes before apply reads it. The
    // withdrawal waits for the consumer, which is still loading.
    let withdrawn = tokio::spawn(first.dispose());
    soon(read.notified()).await;
    if let Some(release) = release {
        // Back before the failed read is handled.
        ctx.provide(Llm(2)).unwrap();
        release.notify_one();
    } else {
        reach(&view, FiberState::Pending).await;
        assert!(view.state().error.is_none());
        assert!(seen.lock().unwrap().is_empty());
        ctx.provide(Llm(2)).unwrap();
    }
    soon(withdrawn).await.unwrap().unwrap();
    reach(&view, FiberState::Active).await;
    assert!(view.state().error.is_none());
    let seen = seen.lock().unwrap().clone();
    let states = states.lock().unwrap().clone();
    (seen, states)
}

#[tokio::test]
async fn a_dependency_lost_before_it_is_read_returns_the_plugin_to_pending() {
    let (seen, states) = lose_and_restore(false).await;
    assert_eq!(seen, [2]);
    assert!(!states.contains(&FiberState::Failed), "{states:?}");
}

#[tokio::test]
async fn a_dependency_back_before_the_failure_is_handled_still_reloads() {
    let (seen, states) = lose_and_restore(true).await;
    assert_eq!(seen, [2]);
    assert!(!states.contains(&FiberState::Failed), "{states:?}");
}

/// A root whose ErrorSink records everything sent to it.
fn root_with_recording_sink() -> (Ctx, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let sunk = Arc::new(Mutex::new(Vec::new()));
    let sink: ErrorSink = {
        let sunk = sunk.clone();
        Arc::new(move |error| sunk.lock().unwrap().push(error))
    };
    let ctx = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
    (ctx, sunk)
}

/// The `InjectUnsatisfied` that returns a generation to Pending is not a
/// failure anyone has to see: like an eviction, it stays out of the
/// ErrorSink, whether or not the dependency is back before it is handled.
///
/// guarantee: core/typed.lost-dependency-silent
#[tokio::test]
async fn a_lost_dependency_error_does_not_reach_the_error_sink() {
    for hold in [false, true] {
        let (ctx, sunk) = root_with_recording_sink();
        let (seen, states) = lose_and_restore_in(ctx.clone(), hold).await;
        assert_eq!(seen, [2]);
        assert!(!states.contains(&FiberState::Failed), "{states:?}");
        let sunk = sunk.lock().unwrap();
        assert!(
            sunk.is_empty(),
            "hold = {hold}: nothing may reach the sink, got {sunk:?}"
        );
    }
}

/// A `restart()` that lands between the failed read and its handling is
/// answered with `Ok`: the generation goes back to Pending, not to Failed,
/// and the restart has no failure to report. The plugin loads once the
/// dependency is back.
///
/// guarantee: core/typed.lost-dependency-restart
#[tokio::test]
async fn a_restart_in_the_lost_dependency_window_returns_ok() {
    let (ctx, sunk) = root_with_recording_sink();
    let first = ctx.provide(Llm(1)).unwrap();
    let seen = Seen::default();
    let hold = Arc::new(Notify::new());
    let plugin = LosesItsDependency::new(Some(hold.clone()), seen.clone());
    let read = plugin.read.clone();
    let view = ctx.plugin(plugin);
    reach(&view, FiberState::Loading).await;

    let withdrawn = tokio::spawn(first.dispose());
    within("the failed read", read.notified()).await;
    // Registered now, while apply still holds the failed read.
    let mut restart = view.restart();
    tokio::select! {
        biased;
        result = &mut restart => panic!("restart finished inside the window: {result:?}"),
        () = std::future::ready(()) => {}
    }
    hold.notify_one();

    within("the restart", restart)
        .await
        .expect("a restart in the window returns Ok");
    within("the withdrawal", withdrawn).await.unwrap().unwrap();
    let state = view.state();
    assert_eq!(state.state, FiberState::Pending);
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(seen.lock().unwrap().is_empty());

    ctx.provide(Llm(2)).unwrap();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [2]);
    assert!(
        sunk.lock().unwrap().is_empty(),
        "{:?}",
        sunk.lock().unwrap()
    );
}

/// How [`UntypedLoss`] reads its `Llm` and reports it missing.
#[derive(Clone, Copy)]
enum Read {
    /// `get`, then `InjectUnsatisfied`.
    Get,
    /// `require`, then `InjectUnsatisfied`.
    Require,
    /// `require(..)?`: the read's own error.
    RequireAsIs,
}

/// An untyped plugin: its first generation waits until its `Llm` is
/// withdrawn, then reads it as [`Read`] says.
struct UntypedLoss {
    read: Read,
    generations: AtomicUsize,
    seen: Seen,
    injects: Vec<TypeKey>,
}

impl UntypedLoss {
    fn new(read: Read, seen: Seen) -> Self {
        Self {
            read,
            generations: AtomicUsize::new(0),
            seen,
            injects: vec![TypeKey::of::<Llm>()],
        }
    }
}

impl DynPlugin for UntypedLoss {
    fn name(&self) -> &str {
        "untyped-loss"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if self.generations.fetch_add(1, Ordering::SeqCst) == 0 {
                // Withdrawing the provider cancels this generation.
                ctx.cancelled().await;
            }
            let lost = || Error::from(ErrorKind::InjectUnsatisfied(vec!["Llm".into()]));
            let llm = match self.read {
                Read::Get => ctx.get::<Llm>().ok_or_else(lost)?,
                Read::Require => ctx.require::<Llm>().map_err(|_| lost())?,
                Read::RequireAsIs => ctx.require::<Llm>()?,
            };
            self.seen.lock().unwrap().push(llm.0);
            Ok(Effect::Done)
        })
    }
}

/// Provides `Llm(n)` from a sibling plugin. (A service the root provides
/// stays readable to its descendants while it is withdrawn: they are in
/// its subtree.)
struct ProvidesLlm(u32);

impl DynPlugin for ProvidesLlm {
    fn name(&self) -> &str {
        "provides-llm"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.provide(Llm(self.0))?;
            Ok(Effect::Done)
        })
    }
}

/// Mounts an [`UntypedLoss`], withdraws its `Llm` while the first
/// generation loads, and returns the settled outcome of that load.
async fn untyped_loss(read: Read) -> (Ctx, FiberView, Seen, Result<(), Arc<ErrorKind>>) {
    let ctx = Ctx::root().unwrap();
    let provider = ctx.plugin(ProvidesLlm(1));
    within("the provider", (&provider).into_future())
        .await
        .unwrap();
    let seen = Seen::default();
    let view = ctx.plugin(UntypedLoss::new(read, seen.clone()));
    reach(&view, FiberState::Loading).await;
    // Waits for the consumer, which is still loading.
    within("the withdrawal", provider.dispose()).await.unwrap();
    let settled = within("the first load", (&view).into_future()).await;
    (ctx, view, seen, settled)
}

/// An untyped plugin gets the lost-dependency rule by reading with
/// `require`: the failed strict read is the evidence, the generation goes
/// back to Pending without an error and loads when the dependency is back.
///
/// guarantee: core/typed.untyped-require
#[tokio::test]
async fn an_untyped_plugin_whose_require_fails_returns_to_pending() {
    returns_to_pending(Read::Require).await;
}

/// The same with the strict read's own error returned as is
/// (`require(..)?`): it is the same evidence (#438).
///
/// guarantee: core/typed.untyped-require
#[tokio::test]
async fn an_untyped_plugin_returning_its_failed_require_returns_to_pending() {
    returns_to_pending(Read::RequireAsIs).await;
}

async fn returns_to_pending(read: Read) {
    let (ctx, view, seen, settled) = untyped_loss(read).await;
    settled.expect("Pending is not a failure");
    let state = view.state();
    assert_eq!(state.state, FiberState::Pending);
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(seen.lock().unwrap().is_empty());

    ctx.plugin(ProvidesLlm(2));
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [2]);
}

/// A `get` claim in a generation the driver cancelled (the withdrawal here)
/// is no different from the others: the cancellation decides, not the
/// error's kind, so it returns to `Pending` (#968).
///
/// guarantee: core/D7
#[tokio::test]
async fn a_get_claim_in_a_cancelled_generation_returns_to_pending() {
    returns_to_pending(Read::Get).await;
}

/// A `get` that finds nothing is not evidence of a lost dependency: a plugin
/// that claims one after a `get` gets an ordinary, sticky failure. (`get` is
/// not a strict read and the key is not declared, so the generation records
/// no evidence — and nothing cancelled it.)
///
/// guarantee: core/typed.get-not-evidence
#[tokio::test]
async fn a_get_read_is_not_evidence_of_a_lost_dependency() {
    struct GetOnly;

    impl DynPlugin for GetOnly {
        fn name(&self) -> &str {
            "get-only"
        }

        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                assert!(ctx.get::<Llm>().is_none(), "nothing provides Llm");
                Err(Error::from(ErrorKind::InjectUnsatisfied(
                    vec!["Llm".into()],
                )))
            })
        }
    }

    let ctx = Ctx::root().unwrap();
    let view = ctx.plugin(GetOnly);
    let error = within("the load", (&view).into_future())
        .await
        .expect_err("a get read does not count");
    assert!(
        matches!(*error, ErrorKind::InjectUnsatisfied(_)),
        "{error:?}"
    );
    assert_eq!(view.state().state, FiberState::Failed);
    ctx.shutdown().await.unwrap();
}

type Withdrawal = Arc<Mutex<Option<std::thread::JoinHandle<Result<(), Arc<ErrorKind>>>>>>;

/// Withdraws its `Llm` while being validated, after the gate opened and
/// before `apply` reads it.
struct WithdrawnWhileValidating {
    ctx: Ctx,
    withdraw: Mutex<Option<rutis::Disposer>>,
    withdrawal: Withdrawal,
    seen: Seen,
}

impl Plugin for WithdrawnWhileValidating {
    type Deps = Arc<Llm>;

    fn name(&self) -> &str {
        "withdrawn-while-validating"
    }

    fn validate(&self) -> Result<(), Error> {
        if let Some(disposer) = self.withdraw.lock().unwrap().take() {
            // From another thread: a task spawned here would wait in this
            // (blocked) worker's own slot.
            let runtime = tokio::runtime::Handle::current();
            let withdrawal = std::thread::spawn(move || runtime.block_on(disposer.dispose()));
            *self.withdrawal.lock().unwrap() = Some(withdrawal);
            // Only until the removal is marked: the withdrawal waits for this
            // consumer to stop (core/D14), so it cannot end in here. The
            // provider is a child plugin, so this read is not a cleanup-time
            // self-access and fails as soon as the removal is marked.
            let start = std::time::Instant::now();
            while self.ctx.get::<Llm>().is_some() {
                assert!(start.elapsed() < Duration::from_secs(10), "never withdrawn");
                std::thread::yield_now();
            }
        }
        Ok(())
    }

    async fn apply(&self, _: &Ctx, llm: Arc<Llm>) -> rutis::Result {
        self.seen.lock().unwrap().push(llm.0);
        Ok(())
    }
}

/// Provides `Llm` and hands out the disposer of that one service.
struct HandsOutLlm(u32, Arc<Mutex<Option<rutis::Disposer>>>);

impl DynPlugin for HandsOutLlm {
    fn name(&self) -> &str {
        "hands-out-llm"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.1.lock().unwrap() = Some(ctx.provide(Llm(self.0))?);
            Ok(Effect::Done)
        })
    }
}

/// The dependency is withdrawn while the plugin is validated, after the
/// gate: the withdrawal waits for the plugin, which returns to Pending.
///
/// guarantee: core/typed.lost-dependency-pending, core/D14
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dependency_withdrawn_before_apply_starts_returns_the_plugin_to_pending() {
    let ctx = Ctx::root().unwrap();
    let slot = Arc::new(Mutex::new(None));
    let provider = ctx.plugin(HandsOutLlm(1, slot.clone()));
    within("the provider", (&provider).into_future())
        .await
        .unwrap();
    let first = slot.lock().unwrap().take().expect("provided");
    let seen = Seen::default();
    let withdrawal = Withdrawal::default();
    let view = ctx.plugin(WithdrawnWhileValidating {
        ctx: ctx.clone(),
        withdraw: Mutex::new(Some(first)),
        withdrawal: withdrawal.clone(),
        seen: seen.clone(),
    });
    // Settles once the first load has been handled (it starts Pending).
    soon(&view).await.expect("back to Pending, not Failed");
    assert_eq!(view.state().state, FiberState::Pending);
    let withdrawal = withdrawal.lock().unwrap().take().expect("withdrawn");
    soon(tokio::task::spawn_blocking(move || withdrawal.join()))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    ctx.provide(Llm(2)).unwrap();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [2]);
}

/// Claims a lost dependency without a failed read.
struct FalseClaim(Vec<TypeKey>);

impl DynPlugin for FalseClaim {
    fn name(&self) -> &str {
        "false-claim"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.0.clone()
    }

    fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Err(ErrorKind::InjectUnsatisfied(vec!["Llm".into()]).into()) })
    }
}

#[tokio::test]
async fn the_claim_without_a_failed_read_is_a_plain_failure() {
    let ctx = Ctx::root().unwrap();
    ctx.provide(Llm(1)).unwrap();
    let view = ctx.plugin(FalseClaim(vec![TypeKey::of::<Llm>()]));
    let error = soon(&view).await.expect_err("fails instead of looping");
    assert!(matches!(*error, ErrorKind::InjectUnsatisfied(_)));
    assert_eq!(view.state().state, FiberState::Failed);
}

// ── mixing with untyped plugins ─────────────────────────────────

/// Provides `Llm` to whoever asks, typed or not.
struct Provider;

impl Plugin for Provider {
    type Deps = ();

    fn name(&self) -> &str {
        "provider"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.provide(Llm(7))?;
        Ok(())
    }
}

/// An untyped consumer of `Llm`.
struct Untyped {
    got: Arc<Mutex<Option<u32>>>,
    injects: Vec<TypeKey>,
}

impl DynPlugin for Untyped {
    fn name(&self) -> &str {
        "untyped"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.got.lock().unwrap() = Some(ctx.require::<Llm>()?.0);
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn typed_and_untyped_plugins_depend_on_each_other() {
    let ctx = Ctx::root().unwrap();
    let got = Arc::new(Mutex::new(None));
    let untyped = ctx.plugin(Untyped {
        got: got.clone(),
        injects: vec![TypeKey::of::<Llm>()],
    });
    let seen = Seen::default();
    let typed = ctx.plugin(One(seen.clone()));
    let provider = ctx.plugin(Provider);
    soon(&provider).await.unwrap();
    reach(&untyped, FiberState::Active).await;
    reach(&typed, FiberState::Active).await;
    assert_eq!(*got.lock().unwrap(), Some(7));
    assert_eq!(*seen.lock().unwrap(), [7]);

    // The provider going away evicts both kinds of consumer.
    provider.dispose().await.unwrap();
    reach(&untyped, FiberState::Pending).await;
    reach(&typed, FiberState::Pending).await;
}
