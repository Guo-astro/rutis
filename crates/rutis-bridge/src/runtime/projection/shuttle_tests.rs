//! Shuttle model of a service projection (#628): a Cordis slot's handle
//! changes from one thread while the projection publishes it, and the row
//! closes or withdraws the projection from another. The model runs the
//! production `Projection::changed`, `publish`, `close` and `withdraw`;
//! `crate::sync::Mutex` makes the projection's state shuttle's under
//! `--cfg rutis_shuttle`, so a change and a publisher interleave at every
//! lock.
//!
//! The process behind the projection is inert (`Process::inert`): its slots
//! are handles the model sets, and what it releases is recorded, not sent.
//! The gate in each model's `make` blocks the publisher inside a slot, after
//! the state's lock is dropped: that is where a close or a withdraw races a
//! publication that is already under way.
//!
//! What the model checks: the slot is left holding the newest handle a
//! change reports, a handle is released exactly once, a close applies
//! nothing after it returns, and a withdraw returns only after the
//! publication under way has finished.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! guarantee: boundary/5.1

use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll, Waker};

use rutis::host::{CtxHost, Key};
use rutis::Ctx;
use shuttle::sync::{Condvar, Mutex};
use shuttle::thread;
use tokio::runtime::Builder;

use super::Projection;
use crate::runtime::process::ServiceEvents;
use crate::runtime::Process;

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

/// Poll `future` to completion on this thread, and the runtime's tasks
/// meanwhile: a disposal can wait for a task it spawned, which only the
/// runtime drives. Each `Pending` yields to the model too, so a wakeup from
/// another shuttle thread is not needed for the poll loop to see it.
fn block_on<F: Future>(future: F, runtime: &tokio::runtime::Runtime) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                runtime.block_on(async {
                    for _ in 0..4 {
                        tokio::task::yield_now().await;
                    }
                });
                thread::yield_now();
            }
        }
    }
}

/// A published service, holding its handle until it is dropped: what the
/// projection's `make` hands to the context.
struct Held {
    process: Arc<Process>,
    handle: String,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.process.release(&self.handle);
    }
}

/// The handle `ctx` publishes under `key`, if any.
fn held(ctx: &Ctx, key: &Key<Held>) -> Option<String> {
    ctx.get_at(key).map(|held| held.handle.clone())
}

/// How many times `handle` was released.
fn releases(released: &StdMutex<Vec<String>>, handle: &str) -> usize {
    released
        .lock()
        .unwrap()
        .iter()
        .filter(|released| *released == handle)
        .count()
}

/// A publisher stopped inside `make`: `make` says it entered, waits until
/// the model releases it, and only then lets the publication go on.
#[derive(Default)]
struct Gate {
    entered: Mutex<bool>,
    release: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn enter(&self) {
        *self.entered.lock().unwrap() = true;
        self.changed.notify_all();
        let mut release = self.release.lock().unwrap();
        while !*release {
            release = self.changed.wait(release).unwrap();
        }
    }

    fn wait_entered(&self) {
        let mut entered = self.entered.lock().unwrap();
        while !*entered {
            entered = self.changed.wait(entered).unwrap();
        }
    }

