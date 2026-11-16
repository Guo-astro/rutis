//! Shuttle models of the three places in the kernel where an interleaving
//! has gone wrong before (#179; standard Q6.1.3, risk K2). Shuttle runs the
//! threads below one at a time and picks who runs next at every sync
//! operation it controls, so each iteration tries one interleaving, and a
//! failing one is printed as a schedule that `shuttle::replay` runs again.
//!
//! The models run the kernel's own code, not a copy of it: `TransitionTask`
//! and `join_task`, [`post_join_to`] (what `FiberInner::post_join` runs),
//! `EffectRecord::{claim, finish, join}` (what `EffectRecord::drain` runs),
//! and — for registration, event flights and the close passes — a real `Ctx`
//! built on a tokio runtime handle, so `register_effect_named`,
//! `Entry::start`, `set_state` and `wait_events` are the kernel's too (model
//! 22's deadline is the model's clock, `crate::sync::deadline`). Models
//! 7–9 and 12 are reference models: their comments say which bookkeeping
//! cannot be run here and what stands in for it; models 10 and 11 run the
//! bus's `Table`, `Selected` and `Entry` and mirror the emit chain and the
//! notice delivery. Model 13 runs a record drained early against the unload
//! that takes its list; model 14 pins what several completions do to one task;
//! model 15 mirrors `load`/`unload` to check the live generation the kernel
//! publishes across restarts; model 16 closes a real fiber tree while its
//! listeners dispatch, so `CloseTree for FiberInner` and the walk over the real
//! child lists run, not a stand-in. Model 17 closes a root with the child it
//! mounted, through the mount record that owns it; model 18 pins one
//! transition's result across its driver's exit (#1194);
//! model 19 adds a restart inside the dependency window model 9 covers; model
//! 20 installs the spawn stand-in (`crate::sync::SpawnScope`), so the tasks
//! the kernel spawns run on shuttle's executor and a model drives them; model
//! 21 mounts a child on the root while the root closes, over the real admission
//! and transition locks; model 22 waits on two fibers' calls with the deadline
//! timer, which the model moves (`crate::sync::Clock`); model 23 races a
//! restart against a terminal dispose (#622's "待复现的疑点"), and model 24
//! orders one binding's `Provided` before its `Removed` (#622's third doubt).
//! Model 25 runs the kernel's own `load` and `flush_status` on three fibers
//! restarting at once, to check the order `FiberStatusChanged` is delivered in
//! (`core/D24a`); model 26 runs a mount plugin's cleanup effect and
//! its service through the real reverse pass (`MNT-6-4`); model 27 unloads a row
//! whose `Ctx` a method registered on, while another thread registers
//! (`MNT-11-9b`). Models 26 and 27 install the spawn stand-in too, because the
//! cleanup pass they run is the kernel's.
//! Built with `--cfg rutis_shuttle`, `crate::sync` makes the driver's `alive`
//! flag, a fiber's `lineage_closed` marker, an effect record's state, a spawned
//! task's result and the admission, transition, effects and flight locks
//! shuttle primitives; the tokio channels in between
//! (`watch`, `mpsc`, `Notify`) need no runtime and are single steps to shuttle.
//! Shuttle switches just before an operation on its primitives, so
//! the windows the code guards that no primitive opens are marked with
//! `crate::sync::switch_point()` (nothing in a plain build): between
//! `post_join`'s check and its send, between an effect join's check and its
//! wait, and inside an effect factory, between the two registration checks.
//! What stands in for the rest of the fiber (the driver's exit, the unload's
//! async steps) is written out below, each next to the code it follows;
//! `drain()`'s spawned runner is the kernel's (`crate::sync::spawn`).
//!
//! What is *not* modelled here, and why (#622's remaining items):
//!
//! - **The sequential property test over operation sequences.** It belongs to
//!   `crates/rutis/tests/interleave.rs`: its operations (mount, restart,
//!   update, dispose, `dispose_self`, a failing or panicking `apply`) run
//!   through `FiberView`'s public futures, which post to a driver task on the
//!   runtime. A shuttle thread that parks on that mailbox stops shuttle's
//!   scheduler, so a model would have to write the driver out — and a
//!   reference model compared against a written-out driver tests the copy, not
//!   the kernel. Its one kernel-only piece, a panic in the driver, is out for
//!   the same reason: `recover_driver_panic` is the code a written-out driver
//!   would replace. It is pinned by
//!   `crates/rutis/tests/contract.rs::driver_panic_completes_waiters_with_error`.
//! - **The child process half of an ending runtime (#623).** The kernel sees
//!   it only as an event call that never returns. Its own part, the unload
//!   deadline — `wait_events` giving up at `shutdown_deadline()` and reporting
//!   `ListenersStillRunning` — is model 22, driven by the model's clock. What
//!   is not modelled here is the channel that ends before that: the bridge's
//!   read thread, exit status and close paths (`crates/rutis-bridge`), which
//!   are #623's models (`crates/rutis-bridge/src/pipe/spawn/shuttle_tests.rs`).
//!   The plain build's timer (a `std::thread` with
//!   `recv_timeout`, its limit the process-wide `RUTIS_SHUTDOWN_TIMEOUT`) is
//!   not a shuttle subject — real time is not an interleaving — and is pinned
//!   end to end by
//!   `crates/rutis/tests/unload_deadline.rs::an_unload_gives_up_on_running_listeners_at_the_deadline`.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! `RUTIS_SHUTTLE_ITERATIONS` sets the iterations of each scheduler: 1000
//! by default, as on pull requests; the nightly run (`stress.yml`) uses a
//! million. A failure prints its schedule (`failing schedule: "…"`);
//! `RUTIS_SHUTTLE_REPLAY=<schedule>` replays it, with the failing test's
//! name as the filter.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool as StdBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use shuttle::future::block_on;
use shuttle::sync::atomic::{AtomicBool, AtomicUsize as ShuttleUsize};
use shuttle::sync::{Barrier as ShuttleBarrier, Condvar as ShuttleCondvar};
use shuttle::thread;
use tokio::sync::{mpsc, watch, Notify};

use super::{
    complete_intent, join_task, mark_subtree_lineage, post_join_to, publish_subtree_closing,
    shutdown_deadline, spawn_fiber, CloseTree, FiberInner, FiberState, FiberStatusChanged, Intent,
    NextState, PluginId, TransitionTask, NOT_LIVE,
};
use crate::bus::run::{Inv, SyncNext};
use crate::bus::table::{Body, Entry, Selected, Table};
use crate::effect::{run_cleanups, Cleanup, Effect, EffectRecord, Origin};
use crate::error::{aggregate_arcs, Error, ErrorKind, ErrorSink};
use crate::events::{ErasedSync, Event, ListenerInfo, TableChange, TableEntry, Value};
use crate::host::CtxHost;
use crate::internal::CtxInternal;
use crate::key::{ScopeId, TypeKey};
use crate::sync::{
    switch_point, AtomicBool as SyncBool, Clock, Mutex as SyncMutex, SpawnScope, TaskSet,
};
use crate::Ctx;

/// Iterations of each scheduler: `RUTIS_SHUTTLE_ITERATIONS`, or 1000.
fn iterations() -> usize {
    std::env::var("RUTIS_SHUTTLE_ITERATIONS")
        .ok()
        .map(|value| value.parse().expect("RUTIS_SHUTTLE_ITERATIONS is a number"))
        .unwrap_or(1000)
}

/// Explore `model` with shuttle's random scheduler and with PCT (which
/// finds bugs that need a few preemptions at the right places with better
/// odds than uniform choices), or replay `RUTIS_SHUTTLE_REPLAY`.
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

fn failed(what: &str) -> Arc<ErrorKind> {
    Arc::new(ErrorKind::PluginFailed(what.into()))
}

/// Model 1, TransitionTask lost wakeup (cordis-spec-parity §5.2, item 3): the
/// task's watch channel kept no receiver, so a `complete` before anyone
/// joined was dropped and a later join waited forever. The keep-alive
/// receiver holds the result for joins that come later; every join, early
/// or late, gets the one result (the same `Arc`, D25).
#[test]
fn shuttle_transition_task_reaches_every_join() {
    explore(|| {
        let task = TransitionTask::new();
        let error = failed("transition");
        let completer = {
            let (task, error) = (task.clone(), error.clone());
            thread::spawn(move || task.complete(Some(error)))
        };
        let joins: Vec<_> = (0..2)
            .map(|_| {
                let task = task.clone();
                thread::spawn(move || block_on(join_task(&task)))
            })
            .collect();
        completer.join().unwrap();
        for join in joins {
            let got = join.join().unwrap().expect_err("the task failed");
            assert!(Arc::ptr_eq(&got, &error), "a join got another error");
        }
    });
}

/// Model 2, the `post_join` exit race (design-rust-port, review item 24;
/// design-config-hot-update §10 item 4): a poster finds the driver alive, the driver then
/// stops and drains its mailbox, and only then does the post land, with
/// nobody left to read it. `post_join` checks `alive` again after sending,
/// so the task completes either way; its join must always return.
///
/// The driver stands in for `drive()`'s exit (terminal dispose and
/// shutdown): it may handle one intent while running, then sets `alive`
/// false, drains once, and keeps its receiver open while it releases what
/// the fiber held (`release_transient(..).await`), as `drive()` does.
#[test]
fn shuttle_post_join_completes_across_driver_exit() {
    explore(|| {
        let alive = Arc::new(AtomicBool::new(true));
        let (intents, mut mailbox) = mpsc::unbounded_channel::<Intent>();
        let driver = {
            let alive = alive.clone();
            thread::spawn(move || {
                if let Ok(intent) = mailbox.try_recv() {
                    complete_intent(&intent, None);
                }
                alive.store(false, Ordering::SeqCst);
                while let Ok(intent) = mailbox.try_recv() {
                    complete_intent(&intent, Some(Arc::new(ErrorKind::Closed)));
                }
                // release_transient(..).await: the mailbox is still open.
                thread::yield_now();
                drop(mailbox);
            })
        };
        let posters: Vec<_> = (0..2)
            .map(|_| {
                let (alive, intents) = (alive.clone(), intents.clone());
                thread::spawn(move || {
                    let task = TransitionTask::new();
                    post_join_to(&alive, &intents, task.clone(), Intent::Settle, || {
                        Some(Arc::new(ErrorKind::Closed))
                    });
                    // Ok when the driver handled it, Err(Closed) after it
                    // stopped; what matters is that the join returns.
                    let _ = block_on(join_task(&task));
                })
            })
            .collect();
        drop(intents);
        driver.join().unwrap();
        for poster in posters {
            poster.join().unwrap();
        }
    });
}

/// Model 3, concurrent dispose runs a cleanup exactly once (D6/D20, Q3.1): three
/// callers drain one effect record at once, as `Disposer::dispose`,
/// `drain_effects` and a mount record's drain can. Exactly one of them
/// claims the cleanups; every one of them gets the one result (the same
/// `Arc`); none waits forever for a result posted between its check and
/// its wait.
///
/// Each caller does what `EffectRecord::drain` does: claim, and if it got
/// the cleanups, run them on a task of their own (`handle.spawn` there, a
/// shuttle thread here) that finishes the record; then join.
#[test]
fn shuttle_concurrent_dispose_cleans_up_exactly_once() {
    // `run_cleanups` takes a runtime handle for async cleanups; the model's
    // cleanup is synchronous, so the runtime never runs anything.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();
    explore(move || {
        let runs = Arc::new(AtomicUsize::new(0));
        let effect = {
            let runs = runs.clone();
            Effect::Disposer(Box::new(move || {
                runs.fetch_add(1, Ordering::SeqCst);
                Err(Error::from(ErrorKind::PluginFailed("cleanup".into())))
            }))
        };
        let record = EffectRecord::from(
            effect,
            "model".into(),
            crate::effect::Origin::Other,
            Weak::new(),
        );
        let callers: Vec<_> = (0..3)
            .map(|_| {
                let (record, handle) = (record.clone(), handle.clone());
                thread::spawn(move || {
                    if let Some(mut cleanups) = record.claim() {
                        let record = record.clone();
                        thread::spawn(move || {
                            let error = block_on(run_cleanups(&mut cleanups, &handle));
                            record.finish(error);
                        });
                    }
                    block_on(record.join())
                })
            })
            .collect();
        let results: Vec<_> = callers
            .into_iter()
            .map(|caller| caller.join().unwrap().expect_err("the cleanup failed"))
            .collect();
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the cleanup ran more than once"
        );
        for result in &results[1..] {
            assert!(
                Arc::ptr_eq(result, &results[0]),
                "callers got different errors"
            );
        }
    });
}

/// Model 4, the subtree-close marker pass (#1025): a closer collects a fixed
/// tree, writes every `lineage_closed` marker, and only then publishes the
/// existing `closing` flags. A reader that sees an ancestor closing while its
/// own marker is still clear would admit a call after the close boundary.
///
/// The model runs the kernel's own [`mark_subtree_lineage`] /
/// [`publish_subtree_closing`] through [`CloseTree`], so changing the
/// two-phase order in the kernel changes this model too. The deliberate
/// mutation for review: call `publish_subtree_closing` before
/// `mark_subtree_lineage` (or publish closing per member inside the marker
/// loop); the readers then report an ancestor-closed/child-live window.
#[test]
fn shuttle_lineage_marker_precedes_closing() {
    explore(|| {
        let (root, nodes) = model_tree();
        let done = Arc::new(SyncBool::new(false));
        let readers: Vec<_> = nodes
            .iter()
            .map(|node| {
                let node = node.clone();
                let done = done.clone();
                thread::spawn(move || {
                    while !done.load(Ordering::SeqCst) {
                        if let Some(why) = close_window(&node) {
                            panic!("{why}");
                        }
                        switch_point();
                    }
                })
            })
            .collect();

        let closer = {
            let root = root.clone();
            let done = done.clone();
            thread::spawn(move || {
                switch_point();
                let subtree = mark_subtree_lineage(&root);
                // The kernel publishes `Shared.closing` between the two
                // passes for a root. A subtree has no shared flag; this
                // switch point stands in for the end of the marker pass.
                switch_point();
                publish_subtree_closing(&subtree);
                done.store(true, Ordering::SeqCst);
            })
        };

        closer.join().unwrap();
        for reader in readers {
            reader.join().unwrap();
        }
        for node in &nodes {
            assert!(
                node.lineage_closed.load(Ordering::SeqCst),
                "a member of the fixed subtree was not marked"
            );
            assert!(
                node.closing.load(Ordering::SeqCst),
                "a member of the fixed subtree was not closed"
            );
        }
    });
}

/// Model 5, a registration and the dispose of its fiber (#622, "登记准入与
/// 卸载"): `Ctx::effect` registers a cleanup through
/// `Ctx::register_effect_named`, which checks the lifecycle under
/// `Shared.admission`, runs the plugin's factory **outside** that lock, and
/// checks again under the transition lock before it commits the record to
/// `FiberInner::effects`. A dispose that unloads the fiber in that window must
/// not lose the record: whichever way the race goes, the cleanup the factory
/// made runs exactly once — through the unload's `drain_effects`, or through the
/// registration's own drain of a record it refuses after the factory ran.
///
/// The model runs `register_effect_named` itself (via `effect_named`) and the
/// kernel's own `EffectRecord::claim`/`finish`/`join`; the dispose side is
/// written out where it does not exist as a callable piece: what
/// `FiberInner::unload` does under `Shared.admission` and the transition lock
/// (state change, which is what refuses a late registration), then what
/// `drain_effects` does with the records it takes. The refused record's drain
/// is a `handle.spawn` in the kernel, so the model runs the current-thread
/// runtime's queue after both threads: that is the task the host's runtime
/// would run. The factory is the window between the two checks, so it counts
/// itself.
///
/// Mutations this model is meant to catch: dropping the second check (the
/// record is committed after `drain_effects` took the list: the cleanup never
/// runs, `runs` stays 0 for one factory), and dropping the refusal's
/// `record.drain` (same count, no cleanup). Both make the `runs == factories`
/// assertion fail; the switch points are the factory's own `switch_point` and
/// `EffectRecord`'s state, so a mutation that only moves `push_effect` out of
/// the transition critical section has no switch point between the check and
/// the push and stays invisible here — that is a lock-order question, not an
/// interleaving one.
#[test]
fn shuttle_effect_registration_and_dispose_clean_up_exactly_once() {
    explore(|| {
        // The kernel drains a refused record on the runtime (`handle.spawn`):
        // a current-thread runtime runs that task when the model pumps its
        // queue, as a host's runtime would on its own.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let handle = runtime.handle().clone();
        let ctx = Ctx::root_with(handle.clone());
        let fiber = ctx.root_view().expect("the root view").inner.clone();

        let factories = Arc::new(AtomicUsize::new(0));
        let runs = Arc::new(AtomicUsize::new(0));

        let registrar = {
            let (ctx, factories, runs) = (ctx.clone(), factories.clone(), runs.clone());
            thread::spawn(move || {
                // The dispose may land before the preflight; then the
                // registration is refused before its factory runs.
                switch_point();
                let registered = ctx.effect_named("model", move || {
                    factories.fetch_add(1, Ordering::SeqCst);
                    // `f` runs outside the admission lock: this is the window
                    // between the factory and the state re-check under the
                    // transition lock (`register_effect_named`).
                    switch_point();
                    Effect::Disposer(Box::new(move || {
                        runs.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }))
                });
                registered.is_ok()
            })
        };

        let dispose = {
            let (ctx, fiber) = (ctx.clone(), fiber.clone());
            thread::spawn(move || {
                switch_point();
                // `FiberInner::unload` with `NextState::Disposed`: the state
                // change under both locks. It is what a registration that has
                // not committed yet sees.
                {
                    let _admission = ctx.shared().admission.lock().unwrap();
                    let mut tr = fiber.transition.lock().unwrap();
                    fiber.set_state(&mut tr, FiberState::Unloading);
                }
                // `drain_effects`: the records this unload takes, last
                // registered first, each drained by exactly one caller
                // (`EffectRecord::drain` without its spawned runner: the
                // cleanup is synchronous, so this thread runs it).
                let taken: Vec<Arc<EffectRecord>> =
                    std::mem::take(&mut *fiber.effects.lock().unwrap());
                for record in taken.into_iter().rev() {
                    if let Some(mut cleanups) = record.claim() {
                        let error = block_on(run_cleanups(&mut cleanups, &handle));
                        record.finish(error);
                    }
                    let _ = block_on(record.join());
                }
            })
        };

        let registered = registrar.join().unwrap();
        dispose.join().unwrap();
        // The refused record's drain is the task `register_effect_named`
        // spawned; run the runtime's queue.
        runtime.block_on(async {
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        });

        let made = factories.load(Ordering::SeqCst);
        let ran = runs.load(Ordering::SeqCst);
        assert_eq!(
            ran, made,
            "a registration made an effect whose cleanup did not run exactly once \
             ({ran} runs for {made} factories; the registration returned {registered})"
        );
        assert!(made <= 1, "the factory ran more than once");
        assert!(
            fiber.effects.lock().unwrap().is_empty(),
            "a record was left in the fiber's effect list"
        );
        assert!(
            fiber.effect_index.lock().unwrap().is_empty(),
            "a record was left in the fiber's effect index"
        );
        assert_eq!(
            *fiber.event_flights.lock().unwrap(),
            0,
            "the registration's event flight was never released"
        );
    });
}

/// The body of the flight model's listener: the model runs the call body
/// itself, so the kernel's erased sync listener is never called.
struct NeverCalled;

impl ErasedSync for NeverCalled {
    fn call(&self, _: &Ctx, _: &Inv<'_>, _: Option<SyncNext<'_>>) -> Result<Option<Value>, Error> {
        unreachable!("the model runs the listener body itself")
    }
}

/// A listener entry owned by the model's fiber: the kernel's own [`Entry`],
/// generation 0 (the root fiber's live generation), so `Entry::live` answers
/// from the owner's markers.
fn model_entry(ctx: &Ctx, fiber: &Arc<FiberInner>) -> Entry {
    Entry {
        id: 0,
        position: 0,
        name: Arc::from("model"),
        prefix: false,
        once: false,
        prepend: false,
        info: Arc::new(ListenerInfo {
            name: "model".to_string(),
            prefix: false,
            plugin: "model".to_string(),
            row: None,
            runtime: None,
            node: None,
            scope: Vec::new(),
            global: true,
        }),
        origin: None,
        body: Body::Sync(Arc::new(NeverCalled)),
        ctx: ctx.clone(),
        owner: Arc::downgrade(fiber),
        generation: 0,
        withdrawn: StdBool::new(false),
        claimed: StdBool::new(false),
        calls: AtomicU64::new(0),
    }
}

/// A listener entry of the model's fiber under `name`, with the `id` and
/// `position` a `Table` gave it (models 10 and 11 insert it as
/// `EventBus::register` does): the kernel's own [`Entry`], generation 0 (the
/// root fiber's live generation), its body never called by the kernel — the
/// model runs the call itself.
fn named_entry(ctx: &Ctx, fiber: &Arc<FiberInner>, id: u64, position: i64, name: &str) -> Entry {
    let mut entry = model_entry(ctx, fiber);
    entry.id = id;
    entry.position = position;
    entry.name = Arc::from(name);
    entry.info = Arc::new(ListenerInfo {
        name: name.to_string(),
        prefix: false,
        plugin: "model".to_string(),
        row: None,
        runtime: None,
        node: None,
        scope: Vec::new(),
        global: true,
    });
    entry
}

/// Model 6, an unload waits for the calls in flight on the fiber (#622,
/// "事件调用的在途计数"): `Entry::start` counts the call on its owner
/// (`CallFlight` → `FiberInner::begin_event`) **before** the final liveness
/// check, and the unload publishes `Unloading` (`set_state`, which also
/// publishes `NOT_LIVE`) under `Shared.admission` before it waits
/// (`wait_events`). So the wait returns only when no call body is running: a
/// call that was admitted is counted and waited for, and a call that arrives
/// after the publication is skipped by `live()` and never runs a body. The
/// count must come back to zero, so the drain does not hang on a missed
/// wakeup.
///
/// The model runs the kernel's `Entry::start`, `CallFlight`, `set_state` and
/// `wait_events` (with no deadline timer: this model installs no clock, so the
/// wait has none, as a plain build's has none when its thread cannot be
/// spawned). What is written out is only the
/// call body itself — a dispatch runs the listener between `Entry::start` and
/// dropping the flight — and the unloader's two steps (the state change, then
/// the wait), which are `FiberInner::unload`'s.
///
/// Mutations this model catches: dropping the final `Entry::live` check in
/// `Entry::start` (a dispatcher that begins a flight after the wait returned
/// runs a body, so `running` is non-zero when `wait_events` returns), and
/// making `wait_events` return without waiting for the count (a body is still
/// running when it returns, or a flight outlives the wait). The reordering
/// that has no switch point — checking `live()` before counting the flight —
/// is invisible here: `begin_event` uses std primitives, so shuttle cannot
/// stop between the two.
#[test]
fn shuttle_unload_waits_for_in_flight_listener_calls() {
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let ctx = Ctx::root_with(runtime.handle().clone());
        let fiber = ctx.root_view().expect("the root view").inner.clone();
        let entry = Arc::new(model_entry(&ctx, &fiber));

        let running = Arc::new(ShuttleUsize::new(0));

        let dispatchers: Vec<_> = (0..2)
            .map(|_| {
                let (entry, running) = (entry.clone(), running.clone());
                thread::spawn(move || {
                    for _ in 0..2 {
                        switch_point();
                        // The call is counted before its liveness check: a
                        // flight the kernel admits is what the unload waits
                        // for, even if the check then skips the call.
                        let Some(flight) = entry.start() else {
                            continue;
                        };
                        // The listener body, while the flight is held.
                        running.fetch_add(1, Ordering::SeqCst);
                        // A body is user code and yields; the wait must not
                        // pass it.
                        switch_point();
                        running.fetch_sub(1, Ordering::SeqCst);
                        drop(flight);
                    }
                })
            })
            .collect();

        let unloader = {
            let (ctx, fiber, running) = (ctx.clone(), fiber.clone(), running.clone());
            thread::spawn(move || {
                switch_point();
                // `FiberInner::unload`: `Unloading` under the admission and
                // transition locks, publishing `NOT_LIVE` before the wait.
                {
                    let _admission = ctx.shared().admission.lock().unwrap();
                    let mut tr = fiber.transition.lock().unwrap();
                    fiber.set_state(&mut tr, FiberState::Unloading);
                }
                block_on(fiber.wait_events());
                let still = running.load(Ordering::SeqCst);
                assert_eq!(
                    still, 0,
                    "the unload's wait returned while {still} listener call(s) were running"
                );
            })
        };

        for dispatcher in dispatchers {
            dispatcher.join().unwrap();
        }
        unloader.join().unwrap();
        assert_eq!(
            *fiber.event_flights.lock().unwrap(),
            0,
            "an event flight was never finished"
        );
        assert_eq!(
            running.load(Ordering::SeqCst),
            0,
            "a listener body was still running at the end"
        );
    });
}

/// The model's fiber: parent link, children, and the two markers the kernel
/// publishes (`crate::sync` makes the atomics switch points under shuttle).
struct ModelFiber {
    parent: Option<Weak<ModelFiber>>,
    children: SyncMutex<Vec<Arc<ModelFiber>>>,
    lineage_closed: SyncBool,
    closing: SyncBool,
}

impl ModelFiber {
    fn new(parent: Option<Weak<ModelFiber>>) -> Self {
        Self {
            parent,
            children: SyncMutex::new(Vec::new()),
            lineage_closed: SyncBool::new(false),
            closing: SyncBool::new(false),
        }
    }
}

impl CloseTree for ModelFiber {
    fn close_children(&self) -> Vec<Arc<Self>> {
        self.children.lock().unwrap().clone()
    }

    fn mark_lineage(&self) {
        self.lineage_closed.store(true, Ordering::SeqCst);
    }

    fn publish_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
}

fn model_tree() -> (Arc<ModelFiber>, Vec<Arc<ModelFiber>>) {
    let root = Arc::new(ModelFiber::new(None));
    let left = Arc::new(ModelFiber::new(Some(Arc::downgrade(&root))));
    let right = Arc::new(ModelFiber::new(Some(Arc::downgrade(&root))));
    let nested = Arc::new(ModelFiber::new(Some(Arc::downgrade(&left))));
    root.children
        .lock()
        .unwrap()
        .extend([left.clone(), right.clone()]);
    left.children.lock().unwrap().push(nested.clone());
    (root.clone(), vec![root, left, right, nested])
}

/// `Some(reason)` when this node could admit a call after a close boundary:
/// its own closing flag or an ancestor's close is visible, and its own marker
/// is still clear when read after that observation. The marker is re-read
/// after the close flags, so a node marked concurrently does not look like a
/// window.
fn close_window(node: &Arc<ModelFiber>) -> Option<String> {
    let own_closing = node.closing.load(Ordering::SeqCst);
    let mut ancestor_closed = false;
    let mut current = node.parent.as_ref().and_then(Weak::upgrade);
    while let Some(parent) = current {
        if parent.closing.load(Ordering::SeqCst) || parent.lineage_closed.load(Ordering::SeqCst) {
            ancestor_closed = true;
            break;
        }
        current = parent.parent.as_ref().and_then(Weak::upgrade);
    }
    if (own_closing || ancestor_closed) && !node.lineage_closed.load(Ordering::SeqCst) {
        return Some(if own_closing {
            "this node is closing while its lineage marker is clear".to_string()
        } else {
            "an ancestor is closed while this node's marker is clear".to_string()
        });
    }
    None
}

/// The model's fiber for the shutdown bookkeeping: the tree links and markers
/// [`CloseTree`] publishes, and what `FiberView::shutdown` and
/// `begin_subtree_shutdown_locked` keep — the task every caller of this
/// fiber's shutdown gets, the task its driver completes, its mailbox, and the
/// cleanups the driver runs.
struct ShutdownFiber {
    children: SyncMutex<Vec<Arc<ShutdownFiber>>>,
    lineage_closed: SyncBool,
    closing: SyncBool,
    /// `FiberInner::shutdown_task`: one task for every caller.
    shutdown_task: SyncMutex<Option<Arc<TransitionTask>>>,
    /// `FiberInner::terminal_task`: the task the driver completes, created
    /// once, so the driver is posted one `Intent::Shutdown` only.
    terminal_task: SyncMutex<Option<Arc<TransitionTask>>>,
    intents: mpsc::UnboundedSender<Intent>,
    cleanups: AtomicUsize,
    posts: AtomicUsize,
}

impl CloseTree for ShutdownFiber {
    fn close_children(&self) -> Vec<Arc<Self>> {
        self.children.lock().unwrap().clone()
    }

    fn mark_lineage(&self) {
        self.lineage_closed.store(true, Ordering::SeqCst);
    }

    fn publish_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
}

/// A node and its driver: the driver handles the one intent a shutdown posts
/// to it (`FiberInner::unload`: one cleanup pass, here) and completes the task
/// that intent carries.
fn shutdown_node(children: &[Arc<ShutdownFiber>]) -> (Arc<ShutdownFiber>, thread::JoinHandle<()>) {
    let (intents, mut mailbox) = mpsc::unbounded_channel::<Intent>();
    let fiber = Arc::new(ShutdownFiber {
        children: SyncMutex::new(children.to_vec()),
        lineage_closed: SyncBool::new(false),
        closing: SyncBool::new(false),
        shutdown_task: SyncMutex::new(None),
        terminal_task: SyncMutex::new(None),
        intents,
        cleanups: AtomicUsize::new(0),
        posts: AtomicUsize::new(0),
    });
    let driver = {
        let fiber = fiber.clone();
        thread::spawn(move || {
            if let Some(intent) = block_on(mailbox.recv()) {
                fiber.cleanups.fetch_add(1, Ordering::SeqCst);
                switch_point();
                complete_intent(&intent, None);
            }
        })
    };
    (fiber, driver)
}

/// `begin_subtree_shutdown_locked`, mirrored: called with the admission lock
/// held, every member of `fiber`'s subtree is shut down once, and every caller
/// of the same fiber gets the same task. The kernel's slot checks, its
/// `Intent::Shutdown` post and the joins are the kernel's own types; what is
/// written out is the `handle.spawn`ed joiner (a shuttle thread here) and the
/// join of the driver's handle, which only collects a panic.
fn begin_shutdown(fiber: &Arc<ShutdownFiber>) -> Arc<TransitionTask> {
    if let Some(task) = fiber.shutdown_task.lock().unwrap().as_ref() {
        return task.clone();
    }
    let public = TransitionTask::new();
    *fiber.shutdown_task.lock().unwrap() = Some(public.clone());

    let child_tasks: Vec<Arc<TransitionTask>> =
        fiber.close_children().iter().map(begin_shutdown).collect();

    let (inner, needs_post) = {
        let mut slot = fiber.terminal_task.lock().unwrap();
        match slot.as_ref() {
            Some(task) => (task.clone(), false),
            None => {
                let task = TransitionTask::new();
                *slot = Some(task.clone());
                (task, true)
            }
        }
    };
    if needs_post {
        fiber.posts.fetch_add(1, Ordering::SeqCst);
        // `FiberInner::post`: a driver that already exited completes the
        // intent's task at once.
        let intent = Intent::Shutdown(inner.clone());
        if fiber.intents.send(intent).is_err() {
            inner.complete(Some(Arc::new(ErrorKind::Closed)));
        }
    }
    let result = public.clone();
    thread::spawn(move || {
        let mut errors = Vec::new();
        if let Err(error) = block_on(join_task(&inner)) {
            errors.push(error);
        }
        // The kernel joins the child tasks after its own task: a subtree
        // shutdown returns once the whole subtree has cleaned up.
        for child in child_tasks {
            if let Err(error) = block_on(join_task(&child)) {
                errors.push(error);
            }
        }
        result.complete(crate::error::aggregate_arcs(errors));
    });
    public
}

