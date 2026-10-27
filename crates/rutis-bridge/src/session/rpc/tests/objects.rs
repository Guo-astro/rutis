//! Objects with methods across runtimes, the host's side (#687, the Rust
//! column of `objects.expected.json`): the host forwards them between
//! sessions that declared `objects` (also in the compat format, as the
//! runtimes start), says where an object came from once its runtime is
//! gone, refuses one to a far end that takes none, and keeps `undefined`
//! only as a whole argument or result.

use super::*;
use crate::session::{host_key, HostDispatch};
use rutis::host::CtxHost;

/// The far ends of these tests: compat-format runtimes taking objects,
/// marking their synchronous waits.
const TAKES_OBJECTS: [&str; 3] = ["objects", "sync-wait", "sync-stack"];

/// Host session `a`, named `name` as a runtime's link names it, whose far
/// end owns object 3 and gives it as the result of the first call; the far
/// end and the object's import here.
async fn owner(name: &str) -> (Connection, Remote, Value) {
    let (a, far) = connected(Arc::new(NoExports));
    a.name_far_end(name);
    let mut far = tokio::task::spawn_blocking(move || Remote::start_declaring(far, &TAKES_OBJECTS))
        .await
        .unwrap();
    a.ready().await.unwrap();
    let call = {
        let a = a.clone();
        tokio::spawn(async move { a.invoke_async("t", "give", Value::List(vec![])).await })
    };
    let far = tokio::task::spawn_blocking(move || {
        let Frame::Invoke { id, .. } = far.read() else {
            panic!("invoke expected")
        };
        far.ret(id, reference(3, Kind::Object, false));
        far
    })
    .await
    .unwrap();
    let object = call.await.unwrap().unwrap();
    assert!(object.clone().reference().unwrap().is_object());
    (a, far, object)
}

/// The holder of a proxy goes without releasing it (its process killed):
/// the owner's object is released all the same, as the host's session to
/// the holder ends and drops its relay.
/// objects-case(rust): holder-killed-releases
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_is_released_when_its_holder_goes() {
    let (_a, mut far_a, object) = owner("node").await;
    let (b, far_b) = connected(Arc::new(NoExports));
    let holder = tokio::task::spawn_blocking(move || {
        let mut far_b = Remote::start_declaring(far_b, &TAKES_OBJECTS);
        let Frame::Invoke { id, args, .. } = far_b.read() else {
            panic!("invoke expected")
        };
        let WireValue::List(args) = args else {
            panic!("a list expected")
        };
        assert_eq!(granted(&args[0]).1, Kind::Object, "a relay of the object");
        far_b.ret(id, WireValue::Undefined);
        far_b
    });
    b.ready().await.unwrap();
    b.invoke_async("t", "take", Value::List(vec![object.clone()]))
        .await
        .unwrap();
    let far_b = holder.await.unwrap();
    drop(object);
    // Killed: its channel ends, no release from it.
    drop(far_b);
    let released = tokio::task::spawn_blocking(move || loop {
        if let Frame::Release { reference, count } = far_a.read() {
            break (reference, count);
        }
    });
    let released = tokio::time::timeout(STEP, released)
        .await
        .expect("the owner's object released")
        .unwrap();
    assert_eq!(released, (3, 1));
}

