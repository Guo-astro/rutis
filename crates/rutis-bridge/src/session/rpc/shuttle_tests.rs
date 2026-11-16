//! Shuttle models of a session (#628): its call and reference tables, the
//! claims of a call the far end sends back, the ordered event operations, and
//! the state of a future this side runs.
//!
//! A session's reader, its waiting threads and the tasks that answer calls
//! all touch the same tables: `Calls` (what this side waits for, what the
//! far end may still answer, what it cancelled, what queued calls it has not
//! claimed), `Exports`/`Imports` (what a reference id means on each side), the
//! ordered queue of event operations and a future's `FutureState`. The models
//! run the production tables and the production `Connection::send`, `receive`,
//! `cancel`, `close`, `execute`, `encode` and `decode`; `crate::sync::Mutex`
//! makes those tables shuttle's under `--cfg rutis_shuttle`, so a `send` and a
//! reply, a claim and another claimant, or a waiter giving up and the task it
//! would abort interleave at every lock.
//!
//! What stands in for the channel: the models take the frames `send` and
//! `respond_to` queued and hand them to the other `Connection` through
//! `receive`, as a reader thread would. `Connection::inert` builds the two
//! ends without a reader, writer or dispatched task; the frames are encoded
//! and decoded by the production code. A far end's call is executed by the
//! model with `Connection::execute`, which is what the session's executor
//! would do; the models that race a spawned task drive the session's runtime
//! with `Runtime::block_on`, so the task the reader spawned runs where the
//! production session would run it.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! `RUTIS_SHUTTLE_ITERATIONS` sets the iterations of each scheduler: 1000
//! by default, as on pull requests. A failure prints its schedule;
//! `RUTIS_SHUTTLE_REPLAY=<schedule>` replays it, with the failing test's name
//! as the filter.
//!
//! guarantee: bridge/in-flight-call-fails-once, bridge/replies-match-calls, bridge/cancel-reaches-callee, bridge/release-count, events/hub-lifecycle

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

use serde_json::json;
use shuttle::sync::Barrier;
use shuttle::thread;
use tokio::runtime::Builder;
use tokio::sync::oneshot;

use super::{
    parse_frame, Body, Connection, Dispatch, Error, Frame, Message, Operation, Reference,
    ReferenceInner, Reply, Value, Waiter, Waiting, WireValue, MAX_DEPTH,
};
use crate::sync::Mutex as ModelMutex;

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

/// A runtime whose handle the inert sessions name; no task is driven.
fn runtime() -> tokio::runtime::Runtime {
    Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
}

/// The frames `conn` queued, in send order, as a reader of the far end
/// would take them.
fn frames(conn: &Connection) -> Vec<Frame> {
    let bytes: Vec<Vec<u8>> = {
        let mut queued = conn.0.outbound.queued.lock().unwrap();
        let bytes = queued.frames.drain(..).collect();
        queued.bytes = 0;
        bytes
    };
    bytes
        .into_iter()
        .map(|bytes| {
            let parsed = parse_frame(&bytes).expect("a frame this side wrote parses");
            assert!(
                parsed.ignored.is_empty(),
                "this side wrote unknown fields: {:?}",
                parsed.ignored
            );
            parsed.frame
        })
        .collect()
}

