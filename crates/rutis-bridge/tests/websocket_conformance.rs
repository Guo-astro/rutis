//! The node conformance suite over a loopback WebSocket: against a Cordis
//! node (node/rutis-runtime/test/fixtures/cordis-conformance.mjs), and
//! against a rutis node (the in-process one of node_conformance.rs, here on
//! a real transport). Risk C10: the matrix is in docs/ci.md.
#![cfg(all(feature = "websocket", feature = "testing"))]

use std::path::Path;

use std::sync::Arc;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::channel::PeerId;
use rutis_bridge::websocket::{self, Limits, Listener, ListenerConfig, Trust};
use rutis_bridge::{
    Accepting, Credential, Dialer, IdentityPlugin, LinkConfig, LinkPlugin, Registrations,
    StaticIdentity,
};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// A loopback listener `public` for endpoint `main`, where it accepts.
fn listening() -> (Listener, Accepting) {
    let registrations = Registrations::new();
    let listener = websocket::listen(
        &ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap()),
        &Limits::default(),
        websocket::admit_registered(registrations.clone(), "public"),
    )
    .unwrap();
    let accepting = Accepting {
        registrations,
        listener: "public".into(),
        local: id("main"),
    };
    (listener, accepting)
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
async fn a_cordis_node_meets_the_node_contract() {
    let node = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
    // conformance-greeter, installed where the Cordis node resolves plugins.
    let anchor = tempfile::tempdir().unwrap();
    std::fs::write(anchor.path().join("package.json"), "{}").unwrap();
    let installed = anchor.path().join("node_modules/conformance-greeter");
    std::fs::create_dir_all(&installed).unwrap();
    for file in ["package.json", "index.mjs"] {
        std::fs::copy(
            node.join("test/fixtures/conformance-greeter").join(file),
            installed.join(file),
        )
        .unwrap();
    }

    let main = Ctx::root().unwrap();
    let (listener, accepting) = listening();
    let address = format!("ws://{}/rutis", listener.local_addr());
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("cordis-token", id("cordis")),
    ));
    main.plugin(LinkPlugin::new(
        LinkConfig::listen(id("cordis"), "main", accepting).require("node"),
    ));

    let mut child = tokio::process::Command::new("node")
        .args(["--import", "tsx"])
        .arg(node.join("test/fixtures/cordis-conformance.mjs"))
        .arg(&address)
        .arg(anchor.path().join("package.json"))
        .current_dir(&node)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    rutis_bridge::testing::node(&main, id("cordis")).await;
    child.start_kill().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rutis_node_meets_the_node_contract() {
    // main: listens for the node, and requires its `node` contract.
    let main = Ctx::root().unwrap();
    let (listener, accepting) = listening();
    let address = format!("ws://{}/rutis", listener.local_addr());
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("node-token", id("node")),
    ));
    main.plugin(LinkPlugin::new(
        LinkConfig::listen(id("node"), "main", accepting).require("node"),
    ));

    // node: a rutis node in its own root, dialing main.
    let node = Ctx::root().unwrap();
    node.plugin(IdentityPlugin::new(
        "node",
        StaticIdentity::new(id("node"))
            .present(id("main"), Credential::Bearer("node-token".into())),
    ));
    node.plugin(LinkPlugin::new(LinkConfig::dial(
        id("main"),
        "node",
        dialer(&address),
    )));
    rutis_bridge::testing::fixture(&node, id("main"));

    rutis_bridge::testing::node(&main, id("node")).await;
}
