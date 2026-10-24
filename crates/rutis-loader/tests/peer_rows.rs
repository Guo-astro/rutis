//! Rows hosted on another node (`peer:<id>/<plugin>`): two applications in
//! one process, linked over the memory transport; `mac` hosts installed
//! plugins for `main`, whose loader manages them as rows.
#![cfg(feature = "peer")]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::runtimes::id;
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView, Key, PluginFactory, PluginId,
};
use rutis::{Ctx, Error};
use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
use rutis_bridge::{
    peer_key, Credential, Described, HostPlugin, IdentityPlugin, LinkConfig, LinkPlugin, Peer,
    Retry, StaticCatalog, StaticIdentity,
};
use rutis_loader::peer::{PeerResolver, PeerRowsPlugin};
use rutis_loader::report::EntryStatus;
use rutis_loader::{Chain, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch};
use rutis_test_support::eventually;
use serde_json::{json, Value};

/// Installed on mac: provides `Greeting` with its config's text (keyed by
/// its config's `as`, if any, so two rows can run side by side); counts how
/// often it started.
struct Greeter(Arc<AtomicUsize>);
impl PluginFactory<Value> for Greeter {
    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Greeting(
            config["text"].as_str().unwrap_or("hello").to_owned(),
            config["as"].as_str().map(str::to_owned),
            self.0.clone(),
        )))
    }
}
struct Greeting(String, Option<String>, Arc<AtomicUsize>);
impl DynPlugin for Greeting {
    fn name(&self) -> &str {
        "greeter"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.2.fetch_add(1, Ordering::SeqCst);
            let service = GreetingService(self.0.clone());
            match &self.1 {
                Some(key) => {
                    ctx.provide_at(
                        Key::<GreetingService>::dynamic(key.clone()),
                        Arc::new(service),
                    )?;
                }
                None => {
                    ctx.provide(service)?;
                }
            }
            Ok(Effect::Done)
        })
    }
}
struct GreetingService(String);

struct Nodes {
    main: Ctx,
    mac: Ctx,
    loader: Loader,
    starts: Arc<AtomicUsize>,
    host: FiberView,
    mac_link: FiberView,
    /// main's fibers' state changes, from before its rows exist.
    transitions: common::Transitions,
    /// Each row's fiber before mac linked in.
    placeholders: std::collections::HashMap<String, PluginId>,
}

fn greeting(mac: &Ctx) -> Option<String> {
    mac.get::<GreetingService>()
        .map(|service| service.0.clone())
}

fn greeting_as(mac: &Ctx, key: &str) -> Option<String> {
    mac.get_at(&Key::<GreetingService>::dynamic(key.to_owned()))
        .map(|service| service.0.clone())
}

/// main's loader holds `rows` before anything is linked; then mac links in
/// and opens its host.
async fn nodes(rows: Value) -> Nodes {
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));
    let quick = Retry {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
        jitter: 0.0,
        ..Retry::default()
    };

    let main = Ctx::root().unwrap();
    let transitions = common::Transitions::on(&main);
    let resolver = Arc::new(PeerResolver::new());
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(resolver.clone()),
        LoaderOptions::default(),
    );
    let loader = plugin.handle();
    (&main.plugin(plugin)).await.unwrap();
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    // Before anything is linked: the rows' fibers on their placeholders.
    let placeholders = loader
        .entries()
        .into_iter()
        .filter_map(|entry| Some((entry.id, entry.plugin?)))
        .collect();
    (&main.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    ));
    main.plugin(LinkPlugin::new(
        LinkConfig::listen(id("mac"), "memory", "main", "main-in").retry(quick.clone()),
    ));
    main.plugin(PeerRowsPlugin::new(id("mac"), resolver));

    let mac = Ctx::root().unwrap();
    (&mac.plugin(MemoryPlugin::with_transport(transport)))
        .await
        .unwrap();
    mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    ));
    let mac_link = mac.plugin(LinkPlugin::new(
        LinkConfig::dial(id("main"), "memory", "mac", "main-in").retry(quick),
    ));
    let starts = Arc::new(AtomicUsize::new(0));
    let catalog = StaticCatalog::new().with(
        "greeter",
        Described {
            schema: Some(
                json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
            ),
            version: Some("1.0.0".into()),
            integrity: None,
        },
        Greeter(starts.clone()),
    );
    let catalog = Arc::new(catalog);
    let host = mac.plugin(HostPlugin::new(id("main"), catalog));
    Nodes {
        main,
        mac,
        loader,
        starts,
        host,
        mac_link,
        transitions,
        placeholders,
    }
}

