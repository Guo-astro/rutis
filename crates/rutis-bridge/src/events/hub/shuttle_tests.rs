//! Shuttle model of a session's event host (#628): a row's listeners
//! register while the row unloads or ends, an `events.off` races the row's
//! end, the session leaves while a listener is registering, and the
//! session's emits start in the order they were queued — one cancelled while
//! it waits its turn starts nothing, one that panics does not stop the rest.
//!
//! The model runs the production `SessionEvents::{on, off, row_loading,
//! row_unloading, row_ended, leave, run_emit, next_emit}` and `EmitRun`, and
//! the kernel's real listener table: `crate::sync::Mutex` makes the session's
//! `rows`, `entries` and `emits` shuttle's, so an `on` that has taken the row
//! but not yet recorded its entry and a `row_unloading` that wants the row
//! interleave there. The session's connection is inert (`Connection::inert`):
//! no frame is sent, but the host records and withdraws the entry as it
//! always does.
//!
//! What stands in for the runtime: nothing runs a call on the entry, so
//! "in flight" here is a registration or a session leave already inside
//! the host's locks, not a listener call. A listener call would wait for
//! the far end's reply, which an inert connection never sends. An emit's run
//! is the model's own future: it records that its dispatch started, and the
//! session's task drives the queue on a current-thread runtime the model
//! pumps when the threads are done.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! guarantee: events/hub-lifecycle, events/emit

use std::future::poll_fn;
use std::sync::Arc;
use std::task::{Poll, Waker};

use rutis::host::{BoxFuture, CtxHost};
use rutis::internal::CtxInternal;
use rutis::Ctx;
use serde_json::json;
use shuttle::sync::Barrier;
use shuttle::thread;
use tokio::runtime::Builder;

use super::{Cancel, EmitRun, SessionEvents};
use crate::session::Connection;
use crate::session::Value;

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

/// A runtime for the contexts and the inert session; no task is driven.
fn runtime() -> tokio::runtime::Runtime {
    Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
}

/// One `events.on` request for `row`.
fn registration(id: &str, row: &str) -> Value {
    Value::Data(json!({
        "id": id,
        "name": "t/x",
        "row": row,
        "plugin": "p",
        "kind": "any",
    }))
}

/// The bus has no remote entry left.
fn no_remote_entry(ctx: &Ctx) -> bool {
    ctx.events()
        .subscriptions()
        .iter()
        .all(|entry| !entry.remote)
}

/// Run the disposals `dispose_now` spawned on the session's runtime.
fn pump(runtime: &tokio::runtime::Runtime) {
    runtime.block_on(async {
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
    });
}

/// #628: a row's listeners never outlive the row, whichever starts first —
/// the `on`, the unload, the end or the session's leave. The kernel's table
/// is the one the host registered into, so a missing withdrawal shows up
/// there, not only in the host's own entries map.
#[test]
fn a_rows_listeners_leave_before_or_after_its_unload() {
    explore(|| {
        let runtime = runtime();
        let mut entered = runtime.enter();
        let root = Ctx::root_with(runtime.handle().clone());
        let connection = Connection::inert(runtime.handle().clone());
        let session = SessionEvents::join(Some(&root), connection, "rt".into(), Vec::new())
            .expect("the session joins the bus");

        // An `on` racing the row's unload and end.
        session.row_loading("chat", Vec::new(), "chat".into());
        let step = Arc::new(Barrier::new(3));
        let registering = {
            let (session, step) = (session.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                session.on(registration("1", "chat"))
            })
        };
        let unloading = {
            let (session, step) = (session.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                session.row_unloading("chat");
                session.row_ended("chat");
            })
        };
        step.wait();
        let registered = registering.join().unwrap();
        unloading.join().unwrap();
        // The on may have been answered before the row ended, or refused
        // once it was gone; either way it must not leave an entry.
        if let Err(error) = registered {
            assert!(
                error.to_string().contains("not loaded"),
                "the registration ended as {error:?}"
            );
        }
        assert!(
            session.entries.lock().unwrap().is_empty(),
            "the row's entry stayed in the host"
        );
        drop(entered);
        pump(&runtime);
        entered = runtime.enter();
        assert!(no_remote_entry(&root), "the row's entry stayed on the bus");

        // An `off` racing the row's end: one of the two withdraws the
        // entry, exactly once.
        session.row_loading("chat2", Vec::new(), "chat2".into());
        session
            .on(registration("2", "chat2"))
            .expect("the listener registers");
        session
            .on(registration("3", "chat2"))
            .expect("the listener registers");
        let duplicate = session.on(registration("2", "chat2"));
        assert!(
            duplicate
                .as_ref()
                .is_err_and(|error| error.to_string().contains("already registered")),
            "a repeated id was taken: {duplicate:?}"
        );
        let step = Arc::new(Barrier::new(3));
        let turning_off = {
            let (session, step) = (session.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                session.off(Value::Data(json!(["2"])))
            })
        };
        let ending = {
            let (session, step) = (session.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                session.row_ended("chat2");
            })
        };
        step.wait();
        turning_off.join().unwrap().unwrap();
        ending.join().unwrap();
        assert!(
            session.entries.lock().unwrap().is_empty(),
            "an entry stayed after the off and the row's end"
        );
        drop(entered);
        pump(&runtime);
        entered = runtime.enter();
        assert!(
            no_remote_entry(&root),
            "an entry stayed on the bus after the off and the row's end"
        );

        // The session leaving while a listener registers.
        session.row_loading("chat3", Vec::new(), "chat3".into());
        let step = Arc::new(Barrier::new(3));
        let registering = {
            let (session, step) = (session.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                session.on(registration("4", "chat3"))
            })
        };
        let leaving = {
            let (session, step) = (session.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                session.leave();
            })
        };
        step.wait();
        let registered = registering.join().unwrap();
        leaving.join().unwrap();
        if let Err(error) = registered {
            assert!(
                error.to_string().contains("not loaded"),
                "the registration ended as {error:?}"
            );
        }
        assert!(
            session.entries.lock().unwrap().is_empty(),
            "an entry stayed after the session left"
        );
        drop(entered);
        pump(&runtime);
        assert!(
            no_remote_entry(&root),
            "an entry stayed on the bus after the session left"
        );
    });
}

