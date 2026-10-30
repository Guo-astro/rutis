//! Optional services of Go plugins (`Optional` beside `Inject`, read with
//! `ctx.Get`): never waited for; leased in when there as the row starts and
//! kept until it stops. A `#[rutis::service]` provider that goes, and a Go
//! row that stops while its process lives, say `service <name> is not
//! available`. A name both required and optional is the row's error.
#![cfg(feature = "go")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use common::runtimes::{greet_as_main, layer, row_state, spawner, Llm, COLD};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{call_service, host_key};
use rutis_loader::runtime::binary::{Binaries, BinaryResolver, BinaryRuntimes};
use rutis_loader::runtime::go::Go;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// The fixture binaries (`tests/fixtures/go/cmd/*`), built once.
fn built() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let out = tempfile::tempdir().unwrap().keep();
        let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go");
        for name in ["optional", "netkit"] {
            let status = std::process::Command::new("go")
                .args(["build", "-o"])
                .arg(out.join(exe(name)))
                .arg(format!("./cmd/{name}"))
                .current_dir(&module)
                .status()
                .expect("go is on PATH");
            assert!(status.success(), "go build {name}");
        }
        out
    })
}

fn exe(name: &str) -> String {
    match cfg!(windows) {
        true => format!("{name}.exe"),
        false => name.to_owned(),
    }
}

/// A `#[rutis::service]` type, served to the rows by name (`counter`).
#[derive(Default)]
pub struct Counter;

#[rutis::service]
impl Counter {
    pub fn count(&self) -> u32 {
        7
    }
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    for name in ["optional", "netkit"] {
        std::fs::copy(built().join(exe(name)), plugins.join(exe(name))).unwrap();
    }
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    let catalog = ServiceCatalog::new();
    let resolver =
        Arc::new(BinaryResolver::<Go>::new(Binaries::new().dir(&plugins)).with_catalog(&catalog));
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(resolver.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(BinaryRuntimes::<Go>::new(resolver, dir.path(), spawner()).idle(None))
        .await
        .unwrap();
    Fixture {
        root,
        loader,
        _dir: dir,
    }
}

impl Fixture {
    /// The row `id` of the Go plugin `plugin`, providing `of_<id>`; `round`
    /// in its config, so a new round restarts it.
    fn consumer(&self, id: &str, plugin: &str, round: u32) -> Value {
        json!({
            "id": id,
            "name": format!("go:{plugin}"),
            "config": { "as": format!("of_{id}"), "round": round },
        })
    }

    async fn active(&self, ids: &[&str]) {
        COLD.until(
            || {
                ids.iter()
                    .all(|id| row_state(&self.loader, id) == Some(FiberState::Active))
            },
            &format!("{ids:?} to run: {}", self.loader.status()),
        )
        .await;
    }

    /// What the row `id`'s `of()` answers, once its service is there.
    async fn of(&self, id: &str) -> String {
        let name = format!("of_{id}");
        COLD.until(
            || self.root.get_at(&host_key(&name)).is_some(),
            &format!("{name} to be provided"),
        )
        .await;
        call_service(&self.root, &name, "of", ()).await.unwrap()
    }
}

/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn go_optional_services_are_leased_at_start_and_never_waited_for() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let _llm = fixture
        .root
        .provide_at(host_key("llm"), Arc::new(Llm))
        .unwrap();
    let rows = |round| {
        vec![
            fixture.consumer("with", "uses_llm", 1),
            fixture.consumer("without", "uses_late", round),
        ]
    };
    let report = loader.reconcile(layer(rows(1)), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    // Not waited for: both run, there or not.
    fixture.active(&["with", "without"]).await;
    let status = loader.status().to_string();
    assert!(!status.contains("waiting"), "{status}");
    assert_eq!(fixture.of("with").await, "42");
    assert_eq!(fixture.of("without").await, "none");

    // One that comes later is not leased: sure to be seen once the row restarts.
    let late = fixture
        .root
        .provide_at(host_key("late"), Arc::new(Llm))
        .unwrap();
    assert_eq!(fixture.of("without").await, "none");
    loader.reconcile(layer(rows(2)), None).await.unwrap();
    poll(
        || async { fixture.of("without").await == "42" },
        "the restarted row to see late",
    )
    .await;

    late.dispose().await.unwrap();
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn go_a_marked_service_that_goes_is_not_available() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let counter = fixture.root.provide(Counter).unwrap();
    loader
        .reconcile(layer(vec![fixture.consumer("c", "uses_counter", 1)]), None)
        .await
        .unwrap();
    fixture.active(&["c"]).await;
    assert_eq!(fixture.of("c").await, "7");

    counter.dispose().await.unwrap();
    assert_eq!(row_state(loader, "c"), Some(FiberState::Active));
    assert_eq!(
        fixture.of("c").await,
        "unavailable: service counter is not available"
    );
    fixture.root.shutdown().await.unwrap();
}

/// A Go row providing the service stops while its process lives: the
/// handle the consumer leased is retired there, and says so.
/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn go_another_go_row_that_stops_is_not_available() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    let provider = json!({ "id": "p", "name": "go:ping", "config": {} });
    loader
        .reconcile(layer(vec![provider.clone()]), None)
        .await
        .unwrap();
    fixture.active(&["p"]).await;
    let consumer = fixture.consumer("c", "uses_ping", 1);
    loader
        .reconcile(layer(vec![provider, consumer.clone()]), None)
        .await
        .unwrap();
    fixture.active(&["c"]).await;
    assert_eq!(fixture.of("c").await, "netkit pong ");

    // Its process stays: runtimes here never stop when idle.
    loader.reconcile(layer(vec![consumer]), None).await.unwrap();
    COLD.until(|| loader.get("p").is_none(), "the provider's row to go")
        .await;
    assert_eq!(row_state(loader, "c"), Some(FiberState::Active));
    poll(
        || async { fixture.of("c").await == "unavailable: service ping is not available" },
        "the call to say ping is gone",
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/optional-deps
#[tokio::test(flavor = "multi_thread")]
async fn go_a_name_both_required_and_optional_fails_the_row() {
    let fixture = fixture().await;
    let loader = &fixture.loader;
    loader
        .reconcile(
            layer(vec![
                json!({ "id": "both", "name": "go:both", "config": {} }),
            ]),
            None,
        )
        .await
        .unwrap();
    COLD.until(
        || {
            loader.status().to_string().contains(
                "both: unresolved: cannot load \"go:both\": llm is in both inject and optional; keep it in one",
            )
        },
        &format!("the row to fail: {}", loader.status()),
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// Wait until `check` answers true.
async fn poll<F: std::future::Future<Output = bool>>(mut check: impl FnMut() -> F, what: &str) {
    let deadline = tokio::time::Instant::now() + COLD.limit();
    while !check().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
