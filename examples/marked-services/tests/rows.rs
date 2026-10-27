//! TypeScript and Python rows using a Rust service marked
//! `#[rutis::service]`: provided as usual, called by name (#510).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use marked_services_example::Calendar;
use rutis::host::*;
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{decode_value, host_key, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
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
        let [line]: [String; 1] = decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "record": "sync" }))
    }
}

impl Probe {
    async fn wait_for(&self, line: &str) {
        let lines = self.0.clone();
        COLD.eventually_with(
            || {
                lines
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|known| known == line)
                    .then_some(())
            },
            &format!("{line:?} recorded"),
            || format!("{:?}", lines.lock().unwrap()),
        )
        .await
    }
}

/// The rows, with the TypeScript SDK they import and their declarations.
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
    _loader: Loader,
    probe: Probe,
    _rows: tempfile::TempDir,
}

async fn fixture(rows: Value) -> Fixture {
    let dir = self::rows();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    // The service, provided as usual: that is all it takes.
    root.provide(Calendar::new()).unwrap();

    let catalog = ServiceCatalog::new();
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let python = LocalRuntime::python(dir.path()).python_path(repo().join("python/rutis"));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    root.plugin(node);
    root.plugin(python);
    let plugin = LoaderPlugin::new(
        Chain::new()
            .with_shared(python_rows.clone())
            .with_shared(node_rows.clone()),
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
        _loader: loader,
        probe,
        _rows: dir,
    }
}

/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn a_typescript_row_calls_the_marked_service() {
    let fixture = fixture(json!([{ "id": "ts", "name": "ROWS/calendar.ts" }])).await;
    fixture
        .probe
        .wait_for("ts: Standup at 1760000000000, 1 Standup")
        .await;
    fixture
        .probe
        .wait_for("ts: CalendarError NotFound no entry 99")
        .await;
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn a_python_row_calls_the_same_service() {
    let fixture = fixture(json!([{ "id": "py", "name": "py:calendar_row" }])).await;
    fixture.probe.wait_for("py: Planning True").await;
    fixture
        .probe
        .wait_for("py: CalendarError NotFound {'NotFound': {'id': 99}}")
        .await;
    fixture.root.shutdown().await.unwrap();
}

/// The command that writes rows/services.d.ts, rows/services.py and
/// rows/services.contract.json, from the repository's root.
const TYPES: &str = "rutis-host types examples/marked-services -o examples/marked-services/rows/services.d.ts --python examples/marked-services/rows/services.py --contract examples/marked-services/rows/services.contract.json";

/// rows/services.d.ts is what `rutis-host types` writes for this crate,
/// and the rows written against it pass `tsc --strict`.
#[test]
fn the_declarations_are_current_and_pass_tsc_strict() {
    let written = rutis_bridge::build::Exports::new()
        .declarations(here())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(here().join("rows/services.d.ts")).unwrap(),
        written,
        "rows/services.d.ts is stale: run `{TYPES}`"
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

/// rows/services.contract.json, the contract description the testing
/// tools read, is what `rutis-host types --contract` writes for this crate.
#[test]
fn the_contract_description_is_current() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("services.contract.json");
    rutis_bridge::build::Exports::new()
        .contract(&file)
        .write(here())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(here().join("rows/services.contract.json")).unwrap(),
        std::fs::read_to_string(&file).unwrap(),
        "rows/services.contract.json is stale: run `{TYPES}`"
    );
}

/// rows/services.py is what `rutis-host types --python` writes for this
/// crate; it and the Python row written against it pass pyright in strict
/// mode (Python 3.10), and a call against the types is marked.
#[test]
fn the_python_types_are_current_and_pass_pyright_strict() {
    let dir = rows();
    let file = dir.path().join("written.py");
    rutis_bridge::build::Exports::new()
        .python(&file)
        .write(here())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(here().join("rows/services.py")).unwrap(),
        std::fs::read_to_string(&file).unwrap(),
        "rows/services.py is stale: run `{TYPES}`"
    );
    std::fs::remove_file(&file).unwrap();
    let pyright = |dir: &Path| {
        std::fs::write(
            dir.join("pyrightconfig.json"),
            json!({
                "typeCheckingMode": "strict",
                "pythonVersion": "3.10",
                "extraPaths": [repo().join("python/rutis")],
                "include": ["."],
                "exclude": ["node_modules"],
            })
            .to_string(),
        )
        .unwrap();
        std::process::Command::new("node")
            .arg(repo().join("node/rutis-runtime/node_modules/pyright/index.js"))
            .arg("-p")
            .arg(dir)
            .output()
            .expect("node is on PATH")
    };
    let output = pyright(dir.path());
    assert!(
        output.status.success(),
        "pyright --strict:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // A wrong argument is marked before the row runs.
    std::fs::write(
        dir.path().join("calendar_row.py"),
        std::fs::read_to_string(here().join("rows/calendar_row.py"))
            .unwrap()
            .replace("await calendar.get(99)", "await calendar.get(\"99\")"),
    )
    .unwrap();
    let output = pyright(dir.path());
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success() && said.contains("calendar_row.py") && said.contains("\"get\""),
        "pyright should mark calendar.get(\"99\"):\n{said}"
    );
}
