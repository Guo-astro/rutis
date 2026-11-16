//! guarantee: bridge/marked-services
//! Shuttle models of a marked service's name (#622's `bridge/marked-services b`,
//! `MSV-4-2a`, `MSV-4-2e`, `MSV-4-2f`, `MSV-4-3b`, `MSV-4-6`).
//!
//! The models run the production path: `Ctx::provide`/`Ctx::provide_at` and
//! a provide's `Disposer::dispose` (`crates/rutis/src/ctx.rs`), the bridge's
//! `Exposer::admit`/`Exposer::expose` and the claim registry in
//! [`super`]. The kernel locks the path takes are shuttle's under
//! `--cfg rutis_shuttle` (`crates/rutis/src/sync.rs`: the shared admission
//! lock, a fiber's transition and effects), so two provides, a provide and a
//! disposal, and a claim and its loader interleave at every admission and
//! transition. The claim registry's own lock (`CLAIMS` in this module) stays
//! std's: its critical sections are HashMap operations only, entered and left
//! without a shuttle operation, as `crates/rutis/src/sync.rs` requires.
//!
//! What is written out:
//!
//! - **The runtime that runs a cleanup.** A provide's disposer drains its
//!   effect record, which the kernel spawns through `crate::sync::spawn`; in
//!   a plain build that is `Handle::spawn`. Each model builds a
//!   current-thread runtime on the thread that disposes and drives the
//!   cleanup with `Runtime::block_on` on that same thread, so the cleanup
//!   runs where the model's other shuttle threads can interleave with it at
//!   its locks, not on a foreign OS thread.
//! - **The loader that reads a claim.** `Loader::rekeying` is the production
//!   listener; a model listener stands in for it: it counts the notifications
//!   and would rebuild its copies. The loader's own resolution (`marked_claim`,
//!   `NameKey::key`, the reconcile a claim starts) is not run here; it is
//!   pinned by `rutis-loader/tests/marked_services.rs` (an instance's rows get
//!   the instance's service) and the loader's
//!   `announcements_while_a_rekeying_waits_queue_no_other`.
//!
//! What each model checks, and the mutation it catches:
//!
//! 1. `shuttle_a_marked_service_is_served_after_its_native_and_withdrawn_before_it`
//!    (`MSV-4-2a`, `MSV-4-2f`, `bridge/marked-services b`): an observer polls
//!    "served by name ⇒ the native service is served". The provider registers
//!    the native service first, then the name, and its one disposer withdraws
//!    the name first, then the native service; after it returns, neither is
//!    left. Each of these was made in `ctx.rs` and reverted: exposing the name
//!    before the native service (`expose` before `provide_native`) makes the
//!    observer see the name with no native service behind it; disposing the
//!    native disposer first (`chain(native, exposed)`) makes it see the name
//!    after the native one is gone; returning only the native disposer leaves
//!    the name served after the dispose.
//! 2. `shuttle_a_refused_marked_provide_leaves_nothing` (`MSV-4-2e`,
//!    `MSV-4-6`): a name another marked type holds refuses the second
//!    provide with `ServiceExists` naming the holder, and the refused type's
//!    native service is not registered; a native slot an unmarked binding
//!    holds refuses a marked provide before its name is ever admitted, and the
//!    name stays absent. An `Exposer::admit` that accepts every name (returned
//!    `Ok(())` without `vacant`) makes the name refusal arrive at `expose`
//!    instead, and the native service the refused provide registered is still
//!    there when it returns: the `ctx.get::<Clash>().is_none()` assertion
//!    fails.
//! 3. `shuttle_two_provides_of_one_name_leave_one_service` (`MSV-4-2e`,
//!    `MSV-4-6`): two types with one name provided concurrently: exactly one
//!    succeeds, the other fails with `ServiceExists`, the name reaches the
//!    winner, and the loser's native service is gone once its refusal's
//!    cleanup has run. Not disposing the native binding on an `expose`
//!    failure (the `self.handle().spawn(native.dispose())` in `ctx.rs`) leaves
//!    it registered: the "a refused provide left its native service
//!    registered" assertion fails.
//! 4. `shuttle_an_instance_claims_its_name_once_and_releases_it_once`
//!    (`MSV-4-3b`): providing in an instance claims the name there — the
//!    claim is visible to the loader (`claimed_in`/`claims_in`) and notifies
//!    its listener once; the global name is not served; disposing releases the
//!    claim without notifying, and providing again claims it again. An
//!    observer polls "served by name ⇒ the native service is served" over the
//!    instance's keys. Returning the served dispatcher without the claim
//!    effect (`Exposer::expose`) fails the claim assertions; a claim cleanup
//!    that does not call `unclaim` fails the "the dispose left the claim
//!    behind" assertion.
//!
//! The message a taken name gives is who holds it (`the host`, a row), not
//! both types as `MSV-4-6`'s wording says: #937 settled the one sentence every
//! provider gives (`docs/guarantees.md` `bridge/marked-services` promises only
//! that the second `provide` fails), and the model asserts the sentence that
//! is implemented.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! `RUTIS_SHUTTLE_ITERATIONS` sets the iterations of each scheduler: 1000 by
//! default, as on pull requests. A failure prints its schedule;
//! `RUTIS_SHUTTLE_REPLAY=<schedule>` replays it, with the failing test's name
//! as the filter.

