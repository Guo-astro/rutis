//! Node feature plugins between two applications in one process: services
//! exported and imported, plugins hosted for a peer, events shared.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{Call, Listener};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberView, PluginFactory};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error};
use rutis_bridge::channel::PeerId;
use rutis_bridge::events::{ArgValue, Args, LinkEventsPlugin};
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{settle, Reply, Value};
use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
use rutis_bridge::{
    peer_key, Credential, Described, ExportPlugin, HostPlugin, IdentityPlugin, ImportPlugin,
    LinkConfig, LinkPlugin, Peer, Retry, StaticCatalog, StaticIdentity,
};
use rutis_test_support::{eventually, within};
use serde_json::{json, Value as Json};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

fn quick() -> Retry {
    Retry {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
        rejected: Duration::from_millis(100),
        // No jitter: the same delays every run (#231).
        jitter: 0.0,
        ..Retry::default()
    }
}

/// `main` listens for `mac`, `mac` dials it; both get their peer.
async fn linked() -> (Ctx, Ctx, FiberView) {
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));
    let main = Ctx::root().unwrap();
    (&main.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    ));
    main.plugin(LinkPlugin::new(
        LinkConfig::listen(id("mac"), "memory", "main", "main-in").retry(quick()),
    ));
    let mac = Ctx::root().unwrap();
    (&mac.plugin(MemoryPlugin::with_transport(transport)))
        .await
        .unwrap();
    mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    ));
    let mac_link = mac.plugin(LinkPlugin::new(
        LinkConfig::dial(id("main"), "memory", "mac", "main-in").retry(quick()),
    ));
    eventually(|| main.get_at::<Peer>(&peer_key(&id("mac"))), "main's peer").await;
    eventually(|| mac.get_at::<Peer>(&peer_key(&id("main"))), "mac's peer").await;
    (main, mac, mac_link)
}

/// A clock whose readings start at `base`.
struct Clock(AtomicU64);
impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: Value) -> Reply {
        let now = self.0.fetch_add(1, Ordering::SeqCst);
        match method {
            "now" => Ok(json!(now).into()),
            "later" => Ok(Value::future(async move { Ok(json!(now + 1000).into()) })),
            _ => Err(rutis_bridge::session::Error::Value(method.into())),
        }
    }
    fn methods(&self) -> Option<Json> {
        Some(json!({ "now": "sync", "later": "async" }))
    }
}

fn clock(base: u64) -> Arc<dyn HostDispatch> {
    Arc::new(Clock(AtomicU64::new(base)))
}

#[tokio::test(flavor = "multi_thread")]
async fn exported_services_are_imported_replaced_and_withdrawn() {
    let (main, mac, _) = linked().await;
    let provided = main.provide_at(host_key("clock"), clock(0)).unwrap();
    main.plugin(ExportPlugin::new(id("mac"), ["clock"]));
    mac.plugin(ImportPlugin::new(id("main"), ["clock"]));

    let imported = eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "the import",
    )
    .await;
    assert_eq!(
        imported.methods(),
        Some(json!({ "now": "sync", "later": "async" }))
    );
    let now = tokio::task::spawn_blocking({
        let imported = imported.clone();
        move || imported.invoke("now", Value::List(vec![]))
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(now.json().unwrap(), json!(0));
    let later = settle(imported.invoke("later", Value::List(vec![])).unwrap())
        .await
        .unwrap();
    assert_eq!(later.json().unwrap(), json!(1001));
    drop(imported);

    // Replaced over there: replaced here.
    provided.dispose().await.unwrap();
    let provided = main.provide_at(host_key("clock"), clock(500)).unwrap();
    let replaced = eventually(
        || {
            let service = mac.get_at::<dyn HostDispatch>(&host_key("clock"))?;
            let now = settle(service.invoke("later", Value::List(vec![])).ok()?);
            let now = futures_now(now)?;
            (now >= 1500).then_some(now)
        },
        "the replacement",
    )
    .await;
    assert!(replaced >= 1500);

    // Withdrawn over there: withdrawn here.
    provided.dispose().await.unwrap();
    eventually(
        || {
            mac.get_at::<dyn HostDispatch>(&host_key("clock"))
                .is_none()
                .then_some(())
        },
        "the withdrawal",
    )
    .await;
}

/// Poll a settle future once on a fresh runtime thread (tests only).
fn futures_now(future: impl std::future::Future<Output = Reply> + Send + 'static) -> Option<u64> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
            .ok()?
            .json()
            .ok()?
            .as_u64()
    })
    .join()
    .unwrap()
}

