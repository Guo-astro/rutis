//! Revoking an identity at runtime (#273): links and an access point on
//! one loopback WebSocket listener, the identity layer's [`Revocations`]
//! cutting one far end off while everything keeps running.
//!
//! `main` listens on `public` for the peers `mac` and `pi` (static tokens,
//! a [`Revocable`] identity) and has an access point `phone` there whose
//! runtimes present a shared token and greet with an id of their own (a
//! [`Revocable`] admission). Revoking an identity closes only the
//! connections accepted as it: in-flight calls on them fail once, as a
//! transport failure; later connections are refused as a wrong credential
//! is (403 at the upgrade, or close code 4003 after the hello), and the
//! dialer waits them out as `AuthRejected`. Everything else on the
//! listener carries on.
//!
//! The `guarantee:` names are rows of the `bridge/` family in
//! docs/guarantees.md.
#![cfg(feature = "websocket")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{Call, Listener};
use rutis::host::diagnostics::{ServiceChange, ServiceChanged};
use rutis::host::{BoxFuture, CtxHost, TypeKey};
use rutis::Ctx;
use rutis_bridge::channel::{Channel, ConnectError, PeerId};
use rutis_bridge::session::{Connection, Dispatch, Endpoint, Error, Format, Reply, Value};
use rutis_bridge::websocket::{Config, ListenerConfig, WebSocketPlugin};
use rutis_bridge::{
    peer_key, protocol, Accept, Accepted, AccessEvent, AccessPointConfig, AccessPointPlugin,
    AdmissionPlugin, Credential, Dial, Identity, IdentityPlugin, LinkConfig, LinkPlugin, LinkState,
    Peer, Registration, Release, Retry, Revocable, Revocations, SharedToken, StaticIdentity,
    Transport,
};
use rutis_test_support::{eventually, scaled, within, HANG_GUARD};
use serde_json::json;
use tokio::sync::{broadcast, watch};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// Keeps what the access point accepted, by id.
#[derive(Clone, Default)]
struct Kept(Arc<Mutex<Vec<Accepted>>>);

impl Kept {
    fn current(&self, of: &str) -> Option<Accepted> {
        let accepted = self.0.lock().unwrap();
        accepted.iter().rev().find(|a| a.id() == &id(of)).cloned()
    }
}

impl Accept for Kept {
    fn accept(&self, connection: Accepted) -> BoxFuture<'_, Result<Release, String>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(connection);
            let release: Release = Box::new(|| Box::pin(async {}));
            Ok(release)
        })
    }
}

/// `main`: the listener, its links to `mac` and `pi`, its access point
/// `phone`, and the revocations of each layer.
struct Main {
    root: Ctx,
    address: String,
    /// The static links' identity.
    peers: Revocations,
    /// The access point's admission.
    runtimes: Revocations,
    mac: watch::Receiver<LinkState>,
    pi: watch::Receiver<LinkState>,
    kept: Kept,
    events: broadcast::Receiver<AccessEvent>,
}

async fn main() -> Main {
    let root = Ctx::root().unwrap();
    let websocket = WebSocketPlugin::new(Config::new().listener(ListenerConfig::new(
        "public",
        "127.0.0.1:0".parse().unwrap(),
        id("main"),
    )))
    .unwrap();
    let handle = websocket.clone();
    (&root.plugin(websocket)).await.unwrap();
    let transport = handle.transport().unwrap();
    let address = format!("ws://{}/rutis", transport.local_addr("public").unwrap());

    let peers = Revocations::new();
    (&root.plugin(IdentityPlugin::new(
        "main",
        Revocable::new(
            StaticIdentity::new(id("main"))
                .accept_token("mac-token", id("mac"))
                .accept_token("pi-token", id("pi")),
            peers.clone(),
        ),
    )))
        .await
        .unwrap();
    let mut states = Vec::new();
    for peer in ["mac", "pi"] {
        let link = LinkPlugin::new(LinkConfig::listen(id(peer), "websocket", "main", "public"));
        states.push(link.state());
        (&root.plugin(link)).await.unwrap();
    }

    let runtimes = Revocations::new();
    (&root.plugin(AdmissionPlugin::new(
        "phone",
        Revocable::new(SharedToken::new(id("main"), "shared"), runtimes.clone()),
    )))
        .await
        .unwrap();
    let kept = Kept::default();
    let access = AccessPointPlugin::new(
        AccessPointConfig::new(id("phone"), "websocket", "phone", "public")
            .handshake(scaled(HANG_GUARD)),
    )
    .accept(kept.clone());
    let events = access.events();
    (&root.plugin(access)).await.unwrap();

    let pi = states.pop().unwrap();
    let mac = states.pop().unwrap();
    Main {
        root,
        address,
        peers,
        runtimes,
        mac,
        pi,
        kept,
        events,
    }
}