/// guarantee: loader/peer-rows
#[tokio::test(flavor = "multi_thread")]
async fn rows_wait_for_the_host_then_run_there_once() {
    let nodes = nodes(json!([
        { "id": "g", "name": "peer:mac/greeter", "config": { "text": "hi" } },
        { "id": "x", "name": "peer:mac/not-installed", "config": {} }
    ]))
    .await;
    let greeting_now = eventually(|| greeting(&nodes.mac), "the hosted row").await;
    assert_eq!(greeting_now, "hi");
    // Started once, on its resolved declarations: the row's fibers loaded
    // once. A start on the placeholder resolution, restarted once resolved,
    // would have loaded before the changes `loads` waits for (one fiber's
    // changes arrive in order), and the greeter would have run twice.
    let fibers = [
        nodes.placeholders["g"],
        nodes.loader.get("g").unwrap().plugin.unwrap(),
    ];
    assert_eq!(
        nodes.transitions.loads(&fibers).await,
        1,
        "{:?}",
        fibers.map(|fiber| nodes.transitions.of(fiber))
    );
    assert_eq!(
        nodes.starts.load(Ordering::SeqCst),
        1,
        "started once, on its resolved declarations"
    );
    // Nor again after it ran: a marker, another row on the same host. The
    // loader's operation lock serializes this reconcile with the resolving
    // again that would restart `g` (`PeerRowsPlugin` refreshes the rows of
    // the host's offer, then provides `PeerRows`), so once the marker runs,
    // such a restart has happened.
    nodes
        .loader
        .reconcile(
            vec![Layer::new(
                "rows",
                serde_json::from_value(json!([{ "insert": [
                    { "id": "g", "name": "peer:mac/greeter", "config": { "text": "hi" } },
                    { "id": "x", "name": "peer:mac/not-installed", "config": {} },
                    { "id": "h", "name": "peer:mac/greeter", "config": { "text": "marker", "as": "h" } }
                ] }]))
                .unwrap(),
            )],
            None,
        )
        .await
        .unwrap();
    eventually(|| greeting_as(&nodes.mac, "h"), "the marker row").await;
    assert_eq!(nodes.starts.load(Ordering::SeqCst), 2, "g once, h once");
    assert_eq!(nodes.loader.get("g").unwrap().plugin.unwrap(), fibers[1]);
    assert_eq!(
        nodes.transitions.loads(&fibers).await,
        1,
        "{:?}",
        fibers.map(|fiber| nodes.transitions.of(fiber))
    );
    let schema = nodes.loader.schema_of("peer:mac/greeter").await.unwrap();
    assert_eq!(schema.unwrap()["properties"]["text"]["type"], "string");

    let missing = eventually(
        || match nodes.loader.get("x")?.status {
            EntryStatus::Unresolved(error) => Some(error),
            _ => None,
        },
        "the missing plugin unresolved",
    )
    .await;
    assert!(
        matches!(missing, LoaderError::NotFound { .. }),
        "{missing:?}"
    );

    // A new config restarts it over there.
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
        { "id": "g", "name": "peer:mac/greeter", "config": { "text": "hey" } },
        { "id": "x", "name": "peer:mac/not-installed", "config": {} }
    ] }]))
    .unwrap();
    nodes
        .loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    eventually(
        || (greeting(&nodes.mac)? == "hey").then_some(()),
        "the new config",
    )
    .await;
}

/// guarantee: loader/peer-rows
#[tokio::test(flavor = "multi_thread")]
async fn rows_stop_with_the_host_and_the_link_and_the_peer_stays() {
    let nodes = nodes(json!([{ "id": "g", "name": "peer:mac/greeter", "config": {} }])).await;
    eventually(|| greeting(&nodes.mac), "the hosted row").await;

    // The host closes: the row stops; the link and the peer stay.
    nodes.host.dispose().await.unwrap();
    eventually(
        || greeting(&nodes.mac).is_none().then_some(()),
        "the row stopped",
    )
    .await;
    assert!(nodes.main.get_at(&peer_key(&id("mac"))).is_some());

    // The host again: the row runs again.
    let catalog = StaticCatalog::new().with(
        "greeter",
        Described::default(),
        Greeter(nodes.starts.clone()),
    );
    nodes
        .mac
        .plugin(HostPlugin::new(id("main"), Arc::new(catalog)));
    eventually(|| greeting(&nodes.mac), "the row back").await;

    // The link ends: the row stops, and mac unloads what it hosted.
    nodes.mac_link.dispose().await.unwrap();
    eventually(
        || greeting(&nodes.mac).is_none().then_some(()),
        "the hosted plugin unloaded",
    )
    .await;
    eventually(
        || match nodes.loader.get("g")?.status {
            EntryStatus::Running(snapshot) if snapshot.state != FiberState::Active => Some(()),
            _ => None,
        },
        "the row waiting",
    )
    .await;
}