/// #628: a call's reply and its cancellation race, and a session that
/// closes while a reply is on its way: the call ends exactly once, a late
/// reply is discarded as an orphan, and no table keeps anything afterwards.
#[test]
fn a_call_is_answered_or_cancelled_once() {
    explore(|| {
        let runtime = runtime();
        let client = Connection::inert(runtime.handle().clone());

        // The caller cancels and the far end replies at the same time.
        let (sender, mut receiver) = oneshot::channel();
        let id = client
            .send(
                Operation::Invoke("svc".into(), "go".into(), Value::Undefined),
                Waiting::Async(sender),
            )
            .expect("the session is ready");
        let step = Arc::new(Barrier::new(2));
        let replying = {
            let (client, id, step) = (client.clone(), id.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                let _ = client.receive(Frame::Return {
                    id,
                    value: WireValue::Undefined,
                });
            })
        };
        step.wait();
        client.cancel(&id);
        replying.join().unwrap();

        let answered = receiver.try_recv().is_ok();
        let calls = client.0.calls.lock().unwrap();
        assert!(
            calls.waiting.is_empty() && calls.cancelled.is_empty() && calls.incoming.is_empty(),
            "a call's tables kept an entry after its one outcome"
        );
        assert_eq!(
            calls.orphans,
            if answered { 0 } else { 1 },
            "a late reply was not discarded exactly once"
        );
        drop(calls);

        // A close racing a reply: the call ends once, with the reply or
        // with the close, and the tables are empty when both are done.
        let (sender, mut receiver) = oneshot::channel();
        let id = client
            .send(
                Operation::Invoke("svc".into(), "go".into(), Value::Undefined),
                Waiting::Async(sender),
            )
            .expect("the session is ready");
        let step = Arc::new(Barrier::new(4));
        let replying = {
            let (client, id, step) = (client.clone(), id.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                let _ = client.receive(Frame::Return {
                    id,
                    value: WireValue::Data(serde_json::json!(7)),
                });
            })
        };
        let closing = {
            let (client, step) = (client.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                client.close(Error::Transport("gone".into()));
            })
        };
        let (sender, mut sent_receiver) = oneshot::channel();
        let sending = {
            let (client, step) = (client.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                client.send(
                    Operation::Invoke("svc".into(), "other".into(), Value::Undefined),
                    Waiting::Async(sender),
                )
            })
        };
        step.wait();
        replying.join().unwrap();
        closing.join().unwrap();
        let sent = sending.join().unwrap();
        if sent.is_ok() {
            // The close won the queue: the call it let through is ended by
            // the close, exactly once.
            match sent_receiver.try_recv() {
                Ok(Err(Error::Transport(reason))) => assert_eq!(reason, "gone"),
                other => panic!("a call sent while closing ended as {other:?}"),
            }
        }

        let outcome = receiver
            .try_recv()
            .expect("the waiting call ended exactly once");
        match outcome {
            Ok(Value::Data(value)) => assert_eq!(value, serde_json::json!(7)),
            Err(Error::Transport(reason)) => assert_eq!(reason, "gone"),
            other => panic!("the call ended as {other:?}"),
        }
        assert!(client.close_reason().is_some(), "the session did not end");
        let calls = client.0.calls.lock().unwrap();
        assert!(
            calls.waiting.is_empty() && calls.cancelled.is_empty() && calls.incoming.is_empty(),
            "the closed session kept a call"
        );
        drop(calls);
        assert!(
            client.0.exports.lock().unwrap().entries.is_empty(),
            "the closed session kept an export"
        );
        assert!(
            client.0.imports.lock().unwrap().is_empty(),
            "the closed session kept an import"
        );
        assert!(
            client
                .send(
                    Operation::Invoke("svc".into(), "go".into(), Value::Undefined),
                    Waiting::Async(oneshot::channel().0),
                )
                .is_err(),
            "a closed session sent a call"
        );
    });
}

/// Answers every call with the same function reference.
struct Shared {
    function: Value,
    calls: AtomicUsize,
}

impl Dispatch for Shared {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.function.clone())
    }
}

