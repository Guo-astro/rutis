//! Sessions and links under injected faults (#178, plan 0.10 §4.1).
//!
//! Every channel here is a memory channel wrapped by
//! [`rutis_bridge::channel::testing::fault`]: a link dials through
//! [`Faulty`], a transport that wraps each channel it dials and can refuse
//! dials while "down"; a bare session pair wraps the caller's end. What
//! a fault does to the calls on top is the subject: a call in flight when
//! the connection drops, a slow connection and cancellation, calls made
//! while a link reconnects, cancellation and release across two links,
//! interleaved replies, and large and deep values.
//!
//! The `guarantee:` names are rows of the `bridge/` family in
//! docs/guarantees.md.
#![cfg(feature = "testing")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::channel::testing::{fault, Faults};
use rutis_bridge::channel::{ConnectError, PeerId};
use rutis_bridge::session::{host_key, settle, Error, HostDispatch};
use rutis_bridge::session::{Connection, Dispatch, Endpoint, Reply, Value};
use rutis_bridge::{
    peer_key, Accepting, Credential, Dialer, ExportPlugin, Identity, IdentityPlugin, ImportPlugin,
    LinkConfig, LinkPlugin, LinkState, Peer, Registrations, Retry, StaticIdentity,
};
use rutis_test_support::{eventually, scaled, within, HANG_GUARD};
use serde_json::{json, Value as Json};
use tokio::sync::{mpsc, oneshot, watch};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

fn quick() -> Retry {
    Retry {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
        rejected: Duration::from_millis(100),
        handshake: scaled(HANG_GUARD),
        // No jitter: the same delays every run (#231).
        jitter: 0.0,
        ..Retry::default()
    }
}

/// A memory dialer, with every channel it dials wrapped by `fault`.
/// While `down`, dials are refused as retryable, as a far end that cannot
/// be reached: a test holds a link in its reconnecting state that way.
struct Faulty {
    inner: Dialer,
    dialed: Mutex<Vec<Faults>>,
    down: AtomicBool,
}

impl Faulty {
    /// The faults of the `n`th channel dialed.
    fn dialed(&self, n: usize) -> Faults {
        self.dialed.lock().unwrap()[n].clone()
    }

    fn dialer(self: &Arc<Self>) -> Dialer {
        let faulty = self.clone();
        Arc::new(move |credential| {
            let faulty = faulty.clone();
            Box::pin(async move {
                if faulty.down.load(Ordering::SeqCst) {
                    return Err(ConnectError::Retryable {
                        reason: "faulty: down".into(),
                    });
                }
                let (channel, faults) = fault((faulty.inner)(credential).await?);
                faulty.dialed.lock().unwrap().push(faults);
                Ok(channel)
            })
        })
    }
}

/// `main` listens for `mac` on the memory transport; `mac` dials it through
/// [`Faulty`] and reconnects when its session ends.
struct Linked {
    main: Ctx,
    mac: Ctx,
    mac_states: watch::Receiver<LinkState>,
    faulty: Arc<Faulty>,
}

async fn linked() -> Linked {
    let transport = Registrations::new();
    let main = Ctx::root().unwrap();
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    ));
    main.plugin(LinkPlugin::new(
        LinkConfig::listen(
            id("mac"),
            "main",
            Accepting {
                registrations: transport.clone(),
                listener: "main-in".into(),
                local: id("main"),
            },
        )
        .retry(quick()),
    ));

    let mac = Ctx::root().unwrap();
    let faulty = Arc::new(Faulty {
        inner: rutis_bridge::memory::dialer(transport, "main-in"),
        dialed: Mutex::default(),
        down: AtomicBool::new(false),
    });
    mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    ));
    let link = LinkPlugin::new(LinkConfig::dial(id("main"), "mac", faulty.dialer()).retry(quick()));
    let mac_states = link.state();
    mac.plugin(link);
    Linked {
        main,
        mac,
        mac_states,
        faulty,
    }
}

fn peer(root: &Ctx, of: &str) -> Option<Arc<Peer>> {
    root.get_at::<Peer>(&peer_key(&id(of)))
}

