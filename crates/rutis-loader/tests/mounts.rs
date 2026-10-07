mod common;

use common::*;
use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin};
use rutis_loader::{Layer, LoaderOptions};
use serde_json::json;
use std::sync::{Arc, Mutex};

struct Host(Arc<Mutex<Option<Ctx>>>);
impl Plugin for Host {
    fn name(&self) -> &str {
        "document"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            *self.0.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn expands_and_unloads_each_host() {
    let harness = Harness::new();
    let (root, loader) = mount_with(harness.resolver(), LoaderOptions::default()).await;
    loader
        .reconcile(
            vec![Layer::new(
                "base",
                vec![serde_json::from_value(json!({
        "insert": [{"id":"a", "name":"echo", "mount":"document", "config":{"label":"mounted"}}]
    })).unwrap()],
            )],
            None,
        )
        .await
        .unwrap();
    assert!(loader.entries().is_empty());
    let one = Arc::new(Mutex::new(None));
    let two = Arc::new(Mutex::new(None));
    let first = root.plugin(Host(one.clone()));
    first.clone().await.unwrap();
    let second = root.plugin(Host(two.clone()));
    second.clone().await.unwrap();
    let a = one.lock().unwrap().clone().unwrap();
    let b = two.lock().unwrap().clone().unwrap();
    loader.register_mount("document", &a).await.unwrap();
    loader.register_mount("document", &b).await.unwrap();
    assert_eq!(loader.entries().len(), 2);
    assert_eq!(
        harness
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|s| *s == "apply:mounted")
            .count(),
        2
    );
    assert!(loader.register_mount("document", &a).await.is_err());
    loader.unregister_mount(&a).await;
    assert_eq!(loader.entries().len(), 1);
    second.dispose().await.unwrap();
    assert!(loader.entries().is_empty());
    first.dispose().await.unwrap();
}

#[cfg(all(unix, feature = "node"))]
#[tokio::test(flavor = "multi_thread")]
async fn node_instance_services_remain_native_and_separate() {
    use rutis_bridge::runtime::LocalRuntime;
    use rutis_loader::{
        Chain, LoaderPlugin, RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog, ServiceScope,
    };
    let root = Ctx::root().unwrap();
    let package = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("instance.mjs");
    std::fs::write(&entry, "export function apply(ctx) { ctx.provide('document', { marker: {} }); ctx.plugin({ inject: ['document'], apply(child) { if (child.document !== ctx.document) throw new Error('object identity lost') } }) }").unwrap();
    let runtime = LocalRuntime::node(&package, package.join("package.json"));
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("document");
    catalog
        .set_scope("document", ServiceScope::Instance)
        .unwrap();
    let resolver = Arc::new(RuntimeResolver::node(runtime.handle()).with_catalog(&catalog));
    let runtime_view = root.plugin(runtime);
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(resolver.clone()),
        LoaderOptions {
            catalog,
            ..Default::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(resolver)).await.unwrap();
    (&runtime_view).await.unwrap();
    loader
        .reconcile(
            vec![Layer::new(
                "base",
                vec![serde_json::from_value(
                    json!({"insert": [{"id":"a", "name":entry, "mount":"document"}]}),
                )
                .unwrap()],
            )],
            None,
        )
        .await
        .unwrap();
    let mut hosts = Vec::new();
    for _ in 0..2 {
        let captured = Arc::new(Mutex::new(None));
        let view = root.plugin(Host(captured.clone()));
        (&view).await.unwrap();
        let ctx = captured.lock().unwrap().clone().unwrap();
        loader.register_mount("document", &ctx).await.unwrap();
        hosts.push(view);
    }
    assert_eq!(loader.entries().len(), 2);
    for entry in loader.entries() {
        assert!(
            matches!(entry.status, rutis_loader::EntryStatus::Running(ref state) if state.state == rutis::FiberState::Active),
            "{entry:?}"
        );
    }
    for host in hosts {
        host.dispose().await.unwrap();
    }
}
