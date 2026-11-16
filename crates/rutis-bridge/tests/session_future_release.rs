//! #1216: a session that ends while a call that returns a future is being
//! answered releases its state.
//!
//! The answer is encoded after the session ended, so it can never be
//! written. If the reference it grants is left in the export table, the
//! table holds the future, the future holds the connection that made it
//! (`with_caller`), and the connection holds the table: a cycle with no
//! outside reference, so the session is never released.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rutis_bridge::channel::{Channel, PeerId};
use rutis_bridge::session::{
    Connection, Dispatch, Endpoint, Error, Reply, Value, ENDPOINT_PROTOCOL,
};
use serde_json::{json, Value as Json};

/// Serves one call: says it started, waits for the test to let it go, and
/// returns a future (made while the call is dispatched, so it captures the
/// connection: `Value::future`).
struct Slow {
    started: mpsc::Sender<()>,
    go: Mutex<mpsc::Receiver<()>>,
}

impl Dispatch for Slow {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        self.started.send(()).unwrap();
        self.go
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::Transport("the test did not let the dispatch go".into()))?;
        Ok(Value::future(async { Ok(Value::Undefined) }))
    }
}

fn read(channel: &mut Channel) -> Json {
    serde_json::from_slice(&channel.receiver.recv().unwrap().unwrap()).unwrap()
}

fn send(channel: &mut Channel, frame: Json) {
    channel
        .sender
        .send(&serde_json::to_vec(&frame).unwrap())
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_ends_while_a_future_is_answered_is_released() {
    let (local, mut remote) = rutis_bridge::memory::pair();
    let (started, started_rx) = mpsc::channel();
    let (go, go_rx) = mpsc::channel();
    let dispatch = Arc::new(Slow {
        started,
        go: Mutex::new(go_rx),
    });
    let released = Arc::downgrade(&dispatch);
    let session = Connection::open(
        local,
        dispatch.clone(),
        Endpoint::rust(PeerId::new("slow").unwrap()).expect(PeerId::new("main").unwrap()),
    )
    .unwrap();
    drop(dispatch);

    // The far end greets, so the session is ready for calls.
    let hello = read(&mut remote);
    assert_eq!(
        (&hello["op"], &hello["version"]),
        (&json!("hello"), &json!(ENDPOINT_PROTOCOL))
    );
    send(
        &mut remote,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "main" }),
    );
    session.ready().await.unwrap();

    // A call the far end makes: its dispatch starts and blocks.
    send(
        &mut remote,
        json!({
            "op": "invoke",
            "id": "main:1",
            "path": [],
            "target": "slow",
            "method": "work",
            "args": { "type": "list", "value": [] },
            "sync": false,
        }),
    );
    started_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the dispatch started");

    // The session ends before the dispatch finishes: a release of an
    // unknown reference is a protocol error.
    send(
        &mut remote,
        json!({ "op": "release", "reference": 99, "count": 1 }),
    );
    session.closed().await;
    // It is closed: the export table has been taken and dropped.
    assert!(session.close_reason().is_some());

    // Now the dispatch answers with a future of its own.
    go.send(()).unwrap();
    drop(session);
    drop(remote);

    // The session state (and with it the dispatch) is released: nothing
    // holds the future, or the connection it captured, any more.
    let deadline = Instant::now() + Duration::from_secs(5);
    while released.upgrade().is_some() {
        assert!(
            Instant::now() < deadline,
            "the session state is still alive after the session ended: \
             the answer's future was put back in the cleared export table (#1216)"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