use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

use shuttle::thread;

use rutis::host::diagnostics::BindingDiagnostics;
use rutis::host::TypeKey;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, InstanceId, Key};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};

use crate::session::{host_key, host_key_in, HostDispatch, Value};
use crate::sync::{Count, Flag};

use super::{claimed_in, claims_in, on_claim};

/// Iterations of each scheduler: `RUTIS_SHUTTLE_ITERATIONS`, or 1000.
fn iterations() -> usize {
    std::env::var("RUTIS_SHUTTLE_ITERATIONS")
        .ok()
        .map(|value| value.parse().expect("RUTIS_SHUTTLE_ITERATIONS is a number"))
        .unwrap_or(1000)
}

/// Explore `model` with shuttle's random scheduler and with PCT.
fn explore<F>(model: F)
where
    F: Fn() + Send + Sync + 'static,
{
    if let Ok(schedule) = std::env::var("RUTIS_SHUTTLE_REPLAY") {
        shuttle::replay(model, schedule.trim().trim_matches('"'));
        return;
    }
    let model = Arc::new(model);
    let iterations = iterations();
    let random = model.clone();
    shuttle::check_random(move || random(), iterations);
    shuttle::check_pct(move || model(), iterations, 3);
}

/// Explore a model that runs no concurrent threads (it runs the production
/// path on one thread) with shuttle's random scheduler: it still needs a
/// shuttle execution, because the kernel's locks are shuttle's in this build.
fn explore_solo<F>(model: F)
where
    F: Fn() + Send + Sync + 'static,
{
    if let Ok(schedule) = std::env::var("RUTIS_SHUTTLE_REPLAY") {
        shuttle::replay(model, schedule.trim().trim_matches('"'));
        return;
    }
    shuttle::check_random(model, iterations());
}

/// A current-thread runtime for the cleanup a disposer spawns.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
}

/// Clears the observer's `done` flag on the way out, panic or not, so a failed
/// provider does not leave the observer spinning.
struct Finish(Arc<Flag>);

