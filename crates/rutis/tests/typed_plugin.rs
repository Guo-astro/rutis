//! Typed dependencies (#50): one description gives the gate and the reads.

use std::future::IntoFuture;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::{
    BoxFuture, CordisError, Ctx, DepKey, Deps, Effect, ErrorSink, FiberState, FiberStatusChanged,
    FiberView, Gate, Keyed, KeyedGate, Listener, Plugin, TypeKey, Typed, TypedFactory, TypedPlugin,
    TypedPluginFactory,
};
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

impl TypedPlugin for Chat {
    type Deps = (Arc<Llm>, Option<Arc<Logger>>, Gate<Ready>);

    fn name(&self) -> &str {
        "chat"
    }

    fn apply<'a>(
        &'a self,
        _: &'a Ctx,
        (llm, logger, _): Self::Deps,
    ) -> BoxFuture<'a, Result<Effect, CordisError>> {
        self.0
            .lock()
            .unwrap()
            .push(llm.0 + if logger.is_some() { 100 } else { 0 });
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test]
async fn required_and_gate_dependencies_gate_optional_ones_do_not() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let plugin = Typed::new(Chat(seen.clone()));
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
    let view = ctx.plugin(Typed::new(Chat(seen.clone())));
    soon(&view).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), [101]);
}

#[tokio::test]
async fn losing_a_required_dependency_evicts_and_the_next_one_is_passed() {
    let ctx = Ctx::root().unwrap();
    ctx.provide(Ready).unwrap();
    let first = ctx.provide(Llm(1)).unwrap();
    let seen = Seen::default();
    let view = ctx.plugin(Typed::new(Chat(seen.clone())));
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
    impl TypedPlugin for Twice {
        type Deps = (Arc<Llm>, Option<Arc<Logger>>, Gate<Llm>, Arc<Llm>);
        fn name(&self) -> &str {
            "twice"
        }
        fn apply<'a>(
            &'a self,
            _: &'a Ctx,
            _: Self::Deps,
        ) -> BoxFuture<'a, Result<Effect, CordisError>> {
            Box::pin(async { Ok(Effect::Done) })
        }
    }
    assert_eq!(Typed::new(Twice).injects(), [TypeKey::of::<Llm>()]);
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

impl TypedPlugin for UsesModel {
    type Deps = (Arc<dyn Model>, Option<Arc<dyn Tracer>>);

    fn name(&self) -> &str {
        "uses-model"
    }

    fn apply<'a>(
        &'a self,
        _: &'a Ctx,
        (model, tracer): Self::Deps,
    ) -> BoxFuture<'a, Result<Effect, CordisError>> {
        assert!(tracer.is_none());
        self.0.lock().unwrap().push(model.id());
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test]
async fn trait_object_services_are_dependencies() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let view = ctx.plugin(Typed::new(UsesModel(seen.clone())));
    let model: Arc<dyn Model> = Arc::new(Llm(5));
    ctx.provide_as(TypeKey::of::<dyn Model>(), model).unwrap();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [5]);
}

// ── keys chosen when mounting ───────────────────────────────────

struct Pair(Seen);

impl TypedPlugin for Pair {
    type Deps = (Keyed<Llm>, Option<Keyed<Llm>>, KeyedGate<Ready>);

    fn name(&self) -> &str {
        "pair"
    }

    fn apply<'a>(
        &'a self,
        _: &'a Ctx,
        (primary, backup, _): Self::Deps,
    ) -> BoxFuture<'a, Result<Effect, CordisError>> {
        let mut seen = self.0.lock().unwrap();
        seen.push(primary.0 .0);
        seen.extend(backup.map(|backup| backup.0 .0));
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test]
async fn named_keys_are_chosen_when_mounting() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let keys = (
        DepKey::named("primary"),
        DepKey::dynamic(format!("backup-{}", 1)),
        DepKey::named("ready"),
    );
    let plugin = Typed::with_keys(Pair(seen.clone()), keys);
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

    ctx.provide_as(TypeKey::keyed::<Llm>("primary"), Arc::new(Llm(1)))
        .unwrap();
    ctx.provide_as(TypeKey::keyed::<Llm>("backup-1"), Arc::new(Llm(2)))
        .unwrap();
    ctx.provide_as(TypeKey::keyed::<Ready>("ready"), Arc::new(Ready))
        .unwrap();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [1, 2]);
}

