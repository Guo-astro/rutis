//! What the tests of language runtime rows share: the probe plugins record
//! lines into, the checkout, the rows as a layer, a row's state, and the
//! helpers of the instance tests.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rutis::FiberState;
use rutis_bridge::session::{HostDispatch, Reply, Value as RpcValue};
use rutis_bridge::PeerId;
use rutis_loader::{Build, EntryInfo, EntryStatus, Layer, Loader, Patch};
use rutis_test_support::{Guard, HANG_GUARD};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// Twice HANG_GUARD: language runtimes start cold (#238).
pub const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// This checkout.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// The host service `probe`: plugins `record` lines into it.
#[derive(Clone, Default)]
pub struct Probe(pub Arc<Mutex<Vec<String>>>);

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
    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    pub fn has(&self, line: &str) -> bool {
        self.0.lock().unwrap().iter().any(|l| l == line)
    }

    pub fn position(&self, line: &str) -> Option<usize> {
        self.0.lock().unwrap().iter().position(|l| l == line)
    }

    /// The lines recorded so far, forgotten.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    /// Wait until `line` is recorded.
    pub async fn wait_for(&self, line: &str) {
        COLD.eventually_with(
            || self.has(line).then_some(()),
            &format!("{line:?} recorded"),
            || format!("{:?}", self.lines()),
        )
        .await
    }
}

/// The host service `llm`: answers `ask` with 42.
pub struct Llm;

impl HostDispatch for Llm {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "ask");
        Ok(json!("42").into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "ask": "sync" }))
    }
}

/// `rows` inserted, as the one layer.
pub fn layer(rows: Vec<Value>) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

/// The state of the row `id`'s fiber, when it has one.
pub fn row_state(loader: &Loader, id: &str) -> Option<FiberState> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

/// The Go plugins' binary (`tests/fixtures/go`, `./cmd/multilang`), built
/// once.
pub fn go_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let out = tempfile::tempdir().unwrap().keep();
        let binary = out.join(if cfg!(windows) {
            "multilang.exe"
        } else {
            "multilang"
        });
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./cmd/multilang")
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build");
        binary
    })
}

/// The product `product` of the Swift package (the repository's root
/// `Package.swift`: `RutisMultilang`, `RutisPing`, `RutisConformance`),
/// built with the `swift` on PATH, copied once into a directory of its own
/// as `name`.
#[allow(dead_code)]
pub fn swift_binary(product: &str, name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let bin = BUILT.get_or_init(|| {
        let status = std::process::Command::new("swift")
            .args(["build"])
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build");
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(repo())
            .output()
            .expect("swift is on PATH");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    });
    let out = tempfile::tempdir().unwrap().keep().join(name);
    std::fs::copy(bin.join(product), &out).unwrap();
    out
}

// ── Instances ───────────────────────────────────────────────────

/// The instance's `tools`: tells its instance's title, and remembers the
/// rows that ask ([`tool_callers`]).
pub struct Tools(pub String);

/// `(title, row id, scope)` of every row that asked a `tools` its title.
static TOOL_CALLERS: Mutex<Vec<(String, String, Option<String>)>> = Mutex::new(Vec::new());

/// The rows that asked the `tools` titled `title`, as `(row id, scope)`.
#[allow(dead_code)]
pub fn tool_callers(title: &str) -> Vec<(String, Option<String>)> {
    TOOL_CALLERS
        .lock()
        .unwrap()
        .iter()
        .filter(|(asked, _, _)| asked == title)
        .map(|(_, id, scope)| (id.clone(), scope.clone()))
        .collect()
}

impl HostDispatch for Tools {
    fn invoke(&self, method: &str, _: RpcValue) -> Reply {
        assert_eq!(method, "title");
        if let Some(row) = rutis_bridge::session::caller_row() {
            let scope = row.scope().map(str::to_owned);
            TOOL_CALLERS
                .lock()
                .unwrap()
                .push((self.0.clone(), row.id().to_owned(), scope));
        }
        Ok(json!(self.0).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "title": "sync" }))
    }
}

/// The instance's title, given with `with`.
pub struct Title(pub String);

/// The title `build`'s instance was given.
pub fn title(build: &Build) -> String {
    build
        .value::<Title>()
        .map(|t| t.0.clone())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct NoConfig {}

pub fn active(entry: &EntryInfo) -> bool {
    matches!(&entry.status, EntryStatus::Running(s) if s.state == FiberState::Active)
}