/// Both nodes configured through their loaders: a `rutis-bridge/peer` row
/// each, mac hosting what its own loader resolves.
/// guarantee: loader/peer-rows
#[tokio::test(flavor = "multi_thread")]
async fn loader_rows_compose_links_and_change_features_in_place() {
    use rutis_bridge::session::{host_key, HostDispatch};
    use rutis_bridge::session::{Reply, Value as RpcValue};
    use rutis_loader::peer::register_peer_node;
    use rutis_loader::Builtins;

    struct Fixed(u64);
    impl HostDispatch for Fixed {
        fn invoke(&self, _: &str, _: RpcValue) -> Reply {
            Ok(json!(self.0).into())
        }
        fn methods(&self) -> Option<Value> {
            Some(json!({ "get": "sync" }))
        }
    }

    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));

    // main: exports clock, runs peer:mac/… rows on mac.
    let main = Ctx::root().unwrap();
    main.provide_at(host_key("clock"), Arc::new(Fixed(1)))
        .unwrap();
    main.provide_at(host_key("calendar"), Arc::new(Fixed(2)))
        .unwrap();
    (&main.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    ));
    let rows = Arc::new(PeerResolver::new());
    let mut builtins = Builtins::new();
    register_peer_node(&mut builtins, rows.clone());
    let plugin = LoaderPlugin::new(
        Chain::new().with(builtins).with_shared(rows),
        LoaderOptions::default(),
    );
    let main_loader = plugin.handle();
    (&main.plugin(plugin)).await.unwrap();
    let main_rows = |export: Value| -> Vec<Layer> {
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
            { "id": "link", "name": "rutis-bridge/peer", "config": {
                "peer": "mac", "transport": "memory", "identity": "main", "listen": "main-in",
                "export": export, "rows": true
            } },
            { "id": "g", "name": "peer:mac/greeter", "config": { "text": "composed" } }
        ] }]))
        .unwrap();
        vec![Layer::new("rows", patches)]
    };
    main_loader
        .reconcile(main_rows(json!(["clock"])), None)
        .await
        .unwrap();

    // mac: imports clock and calendar, hosts what its loader resolves.
    let mac = Ctx::root().unwrap();
    (&mac.plugin(MemoryPlugin::with_transport(transport)))
        .await
        .unwrap();
    mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    ));
    let mut installed = Builtins::new();
    installed.register_raw("greeter", Greeter(Arc::new(AtomicUsize::new(0))), None);
    register_peer_node(&mut installed, Arc::new(PeerResolver::new()));
    let plugin = LoaderPlugin::new(Chain::new().with(installed), LoaderOptions::default());
    let mac_loader = plugin.handle();
    (&mac.plugin(plugin)).await.unwrap();
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
        { "id": "link", "name": "rutis-bridge/peer", "config": {
            "peer": "main", "transport": "memory", "identity": "mac", "dial": "main-in",
            "import": ["clock", "calendar"], "host": true
        } }
    ] }]))
    .unwrap();
    mac_loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();

    eventually(
        || greeting(&mac).filter(|text| text == "composed"),
        "the hosted row",
    )
    .await;
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "clock imported",
    )
    .await;
    assert!(mac.get_at(&host_key("calendar")).is_none());
    let session = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();

    // Export calendar too: a volatile change, so the session stays.
    main_loader
        .reconcile(main_rows(json!(["clock", "calendar"])), None)
        .await
        .unwrap();
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("calendar")),
        "calendar imported",
    )
    .await;
    let still = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    assert!(Arc::ptr_eq(&session, &still), "the link kept its session");
    assert_eq!(greeting(&mac).as_deref(), Some("composed"));
}