/// The peer `of` at `root` once its session is `generation`.
async fn generation(root: &Ctx, of: &str, generation: u64) -> Arc<Peer> {
    eventually(
        || peer(root, of).filter(|peer| peer.generation() == generation),
        &format!("generation {generation} of Peer#{of}"),
    )
    .await
}

/// A [`Dispatch`] from a closure.
struct Serve<F>(F);
impl<F> Dispatch for Serve<F>
where
    F: Fn(&Connection, &str, &str, Value) -> Reply + Send + Sync + 'static,
{
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        (self.0)(peer, target, method, args)
    }
}

fn nothing() -> Arc<dyn Dispatch> {
    Arc::new(Serve(|_: &Connection, _: &str, method: &str, _: Value| {
        Err(Error::Value(format!("{method} is not served here")))
    }))
}

/// Two Rust sessions on a memory pair, the caller's end wrapped by `fault`.
async fn sessions(callee: Arc<dyn Dispatch>) -> (Connection, Connection, Faults) {
    let (near, far) = rutis_bridge::memory::pair();
    let (near, faults) = fault(near);
    let caller = Connection::open(
        near,
        nothing(),
        Endpoint::rust(id("caller")).expect(id("callee")),
    )
    .unwrap();
    let served = Connection::open(
        far,
        callee,
        Endpoint::rust(id("callee")).expect(id("caller")),
    )
    .unwrap();
    within("the caller's handshake", caller.ready())
        .await
        .unwrap();
    within("the callee's handshake", served.ready())
        .await
        .unwrap();
    (caller, served, faults)
}

/// Sends on `0` when dropped: a callee's future ends this way when the
/// caller's cancellation and release reach it.
struct Dropped(Option<oneshot::Sender<()>>);
impl Drop for Dropped {
    fn drop(&mut self) {
        if let Some(dropped) = self.0.take() {
            let _ = dropped.send(());
        }
    }
}

/// Whether `future` is unfinished when polled once, now.
fn pending_now<F: std::future::Future + Unpin>(future: &mut F) -> bool {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    std::pin::Pin::new(future).poll(&mut cx).is_pending()
}

/// The seed for this run: `RUTIS_SEED` if set, otherwise random. Printed,
/// so the test output of a failing run carries it.
fn seed() -> u64 {
    let seed = match std::env::var("RUTIS_SEED") {
        Ok(value) => value
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("RUTIS_SEED must be a number, got {value:?}")),
        Err(_) => {
            use std::hash::{BuildHasher, Hasher};
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish()
        }
    };
    println!("fault_links seed: {seed} (replay with RUTIS_SEED={seed})");
    seed
}

/// A small deterministic generator, so a failing order can be replayed.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.next(i as u64 + 1) as usize);
        }
    }
}

// ── 1. A call in flight when the connection drops ────────────────────

