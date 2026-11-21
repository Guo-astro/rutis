//! guarantee: loader/announced-services
//! `SpawnPlugin::undeclared` is the host's policy for the services a
//! runtime's rows provide without declaring them (#1040): `Refuse` keeps
//! such a name out of the host's services, and fails its row with the
//! reason, while the runtime and its other rows go on.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use crate::common::{file_url, repo};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::{Launcher, UndeclaredPolicy};
use rutis_bridge::session::host_key;
use rutis_host::connect::SpawnPlugin;
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::json;

/// Node runtimes start cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 3));

/// `command` (`rutis_test_support::runtimes`) as a launcher.
fn launcher(command: rutis_test_support::runtimes::Command) -> Launcher {
    let mut launcher = Launcher::new(command.program);
    launcher.args = command.args;
    launcher.env = command.env;
    launcher.cwd = command.cwd;
    launcher.inherit_fd = command.inherit_fd;
    launcher
}

/// A runtime started with `SpawnPlugin::undeclared(Refuse)` does not let a
/// row's undeclared `llm` reach the host: the row fails saying so, and the
/// host has no `llm`.
#[tokio::test(flavor = "multi_thread")]
async fn a_spawned_runtime_refuses_what_its_rows_do_not_declare() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    std::fs::write(
        dir.path().join("impostor.mjs"),
        format!(
            "import {{ definePlugin }} from {:?}\n\
             export default definePlugin({{ apply(ctx) {{\n\
               ctx.provide('llm', {{ ask() {{ return 'impostor' }} }})\n\
             }} }})\n",
            file_url(&sdk)
        ),
    )
    .unwrap();

    let root = Ctx::root().unwrap();
    root.provide(rutis_bridge::LocalId(
        rutis_bridge::PeerId::new("main").unwrap(),
    ))
    .unwrap();
    let catalog = ServiceCatalog::new();
    let sessions = Arc::new(SessionResolver::new(&catalog));
    let runtime = SpawnPlugin::new(
        "strict",
        launcher(rutis_test_support::runtimes::node_runtime()),
        repo().join("node/rutis-runtime/package.json"),
    )
    .undeclared(UndeclaredPolicy::Refuse);
    let handle = runtime.handle();
    sessions.runtime(handle.clone());
    root.plugin(runtime);
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    sessions.attach(&loader);
    (&root.plugin(plugin)).await.unwrap();
    sessions.follow_row("strict");
    assert!(
        COLD.within("the strict runtime to run", handle.ready())
            .await
            .is_some(),
        "the runtime runs"
    );
    let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
        { "id": "impostor", "name": format!("strict:{}", dir.path().join("impostor.mjs").display()) }
    ]}]))
    .unwrap();
    loader
        .reconcile(vec![Layer::new("rows", rows)], None)
        .await
        .unwrap();
    let error = COLD
        .eventually_with(
            || match loader.get("impostor")?.status {
                EntryStatus::Running(snapshot) if snapshot.state == FiberState::Failed => Some(
                    snapshot
                        .error
                        .map(|error| rutis_bridge::report::report(&error))
                        .unwrap_or_default(),
                ),
                _ => None,
            },
            "the row to fail",
            || format!("{:?}", loader.get("impostor").map(|entry| entry.status)),
        )
        .await;
    assert_eq!(
        error,
        "llm is provided by row impostor without declaring it; the host does not let a runtime \
         publish names it does not declare"
    );
    assert!(
        root.get_at::<dyn rutis_bridge::session::HostDispatch>(&host_key("llm"))
            .is_none(),
        "the refused name is not the host's"
    );
    root.shutdown().await.unwrap();
}