/// A far end dialing `main` as `name` with `token`: a static peer, or a
/// runtime dialing out to the access point (a link greets with its
/// identity's id, as a runtime does).
struct Far {
    root: Ctx,
    state: watch::Receiver<LinkState>,
}

async fn dial(address: &str, name: &str, token: &str) -> Far {
    dial_with(address, name, token, Revocations::new()).await
}

async fn dial_with(address: &str, name: &str, token: &str, revocations: Revocations) -> Far {
    let root = Ctx::root().unwrap();
    (&root.plugin(WebSocketPlugin::new(Config::new()).unwrap()))
        .await
        .unwrap();
    (&root.plugin(IdentityPlugin::new(
        name,
        Revocable::new(
            StaticIdentity::new(id(name)).present(id("main"), Credential::Bearer(token.into())),
            revocations,
        ),
    )))
        .await
        .unwrap();
    let link = LinkPlugin::new(LinkConfig::dial(id("main"), "websocket", name, address));
    let state = link.state();
    (&root.plugin(link)).await.unwrap();
    Far { root, state }
}

fn peer(root: &Ctx, of: &str) -> Option<Arc<Peer>> {
    root.get_at::<Peer>(&peer_key(&id(of)))
}

async fn ready(states: &watch::Receiver<LinkState>, what: &str) {
    let states = states.clone();
    eventually(
        || matches!(*states.borrow(), LinkState::Ready { .. }).then_some(()),
        what,
    )
    .await;
}

/// Wait for a state matching `check`.
async fn state(
    states: &watch::Receiver<LinkState>,
    what: &str,
    check: impl Fn(&LinkState) -> bool,
) -> LinkState {
    let states = states.clone();
    eventually(
        || {
            let state = states.borrow().clone();
            check(&state).then_some(state)
        },
        what,
    )
    .await
}

/// Waiting out an authentication failure: 30 s, as for a wrong token.
fn auth_rejected(state: &LinkState) -> bool {
    matches!(
        state,
        LinkState::Waiting { failure: rutis_bridge::Failure::AuthRejected, retry_in, .. }
            if *retry_in == Retry::default().rejected
    )
}

/// Offer `work` on `at`: each run is counted, then hangs until the test
/// lets it finish (or `HANG_GUARD`), so a call to it is in flight.
fn hang(
    at: &Peer,
    runs: Arc<Mutex<u32>>,
) -> (
    rutis_bridge::Offered,
    std::sync::mpsc::Sender<()>,
    tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    let (started, running) = tokio::sync::mpsc::unbounded_channel();
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let offered = at
        .register(
            "work",
            Arc::new(move |_: &Connection, _: &str, _: &str, _: Value| {
                *runs.lock().unwrap() += 1;
                let _ = started.send(());
                let _ = gate.lock().unwrap().recv_timeout(scaled(HANG_GUARD));
                Ok(Value::Data(json!("done")))
            }),
        )
        .unwrap();
    (offered, open, running)
}

/// Call `work.run` on `on` (once `work` is offered there), in the
/// background.
async fn call_work(on: &Peer) -> tokio::task::JoinHandle<Result<Value, Error>> {
    let offers = on.offers();
    eventually(
        || offers.borrow().families.contains("work").then_some(()),
        "work offered",
    )
    .await;
    let session = on.connection().clone();
    tokio::spawn(async move {
        session
            .invoke_async("", "work.run", Value::Data(json!([])))
            .await
    })
}

async fn echo(on: &Peer, value: &str) -> serde_json::Value {
    on.connection()
        .invoke_async("", "echo.say", Value::Data(json!([value])))
        .await
        .unwrap()
        .json()
        .unwrap()
}