/// risk E1; requirements-protocol-plugins §6 ("communication broken ...
/// fail explicitly, no false success, no automatic retry of side effects").
/// guarantee: bridge/in-flight-call-fails-once
///
/// mac calls main over a link; while main runs the call, mac's connection
/// drops. The call fails at mac (not success, not a hang), main ran it
/// exactly once, and the link reconnects without sending it again: the
/// next call is the second and last main runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_in_flight_when_the_link_drops_fails_and_ran_once() {
    let linked = linked().await;
    let at_main = generation(&linked.main, "mac", 1).await;
    let at_mac = generation(&linked.mac, "main", 1).await;

    let runs: Arc<Mutex<Vec<Json>>> = Arc::default();
    let (started, mut running) = mpsc::unbounded_channel();
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let _first = at_main
        .register("work", {
            let runs = runs.clone();
            Arc::new(move |_: &Connection, _: &str, _: &str, args: Value| {
                runs.lock().unwrap().push(args.json()?);
                let _ = started.send(());
                // Runs until the test lets it finish: the call is in flight
                // at main when mac's connection drops.
                let _ = gate.lock().unwrap().recv_timeout(scaled(HANG_GUARD));
                Ok(Value::Data(json!("done")))
            })
        })
        .unwrap();

    let call = tokio::spawn({
        let session = at_mac.connection().clone();
        async move {
            session
                .invoke_async("", "work.run", Value::Data(json!(["first"])))
                .await
        }
    });
    within("main to run the call", running.recv())
        .await
        .unwrap();
    linked.faulty.dialed(0).drop_and_close();

    let failed = within("the call in flight to end", call).await.unwrap();
    assert!(
        matches!(&failed, Err(Error::Transport(_))),
        "a call cut off in flight fails as a transport failure: {failed:?}"
    );
    // main's run finishes after its caller is gone; its reply goes nowhere.
    open.send(()).unwrap();

    // The link comes back; the call cut off is not sent again.
    let at_main = generation(&linked.main, "mac", 2).await;
    let at_mac = generation(&linked.mac, "main", 2).await;
    let _second = at_main
        .register("work", {
            let runs = runs.clone();
            Arc::new(move |_: &Connection, _: &str, _: &str, args: Value| {
                runs.lock().unwrap().push(args.json()?);
                Ok(Value::Data(json!("done")))
            })
        })
        .unwrap();
    let offers = at_mac.offers();
    eventually(
        || offers.borrow().families.contains("work").then_some(()),
        "work offered on the new session",
    )
    .await;
    let reply = within(
        "a call on the new session",
        at_mac
            .connection()
            .invoke_async("", "work.run", Value::Data(json!(["second"]))),
    )
    .await
    .unwrap();
    assert_eq!(reply.json().unwrap(), json!("done"));
    assert_eq!(
        *runs.lock().unwrap(),
        [json!(["first"]), json!(["second"])],
        "main ran the cut-off call once and only the new call after it"
    );
}

// ── 2. A slow connection, and cancellation ───────────────────────────

