use super::*;
use crate::channel::Receiver;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A compat-format greeting of a 0.13 far end (`events-v2`, §2.1).
fn hello(version: u32) -> Frame {
    Frame::Hello {
        version,
        endpoint: None,
        implementation: None,
        capabilities: Some(vec![EVENTS_CAPABILITY.into()]),
    }
}

struct NoExports;
impl Dispatch for NoExports {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        Err(Error::Value("no exports".into()))
    }
}
/// The far end of a session's memory channel, scripted by a test. Clones
/// share it, so one thread can read while another writes.
#[derive(Clone)]
struct Far {
    sender: Arc<Mutex<Box<dyn Sender>>>,
    receiver: Arc<Mutex<Box<dyn Receiver>>>,
    _closer: Arc<dyn Closer>,
}
impl Far {
    fn try_clone(&self) -> Result<Self, std::convert::Infallible> {
        Ok(self.clone())
    }
}
fn read(far: &mut Far) -> Frame {
    let message = far.receiver.lock().unwrap().recv().unwrap().unwrap();
    serde_json::from_slice(&message).unwrap()
}
fn send(far: &mut Far, frame: Frame) {
    let message = serde_json::to_vec(&frame).unwrap();
    far.sender.lock().unwrap().send(&message).unwrap();
}
fn far_pair() -> (Channel, Far) {
    let (local, far) = crate::transport::memory::pair();
    let far = Far {
        sender: Arc::new(Mutex::new(far.sender)),
        receiver: Arc::new(Mutex::new(far.receiver)),
        _closer: far.closer,
    };
    (local, far)
}
fn pair() -> (Connection, Far) {
    let (local, far) = far_pair();
    (Connection::open(local, Arc::new(NoExports)).unwrap(), far)
}

/// The chain of a slow dispatch is taken when `with_caller` is made, not
/// when its future is first polled: a filter that suspends before polling
/// still forwards from its caller's chain.
#[tokio::test(flavor = "current_thread")]
async fn with_caller_captures_the_chain_when_it_is_made() {
    let (peer, _far) = pair();
    let path = vec!["node:7".to_owned()];
    let guard = PathGuard::enter(path.clone());
    let future = with_caller(peer, async { current_path() });
    drop(guard);
    assert_eq!(future.await, path);
}

#[tokio::test(flavor = "current_thread")]
async fn admitted_call_pins_its_target_before_a_following_counted_release() {
    let (peer, mut remote) = pair();
    let remote = std::thread::spawn(move || {
        let mut reader = remote.try_clone().unwrap();
        assert!(matches!(
            read(&mut reader),
            Frame::Hello {
                version: VERSION,
                ..
            }
        ));
        send(&mut remote, hello(VERSION));
        let Frame::Invoke {
            id,
            args: WireValue::List(args),
            ..
        } = read(&mut reader)
        else {
            panic!("invoke expected")
        };
        let WireValue::Reference { id: reference, .. } = &args[0] else {
            panic!("ref expected")
        };
        assert!(matches!(&args[1], WireValue::Reference { id, .. } if id == reference));
        // Unrelated work stays queued while the current_thread caller blocks.
        send(
            &mut remote,
            Frame::Call {
                method: None,
                id: "node:1".into(),
                path: vec![],
                reference: *reference,
                args: WireValue::Data(json!([])),
                sync: false,
            },
        );
        send(
            &mut remote,
            Frame::Release {
                reference: *reference,
                count: 2,
            },
        );
        send(
            &mut remote,
            Frame::Return {
                id,
                value: WireValue::Undefined,
            },
        );
        assert!(matches!(read(&mut reader), Frame::Return { id, .. } if id == "node:1"));
    });
    peer.ready().await.unwrap();
    let called = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let guard = Guard(dropped.clone());
    let observed = called.clone();
    let callback = Value::callback(move |_| {
        assert!(!guard.0.load(Ordering::SeqCst));
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Undefined)
    });
    peer.invoke("", "test", Value::List(vec![callback.clone(), callback]))
        .unwrap();
    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert!(!dropped.load(Ordering::SeqCst));
    assert!(peer.0.exports.lock().unwrap().entries.is_empty());
    peer.drain().await;
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert!(dropped.load(Ordering::SeqCst));
    remote.join().unwrap();
    peer.closed().await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_old_release_does_not_remove_a_concurrent_new_grant() {
    let (peer, mut remote) = pair();
    let remote = std::thread::spawn(move || {
        let mut reader = remote.try_clone().unwrap();
        read(&mut reader);
        send(&mut remote, hello(VERSION));
        let Frame::Invoke {
            id,
            args: WireValue::Reference { id: reference, .. },
            ..
        } = read(&mut reader)
        else {
            panic!("reference expected")
        };
        send(
            &mut remote,
            Frame::Return {
                id,
                value: WireValue::Undefined,
            },
        );
        let Frame::Invoke {
            id,
            args: WireValue::Reference { id: again, .. },
            ..
        } = read(&mut reader)
        else {
            panic!("second grant expected")
        };
        assert_eq!(again, reference);
        send(
            &mut remote,
            Frame::Release {
                reference,
                count: 1,
            },
        );
        send(
            &mut remote,
            Frame::Call {
                method: None,
                id: "node:1".into(),
                path: vec![id.clone()],
                reference,
                args: WireValue::Data(json!([])),
                sync: false,
            },
        );
        assert!(matches!(read(&mut reader), Frame::Return { id, .. } if id == "node:1"));
        send(
            &mut remote,
            Frame::Release {
                reference,
                count: 1,
            },
        );
        send(
            &mut remote,
            Frame::Return {
                id,
                value: WireValue::Undefined,
            },
        );
    });
    peer.ready().await.unwrap();
    let called = Arc::new(AtomicUsize::new(0));
    let observed = called.clone();
    let callback = Value::callback(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Undefined)
    });
    peer.invoke("", "test", callback.clone()).unwrap();
    peer.invoke("", "test", callback).unwrap();
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert!(peer.0.exports.lock().unwrap().entries.is_empty());
    remote.join().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn handshake_and_release_count_fail_closed() {
    let (peer, mut remote) = pair();
    send(&mut remote, hello(VERSION + 1));
    assert!(peer.ready().await.is_err());
    peer.closed().await;

    let (peer, mut remote) = pair();
    send(&mut remote, hello(VERSION));
    peer.ready().await.unwrap();
    let reference = peer
        .encode(&Value::callback(|_| Ok(Value::Undefined)), MAX_DEPTH)
        .unwrap();
    let WireValue::Reference { id, .. } = reference else {
        unreachable!()
    };
    send(
        &mut remote,
        Frame::Release {
            reference: id,
            count: 2,
        },
    );
    peer.closed().await;
    assert!(peer.0.exports.lock().unwrap().entries.is_empty());
    assert!(peer.invoke("", "test", Value::Undefined).is_err());
}

/// Q6.2.3, Q5.3.4, risk P3: a message that is not a frame of this protocol,
/// or names a call or reference this side does not have, closes the session
/// with a transport error, whatever was in flight.
#[tokio::test(flavor = "multi_thread")]
async fn malformed_and_dangling_frames_close_the_session() {
    let cases: [(&str, &[u8]); 13] = [
        ("malformed JSON", br#"{"op":"invoke","id":"node:1""#),
    ("empty", b""),
    ("a leading BOM", b"\xef\xbb\xbf{\"op\":\"cancel\",\"id\":\"node:1\"}"),
    (
        "invalid UTF-8 inside a string",
        b"{\"op\":\"invoke\",\"id\":\"node:1\",\"path\":[],\"target\":\"t\",\"method\":\"m\",\"args\":{\"type\":\"data\",\"value\":\"\xff\"}}",
    ),
        ("not an object", b"42"),
        ("unknown op", br#"{"op":"frobnicate","id":"node:1"}"#),
        (
            "wrong field type",
            br#"{"op":"invoke","id":"node:1","path":[],"target":7,"method":"m","args":{"type":"undefined"}}"#,
        ),
        (
            "missing field",
            br#"{"op":"invoke","id":"node:1","path":[],"target":"t","args":{"type":"undefined"}}"#,
        ),
        (
            "unknown wire value",
            br#"{"op":"invoke","id":"node:1","path":[],"target":"t","method":"m","args":{"type":"bogus"}}"#,
        ),
        (
            "call of an unknown reference",
            br#"{"op":"call","id":"node:1","path":[],"reference":99,"args":{"type":"undefined"}}"#,
        ),
        (
            "await of an unknown reference",
            br#"{"op":"await","id":"node:1","path":[],"reference":99}"#,
        ),
        (
            "release of an unknown reference",
            br#"{"op":"release","reference":99,"count":1}"#,
        ),
        (
            "reply to an unknown call",
            br#"{"op":"return","id":"rust:99","value":{"type":"undefined"}}"#,
        ),
    ];
    for (case, message) in cases {
        let (peer, mut remote) = pair();
        send(&mut remote, hello(VERSION));
        peer.ready().await.unwrap();
        remote.sender.lock().unwrap().send(message).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), peer.closed())
            .await
            .unwrap_or_else(|_| panic!("{case}: the session stayed open"));
        assert!(
            matches!(peer.close_reason(), Some(Error::Transport(_))),
            "{case}: {:?}",
            peer.close_reason()
        );
        assert!(peer.invoke("t", "m", Value::Undefined).is_err(), "{case}");
    }
}

/// The exception: a cancel may cross the reply of the call it cancels, so
/// one for a call this side does not have is ignored.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_for_an_unknown_call_is_ignored() {
    struct Echo;
    impl Dispatch for Echo {
        fn invoke(&self, _: &Connection, _: &str, _: &str, args: Value) -> Reply {
            Ok(args)
        }
    }
    let (peer, remote) = connected(Arc::new(Echo));
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start(remote);
        remote.send(Frame::Cancel {
            id: "node:7".into(),
        });
        let echoed = remote.call(
            1,
            Frame::Invoke {
                id: "node:1".into(),
                path: vec![],
                target: "t".into(),
                method: "echo".into(),
                args: data(json!("still here")),
                sync: false,
            },
        );
        (echoed, remote)
    });
    let (echoed, _remote) = remote.await.unwrap();
    assert_eq!(echoed, json!("still here"));
    assert!(peer.close_reason().is_none());
}

fn invoke(id: &str, method: &str, sync: bool) -> Frame {
    Frame::Invoke {
        id: id.into(),
        path: vec![],
        target: "t".into(),
        method: method.into(),
        args: data(json!([])),
        sync,
    }
}

/// What the far ends of the cancellation tests declare: they mark their
/// synchronous waits and run one stack, not reentrant (Node's profile).
const ONE_STACK: [&str; 2] = ["sync-wait", "sync-stack"];
/// How long a step of those tests may take before it counts as hung.
const STEP: std::time::Duration = std::time::Duration::from_secs(20);

