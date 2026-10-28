//! Links between two applications in one process, over the memory
//! transport: sessions, peers, operations and offers, reconnection,
//! takeover, failure categories and stopping.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rutis::host::{CtxHost, FiberView};
use rutis::Ctx;
use rutis_bridge::channel::{Channel, ConnectError, PeerId};
use rutis_bridge::session::Error;
use rutis_bridge::session::{Connection, Endpoint, Value};
use rutis_bridge::{
    peer_key, Accepting, Credential, Dialer, Failure, Identity, IdentityPlugin, LinkConfig,
    LinkPlugin, LinkState, Peer, Registrations, Retry, StaticIdentity,
};
use rutis_test_support::{eventually, scaled, HANG_GUARD};
use serde_json::json;
use tokio::sync::watch;

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

async fn state(
    states: &watch::Receiver<LinkState>,
    wanted: impl Fn(&LinkState) -> bool,
    what: &str,
) -> LinkState {
    eventually(
        || {
            let state = states.borrow().clone();
            wanted(&state).then_some(state)
        },
        what,
    )
    .await
}

fn peer(root: &Ctx, of: &str) -> Option<Arc<Peer>> {
    root.get_at::<Peer>(&peer_key(&id(of)))
}

/// One application: an identity and a link.
struct App {
    root: Ctx,
    link: FiberView,
    identity: FiberView,
    states: watch::Receiver<LinkState>,
}

async fn app(identity: StaticIdentity, link: LinkConfig) -> App {
    let root = Ctx::root().unwrap();
    let name = identity.local().to_string();
    let identity = root.plugin(IdentityPlugin::new(&name, identity));
    let plugin = LinkPlugin::new(link.retry(quick()));
    let states = plugin.state();
    let link = root.plugin(plugin);
    App {
        root,
        link,
        identity,
        states,
    }
}

/// `main` listens for `mac`; `mac` dials it with its token.
async fn linked() -> (Registrations, App, App) {
    let transport = Registrations::new();
    let main = app(
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
        LinkConfig::listen(
            id("mac"),
            "main",
            Accepting {
                registrations: transport.clone(),
                listener: "main-in".into(),
                local: id("main"),
            },
        ),
    )
    .await;
    let mac = app(
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
        LinkConfig::dial(
            id("main"),
            "mac",
            rutis_bridge::memory::dialer(transport.clone(), "main-in"),
        ),
    )
    .await;
    (transport, main, mac)
}

#[tokio::test(flavor = "multi_thread")]
async fn peers_serve_registered_families_and_announce_them() {
    let (_transport, main, mac) = linked().await;
    let at_main = eventually(|| peer(&main.root, "mac"), "Peer#mac at main").await;
    let at_mac = eventually(|| peer(&mac.root, "main"), "Peer#main at mac").await;
    assert_eq!(at_main.generation(), 1);
    assert_eq!(at_main.connection().greeting().unwrap().endpoint, id("mac"));

    let offered = at_main
        .register(
            "echo",
            Arc::new(|_: &Connection, _: &str, method: &str, args: Value| {
                Ok(Value::Data(
                    json!({ "method": method, "args": args.json()? }),
                ))
            }),
        )
        .unwrap();
    assert!(at_main
        .register(
            "echo",
            Arc::new(|_: &Connection, _: &str, _: &str, _| Ok(Value::Undefined))
        )
        .is_err());
    let reply = at_mac
        .connection()
        .invoke_async("", "echo.ping", Value::Data(json!([1])))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(reply, json!({ "method": "echo.ping", "args": [1] }));

    // Unregistered: an error, and the session goes on.
    let refused = at_mac
        .connection()
        .invoke_async("", "plugins.load", Value::Undefined)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("not offered"), "{refused}");

    let mut offers = at_mac.offers();
    eventually(
        || {
            offers
                .borrow_and_update()
                .families
                .contains("echo")
                .then_some(())
        },
        "echo offered",
    )
    .await;
    drop(offered);
    eventually(
        || offers.borrow_and_update().families.is_empty().then_some(()),
        "echo withdrawn",
    )
    .await;
    assert!(at_mac
        .connection()
        .invoke_async("", "echo.ping", Value::Undefined)
        .await
        .is_err());
}

