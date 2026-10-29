//! A host service sees the row that calls it (`session::caller_row`, #441):
//! two rows of one plugin in one runtime are two callers, an effect the
//! service registers on a caller's `Ctx` goes when that row unloads while
//! the runtime keeps running, and a call from Rust names no row. The same
//! in every runtime: Node, Python, Go, Bun and Swift.
#![cfg(feature = "runtimes")]

mod common;

use std::sync::{Arc, Mutex};

use common::runtimes::{layer, COLD};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{caller_row, host_key, native_error, HostDispatch, Reply, Value};
use rutis_loader::report::EntryStatus;
use rutis_loader::{Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::json;

/// A calling row: its id, key, fiber identity and scope.
type Caller = (String, String, String, Option<String>);

/// What `ledger` heard: who registered each name, and which
/// registrations went with their row.
#[derive(Default)]
struct Heard {
    /// `(name, (row id, row key, fiber identity, scope))`, `None` for no row.
    registered: Vec<(String, Option<Caller>)>,
    removed: Vec<String>,
}

/// The host service `ledger`: `register(name)` keeps `name` for the
/// calling row, until that row goes.
#[derive(Clone, Default)]
struct Ledger(Arc<Mutex<Heard>>);

impl HostDispatch for Ledger {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        assert_eq!(method, "register");
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        let row = caller_row();
        if let Some(row) = &row {
            let (heard, removed) = (self.0.clone(), name.clone());
            row.ctx()
                .effect(move || async move {
                    heard.lock().unwrap().removed.push(removed);
                    Ok(())
                })
                .map_err(native_error)?;
        }
        let row = row.map(|row| {
            (
                row.id().to_owned(),
                row.key().to_owned(),
                row.instance().to_string(),
                row.scope().map(str::to_owned),
            )
        });
        self.0.lock().unwrap().registered.push((name, row));
        Ok(Value::Undefined)
    }

    fn methods(&self) -> Option<serde_json::Value> {
        Some(json!({ "register": "sync" }))
    }
}

impl Ledger {
    fn registered(&self, name: &str) -> Option<Option<Caller>> {
        let heard = self.0.lock().unwrap();
        heard
            .registered
            .iter()
            .find(|(registered, _)| registered == name)
            .map(|(_, row)| row.clone())
    }

    fn removed(&self) -> Vec<String> {
        self.0.lock().unwrap().removed.clone()
    }
}

/// A root with `ledger`, and a loader whose chain `chain` builds from the
/// catalog (registering the runtime it needs on `root`).
async fn loader(
    ledger: &Ledger,
    chain: impl FnOnce(&Ctx, &ServiceCatalog) -> rutis_loader::Chain,
) -> (Ctx, Loader) {
    loader_on(Ctx::root().unwrap(), ledger, chain).await
}