/// `services.announce` of a `clock` whose `now` is 7, as an exporter sends
/// it: the importer's reply.
async fn announce(
    session: &rutis_bridge::session::Connection,
    name: &str,
    version: u64,
) -> Result<Value, rutis_bridge::session::Error> {
    let record = Value::Record(
        [
            ("name".to_owned(), Value::Data(json!(name))),
            (
                "service".to_owned(),
                Value::Record(
                    [("now".to_owned(), Value::callback(|_| Ok(json!(7).into())))].into(),
                ),
            ),
            ("shape".to_owned(), Value::Data(json!({ "now": "sync" }))),
            ("version".to_owned(), Value::Data(json!(version))),
        ]
        .into(),
    );
    let reply = session
        .invoke_async("", "services.announce", Value::List(vec![record]))
        .await?;
    settle(reply).await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_never_replaces_a_service_provided_here() {
    let (main, mac, _) = linked().await;
    main.provide_at(host_key("clock"), clock(0)).unwrap();
    mac.provide_at(host_key("clock"), clock(7)).unwrap();
    mac.plugin(ImportPlugin::new(id("main"), ["clock"]));
    let at_main = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    let mut offers = at_main.offers();
    eventually(
        || offers.borrow_and_update().epoch("services"),
        "the importer",
    )
    .await;
    // Announced as main's exporter does. The importer decides before it
    // replies (services.rs, Importer::announce), so the reply is the
    // decision: refused, with the reason.
    let refused = announce(at_main.connection(), "clock", 1).await;
    let error = refused.expect_err("an import over a local service is refused");
    assert!(
        error.to_string().contains("already provided here"),
        "{error}"
    );
    let local = mac.get_at::<dyn HostDispatch>(&host_key("clock")).unwrap();
    let now = tokio::task::spawn_blocking(move || local.invoke("now", Value::List(vec![])))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(now.json().unwrap(), json!(7), "the local clock stays");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_export_waits_for_the_far_end_to_offer_services() {
    let (main, mac, _) = linked().await;
    main.provide_at(host_key("clock"), clock(0)).unwrap();
    main.plugin(ExportPlugin::new(id("mac"), ["clock"]));
    // clock: weak-negative — the exporter waiting for an offer leaves
    // nothing to observe across the session's threads; a slow machine can
    // only miss a premature export here, never fail. The late import below
    // is the positive half.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(mac.get_at::<dyn HostDispatch>(&host_key("clock")).is_none());
    // Importing later still gets it: the exporter announces on the offer.
    mac.plugin(ImportPlugin::new(id("main"), ["clock"]));
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "the late import",
    )
    .await;
}

/// A Rust plugin installed on mac, providing `greeting` with its config.
struct Greeter;
impl PluginFactory<Json> for Greeter {
    fn build(&self, config: &Json) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Greeting(
            config["text"].as_str().unwrap_or("hello").to_owned(),
        )))
    }
}
struct Greeting(String);
impl DynPlugin for Greeting {
    fn name(&self) -> &str {
        "greeter"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.provide(GreetingService(self.0.clone()))?;
            Ok(Effect::Done)
        })
    }
}
struct GreetingService(String);