/// risk P6 (single hop, under delay).
/// guarantee: bridge/cancel-reaches-callee
///
/// Every message waits 20 ms each way. The caller gives up awaiting an
/// async call: the cancellation reaches the callee, which stops awaiting
/// it on the caller's behalf while the caller still holds the result;
/// releasing the result then drops the callee's future.
#[tokio::test(flavor = "multi_thread")]
async fn under_delay_a_cancellation_and_a_release_reach_the_callee() {
    let (started, mut running) = mpsc::unbounded_channel();
    let (dropped, mut gone) = oneshot::channel();
    let dropped = Mutex::new(Some(dropped));
    let callee = Arc::new(Serve(move |_: &Connection, _: &str, _: &str, _: Value| {
        let started = started.clone();
        let dropped = Dropped(dropped.lock().unwrap().take());
        Ok(Value::future(async move {
            let _dropped = dropped;
            let _ = started.send(());
            std::future::pending::<Reply>().await
        }))
    }));
    let (caller, served, faults) = sessions(callee).await;
    // Fault injection, not a pause: each message is held up by 20 ms.
    faults.delay(Duration::from_millis(20));

    let promise = within(
        "the invoke",
        caller.invoke_async("", "work", Value::List(vec![Value::Signal])),
    )
    .await
    .unwrap();
    let mut awaited = Box::pin(settle(promise.clone()));
    tokio::select! {
        biased;
        ended = &mut awaited => panic!("the call ends only when cancelled: {ended:?}"),
        started = within("the callee to start", running.recv()) => started.unwrap(),
    }
    let mut draining = Box::pin(served.drain());
    assert!(
        pending_now(&mut draining),
        "the callee awaits the call for the caller"
    );

    drop(awaited);
    within("the cancellation at the callee", draining).await;
    assert!(
        matches!(gone.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
        "the caller still holds the result: the callee's future lives on"
    );
    drop(promise);
    within("the release at the callee", &mut gone)
        .await
        .unwrap();
    faults.heal();
}

// ── 3. New calls while a link reconnects ─────────────────────────────

/// risk E1, and the generation rule of links (a resource of an older
/// session never applies to a newer one).
/// guarantee: bridge/no-call-on-a-dead-session
///
/// mac's connection drops and redialing fails for a while. Calls made
/// meanwhile, through the old peer or whatever peer mac has, fail
/// explicitly; once the link is back, calls through the new peer reach
/// main's new session, and the old peer never works again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_while_a_link_reconnects_fail_or_reach_the_new_session() {
    let linked = linked().await;
    let at_main = generation(&linked.main, "mac", 1).await;
    let old = generation(&linked.mac, "main", 1).await;

    // main answers with the tag of the session each call arrived on.
    let arrived: Arc<Mutex<Vec<(String, Json)>>> = Arc::default();
    let echo = |arrived: Arc<Mutex<Vec<(String, Json)>>>| {
        Arc::new(move |session: &Connection, _: &str, _: &str, args: Value| {
            let tag = session.tag().to_owned();
            arrived.lock().unwrap().push((tag.clone(), args.json()?));
            Ok(Value::Data(json!(tag)))
        })
    };
    let _first = at_main.register("echo", echo(arrived.clone())).unwrap();
    let old_tag = at_main.connection().tag().to_owned();
    let offers = old.offers();
    eventually(
        || offers.borrow().families.contains("echo").then_some(()),
        "echo offered",
    )
    .await;
    let reply = old
        .connection()
        .invoke_async("", "echo.call", Value::Data(json!(["before"])))
        .await
        .unwrap();
    assert_eq!(reply.json().unwrap(), json!(old_tag));

    linked.faulty.down.store(true, Ordering::SeqCst);
    linked.faulty.dialed(0).drop_and_close();
    eventually(
        || matches!(*linked.mac_states.borrow(), LinkState::Waiting { .. }).then_some(()),
        "mac's link to fail to redial",
    )
    .await;

    // Disconnected: the old peer and whatever peer mac has fail at once.
    for n in 0..3 {
        let failed = within(
            "a call on the old session",
            old.connection()
                .invoke_async("", "echo.call", Value::Data(json!(["old", n]))),
        )
        .await;
        assert!(failed.is_err(), "the old session is gone: {failed:?}");
    }
    assert!(
        peer(&linked.mac, "main").is_none(),
        "no peer is provided while the link is down"
    );

    // Calls keep coming while the link reconnects; each fails or reaches
    // the new session.
    linked.faulty.down.store(false, Ordering::SeqCst);
    let at_main = generation(&linked.main, "mac", 2).await;
    let new_tag = at_main.connection().tag().to_owned();
    assert_ne!(new_tag, old_tag);
    let _second = at_main.register("echo", echo(arrived.clone())).unwrap();
    let mut n = 0;
    let new = within("a call to reach the new session", async {
        loop {
            n += 1;
            let current = peer(&linked.mac, "main");
            let session = current
                .as_ref()
                .map_or_else(|| old.connection().clone(), |p| p.connection().clone());
            match session
                .invoke_async("", "echo.call", Value::Data(json!(["during", n])))
                .await
            {
                Ok(reply) => {
                    assert_eq!(reply.json().unwrap(), json!(new_tag));
                    break current.unwrap();
                }
                // Not linked yet, or echo not registered yet on main's side.
                Err(_) => tokio::task::yield_now().await,
            }
        }
    })
    .await;
    assert_eq!(new.generation(), 2);
    assert!(old
        .connection()
        .invoke_async("", "echo.call", Value::Data(json!(["old again"])))
        .await
        .is_err());

    let arrived = arrived.lock().unwrap().clone();
    assert_eq!(arrived[0], (old_tag, json!(["before"])));
    assert!(
        arrived[1..].iter().all(|(tag, _)| *tag == new_tag),
        "every call after the cut reached the new session: {arrived:?}"
    );
    assert_eq!(arrived.len(), 2, "only the successful call arrived");
}

// ── 4. Cancellation and release across two links ─────────────────────

