//! A mount's `plugins` list is loaded in the mounting plugin's scope: the
//! runtime applies the `mount` call's `scope` to them, and a plugin sees it
//! as its `ctx.rutis.scope` (events protocol §3).
//!
//! rule: EVP-3-7

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::{Mount, Process};
use rutis_test_support::until;
use serde_json::json;

/// A plugin whose fiber is an instance of a group: one link of the scope a
/// mount made inside it carries.
struct Scoped {
    group: &'static str,
    ctx: Arc<Mutex<Option<Ctx>>>,
}

impl DynPlugin for Scoped {
    fn name(&self) -> &str {
        self.group
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.mark_scope(self.group)?;
            *self.ctx.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

/// A plugin the mount's `plugins` list loads: it answers the mount's scope.
const PLUGIN: &str = r#"
export function apply(ctx) {
  ctx.provide('probe', {
    scope() { return ctx.rutis.scope },
  })
}
"#;

#[tokio::test(flavor = "multi_thread")]
async fn plugins_loaded_by_mount_get_the_mounting_plugins_scope() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let root = crate::common::runtime::root();
    let tenant_slot = Arc::new(Mutex::new(None));
    let tenant = root.plugin(Scoped {
        group: "tenant",
        ctx: tenant_slot.clone(),
    });
    (&tenant).await.unwrap();
    let tenant_ctx = tenant_slot.lock().unwrap().clone().unwrap();
    let session_slot = Arc::new(Mutex::new(None));
    let session = tenant_ctx.plugin(Scoped {
        group: "session",
        ctx: session_slot.clone(),
    });
    (&session).await.unwrap();
    let scoped = session_slot.lock().unwrap().clone().unwrap();
    let expected = serde_json::to_value(scoped.scope()).unwrap();
    let chain = expected.as_array().unwrap();
    assert_eq!(
        chain.len(),
        2,
        "the mounting plugin is two nested instances: {expected}"
    );
    let groups: Vec<&str> = chain
        .iter()
        .map(|link| link["group"].as_str().unwrap())
        .collect();
    assert_eq!(groups, ["tenant", "session"], "outermost first: {expected}");
    assert!(
        chain.iter().all(|link| link["id"].is_string()),
        "{expected}"
    );

    let mut plugin = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    plugin.write_all(PLUGIN.as_bytes()).unwrap();
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
    let process = Process::mount(
        &package,
        Mount {
            plugins: vec![(plugin.path(), json!({}))],
            services: json!({ "probe": ["scope"] }),
            ctx: Some(scoped),
            ..Mount::default()
        },
    )
    .await
    .unwrap();
    let probe = process.service("probe").unwrap();
    until(
        || process.call(&probe, "scope", json!([])).ok().as_ref() == Some(&expected),
        "the mount's scope in the plugin",
    )
    .await;
    process.dispose().await.unwrap();
}