/// A synchronous call cancelled while it runs keeps its mark until its
/// reply: a forward it makes afterwards is still a hop, and one closing a
/// cycle is refused. Session A's far end calls `hold`; it is cancelled
/// while running; B's far end calls `work`, which B forwards to A (A's far
/// end is busy, so it waits); then `hold` forwards to B, which waits for A:
/// the cycle.
/// guarantee: bridge/sync-wait-cycle-refused
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_synchronous_call_cancelled_keeps_its_mark() {
    struct Hold {
        started: Mutex<mpsc::Sender<()>>,
        release: Mutex<mpsc::Receiver<()>>,
        outcome: Mutex<mpsc::Sender<Reply>>,
        other: std::sync::OnceLock<Connection>,
    }
    impl Dispatch for Hold {
        fn invoke(&self, peer: &Connection, _: &str, method: &str, args: Value) -> Reply {
            if method == "echo" {
                return Ok(args);
            }
            self.started.lock().unwrap().send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            let other = self.other.get().unwrap();
            let forwarded = other.forward(peer, || other.invoke("t", "work", Value::List(vec![])));
            self.outcome.lock().unwrap().send(forwarded).unwrap();
            Ok(Value::Undefined)
        }
    }
    /// Forwards every call to the other session.
    struct Forward(std::sync::OnceLock<Connection>);
    impl Dispatch for Forward {
        fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
            let other = self.0.get().unwrap();
            other.forward(peer, || other.invoke(target, method, args))
        }
    }
    let (started, has_started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (outcome, outcomes) = mpsc::channel();
    let hold = Arc::new(Hold {
        started: Mutex::new(started),
        release: Mutex::new(released),
        outcome: Mutex::new(outcome),
        other: std::sync::OnceLock::new(),
    });
    let forward = Arc::new(Forward(std::sync::OnceLock::new()));
    let (a, far_a) = connected(hold.clone());
    let (b, far_b) = connected(forward.clone());
    assert!(hold.other.set(b.clone()).is_ok());
    assert!(forward.0.set(a.clone()).is_ok());
    let (a_tag, closing) = (a.tag().to_owned(), (a.clone(), b.clone()));
    let script = tokio::task::spawn_blocking(move || {
        let mut far_a = Remote::start_declaring(far_a, &ONE_STACK);
        let mut far_b = Remote::start_declaring(far_b, &ONE_STACK);
        far_a.send(invoke("node:1", "hold", true));
        has_started.recv_timeout(STEP).expect("hold runs");
        far_a.send(Frame::Cancel {
            id: "node:1".into(),
        });
        // Read after the cancel: the cancel was handled.
        far_a.call(2, invoke("node:2", "echo", false));
        assert_eq!(
            waits::marks_of(&a_tag),
            1,
            "the running call keeps its mark"
        );
        far_b.send(invoke("node:1", "work", true));
        let Frame::Invoke { id: work, .. } = far_a.read() else {
            panic!("the forwarded work expected")
        };
        release.send(()).unwrap();
        let refused = outcomes.recv_timeout(STEP);
        if refused.is_err() {
            // Sent instead of refused: it waits for ever. End both sessions
            // so nothing stays blocked.
            closing.0.close(Error::Transport("test over".into()));
            closing.1.close(Error::Transport("test over".into()));
        }
        assert!(
            matches!(refused, Ok(Err(Error::SyncWaitCycle(_)))),
            "the forward closing the cycle is refused: {refused:?}"
        );
        assert!(matches!(far_a.read(), Frame::Return { id, .. } if id == "node:1"));
        far_a.ret(work, data(json!("worked")));
        match far_b.read() {
            Frame::Return { id, value } if id == "node:1" => {
                assert_eq!(data_of(value), json!("worked"));
            }
            _ => panic!("the reply to work expected"),
        }
        assert_eq!(waits::marks_of(&a_tag), 0);
    });
    script.await.unwrap();
}

/// A synchronous `await` cancelled while the future is awaited on the
/// caller's behalf leaves the wait graph: its mark goes with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_awaited_call_leaves_no_mark() {
    struct Later(Mutex<mpsc::Sender<()>>);
    impl Dispatch for Later {
        fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
            if method == "echo" {
                return Ok(args);
            }
            let polled = self.0.lock().unwrap().clone();
            Ok(Value::future(async move {
                let _ = polled.send(());
                std::future::pending::<Reply>().await
            }))
        }
    }
    let (polled, was_polled) = mpsc::channel();
    let (a, far) = connected(Arc::new(Later(Mutex::new(polled))));
    let tag = a.tag().to_owned();
    let script = tokio::task::spawn_blocking(move || {
        let mut far = Remote::start_declaring(far, &ONE_STACK);
        far.send(invoke("node:1", "later", false));
        let (reference, kind) = match far.read() {
            Frame::Return { id, value } if id == "node:1" => granted(&value),
            _ => panic!("the future expected"),
        };
        assert_eq!(kind, Kind::Future);
        far.send(Frame::Await {
            id: "node:2".into(),
            path: vec![],
            reference,
            sync: true,
        });
        // Polled once awaited on the caller's behalf.
        was_polled
            .recv_timeout(STEP)
            .expect("the future is awaited");
        assert_eq!(waits::marks_of(&tag), 1);
        far.send(Frame::Cancel {
            id: "node:2".into(),
        });
        far.call(3, invoke("node:3", "echo", false));
        assert_eq!(waits::marks_of(&tag), 0, "the cancelled await is unmarked");
    });
    script.await.unwrap();
    drop(a);
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_close_interrupts_a_writer_whose_peer_stopped_reading() {
    let (peer, mut remote) = pair();
    send(&mut remote, hello(VERSION));
    peer.ready().await.unwrap();
    let (entered, blocked) = mpsc::channel();
    let writing = peer.clone();
    let writer = std::thread::spawn(move || -> Result<(), ChannelError> {
        let mut stream = writing.0.writer.lock().unwrap();
        entered.send(()).unwrap();
        // The far end reads nothing: sending blocks once the channel is full.
        loop {
            stream.send(b"{}")?;
        }
    });
    blocked.recv().unwrap();
    peer.close(Error::Transport("explicit close".into()));
    assert!(writer.join().unwrap().is_err());
    peer.closed().await;
}

/// A scripted peer on the far end of a connection, after the handshake.
struct Remote {
    reader: Far,
    writer: Far,
}
impl Remote {
    fn start(stream: Far) -> Self {
        let mut remote = Self {
            reader: stream.try_clone().unwrap(),
            writer: stream,
        };
        assert!(matches!(
            remote.read(),
            Frame::Hello {
                version: VERSION,
                ..
            }
        ));
        remote.send(hello(VERSION));
        remote
    }
    fn read(&mut self) -> Frame {
        read(&mut self.reader)
    }
    fn send(&mut self, frame: Frame) {
        send(&mut self.writer, frame)
    }
    fn ret(&mut self, id: String, value: WireValue) {
        self.send(Frame::Return { id, value })
    }
    /// Make call `node:{n}` and read its return value.
    fn call(&mut self, n: u64, frame: Frame) -> Json {
        self.send(frame);
        match self.read() {
            Frame::Return { id, value } if id == format!("node:{n}") => data_of(value),
            _ => panic!("return of node:{n} expected"),
        }
    }
}
fn reference(id: u64, kind: Kind, home: bool) -> WireValue {
    WireValue::Reference {
        id,
        kind,
        home,
        origin: vec![],
    }
}
fn data(value: Json) -> WireValue {
    WireValue::Data(value)
}
fn data_of(value: WireValue) -> Json {
    match value {
        WireValue::Data(value) => value,
        _ => panic!("data expected"),
    }
}
fn granted(value: &WireValue) -> (u64, Kind) {
    match value {
        WireValue::Reference {
            id,
            kind,
            home: false,
            ..
        } => (*id, *kind),
        _ => panic!("granted reference expected"),
    }
}
fn ids(path: &[&str]) -> Vec<String> {
    path.iter().map(|id| id.to_string()).collect()
}
fn connected(dispatch: Arc<dyn Dispatch>) -> (Connection, Far) {
    let (local, remote) = far_pair();
    (Connection::open(local, dispatch).unwrap(), remote)
}

