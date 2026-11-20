//! Timing promises from the guarantee registry, checked on a paused clock
//! (`tokio::time::start_paused`): a deadline is reached by advancing virtual
//! time, never by sleeping, so the checks are exact and cost nothing.
//!
//! Only promises whose timer is the runtime's own can be checked this way.
//! The kernel's other timers keep time on a thread of their own, not with
//! `tokio::time` (a plugin's long wait for its services, and the wait for
//! event calls at unload), so they are out of this file's reach.
//!
//! guarantee: boundary/5.8
//! guarantee: host/stop-deadline
//! guarantee: core/features.shutdown-timeout-names-stuck

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use rutis::host::{shutdown_deadline, CtxHost, DisposeWaitError, DynPlugin, Effect};
use rutis::{Ctx, Error as RutisError};
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::{Connection, Dispatch, Endpoint, Reply, Value, ENDPOINT_PROTOCOL};
use serde_json::{json, Value as Json};

/// An hour: long enough that a timeout of any length the compatibility
/// layer might add would have fired.
const HOUR: Duration = Duration::from_secs(3600);

fn id(name: &str) -> PeerId {
    PeerId::new(name).unwrap()
}

fn session(dispatch: Arc<dyn Dispatch>) -> (Connection, rutis_bridge::Channel) {
    let (ours, theirs) = rutis_bridge::memory::pair();
    let peer = Connection::open(ours, dispatch, Endpoint::rust(id("main"))).unwrap();
    (peer, theirs)
}

fn read(channel: &mut rutis_bridge::Channel) -> Json {
    match channel.receiver.recv().unwrap() {
        Some(message) => serde_json::from_slice(&message).unwrap(),
        None => panic!("the far end read the channel's end"),
    }
}

fn send(channel: &mut rutis_bridge::Channel, frame: Json) {
    channel
        .sender
        .send(&serde_json::to_vec(&frame).unwrap())
        .unwrap();
}

fn frame_id(frame: &Json) -> String {
    frame["id"].as_str().expect("an id").to_owned()
}