/// Hands out the context of a fiber of its own (a new instance).
struct Capture(Arc<Mutex<Option<Ctx>>>);

impl Plugin for Capture {
    fn name(&self) -> &str {
        "capture"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
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

impl TypedPlugin for One {
    type Deps = Keyed<Llm>;

    fn name(&self) -> &str {
        "one"
    }

    fn apply<'a>(
        &'a self,
        _: &'a Ctx,
        llm: Keyed<Llm>,
    ) -> BoxFuture<'a, Result<Effect, CordisError>> {
        self.0.lock().unwrap().push(llm.0 .0);
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test]
async fn instance_keys_resolve_inside_their_instance_only() {
    let root = Ctx::root().unwrap();
    let a = instance(&root).await;
    let b = instance(&root).await;
    let key = DepKey::<Llm>::of().instance(a.instance());
    a.provide_as(key.key().clone(), Arc::new(Llm(10))).unwrap();

    let seen = Seen::default();
    let inside = a.plugin(Typed::with_keys(One(seen.clone()), key.clone()));
    let outside = b.plugin(Typed::with_keys(One(seen.clone()), key));
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
    let scoped = scope.plugin(Typed::new(One(seen.clone())));
    soon(&scoped).await.unwrap();
    let shared = ctx.plugin(Typed::new(One(seen.clone())));
    soon(&shared).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), [1, 2]);
}

#[tokio::test]
async fn a_failing_check_keeps_the_plugin_pending() {
    let ctx = Ctx::root().unwrap();
    let healthy = Arc::new(AtomicBool::new(false));
    let check = healthy.clone();
    ctx.provide_as_with_check(TypeKey::of::<Llm>(), Arc::new(Llm(1)), move || {
        check.load(Ordering::SeqCst)
    })
    .unwrap();
    let seen = Seen::default();
    let view = ctx.plugin(Typed::new(One(seen.clone())));
    soon(&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Pending);
    healthy.store(true, Ordering::SeqCst);
    ctx.refresh();
    reach(&view, FiberState::Active).await;
    assert_eq!(*seen.lock().unwrap(), [1]);
}

// ── factories ───────────────────────────────────────────────────

/// Builds `Offset` plugins from a configured offset.
struct OffsetFactory(Seen);

struct Offset(u32, Seen);

impl TypedPlugin for Offset {
    type Deps = (Arc<Llm>, Gate<Ready>);

    fn name(&self) -> &str {
        "offset"
    }

    fn apply<'a>(
        &'a self,
        _: &'a Ctx,
        (llm, _): Self::Deps,
    ) -> BoxFuture<'a, Result<Effect, CordisError>> {
        self.1.lock().unwrap().push(llm.0 + self.0);
        Box::pin(async { Ok(Effect::Done) })
    }
}

impl TypedPluginFactory<u32> for OffsetFactory {
    type Plugin = Offset;

    fn build(&self, offset: &u32) -> Result<Offset, CordisError> {
        Ok(Offset(*offset, self.0.clone()))
    }
}