#[test]
fn rebase_tags_the_source_restores_the_target_and_keeps_third_sessions() {
    let path = ids(&["node:3", "rust:5", "s2/node:1", "s7/rust:2"]);
    assert_eq!(
        rebase(&path, "s1", "s2"),
        ids(&["s1/node:3", "s1/rust:5", "node:1", "s7/rust:2"])
    );
    // Forwarding back restores the original chain.
    assert_eq!(rebase(&rebase(&path, "s1", "s2"), "s2", "s1"), path);
    assert!(rebase(&[], "s1", "s2").is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn sessions_have_distinct_tags_and_forward_rebases_the_current_chain() {
    let (a, _ra) = pair();
    let (b, _rb) = pair();
    assert_ne!(a.tag(), b.tag());
    // What runtimes accept in an origin entry: `s[0-9]+` (#325: digits
    // only, so the random nonce changes nothing on the wire). The nonce is
    // the process's; the counter follows it.
    for tag in [a.tag(), b.tag()] {
        let digits = tag.strip_prefix('s').unwrap();
        assert!(digits.len() > 15 && digits.bytes().all(|b| b.is_ascii_digit()));
    }
    assert_eq!(a.tag()[..16], b.tag()[..16]);
    let _path = PathGuard::enter(ids(&["node:1"]));
    let expected = vec![format!("{}/node:1", a.tag())];
    assert_eq!(b.forward(&a, current_path), expected);
    assert_eq!(
        b.forward_async(&a, async { current_path() }).await,
        expected
    );
    assert_eq!(current_path(), ids(&["node:1"]));
}

#[tokio::test(flavor = "current_thread")]
async fn references_forwarded_to_another_session_reach_their_owner() {
    let (a, ra) = pair();
    let (b, rb) = pair();
    let b_tag = b.tag().to_owned();
    // A owns a function (1), a future (2) and an object (3).
    let owner = std::thread::spawn(move || {
        let mut a = Remote::start(ra);
        let Frame::Invoke { id, .. } = a.read() else {
            panic!("invoke expected")
        };
        a.ret(
            id,
            WireValue::List(vec![
                reference(1, Kind::Function, false),
                reference(2, Kind::Future, false),
                reference(3, Kind::Object, false),
            ]),
        );
        let mut paths = Vec::new();
        let mut released = Vec::new();
        while released.len() < 3 {
            match a.read() {
                Frame::Call {
                    id,
                    path,
                    reference: 1,
                    method: None,
                    args,
                    ..
                } => {
                    let n = data_of(args)[0].as_i64().unwrap();
                    paths.push(path);
                    a.ret(id, data(json!(n * 2)));
                }
                Frame::Await {
                    id,
                    path,
                    reference: 2,
                    ..
                } => {
                    paths.push(path);
                    a.ret(id, data(json!(7)));
                }
                Frame::Call {
                    id,
                    reference: 3,
                    method: Some(method),
                    ..
                } => a.ret(id, data(json!(method))),
                Frame::Get {
                    id,
                    reference: 3,
                    property,
                    ..
                } => a.ret(id, data(json!(property))),
                Frame::Invoke { id, args, .. } => {
                    // The relay came home through Rust: A gets its own id.
                    assert!(matches!(
                        args,
                        WireValue::Reference {
                            id: 1,
                            home: true,
                            ..
                        }
                    ));
                    a.ret(id, WireValue::Undefined);
                }
                Frame::Release { reference, count } => {
                    assert_eq!(count, 1);
                    released.push(reference);
                }
                _ => panic!("unexpected frame"),
            }
        }
        released.sort();
        assert_eq!(released, vec![1, 2, 3]);
        paths
    });
    let user = std::thread::spawn(move || {
        let mut b = Remote::start(rb);
        let Frame::Invoke {
            id,
            args: WireValue::List(args),
            ..
        } = b.read()
        else {
            panic!("invoke expected")
        };
        let (function, kind) = granted(&args[0]);
        assert_eq!(kind, Kind::Function);
        let (future, kind) = granted(&args[1]);
        assert_eq!(kind, Kind::Future);
        let (object, kind) = granted(&args[2]);
        assert_eq!(kind, Kind::Object);
        // The same import forwarded twice is one reference.
        assert_eq!(granted(&args[3]).0, function);
        // Pumped by Rust's synchronous waiter: forwarded synchronously.
        let path = vec![id.clone()];
        let call = Frame::Call {
            id: "node:1".into(),
            path: path.clone(),
            reference: function,
            method: None,
            args: data(json!([20])),
            sync: false,
        };
        assert_eq!(b.call(1, call), json!(40));
        let wait = Frame::Await {
            id: "node:2".into(),
            path: path.clone(),
            reference: future,
            sync: false,
        };
        assert_eq!(b.call(2, wait), json!(7));
        let method = Frame::Call {
            id: "node:3".into(),
            path: path.clone(),
            reference: object,
            method: Some("greet".into()),
            args: data(json!([])),
            sync: false,
        };
        assert_eq!(b.call(3, method), json!("greet"));
        let get = Frame::Get {
            id: "node:4".into(),
            path: path.clone(),
            reference: object,
            property: "name".into(),
            sync: false,
        };
        assert_eq!(b.call(4, get), json!("name"));
        // Outside any Rust call chain: forwarded asynchronously.
        let unrelated = Frame::Call {
            id: "node:5".into(),
            path: vec![],
            reference: function,
            method: None,
            args: data(json!([1])),
            sync: false,
        };
        assert_eq!(b.call(5, unrelated), json!(2));
        // A mismatched operation is refused without reaching the owner,
        // which would fail its session on it.
        b.send(Frame::Await {
            id: "node:6".into(),
            path: vec![],
            reference: function,
            sync: false,
        });
        assert!(matches!(b.read(), Frame::Throw { id, .. } if id == "node:6"));
        b.ret(id, WireValue::Undefined);
        // Forwarded again later: still the same reference. Send it home.
        let Frame::Invoke {
            id,
            args: WireValue::List(args),
            ..
        } = b.read()
        else {
            panic!("invoke expected")
        };
        assert_eq!(granted(&args[0]).0, function);
        b.ret(id, reference(function, Kind::Function, true));
        for (reference, count) in [(function, 3), (future, 1), (object, 1)] {
            b.send(Frame::Release { reference, count });
        }
    });
    a.ready().await.unwrap();
    b.ready().await.unwrap();
    let references = a
        .invoke("", "references", Value::Undefined)
        .unwrap()
        .list()
        .unwrap();
    let mut args = references.clone();
    args.push(references[0].clone());
    b.invoke("", "use", Value::List(args)).unwrap();
    let returned = b
        .invoke("", "again", Value::List(vec![references[0].clone()]))
        .unwrap()
        .reference()
        .unwrap();
    // The relay decodes back to the import it forwards to.
    assert!(returned == references[0].clone().reference().unwrap());
    a.invoke("", "home", Value::Reference(returned)).unwrap();
    drop(references);
    user.join().unwrap();
    // B released every relay: their imports drop and A gets its releases.
    let paths = tokio::task::spawn_blocking(move || owner.join().unwrap())
        .await
        .unwrap();
    // B's chain reaches A tagged with B's session.
    let tagged = |id: &str| format!("{b_tag}/{id}");
    assert_eq!(paths[0], vec![tagged("rust:1"), tagged("node:1")]);
    assert_eq!(paths[1], vec![tagged("rust:1"), tagged("node:2")]);
    assert_eq!(paths[2], vec![tagged("node:5")]);
    let exports = b.0.exports.lock().unwrap();
    assert!(exports.entries.is_empty() && exports.relays.is_empty());
}

/// Both sessions have a synchronous call `node:1` outstanding. A's call is
/// forwarded to B, and B synchronously calls back a function from A: the
/// callback reaches the Rust thread serving A's `node:1` (no deadlock), A
/// finds its own `node:1` in the chain, and B's `node:1` stays untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reverse_call_through_a_relay_reaches_the_waiting_session() {
    struct Hold(Mutex<mpsc::Receiver<()>>);
    impl Dispatch for Hold {
        fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
            self.0.lock().unwrap().recv().unwrap();
            Ok(json!("held").into())
        }
    }
    struct Forward(Connection);
    impl Dispatch for Forward {
        fn invoke(&self, peer: &Connection, _: &str, _: &str, args: Value) -> Reply {
            let target = &self.0;
            target.forward(peer, || target.invoke("svc", "run", args))
        }
    }
    let (release, held) = mpsc::channel();
    let (b, rb) = connected(Arc::new(Hold(Mutex::new(held))));
    let (a, ra) = connected(Arc::new(Forward(b.clone())));
    let (a_tag, b_tag) = (a.tag().to_owned(), b.tag().to_owned());

    let (b_waiting, b_waits) = mpsc::channel();
    let (b_ready, b_readied) = mpsc::channel();
    let user = std::thread::spawn(move || {
        let mut b = Remote::start(rb);
        // B's own synchronous call node:1, held open by Rust.
        b.send(Frame::Invoke {
            id: "node:1".into(),
            path: vec![],
            target: "svc".into(),
            method: "hold".into(),
            args: data(json!([])),
            sync: false,
        });
        b_waiting.send(()).unwrap();
        let Frame::Invoke {
            id,
            path,
            args: WireValue::List(args),
            ..
        } = b.read()
        else {
            panic!("forwarded invoke expected")
        };
        // A's node:1 is tagged, so B cannot take it for its own node:1.
        assert_eq!(path, vec![format!("{a_tag}/node:1")]);
        let (function, _) = granted(&args[0]);
        // B calls back synchronously, inside the forwarded call.
        let mut chain = path;
        chain.push(id.clone());
        let callback = Frame::Call {
            id: "node:2".into(),
            path: chain,
            reference: function,
            method: None,
            args: data(json!([4])),
            sync: false,
        };
        let value = b.call(2, callback);
        b.ret(id, data(value));
        b
    });
    let owner = std::thread::spawn(move || {
        let mut a = Remote::start(ra);
        b_waits.recv().unwrap();
        // Rust must have read B's handshake before A's call is forwarded
        // there; it reads it on B's reader thread, apart from this one.
        b_readied.recv().unwrap();
        a.send(Frame::Invoke {
            id: "node:1".into(),
            path: vec![],
            target: "svc".into(),
            method: "forward".into(),
            args: WireValue::List(vec![reference(1, Kind::Function, false)]),
            sync: false,
        });
        let frame = a.read();
        let Frame::Call {
            id,
            path,
            reference: 1,
            args,
            ..
        } = frame
        else {
            panic!(
                "callback expected, got {}",
                serde_json::to_string(&frame).unwrap()
            )
        };
        // A finds its own waiting node:1 in the chain.
        assert_eq!(
            path,
            vec![
                "node:1".to_owned(),
                format!("{b_tag}/rust:1"),
                format!("{b_tag}/node:2")
            ]
        );
        let n = data_of(args)[0].as_i64().unwrap();
        a.ret(id, data(json!(n + 1)));
        let Frame::Return { id, value } = a.read() else {
            panic!("forward return expected")
        };
        assert_eq!((id.as_str(), data_of(value)), ("node:1", json!(5)));
        a
    });
    b.ready().await.unwrap();
    b_ready.send(()).unwrap();
    a.ready().await.unwrap();
    let mut user = tokio::task::spawn_blocking(move || user.join().unwrap())
        .await
        .unwrap();
    let _owner = tokio::task::spawn_blocking(move || owner.join().unwrap())
        .await
        .unwrap();
    // B's own node:1 was never answered on the way.
    release.send(()).unwrap();
    let Frame::Return { id, value } = user.read() else {
        panic!("held return expected")
    };
    assert_eq!((id.as_str(), data_of(value)), ("node:1", json!("held")));
}

mod endpoint_format {
    use super::*;

    fn id(s: &str) -> PeerId {
        PeerId::new(s).unwrap()
    }

    struct Echo(&'static str);
    impl Dispatch for Echo {
        fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
            if method == "callback" {
                // Call back through the reference the far end passed.
                let callback = args.list()?.remove(0).reference()?;
                return callback.call(Value::Data(json!([self.0])));
            }
            Ok(Value::Data(
                json!({ "at": self.0, "target": target, "method": method }),
            ))
        }
    }