#[tokio::test(flavor = "multi_thread")]
async fn a_host_loads_installed_plugins_for_its_peer() {
    let (main, mac, mac_link) = linked().await;
    let catalog = StaticCatalog::new().with(
        "greeter",
        Described {
            schema: Some(json!({ "type": "object" })),
            version: Some("1.0.0".into()),
            integrity: None,
        },
        Greeter,
    );
    mac.plugin(HostPlugin::new(id("main"), Arc::new(catalog)));
    let at_main = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    let mut offers = at_main.offers();
    eventually(
        || {
            offers
                .borrow_and_update()
                .families
                .contains("plugins")
                .then_some(())
        },
        "the host offered",
    )
    .await;
    let session = at_main.connection();

    let described = settle(
        session
            .invoke_async("", "plugins.describe", json!(["greeter"]).into())
            .await
            .unwrap(),
    )
    .await
    .unwrap()
    .json()
    .unwrap();
    assert_eq!(described["version"], "1.0.0");
    let missing = settle(
        session
            .invoke_async("", "plugins.describe", json!(["unknown"]).into())
            .await
            .unwrap(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(missing, rutis_bridge::session::Error::Remote { ref name, .. } if name == "NotFound")
    );
    assert!(session
        .invoke_async(
            "",
            "plugins.load",
            json!(["k", "/etc/plugin.so", {}]).into()
        )
        .await
        .is_err());

    settle(
        session
            .invoke_async(
                "",
                "plugins.load",
                json!(["g1", "greeter", { "text": "hi" }]).into(),
            )
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(mac.get::<GreetingService>().unwrap().0, "hi");
    settle(
        session
            .invoke_async(
                "",
                "plugins.update",
                json!(["g1", { "text": "hey" }]).into(),
            )
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    eventually(
        || (mac.get::<GreetingService>()?.0 == "hey").then_some(()),
        "the update",
    )
    .await;
    settle(
        session
            .invoke_async("", "plugins.unload", json!(["g1"]).into())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert!(mac.get::<GreetingService>().is_none());

    // What a host loaded goes with the link.
    settle(
        session
            .invoke_async("", "plugins.load", json!(["g2", "greeter", {}]).into())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert!(mac.get::<GreetingService>().is_some());
    mac_link.dispose().await.unwrap();
    eventually(
        || mac.get::<GreetingService>().is_none().then_some(()),
        "the hosted plugin unloaded",
    )
    .await;
}

/// Records each event's arguments after a while, and answers `answer`.
fn record(heard: Arc<Mutex<Vec<Value>>>, answer: Option<&'static str>) -> impl Listener<Args> {
    move |event: Arc<Args>, _: Call<Args>| {
        let heard = heard.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            heard.lock().unwrap().push(Value::List(event.0.clone()));
            Ok(answer.map(|answer| ArgValue(Value::Data(json!(answer)))))
        }
    }
}

/// `serial` of `name` from `ctx`, until its result is not none.
async fn serial_until_heard(ctx: &Ctx, name: &str, args: Vec<Value>) -> Value {
    for _ in 0..500 {
        let result = ctx.named(name).serial(Args(args.clone())).await.unwrap();
        if let Some(ArgValue(value)) = result {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("nobody answered {name}");
}

/// guarantee: events/node-links
///
/// A node's listener is an entry of the other node's bus when one exports
/// and the other imports its name (events protocol §9): the emitter waits
/// for it in `parallel`, gets its result in `serial`, its arguments cross
/// as tagged values; a name the link does not share stays on its node;
/// when the link goes, so do the entries.
#[tokio::test(flavor = "multi_thread")]
async fn node_listeners_are_entries_of_the_other_bus() {
    let (main, mac, mac_link) = linked().await;
    let heard = Arc::new(Mutex::new(Vec::new()));
    mac.listen::<Args>()
        .name("t/tick")
        .on(record(heard.clone(), Some("pong")))
        .unwrap();
    mac.listen::<Args>()
        .name("n/kept")
        .on(record(heard.clone(), Some("leaked")))
        .unwrap();
    // mac hears main's `t/*`; main's emissions of every name reach mac.
    (&mac.plugin(LinkEventsPlugin::new(
        id("main"),
        Vec::<String>::new(),
        ["t/*"],
    )))
        .await
        .unwrap();
    (&main.plugin(LinkEventsPlugin::new(
        id("mac"),
        ["*"],
        Vec::<String>::new(),
    )))
        .await
        .unwrap();

    let bytes = Value::Bytes(b"payload".to_vec());
    let answer = serial_until_heard(&main, "t/tick", vec![json!(1).into(), bytes.clone()]).await;
    assert_eq!(answer, Value::Data(json!("pong")));
    assert_eq!(
        heard.lock().unwrap().last().unwrap(),
        &Value::List(vec![Value::Data(json!(1)), bytes]),
        "a tagged value crossed as itself"
    );

    heard.lock().unwrap().clear();
    main.named("t/tick").parallel(Args(vec![])).await.unwrap();
    assert_eq!(heard.lock().unwrap().len(), 1, "parallel waited for mac");

    let kept = main.named("n/kept").serial(Args(vec![])).await.unwrap();
    assert!(kept.is_none(), "mac does not import n/kept");

    // The link goes: so does mac's entry on main's bus.
    mac_link.dispose().await.unwrap();
    let mut gone = false;
    for _ in 0..500 {
        let result = main.named("t/tick").serial(Args(vec![])).await;
        if matches!(result, Ok(None)) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "mac's listener left main's bus with the link");
}

/// A child plugin of `parent`, marked as the scope `tenant`; its context.
async fn in_tenant(parent: &Ctx) -> (FiberView, Ctx) {
    struct Child(std::sync::mpsc::Sender<Ctx>);
    impl DynPlugin for Child {
        fn name(&self) -> &str {
            "in-tenant"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                ctx.mark_scope("tenant")?;
                self.0.send(ctx.clone()).unwrap();
                Ok(Effect::Done)
            })
        }
    }
    let (tx, rx) = std::sync::mpsc::channel::<Ctx>();
    let view = parent.plugin(Child(tx));
    (&view).await.unwrap();
    (view, rx.recv().unwrap())
}

/// A link inside an instance shares names with entries outside it: it
/// announces the root's entries and what it accepts is heard from the root,
/// because delivery is by name, not by scope.
///
/// guarantee: events/flat
#[tokio::test(flavor = "multi_thread")]
async fn a_link_inside_an_instance_shares_names_outside_it() {
    let (main, mac, _mac_link) = linked().await;
    let (_view, tenant) = in_tenant(&main).await;
    // Both directions carry `t/*`; each accepts whatever the other sends.
    (&mac.plugin(LinkEventsPlugin::new(id("main"), ["*"], ["t/*"])))
        .await
        .unwrap();
    (&tenant.plugin(LinkEventsPlugin::new(id("mac"), ["*"], ["t/*"])))
        .await
        .unwrap();

    // The root's listener is announced by the tenant's link to mac...
    main.listen::<Args>()
        .name("t/root")
        .on(record(Arc::new(Mutex::new(Vec::new())), Some("pong")))
        .unwrap();
    assert_eq!(
        serial_until_heard(&mac, "t/root", vec![]).await,
        Value::Data(json!("pong"))
    );

    // ...and mac's listener, accepted at the tenant's scope, is heard by the
    // root's emitter.
    mac.listen::<Args>()
        .name("t/mac")
        .on(record(Arc::new(Mutex::new(Vec::new())), Some("pong")))
        .unwrap();
    assert_eq!(
        serial_until_heard(&main, "t/mac", vec![]).await,
        Value::Data(json!("pong"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_composition_changes_one_feature_and_keeps_its_session() {
    use rutis_bridge::{Features, PeerPlugin};
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));
    let main = Ctx::root().unwrap();
    (&main.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    ));
    main.provide_at(host_key("clock"), clock(0)).unwrap();
    main.provide_at(host_key("calendar"), clock(100)).unwrap();
    let composed = PeerPlugin::new(
        LinkConfig::listen(id("mac"), "memory", "main", "main-in").retry(quick()),
        Features {
            export: vec!["clock".into()],
            ..Features::default()
        },
    )
    .unwrap();
    let handle = composed.handle();
    main.plugin(composed);

    let mac = Ctx::root().unwrap();
    (&mac.plugin(MemoryPlugin::with_transport(transport)))
        .await
        .unwrap();
    mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    ));
    mac.plugin(
        PeerPlugin::new(
            LinkConfig::dial(id("main"), "memory", "mac", "main-in").retry(quick()),
            Features {
                import: vec!["clock".into(), "calendar".into()],
                ..Features::default()
            },
        )
        .unwrap(),
    );
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "clock imported",
    )
    .await;
    let session = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    assert!(mac
        .get_at::<dyn HostDispatch>(&host_key("calendar"))
        .is_none());

    // Export calendar as well: it arrives, and the session is the same.
    handle
        .set(Features {
            export: vec!["clock".into(), "calendar".into()],
            ..Features::default()
        })
        .await
        .unwrap();
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("calendar")),
        "calendar imported",
    )
    .await;
    let still = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    assert_eq!(still.generation(), session.generation());
    assert!(Arc::ptr_eq(&still, &session), "the link kept its session");
}

