//! Shuttle model of a session's event host (#628): a row's listeners
//! register while the row unloads or ends, an `events.off` races the row's
//! end, and the session leaves while a listener is registering.
//!
//! The model runs the production `SessionEvents::{on, off, row_loading,
//! row_unloading, row_ended, leave}` and the kernel's real listener table:
//! `crate::sync::Mutex` makes the session's `rows`, `entries` and `emits`
//! shuttle's, so an `on` that has taken the row but not yet recorded its
//! entry and a `row_unloading` that wants the row interleave there. The
//! session's connection is inert (`Connection::inert`): no frame is sent,
//! but the host records and withdraws the entry as it always does.
//!
//! What stands in for the runtime: nothing runs a call on the entry, so
//! "in flight" here is a registration or a session leave already inside
//! the host's locks, not a listener call. A listener call would wait for
//! the far end's reply, which an inert connection never sends.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```

use std::sync::Arc;

use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::Ctx;
use serde_json::json;
use shuttle::sync::Barrier;
use shuttle::thread;
use tokio::runtime::Builder;

use super::SessionEvents;
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