    fn pair_as(left: Endpoint, right: Endpoint) -> (Connection, Connection) {
        let (a, mut b) = crate::transport::memory::pair();
        let mut a = a;
        a.info.peer = Some(right.local.clone());
        b.info.peer = Some(left.local.clone());
        (
            Connection::open_with(a, Arc::new(Echo("left")), Format::Endpoint(left)).unwrap(),
            Connection::open_with(b, Arc::new(Echo("right")), Format::Endpoint(right)).unwrap(),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_rust_endpoints_call_each_other_with_their_own_ids() {
        let (main, mac) = pair_as(
            Endpoint::rust(id("main")).expect(id("mac")),
            Endpoint::rust(id("mac")).expect(id("main")),
        );
        main.ready().await.unwrap();
        mac.ready().await.unwrap();
        assert_eq!(main.greeting().unwrap().endpoint, id("mac"));
        assert!(main.supports("signals") && main.supports(PLUGINS_CAPABILITY));
        assert!(!main.supports("teleport"));

        let reply = main
            .invoke_async("x", "ping", Value::Undefined)
            .await
            .unwrap();
        assert_eq!(reply.json().unwrap()["at"], "right");
        let reply = mac
            .invoke_async("y", "pong", Value::Undefined)
            .await
            .unwrap();
        assert_eq!(reply.json().unwrap()["at"], "left");

        // A callback crosses back while its caller waits synchronously.
        let main2 = main.clone();
        let reply = tokio::task::spawn_blocking(move || {
            main2.invoke(
                "",
                "callback",
                Value::List(vec![Value::callback(|args| {
                    Ok(Value::Data(json!(format!("called by {}", args.json()?[0]))))
                })]),
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reply.json().unwrap(), json!("called by \"right\""));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_greeting_from_another_endpoint_fails_the_handshake() {
        // The channel verified "mac", but "pi" greets.
        let (main, pi) = pair_as(
            Endpoint::rust(id("main")).expect(id("mac")),
            Endpoint::rust(id("pi")),
        );
        assert!(matches!(
            main.ready().await,
            Err(Error::Handshake(Handshake::IdentityMismatch(_)))
        ));
        drop(pi);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_compat_far_end_is_incompatible_with_the_endpoint_format() {
        let (a, b) = crate::transport::memory::pair();
        let endpoint = Connection::open_with(
            a,
            Arc::new(NoExports),
            Format::Endpoint(Endpoint::rust(id("main"))),
        )
        .unwrap();
        let compat = Connection::open(b, Arc::new(NoExports)).unwrap();
        let result = endpoint.ready().await;
        assert!(
            matches!(result, Err(Error::Handshake(Handshake::Incompatible(_)))),
            "endpoint side: {result:?}"
        );
        let result = compat.ready().await;
        assert!(
            matches!(result, Err(Error::Handshake(Handshake::Incompatible(_)))),
            "compat side: {result:?}"
        );
    }

    /// What a session did on its channel, in order: whether it had tried
    /// to greet, and had greeted, by the time it first read.
    #[derive(Default)]
    struct Order {
        state: Mutex<(bool, bool, Option<bool>)>,
        changed: std::sync::Condvar,
    }
    /// `channel`, noting the [`Order`] of its first send and first read.
    /// With `greet_first`, the first read waits until a send was tried,
    /// fixing the order of a session that greets and reads at once.
    fn ordered(channel: Channel, greet_first: bool) -> (Channel, Arc<Order>) {
        struct Sending(Box<dyn Sender>, Arc<Order>);
        impl Sender for Sending {
            fn send(&mut self, message: &[u8]) -> Result<(), crate::channel::ChannelError> {
                self.1.state.lock().unwrap().0 = true;
                self.1.changed.notify_all();
                let result = self.0.send(message);
                if result.is_ok() {
                    self.1.state.lock().unwrap().1 = true;
                }
                result
            }
        }
        struct Reading(Box<dyn Receiver>, Arc<Order>, bool);
        impl Receiver for Reading {
            fn recv(&mut self) -> Result<Option<Vec<u8>>, crate::channel::ChannelError> {
                let mut state = self.1.state.lock().unwrap();
                while self.2 && !state.0 {
                    state = self.1.changed.wait(state).unwrap();
                }
                let sent = state.1;
                state.2.get_or_insert(sent);
                drop(state);
                self.0.recv()
            }
        }
        let order = Arc::new(Order::default());
        let Channel {
            sender,
            receiver,
            closer,
            info,
        } = channel;
        (
            Channel {
                sender: Box::new(Sending(sender, order.clone())),
                receiver: Box::new(Reading(receiver, order.clone(), greet_first)),
                closer,
                info,
            },
            order,
        )
    }

    // #239: the far end's greeting is waiting when the compat side opens.
    // The compat side finds it incompatible and closes at once; it must
    // have greeted before reading it, or the endpoint side sees only the
    // channel end (`peer disconnected`) and the compat side's `open` fails
    // writing to the channel it closed itself.
    #[tokio::test(flavor = "multi_thread")]
    async fn issue_239_an_incompatible_side_greets_before_it_reads_the_far_greeting() {
        let (a, b) = crate::transport::memory::pair();
        let endpoint = Connection::open_with(
            a,
            Arc::new(NoExports),
            Format::Endpoint(Endpoint::rust(id("main"))),
        )
        .unwrap();
        let (b, order) = ordered(b, false);
        let compat = Connection::open(b, Arc::new(NoExports));
        let compat = compat.unwrap_or_else(|error| panic!("compat side open: {error:?}"));
        let result = compat.ready().await;
        assert!(
            matches!(result, Err(Error::Handshake(Handshake::Incompatible(_)))),
            "compat side: {result:?}"
        );
        assert_eq!(
            order.state.lock().unwrap().2,
            Some(true),
            "read before greeting"
        );
        let result = endpoint.ready().await;
        assert!(
            matches!(result, Err(Error::Handshake(Handshake::Incompatible(_)))),
            "endpoint side: {result:?}"
        );
    }

    // #239, from the other side: a far end that does not greet first may
    // greet and close the channel before this side greets. Its greeting
    // still decides the handshake, not the channel end.
    #[tokio::test(flavor = "multi_thread")]
    async fn issue_239_a_greeting_followed_by_the_channel_end_is_incompatible() {
        let (local, mut far) = far_pair();
        send(&mut far, hello(VERSION));
        far._closer.close("incompatible");
        // This side tries to greet (and fails: the channel ended) before it
        // reads the far end's greeting.
        let (local, _) = ordered(local, true);
        let endpoint = Connection::open_with(
            local,
            Arc::new(NoExports),
            Format::Endpoint(Endpoint::rust(id("main"))),
        )
        .unwrap_or_else(|error| panic!("open: {error:?}"));
        let result = endpoint.ready().await;
        assert!(
            matches!(result, Err(Error::Handshake(Handshake::Incompatible(_)))),
            "{result:?}"
        );
    }

    // Without a greeting, a channel that ended is a channel end.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_channel_end_before_any_greeting_is_a_transport_error() {
        let (local, far) = far_pair();
        far._closer.close("gone");
        let endpoint = Connection::open_with(
            local,
            Arc::new(NoExports),
            Format::Endpoint(Endpoint::rust(id("main"))),
        )
        .unwrap_or_else(|error| panic!("open: {error:?}"));
        let result = endpoint.ready().await;
        assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
    }

    /// Why a session that greets in `format` refuses a far end greeting
    /// with `version` (and `implementation`, in the endpoint format).
    async fn refusal(
        format: Option<Endpoint>,
        version: u32,
        implementation: Option<Implementation>,
    ) -> String {
        let (local, mut far) = far_pair();
        let endpoint = format.is_some();
        send(
            &mut far,
            Frame::Hello {
                version,
                endpoint: endpoint.then(|| "far".into()),
                implementation,
                capabilities: endpoint.then(|| vec![EVENTS_CAPABILITY.into()]),
            },
        );
        let session = match format {
            Some(format) => {
                Connection::open_with(local, Arc::new(NoExports), Format::Endpoint(format))
            }
            None => Connection::open(local, Arc::new(NoExports)),
        }
        .unwrap_or_else(|error| panic!("open: {error:?}"));
        match session.ready().await {
            Err(Error::Handshake(Handshake::Incompatible(reason))) => reason,
            other => panic!("expected an incompatible handshake: {other:?}"),
        }
    }

    // Risk P8, standard Q6.2.4: the refusal of another protocol version
    // says which end to upgrade: this side, naming what to upgrade here,
    // when the far end is newer; the far end, naming its implementation
    // when it said, when it is older.
    // guarantee: bridge/version-refusal
    #[tokio::test(flavor = "multi_thread")]
    async fn a_far_end_of_another_protocol_version_is_told_which_end_to_upgrade() {
        let ours = crate::session::ENDPOINT_PROTOCOL;
        let rust = || Some(Endpoint::rust(id("main")));
        assert_eq!(
            refusal(rust(), 99, None).await,
            format!(
                "the far end speaks protocol 99, this side {ours}: \
                 upgrade this side (Rust crates rutis-host / rutis-bridge)"
            )
        );
        assert_eq!(
            refusal(None, 99, None).await,
            format!(
                "the far end speaks protocol 99, this side {VERSION}: \
                 upgrade this side (Rust crates rutis-host / rutis-bridge)"
            )
        );
        let older = Implementation {
            name: "rutis".into(),
            version: "0.1.0".into(),
        };
        assert_eq!(
            refusal(rust(), 1, Some(older)).await,
            format!(
                "the far end speaks protocol 1, this side {ours}: \
                 upgrade the far end (rutis 0.1.0)"
            )
        );
        assert_eq!(
            refusal(rust(), 1, None).await,
            format!("the far end speaks protocol 1, this side {ours}: upgrade the far end")
        );
        assert_eq!(
            refusal(None, 1, None).await,
            format!("the far end speaks protocol 1, this side {VERSION}: upgrade the far end")
        );
    }

    /// guarantee: events/v2-required
    ///
    /// A far end of the right version that does not declare `events-v2` is
    /// refused right after the handshake (events protocol §2.2), in both
    /// formats: the endpoint format names it, the compat format cannot.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_far_end_without_events_v2_is_refused() {
        for (format, implementation, named) in [
            (None, None, "a runtime older than rutis 0.13".to_owned()),
            (
                Some(Endpoint::rust(id("main"))),
                Some(Implementation {
                    name: "rutis-runtime".into(),
                    version: "0.12.1".into(),
                }),
                "rutis-runtime 0.12.1".to_owned(),
            ),
        ] {
            let (local, mut far) = far_pair();
            let endpoint = format.is_some();
            send(
                &mut far,
                Frame::Hello {
                    version: match endpoint {
                        true => crate::session::ENDPOINT_PROTOCOL,
                        false => VERSION,
                    },
                    endpoint: endpoint.then(|| "far".into()),
                    implementation,
                    capabilities: Some(vec!["sync-wait".into()]),
                },
            );
            let session = match format {
                Some(format) => {
                    Connection::open_with(local, Arc::new(NoExports), Format::Endpoint(format))
                }
                None => Connection::open(local, Arc::new(NoExports)),
            }
            .unwrap();
            match session.ready().await {
                Err(error @ Error::Handshake(Handshake::Incompatible(_))) => assert_eq!(
                    error.to_string(),
                    format!(
                        "incompatible session: the far end ({named}) does not declare events-v2 \
                         (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later"
                    )
                ),
                other => panic!("expected the refusal: {other:?}"),
            }
        }
    }
}

// ── Value depth (#279) ───────────────────────────────────────────────

/// JSON data `depth` arrays deep.
fn arrays(depth: usize) -> Json {
    (0..depth).fold(json!("bottom"), |inner, _| json!([inner]))
}
/// Lists `depth` deep around a reference, as a far end sends them, with
/// `value` written before `type` (serde buffers such an object before it
/// reads it: the deepest stack the reader can take for a frame).
fn wire_lists(depth: usize) -> String {
    let leaf = r#"{"value":{"id":1,"kind":"function","home":false,"origin":["node:1"]},"type":"reference"}"#;
    (0..depth).fold(leaf.to_string(), |inner, _| {
        format!(r#"{{"value":[{inner}],"type":"list"}}"#)
    })
}

#[test]
fn depth_is_counted_in_arrays_and_objects() {
    assert!(fits(&json!("scalar"), 0));
    assert!(!fits(&json!([]), 0));
    assert!(fits(&arrays(MAX_DEPTH), MAX_DEPTH));
    assert!(!fits(&arrays(MAX_DEPTH + 1), MAX_DEPTH));
    assert!(fits(&json!({ "a": [1, { "b": 2 }] }), 3));
    assert!(!fits(&json!({ "a": [1, { "b": 2 }] }), 2));
    // Brackets in strings, escaped quotes, are not levels.
    let text = br#"{"a":"[[[[\"[[[[","b":[[1]]}"#;
    assert!(!deeper_than(text, 3));
    assert!(deeper_than(text, 2));
}

/// How deep arrays and objects nest in `text`, as the reader counts.
fn frame_depth(text: &str) -> usize {
    (0..).find(|&n| !deeper_than(text.as_bytes(), n)).unwrap()
}
const REFERENCE: &str =
    r#"{"value":{"id":1,"kind":"function","home":false,"origin":["node:1"]},"type":"reference"}"#;
/// Records `depth` deep around a reference, `value` before `type`.
fn wire_records(depth: usize) -> String {
    (0..depth).fold(REFERENCE.to_string(), |inner, _| {
        format!(r#"{{"value":{{"k":{inner}}},"type":"record"}}"#)
    })
}
/// Maps `depth` deep around a reference, each holding the next as the
/// value of its one entry, `value` before `type`.
fn wire_maps(depth: usize) -> String {
    (0..depth).fold(REFERENCE.to_string(), |inner, _| {
        format!(r#"{{"value":[[{{"type":"data","value":"k"}},{inner}]],"type":"map"}}"#)
    })
}
/// A JSON object `depth` deep.
fn objects(depth: usize) -> String {
    (0..depth).fold("1".to_string(), |inner, _| format!(r#"{{"k":{inner}}}"#))
}
fn invoke_frame(id: &str, args: &str) -> String {
    format!(r#"{{"args":{args},"op":"invoke","id":"{id}","path":[],"target":"t","method":"m"}}"#)
}
/// Invocations of each kind exactly [`MAX_FRAME_DEPTH`] deep: lists and
/// records around a reference (two levels each), maps around one (three
/// levels each), a data object.
fn deepest_invocations() -> Vec<(&'static str, String)> {
    let levels = (MAX_FRAME_DEPTH - 4) / 2;
    let frames = vec![
        ("lists", invoke_frame("node:1", &wire_lists(levels))),
        ("records", invoke_frame("node:2", &wire_records(levels))),
        (
            "maps",
            invoke_frame("node:3", &wire_maps((MAX_FRAME_DEPTH - 4) / 3)),
        ),
        (
            "a data object",
            invoke_frame(
                "node:4",
                &format!(
                    r#"{{"value":{},"type":"data"}}"#,
                    objects(MAX_FRAME_DEPTH - 2)
                ),
            ),
        ),
    ];
    for (kind, frame) in &frames {
        assert_eq!(frame_depth(frame), MAX_FRAME_DEPTH, "{kind}");
    }
    frames
}
/// A throw whose error graph makes it exactly [`MAX_FRAME_DEPTH`] deep.
fn deepest_throw(id: &str) -> String {
    let frame = format!(
        r#"{{"error":{{"graph":{},"name":"E","message":"deep"}},"op":"throw","id":"{id}"}}"#,
        objects(MAX_FRAME_DEPTH - 2)
    );
    assert_eq!(frame_depth(&frame), MAX_FRAME_DEPTH);
    frame
}

/// A frame the deepest a far end within the limit sends parses: lists
/// take two wire levels each, maps three.
#[test]
fn a_frame_from_within_the_limit_is_within_the_frame_depth() {
    let lists = invoke_frame("node:1", &wire_lists(MAX_DEPTH + 1));
    assert_eq!(frame_depth(&lists), 2 * MAX_DEPTH + 6);
    // The argument list, then maps to the limit.
    let maps = invoke_frame(
        "node:1",
        &format!(r#"{{"value":[{}],"type":"list"}}"#, wire_maps(MAX_DEPTH)),
    );
    assert_eq!(frame_depth(&maps), 3 * MAX_DEPTH + 6);
    for frame in [lists, maps] {
        assert!(frame_depth(&frame) <= MAX_FRAME_DEPTH);
        assert!(matches!(
            parse_frame(frame.as_bytes()),
            Ok(Frame::Invoke { .. })
        ));
    }
}

/// Frames exactly [`MAX_FRAME_DEPTH`] deep, of every kind (lists, records
/// and maps holding references, a data object, an error graph), go the
/// real receive path: parsed and decoded on the reader thread (its stack
/// set explicitly, [`READER_STACK`]), dropped after the call. In a debug
/// build too. Parsing alone is also checked on a thread with that stack.
#[tokio::test(flavor = "multi_thread")]
async fn the_deepest_frames_allowed_go_the_receive_path_on_the_readers_stack() {
    let mut frames = deepest_invocations();
    frames.push(("an error graph", deepest_throw("main:1")));
    std::thread::Builder::new()
        .stack_size(READER_STACK)
        .spawn(move || {
            for (kind, frame) in frames {
                assert!(parse_frame(frame.as_bytes()).is_ok(), "{kind}");
            }
        })
        .unwrap()
        .join()
        .unwrap();

    struct Nothing;
    impl Dispatch for Nothing {
        fn invoke(&self, _: &Connection, _: &str, _: &str, args: Value) -> Reply {
            drop(args);
            Ok(Value::Undefined)
        }
    }
    let (peer, remote) = connected(Arc::new(Nothing));
    let (answered, calls) = tokio::sync::oneshot::channel();
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start(remote);
        // Skip the releases of the references the calls carried.
        let reply = |remote: &mut Remote| loop {
            match remote.read() {
                Frame::Release { .. } => continue,
                frame => return frame,
            }
        };
        for (kind, frame) in deepest_invocations() {
            remote
                .writer
                .sender
                .lock()
                .unwrap()
                .send(frame.as_bytes())
                .unwrap();
            match reply(&mut remote) {
                Frame::Return { .. } => {}
                Frame::Throw { error, .. } => panic!("{kind}: {}", error.message),
                _ => panic!("{kind}: a return expected"),
            }
        }
        answered.send(()).unwrap();
        // This side calls; the reply is a throw with the deepest graph.
        let Frame::Invoke { id, .. } = reply(&mut remote) else {
            panic!("invoke expected")
        };
        let throw = deepest_throw(&id);
        remote
            .writer
            .sender
            .lock()
            .unwrap()
            .send(throw.as_bytes())
            .unwrap();
        remote
    });
    calls.await.unwrap();
    let thrown = peer.invoke_async("t", "m", Value::Undefined).await;
    assert!(
        matches!(&thrown, Err(Error::Remote { graph: Some(_), .. })),
        "the deep error arrives"
    );
    drop(thrown);
    let _remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());
}

struct EchoArgs;
impl Dispatch for EchoArgs {
    fn invoke(&self, _: &Connection, _: &str, _: &str, args: Value) -> Reply {
        Ok(args)
    }
}
fn assert_too_deep(result: Reply) {
    match result {
        Err(Error::Value(message)) => assert!(message.contains("64 levels"), "{message}"),
        other => panic!("a value error naming the limit expected: {other:?}"),
    }
}

/// guarantee: bridge/value-limits
///
/// A value nested deeper than the limit fails its own call on the side
/// that sends it, as a value error naming the limit; nothing is sent, and
/// the session goes on. A value at the limit crosses.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_deeper_than_the_limit_fails_its_call_before_it_is_sent() {
    let (peer, remote) = connected(Arc::new(EchoArgs));
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start(remote);
        // The first frame the far end reads is the call at the limit.
        let message = remote
            .reader
            .receiver
            .lock()
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        let frame = parse_frame(&message).unwrap();
        let Frame::Invoke { id, args, .. } = frame else {
            panic!("invoke expected")
        };
        remote.ret(id, args);
        remote
    });
    peer.ready().await.unwrap();
    assert_too_deep(
        peer.invoke_async("t", "m", Value::Data(json!([arrays(MAX_DEPTH + 1)])))
            .await,
    );
    let lists = (0..MAX_DEPTH + 1).fold(Value::Data(json!(1)), |inner, _| Value::List(vec![inner]));
    assert_too_deep(peer.invoke_async("t", "m", Value::List(vec![lists])).await);
    let back = peer
        .invoke_async("t", "m", Value::Data(json!([arrays(MAX_DEPTH)])))
        .await
        .unwrap();
    assert_eq!(back.json().unwrap(), json!([arrays(MAX_DEPTH)]));
    // Keep the far end until after the check: dropping it closes the
    // channel, which the session would see as the far end leaving.
    let remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());
    drop(remote);
}

/// guarantee: bridge/value-limits
///
/// A far end that does not keep to the limit: a call to this side in a
/// frame too deep to read fails with a throw naming the limit, a reply
/// too deep fails the call waiting for it; the session goes on. A frame
/// too deep that names no call ends the session, saying why.
#[tokio::test(flavor = "multi_thread")]
async fn a_frame_too_deep_fails_its_call_and_the_session_goes_on() {
    let (peer, remote) = connected(Arc::new(EchoArgs));
    let too_deep = arrays(MAX_FRAME_DEPTH + 10);
    let (answered, calls) = tokio::sync::oneshot::channel();
    let remote = tokio::task::spawn_blocking({
        let too_deep = too_deep.clone();
        move || {
            let mut remote = Remote::start(remote);
            let invoke = serde_json::to_vec(&json!({
                "op": "invoke", "id": "node:1", "path": [], "target": "t", "method": "m",
                "args": { "type": "data", "value": [too_deep] },
            }))
            .unwrap();
            remote.writer.sender.lock().unwrap().send(&invoke).unwrap();
            match remote.read() {
                Frame::Throw { id, error } => {
                    assert_eq!(id, "node:1");
                    assert!(error.message.contains("64 levels"), "{}", error.message);
                }
                _ => panic!("throw of node:1 expected"),
            }
            let echoed = remote.call(
                2,
                Frame::Invoke {
                    id: "node:2".into(),
                    path: vec![],
                    target: "t".into(),
                    method: "m".into(),
                    args: data(json!(["after"])),
                    sync: false,
                },
            );
            assert_eq!(echoed, json!(["after"]));
            answered.send(()).unwrap();
            // This side calls: the reply is too deep.
            let Frame::Invoke { id, .. } = remote.read() else {
                panic!("invoke expected")
            };
            let reply = serde_json::to_vec(&json!({
                "op": "return", "id": id, "value": { "type": "data", "value": too_deep },
            }))
            .unwrap();
            remote.writer.sender.lock().unwrap().send(&reply).unwrap();
            let Frame::Invoke { id, args, .. } = remote.read() else {
                panic!("invoke expected")
            };
            remote.ret(id, args);
            remote
        }
    });
    calls.await.unwrap();
    assert_too_deep(peer.invoke_async("t", "m", Value::Data(json!([]))).await);
    let back = peer
        .invoke_async("t", "m", Value::Data(json!(["after"])))
        .await
        .unwrap();
    assert_eq!(back.json().unwrap(), json!(["after"]));
    let remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());

    let cancel = format!(
        r#"{{"op":"cancel","id":"node:9","x":{}}}"#,
        serde_json::to_string(&too_deep).unwrap()
    );
    remote
        .writer
        .sender
        .lock()
        .unwrap()
        .send(cancel.as_bytes())
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), peer.closed())
        .await
        .expect("the session stayed open");
    match peer.close_reason() {
        Some(Error::Transport(message)) => assert!(message.contains("64 levels"), "{message}"),
        other => panic!("{other:?}"),
    }
}

// ── Tagged values (#308) ─────────────────────────────────────────────

/// The capabilities this side declares cover every tagged value, in both
/// formats, and a compat-format far end may declare them.
#[test]
fn this_side_declares_every_value_capability_in_both_formats() {
    use crate::session::values::VALUE_TAGS;
    let endpoint = Endpoint::rust(PeerId::new("main").unwrap());
    for tag in VALUE_TAGS {
        let capability = format!("value-{tag}");
        assert!(VALUE_CAPABILITIES.contains(&capability.as_str()), "{tag}");
        assert!(COMPAT_CAPABILITIES.contains(&capability.as_str()), "{tag}");
        assert!(COMPAT_DECLARED.contains(&capability.as_str()), "{tag}");
        assert!(endpoint.capabilities.contains(&capability), "{tag}");
    }
}

impl Remote {
    /// [`Remote::start`], greeting in the compat format with
    /// `capabilities`.
    fn start_declaring(stream: Far, capabilities: &[&str]) -> Self {
        let mut remote = Self {
            reader: stream.try_clone().unwrap(),
            writer: stream,
        };
        assert!(matches!(remote.read(), Frame::Hello { .. }));
        remote.send(Frame::Hello {
            version: VERSION,
            endpoint: None,
            implementation: None,
            capabilities: Some(
                capabilities
                    .iter()
                    .chain([&EVENTS_CAPABILITY])
                    .map(|c| c.to_string())
                    .collect(),
            ),
        });
        remote
    }
    fn send_json(&mut self, frame: Json) {
        let bytes = serde_json::to_vec(&frame).unwrap();
        self.writer.sender.lock().unwrap().send(&bytes).unwrap();
    }
    fn read_json(&mut self) -> Json {
        let message = self
            .reader
            .receiver
            .lock()
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        serde_json::from_slice(&message).unwrap()
    }
}

/// Each tagged value, and its wire form.
fn tagged_samples() -> Vec<(Value, Json)> {
    let instant: Instant = "2026-10-15T03:22:15.123456789Z".parse().unwrap();
    let uuid: Uuid = "0f8fad5b-d9cb-469f-a165-70867728950e".parse().unwrap();
    vec![
        (
            Value::Instant(instant),
            json!({ "type": "instant", "value": "2026-10-15T03:22:15.123456789Z" }),
        ),
        (
            Value::DateTime("2026-10-15T14:30:05.25".parse().unwrap()),
            json!({ "type": "datetime", "value": "2026-10-15T14:30:05.25" }),
        ),
        (
            Value::Date("2026-10-15".parse().unwrap()),
            json!({ "type": "date", "value": "2026-10-15" }),
        ),
        (
            Value::Time("14:30:05.25".parse().unwrap()),
            json!({ "type": "time", "value": "14:30:05.25" }),
        ),
        (
            Value::Bytes(b"hello".to_vec()),
            json!({ "type": "bytes", "value": "aGVsbG8=" }),
        ),
        (
            Value::Map(vec![
                (Value::Data(json!("a")), Value::Data(json!(1))),
                (Value::BigInt(2.into()), Value::Instant(instant)),
            ]),
            json!({ "type": "map", "value": [
                [{ "type": "data", "value": "a" }, { "type": "data", "value": 1 }],
                [{ "type": "bigint", "value": "2" },
                 { "type": "instant", "value": "2026-10-15T03:22:15.123456789Z" }],
            ] }),
        ),
        (
            Value::Set(vec![Value::Data(json!(1)), Value::Uuid(uuid)]),
            json!({ "type": "set", "value": [
                { "type": "data", "value": 1 },
                { "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" },
            ] }),
        ),
        (
            Value::BigInt("-123456789012345678901".parse().unwrap()),
            json!({ "type": "bigint", "value": "-123456789012345678901" }),
        ),
        (
            Value::Decimal("-12.3400".parse().unwrap()),
            json!({ "type": "decimal", "value": "-12.3400" }),
        ),
        (
            Value::Uuid(uuid),
            json!({ "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" }),
        ),
        // In an object: a record, as with references.
        (
            Value::Record([("at".to_owned(), Value::Instant(instant))].into()),
            json!({ "type": "record", "value": {
                "at": { "type": "instant", "value": "2026-10-15T03:22:15.123456789Z" },
            } }),
        ),
    ]
}

/// guarantee: bridge/value-tags
///
/// To a far end that declared their capabilities, tagged values go out in
/// their wire formats and come back as the same values.
#[tokio::test(flavor = "multi_thread")]
async fn tagged_values_cross_in_their_wire_formats_to_a_far_end_that_declared_them() {
    let (peer, remote) = connected(Arc::new(EchoArgs));
    let (values, wire): (Vec<_>, Vec<_>) = tagged_samples().into_iter().unzip();
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start_declaring(remote, &VALUE_CAPABILITIES);
        let frame = remote.read_json();
        assert_eq!(frame["op"], "invoke");
        assert_eq!(frame["args"], json!({ "type": "list", "value": wire }));
        // Echo the arguments back as they came.
        remote.send_json(json!({ "op": "return", "id": frame["id"], "value": frame["args"] }));
        // A map value undefined is null (design V10).
        let frame = remote.read_json();
        assert_eq!(
            frame["args"]["value"][0]["value"][0][1],
            json!({ "type": "data", "value": null })
        );
        remote.send_json(
            json!({ "op": "return", "id": frame["id"], "value": { "type": "undefined" } }),
        );
        remote
    });
    peer.ready().await.unwrap();
    let back = peer
        .invoke_async("t", "m", Value::List(values.clone()))
        .await
        .unwrap();
    assert_eq!(back, Value::List(values));
    peer.invoke_async(
        "t",
        "m",
        Value::List(vec![Value::Map(vec![(
            Value::Data(json!("gone")),
            Value::Undefined,
        )])]),
    )
    .await
    .unwrap();
    let _remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());
}