/// A run that records that it started, then finishes: what an emit's
/// dispatch does when it begins at once.
type Starts = Arc<std::sync::Mutex<Vec<&'static str>>>;

fn finished(label: &'static str, starts: &Starts) -> BoxFuture<'static, ()> {
    let starts = starts.clone();
    Box::pin(async move {
        starts.lock().unwrap().push(label);
    })
}

/// One `run_emit` call's cancel, and the label of its run, for reading the
/// queue's next run back as that label.
struct Emit {
    label: &'static str,
    cancel: Arc<Cancel>,
}

impl Emit {
    fn queued(label: &'static str) -> Self {
        Self {
            label,
            cancel: Arc::default(),
        }
    }
}

/// The label of the run queued as `run`.
fn label_of(emits: &[Emit], cancel: &Arc<Cancel>) -> &'static str {
    let at = Arc::as_ptr(cancel) as usize;
    emits
        .iter()
        .find(|emit| Arc::as_ptr(&emit.cancel) as usize == at)
        .map(|emit| emit.label)
        .expect("a queued run is one the model emitted")
}

/// #628: emits of one session start in the order they were queued, whichever
/// thread queues or finishes them. Each model thread emits once; the run's
/// dispatch starts at once (the first poll finishes it), so a run is either
/// started in its own `run_emit` or left to the session's task: the model
/// reads the queue and the running emit after the threads join, and the
/// task's starts must be exactly those, in that order.
#[test]
fn emits_start_in_the_order_they_were_queued() {
    const LABELS: [&str; 4] = ["a", "b", "c", "d"];
    explore(|| {
        let runtime = runtime();
        let entered = runtime.enter();
        let root = Ctx::root_with(runtime.handle().clone());
        let connection = Connection::inert(runtime.handle().clone());
        let session = SessionEvents::join(Some(&root), connection, "rt".into(), Vec::new())
            .expect("the session joins the bus");
        let starts: Starts = Arc::default();
        let emits: Arc<std::sync::Mutex<Vec<Emit>>> = Arc::default();
        let step = Arc::new(Barrier::new(LABELS.len() + 1));
        let threads: Vec<_> = LABELS
            .into_iter()
            .map(|label| {
                let (session, starts, emits, step) =
                    (session.clone(), starts.clone(), emits.clone(), step.clone());
                thread::spawn(move || {
                    let emit = Emit::queued(label);
                    step.wait();
                    let run = finished(label, &starts);
                    let own = EmitRun::new(run, emit.cancel.clone());
                    let accepted = session.run_emit(None, own);
                    let mut emits = emits.lock().unwrap();
                    emits.push(emit);
                    accepted
                })
            })
            .collect();
        step.wait();
        for spawned in threads {
            spawned.join().unwrap();
        }
        let emits = std::mem::take(&mut *emits.lock().unwrap());
        // What has started, and what the session's queue still holds: the
        // running emit (whose task has not run yet) and the queued ones, in
        // the order they must start in.
        let started = starts.lock().unwrap().clone();
        let (running, queued) = {
            let queue = session.emits.lock().unwrap();
            let running = queue
                .running
                .as_ref()
                .map(|(_, cancel)| label_of(&emits, cancel));
            let queued: Vec<&'static str> = queue
                .queue
                .iter()
                .map(|(_, run)| label_of(&emits, &run.cancel))
                .collect();
            (running, queued)
        };
        let mut expected = started.clone();
        expected.extend(running);
        expected.extend(queued);
        let mut all = expected.clone();
        all.sort_unstable();
        assert_eq!(all, LABELS, "an emit is neither started nor queued");
        drop(entered);
        pump(&runtime);
        let starts = starts.lock().unwrap().clone();
        assert_eq!(
            starts, expected,
            "the emits did not start in the order they were queued"
        );
        let emits = session.emits.lock().unwrap();
        assert!(
            !emits.busy && emits.queue.is_empty(),
            "the session's emits stayed busy"
        );
    });
}