/// Hosted rows share the link's one session: loading one creates no
/// session, unloading one closes none, and the others keep running.
/// guarantee: loader/peer-rows
#[tokio::test(flavor = "multi_thread")]
async fn hosted_rows_share_the_session_of_their_link() {
    let nodes = nodes(json!([
        { "id": "g1", "name": "peer:mac/greeter", "config": { "text": "one", "as": "g1" } },
        { "id": "g2", "name": "peer:mac/greeter", "config": { "text": "two", "as": "g2" } }
    ]))
    .await;
    eventually(
        || (nodes.starts.load(Ordering::SeqCst) == 2).then_some(()),
        "both hosted rows",
    )
    .await;
    let session = nodes.main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    assert_eq!(session.generation(), 1, "one session carries both rows");

    // Unload one: the session and the other row stay.
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
        { "id": "g2", "name": "peer:mac/greeter", "config": { "text": "two", "as": "g2" } }
    ] }]))
    .unwrap();
    nodes
        .loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    eventually(
        || greeting_as(&nodes.mac, "g1").is_none().then_some(()),
        "the row unloaded",
    )
    .await;
    assert_eq!(
        greeting_as(&nodes.mac, "g2").as_deref(),
        Some("two"),
        "the other row still running"
    );
    let still = nodes.main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    assert!(
        Arc::ptr_eq(&session, &still),
        "unloading a row closes no session"
    );
    assert!(still.connection().greeting().is_some());
}

/// Two nodes host rows for each other, start together, and reconnect,
/// without either waiting on the other's rows: a host never waits for rows.
/// guarantee: loader/peer-rows
#[tokio::test(flavor = "multi_thread")]
async fn nodes_hosting_for_each_other_start_and_reconnect_without_deadlock() {
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("a-in", id("a"));
    let quick = Retry {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
        jitter: 0.0,
        ..Retry::default()
    };
    async fn node(
        transport: &Arc<MemoryTransport>,
        me: &str,
        other: &str,
        link: LinkConfig,
        identity: StaticIdentity,
    ) -> (Ctx, Arc<AtomicUsize>, FiberView) {
        let root = Ctx::root().unwrap();
        let rows = Arc::new(PeerResolver::new());
        let plugin = LoaderPlugin::new(
            Chain::new().with_shared(rows.clone()),
            LoaderOptions::default(),
        );
        let loader = plugin.handle();
        (&root.plugin(plugin)).await.unwrap();
        // Each node's rows run on the other, from the start.
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
            { "id": "r", "name": format!("peer:{other}/greeter"), "config": { "text": format!("for {me}") } }
        ] }]))
        .unwrap();
        loader
            .reconcile(vec![Layer::new("rows", patches)], None)
            .await
            .unwrap();
        (&root.plugin(MemoryPlugin::with_transport(transport.clone())))
            .await
            .unwrap();
        root.plugin(IdentityPlugin::new(me, identity));
        let link = root.plugin(LinkPlugin::new(link));
        root.plugin(PeerRowsPlugin::new(id(other), rows));
        let starts = Arc::new(AtomicUsize::new(0));
        let catalog =
            StaticCatalog::new().with("greeter", Described::default(), Greeter(starts.clone()));
        root.plugin(HostPlugin::new(id(other), Arc::new(catalog)));
        std::mem::forget(loader);
        (root, starts, link)
    }
    let (a, a_hosts, _a_link) = node(
        &transport,
        "a",
        "b",
        LinkConfig::listen(id("b"), "memory", "a", "a-in").retry(quick.clone()),
        StaticIdentity::new(id("a")).accept_token("b-token", id("b")),
    )
    .await;
    let (b, b_hosts, _b_link) = node(
        &transport,
        "b",
        "a",
        LinkConfig::dial(id("a"), "memory", "b", "a-in").retry(quick),
        StaticIdentity::new(id("b")).present(id("a"), Credential::Bearer("b-token".into())),
    )
    .await;
    eventually(|| (greeting(&a)? == "for b").then_some(()), "b's row on a").await;
    eventually(|| (greeting(&b)? == "for a").then_some(()), "a's row on b").await;

    // A reconnect: both rows stop and start again on the new session.
    let session = a.get_at::<Peer>(&peer_key(&id("b"))).unwrap();
    session
        .connection()
        .close(rutis_bridge::session::Error::Transport("cut".into()));
    drop(session);
    eventually(
        || (a_hosts.load(Ordering::SeqCst) >= 2).then_some(()),
        "b's row again on a",
    )
    .await;
    eventually(
        || (b_hosts.load(Ordering::SeqCst) >= 2).then_some(()),
        "a's row again on b",
    )
    .await;
    eventually(|| greeting(&a), "b's row running on a").await;
    eventually(|| greeting(&b), "a's row running on b").await;
}
