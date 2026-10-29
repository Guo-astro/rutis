//! Swift rows: one runtime per binary, several side by side, as Go's
//! (`go_rows.rs`, which covers the machinery both share: binary.rs). Rows
//! name a plugin (`swift:ping`) and find the binary that has it from its
//! manifest; plugins in different binaries use each other's services
//! through rutis; a crash stops only its runtime's rows.
#![cfg(all(feature = "swift", target_os = "macos"))]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::runtimes::{greet_as_main, layer, row_state, spawner, swift_binary, COLD};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{host_key, settle, HostDispatch};
use rutis_loader::internal::read_binary_manifest;
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::binary::{
    Binaries, BinaryResolver, BinaryRuntimeState, BinaryRuntimes, BinaryRuntimesHandle,
};
use rutis_loader::runtime::swift::Swift;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// Put the Swift product `product` into `dir` as `name`.
fn install(dir: &Path, product: &str, name: &str) -> PathBuf {
    let target = dir.join(name);
    std::fs::copy(swift_binary(product, name), &target).unwrap();
    target
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    resolver: Arc<BinaryResolver<Swift>>,
    runtimes: BinaryRuntimesHandle<Swift>,
    dir: tempfile::TempDir,
}

/// When the fixture's runtimes start.
#[derive(Clone, Copy)]
enum Start {
    /// All at once, and they keep running.
    Eager,
    /// As rows use them, stopping once idle this long (never: `None`).
    OnDemand(Option<Duration>),
}

async fn fixture(start: Start, binaries: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    for (product, name) in binaries {
        install(&plugins, product, name);
    }
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    let catalog = ServiceCatalog::new();
    let resolver = Arc::new(
        BinaryResolver::<Swift>::new(Binaries::new().dir(&plugins))
            .with_catalog(&catalog)
            .reserve(["node", "py"]),
    );
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(resolver.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let swift = BinaryRuntimes::<Swift>::new(resolver.clone(), dir.path(), spawner());
    let swift = match start {
        Start::Eager => swift.eager(),
        Start::OnDemand(idle) => swift.idle(idle),
    };
    let runtimes = swift.handle();
    root.plugin(swift).await.unwrap();
    Fixture {
        root,
        loader,
        resolver,
        runtimes,
        dir,
    }
}

fn row(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name })
}

fn unresolved(loader: &Loader, id: &str) -> Option<String> {
    match loader.get(id)?.status {
        EntryStatus::Unresolved(error) => Some(error.to_string()),
        _ => None,
    }
}

fn runtime_state(runtimes: &BinaryRuntimesHandle<Swift>, name: &str) -> Option<BinaryRuntimeState> {
    runtimes
        .runtimes()
        .into_iter()
        .find(|runtime| runtime.name == name)
        .map(|runtime| runtime.state)
}

fn service(root: &Ctx, name: &str) -> Option<Arc<dyn HostDispatch>> {
    root.get_at::<dyn HostDispatch>(&host_key(name))
}

/// Call `method` of the service `name` synchronously, off the async runtime.
async fn call(root: &Ctx, name: &str, method: &str, args: Value) -> Result<Value, String> {
    let service = service(root, name).ok_or(format!("no service {name}"))?;
    let method = method.to_owned();
    tokio::task::spawn_blocking(move || service.invoke(&method, args.into()))
        .await
        .unwrap()
        .map_err(|error| error.to_string())
        .and_then(|value| value.json().map_err(|error| error.to_string()))
}

/// A Swift binary describes itself: the manifest names its plugins, the
/// SDK, the plugin API and the platform, without starting a session.
/// guarantee: loader/swift-runtimes
#[test]
fn a_binary_describes_itself() {
    let binary = swift_binary("RutisMultilang", "multilang");
    let manifest = read_binary_manifest(&binary).unwrap();
    let text = format!("{manifest:?}");
    for plugin in ["swift_provider", "swift_registrant", "caller", "probe"] {
        assert!(text.contains(plugin), "{text}");
    }
    let output = std::process::Command::new(&binary)
        .arg("--rutis-manifest")
        .output()
        .unwrap();
    assert!(output.status.success());
    let manifest: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(manifest["runtime"], json!("rutis-swift-runtime:1"));
    assert_eq!(manifest["pluginApi"], json!(3));
    assert!(manifest["platform"].as_str().unwrap().starts_with("macos/"));
    assert_eq!(
        manifest["plugins"]["caller"]["provides"],
        json!({ "caller": { "relay": "sync", "relayLater": "async" } })
    );
    assert_eq!(manifest["plugins"]["caller"]["announces"], json!(true));
}