/// A run that starts on its first poll and waits for the model to open a
/// gate: an emit whose dispatch has begun but not finished.
struct Gate {
    open: bool,
    waker: Option<Waker>,
}

fn gated(
    label: &'static str,
    starts: &Starts,
) -> (BoxFuture<'static, ()>, Arc<std::sync::Mutex<Gate>>) {
    let gate: Arc<std::sync::Mutex<Gate>> = Arc::new(std::sync::Mutex::new(Gate {
        open: false,
        waker: None,
    }));
    let (starts, held) = (starts.clone(), gate.clone());
    let run = poll_fn(move |cx| {
        let mut gate = held.lock().unwrap();
        if gate.open {
            return Poll::Ready(());
        }
        let mut starts = starts.lock().unwrap();
        if !starts.contains(&label) {
            starts.push(label);
        }
        gate.waker = Some(cx.waker().clone());
        Poll::Pending
    });
    (Box::pin(run), gate)
}

/// Open `gate` and wake the task waiting on it.
fn open(gate: &std::sync::Mutex<Gate>) {
    let mut gate = gate.lock().unwrap();
    gate.open = true;
    if let Some(waker) = gate.waker.take() {
        waker.wake();
    }
}

/// #628: an emit cancelled while it waits its turn starts no call, and one
/// that panics when it starts does not stop the emits after it: the queue
/// moves on and ends idle (§5.5). Another thread queues an emit meanwhile,
/// so the two orders of queueing are the model's.
#[test]
fn a_cancelled_or_panicking_emit_does_not_stop_the_queue() {
    explore(|| {
        let runtime = runtime();
        let entered = runtime.enter();
        let root = Ctx::root_with(runtime.handle().clone());
        let connection = Connection::inert(runtime.handle().clone());
        let session = SessionEvents::join(Some(&root), connection, "rt".into(), Vec::new())
            .expect("the session joins the bus");
        let starts: Starts = Arc::default();
        let emits: Arc<std::sync::Mutex<Vec<Emit>>> = Arc::default();

        // The emit that runs: its dispatch starts, then hangs until the gate
        // opens.
        let (run, gate) = gated("a", &starts);
        assert!(!session.run_emit(None, EmitRun::new(run, Arc::default())));
        assert_eq!(*starts.lock().unwrap(), vec!["a"], "the first emit started");

        // Queued behind it: one cancelled before its turn, and a panicking
        // one; another thread queues one of its own meanwhile, and this one
        // queues the last.
        let cancelled = Emit::queued("b");
        let run = finished("b", &starts);
        assert!(!session.run_emit(None, EmitRun::new(run, cancelled.cancel.clone())));
        cancelled.cancel.cancel();
        emits.lock().unwrap().push(cancelled);
        let step = Arc::new(Barrier::new(2));
        let other = {
            let (session, starts, emits, step) =
                (session.clone(), starts.clone(), emits.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                let emit = Emit::queued("c");
                let run = finished("c", &starts);
                session.run_emit(None, EmitRun::new(run, emit.cancel.clone()));
                emits.lock().unwrap().push(emit);
            })
        };
        step.wait();
        let panic = Emit::queued("p");
        let panics: BoxFuture<'static, ()> = Box::pin(async { panic!("emit panics") });
        assert!(!session.run_emit(None, EmitRun::new(panics, panic.cancel.clone())));
        emits.lock().unwrap().push(panic);
        let last = Emit::queued("d");
        let run = finished("d", &starts);
        assert!(!session.run_emit(None, EmitRun::new(run, last.cancel.clone())));
        emits.lock().unwrap().push(last);
        other.join().unwrap();

        // What must start: `a` and then the queue in its order, without the
        // cancelled and the panicking emits. The running emit is `a` itself.
        let emits = std::mem::take(&mut *emits.lock().unwrap());
        let mut expected = vec!["a"];
        for (_, run) in session.emits.lock().unwrap().queue.iter() {
            let label = label_of(&emits, &run.cancel);
            if label != "b" && label != "p" {
                expected.push(label);
            }
        }
        let mut all = expected.clone();
        all.sort_unstable();
        assert_eq!(all, vec!["a", "c", "d"], "the model queued other emits");

        open(&gate);
        drop(entered);
        pump(&runtime);
        assert_eq!(
            *starts.lock().unwrap(),
            expected,
            "the cancelled emit started or the queue stopped"
        );
        let emits = session.emits.lock().unwrap();
        assert!(
            !emits.busy && emits.queue.is_empty(),
            "the session's emits stayed busy"
        );
    });
}
