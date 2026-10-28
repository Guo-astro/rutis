//! Two Rust endpoints linked over a loopback WebSocket: one listens, one
//! dials; both get the other's peer and call its operations.
#![cfg(feature = "websocket")]

use std::sync::Arc;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::{Connection, Value};
use rutis_bridge::websocket::{self, Limits, ListenerConfig, Trust};
use rutis_bridge::{
    peer_key, Accepting, Credential, Dialer, IdentityPlugin, LinkConfig, LinkPlugin, Peer,
    Registrations, StaticIdentity,
};
use rutis_test_support::eventually;
use serde_json::json;

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// Dial `address` over WebSocket, trusting the system's roots.
fn dialer(address: &str) -> Dialer {
    let address = address.to_owned();
    Arc::new(move |credential: Option<Credential>| {
        let address = address.clone();
        Box::pin(async move {
            websocket::dial(
                &address,
                &rutis_bridge::protocol(),
                credential.as_ref(),
                &Trust::default(),
                &Limits::default(),
            )
            .await
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn two_rust_endpoints_link_over_websocket() {
    // main: listens for mac.
    let main = Ctx::root().unwrap();
    let registrations = Registrations::new();
    let listener = websocket::listen(
        &ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap()),
        &Limits::default(),
        websocket::admit_registered(registrations.clone(), "public"),
    )
    .unwrap();
    let address = format!("ws://{}/rutis", listener.local_addr());
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    ));
    main.plugin(LinkPlugin::new(LinkConfig::listen(
        id("mac"),
        "main",
        Accepting {
            registrations,
            listener: "public".into(),
            local: id("main"),
        },
    )));

    // mac: dials main.
    let mac = Ctx::root().unwrap();
    mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    ));
    mac.plugin(LinkPlugin::new(LinkConfig::dial(
        id("main"),
        "mac",
        dialer(&address),
    )));

    let at_main = eventually(|| main.get_at::<Peer>(&peer_key(&id("mac"))), "the link").await;
    let at_mac = eventually(|| mac.get_at::<Peer>(&peer_key(&id("main"))), "the link").await;
    let _offered = at_mac
        .register(
            "clock",
            Arc::new(|_: &Connection, _: &str, _: &str, _: Value| Ok(Value::Data(json!(42)))),
        )
        .unwrap();
    let now = at_main
        .connection()
        .invoke_async("", "clock.now", Value::Undefined)
        .await
        .unwrap();
    assert_eq!(now.json().unwrap(), json!(42));
    assert_eq!(at_main.connection().greeting().unwrap().endpoint, id("mac"));
}