impl Drop for Finish {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn named(ctx: &Ctx, name: &str) -> bool {
    ctx.get_at::<dyn HostDispatch>(&host_key(name)).is_some()
}

/// Whether `key` has a binding that is not being removed. Read only from one
/// `bindings_snapshot`, so two answers describe the same instant: the
/// registry takes one lock for the whole list, and a binding is inserted,
/// marked `removing` and finalized under it.
fn live(bindings: &[BindingDiagnostics], key: &TypeKey) -> bool {
    bindings
        .iter()
        .any(|binding| &binding.key == key && !binding.removing)
}

/// The registry's bindings at one moment.
fn snapshot(ctx: &Ctx) -> Vec<BindingDiagnostics> {
    ctx.diagnostics().bindings
}

// ── The services ────────────────────────────────────────────────

/// Served as `shuttle-served`.
struct Served;

#[rutis::service(name = "shuttle-served")]
impl Served {
    pub fn ping(&self) -> u32 {
        7
    }
}

/// The second type of the same name: providing it in one scope is `MSV-4-6`'s
/// clash.
struct Clash;

#[rutis::service(name = "shuttle-served")]
impl Clash {
    pub fn ping(&self) -> u32 {
        8
    }
}

/// Marked, but never provided by name: the native-slot half of `MSV-4-2e`.
struct Muted;

#[rutis::service(name = "shuttle-muted")]
impl Muted {
    pub fn ping(&self) -> u32 {
        9
    }
}

fn ping(reply: crate::session::Reply) -> u32 {
    match reply {
        Ok(Value::Data(value)) => value.as_u64().expect("a number") as u32,
        Ok(other) => panic!("the name answered {other:?}, not the native value"),
        Err(error) => panic!("the name did not reach the native service: {error}"),
    }
}

// ── 1. Served after the native, withdrawn before it ────────────

/// See the module docs, model 1.
#[test]
fn shuttle_a_marked_service_is_served_after_its_native_and_withdrawn_before_it() {
    explore(|| {
        let start = Arc::new(shuttle::sync::Barrier::new(2));
        let done = Arc::new(Flag::new(false));
        // A shuttle thread must never block on a std channel: the provider
        // leaves its context in a slot and both meet at the barrier.
        let slot: Arc<std::sync::Mutex<Option<Ctx>>> = Arc::new(std::sync::Mutex::new(None));

        let provider = {
            let start = start.clone();
            let done = done.clone();
            let slot = slot.clone();
            thread::spawn(move || {
                let _finish = Finish(done);
                let runtime = runtime();
                let ctx = Ctx::root_with(runtime.handle().clone());
                *slot.lock().unwrap() = Some(ctx.clone());
                start.wait();
                let disposer = ctx.provide(Served).expect("the marked provide");
                assert!(
                    ctx.get::<Served>().is_some(),
                    "the native service is not served after its provide"
                );
                // The name is served only once the native service is: a call
                // by name reaches the provided value.
                let served = ctx
                    .get_at::<dyn HostDispatch>(&host_key("shuttle-served"))
                    .expect("the name after the native service");
                assert_eq!(
                    ping(served.invoke("ping", Value::List(Vec::new()))),
                    7,
                    "the name did not reach the value it was provided with"
                );
                runtime
                    .block_on(disposer.dispose())
                    .expect("the one disposer");
                assert!(
                    ctx.get::<Served>().is_none(),
                    "the native service outlived its disposer"
                );
                assert!(
                    !named(&ctx, "shuttle-served"),
                    "the name outlived the disposer that withdrew it"
                );
            })
        };

        let ctx = {
            start.wait();
            slot.lock()
                .unwrap()
                .clone()
                .expect("the provider leaves its context")
        };
        let observer = {
            let done = done.clone();
            thread::spawn(move || {
                let name_key = host_key("shuttle-served").erase();
                let native_key = Key::<Served>::of().erase();
                while !done.load(Ordering::SeqCst) {
                    let bindings = snapshot(&ctx);
                    let by_name = live(&bindings, &name_key);
                    let native = live(&bindings, &native_key);
                    assert!(
                        !by_name || native,
                        "a call by name found the name served and the native service gone"
                    );
                    thread::yield_now();
                }
            })
        };

        provider.join().expect("the provider");
        observer.join().expect("the observer");
    });
}

// ── 2. A refused provide leaves nothing ────────────────────────

/// See the module docs, model 2.
#[test]
fn shuttle_a_refused_marked_provide_leaves_nothing() {
    explore_solo(|| {
        let runtime = runtime();
        let ctx = Ctx::root_with(runtime.handle().clone());

        // The name is held: the second type is refused, and neither its native
        // service nor a name of its own is registered.
        let served = ctx.provide(Served).expect("the first type");
        let error = ctx.provide(Clash).expect_err("the clash is refused");
        assert!(
            matches!(error.kind(), Some(ErrorKind::ServiceExists(_))),
            "the second provide failed with {error:?}, not a taken name"
        );
        assert_eq!(
            error.to_string(),
            "shuttle-served is already provided by the host: rename it"
        );
        assert!(
            ctx.get::<Clash>().is_none(),
            "the refused provide registered its native service"
        );
        let shown = ctx
            .get_at::<dyn HostDispatch>(&host_key("shuttle-served"))
            .expect("the name after the refusal");
        assert_eq!(
            ping(shown.invoke("ping", Value::List(Vec::new()))),
            7,
            "the refusal changed who the name serves"
        );

        // The native slot is held by an unmarked binding: the refused provide
        // checked it before admitting the name, so no name appears either.
        let (muted, _writer) = ctx
            .provide_mut_at(Key::<Muted>::of(), Arc::new(Muted))
            .expect("the mutable binding");
        let error = ctx.provide(Muted).expect_err("the slot is taken");
        assert!(
            matches!(error.kind(), Some(ErrorKind::ServiceExists(_))),
            "the native-slot refusal failed with {error:?}, not a taken name"
        );
        assert!(
            !named(&ctx, "shuttle-muted"),
            "a provide refused for its native slot served the name anyway"
        );
        assert!(
            ctx.get::<Muted>().is_some(),
            "the refusal disturbed the binding that held the slot"
        );

        // Nothing above stopped the first name from working.
        assert!(
            named(&ctx, "shuttle-served"),
            "the refusals withdrew the name they did not provide"
        );
        runtime
            .block_on(muted.dispose())
            .expect("the mutable binding's disposer");
        runtime
            .block_on(served.dispose())
            .expect("the first provide's disposer");
    });
}

// ── 3. One name, two provides, one winner ──────────────────────

struct Attempt {
    disposer: Option<rutis::Disposer>,
    error: Option<String>,
    exists: bool,
}

fn attempt(result: Result<rutis::Disposer, RutisError>) -> Attempt {
    match result {
        Ok(disposer) => Attempt {
            disposer: Some(disposer),
            error: None,
            exists: false,
        },
        Err(error) => Attempt {
            disposer: None,
            error: Some(error.to_string()),
            exists: matches!(error.kind(), Some(ErrorKind::ServiceExists(_))),
        },
    }
}

/// See the module docs, model 3.
#[test]
fn shuttle_two_provides_of_one_name_leave_one_service() {
    explore(|| {
        let runtime = runtime();
        let ctx = Ctx::root_with(runtime.handle().clone());
        let start = Arc::new(shuttle::sync::Barrier::new(2));

        let served = {
            let (ctx, start) = (ctx.clone(), start.clone());
            thread::spawn(move || {
                start.wait();
                attempt(ctx.provide(Served))
            })
        };
        let clash = {
            let (ctx, start) = (ctx.clone(), start.clone());
            thread::spawn(move || {
                start.wait();
                attempt(ctx.provide(Clash))
            })
        };
        let Attempt {
            disposer: served_disposer,
            error: served_error,
            exists: served_exists,
        } = served.join().expect("the first provide");
        let Attempt {
            disposer: clash_disposer,
            error: clash_error,
            exists: clash_exists,
        } = clash.join().expect("the second provide");

        let (winner, winner_ping, loser_exists, loser_error) =
            match (served_disposer, clash_disposer) {
                (Some(disposer), None) => (disposer, 7, clash_exists, clash_error),
                (None, Some(disposer)) => (disposer, 8, served_exists, served_error),
                _ => panic!("two provides of one name did not leave exactly one winner"),
            };
        assert!(
            loser_exists,
            "the loser failed with {loser_error:?}, not a taken name"
        );
        let message = loser_error.expect("the loser's error");
        assert!(
            message.contains("shuttle-served") && message.contains("the host"),
            "the refusal says {message:?}, which does not name the name and its holder"
        );

        // The loser's native service may still be registered until the cleanup
        // its refusal spawned runs; drive it, then it must be gone.
        let loser_present = |ctx: &Ctx| {
            if winner_ping == 7 {
                ctx.get::<Clash>().is_some()
            } else {
                ctx.get::<Served>().is_some()
            }
        };
        for _ in 0..1000 {
            if !loser_present(&ctx) {
                break;
            }
            runtime.block_on(tokio::task::yield_now());
        }
        assert!(
            !loser_present(&ctx),
            "a refused provide left its native service registered"
        );

        // The name serves the winner, and the winner's native service is
        // behind it.
        let shown = ctx
            .get_at::<dyn HostDispatch>(&host_key("shuttle-served"))
            .expect("the name after the race");
        assert_eq!(
            ping(shown.invoke("ping", Value::List(Vec::new()))),
            winner_ping,
            "the name does not reach the provide that won it"
        );

        runtime
            .block_on(winner.dispose())
            .expect("the winner's disposer");
        assert!(
            !named(&ctx, "shuttle-served"),
            "the name outlived the disposer that withdrew it"
        );
        assert!(!loser_present(&ctx), "the loser's native service came back");
    });
}

// ── 4. An instance claims its name ─────────────────────────────

/// A plugin that hands its context back, to get an instance's context.
struct Mount(mpsc::Sender<(InstanceId, Ctx)>);

impl DynPlugin for Mount {
    fn name(&self) -> &str {
        "shuttle-instance"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            self.0
                .send((CtxHost::instance(ctx), ctx.clone()))
                .expect("the model takes the instance");
            Ok(Effect::Done)
        })
    }
}

