//! TypeScript and Python rows using Rust services from a dependency crate
//! by name, through the application's own types wrapping them, marked
//! `#[rutis::service]`: no `HostDispatch` written by the application.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rust_services_calendar::{Change, Entry, Kind};
use rust_services_example::{Calendar, Notes};
use rutis::host::*;
use rutis::{Ctx, Plugin};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::resolve::Build;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{
    Builtins, Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog,
};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

/// Language runtimes start cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 3));

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The host service `probe`: rows `record` lines into it.
#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<String>>>);

impl HostDispatch for Probe {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        assert_eq!(method, "record");
        let [line]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "record": "sync" }))
    }
}

impl Probe {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    fn has(&self, line: &str) -> bool {
        self.lines().iter().any(|known| known == line)
    }

    async fn wait_for(&self, line: &str) {
        COLD.eventually_with(
            || self.has(line).then_some(()),
            &format!("{line:?} recorded"),
            || format!("{:?}", self.lines()),
        )
        .await
    }
}

/// The instance's title, given when it is created.
struct Title(String);

#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
struct NoConfig {}

/// The Rust row providing a `session` instance's own `Notes`, as an
/// application does: at its type's key in the instance.
struct NotesRow {
    key: Key<Notes>,
    owner: String,
}

impl NotesRow {
    /// One copy per `session` instance, titled as the instance was.
    fn new(build: &Build, _: &NoConfig) -> rutis::Result<Self> {
        Ok(NotesRow {
            key: Key::of().instance(build.instance("session")?),
            owner: build
                .value::<Title>()
                .map(|title| title.0.clone())
                .unwrap_or_default(),
        })
    }
}

impl Plugin for NotesRow {
    type Deps = ();

    fn name(&self) -> &str {
        "notes"
    }

    async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
        let notes = rust_services_calendar::Notes::new(self.owner.clone());
        ctx.provide_at(self.key.clone(), Arc::new(Notes(notes)))?;
        Ok(())
    }
}

/// The rows, as an application ships them: copied with the TypeScript SDK
/// they import (`@arcships/rutis`) and the generated declarations.
fn rows() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(here().join("rows")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let sdk = dir.path().join("node_modules/@arcships/rutis");
    std::fs::create_dir_all(sdk.join("src")).unwrap();
    std::fs::copy(
        repo().join("node/rutis/package.json"),
        sdk.join("package.json"),
    )
    .unwrap();
    for entry in std::fs::read_dir(repo().join("node/rutis/src")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), sdk.join("src").join(entry.file_name())).unwrap();
    }
    dir
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    calendar: Arc<Calendar>,
    _rows: tempfile::TempDir,
}

async fn fixture(rows: Value) -> Fixture {
    let dir = self::rows();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    // The wrapped service, provided as usual: that serves it by name.
    let calendar = Arc::new(Calendar(rust_services_calendar::Calendar::new()));
    root.provide_as::<Calendar>(calendar.clone()).unwrap();

    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe");

    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let python = LocalRuntime::python(dir.path()).python_path(repo().join("python/rutis"));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    root.plugin(node);
    root.plugin(python);
    let mut builtins = Builtins::new();
    builtins.register_with("notes", NotesRow::new);
    let chain = Chain::new()
        .with(builtins)
        .with_shared(python_rows.clone())
        .with_shared(node_rows.clone());
    let plugin = LoaderPlugin::new(
        chain,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(node_rows));
    root.plugin(RuntimeRowsPlugin::new(python_rows));

    let rows = serde_json::to_string(&rows).unwrap().replace(
        "ROWS/",
        &format!("{}/", dir.path().to_string_lossy().replace('\\', "/")),
    );
    let patches: Vec<Patch> = serde_json::from_value(
        json!([{ "insert": serde_json::from_str::<Value>(&rows).unwrap() }]),
    )
    .unwrap();
    let report = loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    Fixture {
        root,
        loader,
        probe,
        calendar,
        _rows: dir,
    }
}

/// A Node row injects the Rust `Calendar` and calls it: serde structs in
/// and out, an async method, `Err(e)` thrown with its code, message and
/// data, optional and `&str` parameters, a bigint beyond 2^53, a function
/// passed by reference.
/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn a_node_row_calls_a_rust_service_from_a_dependency_crate() {
    let fixture = fixture(json!([{ "id": "calendar", "name": "ROWS/calendar.ts" }])).await;
    for line in [
        "added Standup meeting team",
        "got Standup at 1760000000000",
        r#"missing CalendarError NotFound no entry 99999 {"NotFound":{"id":99999}}"#,
        "refused Invalid invalid entry: an entry needs a title",
        "listed true",
        "tagged true",
        "renamed Retro",
        "last bigint 18446744073709551615",
        "visited true",
        "subscribed",
    ] {
        fixture.probe.wait_for(line).await;
    }
    fixture.root.shutdown().await.unwrap();
}