/// guarantee: loader/swift-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn rows_find_their_binary_and_call_across_binaries() {
    let fixture = fixture(
        Start::OnDemand(None),
        &[("RutisPing", "netkit"), ("RutisMultilang", "multilang")],
    )
    .await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // An executable without the SDK's marker is never run.
        let ran = fixture.dir.path().join("ran");
        let script = fixture.dir.path().join("plugins/tool");
        std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", ran.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fixture
        .loader
        .reconcile(
            layer(vec![
                row("ping", "swift:ping"),
                row("caller", "swift:caller"),
            ]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            ["ping", "caller"].iter().all(|id| {
                row_state(&loader, id) == Some(FiberState::Active)
                    && service(&fixture.root, id).is_some()
            })
        },
        "both rows",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "caller", "relay", json!(["hi"]))
            .await
            .unwrap(),
        json!("netkit pong hi"),
        "a sync call from one binary's plugin into another's"
    );
    let caller = service(&fixture.root, "caller").unwrap();
    let later = settle(caller.invoke("relayLater", json!(["hi"]).into()).unwrap())
        .await
        .unwrap();
    assert_eq!(later.json().unwrap(), json!("netkit later hi"));
    drop(caller);
    for name in ["swift-netkit", "swift-multilang"] {
        assert_eq!(
            runtime_state(&fixture.runtimes, name),
            Some(BinaryRuntimeState::Running),
            "{name}"
        );
    }
    let meta = fixture.loader.get("ping").unwrap().meta;
    assert_eq!(meta["runtime"], json!("swift-netkit"));
    assert_eq!(meta["source"], json!("swift"));
    assert!(
        !fixture.dir.path().join("ran").exists(),
        "an unmarked file was run"
    );
    let diagnostics = fixture.resolver.diagnostics();
    assert!(
        !diagnostics.iter().any(|line| line.contains("tool")),
        "{diagnostics:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/swift-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_in_two_binaries_is_named_with_its_runtime() {
    let fixture = fixture(
        Start::OnDemand(None),
        &[("RutisPing", "netkit"), ("RutisPing", "dup")],
    )
    .await;
    fixture
        .loader
        .reconcile(layer(vec![row("p", "swift:ping")]), None)
        .await
        .unwrap();
    let message = unresolved(&fixture.loader, "p").expect("swift:ping is ambiguous");
    assert!(message.contains("several Swift binaries"), "{message}");
    assert!(
        message.contains("swift-dup") && message.contains("swift-netkit"),
        "{message}"
    );
    fixture
        .loader
        .reconcile(layer(vec![row("p", "swift-dup:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            row_state(&loader, "p") == Some(FiberState::Active)
                && service(&fixture.root, "ping").is_some()
        },
        "the qualified row",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["x"]))
            .await
            .unwrap(),
        json!("dup pong x")
    );
    fixture.root.shutdown().await.unwrap();
}

/// `eager`: every binary's runtime starts at once, side by side, before
/// any row uses it, and a crash stops only that runtime's rows.
/// guarantee: loader/swift-runtimes, loader/runtime-down-stops-its-rows
#[tokio::test(flavor = "multi_thread")]
async fn eager_runtimes_run_side_by_side_and_a_crash_stops_only_its_rows() {
    let fixture = fixture(
        Start::Eager,
        &[("RutisPing", "netkit"), ("RutisMultilang", "multilang")],
    )
    .await;
    let runtimes = fixture.runtimes.clone();
    COLD.until(
        || {
            ["swift-netkit", "swift-multilang"]
                .iter()
                .all(|name| runtime_state(&runtimes, name) == Some(BinaryRuntimeState::Running))
        },
        "both runtimes, with no row",
    )
    .await;
    fixture
        .loader
        .reconcile(
            layer(vec![row("ping", "swift:ping"), row("probe", "swift:probe")]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            row_state(&loader, "ping") == Some(FiberState::Active)
                && row_state(&loader, "probe") == Some(FiberState::Active)
                && service(&fixture.root, "ping").is_some()
                && service(&fixture.root, "work").is_some()
        },
        "the rows",
    )
    .await;
    let _ = call(&fixture.root, "ping", "crash", json!([])).await;
    COLD.until(
        || {
            matches!(
                runtime_state(&runtimes, "swift-netkit"),
                Some(BinaryRuntimeState::Stopped(_))
            )
        },
        "the crashed runtime",
    )
    .await;
    COLD.until(
        || row_state(&loader, "ping") != Some(FiberState::Active),
        "its row stops",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        row_state(&loader, "probe"),
        Some(FiberState::Active),
        "the other binary runs on"
    );
    assert_eq!(
        call(&fixture.root, "work", "ping", json!([]))
            .await
            .unwrap(),
        json!("pong")
    );
    fixture.root.shutdown().await.unwrap();
}

