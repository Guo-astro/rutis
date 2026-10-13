//! Bun rows (`bun:<module>`), with only the Bun runtime: rows gate on what
//! they inject, use each other's services within the process, stop when
//! their provider goes, run edited code after a reload, report their
//! package's version, and a module the project lacks is unresolved without
//! holding up the others.
#![cfg(all(unix, feature = "bun"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rutis::{Ctx, FiberState, FiberView};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{Reply, Value as RpcValue};
use rutis_loader::{
    Chain, EntryStatus, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch,
    RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

/// Twice HANG_GUARD: the Bun runtime starts cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<String>>>);

impl HostDispatch for Probe {
    fn invoke(&self, _method: &str, args: RpcValue) -> Reply {
        let [line]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "record": "sync" }))
    }
}

impl Probe {
    fn position(&self, line: &str) -> Option<usize> {
        self.0.lock().unwrap().iter().position(|l| l == line)
    }

    async fn wait_for(&self, line: &str) {
        COLD.eventually_with(
            || self.position(line).map(|_| ()),
            &format!("{line:?} recorded"),
            || format!("{:?}", self.0.lock().unwrap()),
        )
        .await
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A plugin written with the SDK, imported from this checkout.
fn sdk_plugin(body: &str) -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    format!(
        "import {{ definePlugin }} from {:?}\nexport default definePlugin({body})\n",
        sdk.to_string_lossy()
    )
}

const PROVIDER: &str = r#"{
  inject: ['probe'],
  provides: { forecast: { today: 'sync' } },
  apply(ctx) {
    const probe = ctx.use('probe')
    ctx.provide('forecast', { native: true, today: () => 'sunny' })
    probe.record('provider: start')
    return () => probe.record('provider: bye')
  },
}"#;

const USER: &str = r#"{
  inject: ['forecast', 'probe'],
  apply(ctx) {
    const probe = ctx.use('probe')
    const forecast = ctx.use('forecast')
    probe.record(`user: ${forecast.today()} (${forecast.native === true ? 'native' : 'proxy'})`)
    return () => probe.record('user: bye')
  },
}"#;

const BYSTANDER: &str = r#"{
  inject: ['probe'],
  apply(ctx) {
    const probe = ctx.use('probe')
    probe.record('bystander: start')
    return () => probe.record('bystander: bye')
  },
}"#;

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    runtime: FiberView,
    dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    fixture_with(|_| {}).await
}

/// `install` puts what the project has installed before the runtime starts:
/// Bun does not see a package installed while it runs (it keeps that a
/// package was missing), until the runtime restarts.
async fn fixture_with(install: impl FnOnce(&Path)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    install(dir.path());
    for (name, body) in [
        ("provider", PROVIDER),
        ("user", USER),
        ("bystander", BYSTANDER),
    ] {
        std::fs::write(dir.path().join(format!("{name}.ts")), sdk_plugin(body)).unwrap();
    }
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in ["probe", "forecast"] {
        catalog.register_shared(name);
    }
    let bun = LocalRuntime::bun(repo().join("bun/rutis-bun"), dir.path());
    let rows = Arc::new(RuntimeResolver::modules(bun.handle()).with_catalog(&catalog));
    let runtime = root.plugin(bun);
    (&runtime).await.unwrap();
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(rows.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(rows));
    Fixture {
        root,
        loader,
        probe,
        runtime,
        dir,
    }
}

fn layer(rows: Vec<Value>) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

fn row(id: &str, module: &str) -> Value {
    json!({ "id": id, "name": format!("bun:{module}"), "config": {} })
}

fn state(loader: &Loader, id: &str) -> Option<FiberState> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

/// A row waits for what it injects; within the process it gets the
/// provider's own object; when the provider goes, only its user stops, and
/// before the provider does.
/// guarantee: loader/rows-gate-on-services
#[tokio::test(flavor = "multi_thread")]
async fn rows_gate_share_and_stop_with_their_provider() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let all = vec![
        row("u", "./user.ts"),
        row("p", "./provider.ts"),
        row("b", "./bystander.ts"),
    ];
    let report = loader.reconcile(layer(all.clone()), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.probe.wait_for("user: sunny (native)").await;
    fixture.probe.wait_for("bystander: start").await;

    let remaining: Vec<Value> = all.into_iter().filter(|r| r["id"] != "p").collect();
    loader.reconcile(layer(remaining), None).await.unwrap();
    fixture.probe.wait_for("provider: bye").await;
    COLD.until(
        || state(loader, "u") == Some(FiberState::Pending),
        "the user to wait",
    )
    .await;
    assert_eq!(state(loader, "b"), Some(FiberState::Active));
    assert!(
        fixture.probe.position("user: bye").unwrap()
            < fixture.probe.position("provider: bye").unwrap(),
        "the user stops first"
    );
    fixture.root.shutdown().await.unwrap();
}

