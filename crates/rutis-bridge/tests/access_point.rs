//! Access points over the memory transport: runtimes dialing out to rutis
//! with a shared token, told apart by the id they greet with; admission,
//! concurrent connections, takeover, disconnection, and a static link on
//! the same listener.

use std::sync::{Arc, Mutex};

use rutis::{BoxFuture, Ctx, FiberView};
use rutis_bridge::channel::{Channel, PeerId};
use rutis_bridge::session::{Connection, Dispatch, Endpoint, Format, Reply, Value};
use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
use rutis_bridge::{
    peer_key, protocol, Accept, Accepted, AccessEvent, AccessPointConfig, AccessPointPlugin,
    AdmissionPlugin, ConnectError, Credential, Dial, Identity, IdentityPlugin, LinkConfig,
    LinkPlugin, LinkState, Peer, Release, SharedToken, StaticIdentity, Transport,
};
use rutis_test_support::{eventually, within, HANG_GUARD};
use serde_json::json;
use tokio::sync::broadcast;

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// What the access point's [`Accept`] saw, in order.
#[derive(Clone, Default)]
struct Log {
    lines: Arc<Mutex<Vec<String>>>,
    accepted: Arc<Mutex<Vec<Accepted>>>,
}

impl Log {
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn accepted(&self, generation: u64) -> Option<Accepted> {
        let accepted = self.accepted.lock().unwrap();
        accepted
            .iter()
            .find(|accepted| accepted.peer().generation() == generation)
            .cloned()
    }
}

impl Accept for Log {
    fn accept(&self, connection: Accepted) -> BoxFuture<'_, Result<Release, String>> {
        Box::pin(async move {
            let line = format!("{} {}", connection.id(), connection.peer().generation());
            self.lines.lock().unwrap().push(format!("accept {line}"));
            self.accepted.lock().unwrap().push(connection);
            let lines = self.lines.clone();
            let release: Release = Box::new(move || {
                Box::pin(async move {
                    lines.lock().unwrap().push(format!("release {line}"));
                })
            });
            Ok(release)
        })
    }
}

/// `main`, whose access point `phone` admits runtimes presenting `shared`
/// on its endpoint `main-in`.
struct Main {
    transport: Arc<MemoryTransport>,
    root: Ctx,
    access: FiberView,
    log: Log,
    events: broadcast::Receiver<AccessEvent>,
}

async fn main_with(transport: Arc<MemoryTransport>) -> Main {
    transport.endpoint("main-in", id("main"));
    let root = Ctx::root().unwrap();
    (&root.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    (&root.plugin(AdmissionPlugin::new(
        "phone",
        SharedToken::new(id("main"), "shared"),
    )))
        .await
        .unwrap();
    let log = Log::default();
    let plugin = AccessPointPlugin::new(
        AccessPointConfig::new(id("phone"), "memory", "phone", "main-in").handshake(HANG_GUARD),
    )
    .accept(log.clone());
    let events = plugin.events();
    let access = root.plugin(plugin);
    (&access).await.unwrap();
    Main {
        transport,
        root,
        access,
        log,
        events,
    }
}

async fn main() -> Main {
    main_with(Arc::new(MemoryTransport::default())).await
}

async fn next(events: &mut broadcast::Receiver<AccessEvent>) -> AccessEvent {
    within("an access point event", events.recv())
        .await
        .unwrap()
}

/// Answers every call with the id it greeted as.
struct Whoami(String);
impl Dispatch for Whoami {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        Ok(Value::Data(json!(self.0)))
    }
}

async fn dial_channel(transport: &MemoryTransport, token: &str) -> Result<Channel, ConnectError> {
    // The dialer's own id plays no part in the memory transport's routing.
    let identity: Arc<dyn Identity> = Arc::new(
        StaticIdentity::new(id("runtime")).present(id("main"), Credential::Bearer(token.into())),
    );
    transport
        .dial(
            &Dial::address("main-in")
                .peer(id("main"))
                .identity(identity)
                .protocol(protocol()),
        )
        .await
}