/// #628: a reference granted in a reply is imported once per grant and
/// released once; an encode that fails after granting gives the grant back;
/// a close clears both tables even while a reply is being encoded.
#[test]
fn references_are_granted_released_and_cleared_once() {
    explore(|| {
        let runtime = runtime();
        let _entered = runtime.enter();
        let function = Value::callback(|_| Ok(Value::Undefined));
        let shared = Arc::new(Shared {
            function: function.clone(),
            calls: AtomicUsize::new(0),
        });
        let (client, server) =
            Connection::inert_pair_with(shared.clone(), runtime.handle().clone());

        // Two calls, one export: the server's identity table grants the
        // same id twice, and the client imports it twice.
        let mut waiters = Vec::new();
        for _ in 0..2 {
            let (sender, receiver) = oneshot::channel();
            client
                .send(
                    Operation::Invoke("svc".into(), "go".into(), Value::Undefined),
                    Waiting::Async(sender),
                )
                .expect("the session is ready");
            waiters.push(receiver);
        }
        for frame in frames(&client) {
            server.receive(frame).expect("the server reads the call");
        }
        let ids: Vec<String> = server
            .0
            .calls
            .lock()
            .unwrap()
            .incoming
            .keys()
            .cloned()
            .collect();
        assert_eq!(ids.len(), 2, "the server did not take both calls");
        for id in ids {
            let incoming = server.0.calls.lock().unwrap().incoming.remove(&id);
            server.execute(incoming.expect("an incoming call"), false);
        }
        assert_eq!(
            shared.calls.load(Ordering::SeqCst),
            2,
            "the server did not run both calls"
        );
        let replies = frames(&server);
        assert_eq!(replies.len(), 2, "the server did not answer both calls");
        let step = Arc::new(Barrier::new(3));
        let mut receiving = Vec::new();
        for frame in replies {
            let (client, step) = (client.clone(), step.clone());
            receiving.push(thread::spawn(move || {
                step.wait();
                client.receive(frame)
            }));
        }
        step.wait();
        for receive in receiving {
            receive.join().unwrap().expect("the client reads the reply");
        }
        assert_eq!(
            server.0.exports.lock().unwrap().entries.len(),
            1,
            "the same function was exported twice"
        );
        let mut values = Vec::new();
        for mut receiver in waiters {
            match receiver
                .try_recv()
                .expect("the call ended")
                .expect("the call succeeded")
            {
                Value::Reference(reference) => values.push(reference),
                other => panic!("the reply was {other:?}, not a reference"),
            }
        }
        assert_eq!(values.len(), 2, "the client did not import both references");
        assert_eq!(
            client.0.imports.lock().unwrap().len(),
            1,
            "one far reference id became two imports"
        );

        // The two imports drop together: one release, count two, removes
        // the export. The last `Import` to go is the one that writes it.
        let step = Arc::new(Barrier::new(3));
        let mut dropping = Vec::new();
        for value in values {
            let step = step.clone();
            dropping.push(thread::spawn(move || {
                step.wait();
                drop(value);
            }));
        }
        step.wait();
        for drop in dropping {
            drop.join().unwrap();
        }
        let releases = frames(&client);
        assert_eq!(releases.len(), 1, "the imports wrote more than one release");
        for frame in releases {
            server.receive(frame).expect("the server reads the release");
        }
        assert!(
            server.0.exports.lock().unwrap().entries.is_empty(),
            "the released function stayed exported"
        );
        assert!(
            client.0.imports.lock().unwrap().is_empty(),
            "the released import stayed in the table"
        );

        // An encode that grants and then fails gives the grant back: the
        // function is granted, then the bytes value fails because this far
        // end declared no capability for it.
        let leaky = Value::List(vec![function.clone(), Value::Bytes(vec![1])]);
        let error = match server.encode_granting(&leaky, MAX_DEPTH) {
            Ok(_) => panic!("the value must not encode"),
            Err(error) => error,
        };
        assert!(
            matches!(error, Error::Value { .. }),
            "the encode failed as {error:?}"
        );
        assert!(
            server.0.exports.lock().unwrap().entries.is_empty(),
            "a failed encode kept the reference it granted"
        );

        // A close racing a reply that grants a reference: no export, no
        // import, and the call ends once.
        let (sender, mut receiver) = oneshot::channel();
        client
            .send(
                Operation::Invoke("svc".into(), "go".into(), Value::Undefined),
                Waiting::Async(sender),
            )
            .expect("the session is ready");
        for frame in frames(&client) {
            server.receive(frame).expect("the server reads the call");
        }
        let id = server
            .0
            .calls
            .lock()
            .unwrap()
            .incoming
            .keys()
            .next()
            .cloned()
            .expect("a call waits on the server");
        let step = Arc::new(Barrier::new(4));
        let replying = {
            let (server, id, step) = (server.clone(), id.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                let incoming = server.0.calls.lock().unwrap().incoming.remove(&id);
                if let Some(incoming) = incoming {
                    server.execute(incoming, false);
                }
            })
        };
        let closing = {
            let (server, step) = (server.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                server.close(Error::Transport("server gone".into()));
            })
        };
        let client_closing = {
            let (client, step) = (client.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                client.close(Error::Transport("client gone".into()));
            })
        };
        step.wait();
        replying.join().unwrap();
        closing.join().unwrap();
        client_closing.join().unwrap();
        assert!(
            server.0.exports.lock().unwrap().entries.is_empty(),
            "the closed session kept the export its reply granted"
        );
        assert!(
            server.0.imports.lock().unwrap().is_empty(),
            "the closed session kept an import"
        );
        match receiver.try_recv().expect("the call ended exactly once") {
            Ok(_) => {}
            Err(Error::Transport(reason)) => {
                assert!(
                    reason == "client gone" || reason == "server gone",
                    "the call ended with {reason}"
                )
            }
            other => panic!("the call ended as {other:?}"),
        }
    });
}

/// Records what a session dispatched: the method, and the sequence an
/// `events.*` operation carries in its argument.
#[derive(Default)]
struct Recorder {
    calls: ModelMutex<Vec<(String, Option<u64>)>>,
}

impl Recorder {
    fn recorded(&self) -> Vec<(String, Option<u64>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Dispatch for Recorder {
    fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
        let sequence = match &args {
            Value::Data(value) => value.as_u64(),
            _ => None,
        };
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), sequence));
        Ok(Value::Undefined)
    }
}