/// After the module's source changes, reloading the row runs the new code.
/// guarantee: loader/reload-all-or-nothing
#[tokio::test(flavor = "multi_thread")]
async fn reloading_a_row_runs_the_edited_module() {
    let fixture = fixture().await;
    let module = fixture.dir.path().join("edited.ts");
    let plugin = |version: &str| {
        sdk_plugin(&format!(
            "{{ inject: ['probe'], apply(ctx) {{ const probe = ctx.use('probe'); probe.record('{version}: start'); return () => probe.record('{version}: bye') }} }}"
        ))
    };
    std::fs::write(&module, plugin("v1")).unwrap();
    let report = fixture
        .loader
        .reconcile(layer(vec![row("e", "./edited.ts")]), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture.probe.wait_for("v1: start").await;

    std::fs::write(&module, plugin("v2")).unwrap();
    // A later modification time, even within the same clock tick.
    std::fs::File::options()
        .write(true)
        .open(&module)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    fixture.loader.reload("e").await.unwrap();
    fixture.probe.wait_for("v1: bye").await;
    fixture.probe.wait_for("v2: start").await;
    fixture.root.shutdown().await.unwrap();
}

/// A row of an installed package records the package's version; a module
/// the project lacks is unresolved, and the other rows run.
/// guarantee: loader/unresolved-rows
#[tokio::test(flavor = "multi_thread")]
async fn packages_report_versions_and_missing_modules_are_unresolved() {
    let fixture = fixture_with(|dir| {
        let package = dir.join("node_modules/@acme/hello");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{ "name": "@acme/hello", "version": "2.0.1", "type": "module", "main": "index.ts" }"#,
        )
        .unwrap();
        std::fs::write(package.join("index.ts"), sdk_plugin(BYSTANDER)).unwrap();
    })
    .await;
    let report = fixture
        .loader
        .reconcile(
            layer(vec![row("h", "@acme/hello"), row("m", "@acme/missing")]),
            None,
        )
        .await
        .unwrap();
    fixture.probe.wait_for("bystander: start").await;
    assert_eq!(
        fixture.loader.get("h").unwrap().meta["version"],
        json!("2.0.1")
    );
    match fixture.loader.get("m").unwrap().status {
        EntryStatus::Unresolved(LoaderError::NotFound { .. }) => {}
        other => panic!("a missing module is unresolved, got {other:?} ({report:?})"),
    }
    fixture.root.shutdown().await.unwrap();
}

