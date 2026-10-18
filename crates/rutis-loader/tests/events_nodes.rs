//! Events between nodes (#464, docs/design-events-protocol-2026-10-18.md §9):
//! two and three applications in one process, linked by
//! `rutis-bridge/peer` rows over the memory transport. The cases of the
//! expectation file with a `node` participant run in
//! `events_conformance.rs`; these are the rules of the link itself.
#![cfg(feature = "peer")]

mod common;

use std::sync::Arc;

use common::runtimes::id;
use rutis::Ctx;
use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
use rutis_bridge::{Credential, IdentityPlugin, StaticIdentity};
use rutis_loader::{
    register_peer_node, Builtins, Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch,
    PeerResolver,
};
use serde_json::{json, Value};

fn layer(rows: Value) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

/// A node `name` on `transport`, with a loader holding `rows` (its
/// `rutis-bridge/peer` rows). `accepts`: the peers that dial it, with their
/// tokens; `presents`: the peers it dials.
async fn node(
    transport: &Arc<MemoryTransport>,
    name: &str,
    accepts: &[&str],
    presents: &[&str],
    rows: Value,
) -> (Ctx, Loader) {
    let ctx = Ctx::root().unwrap();
    (&ctx.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    let mut identity = StaticIdentity::new(id(name));
    for peer in accepts {
        identity = identity.accept_token(format!("{peer}-token").as_str(), id(peer));
    }
    for peer in presents {
        identity = identity.present(id(peer), Credential::Bearer(format!("{name}-token")));
    }
    ctx.plugin(IdentityPlugin::new(name, identity));
    let mut builtins = Builtins::new();
    register_peer_node(&mut builtins, Arc::new(PeerResolver::new()));
    let plugin = LoaderPlugin::new(Chain::new().with(builtins), LoaderOptions::default());
    let loader = plugin.handle();
    (&ctx.plugin(plugin)).await.unwrap();
    loader.reconcile(layer(rows), None).await.unwrap();
    (ctx, loader)
}

/// What the loader says about the row `id`: its status and any refusal.
fn said(loader: &Loader, id: &str) -> String {
    let entry = loader.get(id).expect("the row");
    format!("{:?} {:?}", entry.status, entry.rejected)
}

/// §2.4: the old `events: { out, in }` of a link is refused, saying what
/// replaces it.
/// guarantee: events/node-config
#[tokio::test(flavor = "multi_thread")]
async fn the_old_events_config_of_a_link_is_refused() {
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));
    let (main, loader) = node(
        &transport,
        "main",
        &["mac"],
        &[],
        json!([{ "id": "link", "name": "rutis-bridge/peer", "config": {
            "peer": "mac", "transport": "memory", "identity": "main", "listen": "main-in",
            "events": { "out": ["t/tick"], "in": ["t/tock"] }
        } }]),
    )
    .await;
    let expected = "the peer config field events was removed in rutis 0.13: list events: { out } in export and events: { in } in import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)";
    rutis_test_support::eventually(
        || said(&loader, "link").contains(expected).then_some(()),
        "the refusal",
    )
    .await;
    main.shutdown().await.unwrap();
}

/// §9: event names and `*` prefixes in `export` / `import` are accepted
/// (services stay exact names).
/// guarantee: events/link-export-import
#[tokio::test(flavor = "multi_thread")]
async fn event_prefixes_are_accepted_in_export_and_import() {
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));
    let (main, loader) = node(
        &transport,
        "main",
        &["mac"],
        &[],
        json!([{ "id": "link", "name": "rutis-bridge/peer", "config": {
            "peer": "mac", "transport": "memory", "identity": "main", "listen": "main-in",
            "export": ["t/*", "rutis/*"], "import": ["t/*", "*"]
        } }]),
    )
    .await;
    let entry = loader.get("link").unwrap();
    assert!(entry.rejected.is_none(), "{}", said(&loader, "link"));
    assert!(
        !format!("{:?}", entry.status).contains("Failed"),
        "{}",
        said(&loader, "link")
    );
    main.shutdown().await.unwrap();
}

mod dispatch {
    //! The link carries the five modes; an imported entry is never passed
    //! on (no multi-hop); a node announces only what the other imports.
    use super::*;
    use rutis::events::Call;
    use rutis::EventKey;
    use rutis_bridge::events::{ArgValue, Args};
    use rutis_bridge::session::Value as RpcValue;