/// Provides `host_key("svc")`; counts its starts.
struct Provider(Arc<AtomicU64>);
impl PluginFactory<Json> for Provider {
    fn build(&self, _: &Json) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Providing(self.0.clone())))
    }
}
struct Providing(Arc<AtomicU64>);
impl DynPlugin for Providing {
    fn name(&self) -> &str {
        "provider"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            ctx.provide_at(host_key("svc"), clock(0))?;
            Ok(Effect::Done)
        })
    }
}

/// A hosted plugin runs in its row's scope: what the row isolates stays out
/// of the host's scope, and what the row injects is waited for.
#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_plugin_keeps_its_rows_isolate_and_inject() {
    let (main, mac, _link) = linked().await;
    let starts = Arc::new(AtomicU64::new(0));
    let catalog =
        StaticCatalog::new().with("provider", Described::default(), Provider(starts.clone()));
    mac.plugin(HostPlugin::new(id("main"), Arc::new(catalog)));
    let at_main = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    let mut offers = at_main.offers();
    eventually(|| offers.borrow_and_update().epoch("plugins"), "the host").await;
    let session = at_main.connection();
    let load = |args: Json| {
        let session = session.clone();
        async move {
            let reply = session
                .invoke_async("", "plugins.load", args.into())
                .await?;
            settle(reply).await
        }
    };

    // Isolated: provided in the row's own scope, not the host's.
    load(json!(["iso", "provider", {}, [["svc", "L"]], []]))
        .await
        .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(
        mac.get_at::<dyn HostDispatch>(&host_key("svc")).is_none(),
        "an isolated service stays in the row's scope"
    );

    // Injected: it waits for `needed`, then starts.
    let pending = tokio::spawn(load(json!([
        "dep",
        "provider",
        {},
        [["svc", "M"]],
        ["needed"]
    ])));
    // clock: weak-negative — a row gated on an injected service shows no
    // state the far end can wait on; a slow machine can only miss an early
    // start here, never fail. The start once it is there is the positive
    // half.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "it waits for what the row injects"
    );
    let _needed = mac.provide_at(host_key("needed"), clock(0)).unwrap();
    eventually(
        || (starts.load(Ordering::SeqCst) == 2).then_some(()),
        "the start once it is there",
    )
    .await;
    let _ = within("the injected row's load", pending).await;
}