/// A call on an object whose runtime went: `peer-gone`, saying the runtime
/// and how it went, also forwarded to another runtime holding a relay.
async fn gone(name: &str, how: Option<&str>, reason: &str, said: &str) {
    let (a, far_a, object) = owner(name).await;
    let (b, far_b) = connected(Arc::new(NoExports));
    let holder = tokio::task::spawn_blocking(move || {
        let mut far_b = Remote::start_declaring(far_b, &TAKES_OBJECTS);
        let Frame::Invoke { id, args, .. } = far_b.read() else {
            panic!("invoke expected")
        };
        let WireValue::List(args) = args else {
            panic!("a list expected")
        };
        let relay = granted(&args[0]).0;
        far_b.ret(id, WireValue::Undefined);
        (far_b, relay)
    });
    b.ready().await.unwrap();
    b.invoke_async("t", "take", Value::List(vec![object.clone()]))
        .await
        .unwrap();
    let (mut far_b, relay) = holder.await.unwrap();
    if let Some(how) = how {
        a.ending(how);
    }
    a.close(Error::Transport(reason.into()));
    drop(far_a);
    let error = object
        .reference()
        .unwrap()
        .call_method_async("get", Value::List(vec![]))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), said);
    assert_eq!(error.code(), Some("peer-gone"));
    assert_eq!(error.data().unwrap()["runtime"], json!(name));
    // The holder's call, forwarded: the same.
    let thrown = tokio::task::spawn_blocking(move || {
        far_b.send(Frame::Call {
            id: "node:1".into(),
            path: vec![],
            reference: relay,
            method: Some("get".into()),
            args: data(json!([])),
            sync: false,
        });
        let thrown = loop {
            match far_b.read_json() {
                frame if frame["op"] == "throw" => break frame,
                frame if frame["op"] == "return" => panic!("a throw expected: {frame}"),
                _ => {}
            }
        };
        (thrown, far_b)
    })
    .await
    .unwrap();
    let (thrown, _far_b) = thrown;
    assert_eq!(thrown["error"]["message"], json!(said), "{thrown}");
    assert!(b.close_reason().is_none(), "the holder's session goes on");
}

/// objects-case(rust): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_on_an_object_whose_runtime_exited_is_peer_gone() {
    gone(
        "py",
        None,
        "the process exited with exit status: 1",
        "a call on an object from py failed: runtime py exited",
    )
    .await;
}

/// objects-case(rust): idle-stop-peer-gone
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_on_an_object_whose_runtime_stopped_idle_says_so() {
    gone(
        "go-netkit",
        Some("stopped because it was idle"),
        "link stopped",
        "a call on an object from go-netkit failed: runtime go-netkit stopped because it was idle",
    )
    .await;
}

/// Two runtimes calling each other's objects synchronously: the forward
/// closing the cycle is refused (`SyncWaitCycle`), nothing hangs (#687
/// §六, `bridge/sync-wait-cycle-refused`).
/// objects-case(rust): sync-wait-cycle
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_synchronous_cycle_through_objects_is_refused() {
    let (a, mut far_a, object_a) = owner("go-a").await;
    let (b, mut far_b, object_b) = owner("go-b").await;
    // Each far end gets the other's object, as a relay.
    let give = |connection: Connection, object: Value| async move {
        connection
            .invoke_async("t", "take", Value::List(vec![object]))
            .await
    };
    let to_b = tokio::spawn(give(b.clone(), object_a.clone()));
    let to_a = tokio::spawn(give(a.clone(), object_b.clone()));
    let (far_a, far_b, relay_in_a, relay_in_b) = tokio::task::spawn_blocking(move || {
        let take = |far: &mut Remote| {
            let Frame::Invoke { id, args, .. } = far.read() else {
                panic!("invoke expected")
            };
            let WireValue::List(args) = args else {
                panic!("a list expected")
            };
            let relay = granted(&args[0]).0;
            far.ret(id, WireValue::Undefined);
            relay
        };
        let relay_in_a = take(&mut far_a);
        let relay_in_b = take(&mut far_b);
        (far_a, far_b, relay_in_a, relay_in_b)
    })
    .await
    .unwrap();
    to_a.await.unwrap().unwrap();
    to_b.await.unwrap().unwrap();
    let closing = (a.clone(), b.clone());
    let script = tokio::task::spawn_blocking(move || {
        let (mut far_a, mut far_b) = (far_a, far_b);
        // A calls B's object synchronously; B, while answering, calls A's.
        far_a.send(Frame::Call {
            id: "node:1".into(),
            path: vec![],
            reference: relay_in_a,
            method: Some("ping".into()),
            args: data(json!([])),
            sync: true,
        });
        let forwarded = loop {
            let frame = far_b.read_json();
            if frame["op"] != "release" {
                break frame;
            }
        };
        assert_eq!(forwarded["op"], "call", "{forwarded}");
        assert_eq!(forwarded["method"], "ping", "{forwarded}");
        assert_eq!(forwarded["reference"], 3, "{forwarded}");
        let ping = forwarded["id"].as_str().unwrap().to_owned();
        far_b.send(Frame::Call {
            id: "node:1".into(),
            path: vec![ping.clone()],
            reference: relay_in_b,
            method: Some("pong".into()),
            args: data(json!([])),
            sync: true,
        });
        let refused = loop {
            match far_b.read_json() {
                frame if frame["op"] == "throw" || frame["op"] == "return" => break frame,
                _ => {}
            }
        };
        if refused["op"] != "throw" {
            closing.0.close(Error::Transport("test over".into()));
            closing.1.close(Error::Transport("test over".into()));
        }
        assert_eq!(refused["op"], "throw", "{refused}");
        assert_eq!(refused["id"], "node:1");
        assert!(
            refused.to_string().contains("SyncWaitCycle"),
            "the call closing the cycle is refused: {refused}"
        );
        far_b.ret(ping, data(json!("pinged")));
        match far_a.read() {
            Frame::Return { id, value } if id == "node:1" => {
                assert_eq!(data_of(value), json!("pinged"))
            }
            _ => panic!("A's call answered"),
        }
    });
    tokio::time::timeout(STEP, script)
        .await
        .expect("nothing hangs")
        .unwrap();
    drop((object_a, object_b));
}