/// Answer this side's greeting as `node`, so the session is ready.
fn greet(channel: &mut rutis_bridge::Channel) {
    let hello = read(channel);
    assert_eq!(hello["op"], json!("hello"), "{hello}");
    send(
        channel,
        json!({ "op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "node" }),
    );
}

/// Serves `later`: a future that ends only when the test lets it, saying
/// when it was first polled.
struct Deferred {
    polled: tokio::sync::mpsc::UnboundedSender<()>,
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl Dispatch for Deferred {
    fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
        match method {
            "later" => {
                let gate = self.gate.lock().unwrap().take().expect("one call");
                let polled = self.polled.clone();
                Ok(Value::future(async move {
                    let _ = polled.send(());
                    let _ = gate.await;
                    Ok(Value::Data(json!("late")))
                }))
            }
            _ => Ok(args),
        }
    }
}

/// The compatibility layer adds no timeout of its own to a call: one in a
/// call out, and one the far end makes into this side, both outlive an hour
/// of the clock and are answered.
/// guarantee: boundary/5.8
#[tokio::test(start_paused = true)]
async fn an_in_flight_call_is_given_no_timeout() {
    let (polled, mut polled_at) = tokio::sync::mpsc::unbounded_channel();
    let (gate, held) = tokio::sync::oneshot::channel();
    let (peer, mut far) = session(Arc::new(Deferred {
        polled,
        gate: Mutex::new(Some(held)),
    }));
    greet(&mut far);
    peer.ready().await.unwrap();

    // A call this side makes: the far end answers only once an hour has
    // passed.
    let (reached, far_saw) = mpsc::channel();
    let (answer, answered) = mpsc::channel();
    let far_end = std::thread::spawn(move || {
        let call = read(&mut far);
        reached.send(()).unwrap();
        answered.recv().unwrap();
        send(
            &mut far,
            json!({
                "op": "return",
                "id": frame_id(&call),
                "value": { "type": "data", "value": "still waiting" },
            }),
        );
        far
    });
    let call = tokio::spawn({
        let peer = peer.clone();
        async move {
            peer.invoke_async("conformance", "slow", Value::Data(json!([])))
                .await
        }
    });
    tokio::task::spawn_blocking(move || far_saw.recv().unwrap())
        .await
        .unwrap();
    tokio::time::advance(HOUR).await;
    answer.send(()).unwrap();
    let value = call.await.unwrap().expect("the call is not given up on");
    assert_eq!(value.json().unwrap(), json!("still waiting"));

    // A call the far end makes into this side: it gets a future back,
    // awaits it, and is answered after another hour.
    let far = tokio::task::spawn_blocking(move || {
        let mut far = far_end.join().unwrap();
        send(
            &mut far,
            json!({
                "op": "invoke",
                "id": "node:9",
                "path": [],
                "target": "conformance",
                "method": "later",
                "args": { "type": "data", "value": [] },
            }),
        );
        let returned = read(&mut far);
        assert_eq!(returned["op"], json!("return"), "{returned}");
        let reference = returned["value"]["value"]["id"].as_u64().expect("a future");
        send(
            &mut far,
            json!({
                "op": "await",
                "id": "node:10",
                "path": [],
                "reference": reference,
                "sync": false,
            }),
        );
        // Answered once the hour has passed and the method is let go.
        let reply = read(&mut far);
        (reply, far)
    });
    polled_at.recv().await.expect("the method is polled");
    tokio::time::advance(HOUR).await;
    gate.send(()).unwrap();
    let (reply, _far) = far.await.unwrap();
    assert_eq!(frame_id(&reply), "node:10");
    assert_eq!(reply["op"], json!("return"), "{reply}");
    assert_eq!(reply["value"]["value"], json!("late"));
}

/// The cleanup deadline is 10 s unless `RUTIS_SHUTDOWN_TIMEOUT` says
/// otherwise; a value that is not a number of seconds above 0 leaves the
/// default. One test: the variable belongs to the process.
/// guarantee: host/stop-deadline
#[test]
fn the_cleanup_deadline_is_ten_seconds_unless_set() {
    std::env::remove_var("RUTIS_SHUTDOWN_TIMEOUT");
    assert_eq!(shutdown_deadline(), Duration::from_secs(10));
    std::env::set_var("RUTIS_SHUTDOWN_TIMEOUT", "0.25");
    assert_eq!(shutdown_deadline(), Duration::from_millis(250));
    std::env::set_var("RUTIS_SHUTDOWN_TIMEOUT", "3");
    assert_eq!(shutdown_deadline(), Duration::from_secs(3));
    for bad in ["0", "-1", "no", "NaN", "inf", ""] {
        std::env::set_var("RUTIS_SHUTDOWN_TIMEOUT", bad);
        assert_eq!(
            shutdown_deadline(),
            Duration::from_secs(10),
            "{bad:?} is not a deadline"
        );
    }
    std::env::remove_var("RUTIS_SHUTDOWN_TIMEOUT");
}

/// A plugin whose `apply` ends only when the test lets it.
struct Stuck {
    started: Arc<AtomicBool>,
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl DynPlugin for Stuck {
    fn name(&self) -> &str {
        "stuck"
    }

    fn apply<'a>(&'a self, _: &'a Ctx) -> rutis::host::BoxFuture<'a, Result<Effect, RutisError>> {
        let started = self.started.clone();
        let gate = self.gate.lock().unwrap().take().expect("one apply");
        Box::pin(async move {
            started.store(true, Ordering::SeqCst);
            let _ = gate.await;
            Ok(Effect::Done)
        })
    }
}

/// A shutdown that runs out of time returns at the deadline and names the
/// plugin still inside its `apply`; the shutdown goes on and finishes once
/// that plugin does.
/// guarantee: core/features.shutdown-timeout-names-stuck
#[tokio::test(start_paused = true)]
async fn a_shutdown_deadline_is_reached_on_the_clock_and_names_the_plugin() {
    let root = Ctx::root().unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let (gate, held) = tokio::sync::oneshot::channel();
    root.plugin(Stuck {
        started: started.clone(),
        gate: Mutex::new(Some(held)),
    });
    while !started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    let began = tokio::time::Instant::now();
    let error = root
        .shutdown_with_timeout(Duration::from_secs(10))
        .await
        .expect_err("the plugin does not end");
    assert_eq!(
        began.elapsed(),
        Duration::from_secs(10),
        "the deadline is where the registry says"
    );
    let DisposeWaitError::TimedOut { stuck, .. } = &error else {
        panic!("{error}");
    };
    assert_eq!(stuck.len(), 1, "{stuck:?}");
    assert_eq!(stuck[0].plugin, "stuck");
    assert_eq!(stuck[0].doing, "its apply has not returned");
    // The shutdown went on in the background: it finishes once the plugin
    // does.
    gate.send(()).unwrap();
    root.shutdown_with_timeout(Duration::from_secs(10))
        .await
        .expect("the shutdown finishes once the plugin ends");
}