fn offer_echo(at: &Peer) -> rutis_bridge::Offered {
    at.register(
        "echo",
        Arc::new(|_: &Connection, _: &str, _: &str, args: Value| {
            Ok(Value::Data(args.json()?[0].clone()))
        }),
    )
    .unwrap()
}

/// What a raw dial with `token` is told.
async fn refusal(main: &Main, token: &str) -> String {
    let identity: Arc<dyn Identity> = Arc::new(
        StaticIdentity::new(id("someone")).present(id("main"), Credential::Bearer(token.into())),
    );
    let dial = Dial::address(main.address.clone())
        .peer(id("main"))
        .identity(identity)
        .protocol(protocol());
    let client = WebSocketPlugin::new(Config::new()).unwrap();
    let root = Ctx::root().unwrap();
    (&root.plugin(client.clone())).await.unwrap();
    match client.transport().unwrap().dial(&dial).await {
        Err(ConnectError::AuthRejected { reason }) => reason,
        other => panic!("not refused as an authentication failure: {other:?}"),
    }
}

/// guarantee: bridge/revocation-closes-only-its-connections,
/// bridge/revocation-refuses-reconnects
#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_peer_closes_only_its_session_and_its_calls_fail_once() {
    let main = main().await;
    let mac = dial(&main.address, "mac", "mac-token").await;
    let pi = dial(&main.address, "pi", "pi-token").await;
    ready(&main.mac, "main's link to mac").await;
    ready(&main.pi, "main's link to pi").await;
    let mac_at_main = eventually(|| peer(&main.root, "mac"), "mac at main").await;
    let main_at_mac = eventually(|| peer(&mac.root, "main"), "main at mac").await;
    let pi_at_main = eventually(|| peer(&main.root, "pi"), "pi at main").await;
    let main_at_pi = eventually(|| peer(&pi.root, "main"), "main at pi").await;
    let pi_generation = pi_at_main.generation();

    // A call in flight each way between main and mac.
    let runs_at_main = Arc::new(Mutex::new(0));
    let runs_at_mac = Arc::new(Mutex::new(0));
    let (_work_at_main, open_main, mut running_at_main) = hang(&mac_at_main, runs_at_main.clone());
    let (_work_at_mac, open_mac, mut running_at_mac) = hang(&main_at_mac, runs_at_mac.clone());
    let from_mac = call_work(&main_at_mac).await;
    let from_main = call_work(&mac_at_main).await;
    within("main to run mac's call", running_at_main.recv()).await;
    within("mac to run main's call", running_at_mac.recv()).await;
    // And something pi offers main.
    let _echo = offer_echo(&main_at_pi);
    let offers = pi_at_main.offers();
    eventually(
        || offers.borrow().families.contains("echo").then_some(()),
        "echo offered",
    )
    .await;

    main.peers.revoke(&id("mac"));

    for (call, side) in [(from_mac, "mac"), (from_main, "main")] {
        let failed = within("the call in flight to end", call).await.unwrap();
        assert!(
            matches!(&failed, Err(Error::Transport(_))),
            "{side}'s call in flight fails as a transport failure: {failed:?}"
        );
    }
    let _ = (open_main.send(()), open_mac.send(()));
    let at_main = state(&main.mac, "main's link to mac revoked", |state| {
        matches!(state, LinkState::Waiting { .. })
    })
    .await;
    assert!(
        matches!(&at_main, LinkState::Waiting { failure: rutis_bridge::Failure::AuthRejected, error, retry_in }
            if error == "mac: identity revoked" && retry_in.is_zero()),
        "{at_main:?}"
    );
    assert!(peer(&main.root, "mac").is_none(), "Peer#mac is withdrawn");

    // mac reconnects and is refused as a wrong token is: 403, waited out.
    let refused = state(&mac.state, "mac to be refused", auth_rejected).await;
    let LinkState::Waiting { error, .. } = refused else {
        unreachable!()
    };
    assert!(error.contains("403"), "{error}");
    assert_eq!(
        refusal(&main, "mac-token").await,
        refusal(&main, "a-wrong-token").await,
        "a revoked token is told what a wrong one is"
    );
    // Not run again on any side.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(*runs_at_main.lock().unwrap(), 1);
    assert_eq!(*runs_at_mac.lock().unwrap(), 1);

    // pi, on the same listener and identity, carries on: same session.
    assert!(matches!(*main.pi.borrow(), LinkState::Ready { .. }));
    let pi_now = peer(&main.root, "pi").unwrap();
    assert_eq!(pi_now.generation(), pi_generation);
    assert_eq!(echo(&pi_now, "still here").await, json!("still here"));
    // So does the access point there.
    let mut events = main.events.resubscribe();
    let _pixel = dial(&main.address, "pixel", "shared").await;
    assert!(matches!(
        within("pixel to connect", events.recv()).await.unwrap(),
        AccessEvent::Connected { .. }
    ));
}