/// #628: a call the far end sends back while this side's own synchronous
/// wait registers is claimed exactly once — by the reader that finds the
/// waiter, by the waiting thread that finds the queued call, or by the
/// executor task the reader spawned — and runs once, for the one that
/// claimed it.
#[test]
fn an_incoming_call_is_claimed_once() {
    explore(|| {
        let runtime = Arc::new(runtime());
        let recorder = Arc::new(Recorder::default());
        let (client, server) =
            Connection::inert_pair_with(recorder.clone(), runtime.handle().clone());
        let id = format!("{}1", server.remote_prefix());
        let frame = Frame::Invoke {
            id: id.clone(),
            path: Vec::new(),
            target: "svc".into(),
            method: "go".into(),
            args: WireValue::Undefined,
            sync: false,
        };
        let (sender, receiver) = mpsc::channel();
        let step = Arc::new(Barrier::new(3));
        let receiving = {
            let (server, step) = (server.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                server.receive(frame)
            })
        };
        let waiting = {
            let (server, step, id) = (server.clone(), step.clone(), id.clone());
            thread::spawn(move || {
                step.wait();
                server
                    .send(
                        Operation::Invoke("svc".into(), "other".into(), Value::Undefined),
                        Waiting::Sync {
                            sender,
                            origins: vec![id],
                        },
                    )
                    .expect("the session is ready")
            })
        };
        let driving = {
            let runtime = runtime.clone();
            thread::spawn(move || {
                step.wait();
                for _ in 0..4 {
                    runtime.block_on(async {
                        for _ in 0..2 {
                            tokio::task::yield_now().await;
                        }
                    });
                    thread::yield_now();
                }
            })
        };
        receiving
            .join()
            .unwrap()
            .expect("the reader takes the call");
        waiting.join().unwrap();
        driving.join().unwrap();
        // What reached the waiting thread runs as `request_sync` runs it.
        if let Ok(Message::Invoke(incoming)) = receiver.try_recv() {
            server.execute(incoming, true);
        }
        // The reader may have spawned the claim task after the executor
        // stopped.
        runtime.block_on(async {
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        });
        let recorded = recorder.recorded();
        assert_eq!(
            recorded.len(),
            1,
            "the call ran {} times: {recorded:?}",
            recorded.len()
        );
        assert_eq!(recorded[0].0, "go", "the waiter's own call ran here");
        assert!(
            receiver.try_recv().is_err(),
            "the waiting thread was handed the call twice"
        );
        assert!(
            server.0.calls.lock().unwrap().incoming.is_empty(),
            "the claimed call stayed queued"
        );
        // The waiter's own call is never answered: the hop its `send`
        // recorded leaves with the sessions, and the table stays the size
        // of one call (a forward scans every hop).
        super::waits::ended(server.tag());
        super::waits::ended(client.tag());
    });
}

/// #628: the session's event operations each run exactly once, in arrival
/// order, and the ordered task leaves the queue empty and `busy` clear.
#[test]
fn event_operations_take_their_turn_once() {
    explore(|| {
        let runtime = Arc::new(runtime());
        let recorder = Arc::new(Recorder::default());
        let (_client, server) =
            Connection::inert_pair_with(recorder.clone(), runtime.handle().clone());
        let step = Arc::new(Barrier::new(2));
        let receiving = {
            let (server, step) = (server.clone(), step.clone());
            thread::spawn(move || {
                step.wait();
                for sequence in 1..=3 {
                    server
                        .receive(Frame::Invoke {
                            id: format!("{}{sequence}", server.remote_prefix()),
                            path: Vec::new(),
                            target: String::new(),
                            method: "events.on".into(),
                            args: WireValue::Data(json!(sequence)),
                            sync: false,
                        })
                        .expect("the reader takes the event operation");
                }
            })
        };
        let driving = {
            let runtime = runtime.clone();
            thread::spawn(move || {
                step.wait();
                for _ in 0..3 {
                    runtime.block_on(async {
                        for _ in 0..8 {
                            tokio::task::yield_now().await;
                        }
                    });
                    thread::yield_now();
                }
            })
        };
        receiving.join().unwrap();
        driving.join().unwrap();
        runtime.block_on(async {
            for _ in 0..32 {
                tokio::task::yield_now().await;
            }
        });
        let recorded = recorder.recorded();
        let sequences: Vec<Option<u64>> = recorded.iter().map(|(_, sequence)| *sequence).collect();
        assert_eq!(
            sequences,
            vec![Some(1), Some(2), Some(3)],
            "the event operations ran as {recorded:?}"
        );
        let ordered = server.0.ordered.lock().unwrap();
        assert!(
            !ordered.busy && ordered.queue.is_empty(),
            "the ordered task left work behind"
        );
    });
}