/// `undefined` is null inside a value at any depth, both ways; a whole
/// argument or result keeps it (#687 §七).
/// objects-case(rust): undefined-nested-null
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undefined_is_null_inside_values() {
    struct Echo(Mutex<Option<Value>>);
    impl Dispatch for Echo {
        fn invoke(&self, _: &Connection, _: &str, _: &str, args: Value) -> Reply {
            *self.0.lock().unwrap() = Some(args);
            Ok(Value::List(vec![Value::Undefined]))
        }
    }
    let echo = Arc::new(Echo(Mutex::new(None)));
    let (a, far) = connected(echo.clone());
    let script = tokio::task::spawn_blocking(move || {
        let mut far = Remote::start_declaring(far, &TAKES_OBJECTS);
        // Sent: inside, null; a whole argument, undefined.
        let Frame::Invoke { id, args, .. } = far.read() else {
            panic!("invoke expected")
        };
        let WireValue::List(args) = args else {
            panic!("a list expected")
        };
        assert!(
            matches!(args[0], WireValue::Undefined),
            "a whole argument kept"
        );
        let WireValue::List(inner) = &args[1] else {
            panic!("a list expected")
        };
        assert!(
            matches!(&inner[1], WireValue::Data(Json::Null)),
            "null inside"
        );
        far.ret(id, WireValue::Undefined);
        // Received, also from a far end that sends the marker inside.
        far.send(Frame::Invoke {
            id: "node:1".into(),
            path: vec![],
            target: "t".into(),
            method: "m".into(),
            args: WireValue::List(vec![
                WireValue::Undefined,
                WireValue::List(vec![data(json!(1)), WireValue::Undefined, data(json!(3))]),
                WireValue::Record(
                    [("a".to_owned(), WireValue::Undefined)]
                        .into_iter()
                        .collect(),
                ),
            ]),
            sync: false,
        });
        match far.read() {
            Frame::Return { id, value } if id == "node:1" => value,
            _ => panic!("a return expected"),
        }
    });
    a.ready().await.unwrap();
    let sent = Value::List(vec![
        Value::Undefined,
        Value::List(vec![json!(1).into(), Value::Undefined, json!(3).into()]),
    ]);
    let result = a.invoke_async("t", "m", sent).await.unwrap();
    assert!(matches!(result, Value::Undefined), "a whole result kept");
    let returned = script.await.unwrap();
    // This side's result, a list holding undefined: null inside.
    let WireValue::List(items) = returned else {
        panic!("a list expected")
    };
    assert!(matches!(items[0], WireValue::Data(Json::Null)));
    let received = echo.0.lock().unwrap().take().unwrap();
    let Value::List(received) = received else {
        panic!("a list expected")
    };
    assert!(
        matches!(received[0], Value::Undefined),
        "a whole argument kept"
    );
    let Value::List(inner) = &received[1] else {
        panic!("a list expected: {received:?}")
    };
    assert!(matches!(&inner[1], Value::Data(Json::Null)), "{inner:?}");
    let Value::Record(record) = &received[2] else {
        panic!("a record expected: {received:?}")
    };
    assert!(
        matches!(&record["a"], Value::Data(Json::Null)),
        "{record:?}"
    );
}