/// A service name the host cannot map refuses the load: it is not dropped.
#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_plugin_with_an_unknown_service_is_refused() {
    let (main, mac, _link) = linked().await;
    let starts = Arc::new(AtomicU64::new(0));
    let catalog =
        StaticCatalog::new().with("provider", Described::default(), Provider(starts.clone()));
    mac.plugin(
        HostPlugin::new(id("main"), Arc::new(catalog)).services(Arc::new(|name: &str| {
            (name == "svc").then(|| host_key(name).erase())
        })),
    );
    let at_main = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    let mut offers = at_main.offers();
    eventually(|| offers.borrow_and_update().epoch("plugins"), "the host").await;
    let refused = at_main
        .connection()
        .invoke_async(
            "",
            "plugins.load",
            json!(["x", "provider", {}, [], ["unknown"]]).into(),
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("unknown is not a service"),
        "{refused}"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

/// A withdrawal is remembered: an older announcement arriving after it (the
/// far end's calls are dispatched concurrently) does not bring it back.
#[tokio::test(flavor = "multi_thread")]
async fn an_announcement_older_than_a_withdrawal_is_stale() {
    let (main, mac, _link) = linked().await;
    mac.plugin(ImportPlugin::new(id("main"), ["clock"]));
    let at_main = main.get_at::<Peer>(&peer_key(&id("mac"))).unwrap();
    let mut offers = at_main.offers();
    eventually(
        || offers.borrow_and_update().epoch("services"),
        "the importer",
    )
    .await;
    let session = at_main.connection();
    let announce = |version: u64| {
        let session = session.clone();
        async move {
            let record = Value::Record(
                [
                    ("name".to_owned(), Value::Data(json!("clock"))),
                    (
                        "service".to_owned(),
                        Value::Record(
                            [("now".to_owned(), Value::callback(|_| Ok(json!(7).into())))].into(),
                        ),
                    ),
                    ("shape".to_owned(), Value::Data(json!({ "now": "sync" }))),
                    ("version".to_owned(), Value::Data(json!(version))),
                ]
                .into(),
            );
            let reply = session
                .invoke_async("", "services.announce", Value::List(vec![record]))
                .await
                .unwrap();
            settle(reply).await.unwrap();
        }
    };
    let withdraw = |version: u64| {
        let session = session.clone();
        async move {
            let reply = session
                .invoke_async(
                    "",
                    "services.withdraw",
                    json!([{ "name": "clock", "version": version }]).into(),
                )
                .await
                .unwrap();
            settle(reply).await.unwrap();
        }
    };

    announce(1).await;
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "clock imported",
    )
    .await;
    withdraw(3).await;
    eventually(
        || {
            mac.get_at::<dyn HostDispatch>(&host_key("clock"))
                .is_none()
                .then_some(())
        },
        "clock withdrawn",
    )
    .await;
    // The importer decides before it replies (services.rs,
    // Importer::announce): once announce(2) returns, it was ignored or not.
    announce(2).await;
    assert!(
        mac.get_at::<dyn HostDispatch>(&host_key("clock")).is_none(),
        "an announcement older than the withdrawal is stale"
    );
    // A newer one provides it again.
    announce(4).await;
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "clock again",
    )
    .await;
}

