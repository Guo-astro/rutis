//! Links inside instances: each instance's link exports and imports that
//! instance's services. Two applications in one process, over the memory
//! transport.
#![cfg(feature = "peer")]

use std::sync::Arc;
use std::time::Duration;

use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin, PluginFactory, TypeKey};
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::{host_key, host_key_in, HostDispatch, Reply, Value as RpcValue};
use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
use rutis_bridge::{Credential, IdentityPlugin, StaticIdentity};
use rutis_loader::{
    register_peer_node, Build, Builtins, Chain, EntryStatus, Layer, Loader, LoaderError,
    LoaderOptions, LoaderPlugin, Patch, PeerResolver, ServiceCatalog,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

async fn eventually<T>(mut check: impl FnMut() -> Option<T>, what: &str) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(found) = check() {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// A service answering `get` with a fixed value.
struct Fixed(Value);

impl HostDispatch for Fixed {
    fn invoke(&self, _: &str, _: RpcValue) -> Reply {
        Ok(self.0.clone().into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "get": "sync" }))
    }
}

fn get(service: &Arc<dyn HostDispatch>) -> Value {
    service
        .invoke("get", RpcValue::List(Vec::new()))
        .unwrap()
        .json()
        .unwrap()
}

/// The instance's title, given with `with`.
struct Title(String);

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
struct NoConfig {}

/// Provides the instance's `tools`, which tells the instance's title.
struct Tools {
    key: TypeKey,
    title: String,
}

impl Plugin for Tools {
    fn name(&self) -> &str {
        "tools"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            ctx.provide_as::<dyn HostDispatch>(
                self.key.clone(),
                Arc::new(Fixed(json!(self.title))),
            )?;
            Ok(Effect::Done)
        })
    }
}

struct ToolsFactory {
    key: TypeKey,
    title: String,
}

impl PluginFactory<NoConfig> for ToolsFactory {
    fn name(&self) -> &str {
        "tools"
    }

    fn build(&self, _: &NoConfig) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(Tools {
            key: self.key.clone(),
            title: self.title.clone(),
        }))
    }
}

fn layer(rows: Value) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

/// `main`, whose `session` instances each link to `mac`, and `mac`, which
/// imports the instance's `tools` and exports its `clock`.
struct Nodes {
    main: Ctx,
    loader: Loader,
    mac: Ctx,
}

async fn main_node(transport: &Arc<MemoryTransport>, rows: Value) -> (Ctx, Loader) {
    let main = Ctx::root().unwrap();
    (&main.plugin(MemoryPlugin::with_transport(transport.clone())))
        .await
        .unwrap();
    main.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("mac-token", id("mac")),
    ));
    let mut builtins = Builtins::new();
    builtins.register_with::<NoConfig, _, _>("tools", |build: &Build| {
        Ok(ToolsFactory {
            key: host_key_in("tools", build.instance("session")?),
            title: build
                .value::<Title>()
                .map(|t| t.0.clone())
                .unwrap_or_default(),
        })
    });
    let peers = Arc::new(PeerResolver::new());
    register_peer_node(&mut builtins, peers.clone());
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared_instance("tools", "session");
    catalog.register_shared_instance("clock", "session");
    let plugin = LoaderPlugin::new(
        Chain::new().with(builtins).with_shared(peers),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    (&main.plugin(plugin)).await.unwrap();
    loader.reconcile(layer(rows), None).await.unwrap();
    (main, loader)
}