/// guarantee: bridge/revocation-closes-only-its-connections
#[tokio::test(flavor = "multi_thread")]
async fn a_dialing_link_cuts_a_revoked_peer_and_dials_it_no_more() {
    let main = main().await;
    let revocations = Revocations::new();
    let mac = dial_with(&main.address, "mac", "mac-token", revocations.clone()).await;
    ready(&mac.state, "mac's link").await;
    ready(&main.mac, "main's link to mac").await;
    let main_at_mac = eventually(|| peer(&mac.root, "main"), "main at mac").await;

    revocations.revoke(&id("main"));
    within("mac's session to end", main_at_mac.connection().closed()).await;
    let at_mac = state(&mac.state, "mac to wait", auth_rejected).await;
    assert!(
        matches!(&at_mac, LinkState::Waiting { error, .. } if error == "main: identity revoked"),
        "{at_mac:?}"
    );
    assert!(peer(&mac.root, "main").is_none());
    // main sees its far end go, as when the network drops.
    state(&main.mac, "main's link to wait for mac", |state| {
        matches!(state, LinkState::Connecting)
    })
    .await;
}

/// guarantee: bridge/revocation-closes-only-its-connections,
/// bridge/revocation-refuses-reconnects
#[tokio::test(flavor = "multi_thread")]
async fn revoking_one_runtime_id_closes_only_that_connection() {
    let mut main = main().await;
    let pixel = dial(&main.address, "pixel", "shared").await;
    let tablet = dial(&main.address, "tablet", "shared").await;
    let mac = dial(&main.address, "mac", "mac-token").await;
    ready(&main.mac, "main's link to mac").await;
    for _ in 0..2 {
        assert!(matches!(
            within("a runtime to connect", main.events.recv())
                .await
                .unwrap(),
            AccessEvent::Connected { .. }
        ));
    }
    let pixel_at_main = main.kept.current("pixel").unwrap();
    let tablet_at_main = main.kept.current("tablet").unwrap();
    let main_at_pixel = eventually(|| peer(&pixel.root, "main"), "main at pixel").await;
    let main_at_tablet = eventually(|| peer(&tablet.root, "main"), "main at tablet").await;

    // A call in flight from main to pixel, and one from pixel to main.
    let runs_at_pixel = Arc::new(Mutex::new(0));
    let runs_at_main = Arc::new(Mutex::new(0));
    let (_at_pixel, open_pixel, mut running_at_pixel) = hang(&main_at_pixel, runs_at_pixel.clone());
    let (_at_main, open_main, mut running_at_main) =
        hang(pixel_at_main.peer(), runs_at_main.clone());
    let from_main = call_work(pixel_at_main.peer()).await;
    let from_pixel = call_work(&main_at_pixel).await;
    within("pixel to run main's call", running_at_pixel.recv()).await;
    within("main to run pixel's call", running_at_main.recv()).await;
    let _echo = offer_echo(&main_at_tablet);

    main.runtimes.revoke(&id("pixel"));

    for (call, side) in [(from_main, "main"), (from_pixel, "pixel")] {
        let failed = within("the call in flight to end", call).await.unwrap();
        assert!(
            matches!(&failed, Err(Error::Transport(_))),
            "{side}'s call in flight fails as a transport failure: {failed:?}"
        );
    }
    let _ = (open_pixel.send(()), open_main.send(()));
    let event = within("pixel to go", main.events.recv()).await.unwrap();
    assert!(
        matches!(&event, AccessEvent::Disconnected { id: who, reason, .. }
            if *who == id("pixel") && reason == "pixel: identity revoked"),
        "{event:?}"
    );
    // pixel reconnects: the shared token still lets it in, its id does
    // not (close code 4003 after the hello), and it waits that out.
    state(&pixel.state, "pixel to be refused", auth_rejected).await;
    let event = within("pixel's reconnect", main.events.recv())
        .await
        .unwrap();
    assert_eq!(
        event,
        AccessEvent::Rejected {
            reason: "not accepted here".into()
        }
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(*runs_at_pixel.lock().unwrap(), 1);
    assert_eq!(*runs_at_main.lock().unwrap(), 1);

    // tablet and the static link to mac carry on.
    let offers = tablet_at_main.peer().offers();
    eventually(
        || offers.borrow().families.contains("echo").then_some(()),
        "echo offered",
    )
    .await;
    assert_eq!(echo(tablet_at_main.peer(), "tablet").await, json!("tablet"));
    assert!(tablet_at_main.peer().connection().close_reason().is_none());
    assert!(matches!(*main.mac.borrow(), LinkState::Ready { .. }));
    assert!(matches!(*mac.state.borrow(), LinkState::Ready { .. }));
}

/// guarantee: bridge/revocation-closes-only-its-connections,
/// bridge/revocation-refuses-reconnects
#[tokio::test(flavor = "multi_thread")]
async fn revoking_the_shared_token_closes_every_runtime_and_nothing_else() {
    let mut main = main().await;
    let pixel = dial(&main.address, "pixel", "shared").await;
    let tablet = dial(&main.address, "tablet", "shared").await;
    let mac = dial(&main.address, "mac", "mac-token").await;
    ready(&main.mac, "main's link to mac").await;
    let mac_at_main = eventually(|| peer(&main.root, "mac"), "mac at main").await;
    for _ in 0..2 {
        within("a runtime to connect", main.events.recv())
            .await
            .unwrap();
    }

    main.runtimes.revoke_all();

    let mut gone = Vec::new();
    for _ in 0..2 {
        match within("a runtime to go", main.events.recv()).await.unwrap() {
            AccessEvent::Disconnected { id, reason, .. } => {
                assert_eq!(reason, format!("{id}: identity revoked"));
                gone.push(id);
            }
            other => panic!("{other:?}"),
        }
    }
    gone.sort();
    assert_eq!(gone, [id("pixel"), id("tablet")]);
    // Both reconnect and are refused at the upgrade, as a wrong token is.
    for runtime in [&pixel, &tablet] {
        let refused = state(&runtime.state, "a runtime to be refused", auth_rejected).await;
        let LinkState::Waiting { error, .. } = refused else {
            unreachable!()
        };
        assert!(error.contains("403"), "{error}");
    }
    assert_eq!(
        refusal(&main, "shared").await,
        refusal(&main, "a-wrong-token").await
    );
    // The static link on the same listener carries on: same session.
    assert!(matches!(*mac.state.borrow(), LinkState::Ready { .. }));
    assert_eq!(
        peer(&main.root, "mac").unwrap().generation(),
        mac_at_main.generation()
    );
}

/// A raw dial of `address` with `token`: the channel, before any hello.
async fn raw_channel(address: &str, token: &str) -> Channel {
    let identity: Arc<dyn Identity> = Arc::new(
        StaticIdentity::new(id("someone")).present(id("main"), Credential::Bearer(token.into())),
    );
    let dial = Dial::address(address.to_owned())
        .peer(id("main"))
        .identity(identity)
        .protocol(protocol());
    let client = WebSocketPlugin::new(Config::new()).unwrap();
    let root = Ctx::root().unwrap();
    (&root.plugin(client.clone())).await.unwrap();
    client.transport().unwrap().dial(&dial).await.unwrap()
}

/// Answers nothing: a far end that only greets.
struct Quiet;
impl Dispatch for Quiet {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        Ok(Value::Undefined)
    }
}