/// An importer restarted at once looks, to the exporter, like `services`
/// offered anew with no withdrawal seen in between (the two changes merged):
/// it announces again all the same.
#[tokio::test(flavor = "multi_thread")]
async fn an_exporter_announces_again_to_a_new_offer_it_never_saw_withdrawn() {
    let (main, mac, _link) = linked().await;
    let _clock = main.provide_at(host_key("clock"), clock(0)).unwrap();
    // mac's importer, as a handler that counts the announcements.
    let at_mac = mac.get_at::<Peer>(&peer_key(&id("main"))).unwrap();
    let announced = Arc::new(AtomicU64::new(0));
    let count = announced.clone();
    let _services = at_mac
        .register(
            "services",
            Arc::new(
                move |_: &rutis_bridge::session::Connection, _: &str, method: &str, _| {
                    if method == "services.announce" {
                        count.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(Value::Undefined)
                },
            ),
        )
        .unwrap();
    main.plugin(ExportPlugin::new(id("mac"), ["clock"]));
    eventually(
        || (announced.load(Ordering::SeqCst) == 1).then_some(()),
        "the first announcement",
    )
    .await;

    // `services` offered again, registered anew: one change, as main sees it.
    let reply = at_mac
        .connection()
        .invoke_async(
            "",
            "link.offers",
            json!([{ "families": ["services"], "version": 1_000_000, "since": { "services": 999_999 } }]).into(),
        )
        .await
        .unwrap();
    settle(reply).await.unwrap();
    eventually(
        || (announced.load(Ordering::SeqCst) == 2).then_some(()),
        "the announcement to the new offer",
    )
    .await;
}

/// An export restarted with another list goes on counting where the last
/// one ended: the service it withdrew and announces again is back there.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_changed_to_another_list_keeps_what_it_still_exports() {
    let (main, mac, _link) = linked().await;
    let _clock = main.provide_at(host_key("clock"), clock(0)).unwrap();
    let _calendar = main.provide_at(host_key("calendar"), clock(100)).unwrap();
    mac.plugin(ImportPlugin::new(id("main"), ["clock", "calendar"]));
    let first = main.plugin(ExportPlugin::new(id("mac"), ["clock"]));
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "clock",
    )
    .await;

    first.dispose().await.unwrap();
    eventually(
        || {
            mac.get_at::<dyn HostDispatch>(&host_key("clock"))
                .is_none()
                .then_some(())
        },
        "clock withdrawn",
    )
    .await;
    main.plugin(ExportPlugin::new(id("mac"), ["clock", "calendar"]));
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("calendar")),
        "calendar",
    )
    .await;
    eventually(
        || mac.get_at::<dyn HostDispatch>(&host_key("clock")),
        "clock again",
    )
    .await;
}