#[tokio::test]
async fn a_typed_factory_declares_from_the_same_description() {
    let ctx = Ctx::root().unwrap();
    let seen = Seen::default();
    let factory = TypedFactory::new(OffsetFactory(seen.clone()));
    let view = ctx.plugin_with(factory, 10u32);
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
struct States(Arc<Mutex<Vec<FiberState>>>);

impl Listener<FiberStatusChanged> for States {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a FiberStatusChanged,
    ) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        self.0.lock().unwrap().push(event.to);
        Box::pin(async { Ok(None) })
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

impl Plugin for LosesItsDependency {
    fn name(&self) -> &str {
        "loses-its-dependency"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let first = self.generations.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                // Withdrawing the provider cancels this generation.
                ctx.cancelled().await;
            }
            let read = <Arc<Llm> as Deps>::resolve(&(), ctx);
            if first {
                assert!(matches!(read, Err(CordisError::InjectUnsatisfied(_))));
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
    ctx.events()
        .on(&ctx, &rutis::EventKey::of(), States(states.clone()))
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
fn root_with_recording_sink() -> (Ctx, Arc<Mutex<Vec<Arc<CordisError>>>>) {
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

impl Plugin for UntypedLoss {
    fn name(&self) -> &str {
        "untyped-loss"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            if self.generations.fetch_add(1, Ordering::SeqCst) == 0 {
                // Withdrawing the provider cancels this generation.
                ctx.cancelled().await;
            }
            let lost = || CordisError::InjectUnsatisfied(vec!["Llm".into()]);
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

impl Plugin for ProvidesLlm {
    fn name(&self) -> &str {
        "provides-llm"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            ctx.provide(Llm(self.0))?;
            Ok(Effect::Done)
        })
    }
}

/// Mounts an [`UntypedLoss`], withdraws its `Llm` while the first
/// generation loads, and returns the settled outcome of that load.
async fn untyped_loss(read: Read) -> (Ctx, FiberView, Seen, Result<(), Arc<CordisError>>) {
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

/// A `get` that finds nothing is not evidence of a lost dependency: the
/// same claim after a `get` is an ordinary, sticky failure.
///
/// guarantee: core/typed.get-not-evidence
#[tokio::test]
async fn a_get_read_is_not_evidence_of_a_lost_dependency() {
    let (_ctx, view, seen, settled) = untyped_loss(Read::Get).await;
    let error = settled.expect_err("a get read does not count");
    assert!(
        matches!(*error, CordisError::InjectUnsatisfied(_)),
        "{error:?}"
    );
    assert_eq!(view.state().state, FiberState::Failed);
    assert!(seen.lock().unwrap().is_empty());
}

type Withdrawal = Arc<Mutex<Option<std::thread::JoinHandle<Result<(), Arc<CordisError>>>>>>;

/// Withdraws its `Llm` while being validated, after the gate opened and
/// before `apply` reads it.
struct WithdrawnWhileValidating {
    ctx: Ctx,
    withdraw: Mutex<Option<rutis::Disposer>>,
    withdrawal: Withdrawal,
    seen: Seen,
}

impl TypedPlugin for WithdrawnWhileValidating {
    type Deps = Arc<Llm>;

    fn name(&self) -> &str {
        "withdrawn-while-validating"
    }

    fn validate(&self) -> Result<(), CordisError> {
        if let Some(disposer) = self.withdraw.lock().unwrap().take() {
            // From another thread: a task spawned here would wait in this
            // (blocked) worker's own slot.
            let runtime = tokio::runtime::Handle::current();
            let withdrawal = std::thread::spawn(move || runtime.block_on(disposer.dispose()));
            *self.withdrawal.lock().unwrap() = Some(withdrawal);
            // The removal is marked as soon as the withdrawal starts.
            let start = std::time::Instant::now();
            while self.ctx.get::<Llm>().is_some() {
                assert!(start.elapsed() < Duration::from_secs(10), "never withdrawn");
                std::thread::yield_now();
            }
        }
        Ok(())
    }

    fn apply<'a>(
        &'a self,
        _: &'a Ctx,
        llm: Arc<Llm>,
    ) -> BoxFuture<'a, Result<Effect, CordisError>> {
        self.seen.lock().unwrap().push(llm.0);
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dependency_withdrawn_before_apply_starts_returns_the_plugin_to_pending() {
    let ctx = Ctx::root().unwrap();
    let first = ctx.provide(Llm(1)).unwrap();
    let seen = Seen::default();
    let withdrawal = Withdrawal::default();
    let view = ctx.plugin(Typed::new(WithdrawnWhileValidating {
        ctx: ctx.clone(),
        withdraw: Mutex::new(Some(first)),
        withdrawal: withdrawal.clone(),
        seen: seen.clone(),
    }));
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

/// Requires `Llm` and records nothing; another consumer, whose refresh
/// evaluates the provider's check.
struct Bystander(Vec<TypeKey>);

impl Plugin for Bystander {
    fn name(&self) -> &str {
        "bystander"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.0
    }

    fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

/// The first generation waits for `go` before reading its `Llm` the typed
/// way, then for `hold` before reporting the read.
struct HeldRead {
    generations: AtomicUsize,
    entered: Arc<Notify>,
    go: Arc<Notify>,
    read: Arc<Notify>,
    hold: Arc<Notify>,
    seen: Seen,
    injects: Vec<TypeKey>,
}

impl Plugin for HeldRead {
    fn name(&self) -> &str {
        "held-read"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let first = self.generations.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                self.entered.notify_one();
                self.go.notified().await;
            }
            let read = <Arc<Llm> as Deps>::resolve(&(), ctx);
            if first {
                assert!(matches!(read, Err(CordisError::InjectUnsatisfied(_))));
                self.read.notify_one();
                self.hold.notified().await;
            }
            self.seen.lock().unwrap().push(read?.0);
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn a_check_rejected_after_the_gate_returns_the_plugin_to_pending_even_if_it_recovers() {
    let ctx = Ctx::root().unwrap();
    let states = Arc::new(Mutex::new(Vec::new()));
    let healthy = Arc::new(AtomicBool::new(true));
    let check = healthy.clone();
    ctx.provide_as_with_check(TypeKey::of::<Llm>(), Arc::new(Llm(1)), move || {
        check.load(Ordering::SeqCst)
    })
    .unwrap();
    let bystander = ctx.plugin(Bystander(vec![TypeKey::of::<Llm>()]));
    soon(&bystander).await.unwrap();
    let seen = Seen::default();
    let plugin = HeldRead {
        generations: AtomicUsize::new(0),
        entered: Arc::new(Notify::new()),
        go: Arc::new(Notify::new()),
        read: Arc::new(Notify::new()),
        hold: Arc::new(Notify::new()),
        seen: seen.clone(),
        injects: vec![TypeKey::of::<Llm>()],
    };
    let (entered, go, read, hold) = (
        plugin.entered.clone(),
        plugin.go.clone(),
        plugin.read.clone(),
        plugin.hold.clone(),
    );
    let view = ctx.plugin(plugin);
    soon(entered.notified()).await;
    ctx.events()
        .on(&ctx, &rutis::EventKey::of(), States(states.clone()))
        .unwrap();

    // Same binding, but its check now rejects; the bystander's refresh
    // records that before the held plugin reads.
    healthy.store(false, Ordering::SeqCst);
    ctx.refresh();
    reach(&bystander, FiberState::Pending).await;
    go.notify_one();
    soon(read.notified()).await;

    // The check passes again before the failed read is handled.
    healthy.store(true, Ordering::SeqCst);
    ctx.refresh();
    reach(&bystander, FiberState::Active).await;
    hold.notify_one();

    reach(&view, FiberState::Active).await;
    assert!(view.state().error.is_none());
    assert_eq!(*seen.lock().unwrap(), [1]);
    let states = states.lock().unwrap().clone();
    assert!(!states.contains(&FiberState::Failed), "{states:?}");
}

/// Claims a lost dependency without a failed read.
struct FalseClaim(Vec<TypeKey>);

impl Plugin for FalseClaim {
    fn name(&self) -> &str {
        "false-claim"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.0
    }

    fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async { Err(CordisError::InjectUnsatisfied(vec!["Llm".into()])) })
    }
}

#[tokio::test]
async fn the_claim_without_a_failed_read_is_a_plain_failure() {
    let ctx = Ctx::root().unwrap();
    ctx.provide(Llm(1)).unwrap();
    let view = ctx.plugin(FalseClaim(vec![TypeKey::of::<Llm>()]));
    let error = soon(&view).await.expect_err("fails instead of looping");
    assert!(matches!(*error, CordisError::InjectUnsatisfied(_)));
    assert_eq!(view.state().state, FiberState::Failed);
}

// ── mixing with untyped plugins ─────────────────────────────────

/// Provides `Llm` to whoever asks, typed or not.
struct Provider;

impl TypedPlugin for Provider {
    type Deps = ();

    fn name(&self) -> &str {
        "provider"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx, (): ()) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            ctx.provide(Llm(7))?;
            Ok(Effect::Done)
        })
    }
}

/// An untyped consumer of `Llm`.
struct Untyped {
    got: Arc<Mutex<Option<u32>>>,
    injects: Vec<TypeKey>,
}

impl Plugin for Untyped {
    fn name(&self) -> &str {
        "untyped"
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
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
    let typed = ctx.plugin(Typed::new(One(seen.clone())));
    let provider = ctx.plugin(Typed::new(Provider));
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