/// Greet on `channel` as `name`, expecting `expected`.
fn greet(channel: Channel, name: &str, expected: &str) -> Connection {
    Connection::open_with(
        channel,
        Arc::new(Quiet),
        Format::Endpoint(Endpoint::rust(id(name)).expect(id(expected))),
    )
    .unwrap()
}

/// Every state `states` takes from now on.
fn record(mut states: watch::Receiver<LinkState>) -> Arc<Mutex<Vec<LinkState>>> {
    let seen = Arc::new(Mutex::new(vec![states.borrow_and_update().clone()]));
    let log = seen.clone();
    tokio::spawn(async move {
        while states.changed().await.is_ok() {
            let state = states.borrow_and_update().clone();
            log.lock().unwrap().push(state);
        }
    });
    seen
}

/// Counts the provides of one service key, as the registry reports them
/// (every one, however soon it is withdrawn).
#[derive(Clone)]
struct Provides {
    key: TypeKey,
    count: Arc<Mutex<usize>>,
}

impl Provides {
    /// The listener that counts them.
    fn listener(&self) -> impl Listener<ServiceChanged> {
        let this = self.clone();
        move |changed: Arc<ServiceChanged>, _: Call<ServiceChanged>| {
            if changed.change == ServiceChange::Provided && changed.key == this.key {
                *this.count.lock().unwrap() += 1;
            }
            std::future::ready(Ok(None))
        }
    }
}