/// guarantee: bridge/value-tags
///
/// Map keys and set elements this side would send are checked before the
/// call goes out: of a kind every language takes as a key, none equal to
/// another; a map nests as a level of the depth limit. Each failure fails
/// only its call.
#[tokio::test(flavor = "multi_thread")]
async fn map_keys_and_set_elements_are_checked_before_sending() {
    let (peer, remote) = connected(Arc::new(EchoArgs));
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start_declaring(remote, &VALUE_CAPABILITIES);
        // Only the last call is sent.
        let Frame::Invoke { id, args, .. } = remote.read() else {
            panic!("invoke expected")
        };
        remote.ret(id, args);
        remote
    });
    peer.ready().await.unwrap();
    let instant = Instant::from_unix(0, 0).unwrap();
    let refused = |value: Value| {
        let peer = peer.clone();
        async move {
            match peer.invoke_async("t", "m", Value::List(vec![value])).await {
                Err(Error::Value(message)) => message,
                other => panic!("a value error expected, got {other:?}"),
            }
        }
    };
    for (key, kind) in [
        (Value::Instant(instant), "an instant"),
        (Value::Bytes(vec![1]), "bytes"),
        (Value::Data(json!(null)), "null"),
        (Value::Data(json!([1])), "an array"),
        (Value::Map(vec![]), "a map"),
        (Value::callback(|_| Ok(Value::Undefined)), "a reference"),
    ] {
        let message = refused(Value::Map(vec![(key.clone(), Value::Data(json!(1)))])).await;
        assert!(
            message.contains(&format!("a map key cannot be {kind}")),
            "{message}"
        );
        let message = refused(Value::Set(vec![key])).await;
        assert!(
            message.contains(&format!("a set element cannot be {kind}")),
            "{message}"
        );
    }
    let message = refused(Value::Map(vec![
        (Value::Data(json!(1)), Value::Data(json!("a"))),
        (Value::Data(json!(1.0)), Value::Data(json!("b"))),
    ]))
    .await;
    assert!(message.contains("duplicate map key 1"), "{message}");
    let message = refused(Value::Set(vec![
        Value::Date("2026-10-15".parse().unwrap()),
        Value::Date("2026-10-15".parse().unwrap()),
    ]))
    .await;
    assert!(
        message.contains("duplicate set element 2026-10-15"),
        "{message}"
    );
    let maps = (0..MAX_DEPTH + 1).fold(Value::Data(json!(1)), |inner, _| {
        Value::Map(vec![(Value::Data(json!("k")), inner)])
    });
    assert_too_deep(peer.invoke_async("t", "m", Value::List(vec![maps])).await);
    let set = Value::List(vec![Value::Set(vec![Value::Data(json!("x"))])]);
    assert_eq!(peer.invoke_async("t", "m", set.clone()).await.unwrap(), set);
    let _remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());
}