/// The names of the entries a link imported into `ctx`'s bus.
fn imported_names(ctx: &Ctx) -> Vec<String> {
    let mut names: Vec<String> = ctx
        .events()
        .subscriptions()
        .into_iter()
        .filter(|entry| entry.remote)
        .map(|entry| entry.name)
        .collect();
    names.sort();
    names
}

/// guarantee: events/node-links
///
/// The importer's events feature restarts on a session that stays (its
/// link's events plugin replaced): what it announced before goes with that
/// offer. The exporter keeps only what the new offer announces, even though
/// the new feature counts its ids from the start again; a dispatch never
/// reaches an entry the importer no longer has.
#[tokio::test(flavor = "multi_thread")]
async fn entries_of_an_events_feature_that_restarted_are_replaced() {
    let (main, mac, _mac_link) = linked().await;
    let heard = Arc::new(Mutex::new(Vec::new()));
    mac.listen::<Args>()
        .name("t/a")
        .on(record(heard.clone(), Some("a")))
        .unwrap();
    let b = mac
        .listen::<Args>()
        .name("t/b")
        .on(record(heard.clone(), Some("b")))
        .unwrap();
    let mac_events = mac.plugin(LinkEventsPlugin::new(
        id("main"),
        Vec::<String>::new(),
        ["t/*"],
    ));
    (&mac_events).await.unwrap();
    (&main.plugin(LinkEventsPlugin::new(
        id("mac"),
        ["*"],
        Vec::<String>::new(),
    )))
        .await
        .unwrap();
    eventually(
        || (imported_names(&main) == ["t/a", "t/b"]).then_some(()),
        "both of mac's entries on main",
    )
    .await;

    // mac's events feature stops, mac's `t/b` goes meanwhile, and the
    // feature starts again on the same session.
    mac_events.dispose().await.unwrap();
    b.dispose().await.unwrap();
    (&mac.plugin(LinkEventsPlugin::new(
        id("main"),
        Vec::<String>::new(),
        ["t/*"],
    )))
        .await
        .unwrap();
    eventually(
        || (imported_names(&main) == ["t/a"]).then_some(()),
        "only the new offer's entry on main",
    )
    .await;
    // Nothing stale answers, or fails, for `t/b`.
    within("parallel of t/b", main.named("t/b").parallel(Args(vec![])))
        .await
        .unwrap();
    let answer = serial_until_heard(&main, "t/a", vec![]).await;
    assert_eq!(answer, Value::Data(json!("a")));
}

/// guarantee: events/node-links
///
/// An entry that would be called twice for one event through two prefixes
/// of the link's lists (`t/*` and `t/a/*`) is one entry on the other bus:
/// an event under both reaches it once.
#[tokio::test(flavor = "multi_thread")]
async fn nested_prefixes_of_a_link_deliver_once() {
    let (main, mac, _mac_link) = linked().await;
    let heard = Arc::new(Mutex::new(Vec::new()));
    // Records every event under its prefix.
    let every = heard.clone();
    mac.listen::<Args>()
        .prefix("t/")
        .on(move |event, _| {
            every.lock().unwrap().push(Value::List(event.0.clone()));
            std::future::ready(Ok(None))
        })
        .unwrap();
    (&mac.plugin(LinkEventsPlugin::new(
        id("main"),
        Vec::<String>::new(),
        ["t/*", "t/a/*"],
    )))
        .await
        .unwrap();
    (&main.plugin(LinkEventsPlugin::new(
        id("mac"),
        ["t/*", "t/a/*"],
        Vec::<String>::new(),
    )))
        .await
        .unwrap();
    eventually(
        || (!imported_names(&main).is_empty()).then_some(()),
        "mac's entry on main",
    )
    .await;
    assert_eq!(imported_names(&main), ["t/"], "one entry, the wider prefix");
    main.named("t/a/x")
        .parallel(Args(vec![json!(1).into()]))
        .await
        .unwrap();
    assert_eq!(
        heard.lock().unwrap().len(),
        1,
        "{:?}",
        heard.lock().unwrap()
    );
}