/// A far end revoked after main dialed it, before it greeted: main's
/// dialing link refuses it before providing the peer, and waits as after
/// an authentication failure. Found by the TLA+ model of remote leases
/// (`specs/remote-lease`, configuration `lease-before-fix`, #210): the
/// link used to provide the peer first and cut the session after, so
/// whatever depends on the peer could start on a revoked far end.
/// guarantee: bridge/revocation-refuses-reconnects
#[tokio::test(flavor = "multi_thread")]
async fn a_far_end_revoked_while_it_greets_never_becomes_a_peer() {
    // `far` listens; its registration hands each channel to the test.
    let far = Ctx::root().unwrap();
    let websocket = WebSocketPlugin::new(Config::new().listener(ListenerConfig::new(
        "public",
        "127.0.0.1:0".parse().unwrap(),
        id("far"),
    )))
    .unwrap();
    let handle = websocket.clone();
    (&far.plugin(websocket)).await.unwrap();
    let transport = handle.transport().unwrap();
    let address = format!("ws://{}/rutis", transport.local_addr("public").unwrap());
    let (sender, mut channels) = tokio::sync::mpsc::unbounded_channel::<Channel>();
    let _registered = transport
        .register(Registration {
            listener: "public".into(),
            peer: id("main"),
            identity: Arc::new(
                StaticIdentity::new(id("far")).accept_token("main-token", id("main")),
            ),
            protocol: protocol(),
            deliver: Box::new(move |channel| {
                let _ = sender.send(channel);
            }),
        })
        .unwrap();

    // `main` dials it with an identity that can revoke it; every provide
    // of the peer is counted (a state watch keeps only the latest state,
    // and would miss a `Ready` replaced at once).
    let main = Ctx::root().unwrap();
    let provides = Provides {
        key: peer_key(&id("far")).erase(),
        count: Arc::default(),
    };
    main.on(provides.listener()).unwrap();
    (&main.plugin(WebSocketPlugin::new(Config::new()).unwrap()))
        .await
        .unwrap();
    let revocations = Revocations::new();
    (&main.plugin(IdentityPlugin::new(
        "main",
        Revocable::new(
            StaticIdentity::new(id("main"))
                .present(id("far"), Credential::Bearer("main-token".into())),
            revocations.clone(),
        ),
    )))
        .await
        .unwrap();
    let link = LinkPlugin::new(LinkConfig::dial(id("far"), "websocket", "main", &address));
    let states = link.state();
    let seen = record(link.state());
    (&main.plugin(link)).await.unwrap();

    let channel = within("main's dial", channels.recv()).await.unwrap();
    revocations.revoke(&id("far"));
    let session = greet(channel, "far", "main");
    within("the session to end", session.closed()).await;
    let waiting = state(&states, "main to wait", auth_rejected).await;
    assert!(
        matches!(&waiting, LinkState::Waiting { error, .. } if error == "far: identity revoked"),
        "{waiting:?}"
    );
    assert!(peer(&main, "far").is_none());
    assert_eq!(
        *provides.count.lock().unwrap(),
        0,
        "the revoked far end was provided as a peer"
    );
    let seen = seen.lock().unwrap().clone();
    assert!(
        !seen
            .iter()
            .any(|state| matches!(state, LinkState::Ready { .. })),
        "the revoked far end never became ready: {seen:?}"
    );
    // It waits as after an authentication failure (30 s), not the first
    // step of the backoff (0.5 s +- 20 %): no dial within a second.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut dials = 0;
    while channels.try_recv().is_ok() {
        dials += 1;
    }
    assert_eq!(dials, 0, "main dialed the revoked far end again at once");
}