/// guarantee: bridge/value-tags
///
/// An integer in data beyond ±(2^53−1) crosses as a bigint (design V6),
/// the arrays and objects holding it as lists and records; data without
/// one, floats included, crosses as it is. As a map key or set element it
/// is the bigint key, equal to the same `BigInt`; a whole number that
/// large written as a float is not exact and is refused as a key. To a far
/// end that did not declare `value-bigint`, the call fails before it is
/// sent and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn integers_beyond_2_53_cross_as_bigints() {
    const SAFE: i64 = (1 << 53) - 1;
    let sent = json!([
        [SAFE, -SAFE, 1.5, 9007199254740993.0],
        { "id": SAFE + 1, "ok": true, "deep": [[u64::MAX], { "small": 1 }] },
    ]);
    let (peer, remote) = connected(Arc::new(EchoArgs));
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start_declaring(remote, &VALUE_CAPABILITIES);
        let frame = remote.read_json();
        let bigint = |text: &str| json!({ "type": "bigint", "value": text });
        assert_eq!(
            frame["args"],
            json!({ "type": "list", "value": [
                { "type": "data", "value": [SAFE, -SAFE, 1.5, 9007199254740993.0] },
                { "type": "record", "value": {
                    "id": bigint("9007199254740992"),
                    "ok": { "type": "data", "value": true },
                    "deep": { "type": "list", "value": [
                        { "type": "list", "value": [bigint("18446744073709551615")] },
                        { "type": "data", "value": { "small": 1 } },
                    ] },
                } },
            ] })
        );
        remote.send_json(json!({ "op": "return", "id": frame["id"], "value": frame["args"] }));
        let frame = remote.read_json();
        assert_eq!(
            frame["args"]["value"][0],
            json!({ "type": "map", "value": [
                [bigint("-9223372036854775808"), { "type": "data", "value": "min" }],
                [{ "type": "data", "value": SAFE }, { "type": "data", "value": "safe" }],
            ] })
        );
        remote.send_json(json!({ "op": "return", "id": frame["id"], "value": frame["args"] }));
        remote
    });
    peer.ready().await.unwrap();
    let args = Value::Data(sent.clone()).list().unwrap();
    let back = peer
        .invoke_async("t", "m", Value::List(args))
        .await
        .unwrap();
    // Received, the bigints are `BigInt`s; as data, the numbers sent.
    assert_eq!(back.json().unwrap(), sent);
    let map = Value::Map(vec![
        (Value::Data(json!(i64::MIN)), Value::Data(json!("min"))),
        (Value::Data(json!(SAFE)), Value::Data(json!("safe"))),
    ]);
    let back = peer
        .invoke_async("t", "m", Value::List(vec![map]))
        .await
        .unwrap();
    let Value::List(back) = back else {
        panic!("a list expected")
    };
    assert_eq!(
        back[0],
        Value::Map(vec![
            (Value::BigInt(i64::MIN.into()), Value::Data(json!("min"))),
            (Value::Data(json!(SAFE)), Value::Data(json!("safe"))),
        ])
    );
    let refused = |value: Value| {
        let peer = peer.clone();
        async move {
            match peer.invoke_async("t", "m", Value::List(vec![value])).await {
                Err(Error::Value(message)) => message,
                other => panic!("a value error expected, got {other:?}"),
            }
        }
    };
    let message = refused(Value::Set(vec![
        Value::Data(json!(SAFE + 1)),
        Value::BigInt((SAFE + 1).into()),
    ]))
    .await;
    assert!(
        message.contains("duplicate set element 9007199254740992"),
        "{message}"
    );
    let message = refused(Value::Map(vec![(
        Value::Data(json!(9007199254740993.0)),
        Value::Data(json!(1)),
    )]))
    .await;
    assert!(
        message.contains("a map key cannot be the float 9007199254740992.0"),
        "{message}"
    );
    let _remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());

    // A far end without `value-bigint`: data within the range still
    // crosses.
    let (peer, remote) = connected(Arc::new(EchoArgs));
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start_declaring(remote, &["value-date"]);
        let frame = remote.read_json();
        remote.send_json(json!({ "op": "return", "id": frame["id"], "value": frame["args"] }));
        remote
    });
    peer.ready().await.unwrap();
    match peer
        .invoke_async(
            "t",
            "m",
            Value::List(vec![Value::Data(json!({ "n": [u64::MAX] }))]),
        )
        .await
    {
        Err(Error::Value(message)) => assert_eq!(
            message,
            "cannot send the integer 18446744073709551615, beyond ±(2^53-1) and so a bigint: \
             the far end does not take bigint values (it did not declare the capability \
             value-bigint)"
        ),
        other => panic!("a value error expected, got {other:?}"),
    }
    let safe = Value::List(vec![Value::Data(json!([SAFE, -SAFE]))]);
    assert_eq!(
        peer.invoke_async("t", "m", safe.clone()).await.unwrap(),
        safe
    );
    let _remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());
}