/// [`loader`] on `root`.
async fn loader_on(
    root: Ctx,
    ledger: &Ledger,
    chain: impl FnOnce(&Ctx, &ServiceCatalog) -> rutis_loader::Chain,
) -> (Ctx, Loader) {
    root.provide_at(host_key("ledger"), Arc::new(ledger.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("ledger");
    let chain = chain(&root, &catalog);
    let plugin = LoaderPlugin::new(
        chain,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    (root, loader)
}

fn active(loader: &Loader, id: &str) -> Option<u64> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active => {
            Some(snapshot.generation)
        }
        _ => None,
    }
}

/// The suite, for rows of the plugin `name` in a runtime whose state
/// `running` reports.
async fn rows_are_callers(
    root: &Ctx,
    loader: &Loader,
    ledger: &Ledger,
    name: &str,
    running: impl Fn() -> bool,
) {
    let row = |id: &str| json!({ "id": id, "name": name, "config": { "name": id } });
    let report = loader
        .reconcile(layer(vec![row("a"), row("b")]), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    // A row registers while it starts: it runs a little later.
    let b_generation = COLD
        .eventually_with(
            || {
                (ledger.registered("a").is_some() && ledger.registered("b").is_some())
                    .then(|| active(loader, "b"))
                    .flatten()
            },
            "both rows registered, b running",
            || format!("{:?}", loader.entries()),
        )
        .await;

    // Two rows of one plugin in one runtime are two callers.
    let a = ledger.registered("a").unwrap().expect("a names its row");
    let b = ledger.registered("b").unwrap().expect("b names its row");
    assert_eq!((a.0.as_str(), b.0.as_str()), ("a", "b"));
    assert_ne!(a.1, b.1, "the rows' keys");
    assert_ne!(a.2, b.2, "the rows' fibers");
    assert_eq!((a.3, b.3), (None, None), "outside any scope");

    // Row a goes while the runtime keeps running: so does what it
    // registered, and only that.
    loader.reconcile(layer(vec![row("b")]), None).await.unwrap();
    COLD.eventually_with(
        || (ledger.removed() == ["a"]).then_some(()),
        "a's registration removed",
        || format!("{:?}", ledger.removed()),
    )
    .await;
    assert!(running(), "the runtime keeps running");
    assert_eq!(active(loader, "b"), Some(b_generation), "b runs on");
    assert_eq!(ledger.removed(), ["a"]);

    // A call from Rust names no row.
    let service = root
        .get_at::<dyn HostDispatch>(&host_key("ledger"))
        .unwrap();
    service.invoke("register", json!(["rust"]).into()).unwrap();
    assert_eq!(ledger.registered("rust"), Some(None));

    root.shutdown().await.unwrap();
    assert_eq!(ledger.removed(), ["a", "b"]);
}

/// A leaf plugin written with the SDK, imported from this checkout.
fn sdk_plugin() -> String {
    let sdk = common::runtimes::repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    // A file URL: an absolute Windows path is no module specifier.
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    let sdk = sdk.as_str();
    format!(
        "import {{ definePlugin }} from {sdk:?}
export default definePlugin({{
  inject: ['ledger'],
  apply(ctx, config) {{ ctx.use('ledger').register(config.name) }},
}})
"
    )
}

/// guarantee: bridge/caller-row
#[tokio::test(flavor = "multi_thread")]
async fn node_rows_are_callers() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    use common::runtimes::{greet_as_main, rows_stage, TestRuntime};
    use rutis_loader::runtime::SessionResolver;
    use rutis_loader::Chain;

    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("registrant.mjs");
    std::fs::write(&entry, sdk_plugin()).unwrap();
    // Cordis-style too: the service read from `ctx`, as Cordis plugins do.
    let cordis = dir.path().join("cordis.mjs");
    std::fs::write(
        &cordis,
        "export const inject = ['ledger']\nexport function apply(ctx, config) { ctx.ledger.register(config.name) }\n",
    )
    .unwrap();
    for entry in [entry, cordis] {
        let ledger = Ledger::default();
        let mut runtime = None;
        let mut resolver = None;
        let (root, loader) = loader(&ledger, |root, catalog| {
            greet_as_main(root);
            let node = TestRuntime::node(
                &common::runtimes::repo().join("node/rutis-runtime/package.json"),
            );
            let sessions = Arc::new(SessionResolver::npm(catalog, "node"));
            sessions.runtime(node.handle());
            runtime = Some(root.plugin(node));
            resolver = Some(sessions.clone());
            Chain::new().with_shared(sessions)
        })
        .await;
        let sessions = resolver.unwrap();
        sessions.attach(&loader);
        rows_stage(&root, &sessions, "node").await;
        let runtime = runtime.unwrap();
        rows_are_callers(&root, &loader, &ledger, &entry.to_string_lossy(), || {
            runtime.state().state == FiberState::Active
        })
        .await;
    }
}

/// guarantee: bridge/caller-row
#[tokio::test(flavor = "multi_thread")]
async fn python_rows_are_callers() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    use common::runtimes::{greet_as_main, rows_stage, TestRuntime};
    use rutis_loader::runtime::SessionResolver;
    use rutis_loader::Chain;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("registrant.py"),
        "inject = [\"ledger\"]\n\n\ndef apply(ctx, config):\n    ctx.use(\"ledger\").register(config[\"name\"])\n",
    )
    .unwrap();
    let ledger = Ledger::default();
    let mut runtime = None;
    let mut resolver = None;
    let (root, loader) = loader(&ledger, |root, catalog| {
        greet_as_main(root);
        let python = TestRuntime::python(dir.path());
        let sessions = Arc::new(SessionResolver::new(catalog));
        sessions.runtime(python.handle());
        runtime = Some(root.plugin(python));
        resolver = Some(sessions.clone());
        Chain::new().with_shared(sessions)
    })
    .await;
    let sessions = resolver.unwrap();
    sessions.attach(&loader);
    rows_stage(&root, &sessions, "py").await;
    let runtime = runtime.unwrap();
    rows_are_callers(&root, &loader, &ledger, "py:registrant", || {
        runtime.state().state == FiberState::Active
    })
    .await;
}