    fn release(&self) {
        *self.release.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

/// One projection on a bus, with the inert process whose slot `svc` starts
/// at `h0`, and a gate that stops the publication of `h1`.
fn setup() -> (
    tokio::runtime::Runtime,
    Ctx,
    Arc<Process>,
    Arc<Projection>,
    Key<Held>,
    Arc<Gate>,
    Arc<StdMutex<Vec<String>>>,
) {
    let runtime = Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    let entered = runtime.enter();
    let ctx = Ctx::root_with(runtime.handle().clone());
    let released: Arc<StdMutex<Vec<String>>> = Arc::default();
    let process = Process::inert(
        runtime.handle().clone(),
        vec![("svc".to_owned(), "h0".to_owned())],
        released.clone(),
    );
    let projection = Projection::new();
    let key = Key::<Held>::of();
    let gate = Arc::new(Gate::default());
    {
        let gate = gate.clone();
        projection.service_keyed("svc", key.clone(), move |process, handle: String| {
            if handle == "h1" {
                gate.enter();
            }
            Arc::new(Held { process, handle })
        });
    }
    projection
        .attach(&ctx, process.clone())
        .expect("the projection attaches");
    assert_eq!(held(&ctx, &key), Some("h0".to_owned()));
    drop(entered);
    (runtime, ctx, process, projection, key, gate, released)
}

/// #628: a change under way when another arrives and the projection closes
/// finishes (its handle is published), the later one does not start, and
/// every handle made is released exactly once; the one published is released
/// when its context goes.
#[test]
fn a_change_in_flight_and_a_close_leave_the_latest_handle_applied() {
    explore(|| {
        let (runtime, ctx, _process, projection, key, gate, released) = setup();
        let changing = {
            let projection = projection.clone();
            thread::spawn(move || projection.changed("svc", Some("h1".to_owned()), 2))
        };
        gate.wait_entered();
        // Another change arrives while h1 publishes, then the row closes the
        // projection: the change after h1 must not start.
        projection.changed("svc", Some("h2".to_owned()), 3);
        projection.close();
        assert_eq!(
            held(&ctx, &key),
            Some("h0".to_owned()),
            "a handle was applied after close returned"
        );
        gate.release();
        changing.join().unwrap();
        assert_eq!(
            held(&ctx, &key),
            Some("h1".to_owned()),
            "the change under way did not finish or the later one applied"
        );
        assert!(
            projection.state.lock().unwrap().slots.is_empty(),
            "the projection kept its slots after close"
        );

        // The published service goes with its context, once.
        block_on(ctx.shutdown(), &runtime).expect("the context shuts down");
        assert_eq!(releases(&released, "h0"), 1, "h0 was not released once");
        assert_eq!(releases(&released, "h1"), 1, "h1 was not released once");
        assert_eq!(
            releases(&released, "h2"),
            0,
            "a handle never published was released"
        );
    });
}

/// #628: a withdraw returns only once the publication under way is idle, and
/// then every service it published is gone, each handle released once.
#[test]
fn a_withdraw_waits_for_a_publisher_in_flight() {
    explore(|| {
        let (runtime, ctx, _process, projection, key, gate, released) = setup();
        let changing = {
            let projection = projection.clone();
            thread::spawn(move || projection.changed("svc", Some("h1".to_owned()), 2))
        };
        gate.wait_entered();
        let mut withdrawing = Box::pin(async { projection.withdraw().await });
        let mut context = Context::from_waker(Waker::noop());
        assert!(
            withdrawing.as_mut().poll(&mut context).is_pending(),
            "withdraw returned while a publication was under way"
        );
        gate.release();
        changing.join().unwrap();
        block_on(withdrawing, &runtime);
        assert!(
            held(&ctx, &key).is_none(),
            "a service stayed after withdraw"
        );
        assert!(
            projection.state.lock().unwrap().slots.is_empty(),
            "the projection kept its slots after withdraw"
        );
        assert_eq!(releases(&released, "h0"), 1, "h0 was not released once");
        assert_eq!(releases(&released, "h1"), 1, "h1 was not released once");
    });
}

/// Out-of-order withdrawal and reprovision (two concurrent `service` RPCs
/// racing between `Imports::update` and `Projection::changed`): the newer
/// version wins, so an older withdrawal never leaves the slot empty.
#[test]
fn out_of_order_withdrawal_and_reprovision_keep_the_newer_service() {
    explore(|| {
        let (runtime, ctx, _process, projection, key, _gate, released) = setup();
        let withdraw = {
            let projection = projection.clone();
            thread::spawn(move || projection.changed("svc", None, 2))
        };
        let provide = {
            let projection = projection.clone();
            thread::spawn(move || projection.changed("svc", Some("h2".to_owned()), 3))
        };
        withdraw.join().unwrap();
        provide.join().unwrap();
        // Drive any async withdrawal spawned on the tokio runtime until idle.
        loop {
            let settled = {
                let state = projection.state.lock().unwrap();
                !state.applying && state.slots.get("svc").is_none_or(|slot| !slot.withdrawing)
            };
            if settled {
                break;
            }
            runtime.block_on(async {
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
            });
            thread::yield_now();
        }
        assert_eq!(
            held(&ctx, &key),
            Some("h2".to_owned()),
            "an older withdrawal overrode a newer reprovision"
        );
        projection.close();
        block_on(ctx.shutdown(), &runtime).expect("the context shuts down");
        assert_eq!(releases(&released, "h0"), 1, "h0 was not released once");
        assert_eq!(releases(&released, "h2"), 1, "h2 was not released once");
    });
}

/// Out-of-order replacements (`v2` vs `v3`): the newer handle stays published
/// and every handle is released exactly once.
#[test]
fn out_of_order_replacements_keep_the_newer_handle_and_release_each_once() {
    explore(|| {
        let (runtime, ctx, _process, projection, key, _gate, released) = setup();
        let older = {
            let projection = projection.clone();
            thread::spawn(move || projection.changed("svc", Some("h2".to_owned()), 2))
        };
        let newer = {
            let projection = projection.clone();
            thread::spawn(move || projection.changed("svc", Some("h3".to_owned()), 3))
        };
        older.join().unwrap();
        newer.join().unwrap();
        assert_eq!(
            held(&ctx, &key),
            Some("h3".to_owned()),
            "an older replacement overrode a newer one"
        );
        projection.close();
        block_on(ctx.shutdown(), &runtime).expect("the context shuts down");
        assert_eq!(releases(&released, "h0"), 1, "h0 was not released once");
        assert_eq!(releases(&released, "h2"), 1, "h2 was not released once");
        assert_eq!(releases(&released, "h3"), 1, "h3 was not released once");
    });
}