/// b's clock: `later` never finishes; it says when it starts and when it is
/// dropped.
struct Clock {
    started: mpsc::UnboundedSender<()>,
    dropped: Mutex<Option<oneshot::Sender<()>>>,
}
impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _: Value) -> Reply {
        match method {
            "later" => {
                let started = self.started.clone();
                let dropped = Dropped(self.dropped.lock().unwrap().take());
                Ok(Value::future(async move {
                    let _dropped = dropped;
                    let _ = started.send(());
                    std::future::pending::<Reply>().await
                }))
            }
            _ => Err(Error::Value(method.into())),
        }
    }
    fn methods(&self) -> Option<Json> {
        Some(json!({ "later": "async" }))
    }
}

async fn node(identity: StaticIdentity) -> Ctx {
    let root = Ctx::root().unwrap();
    let name = identity.local().to_string();
    root.plugin(IdentityPlugin::new(&name, identity));
    root
}

/// risks P6, E9.
/// guarantee: bridge/multihop-cancel-and-release
///
/// b exports `clock` to a, a re-exports it to c (as in `multihop.rs`). c
/// calls the async `later` and gives it up, dropping its result. Each hop
/// passes both on: a stops awaiting for c (cancel) and drops its own
/// future for c (release), which cancels and releases its call to b; b
/// then stops awaiting for a and drops `later`'s future. A Rust callee is
/// cancelled by its future being dropped, so `later` ending at b needs the
/// release to have passed both hops.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_and_release_pass_hop_by_hop() {
    let transport = Registrations::new();
    let a = node(
        StaticIdentity::new(id("a"))
            .accept_token("b-token", id("b"))
            .accept_token("c-token", id("c")),
    )
    .await;
    a.plugin(LinkPlugin::new(
        LinkConfig::listen(
            id("b"),
            "a",
            Accepting {
                registrations: transport.clone(),
                listener: "a-in".into(),
                local: id("a"),
            },
        )
        .retry(quick()),
    ));
    a.plugin(LinkPlugin::new(
        LinkConfig::listen(
            id("c"),
            "a",
            Accepting {
                registrations: transport.clone(),
                listener: "a-in".into(),
                local: id("a"),
            },
        )
        .retry(quick()),
    ));
    a.plugin(ImportPlugin::new(id("b"), ["clock"]));
    a.plugin(ExportPlugin::new(id("c"), ["clock"]));

    let (started, mut running) = mpsc::unbounded_channel();
    let (dropped, mut gone) = oneshot::channel();
    let b =
        node(StaticIdentity::new(id("b")).present(id("a"), Credential::Bearer("b-token".into())))
            .await;
    let _provided = b
        .provide_at(
            host_key("clock"),
            Arc::new(Clock {
                started,
                dropped: Mutex::new(Some(dropped)),
            }),
        )
        .unwrap();
    b.plugin(LinkPlugin::new(
        LinkConfig::dial(
            id("a"),
            "b",
            rutis_bridge::memory::dialer(transport.clone(), "a-in"),
        )
        .retry(quick()),
    ));
    b.plugin(ExportPlugin::new(id("a"), ["clock"]));

    let c =
        node(StaticIdentity::new(id("c")).present(id("a"), Credential::Bearer("c-token".into())))
            .await;
    c.plugin(LinkPlugin::new(
        LinkConfig::dial(
            id("a"),
            "c",
            rutis_bridge::memory::dialer(transport.clone(), "a-in"),
        )
        .retry(quick()),
    ));
    c.plugin(ImportPlugin::new(id("a"), ["clock"]));

    let clock = eventually(
        || c.get_at::<dyn HostDispatch>(&host_key("clock")),
        "the clock at c",
    )
    .await;
    // a's session with c, and b's session with a.
    let a_for_c = eventually(|| peer(&a, "c"), "Peer#c at a").await;
    let b_for_a = eventually(|| peer(&b, "a"), "Peer#a at b").await;

    let promise = clock.invoke("later", Value::List(vec![])).unwrap();
    let mut awaited = Box::pin(settle(promise));
    tokio::select! {
        biased;
        ended = &mut awaited => panic!("later ends only when cancelled: {ended:?}"),
        started = within("later to start at b", running.recv()) => started.unwrap(),
    }
    let mut at_a = Box::pin(a_for_c.connection().drain());
    let mut at_b = Box::pin(b_for_a.connection().drain());
    assert!(pending_now(&mut at_a), "a awaits later for c");
    assert!(pending_now(&mut at_b), "b awaits later for a");

    // c gives the call up: its result is dropped with it.
    drop(awaited);
    within("the cancellation at a", at_a).await;
    within("the cancellation at b", at_b).await;
    within("the release at b", &mut gone).await.unwrap();
}