/// A Rust service (`#[rutis::service]`) takes data and functions: an object
/// as its argument fails that call with `bad-argument`, saying where the
/// object came from (#687 §九).
/// objects-case(rust): declared-type-refuses
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rust_service_refuses_an_object_argument() {
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    pub struct Config {
        name: String,
    }
    pub struct Store;
    #[rutis::service]
    impl Store {
        pub fn put(&self, config: Config) -> u32 {
            config.name.len() as u32
        }
    }
    let (_a, _far, object) = owner("py").await;
    let root = rutis::Ctx::root().unwrap();
    root.provide(Store).unwrap();
    let service = root.get_at::<dyn HostDispatch>(&host_key("store")).unwrap();
    let error = match service.invoke("put", Value::List(vec![object])) {
        Err(error) => error,
        Ok(value) => settle(value).await.unwrap_err(),
    };
    assert_eq!(
        error.to_string(),
        "store.put: argument 1 should be a Config, got an object from py; \
         store takes data and functions, so pass a function for each method"
    );
    assert_eq!(error.code(), Some("bad-argument"));
    assert_eq!(error.data().unwrap()["argument"], json!(1));
}

/// The host's import of `object` given to a new host session `b`, whose far
/// end takes it: that session, the far end, and the relay's id there.
async fn hand(object: &Value) -> (Connection, Remote, u64) {
    let (b, far_b) = connected(Arc::new(NoExports));
    let holder = tokio::task::spawn_blocking(move || {
        let mut far_b = Remote::start_declaring(far_b, &TAKES_OBJECTS);
        let Frame::Invoke { id, args, .. } = far_b.read() else {
            panic!("invoke expected")
        };
        let WireValue::List(args) = args else {
            panic!("a list expected")
        };
        let relay = granted(&args[0]).0;
        far_b.ret(id, WireValue::Undefined);
        (far_b, relay)
    });
    b.ready().await.unwrap();
    b.invoke_async("t", "take", Value::List(vec![object.clone()]))
        .await
        .unwrap();
    let (far_b, relay) = holder.await.unwrap();
    (b, far_b, relay)
}