/// The row's handler is called for the calendar's changes: `publish` is
/// done only once the handler's Promise settled, and fails when the handler
/// throws.
/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn a_node_rows_handler_settles_before_the_native_side_goes_on() {
    let fixture = fixture(json!([{ "id": "calendar", "name": "ROWS/calendar.ts" }])).await;
    fixture.probe.wait_for("subscribed").await;
    let entry = Entry {
        id: 7,
        title: "Lunch".into(),
        starts_at: 0,
        tags: Vec::new(),
        kind: Kind::Reminder,
        note: None,
    };
    rutis_test_support::within("publish", fixture.calendar.publish(Change::Added { entry }))
        .await
        .unwrap();
    // Recorded after the handler's own 50 ms wait, before publish returned.
    assert!(
        fixture.probe.has("handled added Lunch"),
        "{:?}",
        fixture.probe.lines()
    );
    let failed = rutis_test_support::within(
        "publish",
        fixture.calendar.publish(Change::Removed { id: 13 }),
    )
    .await
    .unwrap_err();
    assert!(failed.to_string().contains("handler refused"), "{failed}");
    fixture.root.shutdown().await.unwrap();
}

/// `subscribe` takes the calling row's `Ctx` (hidden from TypeScript): the
/// subscription is that row's, and goes when the row unloads while the
/// runtime keeps running. Called by name from Rust, outside any row, it is
/// refused.
/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_goes_when_its_row_unloads() {
    let fixture = fixture(json!([{ "id": "calendar", "name": "ROWS/calendar.ts" }])).await;
    fixture.probe.wait_for("subscribed").await;
    assert_eq!(fixture.calendar.subscribers(), 1);
    let report = fixture
        .loader
        .reconcile(vec![Layer::new("rows", Vec::new())], None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    COLD.until(
        || fixture.calendar.subscribers() == 0,
        "the row's subscription removed",
    )
    .await;
    let by_name = fixture
        .root
        .get_at::<dyn HostDispatch>(&host_key("calendar"))
        .unwrap();
    let refused = by_name
        .invoke("subscribe", RpcValue::List(Vec::new()))
        .unwrap_err();
    assert!(
        refused.to_string().contains("calling row's context"),
        "{refused}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// A service provided inside `session` instances: each instance's row gets
/// its own instance's `Notes`.
/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn each_instance_rows_get_their_own_instances_service() {
    let fixture = fixture(json!([
        { "id": "session", "group": true, "instanced": true, "config": [
            { "id": "notes", "name": "notes" },
            { "id": "notes-ts", "name": "ROWS/notes.ts" }
        ] }
    ]))
    .await;
    for title in ["A", "B"] {
        fixture
            .loader
            .create_instance(&fixture.root, "session")
            .with(Title(title.into()))
            .await
            .unwrap();
    }
    fixture.probe.wait_for("notes of A: 1 hello from A").await;
    fixture.probe.wait_for("notes of B: 1 hello from B").await;
    fixture.root.shutdown().await.unwrap();
}

/// A Python row calls the same service through the same dispatch.
/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn a_python_row_calls_the_same_service() {
    let fixture = fixture(json!([{ "id": "py", "name": "py:calendar_row" }])).await;
    fixture.probe.wait_for("py got Planning all_day True").await;
    fixture
        .probe
        .wait_for("py missing CalendarError NotFound no entry 99999 {'NotFound': {'id': 99999}}")
        .await;
    fixture.root.shutdown().await.unwrap();
}

/// The generated declarations, and the rows written against them, pass
/// `tsc --strict`; the copy in rows/ is what the build writes.
/// guarantee: bridge/marked-services
#[test]
fn the_declarations_pass_tsc_strict() {
    let written = rutis_bridge::build::Exports::new()
        .declarations(here())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(here().join("rows/services.d.ts")).unwrap(),
        written,
        "rows/services.d.ts is not what the build writes: build again and commit it"
    );
    let dir = rows();
    std::fs::write(
        dir.path().join("tsconfig.json"),
        json!({
            "compilerOptions": {
                "strict": true,
                "noEmit": true,
                "target": "es2022",
                "module": "nodenext",
                "moduleResolution": "nodenext",
                "allowImportingTsExtensions": true,
                "types": [],
                "lib": ["es2022", "dom"]
            },
            "include": ["*.ts"]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(dir.path().join("package.json"), r#"{ "type": "module" }"#).unwrap();
    let tsc = repo().join("node/rutis-runtime/node_modules/typescript/bin/tsc");
    let output = std::process::Command::new("node")
        .arg(&tsc)
        .arg("-p")
        .arg(dir.path())
        .output()
        .expect("node is on PATH");
    assert!(
        output.status.success(),
        "tsc --strict:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