/// guarantee: bridge/caller-row
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn go_rows_are_callers() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    use rutis_loader::runtime::binary::{
        Binaries, BinaryResolver, BinaryRuntimeState, BinaryRuntimes,
    };
    use rutis_loader::runtime::go::Go;
    use rutis_loader::Chain;

    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::default();
    let mut resolver = None;
    let (root, loader) = loader(&ledger, |root, catalog| {
        common::runtimes::greet_as_main(root);
        let go = Arc::new(
            BinaryResolver::<Go>::new(Binaries::new().file(common::runtimes::go_binary()))
                .with_catalog(catalog),
        );
        resolver = Some(go.clone());
        Chain::new().with_shared(go)
    })
    .await;
    let runtimes =
        BinaryRuntimes::<Go>::new(resolver.unwrap(), dir.path(), common::runtimes::spawner())
            .idle(None);
    let go = runtimes.handle();
    root.plugin(runtimes).await.unwrap();
    rows_are_callers(&root, &loader, &ledger, "go:go_registrant", || {
        go.runtimes()
            .iter()
            .any(|runtime| runtime.state == BinaryRuntimeState::Running)
    })
    .await;
}

/// guarantee: bridge/caller-row
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn swift_rows_are_callers() {
    if !rutis_test_support::runtimes::enabled("swift") {
        return;
    }
    use rutis_loader::runtime::binary::{
        Binaries, BinaryResolver, BinaryRuntimeState, BinaryRuntimes,
    };
    use rutis_loader::runtime::swift::Swift;
    use rutis_loader::Chain;

    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::default();
    let mut resolver = None;
    let (root, loader) = loader(&ledger, |root, catalog| {
        common::runtimes::greet_as_main(root);
        let swift = Arc::new(
            BinaryResolver::<Swift>::new(Binaries::new().file(common::runtimes::swift_binary(
                "RutisMultilang",
                "multilang",
            )))
            .with_catalog(catalog),
        );
        resolver = Some(swift.clone());
        Chain::new().with_shared(swift)
    })
    .await;
    let runtimes =
        BinaryRuntimes::<Swift>::new(resolver.unwrap(), dir.path(), common::runtimes::spawner())
            .idle(None);
    let swift = runtimes.handle();
    root.plugin(runtimes).await.unwrap();
    rows_are_callers(&root, &loader, &ledger, "swift:swift_registrant", || {
        swift
            .runtimes()
            .iter()
            .any(|runtime| runtime.state == BinaryRuntimeState::Running)
    })
    .await;
}

/// guarantee: bridge/caller-row
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bun_rows_are_callers() {
    if !rutis_test_support::runtimes::enabled("bun") {
        return;
    }
    use common::runtimes::{greet_as_main, rows_stage, TestRuntime};
    use rutis_loader::runtime::SessionResolver;
    use rutis_loader::Chain;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    std::fs::write(dir.path().join("registrant.ts"), sdk_plugin()).unwrap();
    let ledger = Ledger::default();
    // Started first, as in bun_rows.rs.
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    let bun = TestRuntime::bun(dir.path());
    let handle = bun.handle();
    let runtime = root.plugin(bun);
    (&runtime).await.unwrap();
    let mut resolver = None;
    let (root, loader) = loader_on(root, &ledger, |_, catalog| {
        let sessions = Arc::new(SessionResolver::new(catalog));
        sessions.runtime(handle);
        resolver = Some(sessions.clone());
        Chain::new().with_shared(sessions)
    })
    .await;
    let sessions = resolver.unwrap();
    sessions.attach(&loader);
    rows_stage(&root, &sessions, "bun").await;
    rows_are_callers(&root, &loader, &ledger, "bun:./registrant.ts", || {
        runtime.state().state == FiberState::Active
    })
    .await;
}