    fn link(peer: &str, me: &str, how: Value, share: Value) -> Value {
        let mut config = json!({
            "peer": peer, "transport": "memory", "identity": me,
            "export": share["export"], "import": share["import"],
        });
        for (key, value) in how.as_object().unwrap() {
            config[key] = value.clone();
        }
        json!({ "id": format!("link-{peer}"), "name": "rutis-bridge/peer", "config": config })
    }

    struct Answer(&'static str);

    impl rutis::Listener<Args> for Answer {
        fn call<'a>(
            &'a self,
            _: &'a Ctx,
            _: &'a Args,
            _: Call<'a, ArgValue>,
        ) -> rutis::BoxFuture<'a, Result<Option<ArgValue>, rutis::CordisError>> {
            let answer = ArgValue(RpcValue::Data(json!(self.0)));
            Box::pin(std::future::ready(Ok(Some(answer))))
        }
    }

    /// Listen to `name` on `ctx`, answering `answer`.
    fn answer(ctx: &Ctx, name: &str, answer: &'static str) {
        ctx.events()
            .on(
                ctx,
                &EventKey::<Args>::dynamic(name.to_owned()),
                Answer(answer),
            )
            .unwrap();
    }

    /// `serial` of `name` from `ctx`, once the link has announced a
    /// listener (at most 10 s).
    async fn serial(ctx: &Ctx, name: &str) -> Option<Value> {
        let key = EventKey::<Args>::dynamic(name.to_owned());
        for _ in 0..1000 {
            let result = ctx
                .events()
                .serial(ctx, &key, Arc::new(Args(Vec::new())))
                .await
                .unwrap();
            if let Some(value) = result {
                return Some(value.0.json().unwrap());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        None
    }

    /// a — b — c: b imports `t/*` from both; c's listener of `t/x` reaches
    /// b, never a (§9: imported entries are not announced again).
    /// guarantee: events/no-multi-hop
    #[tokio::test(flavor = "multi_thread")]
    async fn an_imported_listener_is_not_passed_on() {
        let transport = Arc::new(MemoryTransport::default());
        transport.endpoint("b-in", id("b"));
        let all = json!({ "export": ["t/*"], "import": ["t/*"] });
        let (b, _) = node(
            &transport,
            "b",
            &["a", "c"],
            &[],
            json!([
                link("a", "b", json!({ "listen": "b-in" }), all.clone()),
                link("c", "b", json!({ "listen": "b-in" }), all.clone()),
            ]),
        )
        .await;
        let (a, _) = node(
            &transport,
            "a",
            &[],
            &["b"],
            json!([link("b", "a", json!({ "dial": "b-in" }), all.clone())]),
        )
        .await;
        let (c, _) = node(
            &transport,
            "c",
            &[],
            &["b"],
            json!([link("b", "c", json!({ "dial": "b-in" }), all)]),
        )
        .await;
        answer(&c, "t/x", "from c");
        assert_eq!(serial(&b, "t/x").await, Some(json!("from c")), "b hears c");
        let key = EventKey::<Args>::dynamic("t/x".to_owned());
        let from_a = a
            .events()
            .serial(&a, &key, Arc::new(Args(Vec::new())))
            .await
            .unwrap();
        assert!(from_a.is_none(), "a must not reach c through b");
        for ctx in [a, b, c] {
            ctx.shutdown().await.unwrap();
        }
    }

    /// `import: ["t/s/*"]` on the listening side narrows what it announces:
    /// a listener of `t/other` is not reached.
    /// guarantee: events/link-export-import
    #[tokio::test(flavor = "multi_thread")]
    async fn import_narrows_what_is_announced() {
        let transport = Arc::new(MemoryTransport::default());
        transport.endpoint("main-in", id("main"));
        let (main, _) = node(
            &transport,
            "main",
            &["mac"],
            &[],
            json!([link(
                "mac",
                "main",
                json!({ "listen": "main-in" }),
                json!({ "export": ["t/*"], "import": [] })
            ),]),
        )
        .await;
        let (mac, _) = node(
            &transport,
            "mac",
            &[],
            &["main"],
            json!([link(
                "main",
                "mac",
                json!({ "dial": "main-in" }),
                json!({ "export": [], "import": ["t/s/*"] })
            ),]),
        )
        .await;
        answer(&mac, "t/s/hello", "inside");
        answer(&mac, "t/other", "outside");
        assert_eq!(serial(&main, "t/s/hello").await, Some(json!("inside")));
        let key = EventKey::<Args>::dynamic("t/other".to_owned());
        assert!(main
            .events()
            .serial(&main, &key, Arc::new(Args(Vec::new())))
            .await
            .unwrap()
            .is_none());
        main.shutdown().await.unwrap();
        mac.shutdown().await.unwrap();
    }
}