/// A method the object does not have: the owner answers `no-method` with
/// its data, and the holder gets it as it was, only that call failing.
/// objects-case(rust): no-method
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_method_is_answered_by_the_owner() {
    let (_a, mut far_a, object) = owner("node").await;
    let (b, mut far_b, relay) = hand(&object).await;
    drop(object);
    let script = tokio::task::spawn_blocking(move || {
        far_b.send(Frame::Call {
            id: "node:1".into(),
            path: vec![],
            reference: relay,
            method: Some("gett".into()),
            args: data(json!([])),
            sync: false,
        });
        let Frame::Call { id, method, .. } = (loop {
            match far_a.read() {
                Frame::Release { .. } => continue,
                frame => break frame,
            }
        }) else {
            panic!("the forwarded call expected")
        };
        assert_eq!(method.as_deref(), Some("gett"));
        let refused = Error::binding(
            crate::session::code::NO_METHOD,
            "Cache has no method gett (methods: get, set)".into(),
            Some(json!({ "object": "Cache", "method": "gett", "methods": ["get", "set"] })),
        );
        far_a.send(Frame::Throw {
            id,
            error: crate::session::protocol::Failure::from(refused),
        });
        let thrown = loop {
            match far_b.read_json() {
                frame if frame["op"] == "throw" => break frame,
                _ => {}
            }
        };
        (thrown, far_a, far_b)
    });
    let (thrown, _far_a, _far_b) = script.await.unwrap();
    let error = Error::from(
        serde_json::from_value::<crate::session::protocol::Failure>(thrown["error"].clone())
            .unwrap(),
    );
    assert_eq!(error.code(), Some("no-method"));
    assert_eq!(
        error.data(),
        Some(json!({ "object": "Cache", "method": "gett", "methods": ["get", "set"] }))
    );
    assert!(error
        .to_string()
        .contains("Cache has no method gett (methods: get, set)"));
    assert!(b.close_reason().is_none(), "the session goes on");
}

/// A call through a relay given up by its caller is cancelled on the
/// owner's side.
/// objects-case(rust): cancel
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_on_an_object_given_up_is_cancelled_at_its_owner() {
    let (_a, mut far_a, object) = owner("node").await;
    let (_b, mut far_b, relay) = hand(&object).await;
    let script = tokio::task::spawn_blocking(move || {
        far_b.send(Frame::Call {
            id: "node:1".into(),
            path: vec![],
            reference: relay,
            method: Some("slow".into()),
            args: data(json!([])),
            sync: false,
        });
        let forwarded = loop {
            match far_a.read() {
                Frame::Call { id, .. } => break id,
                _ => continue,
            }
        };
        far_b.send(Frame::Cancel {
            id: "node:1".into(),
        });
        loop {
            if let Frame::Cancel { id } = far_a.read() {
                assert_eq!(id, forwarded);
                break;
            }
        }
        (far_a, far_b)
    });
    let _ends = tokio::time::timeout(STEP, script)
        .await
        .expect("the cancel reaches the owner")
        .unwrap();
    drop(object);
}

/// Once given, an object stays callable whatever else lets go of it: the
/// host keeps nothing but the relay (the row that passed it may be gone),
/// and the holder's calls reach the owner, as for objects in one process.
/// objects-case(rust): outlives-its-row
/// objects-case(rust): send-call-state
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_stays_callable_through_its_relay_alone() {
    let (_a, mut far_a, object) = owner("node").await;
    let (_b, mut far_b, relay) = hand(&object).await;
    drop(object);
    let script = tokio::task::spawn_blocking(move || {
        far_b.send(Frame::Call {
            id: "node:1".into(),
            path: vec![],
            reference: relay,
            method: Some("count".into()),
            args: data(json!([2])),
            sync: false,
        });
        let (id, args) = loop {
            match far_a.read() {
                Frame::Call {
                    id,
                    reference: 3,
                    method: Some(method),
                    args,
                    ..
                } if method == "count" => break (id, args),
                Frame::Release { .. } => panic!("released while held"),
                _ => continue,
            }
        };
        assert_eq!(data_of(args), json!([2]));
        far_a.ret(id, data(json!(3)));
        match far_b.read() {
            Frame::Return { id, value } if id == "node:1" => data_of(value),
            _ => panic!("the result expected"),
        }
    });
    assert_eq!(script.await.unwrap(), json!(3));
}