/// On demand: a runtime starts when a row uses it, keeps running while one
/// does, stops once idle, and starts again for the next row (design S5).
/// guarantee: loader/swift-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_runtime_stops_and_starts_again_when_used() {
    let fixture = fixture(
        Start::OnDemand(Some(Duration::from_millis(300))),
        &[("RutisPing", "netkit"), ("RutisMultilang", "multilang")],
    )
    .await;
    assert_eq!(
        runtime_state(&fixture.runtimes, "swift-netkit"),
        Some(BinaryRuntimeState::NotStarted),
        "nothing starts before a row uses it"
    );
    fixture
        .loader
        .reconcile(
            layer(vec![row("ping", "swift:ping"), row("probe", "swift:probe")]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            row_state(&loader, "ping") == Some(FiberState::Active)
                && row_state(&loader, "probe") == Some(FiberState::Active)
                && service(&fixture.root, "ping").is_some()
        },
        "the rows",
    )
    .await;
    let ping = service(&fixture.root, "ping").unwrap();
    // The probe no longer used: its runtime stops once idle, while the one
    // a row still uses runs on, its row never stopped.
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "swift:ping")]), None)
        .await
        .unwrap();
    let runtimes = fixture.runtimes.clone();
    COLD.until(
        || runtime_state(&runtimes, "swift-multilang") == Some(BinaryRuntimeState::NotStarted),
        "the idle stop of swift-multilang",
    )
    .await;
    assert_eq!(
        runtime_state(&fixture.runtimes, "swift-netkit"),
        Some(BinaryRuntimeState::Running)
    );
    let now = service(&fixture.root, "ping").expect("ping's service");
    assert!(
        std::ptr::addr_eq(Arc::as_ptr(&now), Arc::as_ptr(&ping)),
        "the same row, never stopped"
    );
    drop((now, ping));

    fixture.loader.reconcile(layer(vec![]), None).await.unwrap();
    COLD.until(
        || runtime_state(&runtimes, "swift-netkit") == Some(BinaryRuntimeState::NotStarted),
        "the idle stop of swift-netkit",
    )
    .await;

    fixture
        .loader
        .reconcile(layer(vec![row("ping", "swift:ping")]), None)
        .await
        .unwrap();
    COLD.until(
        || {
            row_state(&loader, "ping") == Some(FiberState::Active)
                && service(&fixture.root, "ping").is_some()
        },
        "the row again",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["again"]))
            .await
            .unwrap(),
        json!("netkit pong again")
    );
    fixture.root.shutdown().await.unwrap();
}

/// A name clash of an announced service fails the Swift row that provides
/// it later, naming both, as in every runtime (contract §4): the host
/// answers the announcement with `undefined` whether or not it follows it,
/// and fails the row itself once `rows.load` is answered; the SDK only
/// waits for that answer. A row announcing a free name loads, and its
/// service reaches rutis. (#542)
/// guarantee: loader/announced-services
#[tokio::test(flavor = "multi_thread")]
async fn an_announced_service_another_provider_has_fails_the_later_row() {
    let fixture = fixture(Start::OnDemand(None), &[("RutisMultilang", "multilang")]).await;
    // The application provides `llm` itself.
    fixture
        .root
        .provide_at(host_key("llm"), Arc::new(common::runtimes::Llm))
        .unwrap();
    let undeclared = |id: &str, service: &str| json!({ "id": id, "name": "swift:swift_undeclared", "config": { "service": service } });
    fixture
        .loader
        .reconcile(
            layer(vec![undeclared("clash", "llm"), undeclared("free", "pong")]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            row_state(&loader, "clash") == Some(FiberState::Failed)
                && row_state(&loader, "free") == Some(FiberState::Active)
        },
        "the clashing row to fail and the other to load",
    )
    .await;
    let EntryStatus::Running(snapshot) = fixture.loader.get("clash").unwrap().status else {
        panic!("clash has no fiber");
    };
    let error = snapshot.error.expect("why it failed").to_string();
    assert!(
        error.contains("llm is already provided by the host"),
        "{error}"
    );
    // Announced methods are all `async`.
    let pong = service(&fixture.root, "pong").expect("the announced service");
    let echoed = settle(pong.invoke("echo", json!(["hi"]).into()).unwrap())
        .await
        .unwrap();
    assert_eq!(echoed.json().unwrap(), json!("undeclared pong hi"));
    drop(pong);
    assert_eq!(
        call(&fixture.root, "llm", "ask", json!([])).await.unwrap(),
        json!("42"),
        "the earlier provider keeps the name"
    );
    fixture.root.shutdown().await.unwrap();
}