fn instance(root: &Ctx, runtime: &tokio::runtime::Runtime) -> (InstanceId, Ctx) {
    let (tx, rx) = mpsc::channel();
    runtime.block_on(async {
        root.plugin(Mount(tx)).await.expect("the instance plugin");
    });
    rx.recv().expect("the plugin's context")
}

/// See the module docs, model 4.
#[test]
fn shuttle_an_instance_claims_its_name_once_and_releases_it_once() {
    explore(|| {
        let runtime = runtime();
        let root = Ctx::root_with(runtime.handle().clone());
        let (id, ctx) = instance(&root, &runtime);

        // The loader's listener: production calls it on a new claim, and a
        // loader would rebuild its copies there.
        let notified = Arc::new(Count::new(0));
        let listener: Arc<dyn Fn() + Send + Sync> = {
            let notified = notified.clone();
            Arc::new(move || {
                notified.fetch_add(1, Ordering::SeqCst);
            })
        };
        on_claim(&listener);

        let start = Arc::new(shuttle::sync::Barrier::new(2));
        let served = {
            let (ctx, start) = (ctx.clone(), start.clone());
            thread::spawn(move || {
                start.wait();
                attempt(ctx.provide_at(Key::<Served>::of().instance(id), Arc::new(Served)))
            })
        };
        let clash = {
            let (ctx, start) = (ctx.clone(), start.clone());
            thread::spawn(move || {
                start.wait();
                attempt(ctx.provide_at(Key::<Clash>::of().instance(id), Arc::new(Clash)))
            })
        };
        let Attempt {
            disposer: served_disposer,
            ..
        } = served.join().expect("the first provide");
        let Attempt {
            disposer: clash_disposer,
            ..
        } = clash.join().expect("the second provide");

        let (mut winner, winner_ping) = match (served_disposer, clash_disposer) {
            (Some(disposer), None) => (disposer, 7),
            (None, Some(disposer)) => (disposer, 8),
            _ => panic!("two provides of one name in an instance left no single winner"),
        };
        assert_eq!(
            claims_in(id),
            std::iter::once("shuttle-served".to_owned()).collect(),
            "the instance did not claim exactly the name it serves"
        );
        assert!(
            claimed_in(id, "shuttle-served"),
            "the instance's claim is not visible to the loader"
        );
        assert_eq!(
            notified.load(Ordering::SeqCst),
            1,
            "a new claim did not notify the loader exactly once"
        );
        assert!(
            !named(&root, "shuttle-served"),
            "a service provided in an instance is served globally"
        );
        let shown = ctx
            .get_at::<dyn HostDispatch>(&host_key_in("shuttle-served", id))
            .expect("the instance's name inside it");
        assert_eq!(
            ping(shown.invoke("ping", Value::List(Vec::new()))),
            winner_ping,
            "the instance's name does not reach the value it was provided with"
        );

        // An observer: the name is served only while the native service
        // exists. Both answers come from one binding snapshot; a claim is not
        // read alongside them, because its own lock is not the registry's and
        // one read taken after a switch could describe another moment (the
        // claim checks below are made between the threads' turns).
        let done = Arc::new(Flag::new(false));
        let observer = {
            let (ctx, done) = (ctx.clone(), done.clone());
            thread::spawn(move || {
                let name_key = host_key_in("shuttle-served", id).erase();
                let served_key = Key::<Served>::of().instance(id).erase();
                let clash_key = Key::<Clash>::of().instance(id).erase();
                while !done.load(Ordering::SeqCst) {
                    let bindings = snapshot(&ctx);
                    let by_name = live(&bindings, &name_key);
                    let native = live(&bindings, &served_key) || live(&bindings, &clash_key);
                    assert!(
                        !by_name || native,
                        "a call by name found the name served and the native service gone"
                    );
                    thread::yield_now();
                }
            })
        };

        runtime
            .block_on(winner.dispose())
            .expect("the winner's disposer");
        done.store(true, Ordering::SeqCst);
        observer.join().expect("the observer");

        assert!(
            !claimed_in(id, "shuttle-served"),
            "the dispose left the claim behind"
        );
        assert!(
            !ctx.get_at::<dyn HostDispatch>(&host_key_in("shuttle-served", id))
                .is_some(),
            "the dispose left the instance's name served"
        );
        assert_eq!(
            notified.load(Ordering::SeqCst),
            1,
            "releasing a claim notified the loader"
        );

        // A new claim is a new notification: the loader would rebuild the
        // copies it built for the first one.
        winner = ctx
            .provide_at(Key::<Served>::of().instance(id), Arc::new(Served))
            .expect("the instance's provide again");
        assert!(
            claimed_in(id, "shuttle-served"),
            "providing again did not claim the name again"
        );
        assert_eq!(
            notified.load(Ordering::SeqCst),
            2,
            "a new claim did not notify the loader"
        );
        assert_eq!(
            ctx.provide_at(Key::<Clash>::of().instance(id), Arc::new(Clash))
                .expect_err("the name is still taken")
                .to_string(),
            "shuttle-served is already provided by the host: rename it",
            "the clash inside the instance does not name the holder"
        );
        runtime
            .block_on(winner.dispose())
            .expect("the second disposer");
        drop(listener);
    });
}