/// A runtime dialing out as `name` with `token`, its session ready.
async fn runtime(transport: &MemoryTransport, name: &str, token: &str) -> Connection {
    let channel = dial_channel(transport, token).await.unwrap();
    let session = Connection::open_with(
        channel,
        Arc::new(Whoami(name.into())),
        Format::Endpoint(Endpoint::rust(id(name)).expect(id("main"))),
    )
    .unwrap();
    within("the runtime's session", session.ready())
        .await
        .unwrap();
    session
}

async fn whoami(accepted: &Accepted) -> serde_json::Value {
    accepted
        .runtime_session()
        .connection()
        .invoke_async("", "whoami", Value::Undefined)
        .await
        .unwrap()
        .json()
        .unwrap()
}

/// Greet with a raw hello, then read until the channel ends: whether it
/// was told anything besides `main`'s own hello.
fn greet_raw(mut channel: Channel, hello: serde_json::Value) -> Vec<String> {
    channel.sender.send(hello.to_string().as_bytes()).unwrap();
    let mut read = Vec::new();
    while let Ok(Some(message)) = channel.receiver.recv() {
        read.push(String::from_utf8(message).unwrap());
    }
    read
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_or_missing_token_is_refused_before_any_session() {
    let main = main().await;
    let wrong = dial_channel(&main.transport, "guess").await;
    assert!(
        matches!(wrong, Err(ConnectError::AuthRejected { .. })),
        "{wrong:?}"
    );
    let identity: Arc<dyn Identity> = Arc::new(StaticIdentity::new(id("runtime")));
    let none = main
        .transport
        .dial(
            &Dial::address("main-in")
                .peer(id("main"))
                .identity(identity)
                .protocol(protocol()),
        )
        .await;
    assert!(
        matches!(none, Err(ConnectError::AuthRejected { .. })),
        "{none:?}"
    );
    assert!(main.log.lines().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_or_missing_id_fails_the_handshake_as_incompatible() {
    let mut main = main().await;
    for hello in [
        json!({ "op": "hello", "version": 3, "endpoint": "Pixel 7" }),
        json!({ "op": "hello", "version": 3 }),
    ] {
        let channel = dial_channel(&main.transport, "shared").await.unwrap();
        let read = tokio::task::spawn_blocking(move || greet_raw(channel, hello))
            .await
            .unwrap();
        // Only main's hello, then the end: the session never went further.
        // Retrying cannot fix the id, so it is not an admission's refusal.
        assert_eq!(read.len(), 1, "{read:?}");
        assert!(read[0].contains(r#""op":"hello""#), "{read:?}");
        let event = next(&mut main.events).await;
        assert!(
            matches!(&event, AccessEvent::Failed { reason } if reason.contains("endpoint")),
            "{event:?}"
        );
    }
    assert!(main.log.lines().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn runtimes_with_different_ids_each_get_their_own_session() {
    let mut main = main().await;
    let pixel = runtime(&main.transport, "pixel", "shared").await;
    let tablet = runtime(&main.transport, "tablet", "shared").await;
    let mut connected = Vec::new();
    for _ in 0..2 {
        match next(&mut main.events).await {
            AccessEvent::Connected { id, generation } => connected.push((id, generation)),
            other => panic!("{other:?}"),
        }
    }
    connected.sort();
    assert_eq!(connected[0].0, id("pixel"));
    assert_eq!(connected[1].0, id("tablet"));
    for (who, generation) in connected {
        let accepted = main.log.accepted(generation).unwrap();
        assert_eq!(accepted.id(), &who);
        assert_eq!(whoami(&accepted).await, json!(who.as_str()));
    }
    // Nothing keyed by a connection id is provided in the application.
    assert!(main.root.get_as::<Peer>(peer_key(&id("pixel"))).is_none());
    drop((pixel, tablet));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_id_takes_over_once_the_old_connection_is_released() {
    let mut main = main().await;
    let first = runtime(&main.transport, "pixel", "shared").await;
    let AccessEvent::Connected {
        generation: old, ..
    } = next(&mut main.events).await
    else {
        panic!("not connected");
    };
    let second = runtime(&main.transport, "pixel", "shared").await;
    assert_eq!(
        next(&mut main.events).await,
        AccessEvent::Replaced {
            id: id("pixel"),
            generation: old
        }
    );
    let AccessEvent::Connected {
        id: again,
        generation: new,
    } = next(&mut main.events).await
    else {
        panic!("not connected again");
    };
    assert_eq!(again, id("pixel"));
    within("the old session's end", first.closed()).await;
    assert_eq!(
        main.log.lines(),
        [
            format!("accept pixel {old}"),
            format!("release pixel {old}"),
            format!("accept pixel {new}"),
        ]
    );
    let accepted = main.log.accepted(new).unwrap();
    assert_eq!(whoami(&accepted).await, json!("pixel"));
    assert!(accepted.peer().connection().close_reason().is_none());
    drop(second);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disconnection_is_released_and_observed() {
    let mut main = main().await;
    let pixel = runtime(&main.transport, "pixel", "shared").await;
    let AccessEvent::Connected { generation, .. } = next(&mut main.events).await else {
        panic!("not connected");
    };
    pixel.close(rutis_bridge::session::Error::Transport("stopping".into()));
    let event = next(&mut main.events).await;
    assert!(
        matches!(&event, AccessEvent::Disconnected { id: who, generation: g, .. } if *who == id("pixel") && *g == generation),
        "{event:?}"
    );
    assert_eq!(
        main.log.lines(),
        [
            format!("accept pixel {generation}"),
            format!("release pixel {generation}"),
        ]
    );
    // It may connect again: a new connection.
    let _again = runtime(&main.transport, "pixel", "shared").await;
    assert!(matches!(
        next(&mut main.events).await,
        AccessEvent::Connected { generation: g, .. } if g > generation
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_the_access_point_releases_its_connections() {
    let mut main = main().await;
    let pixel = runtime(&main.transport, "pixel", "shared").await;
    next(&mut main.events).await;
    main.access.dispose().await.unwrap();
    within("the runtime's session end", pixel.closed()).await;
    assert_eq!(main.log.lines().last().unwrap(), "release pixel 1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_static_link_and_an_access_point_share_a_listener() {
    let transport = Arc::new(MemoryTransport::default());
    let mut main = main_with(transport.clone()).await;
    (&main.root.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    )))
        .await
        .unwrap();
    (&main.root.plugin(LinkPlugin::new(LinkConfig::listen(
        id("mac"),
        "memory",
        "main",
        "main-in",
    ))))
        .await
        .unwrap();

    let mac = Ctx::root().unwrap();
    (&mac.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    (&mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    )))
        .await
        .unwrap();
    (&mac.plugin(LinkPlugin::new(LinkConfig::dial(
        id("main"),
        "memory",
        "mac",
        "main-in",
    ))))
        .await
        .unwrap();
    eventually(
        || main.root.get_as::<Peer>(peer_key(&id("mac"))),
        "the static link's peer",
    )
    .await;

    let _pixel = runtime(&transport, "pixel", "shared").await;
    assert!(matches!(
        next(&mut main.events).await,
        AccessEvent::Connected { id: who, .. } if who == id("pixel")
    ));
    // The access point's name is taken on this listener: a link for a peer
    // of that name cannot register there.
    let clash = LinkPlugin::new(LinkConfig::listen(id("phone"), "memory", "main", "main-in"));
    let states = clash.state();
    (&main.root.plugin(clash)).await.unwrap();
    let stopped = eventually(
        || match &*states.borrow() {
            LinkState::Stopped { error, .. } => Some(error.clone()),
            _ => None,
        },
        "the clashing link to stop",
    )
    .await;
    assert!(stopped.contains("already registered"), "{stopped}");
}
