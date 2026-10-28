//! The node conformance suite against a rutis node, in process.
#![cfg(feature = "testing")]

use std::time::Duration;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::channel::PeerId;
use rutis_bridge::{
    Accepting, Credential, IdentityPlugin, LinkConfig, LinkPlugin, Registrations, Retry,
    StaticIdentity,
};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rutis_node_meets_the_node_contract() {
    let quick = Retry {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
        ..Retry::default()
    };
    let transport = Registrations::new();
    let main = Ctx::root().unwrap();
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("node-token", id("node")),
    ));
    main.plugin(LinkPlugin::new(
        LinkConfig::listen(
            id("node"),
            "main",
            Accepting {
                registrations: transport.clone(),
                listener: "main-in".into(),
                local: id("main"),
            },
        )
        .require("node")
        .retry(quick.clone()),
    ));

    let node = Ctx::root().unwrap();
    node.plugin(IdentityPlugin::new(
        "node",
        StaticIdentity::new(id("node"))
            .present(id("main"), Credential::Bearer("node-token".into())),
    ));
    node.plugin(LinkPlugin::new(
        LinkConfig::dial(
            id("main"),
            "node",
            rutis_bridge::memory::dialer(transport.clone(), "main-in"),
        )
        .retry(quick),
    ));
    rutis_bridge::testing::fixture(&node, id("main"));

    rutis_bridge::testing::node(&main, id("node")).await;
}