// ── 5. Concurrent calls, replies interleaved ─────────────────────────

/// guarantee: bridge/replies-match-calls
///
/// 32 calls in flight at once; the callee finishes them in an order drawn
/// from the printed seed. Each reply reaches its own call, at the moment
/// its call is finished, whatever the order.
#[tokio::test(flavor = "multi_thread")]
async fn interleaved_replies_reach_their_own_calls() {
    const CALLS: usize = 32;
    let mut order = Lcg(seed());
    let gates: Arc<Mutex<HashMap<u64, oneshot::Sender<()>>>> = Arc::default();
    let (polled, mut awaited) = mpsc::unbounded_channel::<u64>();
    let callee = Arc::new(Serve({
        let gates = gates.clone();
        move |_: &Connection, _: &str, _: &str, args: Value| {
            let [n]: [u64; 1] = rutis_bridge::session::decode_value(args)?;
            let (open, gate) = oneshot::channel();
            gates.lock().unwrap().insert(n, open);
            let polled = polled.clone();
            Ok(Value::future(async move {
                let _ = polled.send(n);
                gate.await
                    .map_err(|_| Error::Value("gate dropped".into()))?;
                Ok(Value::Data(json!({ "call": n, "square": n * n })))
            }))
        }
    }));
    let (caller, _served, _faults) = sessions(callee).await;

    let (replied, mut replies) = mpsc::unbounded_channel();
    for n in 0..CALLS as u64 {
        let caller = caller.clone();
        let replied = replied.clone();
        tokio::spawn(async move {
            let reply = match caller
                .invoke_async("", "square", Value::Data(json!([n])))
                .await
            {
                Ok(promise) => settle(promise).await,
                Err(error) => Err(error),
            };
            let _ = replied.send((n, reply));
        });
    }
    // Every call is awaited at the callee before any finishes.
    let mut waiting = Vec::new();
    for _ in 0..CALLS {
        waiting.push(within("every call awaited", awaited.recv()).await.unwrap());
    }
    waiting.sort_unstable();
    assert_eq!(waiting, (0..CALLS as u64).collect::<Vec<_>>());

    let mut finishing: Vec<u64> = (0..CALLS as u64).collect();
    order.shuffle(&mut finishing);
    println!("finishing order: {finishing:?}");
    for n in finishing {
        gates.lock().unwrap().remove(&n).unwrap().send(()).unwrap();
        // Only call n can finish now: the next reply is its.
        let (call, reply) = within("the reply", replies.recv()).await.unwrap();
        assert_eq!(call, n, "the reply that arrived is of the call finished");
        assert_eq!(
            reply.unwrap().json().unwrap(),
            json!({ "call": n, "square": n * n })
        );
    }
}

// ── 6. Large and deep values ─────────────────────────────────────────

fn nested(depth: usize) -> Json {
    (0..depth).fold(
        json!("bottom"),
        |inner, level| json!({ "level": level, "inner": inner }),
    )
}

/// guarantee: bridge/large-and-deep-values
///
/// A call whose argument and result are each close to 16 MiB (the local
/// transport's and WebSocket's default message limit) arrives whole both
/// ways, and so does a value nested 64 deep (the limit: see
/// `a_value_too_deep_fails_its_call_and_the_session_goes_on`); the
/// session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn large_and_deep_values_cross_a_session_whole() {
    let callee = Arc::new(Serve(|_: &Connection, _: &str, _: &str, args: Value| {
        Ok(Value::Data(args.json()?))
    }));
    let (caller, _served, _faults) = sessions(callee).await;

    let large = "x".repeat(16 * 1024 * 1024 - 4096);
    let back = within(
        "a large value both ways",
        caller.invoke_async("", "echo", Value::Data(json!([large]))),
    )
    .await
    .unwrap()
    .json()
    .unwrap();
    assert!(back == json!([large]), "the large value arrived changed");

    // The callee returns the whole argument list: 64 levels with it.
    let deep = nested(63);
    let back = within(
        "a deep value both ways",
        caller.invoke_async("", "echo", Value::Data(json!([deep]))),
    )
    .await
    .unwrap()
    .json()
    .unwrap();
    assert_eq!(back, json!([deep]));
}