/// A plugin that ends the process takes the runtime's rows down with it:
/// they wait for the runtime instead of holding a dead process, and run
/// again once the runtime restarts.
/// guarantee: loader/runtime-down-stops-its-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_process_stops_the_rows_until_restart() {
    let fixture = fixture().await;
    std::fs::write(
        fixture.dir.path().join("exit.ts"),
        "export function apply() { setTimeout(() => process.exit(3), 50) }\n",
    )
    .unwrap();
    let loader = &fixture.loader;
    let base = vec![row("p", "./provider.ts"), row("u", "./user.ts")];
    loader.reconcile(layer(base.clone()), None).await.unwrap();
    fixture.probe.wait_for("user: sunny (native)").await;

    let mut dying = base.clone();
    dying.push(row("x", "./exit.ts"));
    loader.reconcile(layer(dying), None).await.unwrap();
    COLD.until(
        || {
            ["p", "u"]
                .iter()
                .all(|id| state(loader, id) == Some(FiberState::Pending))
        },
        "rows waiting after the process ended",
    )
    .await;

    loader.reconcile(layer(base), None).await.unwrap();
    fixture.probe.0.lock().unwrap().clear();
    fixture.runtime.restart().await.unwrap();
    fixture.probe.wait_for("user: sunny (native)").await;
    COLD.until(
        || state(loader, "u") == Some(FiberState::Active),
        "rows running again",
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// A Bun runtime that cannot start (no runtime package here) fails, and
/// resolving its rows does not wait for it: they wait as rows.
/// guarantee: loader/runtime-start-failure-isolated
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_cannot_start_does_not_block_resolution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    std::fs::write(dir.path().join("bystander.ts"), sdk_plugin(BYSTANDER)).unwrap();
    let root = Ctx::root().unwrap();
    let bun = LocalRuntime::bun(dir.path().join("no-runtime-here"), dir.path());
    let resolver = Arc::new(RuntimeResolver::modules(bun.handle()));
    let runtime = root.plugin(bun);
    let _ = (&runtime).await;
    assert_eq!(runtime.state().state, FiberState::Failed);
    let plugin = LoaderPlugin::new(Chain::new().with_shared(resolver), LoaderOptions::default());
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let report = COLD
        .within(
            "resolution, which does not wait for a failed runtime",
            loader.reconcile(layer(vec![row("b", "./bystander.ts")]), None),
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(state(&loader, "b"), Some(FiberState::Pending));
    root.shutdown().await.unwrap();
}

/// An edit that breaks the plugin fails the reload, and the previous module
/// stays (`Loader::reload` is all or nothing): the row keeps running the old
/// code. Once the plugin is fixed, a reload runs the new one.
/// guarantee: loader/reload-all-or-nothing
#[tokio::test(flavor = "multi_thread")]
async fn a_broken_edit_keeps_the_previous_module_and_a_fix_reloads() {
    let fixture = fixture().await;
    let module = fixture.dir.path().join("edited.ts");
    let plugin = |version: &str| {
        sdk_plugin(&format!(
            "{{ inject: ['probe'], apply(ctx) {{ const probe = ctx.use('probe'); probe.record('{version}: start'); return () => probe.record('{version}: bye') }} }}"
        ))
    };
    let touch = |text: String, ahead: u64| {
        std::fs::write(&module, text).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&module)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(ahead))
            .unwrap();
    };
    touch(plugin("v1"), 0);
    fixture
        .loader
        .reconcile(layer(vec![row("e", "./edited.ts")]), None)
        .await
        .unwrap();
    fixture.probe.wait_for("v1: start").await;

    touch("export default {{ this is not TypeScript\n".into(), 5);
    let reload = fixture.loader.reload("e").await;
    assert!(
        reload.is_err(),
        "a broken plugin fails the reload: {reload:?}"
    );
    assert_eq!(
        state(&fixture.loader, "e"),
        Some(FiberState::Active),
        "the row keeps running"
    );
    assert!(
        fixture.probe.position("v1: bye").is_none(),
        "the previous module was not unloaded: {:?}",
        fixture.probe.0.lock().unwrap()
    );

    touch(plugin("v2"), 10);
    fixture.loader.reload("e").await.unwrap();
    fixture.probe.wait_for("v1: bye").await;
    fixture.probe.wait_for("v2: start").await;
    fixture.root.shutdown().await.unwrap();
}

/// A package linked into node_modules (`bun link`) reloads from its real
/// files: an edit there is what the reload runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_linked_package_reloads_from_its_real_files() {
    let source = tempfile::tempdir().unwrap();
    let plugin = |version: &str| {
        sdk_plugin(&format!(
            "{{ inject: ['probe'], apply(ctx) {{ ctx.use('probe').record('linked {version}') }} }}"
        ))
    };
    std::fs::write(
        source.path().join("package.json"),
        r#"{ "name": "@acme/linked", "version": "1.0.0", "type": "module", "main": "index.ts" }"#,
    )
    .unwrap();
    let index = source.path().join("index.ts");
    std::fs::write(&index, plugin("v1")).unwrap();
    let target = source.path().to_owned();
    let fixture = fixture_with(move |dir| {
        std::fs::create_dir_all(dir.join("node_modules/@acme")).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("node_modules/@acme/linked")).unwrap();
    })
    .await;
    fixture
        .loader
        .reconcile(layer(vec![row("l", "@acme/linked")]), None)
        .await
        .unwrap();
    fixture.probe.wait_for("linked v1").await;
    std::fs::write(&index, plugin("v2")).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&index)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    fixture.loader.reload("l").await.unwrap();
    fixture.probe.wait_for("linked v2").await;
    fixture.root.shutdown().await.unwrap();
}