/// `FiberView::shutdown`'s synchronous part (#1025, #622): under the admission
/// lock, the marker pass, then the existing `closing` flags, then the task
/// bookkeeping.
fn request_shutdown(fiber: &Arc<ShutdownFiber>, admission: &SyncMutex<()>) -> Arc<TransitionTask> {
    let _admission = admission.lock().unwrap();
    let subtree = mark_subtree_lineage(fiber);
    publish_subtree_closing(&subtree);
    begin_shutdown(fiber)
}

/// Model 7, concurrent parent, child and sibling subtree shutdowns (#622,
/// "根关闭与根 dispose" / "子树关闭"): three callers shut down the root, a child
/// and a sibling at once, and one of them twice. Every caller returns after
/// the whole subtree it named has run its cleanups — each fiber's cleanup
/// exactly once — no caller waits forever (the drivers are posted once, and
/// every task is completed), and two callers of the same fiber get the same
/// task, so they cannot disagree about the result.
///
/// This is the one reference model in this file: the kernel's
/// `begin_subtree_shutdown_locked` needs a real `FiberInner` and spawns its
/// joiner on a tokio runtime, and a real `FiberView::shutdown` cannot run from
/// two shuttle threads — it holds `Shared.admission`, a **std** mutex, while
/// `mark_subtree_lineage` stores a shuttle `lineage_closed` atomic, so the
/// second caller would block in a std mutex while the first is parked in
/// shuttle (a model deadlock, not a kernel one). The marker and closing passes
/// and every task are the kernel's own; the two slots and the post mirror
/// `begin_subtree_shutdown_locked` line for line.
///
/// Mutations this model catches: dropping the `shutdown_task` early return
/// (two callers of one fiber get different tasks: the `Arc::ptr_eq` assertion
/// fails) and completing the public task before the child tasks (a caller
/// returns before a child's cleanup ran: its check right after the join sees a
/// zero). The cleanups are std atomics on purpose: a shuttle load is a switch
/// point, and the check after the join must not be one, or a driver could run
/// inside it and hide an early return. The `terminal_task` slot is not
/// exercised here: concurrent shutdowns alone already serialize on
/// `shutdown_task`, so only a dispose racing a shutdown can post twice, which
/// model 8 (root only) and model 17 (root with the child it mounted) cover.
#[test]
fn shuttle_concurrent_subtree_shutdowns_run_each_cleanup_once() {
    explore(|| {
        let admission = Arc::new(SyncMutex::new(()));
        let (nested, nested_driver) = shutdown_node(&[]);
        let (left, left_driver) = shutdown_node(&[nested.clone()]);
        let (right, right_driver) = shutdown_node(&[]);
        let (root, root_driver) = shutdown_node(&[left.clone(), right.clone()]);

        // Root, child, the same child again, sibling: the parent's subtree
        // shutdown and the child's own one race, and the child's two callers
        // must share one task. Each caller gets the members of the subtree it
        // names up front: its check after the join must not take a lock or
        // store a marker, or shuttle could run a driver inside it.
        let members: Vec<Vec<Arc<ShutdownFiber>>> =
            [root.clone(), left.clone(), left.clone(), right.clone()]
                .iter()
                .map(|fiber| {
                    let mut out = Vec::new();
                    let mut pending = vec![fiber.clone()];
                    while let Some(node) = pending.pop() {
                        pending.extend(node.children.lock().unwrap().iter().cloned());
                        out.push(node);
                    }
                    out
                })
                .collect();
        let callers: Vec<_> = [root.clone(), left.clone(), left.clone(), right.clone()]
            .into_iter()
            .zip(members)
            .enumerate()
            .map(|(index, (fiber, members))| {
                let admission = admission.clone();
                thread::spawn(move || {
                    switch_point();
                    let task = request_shutdown(&fiber, &admission);
                    let result = block_on(join_task(&task));
                    // Right after the join, with no yield in between:
                    // returning means the whole subtree it named has cleaned
                    // up. The counters are std atomics for that reason.
                    let cleaned: Vec<usize> = members
                        .iter()
                        .map(|member| member.cleanups.load(Ordering::SeqCst))
                        .collect();
                    (index, task, result, cleaned)
                })
            })
            .collect();

        let mut results = Vec::new();
        for caller in callers {
            let (index, task, result, cleaned) = caller.join().unwrap();
            assert!(result.is_ok(), "a shutdown returned an error");
            assert_eq!(
                cleaned,
                vec![1; cleaned.len()],
                "a subtree shutdown returned before one of its cleanups ran"
            );
            results.push((index, task));
        }
        assert!(
            Arc::ptr_eq(&results[1].1, &results[2].1),
            "two shutdowns of the same fiber returned different tasks"
        );

        for driver in [root_driver, left_driver, right_driver, nested_driver] {
            driver.join().unwrap();
        }
        for node in [&root, &left, &right, &nested] {
            let cleanups = node.cleanups.load(Ordering::SeqCst);
            let posts = node.posts.load(Ordering::SeqCst);
            assert_eq!(
                cleanups, 1,
                "a fiber's cleanup did not run exactly once ({cleanups} runs)"
            );
            assert_eq!(
                posts, 1,
                "a fiber's driver was posted {posts} intents, not one"
            );
            assert!(
                node.lineage_closed.load(Ordering::SeqCst),
                "a member of a closed subtree was not marked"
            );
            assert!(
                node.closing.load(Ordering::SeqCst),
                "a member of a closed subtree was not closed"
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Model 8: root dispose, root shutdown and restart (#622, "根关闭与根 dispose")

/// The transition bookkeeping of the model's root: the fields of `Trans` the
/// close paths touch.
struct RootTrans {
    state: FiberState,
    generation: u64,
    /// `Trans::terminal_task`: what a dispose registers and the driver
    /// completes; a shutdown also completes it if a dispose came first.
    terminal_task: Option<Arc<TransitionTask>>,
    pending_restarts: usize,
    pre_cancelled: bool,
    error: Option<Arc<ErrorKind>>,
}

/// The root of the reference model: the slots `FiberView::dispose` and
/// `CtxHost::shutdown` share, its driver's mailbox, and the effect records
/// its `unload` drains. One effect is registered at the start, as a host's
/// `ctx.effect` on the root would be.
struct RootFiber {
    handle: tokio::runtime::Handle,
    /// `Shared.admission`. A **std** mutex in the kernel; shuttle's here, so
    /// the other close caller can run between the two calls the kernel makes
    /// as separate steps (see the model's comment).
    admission: SyncMutex<()>,
    /// `Shared.closing`.
    shared_closing: SyncBool,
    /// `Shared.shutdown_task`.
    shared_shutdown_task: SyncMutex<Option<Arc<TransitionTask>>>,
    transition: SyncMutex<RootTrans>,
    /// `FiberInner::closing`.
    closing: SyncBool,
    lineage_closed: SyncBool,
    alive: SyncBool,
    intents: mpsc::UnboundedSender<Intent>,
    effects: SyncMutex<Vec<Arc<EffectRecord>>>,
    /// Std counters: read after the joins, where a shuttle load would itself
    /// be a switch point and could run a driver inside the check.
    cleanups: Arc<AtomicUsize>,
    dispose_posts: AtomicUsize,
    shutdown_posts: AtomicUsize,
    restart_posts: AtomicUsize,
}

impl CloseTree for RootFiber {
    fn close_children(&self) -> Vec<Arc<Self>> {
        // The root's subtree is the root: child subtrees are model 7's.
        Vec::new()
    }

    fn mark_lineage(&self) {
        self.lineage_closed.store(true, Ordering::SeqCst);
    }

    fn publish_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
        // `FiberInner::publish_closing` pre-cancels the generation as well.
        self.transition.lock().unwrap().pre_cancelled = true;
    }
}

fn set_root_state(tr: &mut RootTrans, new: FiberState) {
    // `set_state`: only a change is a state change (the watch publication is
    // not part of this model).
    if tr.state != new {
        tr.state = new;
    }
}

fn stopped_error_root(root: &RootFiber) -> Option<Arc<ErrorKind>> {
    root.transition.lock().unwrap().error.clone().or_else(|| {
        root.shared_closing
            .load(Ordering::SeqCst)
            .then(|| Arc::new(ErrorKind::Closed))
    })
}

/// `FiberInner::post` for the model's root: refuse and complete the intent
/// once the driver stopped, and count the intents the driver will see.
fn post_root(root: &Arc<RootFiber>, intent: Intent) -> bool {
    if !root.alive.load(Ordering::SeqCst) {
        complete_intent(&intent, stopped_error_root(root));
        return false;
    }
    let counter = match &intent {
        Intent::Dispose => Some(&root.dispose_posts),
        Intent::Shutdown(_) => Some(&root.shutdown_posts),
        _ => None,
    };
    match root.intents.send(intent) {
        Ok(()) => {
            if let Some(counter) = counter {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            true
        }
        Err(error) => {
            complete_intent(&error.0, stopped_error_root(root));
            false
        }
    }
}

fn new_root(handle: tokio::runtime::Handle) -> (Arc<RootFiber>, mpsc::UnboundedReceiver<Intent>) {
    let (intents, mailbox) = mpsc::unbounded_channel();
    let cleanups = Arc::new(AtomicUsize::new(0));
    let record = EffectRecord::from(
        Effect::Disposer(Box::new({
            let cleanups = cleanups.clone();
            move || {
                cleanups.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })),
        "model root effect".into(),
        Origin::Other,
        Weak::new(),
    );
    let root = Arc::new(RootFiber {
        handle,
        admission: SyncMutex::new(()),
        shared_closing: SyncBool::new(false),
        shared_shutdown_task: SyncMutex::new(None),
        transition: SyncMutex::new(RootTrans {
            state: FiberState::Active,
            generation: 0,
            terminal_task: None,
            pending_restarts: 0,
            pre_cancelled: false,
            error: None,
        }),
        closing: SyncBool::new(false),
        lineage_closed: SyncBool::new(false),
        alive: SyncBool::new(true),
        intents,
        effects: SyncMutex::new(vec![record]),
        cleanups,
        dispose_posts: AtomicUsize::new(0),
        shutdown_posts: AtomicUsize::new(0),
        restart_posts: AtomicUsize::new(0),
    });
    (root, mailbox)
}

/// `CtxHost::shutdown` for the root, mirrored: under the admission lock, one
/// shared task, the marker pass, `Shared.closing`, the existing `closing`
/// flags, then one `Intent::Shutdown`.
fn root_shutdown(root: &Arc<RootFiber>) -> Arc<TransitionTask> {
    let _admission = root.admission.lock().unwrap();
    let mut slot = root.shared_shutdown_task.lock().unwrap();
    if let Some(task) = slot.as_ref() {
        return task.clone();
    }
    let task = TransitionTask::new();
    *slot = Some(task.clone());
    let subtree = mark_subtree_lineage(root);
    root.shared_closing.store(true, Ordering::SeqCst);
    publish_subtree_closing(&subtree);
    post_root(root, Intent::Shutdown(task.clone()));
    task
}

/// `FiberView::dispose`'s root branch, mirrored: deciding and registering
/// share the admission lock; after `Shared.closing` the dispose joins the
/// shutdown's task instead of registering a second one.
fn root_dispose(root: &Arc<RootFiber>) -> (Arc<TransitionTask>, bool) {
    let _admission = root.admission.lock().unwrap();
    if root.shared_closing.load(Ordering::SeqCst) {
        let task = root
            .shared_shutdown_task
            .lock()
            .unwrap()
            .clone()
            .expect("shutdown stores its task before closing admission");
        return (task, true);
    }
    // A scheduling point inside the admission critical section: the kernel's
    // thread can be preempted here, but every other root-close caller then
    // blocks on the admission lock. A restart does not take it, so it may
    // run.
    switch_point();
    (register_root_dispose(root), false)
}

/// `register_root_dispose`, mirrored: one terminal task, one `Intent::Dispose`.
fn register_root_dispose(root: &Arc<RootFiber>) -> Arc<TransitionTask> {
    let (task, newly_registered) = {
        let mut tr = root.transition.lock().unwrap();
        match &tr.terminal_task {
            Some(task) => (task.clone(), false),
            None => {
                tr.pre_cancelled = true;
                let task = TransitionTask::new();
                tr.terminal_task = Some(task.clone());
                (task, true)
            }
        }
    };
    if newly_registered {
        post_root(root, Intent::Dispose);
        if !root.alive.load(Ordering::SeqCst) {
            task.complete(stopped_error_root(root));
        }
    }
    task
}

/// `FiberView::restart`, mirrored for the root: the closing checks, the
/// pre-cancel, then `post_join` and the join.
fn root_restart(root: &Arc<RootFiber>) -> Result<(), Arc<ErrorKind>> {
    {
        let mut tr = root.transition.lock().unwrap();
        if root.shared_closing.load(Ordering::SeqCst) || root.closing.load(Ordering::SeqCst) {
            return Err(Arc::new(ErrorKind::Closed));
        }
        tr.pending_restarts += 1;
        tr.pre_cancelled = true;
    }
    let task = TransitionTask::new();
    let sent = post_join_to(
        &root.alive,
        &root.intents,
        task.clone(),
        Intent::Restart,
        || stopped_error_root(root),
    );
    if sent {
        root.restart_posts.fetch_add(1, Ordering::SeqCst);
    }
    block_on(join_task(&task))
}

/// What `unload` moves to: `NextState` from the kernel.
enum RootNext {
    Pending,
    Disposed,
}

/// `drain_effects` for the model's one record: take the list, claim each
/// record once, run it, finish it.
fn drain_root_effects(root: &Arc<RootFiber>) {
    let effects: Vec<Arc<EffectRecord>> = std::mem::take(&mut *root.effects.lock().unwrap());
    for record in effects.into_iter().rev() {
        if let Some(mut cleanups) = record.claim() {
            let error = block_on(run_cleanups(&mut cleanups, &root.handle));
            record.finish(error);
        }
        let _ = block_on(record.join());
    }
}

/// `FiberInner::unload` for the root: `Unloading` under the admission and
/// transition locks, the (empty) event wait, the cleanup pass, then the next
/// state.
fn unload_root(root: &Arc<RootFiber>, next: RootNext) {
    {
        let _admission = root.admission.lock().unwrap();
        let mut tr = root.transition.lock().unwrap();
        set_root_state(&mut tr, FiberState::Unloading);
    }
    drain_root_effects(root);
    let mut tr = root.transition.lock().unwrap();
    match next {
        RootNext::Pending => set_root_state(&mut tr, FiberState::Pending),
        RootNext::Disposed => {
            tr.error = None;
            set_root_state(&mut tr, FiberState::Disposed);
        }
    }
}

/// The root's driver, mirroring `drive`'s `Dispose`, `Shutdown` and `Restart`
/// arms. The Shutdown arm is the one that stops the driver: it drains the
/// mailbox (a restart queued behind it gets `Closed`), completes the
/// terminal task and the shutdown task, then returns.
fn drive_root(root: Arc<RootFiber>, mut mailbox: mpsc::UnboundedReceiver<Intent>) {
    while let Some(intent) = block_on(mailbox.recv()) {
        match intent {
            Intent::Dispose => {
                if root.transition.lock().unwrap().state != FiberState::Disposed {
                    unload_root(&root, RootNext::Disposed);
                }
                let (task, error) = {
                    let tr = root.transition.lock().unwrap();
                    (tr.terminal_task.clone(), tr.error.clone())
                };
                if let Some(task) = task {
                    task.complete(error);
                }
            }
            Intent::Shutdown(task) => {
                if root.transition.lock().unwrap().state != FiberState::Disposed {
                    unload_root(&root, RootNext::Disposed);
                }
                root.alive.store(false, Ordering::SeqCst);
                let (terminal, error) = {
                    let tr = root.transition.lock().unwrap();
                    (tr.terminal_task.clone(), tr.error.clone())
                };
                while let Ok(intent) = mailbox.try_recv() {
                    let completion = match &intent {
                        Intent::Settle(_) => None,
                        Intent::Shutdown(_) => error.clone(),
                        _ => Some(Arc::new(ErrorKind::Closed)),
                    };
                    complete_intent(&intent, completion);
                }
                if let Some(terminal) = terminal {
                    terminal.complete(error.clone());
                }
                task.complete(error);
                return;
            }
            Intent::Restart(task) => {
                {
                    let mut tr = root.transition.lock().unwrap();
                    tr.pending_restarts = tr.pending_restarts.saturating_sub(1);
                }
                // The take-up's own linearization point: a restart taken up
                // after the close is refused here; one taken up before it may
                // unload/reactivate, and the queued Shutdown disposes it.
                let closing = root.shared_closing.load(Ordering::SeqCst)
                    || root.closing.load(Ordering::SeqCst);
                if closing {
                    task.complete(Some(Arc::new(ErrorKind::Closed)));
                    continue;
                }
                let state = root.transition.lock().unwrap().state;
                if state == FiberState::Disposed {
                    // The guard above is load-bearing: a restart whose take-up
                    // saw the close must never reactivate the root.
                    assert!(
                        !closing,
                        "a restart after the close committed reactivated the root"
                    );
                    {
                        let mut tr = root.transition.lock().unwrap();
                        tr.error = None;
                        tr.terminal_task = None;
                        set_root_state(&mut tr, FiberState::Active);
                    }
                    let error = root.transition.lock().unwrap().error.clone();
                    task.complete(error);
                    continue;
                }
                if state != FiberState::Pending {
                    unload_root(&root, RootNext::Pending);
                }
                // `refresh_deps` returns at its plugin check for the root:
                // no generation starts, so the restart reports the fiber's
                // error (none here), as `complete_task` does.
                let error = root.transition.lock().unwrap().error.clone();
                task.complete(error);
            }
            _ => {}
        }
    }
}

/// Model 8, root dispose × root shutdown × restart (#622, "根关闭与根
/// dispose"): two shutdown callers, one dispose and one restart race. Every
/// caller must return (no lost wakeup), the two shutdown callers must share
/// one task, the dispose must join either that same task or its own terminal
/// task and must never report `Closed` (it registered before admission
/// closed), the root's one cleanup must run exactly once, and a restart whose
/// take-up saw the close must never reactivate the root (a restart ordered
/// before the close may, and the queued shutdown then disposes it).
///
/// This is a reference model. `CtxHost::shutdown` and the root branch of
/// `FiberView::dispose` take `Shared.admission`, a **std** mutex, and then
/// write the shuttle-visible `lineage_closed` markers; two shuttle threads
/// entering them would deadlock in the std mutex while the first is parked at
/// a marker (the same reason model 7 mirrors the subtree shutdown). The
/// mirror keeps the kernel's line order and runs the kernel's own
/// `mark_subtree_lineage` / `publish_subtree_closing` through `CloseTree`,
/// `TransitionTask`, `post_join_to`, `join_task` and `complete_intent`; the
/// driver mirrors `drive`'s close and restart arms and `drain_effects`. The
/// root has no plugin, so `refresh_deps` returns at its plugin check: the
/// model asserts the close guards, not a load.
///
/// Mutations this model catches: dropping the admission lock from the root
/// dispose path (a dispose that saw `closing == false` registers after the
/// shutdown finished and joins a task completed with `Closed`) and dropping
/// the `shared_shutdown_task` early return (two shutdown callers post two
/// `Intent::Shutdown`s and get different tasks; `shutdown_posts == 1` and
/// `Arc::ptr_eq` fail). Run: both fail at the default 1000 iterations, with
/// `a dispose registered before admission closed got Err(Closed)` and `two
/// shutdown callers got different tasks`. The shutdown path's own admission
/// lock has no separately observable mutation in this root-only model: the
/// slot is stored before `Shared.closing`, so a dispose that sees `closing`
/// still finds it.
#[test]
fn shuttle_root_dispose_and_shutdown_share_one_close() {
    // `run_cleanups` takes a runtime handle; the model's cleanup is
    // synchronous, so the runtime never runs anything.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let handle = runtime.handle().clone();
    explore(move || {
        let (root, mailbox) = new_root(handle.clone());
        let driver = {
            let root = root.clone();
            thread::spawn(move || drive_root(root, mailbox))
        };

        let shutdowns: Vec<_> = (0..2)
            .map(|_| {
                let root = root.clone();
                thread::spawn(move || {
                    switch_point();
                    let task = root_shutdown(&root);
                    let result = block_on(join_task(&task));
                    (task, result)
                })
            })
            .collect();
        let disposer = {
            let root = root.clone();
            thread::spawn(move || {
                switch_point();
                let (task, joined_shutdown) = root_dispose(&root);
                let result = block_on(join_task(&task));
                (task, joined_shutdown, result)
            })
        };
        let restarter = {
            let root = root.clone();
            thread::spawn(move || {
                switch_point();
                root_restart(&root)
            })
        };

        let mut shutdown_tasks: Vec<Arc<TransitionTask>> = Vec::new();
        for (task, result) in shutdowns.into_iter().map(|caller| caller.join().unwrap()) {
            assert!(result.is_ok(), "a shutdown returned {result:?}");
            shutdown_tasks.push(task);
        }
        assert!(
            Arc::ptr_eq(&shutdown_tasks[0], &shutdown_tasks[1]),
            "two shutdown callers got different tasks"
        );

        let (dispose_task, joined_shutdown, dispose_result) = disposer.join().unwrap();
        assert!(
            dispose_result.is_ok(),
            "a dispose registered before admission closed got {dispose_result:?}"
        );
        if joined_shutdown {
            assert!(
                Arc::ptr_eq(&dispose_task, &shutdown_tasks[0]),
                "a dispose that joined the shutdown got another task"
            );
        }
        match restarter.join().unwrap() {
            Ok(()) => {}
            Err(error) => assert!(
                matches!(&*error, ErrorKind::Closed),
                "a restart returned {error:?}"
            ),
        }

        assert_eq!(
            root.shutdown_posts.load(Ordering::SeqCst),
            1,
            "the root's driver was posted more than one shutdown"
        );
        assert!(
            root.dispose_posts.load(Ordering::SeqCst) <= 1,
            "the root's driver was posted more than one dispose"
        );
        assert!(
            root.restart_posts.load(Ordering::SeqCst) <= 1,
            "one restart posted more than one intent"
        );
        assert_eq!(
            root.cleanups.load(Ordering::SeqCst),
            1,
            "the root's cleanup did not run exactly once"
        );
        {
            let tr = root.transition.lock().unwrap();
            assert_eq!(
                tr.state,
                FiberState::Disposed,
                "the root did not end disposed"
            );
            assert_eq!(tr.generation, 0, "a close started a root generation");
        }
        assert!(
            root.closing.load(Ordering::SeqCst) && root.lineage_closed.load(Ordering::SeqCst),
            "the root was not marked and closed"
        );
        driver.join().unwrap();

        // A restart after the close is refused, posts nothing and runs no
        // cleanup.
        let posts = root.restart_posts.load(Ordering::SeqCst);
        let late = root_restart(&root);
        assert!(
            matches!(
                &*late.expect_err("a closed root restarted"),
                ErrorKind::Closed
            ),
            "a restart after the close did not report Closed"
        );
        assert_eq!(root.restart_posts.load(Ordering::SeqCst), posts);
        assert_eq!(root.cleanups.load(Ordering::SeqCst), 1);
        assert_eq!(
            root.transition.lock().unwrap().state,
            FiberState::Disposed,
            "a restart after the close changed the root's state"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 9: dependency eviction (#622, Bug B / #620)

/// `(provider, provider generation, key, scope)`: the dependency identity
/// `last_deps` records and a withdrawal matches exactly (`registry.rs`).
type DepQuad = (PluginId, u64, TypeKey, Option<ScopeId>);

/// The model's provider: one identity whose state is the gate (`resolve_dep`
/// answers only an active provider), and whose next generation provides the
/// same key again.
struct DepProvider {
    id: PluginId,
    state: SyncMutex<FiberState>,
}

impl DepProvider {
    fn state(&self) -> FiberState {
        *self.state.lock().unwrap()
    }

    fn set(&self, state: FiberState) {
        *self.state.lock().unwrap() = state;
    }
}

/// One service binding, mirroring `Binding`: provider, generation and the
/// `removing` flag a withdrawal sets before it looks for consumers.
struct DepBinding {
    provider: Weak<DepProvider>,
    provider_id: PluginId,
    provider_gen: u64,
    removing: SyncBool,
}

/// The model's registry: the binding slot and the injectors of the one key.
struct DepRegistry {
    bindings: SyncMutex<HashMap<(TypeKey, Option<ScopeId>), Arc<DepBinding>>>,
    injectors: SyncMutex<Vec<Weak<Consumer>>>,
}

impl DepRegistry {
    fn new() -> Self {
        Self {
            bindings: SyncMutex::new(HashMap::new()),
            injectors: SyncMutex::new(Vec::new()),
        }
    }

    /// `Registry::insert_binding`: a binding still removing may be replaced;
    /// one that is live is `ServiceExists`.
    fn insert(
        &self,
        key: &TypeKey,
        provider: &Arc<DepProvider>,
        provider_gen: u64,
    ) -> Result<Arc<DepBinding>, Error> {
        let mut bindings = self.bindings.lock().unwrap();
        let entry = (key.clone(), None);
        if let Some(existing) = bindings.get(&entry) {
            if !existing.removing.load(Ordering::SeqCst) {
                return Err(Error::from(ErrorKind::ServiceExists("model key".into())));
            }
        }
        let binding = Arc::new(DepBinding {
            provider: Arc::downgrade(provider),
            provider_id: provider.id,
            provider_gen,
            removing: SyncBool::new(false),
        });
        bindings.insert(entry, binding.clone());
        Ok(binding)
    }

    fn lookup(&self, key: &TypeKey) -> Option<Arc<DepBinding>> {
        self.bindings
            .lock()
            .unwrap()
            .get(&(key.clone(), None))
            .cloned()
    }

    /// `Registry::mark_removing_if`: only while the slot still holds this
    /// binding.
    fn mark_removing_if(&self, key: &TypeKey, expected: &Arc<DepBinding>) {
        let bindings = self.bindings.lock().unwrap();
        if bindings
            .get(&(key.clone(), None))
            .is_some_and(|binding| Arc::ptr_eq(binding, expected))
        {
            expected.removing.store(true, Ordering::SeqCst);
        }
    }

    /// `Registry::finalize_binding_if`: a slot a new provide took over stays.
    fn finalize_if(&self, key: &TypeKey, expected: &Arc<DepBinding>) -> bool {
        let mut bindings = self.bindings.lock().unwrap();
        let entry = (key.clone(), None);
        let still_old = bindings
            .get(&entry)
            .is_some_and(|binding| Arc::ptr_eq(binding, expected));
        if still_old {
            bindings.remove(&entry);
        }
        still_old
    }

    /// `Registry::consumers_of`: the injectors whose `last_deps` contains
    /// exactly this quad.
    fn consumers_of(&self, key: &TypeKey, quad: DepQuad) -> Vec<Arc<Consumer>> {
        let index = self.injectors.lock().unwrap();
        index
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|consumer| consumer.inner.declared.contains(key))
            .filter(|consumer| {
                consumer
                    .inner
                    .last_deps
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|deps| deps.contains(&quad))
            })
            .collect()
    }

    fn register_inject(&self, key: &TypeKey, consumer: &Arc<Consumer>) {
        assert!(consumer.inner.declared.contains(key), "an undeclared key");
        self.injectors
            .lock()
            .unwrap()
            .push(Arc::downgrade(consumer));
    }

    /// `Registry::notify_key_changed`: post `RefreshDeps` to every injector
    /// of this key.
    fn notify(&self, key: &TypeKey) {
        let fibers: Vec<Arc<Consumer>> = {
            let index = self.injectors.lock().unwrap();
            index
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|consumer| consumer.inner.declared.contains(key))
                .collect()
        };
        for fiber in fibers {
            post_consumer(&fiber, Intent::RefreshDeps);
        }
    }

    /// `Registry::resolve_dep`: a live binding of an active provider.
    fn resolve_dep(&self, key: &TypeKey) -> Option<(PluginId, u64)> {
        let binding = self.lookup(key)?;
        if binding.removing.load(Ordering::SeqCst) {
            return None;
        }
        let provider = binding.provider.upgrade()?;
        if provider.state() != FiberState::Active {
            return None;
        }
        Some((binding.provider_id, binding.provider_gen))
    }
}

/// The model consumer's transition bookkeeping (`Trans`).
struct ConsumerTrans {
    state: FiberState,
    generation: u64,
    terminal_task: Option<Arc<TransitionTask>>,
    pending_restarts: usize,
    pre_cancelled: bool,
    error: Option<Arc<ErrorKind>>,
}

/// Everything the model's driver needs, without the intent sender, so the
/// driver can outlive the `Consumer` handles and stop when they are dropped.
struct ConsumerInner {
    transition: SyncMutex<ConsumerTrans>,
    /// `FiberInner::last_deps`.
    last_deps: SyncMutex<Option<HashSet<DepQuad>>>,
    /// `FiberInner::effects`: what this generation's `apply` registered.
    effects: SyncMutex<Vec<Arc<EffectRecord>>>,
    alive: SyncBool,
    closing: SyncBool,
    applies: Arc<AtomicUsize>,
    cleanups: Arc<AtomicUsize>,
    handle: tokio::runtime::Handle,
    declared: Vec<TypeKey>,
}

impl ConsumerInner {
    fn state(&self) -> FiberState {
        self.transition.lock().unwrap().state
    }
}

/// A consumer handle: the inner state plus the driver's mailbox sender, so
/// dropping every handle (after the driver is done) closes the mailbox.
struct Consumer {
    inner: Arc<ConsumerInner>,
    intents: mpsc::UnboundedSender<Intent>,
}

/// The registry, the admission lock `load`/`unload` take, the one key and
/// the runtime handle the model's cleanups would use.
struct DepWorld {
    key: TypeKey,
    registry: DepRegistry,
    admission: SyncMutex<()>,
    handle: tokio::runtime::Handle,
}

fn set_consumer_state(tr: &mut ConsumerTrans, new: FiberState) {
    if tr.state != new {
        tr.state = new;
    }
}

fn consumer_stopped_error(inner: &ConsumerInner) -> Option<Arc<ErrorKind>> {
    inner.transition.lock().unwrap().error.clone().or_else(|| {
        inner
            .closing
            .load(Ordering::SeqCst)
            .then(|| Arc::new(ErrorKind::Closed))
    })
}

/// `FiberInner::restart_stopped_error`, mirrored for the model's consumer:
/// `Closed` while it closes, `InactiveEffect` once its dispose is registered
/// (the dispose's clean result is not a restart's), else its stopped error.
fn consumer_restart_stopped_error(inner: &ConsumerInner) -> Option<Arc<ErrorKind>> {
    if inner.closing.load(Ordering::SeqCst) {
        return Some(Arc::new(ErrorKind::Closed));
    }
    if inner.transition.lock().unwrap().terminal_task.is_some() {
        return Some(Arc::new(ErrorKind::InactiveEffect));
    }
    consumer_stopped_error(inner)
}

/// `FiberInner::post` for the model's consumer.
fn post_consumer(consumer: &Arc<Consumer>, intent: Intent) -> bool {
    let inner = &consumer.inner;
    if !inner.alive.load(Ordering::SeqCst) {
        complete_intent(&intent, consumer_stopped_error(inner));
        return false;
    }
    match consumer.intents.send(intent) {
        Ok(()) => true,
        Err(error) => {
            complete_intent(&error.0, consumer_stopped_error(inner));
            false
        }
    }
}

fn new_consumer(world: &DepWorld) -> (Arc<Consumer>, mpsc::UnboundedReceiver<Intent>) {
    let (intents, mailbox) = mpsc::unbounded_channel();
    let inner = Arc::new(ConsumerInner {
        transition: SyncMutex::new(ConsumerTrans {
            state: FiberState::Pending,
            generation: 0,
            terminal_task: None,
            pending_restarts: 0,
            pre_cancelled: false,
            error: None,
        }),
        last_deps: SyncMutex::new(None),
        effects: SyncMutex::new(Vec::new()),
        alive: SyncBool::new(true),
        closing: SyncBool::new(false),
        applies: Arc::new(AtomicUsize::new(0)),
        cleanups: Arc::new(AtomicUsize::new(0)),
        handle: world.handle.clone(),
        declared: vec![world.key.clone()],
    });
    (Arc::new(Consumer { inner, intents }), mailbox)
}

/// `FiberInner::resolve_deps`: the declared keys whose gate is satisfied,
/// and those that are not. The model has one key, one scope and no instance
/// checks; every key it declares is the same one.
fn resolve_deps(world: &DepWorld, inner: &ConsumerInner) -> (HashSet<DepQuad>, Vec<TypeKey>) {
    let mut satisfied = HashSet::new();
    let mut missing = Vec::new();
    for key in &inner.declared {
        match world.registry.resolve_dep(key) {
            Some((provider, generation)) => {
                satisfied.insert((provider, generation, key.clone(), None));
            }
            None => missing.push(key.clone()),
        }
    }
    (satisfied, missing)
}

/// What `unload` moves to.
enum ConsumerNext {
    Pending,
    Disposed,
}

/// `drain_effects` for the model: take the list, drain each record (claim →
/// run → finish), clear `last_deps`, and return the cleanup errors.
fn drain_consumer_effects(inner: &Arc<ConsumerInner>) -> Vec<Arc<ErrorKind>> {
    let effects: Vec<Arc<EffectRecord>> = std::mem::take(&mut *inner.effects.lock().unwrap());
    let mut errors = Vec::new();
    for record in effects.into_iter().rev() {
        if let Some(mut cleanups) = record.claim() {
            let error = block_on(run_cleanups(&mut cleanups, &inner.handle));
            record.finish(error);
        }
        if let Err(error) = block_on(record.join()) {
            errors.push(error);
        }
    }
    *inner.last_deps.lock().unwrap() = None;
    errors
}

/// `FiberInner::unload`: `Unloading` under the admission and transition
/// locks, cancel (unobservable here), the event wait (no calls are counted),
/// the cleanup pass, then `Pending` or `Disposed`.
fn unload(world: &DepWorld, inner: &Arc<ConsumerInner>, next: ConsumerNext) {
    {
        let _admission = world.admission.lock().unwrap();
        let mut tr = inner.transition.lock().unwrap();
        set_consumer_state(&mut tr, FiberState::Unloading);
    }
    let errors = drain_consumer_effects(inner);
    let mut tr = inner.transition.lock().unwrap();
    match next {
        ConsumerNext::Pending => set_consumer_state(&mut tr, FiberState::Pending),
        ConsumerNext::Disposed => {
            tr.error = crate::error::aggregate_arcs(errors);
            set_consumer_state(&mut tr, FiberState::Disposed);
        }
    }
}

/// `FiberInner::load`, mirrored where the two gate checks and the
/// `last_deps` write are: the state change and the new generation under the
/// admission and transition locks, then `last_deps` **before** the second
/// resolve, then `apply` (the model runs it synchronously, with a scheduling
/// point where user code could await), then the fresh-deps adoption and
/// `Active`. The first resolve uses the dependencies passed in (what
/// `refresh_deps` resolved); the second reads the registry again.
fn load(world: &DepWorld, inner: &Arc<ConsumerInner>, deps: HashSet<DepQuad>) {
    let _admission = world.admission.lock().unwrap();
    if inner.closing.load(Ordering::SeqCst) {
        return;
    }
    let generation = {
        let mut tr = inner.transition.lock().unwrap();
        if tr.terminal_task.is_some() {
            return;
        }
        tr.pre_cancelled = tr.pending_restarts > 0;
        tr.generation += 1;
        tr.error = None;
        set_consumer_state(&mut tr, FiberState::Loading);
        tr.generation
    };
    drop(_admission);
    // `#620`'s protocol: the write and the resolve are ordered so that a
    // withdrawal (`mark_removing` then `consumers_of`) and this load see each
    // other — either it finds this fiber in `last_deps`, or the second
    // resolve below sees the removal.
    *inner.last_deps.lock().unwrap() = Some(deps.clone());
    let (now, missing) = resolve_deps(world, inner);
    if !missing.is_empty() || now != deps {
        unload(world, inner, ConsumerNext::Pending);
        return;
    }
    inner.applies.fetch_add(1, Ordering::SeqCst);
    let record = EffectRecord::from(
        Effect::Disposer(Box::new({
            let cleanups = inner.cleanups.clone();
            move || {
                cleanups.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })),
        format!("model apply generation {generation}"),
        Origin::Apply,
        Weak::new(),
    );
    inner.effects.lock().unwrap().push(record);
    // `apply` is user code and may await here.
    switch_point();
    if inner.closing.load(Ordering::SeqCst) {
        unload(world, inner, ConsumerNext::Disposed);
        return;
    }
    let (fresh, missing) = resolve_deps(world, inner);
    if missing.is_empty() {
        *inner.last_deps.lock().unwrap() = Some(fresh);
    }
    {
        let mut tr = inner.transition.lock().unwrap();
        tr.error = None;
        set_consumer_state(&mut tr, FiberState::Active);
    }
}

/// `FiberInner::refresh_deps`: the gate, the equal-set merge, and the unload
/// back to `Pending` before a new load.
fn refresh_deps(world: &DepWorld, inner: &Arc<ConsumerInner>) {
    if inner.closing.load(Ordering::SeqCst) {
        return;
    }
    let state = inner.transition.lock().unwrap().state;
    if state == FiberState::Disposed {
        return;
    }
    if inner.transition.lock().unwrap().terminal_task.is_some() {
        unload(world, inner, ConsumerNext::Disposed);
        return;
    }
    let (satisfied, missing) = resolve_deps(world, inner);
    let state = inner.transition.lock().unwrap().state;
    let loaded = !matches!(state, FiberState::Pending | FiberState::Disposed);
    if !missing.is_empty() {
        if matches!(state, FiberState::Active | FiberState::Loading) {
            unload(world, inner, ConsumerNext::Pending);
        }
        return;
    }
    let unchanged = {
        let last = inner.last_deps.lock().unwrap();
        last.as_ref() == Some(&satisfied)
    };
    if unchanged {
        return;
    }
    if loaded {
        unload(world, inner, ConsumerNext::Pending);
    }
    load(world, inner, satisfied);
}

fn complete_consumer_task(inner: &ConsumerInner, task: Arc<TransitionTask>) {
    let error = inner.transition.lock().unwrap().error.clone();
    task.complete(error);
}

/// `FiberView::restart` for the model's consumer: the closed check, the
/// terminal check (a non-root fiber needs a live driver and no registered
/// dispose), the pre-cancel and the count under the transition lock, then
/// `post_join`. The model has no root close, so `closing` stands for both
/// flags.
fn consumer_restart(consumer: &Arc<Consumer>) -> Restart {
    let inner = &consumer.inner;
    {
        let mut tr = inner.transition.lock().unwrap();
        if inner.closing.load(Ordering::SeqCst) {
            return Restart::Refused(Arc::new(ErrorKind::Closed));
        }
        if tr.terminal_task.is_some() || !inner.alive.load(Ordering::SeqCst) {
            return Restart::Refused(Arc::new(ErrorKind::InactiveEffect));
        }
        tr.pending_restarts += 1;
        tr.pre_cancelled = true;
    }
    let task = TransitionTask::new();
    let sent = post_join_to(
        &inner.alive,
        &consumer.intents,
        task.clone(),
        Intent::Restart,
        || consumer_restart_stopped_error(inner),
    );
    Restart::Posted(task, sent)
}

/// `FiberView::register_dispose` for the model's consumer: the check and the
/// registration share the transition lock, and the pre-cancel is the #619
/// fix. The model's consumer has no generation token to cancel.
fn consumer_dispose(consumer: &Arc<Consumer>) -> Arc<TransitionTask> {
    let inner = &consumer.inner;
    let (task, newly_registered) = {
        let mut tr = inner.transition.lock().unwrap();
        match &tr.terminal_task {
            Some(task) => (task.clone(), false),
            None => {
                tr.pre_cancelled = true;
                let task = TransitionTask::new();
                tr.terminal_task = Some(task.clone());
                (task, true)
            }
        }
    };
    if newly_registered {
        post_consumer(consumer, Intent::Dispose);
        if !inner.alive.load(Ordering::SeqCst) {
            // The driver may have handled the dispose and published its
            // result already (#1194): only a still-running task is completed.
            task.complete_if_running(consumer_stopped_error(inner));
        }
    }
    task
}

/// The consumer's driver: one intent at a time, so a load and an unload
/// never overlap and the notifications queue in the mailbox order.
fn drive_consumer(
    world: Arc<DepWorld>,
    inner: Arc<ConsumerInner>,
    mut mailbox: mpsc::UnboundedReceiver<Intent>,
) {
    while let Some(intent) = block_on(mailbox.recv()) {
        match intent {
            Intent::RefreshDeps => refresh_deps(&world, &inner),
            Intent::RefreshDepsJoin(task) => {
                refresh_deps(&world, &inner);
                complete_consumer_task(&inner, task);
            }
            Intent::Settle(task) => {
                let error = {
                    let tr = inner.transition.lock().unwrap();
                    (tr.state == FiberState::Failed)
                        .then(|| tr.error.clone())
                        .flatten()
                };
                task.complete(error);
            }
            Intent::Restart(task) => {
                // `drive`'s `Intent::Restart` arm for a non-root fiber.
                {
                    let mut tr = inner.transition.lock().unwrap();
                    tr.pending_restarts = tr.pending_restarts.saturating_sub(1);
                }
                if inner.closing.load(Ordering::SeqCst) {
                    task.complete(Some(Arc::new(ErrorKind::Closed)));
                    continue;
                }
                if inner.transition.lock().unwrap().terminal_task.is_some() {
                    // A dispose registered first wins: this restart does not
                    // happen and reports what `restart()` reports behind one.
                    if inner.state() != FiberState::Disposed {
                        unload(&world, &inner, ConsumerNext::Disposed);
                    }
                    task.complete(Some(Arc::new(ErrorKind::InactiveEffect)));
                    continue;
                }
                let state = inner.state();
                if state == FiberState::Disposed {
                    // A non-root fiber is Disposed only with a dispose
                    // registered, which the check above refused.
                    task.complete(inner.transition.lock().unwrap().error.clone());
                    continue;
                }
                if !matches!(state, FiberState::Pending) {
                    unload(&world, &inner, ConsumerNext::Pending);
                }
                let before = inner.transition.lock().unwrap().generation;
                refresh_deps(&world, &inner);
                let started = inner.transition.lock().unwrap().generation != before;
                if !started && inner.transition.lock().unwrap().terminal_task.is_some() {
                    // Registered while this restart was unloading: no new
                    // generation started (`load` checks it).
                    task.complete(Some(Arc::new(ErrorKind::InactiveEffect)));
                    continue;
                }
                if !started {
                    // Waiting for its dependencies: no generation ran, so none
                    // failed.
                    task.complete(None);
                    continue;
                }
                complete_consumer_task(&inner, task);
            }
            Intent::Dispose => {
                // The non-root `Dispose` arm: unload to `Disposed`, complete
                // the terminal task with that result, stop the driver, drain
                // what is queued (a later `Settle` reports no failure; every
                // other join gets the terminal error or its own refusal).
                if inner.state() != FiberState::Disposed {
                    unload(&world, &inner, ConsumerNext::Disposed);
                }
                let (terminal, err) = {
                    let tr = inner.transition.lock().unwrap();
                    (tr.terminal_task.clone(), tr.error.clone())
                };
                if let Some(task) = terminal {
                    task.complete(err);
                }
                inner.alive.store(false, Ordering::SeqCst);
                let error = inner.transition.lock().unwrap().error.clone();
                while let Ok(intent) = mailbox.try_recv() {
                    let completion = match &intent {
                        Intent::Settle(_) => None,
                        // A restart drained behind the terminal dispose never
                        // happened: report that, not the dispose's result.
                        Intent::Restart(_) => consumer_restart_stopped_error(&inner),
                        _ => error.clone(),
                    };
                    complete_intent(&intent, completion);
                }
                return;
            }
            _ => {}
        }
    }
}

fn pre_cancel(inner: &ConsumerInner) {
    inner.transition.lock().unwrap().pre_cancelled = true;
}

/// `evict_and_finalize`, mirrored: publish the provider inactive (its
/// `unload` does that before the provide effect runs), mark the binding
/// removing, find the consumers bound to exactly this quad, pre-cancel them,
/// have each re-check (`RefreshDepsJoin`) and join them, notify the rest,
/// then remove the binding only if the slot still holds it. The model has
/// no `provided` table and no scopes (the quad's scope is always `None`).
///
/// `in_window` runs after the removal is marked and before the consumers are
/// read: the one window a restart can land in and still see this binding as
/// the one being removed (model 19). Model 9 passes an empty closure.
fn withdraw(
    world: &Arc<DepWorld>,
    provider: &Arc<DepProvider>,
    binding: &Arc<DepBinding>,
    removal: &Notify,
    in_window: &dyn Fn(),
) {
    provider.set(FiberState::Unloading);
    world.registry.mark_removing_if(&world.key, binding);
    removal.notify_one();
    in_window();
    let old_quad = (provider.id, binding.provider_gen, world.key.clone(), None);
    let consumers = world.registry.consumers_of(&world.key, old_quad);
    let mut tasks = Vec::new();
    for consumer in &consumers {
        pre_cancel(&consumer.inner);
        let task = TransitionTask::new();
        // `post_join` returns false once the driver stopped; the task is
        // then complete already, and joining it still returns.
        post_join_to(
            &consumer.inner.alive,
            &consumer.intents,
            task.clone(),
            Intent::RefreshDepsJoin,
            || consumer_stopped_error(&consumer.inner),
        );
        tasks.push(task);
    }
    // A scheduling point between finding the consumers and notifying the
    // rest: the kernel runs these as separate steps and can be preempted.
    switch_point();
    world.registry.notify(&world.key);
    for task in tasks {
        let _ = block_on(join_task(&task));
    }
    world.registry.finalize_if(&world.key, binding);
}

/// The provider's next generation provides the same key again. Like `load`'s
/// provide path, the binding appears while the provider is still `Loading`,
/// and the notify follows `Active`: consumers resolve only an active
/// provider.
fn provide(
    world: &Arc<DepWorld>,
    provider: &Arc<DepProvider>,
    provider_gen: u64,
) -> Arc<DepBinding> {
    provider.set(FiberState::Loading);
    let binding = world
        .registry
        .insert(&world.key, provider, provider_gen)
        .expect("the next generation provides the same key");
    provider.set(FiberState::Active);
    world.registry.notify(&world.key);
    binding
}

/// Model 9, dependency eviction (#622, Bug B / #620): a provider leaves
/// while its consumer is between `Pending` and `Loading`, and comes back
/// (replacing the removing binding) while the consumer is still settling.
/// The consumer must not still be running on the withdrawn generation when
/// the withdrawal returns, the withdrawal must not remove the replacement,
/// the consumer must reload on the returned generation, and each applied
/// generation's cleanup must run exactly once.
///
/// This is a reference model: the real gate window runs no plugin code and
/// touches std locks only (`FiberInner::load`'s `last_deps` write and
/// `resolve_deps`, `Registry`'s two locks), so shuttle has no switch point
/// inside it. The model runs `TransitionTask`, `post_join_to`, `join_task`,
/// `EffectRecord` and `complete_intent` from the kernel; `load`,
/// `refresh_deps`, `unload`, the registry methods and `evict_and_finalize`
/// are mirrored line for line with shuttle locks, which is what lets the
/// scheduler stop in that window. One key, one scope and one provider
/// generation per provide; the consumer has no dispose, events or child
/// fibers.
///
/// Mutations this model catches: dropping the second gate check in `load`
/// (a consumer that passed the gate with the old quad writes `last_deps`
/// after the withdrawal read it and still reaches `Active`; the check after
/// `withdraw` sees it running on the withdrawn generation), dropping the
/// consumer join loop in `withdraw` (same check, now with the consumer
/// legitimately found), and dropping the `removing` or provider-state check
/// in `resolve_dep` (the re-check stays `Active`, so the withdrawal returns
/// on a dead dependency). Run: removing the second gate check fails at the
/// default 1000 iterations with `the withdrawal returned while the consumer
/// still ran on the withdrawn generation`.
#[test]
fn shuttle_dependency_withdrawal_stops_the_loading_consumer() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let handle = runtime.handle().clone();
    explore(move || {
        let key = TypeKey::of::<u32>();
        let world = Arc::new(DepWorld {
            key: key.clone(),
            registry: DepRegistry::new(),
            admission: SyncMutex::new(()),
            handle: handle.clone(),
        });
        let provider = Arc::new(DepProvider {
            id: PluginId(7),
            state: SyncMutex::new(FiberState::Active),
        });
        // The provider's first generation provides the key.
        provider.set(FiberState::Loading);
        let first = world
            .registry
            .insert(&key, &provider, 1)
            .expect("the first provide");
        provider.set(FiberState::Active);

        let (consumer, mailbox) = new_consumer(&world);
        world.registry.register_inject(&key, &consumer);
        let driver = {
            let (world, inner) = (world.clone(), consumer.inner.clone());
            thread::spawn(move || drive_consumer(world, inner, mailbox))
        };
        // The mount posts the first refresh.
        post_consumer(&consumer, Intent::RefreshDeps);

        let removal = Arc::new(Notify::new());
        let leaver = {
            let (world, provider, first, consumer, removal) = (
                world.clone(),
                provider.clone(),
                first.clone(),
                consumer.clone(),
                removal.clone(),
            );
            thread::spawn(move || {
                switch_point();
                withdraw(&world, &provider, &first, &removal, &|| {});
                let state = consumer.inner.state();
                let stale = {
                    let last = consumer.inner.last_deps.lock().unwrap();
                    last.as_ref().is_some_and(|deps| {
                        deps.contains(&(provider.id, 1, world.key.clone(), None))
                    })
                };
                assert!(
                    !(state == FiberState::Active && stale),
                    "the withdrawal returned while the consumer still ran on the \
                     withdrawn generation"
                );
            })
        };
        let returner = {
            let (world, provider, removal) = (world.clone(), provider.clone(), removal.clone());
            thread::spawn(move || {
                // `notify_one`'s permit is kept when it fires before the wait,
                // so the return never starts before the removal is marked.
                block_on(removal.notified());
                provide(&world, &provider, 2)
            })
        };

        leaver.join().unwrap();
        let returned = returner.join().unwrap();

        // Settle after every notification the two threads posted: the
        // consumer handles what is queued before the barrier.
        let settle = TransitionTask::new();
        let sent = post_join_to(
            &consumer.inner.alive,
            &consumer.intents,
            settle.clone(),
            Intent::Settle,
            || consumer_stopped_error(&consumer.inner),
        );
        assert!(sent, "the consumer's driver was not alive");
        assert!(block_on(join_task(&settle)).is_ok());

        let inner = consumer.inner.clone();
        let applies = inner.applies.load(Ordering::SeqCst);
        let cleanups = inner.cleanups.load(Ordering::SeqCst);
        assert!(applies >= 1, "the consumer never loaded");
        assert_eq!(
            inner.state(),
            FiberState::Active,
            "the consumer did not reload on the returned generation"
        );
        // The generation that is still Active has not been unloaded: its
        // record is still in the list and its cleanup is not due. Every
        // earlier generation's cleanup ran exactly once.
        assert_eq!(
            cleanups,
            applies - 1,
            "an unloaded generation's cleanup did not run exactly once ({cleanups} \
             cleanups for {applies} applies, one still active)"
        );
        assert_eq!(
            inner.effects.lock().unwrap().len(),
            1,
            "the active generation's record is not the only one left"
        );
        let expected: HashSet<DepQuad> =
            std::iter::once((provider.id, 2, key.clone(), None)).collect();
        assert_eq!(
            inner.last_deps.lock().unwrap().as_ref(),
            Some(&expected),
            "the consumer is not bound to the returned generation"
        );
        let now = world.registry.lookup(&key).expect("the returned binding");
        assert!(
            Arc::ptr_eq(&now, &returned),
            "the withdrawal removed or replaced the new binding"
        );
        assert_eq!(now.provider_gen, 2);
        assert!(!now.removing.load(Ordering::SeqCst));

        // Dispose the loaded generation too: its cleanup must run exactly
        // once, and the record list must end empty. The driver handles
        // `Dispose` and exits (the non-root `drive` arm), so the settle that
        // follows may complete during the drain.
        post_consumer(&consumer, Intent::Dispose);
        let settle = TransitionTask::new();
        let _ = post_join_to(
            &inner.alive,
            &consumer.intents,
            settle.clone(),
            Intent::Settle,
            || consumer_stopped_error(&inner),
        );
        assert!(block_on(join_task(&settle)).is_ok());
        drop(consumer);
        driver.join().unwrap();
        assert_eq!(
            inner.state(),
            FiberState::Disposed,
            "the dispose did not end"
        );
        assert_eq!(
            inner.cleanups.load(Ordering::SeqCst),
            applies,
            "the active generation's cleanup did not run exactly once"
        );
        assert!(
            inner.effects.lock().unwrap().is_empty(),
            "a generation's effect record was left behind"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 10: the emit chain (#622, `core/D31a`, `core/D31c`, `events/emit b`)

/// The emit half of `bus.rs::BusState`: the table the dispatches select from,
/// the tail of each name's chain, and the accepted emits that have not
/// finished. `starting` is the model's own guard: the chain lets one emit of
/// a name run its start loop at a time, which this catches if the tail wait is
/// dropped.
#[derive(Default)]
struct EmitBus {
    table: Table,
    /// `state.tails`: which emit of a name is last, and its generation.
    tails: HashMap<Arc<str>, EmitTail>,
    /// `state.backlog`: the accepted emits of a name whose calls are not all
    /// finished.
    backlog: HashMap<Arc<str>, HashSet<u64>>,
    next_emit: u64,
    starting: Option<u64>,
}

/// `bus.rs::Tail`: the tail the next emit of the name awaits, as a task the
/// model's dispatch completes instead of a `JoinHandle`.
struct EmitTail {
    generation: u64,
    task: Arc<TransitionTask>,
}

/// A call one dispatch started: the emit's sequence, the emitter, and that
/// emitter's own index of the emit.
type StartedCall = (u64, u64, u64);

/// The emitter id the listener's own emit is recorded under: the two
/// emitters of the model are 0 and 1.
const EMIT_FROM_LISTENER: u64 = 2;

struct EmitModel {
    name: Arc<str>,
    bus: SyncMutex<EmitBus>,
    /// The calls each listener got, in start order.
    calls: SyncMutex<HashMap<u64, Vec<StartedCall>>>,
    /// The sequences whose start loop has finished.
    finished: SyncMutex<HashSet<u64>>,
    /// The listener that emits the name again from inside its body.
    re_emitter: AtomicU64,
    re_emitted: SyncMutex<bool>,
}

impl EmitModel {
    /// `EventBus::register` under its state lock: id, position, entry,
    /// insertion. No effect record: nothing in this model withdraws a
    /// listener.
    fn register(&self, ctx: &Ctx, fiber: &Arc<FiberInner>) -> Arc<Entry> {
        let mut bus = self.bus.lock().unwrap();
        let id = bus.table.next_id();
        let position = bus.table.position(false);
        let entry = Arc::new(named_entry(ctx, fiber, id, position, &self.name));
        bus.table.insert(entry.clone());
        entry
    }

    /// `EventBus::emit_erased`'s bookkeeping: select the listeners under the
    /// state lock, append to the name's backlog, replace the tail, and start
    /// the dispatch, which first waits for the tail it replaced. Returns the
    /// emit's task, what the emitter's receiver waits on.
    fn emit(self: &Arc<Self>, emitter: u64, index: u64) -> Arc<TransitionTask> {
        let (selected, seq, prev, generation, task) = {
            let mut bus = self.bus.lock().unwrap();
            // The list the emits of the name share, plus what was appended
            // since it was built (#977, #980); `None` with no live listener.
            let selected = bus.table.shared(&self.name);
            bus.next_emit += 1;
            let seq = bus.next_emit;
            bus.backlog
                .entry(self.name.clone())
                .or_default()
                .insert(seq);
            let generation = bus
                .tails
                .get(&self.name)
                .map_or(0, |tail| tail.generation + 1);
            let prev = bus.tails.get(&self.name).map(|tail| tail.task.clone());
            let task = TransitionTask::new();
            bus.tails.insert(
                self.name.clone(),
                EmitTail {
                    generation,
                    task: task.clone(),
                },
            );
            (selected, seq, prev, generation, task)
        };
        let this = self.clone();
        // The kernel spawns the task while it holds the state, so the task's
        // own steps start after the lock is released. The model starts its
        // thread here, after the lock, and its first step is the tail wait.
        let dispatch = task.clone();
        thread::spawn(move || {
            this.dispatch(emitter, index, seq, generation, prev, selected, dispatch)
        });
        task
    }

    /// One emit's task: the tail wait, then the start loop over the entries
    /// the dispatch selected. A synchronous listener runs inline, as the
    /// kernel's emit runs it (`run::call_sync_listener`).
    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        self: Arc<Self>,
        emitter: u64,
        index: u64,
        seq: u64,
        generation: u64,
        prev: Option<Arc<TransitionTask>>,
        selected: Option<Selected>,
        task: Arc<TransitionTask>,
    ) {
        // `prev.await`: every call of the name's previous emit started before
        // any of this one does.
        if let Some(prev) = prev {
            let _ = block_on(join_task(&prev));
        }
        // `Selected::to_vec`: the list as it is when the dispatch starts, so an
        // entry appended after the base list was built is called (#980), and
        // one withdrawn in between is skipped by `Entry::start`.
        let entries: Vec<Arc<Entry>> = selected
            .map(|selected| selected.to_vec())
            .unwrap_or_default();
        {
            let finished = self.finished.lock().unwrap();
            assert!(
                (1..seq).all(|earlier| finished.contains(&earlier)),
                "emit {seq} of {} started its calls before an earlier emit finished",
                self.name
            );
        }
        {
            let mut bus = self.bus.lock().unwrap();
            assert!(
                bus.starting.is_none(),
                "two emits of {} were in their start loop at once",
                self.name
            );
            bus.starting = Some(seq);
        }
        let mut called = Vec::new();
        for entry in &entries {
            let Some(flight) = entry.start() else {
                continue;
            };
            called.push(entry.id);
            {
                let mut calls = self.calls.lock().unwrap();
                let record = calls.entry(entry.id).or_default();
                if let Some((last, _, _)) = record.last() {
                    assert!(
                        *last < seq,
                        "listener {} started emit {seq} before the previous emit {last}",
                        entry.id
                    );
                }
                record.push((seq, emitter, index));
            }
            // A listener body is user code: it yields, and this one emits the
            // name again from here.
            switch_point();
            if entry.id == self.re_emitter.load(Ordering::SeqCst) {
                self.re_emit(seq);
            }
            drop(flight);
        }
        let planned: Vec<u64> = entries.iter().map(|entry| entry.id).collect();
        assert_eq!(
            called, planned,
            "an emit of {} did not call every entry it selected once",
            self.name
        );
        {
            // `TailCleanup` and `EmitTicket`: the start loop is over, so the
            // backlog record goes, and so does the tail while this dispatch is
            // the name's last. A later emit that replaced the tail already
            // holds this task as its `prev`.
            let mut bus = self.bus.lock().unwrap();
            bus.starting = None;
            if let Some(list) = bus.backlog.get_mut(&self.name) {
                list.remove(&seq);
                if list.is_empty() {
                    bus.backlog.remove(&self.name);
                }
            }
            if bus
                .tails
                .get(&self.name)
                .is_some_and(|tail| tail.generation == generation)
            {
                bus.tails.remove(&self.name);
            }
        }
        self.finished.lock().unwrap().insert(seq);
        // `drop(ticket)` and the `started` receiver: the emit's task is done.
        task.complete(None);
    }

    /// A listener emitting the name it is called for (`core/D31c`): the emit
    /// is queued behind the one whose start loop this call belongs to, and the
    /// listener does not wait for it — the chain would wait for itself.
    fn re_emit(self: &Arc<Self>, seq: u64) {
        if seq != 1 {
            return;
        }
        {
            let mut done = self.re_emitted.lock().unwrap();
            if *done {
                return;
            }
            *done = true;
        }
        drop(self.emit(EMIT_FROM_LISTENER, 0));
    }
}

/// Model 10, the emit chain (#622, `core/D31a`, `core/D31c`, `events/emit b`):
/// two emitters emit twice each while a third listener registers, and one
/// listener emits the same name again from inside its body. Every emit's calls
/// start after the previous emit's of that name did — for both emitters, in
/// each emitter's own order — one start loop runs at a time, each dispatch
/// calls exactly the entries it selected, once each, and the name ends with no
/// tail and no backlog: nothing was left waiting.
///
/// The model runs the kernel's `Table` (the shared list and the chains of what
/// was appended since, #977/#980), `Selected`, `Entry`, `Entry::start` and
/// `TransitionTask`/`join_task`. What is mirrored is `emit_erased`'s
/// bookkeeping (selection, backlog, tail, generation) and the start loop,
/// because the kernel's tail is a `tokio::JoinHandle` that only a running
/// runtime completes; the model's tail is a task its dispatch thread completes,
/// which the next emit joins. The kernel spawns while holding the state lock;
/// the model starts its thread after it, so the thread's first step is the same
/// tail wait.
///
/// Mutations this model catches: dropping the tail wait (`emit_erased`'s
/// `prev.await`) makes an emit start its calls before the earlier one finished
/// (`emit 2 of model::emit started its calls before an earlier emit finished`),
/// and dispatching `Selected::base()` alone (dropping `chains()`, #980) leaves
/// the listener registered after the shared list was built uncalled (`listener
/// 3 was never called`).
#[test]
fn shuttle_emit_chain_starts_calls_in_emission_order() {
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let ctx = Ctx::root_with(runtime.handle().clone());
        let fiber = ctx.root_view().expect("the root view").inner.clone();
        let model = Arc::new(EmitModel {
            name: Arc::from("model::emit"),
            bus: SyncMutex::new(EmitBus::default()),
            calls: SyncMutex::new(HashMap::new()),
            finished: SyncMutex::new(HashSet::new()),
            re_emitter: AtomicU64::new(u64::MAX),
            re_emitted: SyncMutex::new(false),
        });
        // The two listeners every emit sees; the first one emits the name
        // again from inside its body.
        let first = model.register(&ctx, &fiber);
        model.re_emitter.store(first.id, Ordering::SeqCst);
        let second = model.register(&ctx, &fiber);

        // The list the name's emits share is built here, as the first emit
        // builds it and holds it (`table.shared` returns the `Selected` the
        // emit's task keeps): a listener registered after this reaches the
        // emits through `Selected::chains` only (#980).
        let _priming = model.bus.lock().unwrap().table.shared(&model.name);
        let late = model.register(&ctx, &fiber);
        // And one registered while the emits are being queued: it may land
        // after some dispatch took its list, so only its order is asserted.
        let registrar = {
            let (model, ctx, fiber) = (model.clone(), ctx.clone(), fiber.clone());
            thread::spawn(move || {
                switch_point();
                model.register(&ctx, &fiber)
            })
        };
        // 2 emitters × 2 emits each.
        let emitters: Vec<_> = (0..2u64)
            .map(|emitter| {
                let model = model.clone();
                thread::spawn(move || {
                    let mut tasks = Vec::new();
                    for index in 0..2u64 {
                        switch_point();
                        tasks.push(model.emit(emitter, index));
                    }
                    // `emit`'s receiver: it fires once every call started.
                    for task in tasks {
                        assert!(block_on(join_task(&task)).is_ok(), "an emit failed");
                    }
                })
            })
            .collect();

        let racing = registrar.join().unwrap();
        for emitter in emitters {
            emitter.join().unwrap();
        }
        // The listener's own emit was queued last: wait for the chain's tail.
        let tail = model
            .bus
            .lock()
            .unwrap()
            .tails
            .get(&model.name)
            .map(|tail| tail.task.clone());
        if let Some(tail) = tail {
            assert!(block_on(join_task(&tail)).is_ok(), "an emit failed");
        }

        {
            let bus = model.bus.lock().unwrap();
            assert!(bus.tails.is_empty(), "an emit chain was left behind");
            assert!(
                bus.backlog.is_empty(),
                "an accepted emit was never finished"
            );
            assert!(bus.starting.is_none(), "an emit is still in its start loop");
        }
        assert!(
            *model.re_emitted.lock().unwrap(),
            "the listener never emitted the name it was called for"
        );

        let calls = model.calls.lock().unwrap();
        // 2 emitters × 2 emits, plus the listener's own emit. The listener
        // registered after the shared list was built is in every one of them
        // through the chains.
        for id in [first.id, second.id, late.id] {
            let record = calls
                .get(&id)
                .unwrap_or_else(|| panic!("listener {id} was never called"));
            assert_eq!(
                record.len(),
                5,
                "listener {id} got {} calls, not 5",
                record.len()
            );
            let seqs: Vec<u64> = record.iter().map(|(seq, _, _)| *seq).collect();
            let mut ordered = seqs.clone();
            ordered.sort_unstable();
            assert_eq!(
                seqs, ordered,
                "listener {id} started the emits out of order"
            );
            // Each emitter's own two emits start in the order it made them.
            for emitter in [0u64, 1u64] {
                let own: Vec<u64> = record
                    .iter()
                    .filter(|(_, heard, _)| *heard == emitter)
                    .map(|(_, _, index)| *index)
                    .collect();
                let mut ordered = own.clone();
                ordered.sort_unstable();
                assert_eq!(
                    own, ordered,
                    "emitter {emitter}'s emits started out of order at listener {id}"
                );
            }
        }
        // The racing listener is in the chain of every dispatch that turned
        // its `Selected` into a list after the insertion; wherever it is
        // called, the order is the emit order, and no emit calls it twice.
        if let Some(record) = calls.get(&racing.id) {
            let seqs: Vec<u64> = record.iter().map(|(seq, _, _)| *seq).collect();
            let mut ordered = seqs.clone();
            ordered.sort_unstable();
            assert_eq!(
                seqs, ordered,
                "the racing listener started the emits out of order"
            );
            assert!(
                record.len() <= 5,
                "an emit called the racing listener more than once"
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Model 11: the listener table's watchers (#622, `events/remote`,
// EVP-4.4-5, EVP-12.1-9)

type Watcher = Arc<dyn Fn(&TableChange) + Send + Sync>;

/// The table-watch half of `BusState`: the table and the watchers registered
/// on it, under one lock (`EventBus::change` queues the notice of a change
/// under it, so the notice order is the order the changes were made in).
#[derive(Default)]
struct WatchedTable {
    table: Table,
    watchers: Vec<(u64, Watcher)>,
    next_watcher: u64,
}

/// `bus.rs::Notices`: the changes not yet heard, in the order they were made,
/// one delivery at a time.
#[derive(Default)]
struct Notices {
    queue: VecDeque<(u64, Vec<Watcher>, Vec<TableChange>)>,
    queued: u64,
    delivered: u64,
    delivering: bool,
}

/// One change the watcher heard: its sequence, the entry, and whether it was
/// an addition.
type Heard = (u64, u64, bool);

thread_local! {
    /// The kernel's `DELIVERING`: this thread is delivering the bus's notices,
    /// so a change a watcher makes is delivered by that delivery, not by a
    /// nested one that would wait for itself.
    static DELIVERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn heard_of(seq: u64, change: &TableChange) -> Heard {
    match change {
        TableChange::Added(entry) => (seq, entry.id, true),
        TableChange::Removed(entry) => (seq, entry.id, false),
    }
}

fn delivering() -> bool {
    DELIVERING.with(std::cell::Cell::get)
}

struct WatchModel {
    state: SyncMutex<WatchedTable>,
    notices: SyncMutex<Notices>,
    delivered: ShuttleCondvar,
    /// The model's own monitor, written without a scheduling point: the last
    /// sequence every watcher heard. It is what a caller of `deliver` checks,
    /// so a `deliver` that returns before the delivery finished is caught even
    /// if another thread would deliver the change later.
    heard: AtomicU64,
    /// What the watcher heard, in delivery order.
    log: SyncMutex<Vec<Heard>>,
    /// The entries this model registered, by id, so a watcher can read the
    /// `withdrawn` flag the disposer sets before it changes the table.
    entries: SyncMutex<HashMap<u64, Arc<Entry>>>,
}

impl WatchModel {
    /// `EventBus::change`: change the table and queue the notice under the same
    /// lock, then deliver it before returning.
    fn change_seq<R>(
        &self,
        f: impl FnOnce(&mut Table) -> (R, Vec<TableChange>),
    ) -> (R, Option<u64>) {
        let (out, seq) = {
            let mut state = self.state.lock().unwrap();
            let (out, changes) = f(&mut state.table);
            let seq = self.enqueue(&state, changes);
            (out, seq)
        };
        if let Some(seq) = seq {
            self.deliver(seq);
        }
        (out, seq)
    }

    /// `EventBus::enqueue`: the watchers of this moment hear the changes, in
    /// the order the changes were queued.
    fn enqueue(&self, state: &WatchedTable, changes: Vec<TableChange>) -> Option<u64> {
        if changes.is_empty() {
            return None;
        }
        let watchers: Vec<Watcher> = state
            .watchers
            .iter()
            .map(|(_, watcher)| watcher.clone())
            .collect();
        let mut notices = self.notices.lock().unwrap();
        notices.queued += 1;
        let seq = notices.queued;
        notices.queue.push_back((seq, watchers, changes));
        Some(seq)
    }

    /// `EventBus::deliver`: return once change `seq` and those before it were
    /// heard — delivered here, one at a time and in order, or by the thread
    /// delivering now. A change a watcher makes while it hears one is delivered
    /// right after by the same delivery.
    fn deliver(&self, seq: u64) {
        if delivering() {
            return;
        }
        let mut notices = self.notices.lock().unwrap();
        loop {
            if !notices.delivering {
                let Some((at, watchers, changes)) = notices.queue.pop_front() else {
                    return;
                };
                notices.delivering = true;
                drop(notices);
                DELIVERING.with(|delivering| delivering.set(true));
                for change in &changes {
                    self.log.lock().unwrap().push(heard_of(at, change));
                    for watcher in &watchers {
                        watcher(change);
                    }
                }
                DELIVERING.with(|delivering| delivering.set(false));
                self.heard.store(at, Ordering::SeqCst);
                notices = self.notices.lock().unwrap();
                notices.delivered = at;
                notices.delivering = false;
                self.delivered.notify_all();
                continue;
            }
            if notices.delivered >= seq {
                return;
            }
            notices = self.delivered.wait(notices).unwrap();
        }
    }

    /// What `deliver`'s contract promises a caller that is not itself
    /// delivering: the change was heard before it returned. A change made by a
    /// watcher is heard by the delivery in progress after the watcher returns,
    /// so it is not checked here.
    fn heard_before_returning(&self, seq: Option<u64>) {
        if delivering() {
            return;
        }
        let Some(seq) = seq else {
            return;
        };
        let heard = self.heard.load(Ordering::SeqCst);
        assert!(
            heard >= seq,
            "a change was reported before the watcher heard it ({heard} < {seq})"
        );
    }

    /// `EventBus::register`'s insertion and notice, delivered before `on`
    /// returns (a watcher that makes the change hears it after this call).
    fn register(&self, ctx: &Ctx, fiber: &Arc<FiberInner>, name: &str) -> (Arc<Entry>, u64) {
        let (entry, seq) = {
            let mut state = self.state.lock().unwrap();
            let id = state.table.next_id();
            let position = state.table.position(false);
            let entry = Arc::new(named_entry(ctx, fiber, id, position, name));
            state.table.insert(entry.clone());
            self.entries.lock().unwrap().insert(entry.id, entry.clone());
            let seq = self
                .enqueue(&state, vec![TableChange::Added(entry.table_entry())])
                .expect("an insertion is a change");
            (entry, seq)
        };
        self.deliver(seq);
        self.heard_before_returning(Some(seq));
        (entry, seq)
    }

    /// `EventBus`'s listener disposer: withdrawn first, then the table change,
    /// so a call that starts after the removal is skipped. `false` when the
    /// entry was not in the table any more — no change, no notice.
    fn withdraw(&self, entry: &Arc<Entry>) -> bool {
        entry.withdrawn.store(true, Ordering::SeqCst);
        let (removed, seq) = self.change_seq(|table| {
            let removed = table.remove(entry);
            let changes = if removed {
                vec![TableChange::Removed(entry.table_entry())]
            } else {
                Vec::new()
            };
            (removed, changes)
        });
        self.heard_before_returning(seq);
        removed
    }

    /// `EventBus::watch_table`: the current entries, then every later change,
    /// both under the state lock, so a change made after this is queued and one
    /// made before it is in the snapshot.
    fn watch(&self, watcher: Watcher) -> Vec<TableEntry> {
        let mut state = self.state.lock().unwrap();
        let snapshot: Vec<TableEntry> = state
            .table
            .entries()
            .iter()
            .map(|entry| entry.table_entry())
            .collect();
        state.next_watcher += 1;
        let id = state.next_watcher;
        state.watchers.push((id, watcher));
        snapshot
    }
}

/// Model 11, the listener table's watchers (#622, `events/remote`,
/// EVP-4.4-5, EVP-12.1-9): two registrars add two listeners each, a third
/// caller withdraws one of them, and one watcher hears every change — and,
/// while hearing one, withdraws that listener and registers another. Every
/// change is heard exactly once, in the order it was made; each caller that is
/// not itself delivering returns only after its own change was heard
/// (`deliver(seq)`), and a change a watcher makes is delivered by the delivery
/// in progress instead of one that would wait for itself. A listener withdrawn
/// by the time a dispatch asks for it never starts a call.
///
/// The model runs the kernel's `Table` (insert, remove, the shared lists the
/// emits hold, `entries`), `TableChange`/`TableEntry` and `Entry::start`. The
/// bus's std locks and its std `Condvar` are mirrored with shuttle's
/// (`SyncMutex`, `ShuttleCondvar`): a model that ran the real `BusState` would
/// park on a std condvar, which shuttle cannot see. The thread-local
/// `DELIVERING` is the kernel's.
///
/// Mutations this model catches: returning from `deliver` without waiting for
/// `seq` (a registrar returns before the watcher heard its addition: `a change
/// was reported before the watcher heard it (2 < 6)`); dropping `notify_all`
/// after a delivery (the waiting caller never wakes: `deadlock!`); dropping the
/// `DELIVERING` guard (a watcher that changes the table waits for its own
/// delivery: `deadlock!`); and removing the entry before setting `withdrawn`
/// (the removal is heard while the entry still counts as live: `a removal was
/// heard for a listener that was not withdrawn`).
#[test]
fn shuttle_table_watcher_hears_each_change_once_in_order() {
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let ctx = Ctx::root_with(runtime.handle().clone());
        let fiber = ctx.root_view().expect("the root view").inner.clone();
        let name = "model::watched";
        let model = Arc::new(WatchModel {
            state: SyncMutex::new(WatchedTable::default()),
            notices: SyncMutex::new(Notices::default()),
            delivered: ShuttleCondvar::new(),
            heard: AtomicU64::new(0),
            log: SyncMutex::new(Vec::new()),
            entries: SyncMutex::new(HashMap::new()),
        });
        // The watcher: on the first addition it hears, it withdraws that
        // listener and registers another, both from inside the delivery.
        let reentered = StdBool::new(false);
        let watcher: Watcher = {
            let model = Arc::downgrade(&model);
            let (ctx, fiber) = (ctx.clone(), fiber.clone());
            let name = name.to_string();
            Arc::new(move |change: &TableChange| {
                let Some(bus) = model.upgrade() else {
                    return;
                };
                // The watcher is user code: let the other threads run here.
                switch_point();
                if let TableChange::Removed(entry) = change {
                    // The disposer withdraws the entry before it changes the
                    // table, so a removal is never heard while the entry still
                    // counts as live (`Entry::live`).
                    let withdrawn = bus
                        .entries
                        .lock()
                        .unwrap()
                        .get(&entry.id)
                        .map(|entry| entry.withdrawn.load(Ordering::SeqCst));
                    assert_eq!(
                        withdrawn,
                        Some(true),
                        "a removal was heard for a listener that was not withdrawn"
                    );
                }
                let TableChange::Added(entry) = change else {
                    return;
                };
                if reentered.swap(true, Ordering::SeqCst) {
                    return;
                }
                let entry = bus.state.lock().unwrap().table.get(entry.id);
                if let Some(entry) = entry {
                    bus.withdraw(&entry);
                }
                bus.register(&ctx, &fiber, &name);
            })
        };
        let snapshot = model.watch(watcher);
        assert!(
            snapshot.is_empty(),
            "the watcher's snapshot is not the table before it registered"
        );

        // A dispatch over the table, interleaved with the registrations: a
        // listener withdrawn before its call starts never runs one.
        let dispatcher = {
            let (model, name) = (model.clone(), name.to_string());
            thread::spawn(move || {
                for _ in 0..4 {
                    switch_point();
                    let selected = {
                        let mut state = model.state.lock().unwrap();
                        let name: Arc<str> = Arc::from(name.as_str());
                        state.table.shared(&name)
                    };
                    let Some(selected) = selected else {
                        continue;
                    };
                    for entry in selected.to_vec() {
                        let withdrawn = entry.withdrawn.load(Ordering::SeqCst);
                        let flight = entry.start();
                        assert!(
                            !withdrawn || flight.is_none(),
                            "listener {} started a call after it was withdrawn",
                            entry.id
                        );
                        if let Some(flight) = flight {
                            // `EventBus::start` counts the call after the
                            // liveness check.
                            entry.calls.fetch_add(1, Ordering::Relaxed);
                            switch_point();
                            drop(flight);
                        }
                    }
                }
            })
        };

        // Registrar A: two listeners, then its own withdrawals.
        let (give, given) = tokio::sync::oneshot::channel::<Arc<Entry>>();
        let a = {
            let (model, ctx, fiber) = (model.clone(), ctx.clone(), fiber.clone());
            let name = name.to_string();
            thread::spawn(move || {
                switch_point();
                let (first, _) = model.register(&ctx, &fiber, &name);
                let _ = give.send(first.clone());
                let (second, _) = model.register(&ctx, &fiber, &name);
                switch_point();
                // The withdrawer, or the watcher, may have taken either first:
                // whoever did, its removal is reported once.
                let _ = model.withdraw(&second);
                let _ = model.withdraw(&first);
            })
        };
        // Registrar B: two more, then its own withdrawals. The watcher takes
        // one of them from inside a delivery.
        let b = {
            let (model, ctx, fiber) = (model.clone(), ctx.clone(), fiber.clone());
            let name = name.to_string();
            thread::spawn(move || {
                switch_point();
                let (third, _) = model.register(&ctx, &fiber, &name);
                let (fourth, _) = model.register(&ctx, &fiber, &name);
                switch_point();
                let _ = model.withdraw(&fourth);
                let _ = model.withdraw(&third);
            })
        };
        // The withdrawer: one listener of A's, twice.
        let w = {
            let model = model.clone();
            thread::spawn(move || {
                switch_point();
                let first = block_on(given).expect("the first registration");
                let _ = model.withdraw(&first);
                assert!(
                    !model.withdraw(&first),
                    "a second withdrawal reported a change"
                );
            })
        };

        w.join().unwrap();
        a.join().unwrap();
        b.join().unwrap();
        dispatcher.join().unwrap();

        // 5 additions (A's two, B's two, the watcher's one) and 4 removals
        // (every listener A and B registered, by whoever got there first).
        let log = model.log.lock().unwrap().clone();
        let seqs: Vec<u64> = log.iter().map(|(seq, _, _)| *seq).collect();
        assert_eq!(
            seqs,
            (1..=9u64).collect::<Vec<_>>(),
            "the watcher did not hear every change exactly once, in order"
        );
        let added: HashSet<u64> = log
            .iter()
            .filter(|(_, _, added)| *added)
            .map(|(_, id, _)| *id)
            .collect();
        let removed: HashSet<u64> = log
            .iter()
            .filter(|(_, _, added)| !*added)
            .map(|(_, id, _)| *id)
            .collect();
        assert_eq!(added.len(), 5, "the additions heard are not five listeners");
        assert_eq!(
            removed.len(),
            4,
            "the removals heard are not four listeners"
        );
        for id in &added {
            let times = log
                .iter()
                .filter(|(_, at, added)| at == id && *added)
                .count();
            assert_eq!(times, 1, "listener {id}'s addition was heard {times} times");
        }
        for id in &removed {
            assert!(
                added.contains(id),
                "listener {id}'s removal was heard before its addition"
            );
            let times = log
                .iter()
                .filter(|(_, at, added)| at == id && !*added)
                .count();
            assert_eq!(times, 1, "listener {id}'s removal was heard {times} times");
        }
        {
            let state = model.state.lock().unwrap();
            let left = state.table.entries();
            assert_eq!(
                left.len(),
                1,
                "the table does not hold just the watcher's listener"
            );
            assert!(
                added.contains(&left[0].id) && !removed.contains(&left[0].id),
                "the listener left in the table is not the one the watcher added"
            );
            assert_eq!(
                state.watchers.len(),
                1,
                "a watcher was registered or lost more than once"
            );
        }
        {
            let notices = model.notices.lock().unwrap();
            assert!(notices.queue.is_empty(), "a notice was never delivered");
            assert_eq!(notices.queued, 9, "a change was made without a notice");
            assert_eq!(notices.delivered, 9, "a notice was never heard");
        }
    });
}

// ---------------------------------------------------------------------------
// Model 12: dispose registration and per-generation cancellation (#622,
// `core/D12`, `core/features.errors`)

/// The model's event: one listener per generation, never called.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ModelEvent;

impl Event for ModelEvent {
    const NAME: &'static str = "model::generation";
    type Value = ();
}

/// The counters the model asserts over. Std atomics: they are read after the
/// joins, where a shuttle load would itself be a scheduling point.
#[derive(Default)]
struct GenerationCounts {
    /// Generations `load` started (the terminal check passed).
    starts: AtomicUsize,
    /// Applies that ran.
    applies: AtomicUsize,
    /// Applies whose `ctx.cancelled()` returned.
    cancelled: AtomicUsize,
    /// Applies that were cancelled at birth (`pre_cancelled` in `load`).
    born_cancelled: AtomicUsize,
    /// Effect factories that ran.
    factories: AtomicUsize,
    /// Their cleanups.
    cleanups: AtomicUsize,
    /// `ctx.effect`/`ctx.on_sync` calls the fiber refused.
    refused: AtomicUsize,
}

/// What the driver hands the model's plugin: the counters, and the generation
/// whose apply is running (the initiators wait for it, so every run registers
/// the dispose while an apply is running).
struct Generations {
    counts: Arc<GenerationCounts>,
    started: watch::Sender<u64>,
}

/// `FiberInner::post` on the model's mailbox: a stopped driver completes the
/// intent's task at once.
fn post_model(fiber: &FiberInner, intents: &mpsc::UnboundedSender<Intent>, intent: Intent) -> bool {
    if !fiber.alive.load(Ordering::SeqCst) {
        complete_intent(&intent, fiber.stopped_error());
        return false;
    }
    match intents.send(intent) {
        Ok(()) => true,
        Err(error) => {
            complete_intent(&error.0, fiber.stopped_error());
            false
        }
    }
}

/// `FiberView::register_dispose`, mirrored on the model's mailbox: the check
/// and the registration share the transition lock, so a restart registered
/// after it sees the terminal task, and a `load` that starts after it is
/// refused. The pre-cancel is the #619 fix: a running `apply` returns through
/// `ctx.cancelled()` instead of holding the serial driver forever.
fn register_dispose(
    fiber: &Arc<FiberInner>,
    intents: &mpsc::UnboundedSender<Intent>,
) -> Arc<TransitionTask> {
    let (task, newly_registered) = {
        let mut tr = fiber.transition.lock().unwrap();
        match &tr.terminal_task {
            Some(task) => (task.clone(), false),
            None => {
                tr.pre_cancelled = true;
                fiber.cancel_current();
                let task = TransitionTask::new();
                tr.terminal_task = Some(task.clone());
                (task, true)
            }
        }
    };
    if newly_registered {
        post_model(fiber, intents, Intent::Dispose);
        if !fiber.alive.load(Ordering::SeqCst) {
            task.complete(fiber.stopped_error());
        }
    }
    task
}

/// `FiberView::restart`'s registration, mirrored: the pre-cancel is the same
/// #619/#968 rule. The kernel reads `alive` under the transition lock only for
/// a non-root fiber (`!this.is_root && …`); the model's fiber is the root, so
/// the read does not happen here either — it would be a scheduling point under
/// a std lock.
fn register_restart(
    fiber: &Arc<FiberInner>,
    intents: &mpsc::UnboundedSender<Intent>,
) -> Result<Arc<TransitionTask>, Arc<ErrorKind>> {
    {
        let mut tr = fiber.transition.lock().unwrap();
        if fiber.closing.load(Ordering::SeqCst) || fiber.ctx.shared().closing.load(Ordering::SeqCst)
        {
            return Err(Arc::new(ErrorKind::Closed));
        }
        // Counted under the same lock as the load's `pre_cancelled`, so a
        // generation that starts before the driver takes this up starts
        // cancelled too, and neither counts as a failed load (#968).
        tr.pending_restarts += 1;
        tr.pre_cancelled = true;
        fiber.cancel_current();
    }
    let task = TransitionTask::new();
    post_join_to(&fiber.alive, intents, task.clone(), Intent::Restart, || {
        fiber.stopped_error()
    });
    Ok(task)
}

/// `FiberView::restart`, mirrored: register, then wait for the new generation.
fn restart(
    fiber: &Arc<FiberInner>,
    intents: &mpsc::UnboundedSender<Intent>,
) -> Result<(), Arc<ErrorKind>> {
    let task = register_restart(fiber, intents)?;
    block_on(join_task(&task))
}

/// `FiberInner::drain_effects` for the model: take the records, drain each in
/// reverse order (`claim`, run, `finish`, `join` — the kernel's `drain` spawns
/// the runner on the runtime, which a shuttle model cannot pump), then clear
/// `last_deps` and the provided table. The model has no definition copies and
/// no provided services.
fn drain_model_effects(fiber: &Arc<FiberInner>) -> Vec<Arc<ErrorKind>> {
    let handle = fiber.ctx.handle().clone();
    let effects: Vec<Arc<EffectRecord>> = std::mem::take(&mut *fiber.effects.lock().unwrap());
    let mut errors = Vec::new();
    for record in effects.into_iter().rev() {
        if let Some(mut cleanups) = record.claim() {
            let error = block_on(run_cleanups(&mut cleanups, &handle));
            record.finish(error);
        }
        if let Err(error) = block_on(record.join()) {
            errors.push(error);
        }
    }
    errors.extend(std::mem::take(&mut *fiber.drained_errors.lock().unwrap()));
    *fiber.last_deps.lock().unwrap() = None;
    fiber.provided.lock().unwrap().clear();
    errors
}

/// The model's plugin: one effect and one listener per generation, then the
/// generation's cancellation (a cooperative plugin, D27). A pre-cancelled
/// generation must see the cancellation before it registers anything (#968).
fn apply_generation(
    fiber: &Arc<FiberInner>,
    generation: u64,
    born_cancelled: bool,
    generations: &Arc<Generations>,
) {
    let counts = &generations.counts;
    let ctx = fiber.ctx.for_generation(generation);
    let seen = ctx.cancellation_token().is_cancelled();
    if born_cancelled {
        assert!(
            seen,
            "a pre-cancelled generation's apply did not see the cancellation"
        );
        counts.born_cancelled.fetch_add(1, Ordering::SeqCst);
    }
    let registered = ctx.effect_named("model generation", || {
        counts.factories.fetch_add(1, Ordering::SeqCst);
        let counts = counts.clone();
        // The factory runs outside the admission lock: a scheduling point
        // between the two checks (`register_effect_named`).
        switch_point();
        Effect::Disposer(Box::new(move || {
            counts.cleanups.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }))
    });
    if registered.is_err() {
        counts.refused.fetch_add(1, Ordering::SeqCst);
    }
    if ctx.on_sync::<ModelEvent>(|_, _| Ok(None)).is_err() {
        counts.refused.fetch_add(1, Ordering::SeqCst);
    }
    counts.applies.fetch_add(1, Ordering::SeqCst);
    // The plugin's own work: it waits for the generation to be cancelled, so a
    // dispose of this fiber returns instead of waiting for an apply that never
    // ends (Bug A, #619).
    let _ = generations.started.send(generation);
    switch_point();
    block_on(ctx.cancelled());
    counts.cancelled.fetch_add(1, Ordering::SeqCst);
}

/// `FiberInner::load`, mirrored where the two checks, the token and the
/// generation are: the terminal check and the new generation token share the
/// transition lock the dispose registration takes, so a generation either
/// starts and is cancelled, or does not start at all — it never starts
/// uncancelled behind a registered dispose.
fn load_generation(fiber: &Arc<FiberInner>, generations: &Arc<Generations>) {
    let shared = fiber.ctx.shared().clone();
    let _admission = shared.admission.lock().unwrap();
    if fiber.closing.load(Ordering::SeqCst) || shared.closing.load(Ordering::SeqCst) {
        return;
    }
    let (generation, born_cancelled) = {
        let mut tr = fiber.transition.lock().unwrap();
        if tr.terminal_task.is_some() {
            return;
        }
        let token = fiber.new_generation_token();
        // A generation that starts while a restart waits starts cancelled: the
        // driver is stopping it before its `apply` runs (#968).
        tr.pre_cancelled = tr.pending_restarts > 0;
        if tr.pre_cancelled {
            token.cancel();
        }
        tr.generation += 1;
        tr.error = None;
        fiber.set_state(&mut tr, FiberState::Loading);
        (tr.generation, tr.pre_cancelled)
    };
    generations.counts.starts.fetch_add(1, Ordering::SeqCst);
    drop(_admission);
    // `apply` is user code and runs on the driver (D7).
    apply_generation(fiber, generation, born_cancelled, generations);
    if fiber.closing.load(Ordering::SeqCst) {
        unload_generation(fiber, NextState::Disposed);
        return;
    }
    // `refresh_deps` returns at the plugin check: the model declares no
    // injects, so `apply` sets the state.
    {
        let mut tr = fiber.transition.lock().unwrap();
        tr.error = None;
        fiber.set_state(&mut tr, FiberState::Active);
    }
}

/// `FiberInner::unload`, mirrored: `Unloading` under the admission and
/// transition locks, cancel, the (empty) event wait, the cleanup pass, then the
/// next state. `flush_status`'s `FiberStatusChanged` emits are not modelled:
/// they read no lifecycle state.
fn unload_generation(fiber: &Arc<FiberInner>, next: NextState) {
    {
        let _admission = fiber.ctx.shared().admission.lock().unwrap();
        let mut tr = fiber.transition.lock().unwrap();
        fiber.set_state(&mut tr, FiberState::Unloading);
    }
    fiber.cancel_current();
    let errors = drain_model_effects(fiber);
    let err = aggregate_arcs(errors);
    let mut tr = fiber.transition.lock().unwrap();
    match next {
        NextState::Pending => {
            if let Some(error) = err {
                (fiber.ctx.error_sink())(error);
            }
            fiber.set_state(&mut tr, FiberState::Pending);
        }
        NextState::Disposed => {
            tr.error = err;
            fiber.set_state(&mut tr, FiberState::Disposed);
        }
    }
}

/// The root's driver, mirroring `drive`'s Dispose and Restart arms and the load
/// they reach. The root's driver keeps running after a dispose
/// (`terminal_dispose` is for a non-root fiber), so the model ends it by closing
/// the mailbox.
fn drive_generations(
    fiber: Arc<FiberInner>,
    mut mailbox: mpsc::UnboundedReceiver<Intent>,
    generations: Arc<Generations>,
) {
    while let Some(intent) = block_on(mailbox.recv()) {
        match intent {
            Intent::Dispose => {
                if fiber.state() != FiberState::Disposed {
                    unload_generation(&fiber, NextState::Disposed);
                }
                let (task, err) = {
                    let tr = fiber.transition.lock().unwrap();
                    (tr.terminal_task.clone(), tr.error.clone())
                };
                if let Some(task) = task {
                    task.complete(err);
                }
            }
            Intent::Restart(task) => {
                {
                    let mut tr = fiber.transition.lock().unwrap();
                    tr.pending_restarts = tr.pending_restarts.saturating_sub(1);
                }
                if fiber.closing.load(Ordering::SeqCst)
                    || fiber.ctx.shared().closing.load(Ordering::SeqCst)
                {
                    task.complete(Some(Arc::new(ErrorKind::Closed)));
                    continue;
                }
                let state = fiber.state();
                if state == FiberState::Disposed {
                    // The root: a disposed root becomes Active again, with a
                    // new token and no terminal task (the kernel's
                    // `if this.is_root` branch).
                    fiber.new_generation_token();
                    {
                        let mut tr = fiber.transition.lock().unwrap();
                        tr.error = None;
                        tr.terminal_task = None;
                        fiber.set_state(&mut tr, FiberState::Active);
                    }
                    let error = fiber.transition.lock().unwrap().error.clone();
                    task.complete(error);
                    continue;
                }
                if state != FiberState::Pending {
                    unload_generation(&fiber, NextState::Pending);
                }
                load_generation(&fiber, &generations);
                // `complete_task`: the fiber's error, which `load` sets to
                // `None` when it refuses the generation.
                let error = fiber.transition.lock().unwrap().error.clone();
                task.complete(error);
            }
            _ => {}
        }
    }
}

/// One run of model 12: two restarts start two generations — the first
/// cancelled at birth (the second restart is still waiting), the second
/// applying on a live token — and then a dispose registers while that apply
/// runs, with (`with_restart`) or without a restart registering beside it.
fn close_race(with_restart: bool) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let ctx = Ctx::root_with(runtime.handle().clone());
    let fiber = ctx.root_view().expect("the root view").inner.clone();
    let (started, started_rx) = watch::channel(0u64);
    let generations = Arc::new(Generations {
        counts: Arc::new(GenerationCounts::default()),
        started,
    });
    let counts = generations.counts.clone();
    let (intents, mailbox) = mpsc::unbounded_channel::<Intent>();
    // Both restarts are queued before the driver starts, so the first
    // generation starts while the second restart still waits: it is cancelled
    // at birth, as #968 has it.
    let prime = [
        register_restart(&fiber, &intents).expect("a restart"),
        register_restart(&fiber, &intents).expect("a restart"),
    ];
    let driver = {
        let (fiber, generations) = (fiber.clone(), generations.clone());
        thread::spawn(move || drive_generations(fiber, mailbox, generations))
    };

    // The initiators wait for the second generation: its apply runs on a
    // live token, so only their registrations can cancel it.
    let wait_for_apply = {
        let started = started_rx.clone();
        move || {
            let mut started = started;
            block_on(async {
                while *started.borrow_and_update() < 2 {
                    if started.changed().await.is_err() {
                        break;
                    }
                }
            });
        }
    };
    let disposer = {
        let (fiber, intents) = (fiber.clone(), intents.clone());
        let wait_for_apply = wait_for_apply.clone();
        thread::spawn(move || {
            switch_point();
            wait_for_apply();
            let task = register_dispose(&fiber, &intents);
            block_on(join_task(&task))
        })
    };
    let restarter = with_restart.then(|| {
        let (fiber, intents) = (fiber.clone(), intents.clone());
        let wait_for_apply = wait_for_apply.clone();
        thread::spawn(move || {
            switch_point();
            wait_for_apply();
            restart(&fiber, &intents)
        })
    });

    assert!(
        block_on(join_task(&prime[0])).is_ok(),
        "the first primed restart failed"
    );
    assert!(
        block_on(join_task(&prime[1])).is_ok(),
        "the second primed restart failed"
    );
    let dispose_result = disposer.join().unwrap();
    assert!(
        dispose_result.is_ok(),
        "a dispose returned {dispose_result:?}"
    );
    if let Some(restarter) = restarter {
        let restart_result = restarter.join().unwrap();
        assert!(
            restart_result.is_ok(),
            "a restart returned {restart_result:?}"
        );
    }

    // One last dispose, after the initiators: whatever the restart left is
    // unloaded, and its cleanup runs.
    let last = register_dispose(&fiber, &intents);
    assert!(
        block_on(join_task(&last)).is_ok(),
        "the last dispose returned an error"
    );
    drop(intents);
    driver.join().unwrap();

    let starts = counts.starts.load(Ordering::SeqCst);
    let applies = counts.applies.load(Ordering::SeqCst);
    let cancelled = counts.cancelled.load(Ordering::SeqCst);
    let born_cancelled = counts.born_cancelled.load(Ordering::SeqCst);
    let factories = counts.factories.load(Ordering::SeqCst);
    let cleanups = counts.cleanups.load(Ordering::SeqCst);
    assert!(
        starts >= 2,
        "the priming restarts started {starts} generations, not two"
    );
    assert!(
        born_cancelled >= 1,
        "no generation was cancelled at birth while a restart waited"
    );
    assert_eq!(
        applies, starts,
        "a generation started without running its apply"
    );
    assert_eq!(
        cancelled, starts,
        "an apply returned without its generation being cancelled"
    );
    assert_eq!(
        counts.refused.load(Ordering::SeqCst),
        0,
        "a generation's apply could not register its effect or its listener"
    );
    assert_eq!(
        cleanups, factories,
        "a generation's effect was not cleaned up exactly once ({cleanups} cleanups for \
         {factories} factories)"
    );
    assert!(
        fiber.effects.lock().unwrap().is_empty(),
        "a generation's effect record was left in the fiber"
    );
    assert!(
        fiber.effect_index.lock().unwrap().is_empty(),
        "a generation's effect record was left in the index"
    );
    assert_eq!(
        *fiber.event_flights.lock().unwrap(),
        0,
        "a listener call was never finished"
    );
    assert_eq!(
        fiber.state(),
        FiberState::Disposed,
        "the last dispose did not end the root"
    );
    assert_eq!(
        ctx.events().table_counts().0,
        0,
        "a generation's listener was left in the table"
    );
    assert!(
        fiber.alive.load(Ordering::SeqCst),
        "the root's driver stopped on a dispose"
    );
    assert_eq!(
        fiber.transition.lock().unwrap().pending_restarts,
        0,
        "a restart was never taken up"
    );
}

/// Model 12, dispose registration and per-generation cancellation (#622, see
/// also Bug A / #619 and #968): a generation is applying while a dispose and a
/// restart register. Whichever registers first, the running generation is
/// cancelled (`ctx.cancelled()` returns, so the plugin returns and the dispose
/// completes) — a dispose alone does too, with no restart to cancel for it — a
/// generation that starts after the terminal registration is refused, a
/// generation that starts while a restart waits is cancelled at birth, and
/// every generation's effect and listener are cleaned up exactly once — the
/// listener table and the effect lists end empty.
///
/// The model runs the kernel's own registration (`Ctx::effect_named` →
/// `register_effect_named`, `on_sync`), `for_generation`, the cancellation
/// token, `pre_cancelled`, `cancel_current`, `new_generation_token`,
/// `set_state`, `EffectRecord` and `TransitionTask`. `register_dispose`,
/// `restart`, `load`, `unload` and `drive`'s close arms are mirrored line for
/// line, because the real ones post to the root's driver task, whose
/// `drain_effects` spawns its cleanup runner on the runtime — a shuttle model
/// cannot pump that runtime, so the model drains the records itself
/// (`claim` → run → `finish`, as models 3 and 5 do) and owns the mailbox. The
/// root is the fiber under test, so `restart` takes the kernel's root branch
/// (no `alive` read under the transition lock); a non-root fiber's dispose and
/// restart are models 5 and 8's.
///
/// Mutations this model catches: dropping the dispose's `cancel_current` (the
/// apply of the second generation waits for a cancellation nobody sends, so the
/// driver, the joining dispose and the test park: `deadlock!`), and dropping
/// `load`'s `token.cancel()` after `pre_cancelled` (a generation that starts
/// behind a waiting restart applies without seeing the cancellation: `a
/// pre-cancelled generation's apply did not see the cancellation`).
#[test]
fn shuttle_dispose_registration_cancels_the_running_generation() {
    explore(|| {
        // The dispose and the restart race on the same running generation.
        close_race(true);
        // The dispose alone: no restart's pre-cancel can stand in for it.
        close_race(false);
    });
}

// ---------------------------------------------------------------------------
// Model 13: an effect record with an owner fiber (#622, "EffectRecord 加所属
// fiber")

/// What a cleanup's error says, for matching the reports: the label is the
/// only part the model sets.
fn cleanup_label(error: &Arc<ErrorKind>) -> String {
    error.to_string()
}

/// Model 13, a record a `Disposer` drains early against the unload that takes
/// the fiber's list (#622, "加强现有模型": `finish()`'s self-removal, where its
/// error is kept, and the `mem::take` race). `EffectRecord::finish` removes the
/// record from its owner's `effects` and `effect_index` and — only when it was
/// still in `effects` — parks the error in `drained_errors`; `drain_effects`
/// takes the whole list under the same lock and joins what it took. So each
/// failed cleanup is reported by the fiber's unload exactly once: by the join
/// of the record the unload took, or by the parked error of a record drained
/// before the take — never twice, never not at all. A record whose cleanup is
/// running when the take happens is reported by the unload's join, and its
/// caller still gets the same `Arc` from its own join.
///
/// The model runs `push_effect`, `EffectRecord::{claim, finish, join}` and the
/// kernel's `run_cleanups`; the unload side is `drain_model_effects`, the
/// mirror of `FiberInner::drain_effects` models 5 and 12 use (the real drain
/// spawns each record's runner on the runtime, which a shuttle model cannot
/// pump).
///
/// Mutations this model catches (each one made in the kernel and reverted;
/// the message after it is what this model then reports): parking the error
/// without the "still in the list" check (`the unload's reports do not carry
/// each failed cleanup exactly once (6 reports for 3 cleanups)`: the records
/// the unload took are reported by their joins and again by their parked
/// errors); dropping the parking (`(2 reports for 3 cleanups)`: the early
/// record's error is lost); dropping the `retain` from `finish` (`(4 reports
/// for 3 cleanups)`: the finished record stays in the list, so the unload parks
/// its error and joins it too).
#[test]
fn shuttle_effect_record_owner_reports_each_cleanup_error_once() {
    // The cleanups are synchronous; the runtime is what `run_cleanups` takes a
    // handle for.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let handle = runtime.handle().clone();
    let labels: Vec<String> = (0..3).map(|index| format!("cleanup {index}")).collect();
    explore(move || {
        let ctx = Ctx::root_with(handle.clone());
        let fiber = ctx.root_view().expect("the root view").inner.clone();
        let runs = Arc::new((0..3).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>());
        let records: Vec<Arc<EffectRecord>> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let runs = runs.clone();
                let label = label.clone();
                let message = label.clone();
                let record = EffectRecord::from(
                    Effect::Disposer(Box::new(move || {
                        runs[index].fetch_add(1, Ordering::SeqCst);
                        Err(Error::msg(message))
                    })),
                    label,
                    crate::effect::Origin::Other,
                    Arc::downgrade(&fiber),
                );
                fiber.push_effect(record.clone());
                record
            })
            .collect();

        // The middle record is drained early, by a `Disposer`'s caller: before
        // the take, while the unload has the list, or while the unload drains
        // another record.
        let early = {
            let early = records[1].clone();
            let handle = handle.clone();
            thread::spawn(move || {
                switch_point();
                if let Some(mut cleanups) = early.claim() {
                    let error = block_on(run_cleanups(&mut cleanups, &handle));
                    switch_point();
                    early.finish(error);
                }
                let result = block_on(early.join());
                (early, result)
            })
        };
        let unload = {
            let fiber = fiber.clone();
            thread::spawn(move || drain_model_effects(&fiber))
        };

        let (early, early_result) = early.join().unwrap();
        let reported = unload.join().unwrap();

        for (index, runs) in runs.iter().enumerate() {
            assert_eq!(
                runs.load(Ordering::SeqCst),
                1,
                "the cleanup of record {index} ran more than once"
            );
        }
        // The early caller's own join is the result its cleanup returned — the
        // same `Arc`, whether this thread or the unload ran the cleanups.
        let early_error = early_result.expect_err("the early cleanup failed");
        assert_eq!(
            cleanup_label(&early_error),
            labels[1],
            "the early join got another cleanup's result"
        );
        // The fiber's unload reports every failed cleanup exactly once.
        let mut reported_labels: Vec<String> = reported.iter().map(cleanup_label).collect();
        reported_labels.sort();
        let mut expected = labels.clone();
        expected.sort();
        assert_eq!(
            reported_labels,
            expected,
            "the unload's reports do not carry each failed cleanup exactly once \
             ({} reports for {} cleanups)",
            reported.len(),
            labels.len()
        );
        assert!(
            fiber.effects.lock().unwrap().is_empty(),
            "a finished record was left in the fiber's effect list"
        );
        assert!(
            fiber.effect_index.lock().unwrap().is_empty(),
            "a finished record was left in the fiber's effect index"
        );
        assert!(
            fiber.drained_errors.lock().unwrap().is_empty(),
            "a parked error was left unread"
        );
        drop(early);
    });
}

// ---------------------------------------------------------------------------
// Model 14: one transition task, several completions (#622, "TransitionTask 加
// 多个完成方")

/// Model 14, `TransitionTask::complete` called more than once (#622, item 3 of
/// "加强现有模型"; the primitive behind #1194's overwrite). `complete` sends
/// the result into the task's watch channel, so:
///
/// - a completion before anyone joined is kept (the keep-alive receiver), and
///   a join that comes later returns it instead of waiting forever;
/// - every join returns one of the results someone announced, never a made-up
///   error;
/// - the **last** completion is what a later join reads — the overwrite. The
///   kernel's late `alive` rechecks no longer use `complete`: they complete
///   only a `Running` task (`complete_if_running`, #1194). This model pins the
///   primitive's last-write behaviour: the first joiner sees the driver's
///   result, a joiner that comes after a later `complete` sees the later
///   result.
///
/// Mutations this model catches (each one made in the kernel and reverted): a
/// task with no keep-alive receiver — the completion before any subscriber is
/// lost and every join after it blocks (`deadlock! blocked tasks: [main-thread,
/// …]`); `complete` keeping the first result (`the last completion is not what
/// a later join reads`).
#[test]
fn shuttle_transition_task_several_completions() {
    explore(|| {
        let driver = failed("driver");
        let poster = failed("poster");

        // A completion before any joiner exists (the driver can complete while
        // the poster is between its send and its join).
        let task = TransitionTask::new();
        task.complete(Some(driver.clone()));
        let early = {
            let task = task.clone();
            thread::spawn(move || block_on(join_task(&task)))
        };
        let early = early.join().unwrap().expect_err("the driver failed");
        assert!(
            Arc::ptr_eq(&early, &driver),
            "a join started after the only completion got another result"
        );

        // The poster's second completion (`post_join_to`'s `alive` recheck):
        // the last one wins, and every join after it reads it.
        task.complete(Some(poster.clone()));
        let late = block_on(join_task(&task)).expect_err("the poster failed");
        assert!(
            Arc::ptr_eq(&late, &poster),
            "the last completion is not what a later join reads"
        );

        // Two completers at once, two joiners that started before them: every
        // join returns one of the two results.
        let raced = TransitionTask::new();
        let (one, two) = (failed("one"), failed("two"));
        let joiners: Vec<_> = (0..2)
            .map(|_| {
                let task = raced.clone();
                thread::spawn(move || block_on(join_task(&task)))
            })
            .collect();
        let completers: Vec<_> = [one.clone(), two.clone()]
            .into_iter()
            .map(|error| {
                let task = raced.clone();
                thread::spawn(move || task.complete(Some(error)))
            })
            .collect();
        for completer in completers {
            completer.join().unwrap();
        }
        for joiner in joiners {
            let result = joiner.join().unwrap().expect_err("both completions failed");
            assert!(
                Arc::ptr_eq(&result, &one) || Arc::ptr_eq(&result, &two),
                "a join got a result nobody completed the task with"
            );
        }

        // A clean completion, then a later `Closed`: `complete` overwrites, so
        // a joiner after the second call reads the error. The kernel's late
        // `alive` rechecks use `complete_if_running` and keep the clean result
        // (#1194); this pins the primitive they build on.
        let clean = TransitionTask::new();
        clean.complete(None);
        assert!(
            block_on(join_task(&clean)).is_ok(),
            "a clean completion was not reported as success"
        );
        clean.complete(Some(failed("closed")));
        assert!(
            block_on(join_task(&clean)).is_err(),
            "the late completion did not overwrite the clean result"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 15: the live generation a restart publishes (#980, #622 item 3)

/// A listener entry of the model's fiber in `generation`: the kernel's own
/// [`Entry`], owned by the fiber, so `Entry::live` answers from the owner's
/// published generation.
fn generation_entry(ctx: &Ctx, fiber: &Arc<FiberInner>, generation: u64) -> Entry {
    let mut entry = model_entry(ctx, fiber);
    entry.generation = generation;
    entry
}

/// `FiberInner::load` for the live-generation model, mirrored where the driver
/// is: the terminal check, the new token, the generation bump and `Loading`
/// under the transition lock, then the generation's `apply` — a listener
/// registered through the kernel's `Ctx::on_sync`, so its `Entry` carries this
/// generation — and `Active` under the lock again. Dependencies, the factory,
/// `flush_status`'s emits and the pre-cancel are not modelled: none of them
/// publishes the live generation.
fn live_load(fiber: &Arc<FiberInner>) -> u64 {
    let generation = {
        let mut tr = fiber.transition.lock().unwrap();
        assert!(
            tr.terminal_task.is_none(),
            "the live-generation model registers no dispose"
        );
        fiber.new_generation_token();
        tr.pre_cancelled = false;
        tr.generation += 1;
        tr.error = None;
        fiber.set_state(&mut tr, FiberState::Loading);
        tr.generation
    };
    let ctx = fiber.ctx.for_generation(generation);
    assert!(
        ctx.on_sync::<ModelEvent>(|_, _| Ok(None)).is_ok(),
        "the generation's listener was refused"
    );
    {
        let mut tr = fiber.transition.lock().unwrap();
        fiber.set_state(&mut tr, FiberState::Active);
    }
    generation
}

/// `FiberInner::unload` for the live-generation model, mirrored: `Unloading`
/// under the admission and transition locks — which publishes `NOT_LIVE` before
/// anything waits — the cancel, the kernel's own bounded `wait_events`, the
/// cleanup pass, then `Pending` (cleanup errors to the sink, as a non-terminal
/// unload does).
fn live_unload(fiber: &Arc<FiberInner>) {
    {
        let _admission = fiber.ctx.shared().admission.lock().unwrap();
        let mut tr = fiber.transition.lock().unwrap();
        fiber.set_state(&mut tr, FiberState::Unloading);
    }
    fiber.cancel_current();
    block_on(fiber.wait_events());
    if let Some(error) = aggregate_arcs(drain_model_effects(fiber)) {
        (fiber.ctx.error_sink())(error);
    }
    let mut tr = fiber.transition.lock().unwrap();
    fiber.set_state(&mut tr, FiberState::Pending);
}

/// Model 15, the generation whose listeners are live across restarts (#980;
/// #622 item 3: a listener's owner publishes the live generation, and an event
/// call reads it instead of the snapshot). A fiber that unloads and loads again
/// must stop admitting the old generation's listeners the moment the unload
/// publishes `Unloading` (`NOT_LIVE`), and admit the new generation's only once
/// the load publishes it — `live_generation` is written under the transition
/// lock by `set_state`'s `publish_live`, in the same critical section as the
/// state and generation a reader takes that lock to see. A listener of an old
/// generation must never start a call after the restart, however the schedule
/// interleaves the readers, the dispatchers and the driver.
///
/// The model runs the kernel's `set_state`/`publish_live`, `Entry::live`/
/// `Entry::start`/`CallFlight`, `new_generation_token`, `cancel_current`,
/// `wait_events` and `drain_effects`'s records (`drain_model_effects`, the
/// mirror models 5, 12 and 13 use). `load` and `unload` are mirrored where the
/// driver is (the real ones post to a driver task on a runtime a shuttle model
/// cannot pump); the generation bump and the state changes they make are the
/// steps that publish the generation.
///
/// Mutations this model catches (each one made in the kernel and reverted;
/// the message after it is what this model then reports): `publish_live`
/// answering `tr.generation` for `Unloading`/`Pending`/`Disposed` (`a reader
/// takes the transition lock between two transitions and sees a live generation
/// that is not running: live_generation is 0 while the fiber is Unloading at
/// generation 0`), and `Entry::live` not comparing the generation (`the listener
/// of generation 1 is still live after the restart`).
#[test]
fn shuttle_restart_publishes_the_live_generation() {
    const RESTARTS: u64 = 3;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let handle = runtime.handle().clone();
    explore(move || {
        let ctx = Ctx::root_with(handle.clone());
        let fiber = ctx.root_view().expect("the root view").inner.clone();
        // One entry per generation the model's loads start, owned by the root.
        let entries: Vec<Arc<Entry>> = (1..=RESTARTS)
            .map(|generation| Arc::new(generation_entry(&ctx, &fiber, generation)))
            .collect();
        // The root starts `Active` at generation 0: the invariant holds before
        // the first load, too.
        let done = Arc::new(SyncBool::new(false));

        let readers: Vec<_> = (0..2)
            .map(|_| {
                let (fiber, done) = (fiber.clone(), done.clone());
                thread::spawn(move || {
                    loop {
                        {
                            // The lock a `set_state` takes: the state, its
                            // generation and the published generation are one
                            // snapshot, never a mix of two generations.
                            let tr = fiber.transition.lock().unwrap();
                            let live = fiber.live_generation.load(Ordering::SeqCst);
                            let expected = match tr.state {
                                FiberState::Loading | FiberState::Active => tr.generation,
                                _ => NOT_LIVE,
                            };
                            assert_eq!(
                                live, expected,
                                "live_generation is {live} while the fiber is {:?} at \
                                 generation {}",
                                tr.state, tr.generation
                            );
                        }
                        if done.load(Ordering::SeqCst) {
                            return;
                        }
                        switch_point();
                    }
                })
            })
            .collect();

        // Dispatchers hold a listener of every generation: the old ones must
        // stop being admitted as the restart unloads, the new one must start
        // admitting once the load publishes it.
        let dispatchers: Vec<_> = (0..2)
            .map(|_| {
                let (done, entries) = (done.clone(), entries.clone());
                thread::spawn(move || {
                    let mut admitted = 0usize;
                    loop {
                        for entry in &entries {
                            if let Some(flight) = entry.start() {
                                admitted += 1;
                                // The call body: user code, which yields while
                                // the unload waits for it.
                                switch_point();
                                drop(flight);
                            }
                        }
                        if done.load(Ordering::SeqCst) {
                            return admitted;
                        }
                        switch_point();
                    }
                })
            })
            .collect();

        let driver = {
            let fiber = fiber.clone();
            thread::spawn(move || {
                for _ in 0..RESTARTS {
                    switch_point();
                    live_unload(&fiber);
                    switch_point();
                    live_load(&fiber);
                }
            })
        };

        driver.join().unwrap();
        done.store(true, Ordering::SeqCst);
        for reader in readers {
            reader.join().unwrap();
        }
        for dispatcher in dispatchers {
            dispatcher.join().unwrap();
        }

        let (state, generation) = {
            let tr = fiber.transition.lock().unwrap();
            (tr.state, tr.generation)
        };
        assert_eq!(
            (state, generation),
            (FiberState::Active, RESTARTS),
            "the restarts did not end on the last generation"
        );
        assert_eq!(
            fiber.live_generation.load(Ordering::SeqCst),
            RESTARTS,
            "the last generation is not the published one"
        );
        assert_eq!(
            *fiber.event_flights.lock().unwrap(),
            0,
            "a listener call was never finished"
        );
        assert!(
            entries[RESTARTS as usize - 1].live(),
            "the last generation's listener is not live"
        );
        let last = entries[RESTARTS as usize - 1]
            .start()
            .expect("the last generation's listener was refused");
        drop(last);
        for (index, entry) in entries[..RESTARTS as usize - 1].iter().enumerate() {
            let generation = index as u64 + 1;
            assert!(
                !entry.live(),
                "the listener of generation {generation} is still live after the restart"
            );
            assert!(
                entry.start().is_none(),
                "a listener of generation {generation} was admitted after {RESTARTS} restarts"
            );
        }
        // The unloaded generations' records are drained; the last one is still
        // registered, so its cleanup is not due.
        assert!(
            !fiber.effects.lock().unwrap().is_empty(),
            "the last generation's effect record is missing"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 16: a close over a real fiber tree, while its listeners dispatch
// (#622; the kernel side of removing a row on a reload, `events/lifecycle`)

/// A plugin the tree model only needs for its fiber's name: the model runs the
/// members' generations itself, so `apply` never runs.
struct Named(&'static str);

impl crate::DynPlugin for Named {
    fn name(&self) -> &str {
        self.0
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> crate::BoxFuture<'a, Result<Effect, crate::Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

/// `FiberInner::load` for the tree model, mirrored where the driver is: the
/// generation bump, the new token and `Loading` → `Active` under the transition
/// lock — the same critical section that publishes the live generation. The
/// plugin's `apply`, the dependencies, the factory and `flush_status`'s emits
/// are not modelled: none of them decides whether a listener is admitted.
fn tree_activate(fiber: &Arc<FiberInner>) -> u64 {
    let _admission = fiber.ctx.shared().admission.lock().unwrap();
    let mut tr = fiber.transition.lock().unwrap();
    fiber.new_generation_token();
    tr.generation += 1;
    tr.error = None;
    fiber.set_state(&mut tr, FiberState::Loading);
    fiber.set_state(&mut tr, FiberState::Active);
    tr.generation
}

/// A listener entry owned by `fiber` in `generation`: the kernel's own
/// [`Entry`], so `live()` and `start()` read the owner's markers and published
/// generation.
fn tree_entry(fiber: &Arc<FiberInner>, generation: u64) -> Arc<Entry> {
    let mut entry = model_entry(&fiber.ctx, fiber);
    entry.generation = generation;
    Arc::new(entry)
}

/// `FiberInner::unload` for one member of the closing tree, mirrored where the
/// driver is: `Unloading` under the admission and transition locks (which
/// publishes `NOT_LIVE`), the cancel, the kernel's own `wait_events`, then
/// `Disposed`. The member registers no cleanup, so the cleanup pass is empty.
fn tree_unload(fiber: &Arc<FiberInner>) {
    {
        let _admission = fiber.ctx.shared().admission.lock().unwrap();
        let mut tr = fiber.transition.lock().unwrap();
        fiber.set_state(&mut tr, FiberState::Unloading);
    }
    fiber.cancel_current();
    block_on(fiber.wait_events());
    let mut tr = fiber.transition.lock().unwrap();
    fiber.set_state(&mut tr, FiberState::Disposed);
}

/// The window [`mark_subtree_lineage`]'s two-phase publication must never
/// leave open, read over a real fiber: no node may have its own `closing` flag
/// (or an ancestor's `closing`/marker) set while its own marker is clear.
fn tree_close_window(fiber: &Arc<FiberInner>) -> Option<String> {
    let own_closing = fiber.closing.load(Ordering::SeqCst);
    let mut ancestor_closed = false;
    let mut current = fiber.parent_fiber.as_ref().and_then(Weak::upgrade);
    while let Some(parent) = current {
        if parent.closing.load(Ordering::SeqCst) || parent.lineage_closed.load(Ordering::SeqCst) {
            ancestor_closed = true;
            break;
        }
        current = parent.parent_fiber.as_ref().and_then(Weak::upgrade);
    }
    if (own_closing || ancestor_closed) && !fiber.lineage_closed.load(Ordering::SeqCst) {
        return Some(if own_closing {
            format!(
                "the {} is closing while its lineage marker is clear",
                fiber.name
            )
        } else {
            format!(
                "an ancestor of the {} is closed while its marker is clear",
                fiber.name
            )
        });
    }
    None
}

/// Model 16, a subtree close over a real fiber tree while its listeners are
/// dispatching (#622; the kernel side of what a host reload does when it
/// removes a row): the fixed set is collected from the kernel's own
/// `FiberInner::children` ([`CloseTree`]), every member is marked before any
/// `closing` flag is published, a member that unloads waits for the calls it
/// admitted, and after the close a member admits no new call — its marker and
/// `closing` flag alone say so for the grandchild, which never unloads. A
/// sibling outside the closed subtree keeps admitting calls: the collection
/// must not reach past the root it was given. The subtree is three deep, so a
/// walk that stops at the direct children leaves the great-grandchild open.
///
/// The model runs the kernel's own `mark_subtree_lineage` /
/// `publish_subtree_closing` over real [`FiberInner`]s (model 4's tree is a
/// stand-in with a hand-written `close_children`, so the walk over the real
/// child lists was not covered), real [`Entry`]s owned by the members (`live`,
/// `start`, `CallFlight` → `begin_event`/`finish_event`), real
/// `set_state`/`publish_live`, `new_generation_token`, `cancel_current` and
/// `wait_events`. The members' drivers are written out: `tree_activate` stands
/// in for `load`, `tree_unload` for the unload `drive`'s Shutdown arm runs, and
/// the dispatcher's body is the model's (a listener body is user code).
///
/// Mutations this model catches (each made in the kernel and reverted; the
/// message after it is what this model then reports): `collect_postorder` not
/// recursing, so the walk stops at the direct children (`an ancestor of the
/// great-grandchild is closed while its marker is clear` from the monitor; the
/// marker pass left it unmarked), publishing `closing` before the marker pass
/// (`the grandchild is closing while its lineage marker is clear`), `Entry::live`
/// keeping only the published-generation check, with no `alive`, `closing` or
/// `lineage_closed` read (`the grandchild's listener was admitted after the
/// close`), and `wait_events` returning without waiting (`the child's unload
/// returned while 1 call(s) of it were running`).
#[test]
fn shuttle_subtree_close_marks_every_member_of_a_real_tree() {
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let ctx = Ctx::root_with(runtime.handle().clone());
        // The closed subtree is `child` → `grandchild` → `great`; `sibling` is
        // outside it.
        let tree = |name, parent: Option<&Ctx>| {
            spawn_fiber(
                ctx.shared(),
                parent,
                Some(Arc::new(Named(name)) as Arc<dyn crate::DynPlugin>),
                false,
            )
        };
        let child = tree("child", Some(&ctx));
        let grandchild = tree("grandchild", Some(&child.ctx));
        let great = tree("great-grandchild", Some(&grandchild.ctx));
        let sibling = tree("sibling", Some(&ctx));
        let entries = [
            tree_entry(&child, tree_activate(&child)),
            tree_entry(&grandchild, tree_activate(&grandchild)),
            tree_entry(&sibling, tree_activate(&sibling)),
        ];
        tree_activate(&great);
        // Bodies running per entry. Shuttle atomics: the dispatchers only
        // read them after their joins, where a load is not a scheduling point
        // that matters.
        let running: Arc<Vec<ShuttleUsize>> = Arc::new(vec![
            ShuttleUsize::new(0),
            ShuttleUsize::new(0),
            ShuttleUsize::new(0),
        ]);

        let dispatchers: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let (entry, running) = (entry.clone(), running.clone());
                thread::spawn(move || {
                    for _ in 0..2 {
                        switch_point();
                        let Some(flight) = entry.start() else {
                            continue;
                        };
                        // The listener body, while the flight is held.
                        running[index].fetch_add(1, Ordering::SeqCst);
                        switch_point();
                        running[index].fetch_sub(1, Ordering::SeqCst);
                        drop(flight);
                    }
                })
            })
            .collect();

        let done = Arc::new(SyncBool::new(false));
        let monitor = {
            let members = vec![child.clone(), grandchild.clone(), great.clone()];
            let done = done.clone();
            thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    for member in &members {
                        if let Some(window) = tree_close_window(member) {
                            panic!("{window}");
                        }
                    }
                    switch_point();
                }
            })
        };

        let closer = {
            let (child, done) = (child.clone(), done.clone());
            thread::spawn(move || {
                switch_point();
                let subtree = mark_subtree_lineage(&child);
                // The publication point the kernel's admission lock leaves
                // between the marker pass and the flags.
                switch_point();
                publish_subtree_closing(&subtree);
                done.store(true, Ordering::SeqCst);
            })
        };
        let unloader = {
            let (child, running) = (child.clone(), running.clone());
            thread::spawn(move || {
                switch_point();
                tree_unload(&child);
                let still = running[0].load(Ordering::SeqCst);
                assert_eq!(
                    still, 0,
                    "the child's unload returned while {still} call(s) of it were running"
                );
            })
        };

        closer.join().unwrap();
        unloader.join().unwrap();
        monitor.join().unwrap();
        for dispatcher in dispatchers {
            dispatcher.join().unwrap();
        }

        for (name, member) in [
            ("child", &child),
            ("grandchild", &grandchild),
            ("great-grandchild", &great),
        ] {
            assert!(
                member.lineage_closed.load(Ordering::SeqCst),
                "the {name} of the closed subtree was not marked"
            );
            assert!(
                member.closing.load(Ordering::SeqCst),
                "the {name} of the closed subtree was not closed"
            );
            assert_eq!(
                *member.event_flights.lock().unwrap(),
                0,
                "the {name} left a call flight behind"
            );
        }
        assert_eq!(
            child.state(),
            FiberState::Disposed,
            "the member that unloaded did not end Disposed"
        );
        assert!(
            entries[0].start().is_none(),
            "the unloaded member's listener was admitted after its unload"
        );
        assert_eq!(
            grandchild.state(),
            FiberState::Active,
            "the grandchild is the member that never unloads"
        );
        // Its generation is still the live one; only the close's marker and
        // flag keep its listener out.
        assert!(
            entries[1].start().is_none(),
            "the grandchild's listener was admitted after the close"
        );

        assert!(
            !sibling.lineage_closed.load(Ordering::SeqCst),
            "the sibling outside the closed subtree was marked"
        );
        assert!(
            !sibling.closing.load(Ordering::SeqCst),
            "the sibling outside the closed subtree was closed"
        );
        assert_eq!(
            sibling.state(),
            FiberState::Active,
            "the sibling outside the closed subtree changed state"
        );
        assert!(
            entries[2].live(),
            "the sibling outside the closed subtree stopped being live"
        );
        assert!(
            entries[2].start().is_some(),
            "the sibling outside the closed subtree stopped admitting calls"
        );
        assert_eq!(
            running[2].load(Ordering::SeqCst),
            0,
            "the sibling left a call body running"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 17: the root's close and the child it owns (#622, "根关闭与根
// dispose", "子树关闭", `core/features.shutdown-state` a,
// `core/features.errors`)

/// What a `restart()` registration did: refused at its checks, or counted and
/// posted to the driver. A caller that drops the future waits for neither.
enum Restart {
    Refused(Arc<ErrorKind>),
    /// The posted task, and whether `post_join` sent it (`false`: the driver
    /// had already stopped, and the task is complete).
    Posted(Arc<TransitionTask>, bool),
}

/// `Shared`'s close state: what `CtxHost::shutdown`, `Ctx::is_shutting_down`
/// and a root dispose share.
struct CloseShared {
    /// `Shared.admission`. A **std** mutex in the kernel; shuttle's here (as in
    /// models 7 and 8), so the steps the kernel takes as separate calls can be
    /// interleaved without parking a thread in a real mutex.
    admission: SyncMutex<()>,
    /// `Shared.closing`: what `CtxHost::is_shutting_down()` reads.
    closing: SyncBool,
    /// `Shared.shutdown_task`: the one task every root shutdown caller gets.
    shutdown_task: SyncMutex<Option<Arc<TransitionTask>>>,
}

/// The fields of `Trans` the close paths touch.
struct NodeTrans {
    state: FiberState,
    generation: u64,
    terminal_task: Option<Arc<TransitionTask>>,
    pending_restarts: usize,
    pre_cancelled: bool,
    error: Option<Arc<ErrorKind>>,
}

/// One member of the model's tree. The root and the child it mounted are the
/// same type, so the kernel's own passes work on a real tree: the marker and
/// closing passes through [`CloseTree`], and the recursion over `children`
/// that `begin_subtree_shutdown_locked` does.
struct CloseNode {
    is_root: bool,
    shared: Arc<CloseShared>,
    children: SyncMutex<Vec<Arc<CloseNode>>>,
    lineage_closed: SyncBool,
    closing: SyncBool,
    alive: SyncBool,
    transition: SyncMutex<NodeTrans>,
    /// `FiberInner::shutdown_task`: the task every caller of this fiber's
    /// `shutdown()` gets (children; the root's is `shared.shutdown_task`).
    shutdown_task: SyncMutex<Option<Arc<TransitionTask>>>,
    /// `FiberInner::shutdown_inner`: the task `begin_subtree_shutdown_locked`
    /// records, and what the parent's mount cleanup joins. Joining the public
    /// result instead would wait for that mount to detach while the mount
    /// waits for this cleanup.
    shutdown_inner: SyncMutex<Option<Arc<TransitionTask>>>,
    /// `FiberInner::mount`: the record in the parent's effects that owns this
    /// child. `None` for the root.
    mount: SyncMutex<Option<Arc<EffectRecord>>>,
    intents: mpsc::UnboundedSender<Intent>,
    effects: SyncMutex<Vec<Arc<EffectRecord>>>,
    /// Where a non-terminal unload's cleanup errors go: the kernel's ErrorSink.
    sink_errors: Arc<AtomicUsize>,
    /// Std counters: read right after a join, where a shuttle load would be a
    /// scheduling point and could run a driver inside the check.
    cleanups: Arc<AtomicUsize>,
    dispose_posts: AtomicUsize,
    shutdown_posts: AtomicUsize,
    restart_posts: AtomicUsize,
}

impl CloseTree for CloseNode {
    fn close_children(&self) -> Vec<Arc<Self>> {
        self.children.lock().unwrap().clone()
    }

    fn mark_lineage(&self) {
        self.lineage_closed.store(true, Ordering::SeqCst);
    }

    fn publish_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
        // `FiberInner::publish_closing` pre-cancels the running generation.
        self.transition.lock().unwrap().pre_cancelled = true;
    }
}

fn set_node_state(tr: &mut NodeTrans, new: FiberState) {
    // `set_state`: only a change is a state change (the watch publication is
    // not part of this model).
    if tr.state != new {
        tr.state = new;
    }
}

fn node_state(node: &CloseNode) -> FiberState {
    node.transition.lock().unwrap().state
}

fn node_stopped_error(node: &CloseNode) -> Option<Arc<ErrorKind>> {
    node.transition.lock().unwrap().error.clone().or_else(|| {
        node.shared
            .closing
            .load(Ordering::SeqCst)
            .then(|| Arc::new(ErrorKind::Closed))
    })
}

/// `run_cleanups` for the model: the record's cleanups are run here, on the
/// claimer's thread. The kernel `handle.spawn`s an async cleanup on the
/// runtime, which the model never drives, so awaiting it here is the same
/// order without a task in between. Panics go where the kernel sends them.
fn run_cleanups_here(cleanups: &mut Vec<Cleanup>) -> Option<Arc<ErrorKind>> {
    let mut errors = Vec::new();
    while let Some(cleanup) = cleanups.pop() {
        let result = match cleanup {
            Cleanup::Sync(run) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                .unwrap_or_else(|panic| Err(crate::error::panic_error(panic))),
            Cleanup::Async(run) => {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
                    Ok(future) => block_on(future),
                    Err(panic) => Err(crate::error::panic_error(panic)),
                }
            }
        };
        if let Err(error) = result {
            errors.push(error);
        }
    }
    aggregate_arcs(errors.into_iter().map(Error::shared).collect())
}

/// `EffectRecord::drain` for the model: claim, run, finish, then join — the
/// claimer runs the cleanups, and every other drain of the same record joins
/// its result.
fn drain_record(record: &Arc<EffectRecord>) -> Result<(), Arc<ErrorKind>> {
    if let Some(mut cleanups) = record.claim() {
        let error = run_cleanups_here(&mut cleanups);
        record.finish(error);
    }
    block_on(record.join())
}

/// A node with one effect of its own (its `apply`'s cleanup), and the mailbox
/// its driver reads.
fn new_close_node(
    shared: &Arc<CloseShared>,
    is_root: bool,
    sink_errors: &Arc<AtomicUsize>,
) -> (Arc<CloseNode>, mpsc::UnboundedReceiver<Intent>) {
    let (intents, mailbox) = mpsc::unbounded_channel();
    let cleanups = Arc::new(AtomicUsize::new(0));
    let record = EffectRecord::from(
        Effect::Disposer(Box::new({
            let cleanups = cleanups.clone();
            move || {
                cleanups.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })),
        "model node effect".into(),
        Origin::Other,
        Weak::new(),
    );
    let node = Arc::new(CloseNode {
        is_root,
        shared: shared.clone(),
        children: SyncMutex::new(Vec::new()),
        lineage_closed: SyncBool::new(false),
        closing: SyncBool::new(false),
        alive: SyncBool::new(true),
        transition: SyncMutex::new(NodeTrans {
            state: FiberState::Active,
            generation: 0,
            terminal_task: None,
            pending_restarts: 0,
            pre_cancelled: false,
            error: None,
        }),
        shutdown_task: SyncMutex::new(None),
        shutdown_inner: SyncMutex::new(None),
        mount: SyncMutex::new(None),
        intents,
        effects: SyncMutex::new(vec![record]),
        sink_errors: sink_errors.clone(),
        cleanups,
        dispose_posts: AtomicUsize::new(0),
        shutdown_posts: AtomicUsize::new(0),
        restart_posts: AtomicUsize::new(0),
    });
    (node, mailbox)
}

/// `Ctx::plugin`'s mount effect: what the parent's unload runs for the child it
/// mounted. It joins the child's own cleanup when a subtree shutdown recorded
/// one, and disposes the child otherwise; an error of that dispose goes to the
/// ErrorSink (`sink_errors`), not to the parent's cleanup result, because the
/// dispose's own caller already reports it.
fn mount_record(
    child: &Arc<CloseNode>,
    runs: &Arc<AtomicUsize>,
    sink_errors: &Arc<AtomicUsize>,
) -> Arc<EffectRecord> {
    let (child, runs, sink_errors) = (child.clone(), runs.clone(), sink_errors.clone());
    EffectRecord::from(
        Effect::AsyncDisposer(Box::new(move || {
            let (child, runs, sink_errors) = (child.clone(), runs.clone(), sink_errors.clone());
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                let closing = {
                    let _admission = child.shared.admission.lock().unwrap();
                    child.shutdown_inner.lock().unwrap().clone()
                };
                if let Some(task) = closing {
                    // The child's own cleanup, not its public shutdown result:
                    // the latter waits for this mount to detach.
                    let _ = join_task(&task).await;
                    return Ok(());
                }
                let delivered = node_state(&child) == FiberState::Disposed;
                if join_task(&node_dispose(&child)).await.is_err() && !delivered {
                    sink_errors.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            })
        })),
        "plugin mount: model child".into(),
        Origin::Mount,
        Weak::new(),
    )
}

/// `FiberInner::post` for the model's node, counting the intents a driver will
/// see.
fn post_node(node: &Arc<CloseNode>, intent: Intent) -> bool {
    if !node.alive.load(Ordering::SeqCst) {
        complete_intent(&intent, node_stopped_error(node));
        return false;
    }
    let counter = match &intent {
        Intent::Dispose => Some(&node.dispose_posts),
        Intent::Shutdown(_) => Some(&node.shutdown_posts),
        _ => None,
    };
    match node.intents.send(intent) {
        Ok(()) => {
            if let Some(counter) = counter {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            true
        }
        Err(error) => {
            complete_intent(&error.0, node_stopped_error(node));
            false
        }
    }
}

/// `register_dispose` for a node: one terminal task, one `Intent::Dispose`.
fn node_dispose(node: &Arc<CloseNode>) -> Arc<TransitionTask> {
    let (task, newly_registered) = {
        let mut tr = node.transition.lock().unwrap();
        match &tr.terminal_task {
            Some(task) => (task.clone(), false),
            None => {
                tr.pre_cancelled = true;
                let task = TransitionTask::new();
                tr.terminal_task = Some(task.clone());
                (task, true)
            }
        }
    };
    if newly_registered {
        post_node(node, Intent::Dispose);
        if !node.alive.load(Ordering::SeqCst) {
            task.complete_if_running(node_stopped_error(node));
        }
    }
    task
}

/// `CtxHost::shutdown`, mirrored: under the admission lock one shared task, the
/// marker pass over the real tree, `Shared.closing` (so `is_shutting_down()` is
/// true as soon as the call is made), the existing `closing` flags, then one
/// `Intent::Shutdown` for the root. The mounted children are shut down by the
/// root's unload draining their mount records, not by a second post here.
fn root_close(root: &Arc<CloseNode>) -> Arc<TransitionTask> {
    let _admission = root.shared.admission.lock().unwrap();
    let mut slot = root.shared.shutdown_task.lock().unwrap();
    if let Some(task) = slot.as_ref() {
        return task.clone();
    }
    let task = TransitionTask::new();
    *slot = Some(task.clone());
    let subtree = mark_subtree_lineage(root);
    root.shared.closing.store(true, Ordering::SeqCst);
    publish_subtree_closing(&subtree);
    post_node(root, Intent::Shutdown(task.clone()));
    task
}

/// `FiberView::dispose`'s root branch, mirrored: deciding and registering share
/// the admission lock, so a dispose that sees the close joins the shutdown's
/// task instead of registering behind it.
fn tree_root_dispose(root: &Arc<CloseNode>) -> (Arc<TransitionTask>, bool) {
    let _admission = root.shared.admission.lock().unwrap();
    if root.shared.closing.load(Ordering::SeqCst) {
        let task = root
            .shared
            .shutdown_task
            .lock()
            .unwrap()
            .clone()
            .expect("shutdown stores its task before closing admission");
        return (task, true);
    }
    // A scheduling point inside the admission critical section: the kernel's
    // thread can be preempted here, and every other root-close caller then
    // waits for admission. A restart does not take it, so it may run.
    switch_point();
    (node_dispose(root), false)
}

/// `FiberView::shutdown`'s non-root branch (#1025): under the admission lock,
/// the marker pass over the subtree, the existing `closing` flags, then the
/// per-fiber tasks.
fn subtree_shutdown(node: &Arc<CloseNode>) -> Arc<TransitionTask> {
    let _admission = node.shared.admission.lock().unwrap();
    let subtree = mark_subtree_lineage(node);
    publish_subtree_closing(&subtree);
    begin_node_shutdown(node)
}

/// `begin_subtree_shutdown_locked`, mirrored: one public task per fiber, one
/// recursive call per child, one `Intent::Shutdown` per driver, and the joiner
/// that completes the public task: its own task first, then the child tasks, so
/// a caller returns only after the whole subtree cleaned up. The kernel's
/// joiner also awaits the driver handle (a panic collector), and
/// `release_transient` there detaches the mount; the model drains the node's
/// mount record here instead, which is the detach the public result waits for.
fn begin_node_shutdown(node: &Arc<CloseNode>) -> Arc<TransitionTask> {
    let public = {
        let mut slot = node.shutdown_task.lock().unwrap();
        if let Some(task) = slot.as_ref() {
            return task.clone();
        }
        let task = TransitionTask::new();
        *slot = Some(task.clone());
        task
    };

    let children: Vec<Arc<CloseNode>> = node.children.lock().unwrap().clone();
    let child_tasks: Vec<Arc<TransitionTask>> = children.iter().map(begin_node_shutdown).collect();

    let (inner, needs_post) = {
        let mut tr = node.transition.lock().unwrap();
        match tr.terminal_task.as_ref() {
            Some(task) => (task.clone(), false),
            None => {
                let task = TransitionTask::new();
                tr.terminal_task = Some(task.clone());
                (task, true)
            }
        }
    };
    *node.shutdown_inner.lock().unwrap() = Some(inner.clone());
    if needs_post {
        post_node(node, Intent::Shutdown(inner.clone()));
    }
    let result = public.clone();
    let owner = node.clone();
    thread::spawn(move || {
        let mut errors = Vec::new();
        if let Err(error) = block_on(join_task(&inner)) {
            errors.push(error);
        }
        for child in child_tasks {
            if let Err(error) = block_on(join_task(&child)) {
                errors.push(error);
            }
        }
        let mount = owner.mount.lock().unwrap().clone();
        if let Some(mount) = mount {
            if let Err(error) = drain_record(&mount) {
                errors.push(error);
            }
        }
        drop(owner);
        result.complete(aggregate_arcs(errors));
    });
    public
}

/// `FiberView::restart`, mirrored: the closed checks and the pre-cancel share
/// the transition lock with the count, so a generation `load` starts after the
/// registration starts pre-cancelled too, then `post_join`.
fn node_restart(node: &Arc<CloseNode>) -> Restart {
    {
        let mut tr = node.transition.lock().unwrap();
        if node.shared.closing.load(Ordering::SeqCst) || node.closing.load(Ordering::SeqCst) {
            return Restart::Refused(Arc::new(ErrorKind::Closed));
        }
        if !node.is_root && (tr.terminal_task.is_some() || !node.alive.load(Ordering::SeqCst)) {
            return Restart::Refused(Arc::new(ErrorKind::InactiveEffect));
        }
        tr.pending_restarts += 1;
        tr.pre_cancelled = true;
    }
    let task = TransitionTask::new();
    let sent = post_join_to(
        &node.alive,
        &node.intents,
        task.clone(),
        Intent::Restart,
        || node_stopped_error(node),
    );
    if sent {
        node.restart_posts.fetch_add(1, Ordering::SeqCst);
    }
    Restart::Posted(task, sent)
}

/// What `unload` moves to: `NextState` from the kernel.
enum NodeNext {
    Pending,
    Disposed,
}

/// `drain_effects`: take the list, drain each record in reverse order (LIFO),
/// and return what their joins reported.
fn drain_node_effects(node: &Arc<CloseNode>) -> Option<Arc<ErrorKind>> {
    let effects: Vec<Arc<EffectRecord>> = std::mem::take(&mut *node.effects.lock().unwrap());
    let mut errors = Vec::new();
    for record in effects.into_iter().rev() {
        if let Err(error) = drain_record(&record) {
            errors.push(error);
        }
    }
    aggregate_arcs(errors)
}

/// `FiberInner::unload` for a node: `Unloading` under the admission and
/// transition locks, the (empty) event wait, the cleanup pass, then the next
/// state. A non-terminal unload reports its cleanup errors to the sink.
fn unload_node(node: &Arc<CloseNode>, next: NodeNext) {
    {
        let _admission = node.shared.admission.lock().unwrap();
        let mut tr = node.transition.lock().unwrap();
        set_node_state(&mut tr, FiberState::Unloading);
    }
    let error = drain_node_effects(node);
    let mut tr = node.transition.lock().unwrap();
    match next {
        NodeNext::Pending => {
            if error.is_some() {
                node.sink_errors.fetch_add(1, Ordering::SeqCst);
            }
            set_node_state(&mut tr, FiberState::Pending);
        }
        NodeNext::Disposed => {
            tr.error = error;
            set_node_state(&mut tr, FiberState::Disposed);
        }
    }
}

/// `release_transient`: the mount record this node holds in its parent's
/// effects is drained, so the mount detaches. The model has no inject index.
fn release_transient_node(node: &Arc<CloseNode>) {
    let mount = node.mount.lock().unwrap().clone();
    if let Some(mount) = mount {
        let _ = drain_record(&mount);
    }
}

/// The node's driver, mirroring `drive`'s `Restart`, `Dispose` and `Shutdown`
/// arms, the terminal-dispose exit of a non-root fiber, and the shutdown exit.
/// A non-root driver completes its own task before `release_transient` and the
/// terminal task after it; the root's shared task is completed last.
fn drive_node(node: Arc<CloseNode>, mut mailbox: mpsc::UnboundedReceiver<Intent>) {
    let mut disposal = false;
    let mut shutdown = None;
    while let Some(intent) = block_on(mailbox.recv()) {
        match intent {
            Intent::Restart(task) => {
                {
                    let mut tr = node.transition.lock().unwrap();
                    tr.pending_restarts = tr.pending_restarts.saturating_sub(1);
                }
                let closing = node.shared.closing.load(Ordering::SeqCst)
                    || node.closing.load(Ordering::SeqCst);
                if closing {
                    task.complete(Some(Arc::new(ErrorKind::Closed)));
                    continue;
                }
                if node.transition.lock().unwrap().terminal_task.is_some() {
                    // A dispose registered first wins: this restart does not
                    // happen and reports what `restart()` reports behind one.
                    unload_node(&node, NodeNext::Disposed);
                    task.complete(Some(Arc::new(ErrorKind::InactiveEffect)));
                    continue;
                }
                let state = node_state(&node);
                if state == FiberState::Disposed {
                    // Only the root gets here: a non-root fiber is Disposed
                    // only with a terminal task registered.
                    assert!(
                        node.is_root,
                        "a non-root node was Disposed without a dispose"
                    );
                    {
                        let mut tr = node.transition.lock().unwrap();
                        tr.error = None;
                        tr.terminal_task = None;
                        set_node_state(&mut tr, FiberState::Active);
                    }
                    let error = node.transition.lock().unwrap().error.clone();
                    task.complete(error);
                    continue;
                }
                if state != FiberState::Pending {
                    unload_node(&node, NodeNext::Pending);
                }
                // `refresh_deps` returns at its plugin check for the model's
                // nodes: no generation starts, so the restart reports the
                // fiber's error (none here).
                let error = node.transition.lock().unwrap().error.clone();
                task.complete(error);
            }
            Intent::Dispose => {
                if node_state(&node) != FiberState::Disposed {
                    unload_node(&node, NodeNext::Disposed);
                }
                let (terminal, error) = {
                    let tr = node.transition.lock().unwrap();
                    (tr.terminal_task.clone(), tr.error.clone())
                };
                if let Some(terminal) = terminal {
                    terminal.complete(error);
                }
                if !node.is_root {
                    disposal = true;
                    break;
                }
            }
            Intent::Shutdown(task) => {
                if node_state(&node) != FiberState::Disposed {
                    unload_node(&node, NodeNext::Disposed);
                }
                shutdown = Some(task);
                break;
            }
            _ => {}
        }
    }
    if let Some(task) = shutdown {
        node.alive.store(false, Ordering::SeqCst);
        let (terminal, error) = {
            let tr = node.transition.lock().unwrap();
            (tr.terminal_task.clone(), tr.error.clone())
        };
        while let Ok(intent) = mailbox.try_recv() {
            let completion = match &intent {
                Intent::Settle(_) => None,
                Intent::Shutdown(_) => error.clone(),
                _ => Some(Arc::new(ErrorKind::Closed)),
            };
            complete_intent(&intent, completion);
        }
        if !node.is_root {
            // The mount cleanup may join this one. Publish it before
            // `release_transient` drains that same mount record.
            task.complete(error.clone());
            release_transient_node(&node);
        }
        if let Some(terminal) = terminal {
            terminal.complete(error.clone());
        }
        drop(node);
        task.complete(error);
        return;
    }
    if disposal {
        node.alive.store(false, Ordering::SeqCst);
        let terminal_error = node.transition.lock().unwrap().error.clone();
        while let Ok(intent) = mailbox.try_recv() {
            let error = if matches!(intent, Intent::Settle(_)) {
                None
            } else {
                terminal_error.clone()
            };
            complete_intent(&intent, error);
        }
        release_transient_node(&node);
    }
}

/// Model 17, the root's close and the child it owns (#622, "根关闭与根
/// dispose", "子树关闭"): two root shutdown callers and a root dispose race
/// with a restart whose future is dropped at its await, while the root closes
/// the child it mounted. Every join returns: the two shutdown callers share one
/// task, the dispose registered before admission closed joins that task and
/// never reports `Closed`, and each shutdown returns only after the whole
/// subtree it named ran its cleanups — the root's only after its mount record
/// ran and the child cleaned up, exactly once each. Dropping the restart's
/// future stops neither the unload it queued nor the close behind it, and
/// `is_shutting_down()` is true from the shutdown call on.
///
/// This is a reference model. `CtxHost::shutdown`, `FiberView::dispose` and
/// `FiberView::shutdown` take `Shared.admission`, a **std** mutex, and then store
/// shuttle-visible markers; two shuttle threads entering them would park in the
/// std mutex while the first waits at a switch point (models 7 and 8 say the
/// same). The kernel's own passes and types run: `mark_subtree_lineage` /
/// `publish_subtree_closing` through `CloseTree` over a real parent/child tree,
/// `TransitionTask`, `post_join_to`, `join_task`, `complete_intent`, and
/// `EffectRecord::{claim, finish, join}`. Mirrored: the admission-locked
/// sequences, `begin_subtree_shutdown_locked`'s recursion and joiner, the mount
/// effect's cleanup (`ctx.rs`, an `AsyncDisposer`), `unload`/`drain_effects` and
/// `drive`'s arms. The nodes have no plugin, so `refresh_deps` returns at its
/// plugin check and no generation ever starts (`generation` stays 0).
///
/// Mutations this model catches: dropping the admission lock from the root
/// dispose (a dispose that saw `closing == false` registers behind the finished
/// shutdown and joins a task completed with `Closed`), dropping the
/// `shared_shutdown_task` early return (two tasks, so the `Arc::ptr_eq` fails),
/// and completing the root's shared task before its unload (a caller returns
/// before the subtree's cleanups ran). Run: the three fail at the default 1000
/// iterations with `a dispose registered before admission closed got
/// Err(Closed)`, `two shutdown callers got different tasks`, and `a root
/// shutdown returned before its subtree cleaned up`.
#[test]
fn shuttle_root_close_owns_the_child_it_mounted() {
    explore(|| {
        let shared = Arc::new(CloseShared {
            admission: SyncMutex::new(()),
            closing: SyncBool::new(false),
            shutdown_task: SyncMutex::new(None),
        });
        let sink_errors = Arc::new(AtomicUsize::new(0));
        let (root, root_mailbox) = new_close_node(&shared, true, &sink_errors);
        let (child, child_mailbox) = new_close_node(&shared, false, &sink_errors);
        root.children.lock().unwrap().push(child.clone());
        let mount_runs = Arc::new(AtomicUsize::new(0));
        let mount = mount_record(&child, &mount_runs, &sink_errors);
        *child.mount.lock().unwrap() = Some(mount.clone());
        // Registered after the root's own effect, and `drain_effects` runs the
        // list in reverse: the mount drains first, as it does for a host that
        // registered its own effect before mounting the plugin.
        root.effects.lock().unwrap().push(mount);

        let root_driver = {
            let root = root.clone();
            thread::spawn(move || drive_node(root, root_mailbox))
        };
        let child_driver = {
            let child = child.clone();
            thread::spawn(move || drive_node(child, child_mailbox))
        };

        let shutdowns: Vec<_> = (0..2)
            .map(|_| {
                let (root, child) = (root.clone(), child.clone());
                let mount_runs = mount_runs.clone();
                thread::spawn(move || {
                    switch_point();
                    let task = root_close(&root);
                    // `is_shutting_down()` from the call on, before any cleanup
                    // finished.
                    assert!(
                        root.shared.closing.load(Ordering::SeqCst),
                        "is_shutting_down() is false after shutdown() returned"
                    );
                    let result = block_on(join_task(&task));
                    // Right after the join, with no yield in between: the
                    // subtree this close owns has cleaned up. The counters are
                    // std atomics for that reason.
                    let cleanups = (
                        root.cleanups.load(Ordering::SeqCst),
                        child.cleanups.load(Ordering::SeqCst),
                        mount_runs.load(Ordering::SeqCst),
                    );
                    (task, result, cleanups)
                })
            })
            .collect();

        let disposer = {
            let root = root.clone();
            thread::spawn(move || {
                switch_point();
                let (task, joined_shutdown) = tree_root_dispose(&root);
                let result = block_on(join_task(&task));
                (task, joined_shutdown, result)
            })
        };
        // The restart's future is dropped at its await: its registration and
        // post already happened, and neither the unload it queues nor the close
        // behind it waits for a joiner.
        let restarter = {
            let root = root.clone();
            thread::spawn(move || {
                switch_point();
                match node_restart(&root) {
                    Restart::Refused(error) => {
                        assert!(
                            matches!(&*error, ErrorKind::Closed | ErrorKind::InactiveEffect),
                            "a restart in the close reported {error:?}"
                        );
                        None
                    }
                    Restart::Posted(task, _sent) => Some(task),
                }
            })
        };
        let mut shutdown_tasks = Vec::new();
        for (task, result, cleanups) in shutdowns.into_iter().map(|caller| caller.join().unwrap()) {
            assert!(result.is_ok(), "a shutdown returned {result:?}");
            assert_eq!(
                cleanups,
                (1, 1, 1),
                "a root shutdown returned before its subtree cleaned up"
            );
            shutdown_tasks.push(task);
        }
        assert!(
            Arc::ptr_eq(&shutdown_tasks[0], &shutdown_tasks[1]),
            "two shutdown callers got different tasks"
        );

        let (dispose_task, joined_shutdown, dispose_result) = disposer.join().unwrap();
        assert!(
            dispose_result.is_ok(),
            "a dispose registered before admission closed got {dispose_result:?}"
        );
        if joined_shutdown {
            assert!(
                Arc::ptr_eq(&dispose_task, &shutdown_tasks[0]),
                "a dispose that joined the shutdown got another task"
            );
        }
        // A restart whose future was dropped still completes: the driver takes
        // it up before the close, or the close's drain completes it with the
        // terminal result. Neither waits for the joiner.
        if let Some(task) = restarter.join().unwrap() {
            match block_on(join_task(&task)) {
                Ok(()) => {}
                Err(error) => assert!(
                    matches!(&*error, ErrorKind::Closed | ErrorKind::InactiveEffect),
                    "a dropped restart's task reported {error:?}"
                ),
            }
        }

        assert_eq!(
            mount_runs.load(Ordering::SeqCst),
            1,
            "the child's mount record ran more than once"
        );
        // The mount cleanup reports a child dispose error to the sink unless the
        // child was already `Disposed` when it asked, and a successful cleanup
        // reports nothing: `register_dispose` keeps the driver's result instead
        // of replacing it with `Closed` (#1194).
        assert_eq!(
            sink_errors.load(Ordering::SeqCst),
            0,
            "the mount cleanup reported an error to the sink"
        );
        assert_eq!(
            root.shutdown_posts.load(Ordering::SeqCst),
            1,
            "the root's driver was posted more than one shutdown"
        );
        assert!(
            root.dispose_posts.load(Ordering::SeqCst) <= 1,
            "the root's driver was posted more than one dispose"
        );
        assert!(
            root.restart_posts.load(Ordering::SeqCst) <= 1,
            "one restart posted more than one intent"
        );
        assert!(
            child.shutdown_posts.load(Ordering::SeqCst) <= 1,
            "the child's driver was posted more than one shutdown"
        );

        root_driver.join().unwrap();
        child_driver.join().unwrap();

        // Both drivers ended: a fiber's own result may complete inside its
        // driver's unload, before the driver stops taking intents.
        for (name, node) in [("the root", &root), ("the child", &child)] {
            assert!(
                !node.alive.load(Ordering::SeqCst),
                "{name}'s driver is still taking intents"
            );
            assert_eq!(
                node.cleanups.load(Ordering::SeqCst),
                1,
                "{name}'s cleanup did not run exactly once"
            );
            assert_eq!(
                node_state(node),
                FiberState::Disposed,
                "{name} did not end Disposed"
            );
            assert_eq!(
                node.transition.lock().unwrap().generation,
                0,
                "a close started a generation for {name}"
            );
            assert!(
                node.closing.load(Ordering::SeqCst) && node.lineage_closed.load(Ordering::SeqCst),
                "{name} was not marked and closed"
            );
        }

        // The flag stays true, and a restart after the close is refused
        // without posting anything.
        assert!(root.shared.closing.load(Ordering::SeqCst));
        let posts = root.restart_posts.load(Ordering::SeqCst);
        match node_restart(&root) {
            Restart::Refused(error) => assert!(
                matches!(&*error, ErrorKind::Closed),
                "a restart after the close reported {error:?}"
            ),
            Restart::Posted(..) => panic!("a closed root accepted a restart"),
        }
        assert_eq!(root.restart_posts.load(Ordering::SeqCst), posts);
        assert_eq!(root.cleanups.load(Ordering::SeqCst), 1);
    });
}

/// Model 18, one transition's result across its driver's exit (#1194, #622's
/// "待复现的疑点": "`complete` 后写覆盖先写：干净的子 dispose 与根关闭竞争时，晚到的
/// join 拿到 `Err(Closed)`"): a child's dispose and a child subtree shutdown
/// race while the root is closing. `register_dispose` posts the dispose and
/// then looks at `alive`; a driver that finished the dispose in between already
/// completed the terminal task with the cleanup's result. The late check must
/// not complete that task again: `stopped_error()` is `Closed` while
/// `Shared.closing` is set, so the overwrite would turn a successful cleanup
/// into an error for every join that attaches after it and make the mount
/// cleanup report a false error to the sink. The fix completes only while the
/// task is still `Running`; the driver completes before it clears `alive`, so a
/// check made after reading `alive == false` sees `Done` and keeps the result.
#[test]
fn shuttle_dispose_result_survives_the_driver_exit() {
    explore(|| {
        // The root's close is in progress: `Shared.closing` is what turns the
        // late completion's `stopped_error()` into `Closed`.
        let shared = Arc::new(CloseShared {
            admission: SyncMutex::new(()),
            closing: SyncBool::new(true),
            shutdown_task: SyncMutex::new(None),
        });
        let sink_errors = Arc::new(AtomicUsize::new(0));
        let (node, mailbox) = new_close_node(&shared, false, &sink_errors);
        let driver = {
            let node = node.clone();
            thread::spawn(move || drive_node(node, mailbox))
        };

        let disposer = {
            let node = node.clone();
            thread::spawn(move || {
                switch_point();
                block_on(join_task(&node_dispose(&node)))
            })
        };
        let closer = {
            let node = node.clone();
            thread::spawn(move || {
                switch_point();
                let task = subtree_shutdown(&node);
                block_on(join_task(&task))
            })
        };

        let dispose_result = disposer.join().unwrap();
        let close_result = closer.join().unwrap();
        driver.join().unwrap();
        assert_eq!(
            node.cleanups.load(Ordering::SeqCst),
            1,
            "the child's cleanup did not run exactly once"
        );
        assert_eq!(
            node_state(&node),
            FiberState::Disposed,
            "the child did not end Disposed"
        );
        assert!(
            dispose_result.is_ok(),
            "the child's dispose reported {dispose_result:?} although its cleanup ran"
        );
        assert!(
            close_result.is_ok(),
            "the child's close reported {close_result:?} although its cleanup ran"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 19: a restart inside the dependency window (#622, Bug B,
// `core/typed.lost-dependency-restart`, `core/features.errors`)

/// Model 19, a restart inside the dependency window (#622, Bug B / #620): a
/// provider leaves while its consumer is between `Pending` and `Loading`, a
/// restart is registered in that window and its future is dropped at the await,
/// a second restart is joined while the withdrawal runs, and the provider comes
/// back. The consumer must never be `Active` on the withdrawn generation when a
/// withdrawal or a restart returns, must end bound to the returned generation,
/// and every unloaded generation's cleanup must run exactly once. The dropped
/// restart's task still completes: the unload and the reload it queued do not
/// depend on a waiter.
///
/// Reference model for the same reason as model 9: the gate window runs no
/// plugin code and takes std locks only. `load`, `refresh_deps`, `unload`, the
/// registry methods, `evict_and_finalize` and `drive`'s `Restart` arm are
/// mirrored; `TransitionTask`, `post_join_to`, `join_task`, `complete_intent`
/// and `EffectRecord` are the kernel's.
///
/// Mutations this model catches: dropping the `pending_restarts` decrement from
/// the restart's take-up (the count a later `load` reads never clears: `a
/// restart was left counted after the driver took it up`, left 2). Dropping
/// `load`'s second gate resolve or the withdrawal's consumer join loop does not
/// fail here: the restart's own unload to `Pending` takes the consumer out of
/// the window before either can be observed, which is why model 9 pins those
/// two by itself. This model pins what a restart that lands in the window does
/// and leaves behind.
#[test]
fn shuttle_dependency_restart_in_the_window_returns_to_the_returned_generation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let handle = runtime.handle().clone();
    explore(move || {
        let key = TypeKey::of::<u32>();
        let world = Arc::new(DepWorld {
            key: key.clone(),
            registry: DepRegistry::new(),
            admission: SyncMutex::new(()),
            handle: handle.clone(),
        });
        let provider = Arc::new(DepProvider {
            id: PluginId(7),
            state: SyncMutex::new(FiberState::Active),
        });
        provider.set(FiberState::Loading);
        let first = world
            .registry
            .insert(&key, &provider, 1)
            .expect("the first provide");
        provider.set(FiberState::Active);

        let (consumer, mailbox) = new_consumer(&world);
        world.registry.register_inject(&key, &consumer);
        let driver = {
            let (world, inner) = (world.clone(), consumer.inner.clone());
            thread::spawn(move || drive_consumer(world, inner, mailbox))
        };
        // The mount posts the first refresh.
        post_consumer(&consumer, Intent::RefreshDeps);

        let removal = Arc::new(Notify::new());
        // The restart registered in the window, whose future is dropped: its
        // task is kept only to check that the driver completed it anyway.
        let window_restart = Arc::new(SyncMutex::new(None::<Arc<TransitionTask>>));
        let leaver = {
            let (world, provider, first, removal, consumer, window_restart) = (
                world.clone(),
                provider.clone(),
                first.clone(),
                removal.clone(),
                consumer.clone(),
                window_restart.clone(),
            );
            thread::spawn(move || {
                let in_window = || {
                    switch_point();
                    match consumer_restart(&consumer) {
                        Restart::Refused(error) => {
                            panic!("a restart in the window was refused: {error:?}")
                        }
                        Restart::Posted(task, sent) => {
                            assert!(sent, "the consumer's driver was not alive");
                            *window_restart.lock().unwrap() = Some(task);
                        }
                    }
                };
                withdraw(&world, &provider, &first, &removal, &in_window);
                // Bug B: the withdrawal returns only after the consumer
                // stopped or saw the removal.
                let state = consumer.inner.state();
                let stale = {
                    let last = consumer.inner.last_deps.lock().unwrap();
                    last.as_ref().is_some_and(|deps| {
                        deps.contains(&(provider.id, 1, world.key.clone(), None))
                    })
                };
                assert!(
                    !(state == FiberState::Active && stale),
                    "the withdrawal returned while the consumer still ran on the \
                     withdrawn generation"
                );
            })
        };
        let returner = {
            let (world, provider, removal) = (world.clone(), provider.clone(), removal.clone());
            thread::spawn(move || {
                // `notify_one`'s permit is kept when it fires before the wait,
                // so the return never starts before the removal is marked.
                block_on(removal.notified());
                provide(&world, &provider, 2)
            })
        };
        // A restart that waits for its result, racing the whole withdrawal.
        let restarter = {
            let consumer = consumer.clone();
            thread::spawn(move || {
                switch_point();
                match consumer_restart(&consumer) {
                    Restart::Refused(error) => Some(error),
                    Restart::Posted(task, sent) => {
                        assert!(sent, "the consumer's driver was not alive");
                        block_on(join_task(&task)).err()
                    }
                }
            })
        };

        leaver.join().unwrap();
        let returned = returner.join().unwrap();
        assert!(
            restarter.join().unwrap().is_none(),
            "a restart outside a dispose reported an error"
        );

        // Settle after every notification the threads posted: the consumer
        // handles what is queued before the barrier.
        let settle = TransitionTask::new();
        let sent = post_join_to(
            &consumer.inner.alive,
            &consumer.intents,
            settle.clone(),
            Intent::Settle,
            || consumer_stopped_error(&consumer.inner),
        );
        assert!(sent, "the consumer's driver was not alive");
        assert!(block_on(join_task(&settle)).is_ok());

        let inner = consumer.inner.clone();
        let applies = inner.applies.load(Ordering::SeqCst);
        let cleanups = inner.cleanups.load(Ordering::SeqCst);
        assert!(
            applies >= 1,
            "the consumer never loaded, so nothing was evicted"
        );
        assert_eq!(
            inner.state(),
            FiberState::Active,
            "the consumer did not reload on the returned generation"
        );
        // The generation that is still Active has not been unloaded: its
        // record is still in the list and its cleanup is not due. Every
        // earlier generation's cleanup ran exactly once, no matter how often a
        // restart unloaded and reloaded in between.
        assert_eq!(
            cleanups,
            applies - 1,
            "an unloaded generation's cleanup did not run exactly once ({cleanups} \
             cleanups for {applies} applies, one still active)"
        );
        assert_eq!(
            inner.effects.lock().unwrap().len(),
            1,
            "the active generation's record is not the only one left"
        );
        let expected: HashSet<DepQuad> =
            std::iter::once((provider.id, 2, key.clone(), None)).collect();
        assert_eq!(
            inner.last_deps.lock().unwrap().as_ref(),
            Some(&expected),
            "the consumer is not bound to the returned generation"
        );
        assert_eq!(
            inner.transition.lock().unwrap().pending_restarts,
            0,
            "a restart was left counted after the driver took it up"
        );
        let now = world.registry.lookup(&key).expect("the returned binding");
        assert!(
            Arc::ptr_eq(&now, &returned),
            "the withdrawal removed or replaced the new binding"
        );
        assert!(!now.removing.load(Ordering::SeqCst));

        // The dropped restart's task completed: dropping the future stopped
        // neither the queued unload nor the reload behind it.
        if let Some(task) = window_restart.lock().unwrap().take() {
            assert!(
                block_on(join_task(&task)).is_ok(),
                "the window restart's task never completed"
            );
        }

        // Dispose the loaded generation too: its cleanup must run exactly
        // once, and the record list must end empty.
        post_consumer(&consumer, Intent::Dispose);
        let settle = TransitionTask::new();
        let _ = post_join_to(
            &inner.alive,
            &consumer.intents,
            settle.clone(),
            Intent::Settle,
            || consumer_stopped_error(&inner),
        );
        assert!(block_on(join_task(&settle)).is_ok());
        drop(consumer);
        driver.join().unwrap();
        assert_eq!(
            inner.state(),
            FiberState::Disposed,
            "the dispose did not end"
        );
        assert_eq!(
            inner.cleanups.load(Ordering::SeqCst),
            applies,
            "the active generation's cleanup did not run exactly once"
        );
        assert!(
            inner.effects.lock().unwrap().is_empty(),
            "a generation's effect record was left behind"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 20: the spawn stand-in (#622 item 1)

/// Model 20, the spawn stand-in (#622 item 1): under a [`SpawnScope`] the
/// kernel's spawns go to shuttle's executor instead of the host runtime, and
/// [`SpawnScope::run`] is the scheduler that drives them. Two tasks interleave
/// their steps, a third collects two more through a `TaskSet` the way
/// `run_parallel` does from inside the emit chain, and every task runs exactly
/// once. The kernel's handles still join after the scheduler ran their tasks,
/// and `run` waits for the tasks whose handles the kernel dropped (the refused
/// record's drain, an emit's listener call) too. The runtime is never pumped:
/// a spawn that fell back to it would leave its step missing.
///
/// Mutations this model catches: a spawn that goes to the host runtime while a
/// scope is installed (`runs` stays short), a `run` that only waits for the
/// handles the kernel joins (the set's tasks and their collector never finish),
/// and a `join` or `TaskSet::join_next` that loses a result or returns one
/// twice.
#[test]
fn shuttle_spawned_tasks_interleave_and_a_task_set_collects_them() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let handle = runtime.handle().clone();
    explore(move || {
        let scope = SpawnScope::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(SyncMutex::new(Vec::new()));
        let collected = Arc::new(SyncMutex::new(Vec::new()));
        let done = Arc::new(SyncBool::new(false));

        let entered = scope.enter();

        // Two tasks with a switch point between their steps: the other can
        // run in that window, so the two interleave. Each task's own steps
        // stay in order and happen once, whichever way the scheduler goes.
        let mut siblings = Vec::new();
        for who in 0..2u8 {
            let runs = runs.clone();
            let order = order.clone();
            siblings.push(crate::sync::spawn(&handle, async move {
                for step in 0..2u8 {
                    order.lock().unwrap().push((who, step));
                    switch_point();
                    runs.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }

        // A task of its own runs a `TaskSet`, as `run_parallel` does from
        // inside the emit chain: the set's tasks are spawned from a task, are
        // joined through the set, and only then does the task finish.
        let collector = {
            let runs = runs.clone();
            let collected = collected.clone();
            let done = done.clone();
            let handle = handle.clone();
            let task_handle = handle.clone();
            crate::sync::spawn(&task_handle, async move {
                let mut set = TaskSet::new();
                for who in 0..2u8 {
                    let runs = runs.clone();
                    set.spawn_on(
                        async move {
                            switch_point();
                            runs.fetch_add(1, Ordering::SeqCst);
                            who
                        },
                        &handle,
                    );
                }
                let mut got = Vec::new();
                while let Some(joined) = set.join_next().await {
                    got.push(joined.expect("a task of the set failed"));
                }
                *collected.lock().unwrap() = got;
                done.store(true, Ordering::SeqCst);
            })
        };

        drop(entered);
        scope.run();

        assert!(
            done.load(Ordering::SeqCst),
            "the task that joined its set did not finish"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            6,
            "a spawn did not run exactly once"
        );
        {
            let order = order.lock().unwrap();
            assert_eq!(order.len(), 4, "a spawned task's steps were lost");
            for who in 0..2u8 {
                let steps: Vec<u8> = order
                    .iter()
                    .filter(|(task, _)| *task == who)
                    .map(|(_, step)| *step)
                    .collect();
                assert_eq!(
                    steps,
                    vec![0, 1],
                    "a task's steps ran out of order or twice"
                );
            }
        }
        {
            let mut got = collected.lock().unwrap().clone();
            got.sort();
            assert_eq!(
                got,
                vec![0, 1],
                "the task set lost a spawn or returned one twice"
            );
        }
        // The kernel's handles join after the scheduler ran their tasks, with
        // the same result the task produced.
        for sibling in siblings {
            block_on(sibling.join()).expect("a spawned task failed");
        }
        block_on(collector.join()).expect("the collecting task failed");
    });
}

// ---------------------------------------------------------------------------
// Model 21: mount admission against a root close (#622 item 1, the race
// `ctx.rs::child_created_before_root_closes_but_mounted_afterward_never_applies`
// pins deterministically)

/// What the mount thread did: the child it spawned, and whether the mount
/// record committed (`Ctx::mount_fiber`'s `Ok` branch). A child whose spawn
/// already saw `Shared.closing` is born closed, never enters the parent's
/// children, and is never mounted (`mount_fiber` returns before registering).
struct MountAttempt {
    child: Arc<FiberInner>,
    born_closed: bool,
    committed: bool,
}

/// `FiberInner::unload`'s terminal pass with the cleanup pass `drain_effects`
/// runs: `Unloading` under the admission and transition locks together (which
/// publishes `NOT_LIVE`), the cancel, the kernel's `wait_events`, the records
/// in reverse order, then `Disposed`. The mount cleanup this model registers
/// is synchronous, so the model runs it on its own thread instead of through
/// `record.drain`'s spawned runner.
fn close_unload(fiber: &Arc<FiberInner>) {
    {
        let _admission = fiber.ctx.shared().admission.lock().unwrap();
        let mut tr = fiber.transition.lock().unwrap();
        fiber.set_state(&mut tr, FiberState::Unloading);
    }
    fiber.cancel_current();
    block_on(fiber.wait_events());
    drain_model_effects(fiber);
    let mut tr = fiber.transition.lock().unwrap();
    fiber.set_state(&mut tr, FiberState::Disposed);
}

/// Model 21, mount admission against a root close (#622 item 1; the kernel
/// side of a host reload's mount): a child is spawned on the root and its
/// mount record registered while a root close fixes the subtree and sets
/// `Shared.closing`. Both sides take `Shared.admission`; the mount then takes
/// the parent's transition lock and commits into the parent's effects, and the
/// close's unload takes that transition lock and drains the same list. Both
/// critical sections touch shuttle atomics (`alive`, `lineage_closed`), so
/// with a std mutex the second shuttle thread would park for real at a switch
/// point and the model would hang — the reason this race had no model. The
/// lock stand-in (`crate::sync::Mutex`) parks it on shuttle's scheduler
/// instead, and every interleaving ends.
///
/// The invariant is the pinned test's: a mount that did not commit leaves no
/// record, so a child created before the close and mounted after it never
/// applies (generation 0, no cleanup); a committed mount's cleanup runs
/// exactly once, because the close drains it; and the child ends `Disposed`
/// either way. The model runs `spawn_fiber_inner`, `register_mount_effect`,
/// `mark_subtree_lineage`, `publish_subtree_closing` (and through it
/// `pre_cancel`), `set_state`, `wait_events`, the effect records'
/// `claim`/`finish`/`join` and the admission and transition locks as the
/// kernel's own; what is written out is the mount cleanup's body (the kernel's
/// disposes the child through its driver, which the model does not run), the
/// close's unload of each fixed member, and `mount_fiber`'s refusal branch: the
/// model lets the close unload the child instead of posting and dropping the
/// `Shutdown` intent the kernel posts there, which reaches the same terminal
/// state.
///
/// Mutations this model catches: dropping the commit into the parent's effects
/// (`parent.push_effect`; a committed mount's cleanup never runs, so `runs`
/// stays 0), and dropping the commit into the child's mount slot (the slot no
/// longer says `committed`). The seam itself is pinned by putting
/// `std::sync::Mutex` back for `Shared.admission`: the mounter then parks a
/// real thread at a shuttle switch point inside the section and the model never
/// finishes (observed: still running after a minute), which is why this race
/// had no model before the lock stand-in.
#[test]
fn shuttle_mount_admission_races_root_close() {
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let ctx = Ctx::root_with(runtime.handle().clone());
        let root = ctx.root_view().expect("the root view").inner.clone();
        // Cleanups are std atomics on purpose: the checks after the joins must
        // not be shuttle switch points that could hide an early return.
        let runs = Arc::new(AtomicUsize::new(0));

        let mount = {
            let (ctx, runs) = (ctx.clone(), runs.clone());
            thread::spawn(move || {
                // `Ctx::plugin`: the spawn, then the mount. The spawn takes
                // the admission lock first; a child created after the close
                // is born closed.
                switch_point();
                let child = spawn_fiber(
                    ctx.shared(),
                    Some(&ctx),
                    Some(Arc::new(Named("mounted")) as Arc<dyn crate::DynPlugin>),
                    false,
                );
                let born_closed = !child.alive.load(Ordering::SeqCst);
                if born_closed {
                    return MountAttempt {
                        child,
                        born_closed,
                        committed: false,
                    };
                }
                switch_point();
                // `Ctx::mount_fiber` → `register_mount_effect`: admission, the
                // parent's transition, then the commit into the parent's
                // effects and the child's mount slot.
                match ctx.register_mount_effect(&child, || {
                    Effect::Disposer(Box::new(move || {
                        runs.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }))
                }) {
                    Ok(record) => {
                        // Mount ownership was committed under the admission lock.
                        drop(record);
                        child.post(Intent::RefreshDeps);
                        MountAttempt {
                            child,
                            born_closed,
                            committed: true,
                        }
                    }
                    Err(_) => MountAttempt {
                        child,
                        born_closed,
                        committed: false,
                    },
                }
            })
        };

        let close = {
            let (ctx, root) = (ctx.clone(), root.clone());
            thread::spawn(move || {
                switch_point();
                // `CtxHost::shutdown`'s synchronous part: the fixed set and
                // the closing flags under admission. `publish_subtree_closing`
                // takes each member's transition lock (`pre_cancel`) while the
                // admission lock is held.
                let subtree = {
                    let _admission = ctx.shared().admission.lock().unwrap();
                    let subtree = mark_subtree_lineage(&root);
                    ctx.shared().closing.store(true, Ordering::SeqCst);
                    publish_subtree_closing(&subtree);
                    subtree
                };
                // `drive`'s `Shutdown` arm, written out: every member of the
                // fixed set unloads once, in the postorder the kernel collects.
                for member in &subtree {
                    close_unload(member);
                }
                subtree
            })
        };

        let attempt = mount.join().unwrap();
        let subtree = close.join().unwrap();

        // `spawn_fiber_inner`'s closing check and the close's fixed set share
        // the admission lock: a child spawned before the close is in the set,
        // one spawned after is born closed and is not.
        let in_set = subtree
            .iter()
            .any(|member| Arc::ptr_eq(member, &attempt.child));
        assert_eq!(
            in_set, !attempt.born_closed,
            "a child spawned before the close is missing from its fixed set, or one spawned \
             after it is in the set"
        );
        if attempt.committed {
            assert!(
                in_set,
                "a committed mount's child was not in the close's fixed set"
            );
            assert_eq!(
                runs.load(Ordering::SeqCst),
                1,
                "the close did not run the committed mount's cleanup exactly once"
            );
        } else {
            assert_eq!(
                runs.load(Ordering::SeqCst),
                0,
                "a mount that did not commit left a cleanup to run"
            );
        }
        assert!(
            root.effects.lock().unwrap().is_empty(),
            "the close left an effect record in the root's effects"
        );
        assert_eq!(
            attempt.child.mount.lock().unwrap().is_some(),
            attempt.committed,
            "the child's mount slot does not match whether the mount committed"
        );
        let snapshot = attempt.child.state_snapshot();
        assert_eq!(
            snapshot.generation, 0,
            "a child that never loaded ran a generation"
        );
        assert_eq!(
            snapshot.state,
            FiberState::Disposed,
            "the child did not end Disposed"
        );
    });
}

/// One fiber of the deadline model: the root context whose sink takes the
/// report, the listener entry whose calls are in flight, and how many of its
/// bodies the model runs.
struct DeadlineFiber {
    ctx: Ctx,
    fiber: Arc<FiberInner>,
    entry: Arc<Entry>,
    errors: Arc<std::sync::Mutex<Vec<Arc<ErrorKind>>>>,
    running: Arc<ShuttleUsize>,
}

/// A root on `runtime` with a sink that keeps what it is given, the root
/// fiber, and a listener entry the model starts calls through.
fn deadline_fiber(runtime: &tokio::runtime::Runtime) -> DeadlineFiber {
    let errors: Arc<std::sync::Mutex<Vec<Arc<ErrorKind>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = errors.clone();
    let sink: ErrorSink = Arc::new(move |error| into.lock().unwrap().push(error));
    let ctx = Ctx::root_with_sink(runtime.handle().clone(), sink);
    let fiber = ctx.root_view().expect("the root view").inner.clone();
    let entry = Arc::new(model_entry(&ctx, &fiber));
    DeadlineFiber {
        ctx,
        fiber,
        entry,
        errors,
        running: Arc::new(ShuttleUsize::new(0)),
    }
}

/// Model 22, the unload deadline (#622 item 1's last stand-in; #623 covers the
/// child process that makes a call never return): an unload waits for the event
/// calls counted on its fiber at most `shutdown_deadline()` (`wait_events`),
/// reports `listeners of plugin root still running after <s>s; unloading
/// anyway`, and goes on. Two fibers unload at once, each with two calls in
/// flight, so the model drives two timers; the deadline is the model's clock
/// (`crate::sync::Clock`), not a thread. What runs is the kernel's
/// `Entry::start`/`CallFlight` (the call counted before its liveness check),
/// `FiberInner::begin_event`/`finish_event`, `set_state(Unloading)` under the
/// admission and transition locks, `shutdown_deadline`, `wait_events` with its
/// own `select!`, and `crate::sync::deadline`'s registration on the clock. What
/// is written out is the listener body (the model runs it and holds it at a
/// barrier), the unloader's two steps around the wait, and the scheduler: the
/// model thread that waits until both timers are registered, moves time, and
/// lets the calls end.
///
/// The two orders, both in this test: the calls outlive the deadline, so both
/// waits return at it and report it — a body is still running when they return;
/// and the calls end first, so both waits return on the count without a word,
/// and the registrations they take back fire nothing when time goes on. Half
/// the limit is never enough to fire either timer.
///
/// Mutations this model catches: a `wait_events` that never registers (the
/// deadline-first order waits for registrations that never come, and shuttle
/// reports the deadlock), a clock that fires early (the half-limit advance
/// fires), a deadline that never fires (the deadline-first order's second
/// advance fires nothing and the wait hangs), a registration that outlives its
/// wait (the calls-first order's last advance fires it), a wait that reports
/// though its calls finished (the calls-first order's `reported`), a wait that
/// returns without watching the count (its `running` is non-zero), a wait that
/// goes on past the deadline (it never joins), and a report that carries
/// another limit or names another fiber (the `seconds` and `listeners`). Each
/// was made in `wait_events` or `crate::sync` and reverted; the first three
/// fail before reaching the deadline-first order's joins: a `wait_events` that
/// keeps its own `std::thread` with `recv_timeout` registers nothing, so
/// `wait_for_waiters` blocks with every other thread too and shuttle reports
/// the deadlock; a timer due at once fires on the half-limit advance; one due
/// at twice the limit fires on neither.
#[test]
fn shuttle_event_waits_end_at_their_deadline() {
    // The deadline first: the calls outlive it, so both waits return at it
    // and report it.
    explore(|| deadline_waits(false));
    // The calls first: both finish inside the deadline, so both waits return
    // on the count and the timers they took back fire nothing.
    explore(|| deadline_waits(true));
}

fn deadline_waits(calls_first: bool) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let one = deadline_fiber(&runtime);
    let two = deadline_fiber(&runtime);
    let clock = Clock::new();
    let limit = shutdown_deadline();
    // Four calls and the model: every call is in flight before anything
    // unloads it, and the model lets the bodies end when it says so.
    let claimed = Arc::new(ShuttleBarrier::new(5));
    let released = Arc::new(ShuttleBarrier::new(5));

    let mut dispatchers = Vec::new();
    for fiber in [&one, &two] {
        for _ in 0..2 {
            let (entry, running) = (fiber.entry.clone(), fiber.running.clone());
            let (claimed, released) = (claimed.clone(), released.clone());
            dispatchers.push(thread::spawn(move || {
                // The kernel's admission and flight: `Entry::start` counts the
                // call on its owner before the liveness check, and the guard
                // counts it back down when the body is done.
                let flight = entry
                    .start()
                    .expect("a call was refused before its unload published");
                running.fetch_add(1, Ordering::SeqCst);
                claimed.wait();
                released.wait();
                running.fetch_sub(1, Ordering::SeqCst);
                drop(flight);
            }));
        }
    }

    claimed.wait();
    for fiber in [&one, &two] {
        assert_eq!(
            *fiber.fiber.event_flights.lock().unwrap(),
            2,
            "the model's two calls were not both counted on the fiber"
        );
    }

    let mut waiters = Vec::new();
    for fiber in [&one, &two] {
        let (ctx, inner, running) = (
            fiber.ctx.clone(),
            fiber.fiber.clone(),
            fiber.running.clone(),
        );
        let errors = fiber.errors.clone();
        let clock = clock.clone();
        waiters.push(thread::spawn(move || {
            // The deadline this wait registers is this clock's; the model's
            // own thread is its scheduler.
            let _entered = clock.enter();
            {
                let _admission = ctx.shared().admission.lock().unwrap();
                let mut tr = inner.transition.lock().unwrap();
                inner.set_state(&mut tr, FiberState::Unloading);
            }
            let before = errors.lock().unwrap().len();
            block_on(inner.wait_events());
            let after = errors.lock().unwrap().len();
            (after - before, running.load(Ordering::SeqCst))
        }));
    }

    // Both timers are registered, half the limit is not enough to fire
    // either, and then the model picks the order.
    clock.wait_for_waiters(2);
    let half = limit / 2;
    let rest = limit - half;
    assert_eq!(
        clock.advance(half),
        0,
        "a deadline fired before its limit had passed"
    );

    if calls_first {
        released.wait();
        for waiter in waiters {
            let (reported, still_running) = waiter.join().unwrap();
            assert_eq!(
                reported, 0,
                "a wait reported the deadline although its calls finished first"
            );
            assert_eq!(
                still_running, 0,
                "a wait returned while a call body was still running"
            );
        }
        assert_eq!(
            clock.advance(rest),
            0,
            "a deadline outlived the wait that registered it"
        );
    } else {
        assert_eq!(
            clock.advance(rest),
            2,
            "a deadline did not fire when its limit had passed"
        );
        for waiter in waiters {
            let (reported, still_running) = waiter.join().unwrap();
            assert_eq!(
                reported, 1,
                "a wait past the deadline did not report it exactly once"
            );
            assert_eq!(
                still_running, 2,
                "the wait past the deadline returned after its calls did"
            );
        }
        released.wait();
    }

    for dispatcher in dispatchers {
        dispatcher.join().unwrap();
    }
    // Half the limit and then the rest: the clock is where the model moved
    // it, and no registration is left to fire.
    assert_eq!(
        clock.now(),
        limit,
        "the clock is not where the model moved it"
    );

    for fiber in [&one, &two] {
        assert_eq!(
            *fiber.fiber.event_flights.lock().unwrap(),
            0,
            "an event flight was never finished"
        );
        assert_eq!(
            fiber.running.load(Ordering::SeqCst),
            0,
            "a call body was still running at the end"
        );
        let errors = fiber.errors.lock().unwrap();
        if calls_first {
            assert!(
                errors.is_empty(),
                "a sink got {errors:?} although the calls finished first"
            );
        } else {
            assert_eq!(errors.len(), 1, "a wait reported {} times", errors.len());
            match &*errors[0] {
                ErrorKind::ListenersStillRunning { listeners, seconds } => {
                    assert_eq!(
                        *seconds,
                        limit.as_secs_f64(),
                        "the report carries another limit"
                    );
                    assert_eq!(listeners, "plugin root", "the report names another fiber");
                }
                other => panic!("the sink got {other:?}, not the deadline's error"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Model 23: a restart that loses to a dispose (#622's doubt,
// `core/features.errors`)

/// Model 23, a restart and a dispose registered at the same time (#622's
/// doubt "`restart()` / `update()` 在检查与投递之间输给终止的 Dispose 时被排干成
/// Ok"): the restart's check and pre-cancel run under the transition lock, its
/// post after it. A dispose registered in between is taken up first, and the
/// restart's intent is either drained by the driver's exit or posted after the
/// driver stopped. Both must report that the restart did not happen
/// (`InactiveEffect`), not the dispose's (usually clean) result: a join that
/// returns `Ok` while no generation started reads as a successful restart that
/// never restarted.
///
/// The consumer is loaded once before the race, and only a handled restart
/// can add a generation: a restart that returns `Ok` must have started one
/// (`generation` grows), and one that returns `InactiveEffect` must not.
/// `load`, `unload`, `refresh_deps`, the registry methods, `evict_and_finalize`
/// and `drive`'s `Restart` and `Dispose` arms are mirrored as in models 9 and
/// 19; `consumer_restart`/`consumer_dispose`/`consumer_restart_stopped_error`,
/// `post_join_to`, `join_task`, `TransitionTask` and `EffectRecord` follow the
/// kernel's own.
///
/// Mutations this model catches: completing a drained restart with the
/// terminal error (the pre-fix `drive` drain) and completing a post that
/// landed after the driver stopped with `stopped_error` (the pre-fix
/// `post_join`). In both, the restart joins `Ok` with the generation
/// unchanged, and the model fails with "the restart reported success without
/// starting a generation".
#[test]
fn shuttle_restart_losing_to_a_dispose_reports_it() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let handle = runtime.handle().clone();
    explore(move || {
        let key = TypeKey::of::<u32>();
        let world = Arc::new(DepWorld {
            key: key.clone(),
            registry: DepRegistry::new(),
            admission: SyncMutex::new(()),
            handle: handle.clone(),
        });
        let provider = Arc::new(DepProvider {
            id: PluginId(7),
            state: SyncMutex::new(FiberState::Active),
        });
        provider.set(FiberState::Loading);
        let _first = world
            .registry
            .insert(&key, &provider, 1)
            .expect("the first provide");
        provider.set(FiberState::Active);

        let (consumer, mailbox) = new_consumer(&world);
        world.registry.register_inject(&key, &consumer);
        let driver = {
            let (world, inner) = (world.clone(), consumer.inner.clone());
            thread::spawn(move || drive_consumer(world, inner, mailbox))
        };
        // One load before the race: from here on, only the restart's take-up
        // can start a generation.
        post_consumer(&consumer, Intent::RefreshDeps);
        let settle = TransitionTask::new();
        let sent = post_join_to(
            &consumer.inner.alive,
            &consumer.intents,
            settle.clone(),
            Intent::Settle,
            || consumer_stopped_error(&consumer.inner),
        );
        assert!(sent, "the consumer's driver was not alive");
        assert!(
            block_on(join_task(&settle)).is_ok(),
            "the first load failed"
        );
        assert_eq!(consumer.inner.state(), FiberState::Active);
        let generation = consumer.inner.transition.lock().unwrap().generation;

        let restarter = {
            let consumer = consumer.clone();
            thread::spawn(move || {
                switch_point();
                match consumer_restart(&consumer) {
                    Restart::Refused(error) => Err(error),
                    Restart::Posted(task, _) => block_on(join_task(&task)),
                }
            })
        };
        let disposer = {
            let consumer = consumer.clone();
            thread::spawn(move || {
                switch_point();
                let task = consumer_dispose(&consumer);
                block_on(join_task(&task))
            })
        };

        let restart_result = restarter.join().unwrap();
        let dispose_result = disposer.join().unwrap();
        assert!(
            dispose_result.is_ok(),
            "the dispose reported {dispose_result:?}"
        );

        let inner = consumer.inner.clone();
        let end = inner.state();
        let end_generation = inner.transition.lock().unwrap().generation;
        assert_eq!(
            end,
            FiberState::Disposed,
            "the dispose did not end the fiber"
        );
        match restart_result {
            Ok(()) => assert!(
                end_generation > generation,
                "the restart reported success without starting a generation"
            ),
            Err(error) => {
                assert!(
                    matches!(&*error, ErrorKind::InactiveEffect),
                    "the losing restart reported {error:?}"
                );
                assert_eq!(
                    end_generation, generation,
                    "a restart that reported InactiveEffect started a generation"
                );
            }
        }
        assert_eq!(
            inner.cleanups.load(Ordering::SeqCst),
            inner.applies.load(Ordering::SeqCst),
            "a generation's cleanup did not run exactly once"
        );
        drop(consumer);
        driver.join().unwrap();
    });
}

// ---------------------------------------------------------------------------
// Model 24: a binding's Provided before its Removed (#622's third doubt,
// `events/framework c`)

/// The model's world for one binding's two announcements: the kernel's
/// admission, a mirrored registry slot and the `ServiceChanged` changes in the
/// order their emits were made.
struct ProvideWorld {
    key: TypeKey,
    admission: SyncMutex<()>,
    registry: DepRegistry,
    changes: SyncMutex<Vec<crate::ServiceChange>>,
}

impl ProvideWorld {
    fn announce(&self, change: crate::ServiceChange) {
        self.changes.lock().unwrap().push(change);
    }
}

/// `Ctx::provide_native` where the order is decided: the state check, the
/// insert and the binding's `Provided` announcement commit under one
/// admission. `inserted` fires inside it, so the withdrawal wakes to find the
/// binding and then waits on the admission for the announcement.
fn provide_binding(
    world: &ProvideWorld,
    provider: &Arc<DepProvider>,
    provider_gen: u64,
    inserted: &Notify,
) -> Option<Arc<DepBinding>> {
    let _admission = world.admission.lock().unwrap();
    if matches!(
        provider.state(),
        FiberState::Unloading | FiberState::Disposed
    ) {
        return None;
    }
    let binding = world
        .registry
        .insert(&world.key, provider, provider_gen)
        .expect("the first provide");
    inserted.notify_one();
    world.announce(crate::ServiceChange::Provided);
    Some(binding)
}

/// `FiberInner::unload`'s state change and the `evict_and_finalize` mirror:
/// `Unloading` under the admission, then the withdrawal outside it (mark the
/// binding removing, finalize it) and its `Removed` after the finalize.
fn withdraw_binding(
    world: &ProvideWorld,
    provider: &Arc<DepProvider>,
    binding: &Arc<DepBinding>,
) -> bool {
    {
        let _admission = world.admission.lock().unwrap();
        provider.set(FiberState::Unloading);
    }
    world.registry.mark_removing_if(&world.key, binding);
    let removed = world.registry.finalize_if(&world.key, binding);
    if removed {
        world.announce(crate::ServiceChange::Removed);
    }
    removed
}

/// Model 24, the two announcements of one binding (#622's doubt "同一绑定的
/// `ServiceChanged` 可能先 Removed 后 Provided"): `provide_native` commits the
/// binding and its cleanup record under `Shared.admission`, and the `Provided`
/// event follows in that same critical section; an unload sets `Unloading`
/// under the admission before its cleanup pass can withdraw the binding, so a
/// withdrawal waits for the announcement. Without that, the window between the
/// registration's commit and the event (where the first version emitted it)
/// lets the withdrawal announce `Removed` first: a consumer tracking
/// availability sees the service leave before it arrived.
///
/// The model: the provider's provide and its unload on two threads. The insert
/// and the `Provided` announcement are the admission's critical section
/// (`provide_binding`); the unload's state change takes the admission alone,
/// then the mirrored `evict_and_finalize` marks, finalizes and announces
/// `Removed`. `DepRegistry` mirrors `Registry`'s slots; `ServiceChange`,
/// `SyncMutex` and `Notify` are the shared pieces.
///
/// Mutations this model catches: moving the `Provided` announcement after the
/// admission's release (the pre-fix `provide_native`) lets the withdrawal run
/// first, and the model fails with "the binding's Provided was announced after
/// its Removed"; dropping the announcement fails with "the binding was
/// provided without an announcement".
#[test]
fn shuttle_a_bindings_provided_is_announced_before_its_removed() {
    explore(|| {
        let world = Arc::new(ProvideWorld {
            key: TypeKey::of::<u32>(),
            admission: SyncMutex::new(()),
            registry: DepRegistry::new(),
            changes: SyncMutex::new(Vec::new()),
        });
        let provider = Arc::new(DepProvider {
            id: PluginId(7),
            state: SyncMutex::new(FiberState::Loading),
        });
        let inserted = Arc::new(Notify::new());

        let provider_thread = {
            let (world, provider, inserted) = (world.clone(), provider.clone(), inserted.clone());
            thread::spawn(move || {
                switch_point();
                let binding = provide_binding(&world, &provider, 1, &inserted)
                    .expect("the provider was not unloading");
                provider.set(FiberState::Active);
                binding
            })
        };
        let withdrawer = {
            let (world, provider, inserted) = (world.clone(), provider.clone(), inserted.clone());
            thread::spawn(move || {
                // The unload is what takes the binding away. It can only run
                // after the provide inserted it; the admission decides whether
                // it runs before or after the announcement.
                block_on(inserted.notified());
                switch_point();
                let binding = world
                    .registry
                    .lookup(&world.key)
                    .expect("the binding was inserted before the notification");
                withdraw_binding(&world, &provider, &binding)
            })
        };

        let binding = provider_thread.join().unwrap();
        assert!(
            withdrawer.join().unwrap(),
            "the withdrawal did not remove the binding"
        );
        assert!(
            world.registry.lookup(&world.key).is_none(),
            "the withdrawal left the binding in the slot"
        );
        assert_eq!(binding.provider_gen, 1);
        let changes = world.changes.lock().unwrap().clone();
        assert!(
            changes.contains(&crate::ServiceChange::Provided),
            "the binding was provided without an announcement"
        );
        assert_eq!(
            changes,
            vec![
                crate::ServiceChange::Provided,
                crate::ServiceChange::Removed
            ],
            "the binding's Provided was announced after its Removed"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 25: the order the status queue is delivered in (#622's `core/D24a`)

/// One `FiberStatusChanged` by the fields `core/D24a` is about: whose event,
/// its place in that plugin's own sequence, and the states it goes between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StatusStep {
    plugin: PluginId,
    seq: u64,
    from: FiberState,
    to: FiberState,
}

impl StatusStep {
    fn of(event: &FiberStatusChanged) -> Self {
        Self {
            plugin: event.plugin_id,
            seq: event.seq,
            from: event.from,
            to: event.to,
        }
    }
}

/// `FiberInner::unload` to `Pending`, where it is synchronous: `Unloading`
/// under the admission and transition locks (the commit `set_state` makes),
/// then the kernel's `flush_status`, the cancel, the kernel's `wait_events`, the
/// cleanup pass, then `Pending` and its flush. The cleanup pass is the mirror
/// models 15 and 16 use (`drain_model_effects`): `EffectRecord::drain` spawns its
/// runner on the host runtime, which a shuttle model does not pump. What this
/// model is about — every commit and every delivery — is the kernel's.
fn status_unload(fiber: &Arc<FiberInner>) {
    {
        let _admission = fiber.ctx.shared().admission.lock().unwrap();
        let mut tr = fiber.transition.lock().unwrap();
        fiber.set_state(&mut tr, FiberState::Unloading);
    }
    fiber.flush_status();
    fiber.cancel_current();
    block_on(fiber.wait_events());
    if let Some(error) = aggregate_arcs(drain_model_effects(fiber)) {
        (fiber.ctx.error_sink())(error);
    }
    {
        let mut tr = fiber.transition.lock().unwrap();
        fiber.set_state(&mut tr, FiberState::Pending);
    }
    fiber.flush_status();
}

/// The driver's `Restart` arm where it is synchronous: unload to `Pending`
/// unless the fiber is already there (as `drive()` skips the unload), then load
/// again through the kernel's own `load`.
fn status_restart(fiber: &Arc<FiberInner>) {
    if fiber.state() != FiberState::Pending {
        status_unload(fiber);
    }
    block_on(FiberInner::load(fiber, HashSet::new()));
    assert_eq!(
        fiber.state(),
        FiberState::Active,
        "the {} did not end its restart Active",
        fiber.name
    );
}

/// Model 25, `FiberStatusChanged` delivered in commit order (#622's
/// `core/D24a`; the one piece models 5, 12, 15 and 16 wrote out as "not
/// modelled": `flush_status`'s emits). Each plugin's `seq` is continuous from 1,
/// each event's `from` is the previous event's `to`, the chain ends in the state
/// the fiber is in, and every committed event was delivered — however many
/// fibers commit and flush at once.
///
/// The model runs the kernel's own `load` and `flush_status` on three real
/// fibers that restart on their own threads: `load` commits under the transition
/// lock (`set_state`: the state, the sequence number and the queue entry in one
/// critical section) and flushes outside it (take the whole queue, then emit in
/// order). `unload` is mirrored where the driver is (`status_unload`): its
/// commits and its two `flush_status` calls are the kernel's, its cleanup pass is
/// the mirror models 15 and 16 use, because `EffectRecord::drain` spawns its
/// runner on a runtime a shuttle model does not pump.
///
/// `deliver_status` hands each event to the bus; `FiberInner::watch_status`
/// gives the model a tap there, so what it records is what the bus is given, in
/// the order it is given it, and a switch point after each delivery lets the
/// scheduler run another fiber's driver inside this one's flush. What the bus
/// does with each event afterwards is the emit chain's order — model 10,
/// `core/D31a`: per name, one emit's calls start before the next emit's.
///
/// Mutations this model catches: bumping `seq` after the push instead of before
/// it (`set_state` — "the first delivered seq 0 as its 1th event: a status event
/// was lost or delivered twice"), and dropping `load`'s final `flush_status`
/// ("the first is Active but its last delivered event ends at Loading"). Notice
/// what it cannot catch: `flush_status`'s loop reordered. The kernel flushes
/// after every commit, so in this model — and in the kernel — a flush takes at
/// most one event, and the order inside one flush has nothing to reorder.
#[test]
fn shuttle_status_events_arrive_in_commit_order() {
    const RESTARTS: usize = 3;
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let ctx = Ctx::root_with(runtime.handle().clone());
        // Children start `Pending`, so their first restart commits `Loading` and
        // `Active`, and every later one four states.
        let fibers: Vec<Arc<FiberInner>> = ["first", "second", "third"]
            .into_iter()
            .map(|name| {
                spawn_fiber(
                    ctx.shared(),
                    Some(&ctx),
                    Some(Arc::new(Named(name)) as Arc<dyn crate::DynPlugin>),
                    false,
                )
            })
            .collect();
        let delivered: Arc<std::sync::Mutex<Vec<StatusStep>>> = Arc::default();

        // The tap: what the kernel hands the bus, at the point it hands it over,
        // then a switch point — the scheduler may run another fiber's driver
        // inside this one's flush.
        for fiber in &fibers {
            let tap = delivered.clone();
            fiber.watch_status(move |event: &FiberStatusChanged| {
                tap.lock().unwrap().push(StatusStep::of(event));
                switch_point();
            });
        }
        let drivers: Vec<_> = fibers
            .iter()
            .map(|fiber| {
                let fiber = fiber.clone();
                thread::spawn(move || {
                    for _ in 0..RESTARTS {
                        switch_point();
                        status_restart(&fiber);
                    }
                })
            })
            .collect();
        for driver in drivers {
            driver.join().unwrap();
        }

        let delivered = delivered.lock().unwrap().clone();
        for fiber in &fibers {
            let steps: Vec<StatusStep> = delivered
                .iter()
                .filter(|step| step.plugin == fiber.id)
                .copied()
                .collect();
            assert!(
                steps.len() >= 2 * RESTARTS,
                "the {} delivered {} status events, too few for {RESTARTS} restarts",
                fiber.name,
                steps.len()
            );
            let mut from = FiberState::Pending;
            for (index, step) in steps.iter().enumerate() {
                assert_eq!(
                    step.seq,
                    index as u64 + 1,
                    "the {} delivered seq {} as its {}th event: a status event was lost or \
                     delivered twice",
                    fiber.name,
                    step.seq,
                    index + 1
                );
                assert_eq!(
                    step.from, from,
                    "the {} delivered {:?} → {:?} after ending at {from:?}: the events are not \
                     delivered in commit order",
                    fiber.name, step.from, step.to
                );
                from = step.to;
            }
            let snapshot = fiber.state_snapshot();
            assert_eq!(
                from, snapshot.state,
                "the {} is {:?} but its last delivered event ends at {from:?}",
                fiber.name, snapshot.state
            );
            let tr = fiber.transition.lock().unwrap();
            assert_eq!(
                tr.seq,
                steps.len() as u64,
                "the {} committed {} status events and delivered {}",
                fiber.name,
                tr.seq,
                steps.len()
            );
            assert!(
                tr.status_queue.is_empty(),
                "the {} left {} status event(s) queued",
                fiber.name,
                tr.status_queue.len()
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Model 26: a mount's cleanup against the services it provides (#622's
// `MNT-6-4`, design-protocol-plugin-mount §6)

/// What one mount's process did, in the order it did it: the service withdrawn,
/// the process closed. Std mutex: nothing is asserted while it is held, and a
/// push is never a switch point.
type MountLog = Arc<std::sync::Mutex<Vec<&'static str>>>;

/// The process behind a mount: open until the mount's cleanup closes it, and the
/// thing a service call reaches. `gate` makes a call atomic with the close, so
/// the model can ask whether a call ever found a closed process behind a service
/// it could still get.
struct MountProcess {
    log: MountLog,
    open: StdBool,
    gate: SyncMutex<()>,
}

/// The service a mount provides (`projection.attach`'s binding). Dropping it is
/// the withdrawal: the kernel drops the binding's value when it finalizes the
/// binding, inside the cleanup record the provide registered.
struct MountService(Arc<MountProcess>);

impl MountService {
    /// Whether a call would reach the process. The caller holds the gate, so
    /// this cannot interleave with the close.
    fn answers(&self) -> bool {
        self.0.open.load(Ordering::SeqCst)
    }
}

impl Drop for MountService {
    fn drop(&mut self) {
        self.0.log.lock().unwrap().push("withdraw");
    }
}

/// Two services, so one model can provide both orders: the generated order, and
/// the order the design forbids.
trait Mounted: Send + Sync {
    /// What a call of the service asks the process.
    fn call(&self) -> bool;
}

/// The second mount's service: only its binding and its withdrawal matter.
trait Mismounted: Send + Sync {}

impl Mounted for MountService {
    fn call(&self) -> bool {
        self.answers()
    }
}

impl Mismounted for MountService {}

/// A mount's cleanup effect: the last thing an unload runs when the effect was
/// registered first, which is what the design asks for.
fn mount_cleanup(process: &Arc<MountProcess>) -> Effect {
    let process = process.clone();
    Effect::AsyncDisposer(Box::new(move || {
        let process = process.clone();
        Box::pin(async move {
            // Under the gate a call takes: no call can be between getting the
            // service and calling it while the process closes.
            let _gate = process.gate.lock().unwrap();
            process.log.lock().unwrap().push("close");
            process.open.store(false, Ordering::SeqCst);
            Ok(())
        })
    }))
}

/// Model 26, the order between a mount's cleanup effect and the services it
/// provides (#622's `MNT-6-4`; the generated mount's `apply`, §6: "挂载插件先登记
/// 清理 effect、再注册服务。rutis 逆序清理，所以先撤销服务、执行消费者的 disposer
/// （此时仍可调用远端服务），最后才关闭 Node 进程").
///
/// The model runs the kernel's own pieces of that order on a real fiber: the
/// mount's cleanup is a real `Ctx::effect` (through `effect_raw`, which registers
/// the same record), its service a real `Ctx::provide_as` — the registration
/// `projection.attach` makes through `provide_at`, as
/// `session/plugin/services.rs` does — and the unload is `FiberInner::unload`, so
/// the reverse pass is `drain_effects`'s; `SpawnScope` drives the runners it
/// spawns. One fiber registers the two in the generated order, one in the order
/// the design forbids, and a third thread keeps calling the service while the
/// first mount unloads.
///
/// The assertions are the order and its consequence: the withdrawal (the binding
/// value dropped) before the close in the generated order, the close before the
/// withdrawal in the other, and no call that can still get the service finding
/// its process closed.
///
/// Mutations this model catches: draining the records in registration order
/// instead of in reverse (`drain_effects`'s `rev()` dropped made the model report
/// "a call found the service with the process already closed": the process closes
/// before its service is withdrawn, so a call that still gets the service finds
/// it dead). The second fiber registers the same two the other way round, and the
/// model reads that order as `close` then `withdraw` — so the assertions are
/// about the order, not about one fiber's shape.
///
/// Not modelled: the consumer's own disposer, which the withdrawal stops
/// (`evict_and_finalize` posts to the consumer's driver and joins it — a driver
/// a shuttle model does not run). It is model 9's subject (a withdrawal returns
/// with the consumer stopped) and
/// `instance_subtrees.rs::closing_subtree_disposes_consumer_before_provider`
/// pins the order over a real tree.
#[test]
fn shuttle_mount_cleanup_runs_after_its_services() {
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let scope = SpawnScope::new();
        let ctx = Ctx::root_with(runtime.handle().clone());
        // Both mount plugins exist before the scope goes in: a fiber's driver
        // waits on its mailbox, and a driver in the scope would never finish.
        let mounted = spawn_fiber(
            ctx.shared(),
            Some(&ctx),
            Some(Arc::new(Named("mount")) as Arc<dyn crate::DynPlugin>),
            false,
        );
        let wrong = spawn_fiber(
            ctx.shared(),
            Some(&ctx),
            Some(Arc::new(Named("wrong")) as Arc<dyn crate::DynPlugin>),
            false,
        );
        // The cleanup pass spawns its runners (`EffectRecord::drain`,
        // `run_cleanups`), so the model installs the spawn stand-in and drives
        // them.
        let _entered = scope.enter();
        block_on(FiberInner::load(&mounted, HashSet::new()));
        block_on(FiberInner::load(&wrong, HashSet::new()));

        // The mounted plugin as the generator registers it: the cleanup effect
        // first, the service after it.
        let log: MountLog = Arc::default();
        let process = Arc::new(MountProcess {
            log: log.clone(),
            open: StdBool::new(true),
            gate: SyncMutex::new(()),
        });
        let close = mount_cleanup(&process);
        mounted
            .ctx
            .effect_raw(move || close)
            .expect("the mount's cleanup effect was refused");
        mounted
            .ctx
            .provide_as::<dyn Mounted>(Arc::new(MountService(process.clone())) as Arc<dyn Mounted>)
            .expect("the mount's service was refused");

        // A call of the mounted process, while the mount unloads: what a
        // consumer's disposer does before the process closes.
        let done = Arc::new(SyncBool::new(false));
        let caller = {
            let (caller_ctx, done, gate) = (mounted.ctx.clone(), done.clone(), process.clone());
            thread::spawn(move || {
                let mut calls = 0usize;
                while !done.load(Ordering::SeqCst) {
                    // Under the gate the close takes. The binding serves until
                    // the withdrawal finalizes it, and the process closes only
                    // after that: a call that still gets the service must find
                    // the process open.
                    let gate_held = gate.gate.lock().unwrap();
                    if let Some(service) = caller_ctx.get::<dyn Mounted>() {
                        assert!(
                            service.call(),
                            "a call found the service with the process already closed"
                        );
                        calls += 1;
                    }
                    drop(gate_held);
                    switch_point();
                }
                calls
            })
        };

        // The mount's unload: the kernel's own, so the reverse pass over the
        // two registrations is `drain_effects`'s.
        block_on(FiberInner::unload(&mounted, NextState::Disposed));
        done.store(true, Ordering::SeqCst);
        scope.run();
        caller.join().unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec!["withdraw", "close"],
            "the mount's cleanup did not run after its service was withdrawn: {:?}",
            *log.lock().unwrap()
        );
        assert!(
            mounted.effects.lock().unwrap().is_empty(),
            "the mount kept an effect record after its unload"
        );
        assert!(
            mounted.ctx.get::<dyn Mounted>().is_none(),
            "the withdrawn mount still serves its service"
        );

        // The same two registrations the other way round, which the design
        // forbids: the model tells the two orders apart, so it is not vacuous.
        let wrong_log: MountLog = Arc::default();
        let wrong_process = Arc::new(MountProcess {
            log: wrong_log.clone(),
            open: StdBool::new(true),
            gate: SyncMutex::new(()),
        });
        wrong
            .ctx
            .provide_as::<dyn Mismounted>(
                Arc::new(MountService(wrong_process.clone())) as Arc<dyn Mismounted>
            )
            .expect("the wrong order's service was refused");
        let wrong_close = mount_cleanup(&wrong_process);
        wrong
            .ctx
            .effect_raw(move || wrong_close)
            .expect("the wrong order's cleanup effect was refused");
        block_on(FiberInner::unload(&wrong, NextState::Disposed));
        scope.run();
        assert_eq!(
            *wrong_log.lock().unwrap(),
            vec!["close", "withdraw"],
            "a service registered before its cleanup was still withdrawn after it: the model \
             cannot tell the design's order from the other"
        );
    });
}

// ---------------------------------------------------------------------------
// Model 27: what a method registers on the calling row's context (#622's
// `MNT-11-9b`)

/// Emit the model's event and run what the emit spawned: the listeners of a
/// fiber that is live hear it; a fiber that is not has none.
fn emit_model_event(ctx: &Ctx, scope: &SpawnScope) {
    ctx.emit(ModelEvent).expect("the emit was refused");
    scope.run();
}

/// Register a listener on `ctx` that counts into `calls`: what a method taking
/// the calling row's `&Ctx` does with it (§11's "effect、监听").
fn listen_row(ctx: &Ctx, calls: &Arc<AtomicUsize>) -> Result<(), Error> {
    let calls = calls.clone();
    ctx.on_sync::<ModelEvent>(move |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    })
    .map(drop)
}

/// Model 27, a row's registrations go with the row (#622's `MNT-11-9b`; mount
/// design §11: "方法在调用方 `Ctx` 上登记的东西（effect、监听）随该行卸载或重启而
/// 移除，运行时照常运行").
///
/// A method of a Rust service that takes the calling row's `&Ctx` registers on
/// that row's fiber, not on its own (`CallerRow::ctx`, `session/services.rs`).
/// The model runs the kernel's side of that promise on two real rows: a listener
/// and an effect registered on one row's `Ctx` through `Ctx::on_sync` and
/// `Ctx::effect`, then the row's `unload` — the real one, so the listener is
/// withdrawn and the effect runs in `drain_effects` — and a second thread
/// registers on the same `Ctx` while that unload runs. Every registration the row
/// took is a cleanup of that row: it is run once by an unload of the row and
/// never left behind, and a listener of the row is not called after it. The other
/// row — the runtime — still hears the event, across the row's unload and
/// restart. A row in a terminal state takes no registration at all; a restarted
/// one takes them again.
///
/// Mutations this model catches: dropping the terminal state check in
/// `register_effect_named` (both of them: `Ctx::effect` on a `Disposed` row then
/// returns `Ok` and leaves a cleanup nothing will run — the model reports "a
/// pulled-down row accepted a registration: effect true, listener false"). The
/// bounds around the race pin the rest: a cleanup never runs twice, a record the
/// row kept is at most the registrations it took, and nothing of the method's is
/// left when the row has ended.
///
/// Not modelled: the dispatcher that reads `caller_row()` and the method that
/// registers — `crates/rutis-bridge/tests/caller_rows.rs` drives those over a real
/// runtime. What this model adds is the lifecycle half: what a registration on a
/// row's `Ctx` does when the row unloads, restarts or ends.
#[test]
fn shuttle_row_registrations_go_with_the_row() {
    explore(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let scope = SpawnScope::new();
        let ctx = Ctx::root_with(runtime.handle().clone());
        // The runtime row keeps running; the row whose call the model makes
        // goes. Both before the scope goes in: a fiber's driver waits on its
        // mailbox, and a driver in the scope would never finish.
        let runtime_row = spawn_fiber(
            ctx.shared(),
            Some(&ctx),
            Some(Arc::new(Named("runtime")) as Arc<dyn crate::DynPlugin>),
            false,
        );
        let row = spawn_fiber(
            ctx.shared(),
            Some(&ctx),
            Some(Arc::new(Named("row")) as Arc<dyn crate::DynPlugin>),
            false,
        );
        // The cleanup pass and the emit chain spawn their runners
        // (`EffectRecord::drain`, an emit's calls), so the model installs the
        // spawn stand-in and drives them.
        let _entered = scope.enter();
        block_on(FiberInner::load(&runtime_row, HashSet::new()));
        block_on(FiberInner::load(&row, HashSet::new()));

        // Std atomics: read after the joins and the emits, where a shuttle load
        // would itself be a scheduling point.
        let runtime_calls = Arc::new(AtomicUsize::new(0));
        let row_calls = Arc::new(AtomicUsize::new(0));
        let cleanups: Arc<std::sync::Mutex<Vec<&'static str>>> = Arc::default();
        listen_row(&runtime_row.ctx, &runtime_calls)
            .expect("the runtime row's listener was refused");
        listen_row(&row.ctx, &row_calls).expect("the row's listener was refused");
        emit_model_event(&ctx, &scope);
        assert_eq!(
            (
                runtime_calls.load(Ordering::SeqCst),
                row_calls.load(Ordering::SeqCst)
            ),
            (1, 1),
            "an emit before the row unloaded did not reach both rows"
        );

        // The method's registrations on the row's context, racing the row's
        // unload: a listener and an effect, as a method of §11 would register
        // them.
        let registrar = {
            let (row_ctx, cleanups, calls) = (row.ctx.clone(), cleanups.clone(), row_calls.clone());
            thread::spawn(move || {
                switch_point();
                let listener = listen_row(&row_ctx, &calls);
                switch_point();
                let effect = row_ctx.effect(move || {
                    let cleanups = cleanups.clone();
                    async move {
                        cleanups.lock().unwrap().push("method cleanup");
                        Ok(())
                    }
                });
                (listener.is_ok(), effect.is_ok())
            })
        };
        switch_point();
        // The row unloads: the kernel's `unload`, whose cleanup pass is
        // `drain_effects`'s reverse one.
        block_on(FiberInner::unload(&row, NextState::Pending));
        scope.run();
        let (listener, effect) = registrar.join().unwrap();
        let accepted = usize::from(listener) + usize::from(effect);
        let ran = cleanups.lock().unwrap().len();
        let left = row.effects.lock().unwrap().len();
        // A registration the unload took is gone with it; one that came after it
        // (the row was already `Pending`) is still the row's, and a later unload
        // runs it. `ctx.effect` runs the cleanup it is given even when it then
        // refuses the registration (the generation went between its two checks),
        // so a refusal may still have run it — never twice, though, and never a
        // record that is not the row's.
        assert!(ran <= 1, "the method's cleanup ran {ran} times");
        assert!(
            left <= accepted,
            "the row kept {left} record(s) for {accepted} registration(s)"
        );
        if effect {
            assert!(
                ran == 1 || left >= 1,
                "the row took the method's effect and neither ran it nor kept it"
            );
        }

        // The row's listeners went with it; the runtime row's did not.
        emit_model_event(&ctx, &scope);
        assert_eq!(
            runtime_calls.load(Ordering::SeqCst),
            2,
            "the runtime row stopped hearing events"
        );
        assert_eq!(
            row_calls.load(Ordering::SeqCst),
            1,
            "a listener of the unloaded row was called"
        );

        // The row restarts: the new generation's registrations are the new
        // row's, and the runtime row was never touched.
        block_on(FiberInner::load(&row, HashSet::new()));
        listen_row(&row.ctx, &row_calls).expect("the restarted row's listener was refused");
        emit_model_event(&ctx, &scope);
        assert_eq!(
            row_calls.load(Ordering::SeqCst),
            2,
            "the restarted row does not hear events"
        );
        assert_eq!(
            runtime_calls.load(Ordering::SeqCst),
            3,
            "the restarted row's emit did not reach the runtime row"
        );
        assert_eq!(
            runtime_row.state(),
            FiberState::Active,
            "the runtime row stopped running"
        );

        // A row that has ended takes nothing more: an effect or a listener
        // registered on it would be a cleanup nothing would ever run.
        block_on(FiberInner::unload(&row, NextState::Disposed));
        scope.run();
        let late_effect = row.ctx.effect(|| async { Ok(()) });
        let late_listener = listen_row(&row.ctx, &row_calls);
        assert!(
            late_effect.is_err() && late_listener.is_err(),
            "a pulled-down row accepted a registration: effect {:?}, listener {:?}",
            late_effect.is_ok(),
            late_listener.is_ok()
        );
        assert!(
            row.effects.lock().unwrap().is_empty(),
            "the ended row kept {} effect record(s), which no unload will run",
            row.effects.lock().unwrap().len()
        );
        assert!(
            !effect || cleanups.lock().unwrap().len() == 1,
            "the row took the method's effect and never ran its cleanup"
        );
        assert!(
            cleanups.lock().unwrap().len() <= 1,
            "the method's cleanup ran more than once"
        );
    });
}