/// guarantee: bridge/value-tags
///
/// A call refused for a value it carries still takes the references it
/// carries, and releases them: the far end's exports do not stay granted
/// until the session ends. The session keeps serving.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_call_releases_the_references_it_carries() {
    let (peer, remote) = connected(Arc::new(EchoArgs));
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote::start_declaring(remote, &VALUE_CAPABILITIES);
        let function = |id: u64| {
            json!({ "type": "reference",
                    "value": { "id": id, "kind": "function", "home": false, "origin": [] } })
        };
        let refused = [
            // Bad base64 beside a function.
            json!([{ "type": "bytes", "value": "not base64!" }, function(1)]),
            // A map with colliding keys holding one.
            json!([{ "type": "map", "value": [
                [{ "type": "data", "value": 1 }, function(2)],
                [{ "type": "data", "value": 1 }, { "type": "data", "value": 2 }],
            ] }]),
            // A function after the bad value, nested in a record.
            json!([{ "type": "record", "value": {
                "a": { "type": "instant", "value": "yesterday" },
                "b": function(3),
            } }]),
        ];
        for (n, args) in refused.into_iter().enumerate() {
            let id = format!("node:{}", n + 1);
            remote.send_json(json!({
                "op": "invoke", "id": id, "path": [], "target": "t", "method": "m",
                "args": { "type": "list", "value": args },
            }));
            // A release of the function, and the call's throw, in any order.
            let (mut released, mut thrown) = (false, false);
            while !(released && thrown) {
                let frame = remote.read_json();
                match frame["op"].as_str() {
                    Some("release") => {
                        assert_eq!(frame["reference"], json!(n + 1));
                        assert_eq!(frame["count"], json!(1));
                        released = true;
                    }
                    Some("throw") => {
                        assert_eq!(frame["id"], id);
                        thrown = true;
                    }
                    _ => panic!("a release or a throw expected, got {frame}"),
                }
            }
        }
        // Still serving.
        remote.send_json(json!({
            "op": "invoke", "id": "node:9", "path": [], "target": "t", "method": "m",
            "args": { "type": "list", "value": [{ "type": "date", "value": "2026-10-15" }] },
        }));
        let reply = remote.read_json();
        assert_eq!(reply["op"], "return", "{reply}");
        remote
    });
    peer.ready().await.unwrap();
    let _remote = remote.await.unwrap();
    assert!(peer.0.imports.lock().unwrap().is_empty());
    assert!(peer.close_reason().is_none());
}

/// Returns a tagged value for `instant`, its arguments otherwise.
struct Tagging;
impl Dispatch for Tagging {
    fn invoke(&self, _: &Connection, _: &str, method: &str, args: Value) -> Reply {
        match method {
            "instant" => Ok(Value::Instant(Instant::from_unix(0, 0).unwrap())),
            _ => Ok(args),
        }
    }
}

/// guarantee: bridge/value-tags
///
/// A far end that did not declare a value's capability (an older runtime)
/// is never sent that value: a call carrying it fails before it is sent,
/// with an error naming the value and the capability, as does a reply
/// carrying one (the far end receives the error). Values it declared still
/// cross, and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_tagged_value_to_a_far_end_without_its_capability_fails_only_that_call() {
    let (peer, remote) = connected(Arc::new(Tagging));
    let (answered, calls) = tokio::sync::oneshot::channel();
    let remote = tokio::task::spawn_blocking(move || {
        // `value-date` only; `objects` means nothing in this format.
        let mut remote = Remote::start_declaring(remote, &["value-date", "objects"]);
        // The first frame read is the date: the bytes were not sent.
        let frame = remote.read_json();
        assert_eq!(
            frame["args"]["value"][0],
            json!({ "type": "date", "value": "2026-10-15" })
        );
        remote.send_json(json!({ "op": "return", "id": frame["id"], "value": frame["args"] }));
        // A reply carrying an instant fails on this side's end: the far end
        // receives the error.
        remote.send_json(json!({
            "op": "invoke", "id": "node:1", "path": [], "target": "t", "method": "instant",
            "args": { "type": "data", "value": [] },
        }));
        let reply = remote.read_json();
        assert_eq!(reply["op"], "throw", "{reply}");
        let message = reply["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("cannot send an instant")
                && message.contains("capability value-instant"),
            "{message}"
        );
        answered.send(()).unwrap();
        remote
    });
    peer.ready().await.unwrap();
    assert!(peer.supports("value-date") && !peer.supports("value-bytes"));
    match peer
        .invoke_async("t", "m", Value::List(vec![Value::Bytes(vec![1, 2])]))
        .await
    {
        Err(Error::Value(message)) => assert_eq!(
            message,
            "cannot send bytes: the far end does not take bytes values \
             (it did not declare the capability value-bytes)"
        ),
        other => panic!("a value error expected, got {other:?}"),
    }
    let date = Value::List(vec![Value::Date("2026-10-15".parse().unwrap())]);
    assert_eq!(
        peer.invoke_async("t", "m", date.clone()).await.unwrap(),
        date
    );
    calls.await.unwrap();
    let _remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());
}