/// Records the ids it accepts; releasing the first connection waits for
/// `gate`, after telling `releasing`.
#[derive(Clone)]
struct Gated {
    accepted: Arc<Mutex<Vec<PeerId>>>,
    releasing: tokio::sync::mpsc::UnboundedSender<()>,
    gate: Arc<tokio::sync::Notify>,
}

impl Accept for Gated {
    fn accept(&self, connection: Accepted) -> BoxFuture<'_, Result<Release, String>> {
        Box::pin(async move {
            let first = {
                let mut accepted = self.accepted.lock().unwrap();
                accepted.push(connection.id().clone());
                accepted.len() == 1
            };
            let releasing = self.releasing.clone();
            let gate = self.gate.clone();
            let release: Release = Box::new(move || {
                Box::pin(async move {
                    if first {
                        let _ = releasing.send(());
                        gate.notified().await;
                    }
                })
            });
            Ok(release)
        })
    }
}

/// A runtime id revoked after its newer connection was admitted, while
/// that connection waits for the old one's release (a takeover): the
/// access point rejects it (4003) instead of accepting it, so nothing is
/// set up for a revoked id. Found by the TLA+ model of the access point
/// (`specs/remote-lease`, configuration `access-before-fix`, #210): the
/// access point used to accept it first and cut it after.
/// guarantee: bridge/revocation-refuses-reconnects
#[tokio::test(flavor = "multi_thread")]
async fn an_id_revoked_while_its_connection_takes_over_is_not_accepted() {
    let root = Ctx::root().unwrap();
    let websocket = WebSocketPlugin::new(Config::new().listener(ListenerConfig::new(
        "public",
        "127.0.0.1:0".parse().unwrap(),
        id("main"),
    )))
    .unwrap();
    let handle = websocket.clone();
    (&root.plugin(websocket)).await.unwrap();
    let transport = handle.transport().unwrap();
    let address = format!("ws://{}/rutis", transport.local_addr("public").unwrap());
    let runtimes = Revocations::new();
    (&root.plugin(AdmissionPlugin::new(
        "phone",
        Revocable::new(SharedToken::new(id("main"), "shared"), runtimes.clone()),
    )))
        .await
        .unwrap();
    let (releasing, mut released) = tokio::sync::mpsc::unbounded_channel();
    let gated = Gated {
        accepted: Arc::default(),
        releasing,
        gate: Arc::default(),
    };
    let access = AccessPointPlugin::new(
        AccessPointConfig::new(id("phone"), "websocket", "phone", "public")
            .handshake(scaled(HANG_GUARD)),
    )
    .accept(gated.clone());
    let mut events = access.events();
    (&root.plugin(access)).await.unwrap();

    let first = greet(raw_channel(&address, "shared").await, "pixel", "main");
    within("pixel's first session", first.ready())
        .await
        .unwrap();
    assert!(matches!(
        within("pixel to connect", events.recv()).await.unwrap(),
        AccessEvent::Connected { .. }
    ));
    // A newer connection of pixel takes over; the old one's release hangs.
    let second = greet(raw_channel(&address, "shared").await, "pixel", "main");
    within("the old connection's release", released.recv()).await;
    runtimes.revoke(&id("pixel"));
    gated.gate.notify_one();

    within("the newer connection to end", second.closed()).await;
    let event = loop {
        match within("the newer connection's outcome", events.recv())
            .await
            .unwrap()
        {
            AccessEvent::Replaced { .. } => continue,
            event => break event,
        }
    };
    assert_eq!(
        event,
        AccessEvent::Rejected {
            reason: "not accepted here".into()
        }
    );
    assert_eq!(*gated.accepted.lock().unwrap(), [id("pixel")]);
}