/// guarantee: bridge/value-limits; risk E1, P7.
///
/// A value nested deeper than the limit (64 levels, `MAX_DEPTH`) fails
/// its own call, explicitly, as a value error naming the limit; a call in
/// flight on the same session at the time, and calls after it, go on
/// (#279: the receiver's JSON parser used to end the whole session, at
/// 125 levels of data or 62 of lists). Each argument may nest 64 levels:
/// the argument list is not a level.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_too_deep_fails_its_call_and_the_session_goes_on() {
    use rutis_bridge::session::MAX_DEPTH;
    assert_eq!(MAX_DEPTH, 64);
    let (open, gate) = oneshot::channel::<()>();
    let gate = Arc::new(Mutex::new(Some(gate)));
    // Both methods return their first argument.
    let callee = Arc::new(Serve(
        move |_: &Connection, _: &str, method: &str, args: Value| {
            let first = args.list()?.into_iter().next().unwrap_or(Value::Undefined);
            if method != "slow" {
                return Ok(first);
            }
            let gate = gate.lock().unwrap().take().unwrap();
            Ok(Value::future(async move {
                gate.await.unwrap();
                Ok(first)
            }))
        },
    ));
    let (caller, _served, _faults) = sessions(callee).await;

    let slow = tokio::spawn({
        let caller = caller.clone();
        async move {
            settle(
                caller
                    .invoke_async("", "slow", Value::Data(json!(["slow"])))
                    .await?,
            )
            .await
        }
    });
    // Lists holding references: each level was two on the wire.
    let lists = |depth: usize| {
        (0..depth).fold(Value::Data(json!("bottom")), |inner, _| {
            Value::List(vec![inner, Value::callback(|_| Ok(Value::Undefined))])
        })
    };
    fn depth(value: &Value) -> usize {
        match value {
            Value::List(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
            _ => 0,
        }
    }
    for (case, args) in [
        ("data 200 deep", Value::Data(json!([nested(200)]))),
        ("data 65 deep", Value::Data(json!([nested(65)]))),
        ("lists 65 deep", Value::List(vec![lists(65)])),
    ] {
        let failed = within(case, caller.invoke_async("", "echo", args)).await;
        match &failed {
            Err(Error::Value(message)) => {
                assert!(message.contains("64 levels"), "{case}: {message}")
            }
            other => panic!("{case}: a value too deep fails its call as a value error: {other:?}"),
        }
    }
    // At the limit, both cross, both ways.
    let back = within(
        "data 64 deep",
        caller.invoke_async("", "echo", Value::Data(json!([nested(64)]))),
    )
    .await;
    assert_eq!(back.unwrap().json().unwrap(), nested(64));
    let back = within(
        "lists 64 deep",
        caller.invoke_async("", "echo", Value::List(vec![lists(64)])),
    )
    .await;
    assert_eq!(depth(&back.unwrap()), 64);

    open.send(()).unwrap();
    let slow = within("the call in flight", slow).await.unwrap();
    assert_eq!(slow.unwrap().json().unwrap(), json!("slow"));
    let back = within(
        "a call after it",
        caller.invoke_async("", "echo", Value::Data(json!(["after"]))),
    )
    .await;
    assert_eq!(back.unwrap().json().unwrap(), json!("after"));
    assert!(caller.close_reason().is_none());
}