/// guarantee: bridge/value-tags
///
/// What this side receives is checked: a tagged value it did not declare,
/// one not in its format, a map whose keys collide or a key of a kind no
/// key may be fails only the call it came in (a call from the far end
/// with a throw, a reply by failing the call waiting for it); the session
/// goes on.
#[tokio::test(flavor = "multi_thread")]
async fn received_tagged_values_are_checked_and_only_their_call_fails() {
    // This side takes every tagged value but `uuid`.
    let mut endpoint = Endpoint::rust(PeerId::new("main").unwrap());
    endpoint.capabilities.retain(|c| c != "value-uuid");
    let (local, far) = far_pair();
    let peer =
        Connection::open_with(local, Arc::new(EchoArgs), Format::Endpoint(endpoint)).unwrap();
    let (answered, calls) = tokio::sync::oneshot::channel();
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote {
            reader: far.try_clone().unwrap(),
            writer: far,
        };
        assert!(matches!(remote.read(), Frame::Hello { .. }));
        remote.send(Frame::Hello {
            version: crate::session::ENDPOINT_PROTOCOL,
            endpoint: Some("far".into()),
            implementation: None,
            capabilities: Some(
                VALUE_CAPABILITIES
                    .iter()
                    .chain([&EVENTS_CAPABILITY])
                    .map(|c| c.to_string())
                    .collect(),
            ),
        });
        let data = |value: Json| json!({ "type": "data", "value": value });
        let refusals = [
            (
                json!({ "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" }),
                "received a uuid value, which this side does not take \
                 (it did not declare the capability value-uuid)",
            ),
            (
                json!({ "type": "map", "value": [[data(json!(1)), data(json!("a"))],
                                                 [data(json!(1.0)), data(json!("b"))]] }),
                "duplicate map key 1",
            ),
            (
                json!({ "type": "map", "value": [[{ "type": "bytes", "value": "AA==" },
                                                  data(json!(1))]] }),
                "a map key cannot be bytes",
            ),
            (
                json!({ "type": "set", "value": [data(json!(null))] }),
                "a set element cannot be null",
            ),
            (
                json!({ "type": "instant", "value": "2026-13-01T00:00:00Z" }),
                "invalid instant \"2026-13-01T00:00:00Z\"",
            ),
            (
                json!({ "type": "instant", "value": "2026-10-15T03:22:15+02:00" }),
                "invalid instant",
            ),
            (
                json!({ "type": "bytes", "value": "aGVsbG8" }),
                "invalid bytes",
            ),
            (
                json!({ "type": "bigint", "value": "007" }),
                "invalid bigint",
            ),
            (
                json!({ "type": "decimal", "value": "1e5" }),
                "invalid decimal",
            ),
            // A good one after all of them.
            (
                json!({ "type": "instant", "value": "2026-10-15T03:22:15.5Z" }),
                "",
            ),
        ];
        for (n, (arg, expected)) in refusals.into_iter().enumerate() {
            let id = format!("far:{}", n + 1);
            remote.send_json(json!({
                "op": "invoke", "id": id, "path": [], "target": "t",
                "method": "m", "args": { "type": "list", "value": [arg.clone()] },
            }));
            let reply = remote.read_json();
            assert_eq!(reply["id"], id);
            if expected.is_empty() {
                assert_eq!(reply["op"], "return", "{reply}");
                assert_eq!(reply["value"], json!({ "type": "list", "value": [arg] }));
                continue;
            }
            assert_eq!(reply["op"], "throw", "{reply}");
            let message = reply["error"]["message"].as_str().unwrap();
            assert!(message.contains(expected), "{message}");
        }
        answered.send(()).unwrap();
        // This side calls: the reply is a set with equal elements.
        let frame = remote.read_json();
        remote.send_json(json!({
            "op": "return", "id": frame["id"],
            "value": { "type": "set", "value": [data(json!("x")), data(json!("x"))] },
        }));
        remote
    });
    peer.ready().await.unwrap();
    calls.await.unwrap();
    match peer.invoke_async("t", "m", Value::Undefined).await {
        Err(Error::Value(message)) => {
            assert!(message.contains("duplicate set element \"x\""), "{message}")
        }
        other => panic!("a value error expected, got {other:?}"),
    }
    let _remote = remote.await.unwrap();
    assert!(peer.close_reason().is_none());
}

/// A call refused while decoding its arguments takes its id as any call
/// does (#407): sent again, even with arguments this side takes, it is a
/// repeated invocation identity and closes the session, not a new call.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_call_takes_its_id() {
    let mut endpoint = Endpoint::rust(PeerId::new("main").unwrap());
    endpoint.capabilities.retain(|c| c != "value-uuid");
    let (local, far) = far_pair();
    let peer =
        Connection::open_with(local, Arc::new(EchoArgs), Format::Endpoint(endpoint)).unwrap();
    let remote = tokio::task::spawn_blocking(move || {
        let mut remote = Remote {
            reader: far.try_clone().unwrap(),
            writer: far,
        };
        assert!(matches!(remote.read(), Frame::Hello { .. }));
        remote.send(Frame::Hello {
            version: crate::session::ENDPOINT_PROTOCOL,
            endpoint: Some("far".into()),
            implementation: None,
            capabilities: Some(
                VALUE_CAPABILITIES
                    .iter()
                    .chain([&EVENTS_CAPABILITY])
                    .map(|c| c.to_string())
                    .collect(),
            ),
        });
        let invoke = |arg: Json| {
            json!({
                "op": "invoke", "id": "far:1", "path": [], "target": "t",
                "method": "m", "args": { "type": "list", "value": [arg] },
            })
        };
        remote.send_json(invoke(
            json!({ "type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e" }),
        ));
        let reply = remote.read_json();
        assert_eq!(reply["op"], "throw", "{reply}");
        remote.send_json(invoke(json!({ "type": "data", "value": 1 })));
        remote
    });
    peer.ready().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), peer.closed())
        .await
        .expect("the repeated id closes the session");
    match peer.close_reason() {
        Some(Error::Transport(message)) => {
            assert!(
                message.contains("repeated invocation identity"),
                "{message}"
            )
        }
        other => panic!("a transport error expected, got {other:?}"),
    }
    let _remote = remote.await.unwrap();
}

/// A plugin definition (`plugin` reference) goes back to the runtime that
/// defined it as its own id, and is never relayed to another session: the
/// call that would carry it there fails before anything is sent
/// (`docs/design-plugin-values-2026-10-25.md` §5).
#[tokio::test(flavor = "current_thread")]
async fn a_plugin_definition_goes_home_and_is_never_relayed() {
    let (a, ra) = pair();
    let (b, rb) = pair();
    let owner = std::thread::spawn(move || {
        let mut a = Remote::start(ra);
        let Frame::Invoke { id, .. } = a.read() else {
            panic!("invoke expected")
        };
        a.ret(id, reference(1, Kind::Plugin, false));
        // Sent home twice: as it came, and as a home-only reference.
        for _ in 0..2 {
            let Frame::Invoke {
                id,
                args: WireValue::List(args),
                ..
            } = a.read()
            else {
                panic!("invoke expected")
            };
            assert!(matches!(
                args[0],
                WireValue::Reference {
                    id: 1,
                    home: true,
                    kind: Kind::Plugin,
                    ..
                }
            ));
            a.ret(id, WireValue::Undefined);
        }
        let Frame::Release { reference, count } = a.read() else {
            panic!("release expected")
        };
        assert_eq!((reference, count), (1, 1));
    });
    let user = std::thread::spawn(move || {
        let mut b = Remote::start(rb);
        // Only the call after the refused ones arrives.
        let Frame::Invoke {
            id, method, args, ..
        } = b.read()
        else {
            panic!("invoke expected")
        };
        assert_eq!(method, "done");
        assert!(matches!(args, WireValue::Undefined));
        b.ret(id, WireValue::Undefined);
    });
    a.ready().await.unwrap();
    b.ready().await.unwrap();
    let plugin = a
        .invoke("", "definition", Value::Undefined)
        .unwrap()
        .reference()
        .unwrap();
    assert!(plugin.is_plugin() && !plugin.is_function() && !plugin.is_object());
    let home = plugin.home_only().unwrap();
    assert!(home.is_plugin() && home.is_from(&a) && !home.is_from(&b));
    assert_eq!(plugin.identity(), home.identity());
    for refused in [plugin.clone(), home.clone()] {
        let error = b
            .invoke("", "use", Value::List(vec![Value::Reference(refused)]))
            .expect_err("refused");
        assert!(
            error
                .to_string()
                .contains("can only go back to the runtime that defined it"),
            "{error}"
        );
    }
    assert!(home.call(Value::Undefined).is_err());
    b.invoke("", "done", Value::Undefined).unwrap();
    for sent in [plugin.clone(), home.clone()] {
        a.invoke("", "home", Value::List(vec![Value::Reference(sent)]))
            .unwrap();
    }
    drop((plugin, home));
    user.join().unwrap();
    tokio::task::spawn_blocking(move || owner.join().unwrap())
        .await
        .unwrap();
}

/// Rust takes plugin definitions in both handshake formats.
#[tokio::test(flavor = "current_thread")]
async fn rust_declares_plugins_in_the_compat_format() {
    let (peer, mut far) = pair();
    let Frame::Hello { capabilities, .. } = read(&mut far) else {
        panic!("hello expected")
    };
    assert!(capabilities
        .unwrap_or_default()
        .iter()
        .any(|capability| capability == PLUGINS_CAPABILITY));
    drop(peer);
}

/// Test-only hooks into the session's code paths.
pub(super) mod hooks {
    use super::super::SessionState;
    use super::Connection;
    use std::sync::{Arc, Mutex};

    type Hook = Box<dyn FnOnce() + Send>;
    /// Hooks run once, by session, right after decoding a far-end
    /// reference looked up the import it names.
    static AFTER_IMPORT_UPGRADE: Mutex<Vec<(usize, Hook)>> = Mutex::new(Vec::new());

    pub(super) fn on_import_upgrade(session: &Connection, hook: Hook) {
        let key = Arc::as_ptr(&session.0) as usize;
        AFTER_IMPORT_UPGRADE.lock().unwrap().push((key, hook));
    }
    pub(in crate::session::rpc) fn after_import_upgrade(session: &Arc<SessionState>) {
        let key = Arc::as_ptr(session) as usize;
        let hook = {
            let mut hooks = AFTER_IMPORT_UPGRADE.lock().unwrap();
            let found = hooks.iter().position(|(k, _)| *k == key);
            found.map(|i| hooks.swap_remove(i).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// #621: a far-end reference whose kind does not match the import it names
/// closes the session with a transport error, even when the import found is
/// left as the last holder while it is checked. Dropping the last holder
/// takes the imports lock, so the check must not hold it.
#[tokio::test(flavor = "multi_thread")]
async fn a_mismatched_reference_to_an_import_left_last_closes_the_session() {
    let (peer, far) = pair();
    let remote = std::thread::spawn(move || {
        let mut remote = Remote::start(far);
        let Frame::Invoke { id, .. } = remote.read() else {
            panic!("invoke expected")
        };
        remote.ret(id, reference(5, Kind::Function, false));
        let Frame::Invoke { id, .. } = remote.read() else {
            panic!("invoke expected")
        };
        // The same id, now claimed to be an object.
        remote.ret(id, reference(5, Kind::Object, false));
        // Stay connected until the session closes: the release of the
        // dropped import must not be what ends it.
        let mut released = 0;
        while let Ok(Some(message)) = remote.reader.receiver.lock().unwrap().recv() {
            if let Ok(Frame::Release { reference: 5, .. }) = serde_json::from_slice(&message) {
                released += 1;
            }
        }
        released
    });
    peer.ready().await.unwrap();
    let held = peer
        .invoke_async("", "first", Value::Undefined)
        .await
        .unwrap();
    assert!(matches!(held, Value::Reference(_)));
    // Between the reader's lookup and its check, the other holder goes.
    hooks::on_import_upgrade(&peer, Box::new(move || drop(held)));
    let second = {
        let peer = peer.clone();
        tokio::spawn(async move { peer.invoke_async("", "second", Value::Undefined).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), peer.closed())
        .await
        .expect("the session closes instead of hanging");
    let reason = peer.close_reason();
    assert!(
        matches!(&reason, Some(Error::Transport(r)) if r.contains("reference kind mismatch")),
        "{reason:?}"
    );
    assert!(second.await.unwrap().is_err());
    assert!(peer.0.imports.lock().unwrap().is_empty());
    drop(peer);
    // The import was released once, when its last holder dropped.
    assert_eq!(remote.join().unwrap(), 1);
}