/// A family withdrawn and registered again before the far end looks is a
/// new offer there: its epoch changed though the families did not.
#[tokio::test(flavor = "multi_thread")]
async fn a_family_registered_anew_is_a_new_offer() {
    let (_transport, main, mac) = linked().await;
    let at_main = eventually(|| peer(&main.root, "mac"), "main's peer").await;
    let at_mac = eventually(|| peer(&mac.root, "main"), "mac's peer").await;
    let handler = || Arc::new(|_: &Connection, _: &str, _: &str, _: Value| Ok(Value::Undefined));

    let offered = at_main.register("plugins", handler()).unwrap();
    let offers = at_mac.offers();
    let first = eventually(|| offers.borrow().epoch("plugins"), "plugins offered").await;
    // Withdrawn and registered again at once: one look may see neither change.
    drop(offered);
    let _again = at_main.register("plugins", handler()).unwrap();
    let second = eventually(
        || {
            offers
                .borrow()
                .epoch("plugins")
                .filter(|epoch| *epoch != first)
        },
        "the new offer",
    )
    .await;
    assert!(second > first);
    assert_eq!(
        offers.borrow().families,
        ["plugins".to_owned()].into_iter().collect()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dialing_link_reconnects_and_peers_get_a_new_generation() {
    let (_transport, main, mac) = linked().await;
    let first = eventually(|| peer(&mac.root, "main"), "the first session").await;
    first.connection().close(Error::Transport("cut".into()));
    drop(first);
    let second = eventually(
        || peer(&mac.root, "main").filter(|peer| peer.generation() == 2),
        "the second session at mac",
    )
    .await;
    eventually(
        || peer(&main.root, "mac").filter(|peer| peer.generation() == 2),
        "the second session at main",
    )
    .await;
    assert!(second.connection().greeting().is_some());
    state(
        &mac.states,
        |s| matches!(s, LinkState::Ready { generation: 2 }),
        "ready again",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_connection_takes_over_and_the_old_one_is_told() {
    let (transport, main, mac) = linked().await;
    let old = eventually(|| peer(&main.root, "mac"), "the first session").await;
    let old_at_mac = eventually(|| peer(&mac.root, "main"), "mac's session").await;
    // Another connection authenticating as mac, completing its handshake.
    let dial = rutis_bridge::memory::dialer(transport.clone(), "main-in");
    let channel = dial(Some(Credential::Bearer("mac-token".into())))
        .await
        .unwrap();
    struct Nothing;
    impl rutis_bridge::session::Dispatch for Nothing {
        fn invoke(
            &self,
            _: &Connection,
            _: &str,
            _: &str,
            _: Value,
        ) -> rutis_bridge::session::Reply {
            Ok(Value::Undefined)
        }
    }
    let newer = Connection::open(
        channel,
        Arc::new(Nothing),
        Endpoint::rust(id("mac")).expect(id("main")),
    )
    .unwrap();
    newer.ready().await.unwrap();
    // The old session ended, told it was replaced.
    old_at_mac.connection().closed().await;
    old.connection().closed().await;
    let current = eventually(
        || peer(&main.root, "mac").filter(|peer| peer.generation() > old.generation()),
        "the newer session",
    )
    .await;
    assert!(!current.connection().greeting().is_none());
    drop(newer);
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_are_retried_by_category() {
    let transport = Registrations::new();
    let _main = app(
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
        LinkConfig::listen(
            id("mac"),
            "main",
            Accepting {
                registrations: transport.clone(),
                listener: "main-in".into(),
                local: id("main"),
            },
        ),
    )
    .await;

    // app() installs plugins but does not wait for the listener to register.
    // Before registration a dial is Retryable, regardless of its credentials.
    state(
        &_main.states,
        |s| matches!(s, LinkState::Connecting),
        "the listener registration",
    )
    .await;

    // A wrong token: slow retries, reported as an authentication failure.
    let guessing = app(
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("guess".into())),
        LinkConfig::dial(
            id("main"),
            "mac",
            rutis_bridge::memory::dialer(transport.clone(), "main-in"),
        ),
    )
    .await;
    let waiting = state(
        &guessing.states,
        |s| matches!(s, LinkState::Waiting { .. }),
        "a refusal",
    )
    .await;
    assert!(matches!(
        waiting,
        LinkState::Waiting { failure: Failure::AuthRejected, retry_in, .. } if retry_in == quick().rejected
    ));

    // An endpoint speaking another protocol: no more attempts.
    let foreign = transport.clone();
    let _old = foreign
        .register(
            &id("old"),
            rutis_bridge::Registration {
                listener: "old-in".into(),
                peer: id("mac"),
                identity: Arc::new(
                    StaticIdentity::new(id("old")).accept_token("mac-token", id("mac")),
                ),
                protocol: "rutis.2".into(),
                deliver: Box::new(|_| {}),
            },
        )
        .unwrap();
    let dialing_old = app(
        StaticIdentity::new(id("mac")).present(id("old"), Credential::Bearer("mac-token".into())),
        LinkConfig::dial(
            id("old"),
            "mac",
            rutis_bridge::memory::dialer(transport.clone(), "old-in"),
        ),
    )
    .await;
    state(
        &dialing_old.states,
        |s| matches!(s, LinkState::Stopped { .. }),
        "a stop",
    )
    .await;

    // Nothing listening yet: retryable, with backoff.
    let early = app(
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
        LinkConfig::dial(
            id("main"),
            "mac",
            rutis_bridge::memory::dialer(transport.clone(), "nowhere"),
        ),
    )
    .await;
    state(
        &early.states,
        |s| {
            matches!(
                s,
                LinkState::Waiting {
                    failure: Failure::Retryable,
                    ..
                }
            )
        },
        "a retryable failure",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_far_end_greeting_as_someone_else_is_an_identity_failure() {
    let transport = Registrations::new();
    let main = app(
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
        LinkConfig::listen(
            id("mac"),
            "main",
            Accepting {
                registrations: transport.clone(),
                listener: "main-in".into(),
                local: id("main"),
            },
        ),
    )
    .await;
    // Holds mac's token, but greets as pi.
    let _impostor = app(
        StaticIdentity::new(id("pi")).present(id("main"), Credential::Bearer("mac-token".into())),
        LinkConfig::dial(
            id("main"),
            "pi",
            rutis_bridge::memory::dialer(transport.clone(), "main-in"),
        ),
    )
    .await;
    state(
        &main.states,
        |s| {
            matches!(
                s,
                LinkState::Waiting {
                    failure: Failure::AuthRejected,
                    ..
                }
            )
        },
        "the mismatch",
    )
    .await;
    assert!(peer(&main.root, "mac").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_link_withdraws_its_peer_and_revokes_its_registration() {
    let (transport, main, mac) = linked().await;
    eventually(|| peer(&main.root, "mac"), "the session").await;
    let at_mac = eventually(|| peer(&mac.root, "main"), "mac's session").await;

    main.link.dispose().await.unwrap();
    assert!(peer(&main.root, "mac").is_none());
    at_mac.connection().closed().await;
    drop(at_mac);
    // With main's registration revoked, mac is refused until main relinks:
    // its credentials are good, so it retries as for a listener not up yet.
    state(
        &mac.states,
        |s| {
            matches!(
                s,
                LinkState::Waiting {
                    failure: Failure::Retryable,
                    ..
                }
            )
        },
        "the refusal",
    )
    .await;
    let dial = rutis_bridge::memory::dialer(transport.clone(), "main-in");
    assert!(matches!(
        dial(Some(Credential::Bearer("mac-token".into()))).await,
        Err(rutis_bridge::channel::ConnectError::Retryable { .. })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_stops_with_its_identity() {
    let (_transport, main, mac) = linked().await;
    eventually(|| peer(&mac.root, "main"), "the session").await;
    mac.identity.dispose().await.unwrap();
    eventually(
        || peer(&mac.root, "main").is_none().then_some(()),
        "the peer withdrawn",
    )
    .await;
    eventually(
        || peer(&main.root, "mac").is_none().then_some(()),
        "main's peer withdrawn",
    )
    .await;
    assert_eq!(*mac.states.borrow(), LinkState::Idle);
}

/// A dial that holds its thread for a while, as starting a local
/// runtime's process does (fork and exec do not yield), then is refused:
/// `dialing` once it has begun, `dialed` once it is over.
#[derive(Default)]
struct Blocking {
    dialing: AtomicBool,
    dialed: AtomicBool,
}

impl Blocking {
    fn dialer(self: &Arc<Self>) -> Dialer {
        let blocking = self.clone();
        Arc::new(move |_| {
            let blocking = blocking.clone();
            Box::pin(async move {
                blocking.dialing.store(true, Ordering::SeqCst);
                std::thread::sleep(scaled(Duration::from_millis(500)));
                blocking.dialed.store(true, Ordering::SeqCst);
                Err::<Channel, _>(ConnectError::Retryable {
                    reason: "blocking: refused".into(),
                })
            })
        })
    }
}

/// A link stopped while its dial is in the middle of something that does
/// not yield: the stop returns once that is over. A local runtime's process
/// started by such a dial is then recorded, and its channel closed, when
/// the runtime has stopped; a reload that removed the runtime waits for it.
/// guarantee: host/reload-config
#[tokio::test(flavor = "multi_thread")]
async fn a_link_stopped_while_it_dials_stops_once_the_dial_is_over() {
    let root = Ctx::root().unwrap();
    let blocking = Arc::new(Blocking::default());
    root.plugin(IdentityPlugin::new("mac", StaticIdentity::new(id("mac"))));
    let link = root.plugin(LinkPlugin::new(
        LinkConfig::dial(id("main"), "mac", blocking.dialer()).retry(quick()),
    ));
    eventually(
        || blocking.dialing.load(Ordering::SeqCst).then_some(()),
        "the dial",
    )
    .await;

    link.dispose().await.unwrap();
    assert!(
        blocking.dialed.load(Ordering::SeqCst),
        "the link stopped while its dial went on"
    );
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_far_end_without_the_required_contract_stops_the_link() {
    let transport = Registrations::new();
    let main = app(
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
        LinkConfig::listen(
            id("mac"),
            "main",
            Accepting {
                registrations: transport.clone(),
                listener: "main-in".into(),
                local: id("main"),
            },
        ),
    )
    .await;
    // mac requires a runtime; main is a node.
    let mac = app(
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
        LinkConfig::dial(
            id("main"),
            "mac",
            rutis_bridge::memory::dialer(transport.clone(), "main-in"),
        )
        .require("runtime"),
    )
    .await;
    let stopped = state(
        &mac.states,
        |s| matches!(s, LinkState::Stopped { .. }),
        "the stop",
    )
    .await;
    assert!(
        matches!(stopped, LinkState::Stopped { failure: Failure::Incompatible, error } if error.contains("runtime"))
    );
    assert!(peer(&mac.root, "main").is_none());

    // Requiring what it declares works.
    let fine = app(
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
        LinkConfig::dial(
            id("main"),
            "mac",
            rutis_bridge::memory::dialer(transport.clone(), "main-in"),
        )
        .require("node"),
    )
    .await;
    eventually(|| peer(&fine.root, "main"), "the node peer").await;
    drop(main);
}