async fn nodes() -> Nodes {
    let transport = Arc::new(MemoryTransport::default());
    transport.endpoint("main-in", id("main"));
    let (main, loader) = main_node(
        &transport,
        json!([
            { "id": "session", "group": true, "instanced": true, "config": [
                { "id": "tools", "name": "tools" },
                { "id": "link", "name": "rutis-bridge/peer", "config": {
                    "peer": "mac", "transport": "memory", "identity": "main",
                    "listen": "main-in", "export": ["tools"], "import": ["clock"]
                } }
            ] }
        ]),
    )
    .await;

    let mac = Ctx::root().unwrap();
    mac.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(Fixed(json!(7))))
        .unwrap();
    (&mac.plugin(MemoryPlugin::with_transport(transport)))
        .await
        .unwrap();
    mac.plugin(IdentityPlugin::new(
        "mac",
        StaticIdentity::new(id("mac")).present(id("main"), Credential::Bearer("mac-token".into())),
    ));
    let mut builtins = Builtins::new();
    register_peer_node(&mut builtins, Arc::new(PeerResolver::new()));
    let plugin = LoaderPlugin::new(Chain::new().with(builtins), LoaderOptions::default());
    let mac_loader = plugin.handle();
    (&mac.plugin(plugin)).await.unwrap();
    mac_loader
        .reconcile(
            layer(json!([
                { "id": "link", "name": "rutis-bridge/peer", "config": {
                    "peer": "main", "transport": "memory", "identity": "mac",
                    "dial": "main-in", "import": ["tools"], "export": ["clock"]
                } }
            ])),
            None,
        )
        .await
        .unwrap();
    Nodes { main, loader, mac }
}

fn bound(ctx: &Ctx, key: &TypeKey) -> bool {
    ctx.diagnostics().bindings.iter().any(|b| &b.key == key)
}

/// The instance's link exports the instance's `tools`, and imports `clock`
/// as the instance's; removing the instance closes its link.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_inside_an_instance_exchanges_that_instance_s_services() {
    let nodes = nodes().await;
    let a = nodes
        .loader
        .create_instance(&nodes.main, "session")
        .with(Title("A".into()))
        .await
        .unwrap();
    let tools = eventually(
        || nodes.mac.get_as::<dyn HostDispatch>(host_key("tools")),
        "mac to import A's tools",
    )
    .await;
    assert_eq!(get(&tools), json!("A"));
    let clock = host_key_in("clock", a.view.instance());
    eventually(
        || bound(&nodes.main, &clock).then_some(()),
        "clock imported into A",
    )
    .await;
    // Imported as the instance's only.
    assert!(!bound(&nodes.main, &host_key("clock")));

    nodes.loader.remove_instance(a.plugin).await.unwrap();
    eventually(
        || {
            nodes
                .mac
                .get_as::<dyn HostDispatch>(host_key("tools"))
                .is_none()
                .then_some(())
        },
        "A's tools withdrawn from mac",
    )
    .await;
    assert!(!bound(&nodes.main, &clock));
}

fn failure(loader: &Loader, id: &str) -> String {
    match loader.get(id).map(|e| e.status) {
        Some(EntryStatus::Running(snapshot)) => snapshot
            .error
            .map(|e| e.to_string())
            .unwrap_or_else(|| format!("{:?}", snapshot.state)),
        other => format!("{other:?}"),
    }
}

/// What a link cannot do where it is, it refuses, saying why: an instance
/// name on a link outside instances, and rows or a runtime on a link
/// inside them.
#[tokio::test(flavor = "multi_thread")]
async fn links_refuse_what_their_place_cannot_do() {
    let transport = Arc::new(MemoryTransport::default());
    let (main, loader) = main_node(
        &transport,
        json!([
            { "id": "session", "group": true, "instanced": true, "config": [
                { "id": "rows-link", "name": "rutis-bridge/peer", "config": {
                    "peer": "mac", "transport": "memory", "identity": "main",
                    "listen": "main-in", "rows": true
                } }
            ] },
            { "id": "global-link", "name": "rutis-bridge/peer", "config": {
                "peer": "mac", "transport": "memory", "identity": "main",
                "listen": "main-in", "export": ["tools"]
            } }
        ]),
    )
    .await;
    let error = failure(&loader, "global-link");
    assert!(
        error.contains(
            &LoaderError::OutsideInstance {
                name: "tools".into(),
                group: "session".into(),
            }
            .to_string()
        ),
        "{error}"
    );
    let a = loader.create_instance(&main, "session").await.unwrap();
    let error = format!("{:?}", a.report);
    assert!(error.contains("cannot carry rows"), "{error}");
}