/// One object given twice to the same runtime is one relay there; that
/// relay sent back to the host is the original object again.
/// objects-case(rust): same-proxy-home
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_given_twice_is_one_relay_and_comes_back_as_itself() {
    let (_a, _far_a, object) = owner("node").await;
    let (b, far_b) = connected(Arc::new(NoExports));
    let holder = tokio::task::spawn_blocking(move || {
        let mut far_b = Remote::start_declaring(far_b, &TAKES_OBJECTS);
        let mut relays = Vec::new();
        for _ in 0..2 {
            let Frame::Invoke { id, args, .. } = far_b.read() else {
                panic!("invoke expected")
            };
            let WireValue::List(args) = args else {
                panic!("a list expected")
            };
            let relay = granted(&args[0]).0;
            relays.push(relay);
            // Back home: the relay itself.
            far_b.ret(id, reference(relay, Kind::Object, true));
        }
        (far_b, relays)
    });
    b.ready().await.unwrap();
    for _ in 0..2 {
        let back = b
            .invoke_async("t", "take", Value::List(vec![object.clone()]))
            .await
            .unwrap();
        assert!(
            back.reference().unwrap() == object.clone().reference().unwrap(),
            "the relay comes back as the original"
        );
    }
    let (_far_b, relays) = holder.await.unwrap();
    assert_eq!(relays[0], relays[1], "one relay for one object");
}

/// Calls `get` on the object it holds, later, as a host service given an
/// object of another runtime does.
struct Poke(Arc<std::sync::OnceLock<Value>>);
impl Dispatch for Poke {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        let object = self.0.get().unwrap().clone().reference()?;
        Ok(Value::future(async move {
            object.call_method_async("get", Value::List(vec![])).await
        }))
    }
}

/// A host service handling a call of session `b` calls an object of
/// session `a`: the chain `a` gets is in `a`'s terms, `b`'s ids tagged with
/// `b`, so an id of `b`'s never looks like one of `a`'s own (a runtime
/// refuses a call whose chain holds its own id).
///
/// The ids are chosen to meet, as they did in CI (`objects_cross.rs`'s
/// Node → Go case, before the fix): `b`'s caller was waiting on `rust:2`
/// of `b`, and the call `a` gets is `rust:2` of `a` (its second, after
/// `give`). Untagged, `a`'s Node runtime would see its own id in the chain
/// and end the session ("invalid invocation").
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_service_calling_another_sessions_object_rebases_its_chain() {
    let (_a, mut far_a, object) = owner("node").await;
    let held = Arc::new(std::sync::OnceLock::new());
    held.set(object).unwrap();
    let (b, far_b) = connected(Arc::new(Poke(held)));
    let mut far_b =
        tokio::task::spawn_blocking(move || Remote::start_declaring(far_b, &TAKES_OBJECTS))
            .await
            .unwrap();
    b.ready().await.unwrap();
    let tag = b.tag().to_owned();
    let path = tokio::task::spawn_blocking(move || {
        far_b.send(Frame::Invoke {
            id: "node:1".into(),
            path: vec!["rust:2".into()],
            target: "t".into(),
            method: "poke".into(),
            args: data(json!([])),
            sync: false,
        });
        // Its answer is a future; awaited, it runs.
        let Frame::Return { value, .. } = far_b.read() else {
            panic!("return expected")
        };
        far_b.send(Frame::Await {
            id: "node:2".into(),
            path: vec![],
            reference: granted(&value).0,
            sync: false,
        });
        let (id, path) = loop {
            if let Frame::Call { id, path, .. } = far_a.read() {
                far_a.ret(id.clone(), data(json!(1)));
                break (id, path);
            }
        };
        (id, path, far_a, far_b)
    });
    let (id, path, _far_a, _far_b) = tokio::time::timeout(STEP, path)
        .await
        .expect("the object's method called")
        .unwrap();
    assert_eq!(id, "rust:2", "the ids meet");
    // What a runtime checks (node/rutis-runtime's session.mjs): a call
    // whose chain holds its own id is refused.
    assert!(!path.contains(&id), "{path:?} holds {id}");
    assert_eq!(path, vec![format!("{tag}/rust:2"), format!("{tag}/node:1")]);
}