/// Poll `future` once with a no-op waker.
fn poll_once<F: std::future::Future>(future: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    let waker = std::task::Waker::noop();
    std::future::Future::poll(future, &mut std::task::Context::from_waker(waker))
}

/// #628: a future's state once it runs (`FutureState`): the sync await that
/// starts the task records its handle, and when the last waiter gives up
/// the task is aborted — exactly once, by the last one.
#[test]
fn the_last_waiter_gives_up_on_the_future() {
    explore(|| {
        let runtime = runtime();
        let _entered = runtime.enter();
        let value = Value::cancellable_future(std::future::pending());
        let Value::Reference(Reference(ReferenceInner::Local(object))) = value else {
            panic!("a future is a local reference");
        };
        let Body::Future(state) = &object.body else {
            panic!("a future reference is a future");
        };
        assert!(
            object.ready_on_blocked_executor().is_err(),
            "a blocked executor waits for nothing"
        );
        assert!(
            state.future.lock().unwrap().task.is_some(),
            "the sync await did not record the task it started"
        );

        // Two waiters, given up together: the last one aborts the task.
        let step = Arc::new(Barrier::new(3));
        let mut dropping = Vec::new();
        for _ in 0..2 {
            let (object, step) = (object.clone(), step.clone());
            dropping.push(thread::spawn(move || {
                let mut waiting = Box::pin(object.wait(false, Vec::new(), None));
                assert!(
                    poll_once(waiting.as_mut()).is_pending(),
                    "a future that never finishes finished"
                );
                step.wait();
                drop(waiting);
            }));
        }
        step.wait();
        for drop in dropping {
            drop.join().unwrap();
        }
        let future = state.future.lock().unwrap();
        assert_eq!(future.waiters, 0, "a waiter was left counted");
        assert!(
            future.task.is_none(),
            "the last waiter did not abort the task it left running"
        );
        drop(future);
        assert!(
            state.result.borrow().is_none(),
            "the future finished after its last waiter gave up"
        );
    });
}

/// #628, #1250: a future started by a sync await that cannot wait for it is
/// cancelled when the last other waiter gives up, even while the sync await
/// is between taking the future and recording the task it started
/// (`ready_on_blocked_executor`): the drop finds `task` empty, and the task
/// spawned afterwards is aborted instead of stored, so the future does not
/// run with nobody waiting for it.
///
/// The window is reachable: the future's own first poll runs there, and a
/// poll may take any amount of work (a call into another session, a
/// plugin's future). The model puts the waiter's drop inside it with a
/// barrier.
#[test]
fn a_future_started_as_its_last_waiter_leaves_is_cancelled() {
    explore(|| {
        let runtime = runtime();
        let _entered = runtime.enter();
        let step = Arc::new(Barrier::new(2));
        let first = {
            let step = step.clone();
            async move {
                // The first poll of the future: the sync await is now
                // between taking it and recording the task it starts.
                step.wait();
                std::future::pending().await
            }
        };
        let value = Value::cancellable_future(first);
        let Value::Reference(Reference(ReferenceInner::Local(object))) = value else {
            panic!("a future is a local reference");
        };
        let Body::Future(state) = &object.body else {
            panic!("a future reference is a future");
        };

        // The one waiter the future has, given up inside that poll.
        let waiting = {
            let (object, step) = (object.clone(), step.clone());
            thread::spawn(move || {
                let Body::Future(state) = &object.body else {
                    panic!("a future reference is a future");
                };
                state.future.lock().unwrap().waiters += 1;
                let waiter = Waiter(state);
                step.wait();
                drop(waiter);
            })
        };
        // The sync await: it takes the future, polls it (where the waiter
        // leaves) and only then records the task.
        assert!(
            object.ready_on_blocked_executor().is_err(),
            "a blocked executor waits for nothing"
        );
        waiting.join().unwrap();

        let future = state.future.lock().unwrap();
        assert_eq!(future.waiters, 0, "a waiter was left counted");
        assert!(
            future.task.is_none(),
            "the future runs with nobody waiting for it: the abort handle was recorded after \
             the last waiter gave up"
        );
        assert!(
            state.result.borrow().is_none(),
            "the future finished after its last waiter gave up"
        );
    });
}