/// guarantee: bridge/large-and-deep-values, bridge/bad-values; risk E1.
///
/// Over a real WebSocket connection whose limit is 64 KiB: a call whose
/// argument and result come within 512 bytes of the limit succeeds; one
/// whose argument is over it fails explicitly before it is sent, naming
/// the argument, the size and the limit, and the callee never ran it; the
/// connection stays open and the next call is answered (#675: it used to
/// close with 1009).
#[cfg(feature = "websocket")]
#[tokio::test(flavor = "multi_thread")]
async fn over_websocket_a_value_near_the_limit_passes_and_one_over_it_fails() {
    use rutis_bridge::websocket::{self, Limits, ListenerConfig, Trust};
    use rutis_bridge::Registration;

    const LIMIT: usize = 64 * 1024;
    let limits = Limits {
        max_message: LIMIT,
        ..Limits::default()
    };
    let registrations = Registrations::new();
    let server = websocket::listen(
        &ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap()),
        &limits,
        websocket::admit_registered(registrations.clone(), "public"),
    )
    .unwrap();
    let (deliver, mut accepted) = mpsc::unbounded_channel();
    let _registered = registrations
        .register(
            &id("callee"),
            Registration {
                listener: "public".into(),
                peer: id("caller"),
                identity: Arc::new(
                    StaticIdentity::new(id("callee")).accept_token("caller-token", id("caller")),
                ),
                protocol: rutis_bridge::protocol(),
                deliver: Box::new(move |channel| {
                    let _ = deliver.send(channel);
                }),
            },
        )
        .unwrap();
    let identity = StaticIdentity::new(id("caller"))
        .present(id("callee"), Credential::Bearer("caller-token".into()));
    let address = format!("ws://{}/rutis", server.local_addr());
    let dialed = within(
        "the dial",
        websocket::dial(
            &address,
            &rutis_bridge::protocol(),
            identity.credential(&id("callee")).as_ref(),
            &Trust::default(),
            &limits,
        ),
    )
    .await
    .unwrap();
    let accepted = within("the accepted connection", accepted.recv())
        .await
        .unwrap();

    let runs = Arc::new(Mutex::new(Vec::new()));
    let callee = Arc::new(Serve({
        let runs = runs.clone();
        move |_: &Connection, _: &str, _: &str, args: Value| {
            let [text]: [String; 1] = rutis_bridge::session::decode_value(args)?;
            runs.lock().unwrap().push(text.len());
            Ok(Value::Data(json!(text)))
        }
    }));
    let caller = Connection::open(
        dialed,
        nothing(),
        Endpoint::rust(id("caller")).expect(id("callee")),
    )
    .unwrap();
    let served = Connection::open(
        accepted,
        callee,
        Endpoint::rust(id("callee")).expect(id("caller")),
    )
    .unwrap();
    within("the caller's handshake", caller.ready())
        .await
        .unwrap();
    within("the callee's handshake", served.ready())
        .await
        .unwrap();

    // The frames around the text take well under 512 bytes.
    let near = "n".repeat(LIMIT - 512);
    let back = within(
        "a value near the limit both ways",
        caller.invoke_async("", "echo", Value::Data(json!([near]))),
    )
    .await
    .unwrap();
    assert!(back.json().unwrap() == json!(near));

    let over = "o".repeat(LIMIT + 1);
    let failed = within(
        "a value over the limit",
        caller.invoke_async("", "echo", Value::Data(json!([over]))),
    )
    .await;
    assert!(
        matches!(&failed, Err(Error::Value(message))
            if message.starts_with("echo: argument 1 cannot be sent: the message would be ")
                && message.ends_with(" bytes, over the limit of 65536 bytes")),
        "a value over the limit fails explicitly: {:?}",
        failed.as_ref().map(|_| "a reply")
    );
    let back = within(
        "a call after it",
        caller.invoke_async("", "echo", Value::Data(json!(["after"]))),
    )
    .await
    .unwrap();
    assert_eq!(back.json().unwrap(), json!("after"));
    assert!(caller.close_reason().is_none() && served.close_reason().is_none());
    assert_eq!(
        *runs.lock().unwrap(),
        [LIMIT - 512, 5],
        "the callee ran only the calls within the limit"
    );
}
