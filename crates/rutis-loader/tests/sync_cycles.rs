//! Synchronous calls between runtimes that would wait on each other for
//! ever: the host refuses the call that closes the cycle with
//! `SyncWaitCycle`, and everything else completes (#228; multi-language
//! design §9, policy B; `specs/cross-runtime-sync`). A gate, a host service
//! the test opens, fixes the order of the calls: no sleeps.
#![cfg(all(feature = "node", feature = "python"))]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};

use common::runtimes::{repo, rows_stage, COLD};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{Reply, Value as RpcValue};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::RuntimeResolver;
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis_test_support::recv_within;
use serde_json::{json, Value};

// Every wait has a hang guard (Q7.1): a cycle the host misses
// hangs. The gate and each synchronous forward hold a worker of the test's
// runtime while they wait; the tests use 8, twice what they hold at most.

// ── Two Node runtimes calling each other ────────────────────────
// Services of different rows, so no two rows wait for each other's.

/// What each runtime does for the other: `work`, and `echo`.
const NODE_WORK: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: [INJECT],
  provides: { SELF: { work: 'sync', echo: 'sync', relay: 'sync', object: 'sync' } },
  apply(ctx) {
    ctx.provide('SELF', {
      work: () => 'SELF works',
      echo: () => 'SELF echoes',
      relay: () => `SELF relays ${ctx.use('a_work').echo()}`,
      // An object whose method works, passed by reference (#687).
      object: () => ({ work: () => 'SELF works' }),
    })
  },
})
"#;

/// `go` waits at the gate until the other runtime's `go` arrives too, then
/// calls the other runtime's `work` synchronously; `chain` calls the other
/// runtime, which calls back here on the same chain.
const NODE_GO_PEER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['gate', 'OTHER'],
  provides: { SELF: { go: 'sync', chain: 'sync' } },
  apply(ctx) {
    const gate = ctx.use('gate')
    const other = () => ctx.use('OTHER')
    ctx.provide('SELF', {
      go() {
        gate.arrive('SELF')
        try { return `SELF got ${other().work()}` } catch (error) { return `SELF: ${error.name}` }
      },
      chain: () => `SELF ← ${other().relay()}`,
    })
  },
})
"#;

// ── One Node runtime and two Python runtimes (counterexample 2) ──

/// In Node: `go` calls `s1` in the second Python runtime.
const NODE_GO: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['p2_svc'],
  provides: { n_svc: { go: 'sync' } },
  apply(ctx) {
    ctx.provide('n_svc', { go: () => `n got ${ctx.use('p2_svc').s1()}` })
  },
})
"#;

/// In Node: what the second Python runtime calls back while Node waits.
const NODE_BACK: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { n_back: { back: 'sync' } },
  apply(ctx) {
    ctx.provide('n_back', { back: () => 'n is back' })
  },
})
"#;

/// The first Python runtime: `slow` holds at the gate; `kick`, called while
/// `slow` holds, calls `s2` in the second Python runtime.
const PY_ONE: &str = r#"
inject = ["gate", "p2_back"]


class P1:
    def __init__(self, ctx=None):
        self.ctx = ctx

    def slow(self):
        self.ctx.use("gate").arrive("slow")
        return "p1 slow"

    def kick(self):
        return f"p1 kicked {self.ctx.use('p2_back').s2()}"


provides = {"p1_svc": P1}


def apply(ctx, config):
    ctx.provide("p1_svc", P1(ctx))
"#;

/// The second Python runtime: `s1` calls `slow` in the first.
const PY_TWO: &str = r#"
inject = ["p1_svc"]


class P2:
    def __init__(self, ctx=None):
        self.ctx = ctx

    def s1(self):
        return f"s1 after {self.ctx.use('p1_svc').slow()}"


provides = {"p2_svc": P2}


def apply(ctx, config):
    ctx.provide("p2_svc", P2(ctx))
"#;

/// Also in the second Python runtime: `s2`, run nested above `s1`, calls
/// Node back.
const PY_TWO_BACK: &str = r#"
from rutis.advanced import SyncWaitCycle

inject = ["n_back"]


class Back:
    def __init__(self, ctx=None):
        self.ctx = ctx

    def s2(self):
        try:
            return f"s2 got {self.ctx.use('n_back').back()}"
        except SyncWaitCycle as error:
            return f"s2: SyncWaitCycle: {error}"


provides = {"p2_back": Back}


def apply(ctx, config):
    ctx.provide("p2_back", Back(ctx))
"#;

// ── Harness ─────────────────────────────────────────────────────

/// A gate: `arrive(name)` reports the arrival and blocks until the test
/// opens the gate for `name`, or, for `parties`, until all of them arrived.
#[derive(Clone)]
struct Gate {
    state: Arc<(Mutex<GateState>, Condvar)>,
    arrived: Arc<Mutex<mpsc::Sender<String>>>,
}

#[derive(Default)]
struct GateState {
    open: Vec<String>,
    waiting: Vec<String>,
    parties: usize,
}

impl Gate {
    fn new(parties: usize) -> (Self, mpsc::Receiver<String>) {
        let (arrived, arrivals) = mpsc::channel();
        let gate = Self {
            state: Arc::new((
                Mutex::new(GateState {
                    parties,
                    ..GateState::default()
                }),
                Condvar::new(),
            )),
            arrived: Arc::new(Mutex::new(arrived)),
        };
        (gate, arrivals)
    }

    fn open(&self, name: &str) {
        let (state, changed) = &*self.state;
        state.lock().unwrap().open.push(name.to_owned());
        changed.notify_all();
    }
}

impl HostDispatch for Gate {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        assert_eq!(method, "arrive");
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        let _ = self.arrived.lock().unwrap().send(name.clone());
        let (state, changed) = &*self.state;
        let mut state = state.lock().unwrap();
        state.waiting.push(name.clone());
        changed.notify_all();
        let (state, timeout) = changed
            .wait_timeout_while(state, COLD.limit(), |state| {
                !state.open.contains(&name)
                    && !(state.parties > 0 && state.waiting.len() >= state.parties)
            })
            .unwrap();
        assert!(!timeout.timed_out(), "the gate for {name} never opened");
        drop(state);
        Ok(RpcValue::Undefined)
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "arrive": "sync" }))
    }
}

fn plugin_url() -> String {
    let plugin = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    url::Url::from_file_path(&plugin).unwrap().to_string()
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text.replace("PLUGIN", &plugin_url())).unwrap();
    path
}

/// `rows` in one layer.
fn rows(rows: Vec<Value>) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

fn active(loader: &Loader, id: &str) -> bool {
    matches!(
        loader.get(id).map(|entry| entry.status),
        Some(EntryStatus::Running(snapshot)) if snapshot.state == FiberState::Active
    )
}

fn service(root: &Ctx, name: &str) -> Option<Arc<dyn HostDispatch>> {
    root.get_at::<dyn HostDispatch>(&host_key(name))
}

/// Call `method` of `name` synchronously, on a thread of its own, as an
/// application would.
fn call(root: &Ctx, name: &str, method: &str) -> mpsc::Receiver<Result<Value, String>> {
    let service = service(root, name).unwrap_or_else(|| panic!("no service {name}"));
    let method = method.to_owned();
    let (done, result) = mpsc::channel();
    std::thread::spawn(move || {
        let reply = service
            .invoke(&method, RpcValue::List(Vec::new()))
            .map_err(|error| error.to_string())
            .and_then(|value| value.json().map_err(|error| error.to_string()));
        let _ = done.send(reply);
    });
    result
}

fn answer(result: &mpsc::Receiver<Result<Value, String>>, what: &str) -> Value {
    result
        .recv_timeout(COLD.limit())
        .unwrap_or_else(|_| panic!("{what} hangs: the runtimes wait on each other"))
        .unwrap_or_else(|error| panic!("{what} failed: {error}"))
}

/// Runtimes, one loader over them, and the gate.
struct Fixture {
    root: Ctx,
    loader: Loader,
    _dir: tempfile::TempDir,
}

/// The Go plugins' binary (`tests/fixtures/go/cmd/relay`), built once.
#[cfg(feature = "go")]
fn go_binary() -> &'static Path {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let out = tempfile::tempdir().unwrap().keep();
        let binary = out.join(if cfg!(windows) { "relay.exe" } else { "relay" });
        let status = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&binary)
            .arg("./cmd/relay")
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go"))
            .status()
            .expect("go is on PATH");
        assert!(status.success(), "go build");
        binary
    })
}

/// `go`: also a Go runtime, for `go:` rows; `swift`: a Swift one, for
/// `swift:` rows.
async fn fixture(
    runtimes: Vec<(LocalRuntime, bool)>,
    go: bool,
    swift: bool,
    services: &[&str],
    gate: Gate,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("gate"), Arc::new(gate)).unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in std::iter::once(&"gate").chain(services) {
        catalog.register_shared(*name);
    }
    let mut chain = Chain::new();
    #[cfg(not(feature = "go"))]
    assert!(!go, "Go rows need the go feature");
    #[cfg(feature = "go")]
    let go = go.then(|| {
        Arc::new(
            rutis_loader::runtime::go::GoResolver::new(
                rutis_loader::runtime::go::GoBinaries::new().file(go_binary()),
            )
            .with_catalog(&catalog),
        )
    });
    #[cfg(feature = "go")]
    if let Some(go) = &go {
        chain = chain.with_shared(go.clone());
    }
    #[cfg(not(all(feature = "swift", target_os = "macos")))]
    assert!(!swift, "Swift rows need the swift feature, on macOS");
    #[cfg(all(feature = "swift", target_os = "macos"))]
    let swift = swift.then(|| {
        Arc::new(
            rutis_loader::runtime::swift::SwiftResolver::new(
                rutis_loader::runtime::swift::SwiftBinaries::new()
                    .file(common::runtimes::swift_binary("RutisMultilang", "relay")),
            )
            .with_catalog(&catalog),
        )
    });
    #[cfg(all(feature = "swift", target_os = "macos"))]
    if let Some(swift) = &swift {
        chain = chain.with_shared(swift.clone());
    }
    let mut resolvers = Vec::new();
    for (runtime, npm) in runtimes {
        let resolver = Arc::new(
            match npm {
                true => RuntimeResolver::node(runtime.handle()),
                false => RuntimeResolver::modules(runtime.handle()),
            }
            .with_catalog(&catalog),
        );
        root.plugin(runtime);
        chain = chain.with_shared(resolver.clone());
        resolvers.push(resolver);
    }
    let plugin = LoaderPlugin::new(
        chain,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    for resolver in resolvers {
        rows_stage(&root, resolver).await;
    }
    #[cfg(feature = "go")]
    if let Some(go) = go {
        root.plugin(rutis_loader::runtime::go::GoRuntimes::new(go, dir.path()).idle(None))
            .await
            .unwrap();
    }
    #[cfg(all(feature = "swift", target_os = "macos"))]
    if let Some(swift) = swift {
        root.plugin(rutis_loader::runtime::swift::SwiftRuntimes::new(swift, dir.path()).idle(None))
            .await
            .unwrap();
    }
    Fixture {
        root,
        loader,
        _dir: dir,
    }
}

fn node(name: &str) -> LocalRuntime {
    LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    )
    .named(name)
}

/// A Bun runtime whose rows are the `.ts` files of `dir`; the Bun A is
/// written there too.
#[allow(unused_variables)]
fn bun(name: &str, dir: &Path) -> LocalRuntime {
    #[cfg(feature = "bun")]
    {
        let bun = dir.join("bun");
        std::fs::create_dir_all(&bun).unwrap();
        std::fs::write(bun.join("package.json"), "{}").unwrap();
        std::fs::write(bun.join("a.ts"), BUN_A).unwrap();
        LocalRuntime::bun(repo().join("bun/rutis-bun"), &bun).named(name)
    }
    #[cfg(not(feature = "bun"))]
    unreachable!("Bun rows need the bun feature")
}

fn python(name: &str, dir: &Path) -> LocalRuntime {
    LocalRuntime::python(dir)
        .python_path(repo().join("python/rutis"))
        .named(name)
}

// ── The cases ───────────────────────────────────────────────────

/// Counterexample 1: each of two Node runtimes calls the other while the
/// other waits. Without the check both wait for ever; with it the second
/// call to be forwarded gets `SyncWaitCycle`, and once its runtime returns,
/// the first completes. Calls back on the caller's own chain are not
/// refused.
/// risk: P10, C6
/// guarantee: bridge/sync-wait-cycle-refused
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_node_runtimes_calling_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let work = |me: &str, inject: &str| NODE_WORK.replace("SELF", me).replace("INJECT", inject);
    let go = |me: &str, other: &str| NODE_GO_PEER.replace("SELF", me).replace("OTHER", other);
    let files = [
        (
            "aw",
            "node",
            write(dir.path(), "aw.mjs", &work("a_work", "")),
        ),
        (
            "bw",
            "nb",
            write(dir.path(), "bw.mjs", &work("b_work", "'a_work'")),
        ),
        (
            "a",
            "node",
            write(dir.path(), "a.mjs", &go("a_svc", "b_work")),
        ),
        (
            "b",
            "nb",
            write(dir.path(), "b.mjs", &go("b_svc", "a_work")),
        ),
    ];
    let (gate, _arrivals) = Gate::new(2);
    let fixture = fixture(
        vec![(node("node"), true), (node("nb"), false)],
        false,
        false,
        &["a_work", "b_work", "a_svc", "b_svc"],
        gate,
    )
    .await;
    let rows_of = files
        .iter()
        .map(|(id, runtime, file)| match *runtime {
            "node" => json!({ "id": id, "name": file.to_string_lossy() }),
            runtime => json!({ "id": id, "name": format!("{runtime}:{}", file.display()) }),
        })
        .collect();
    fixture.loader.reconcile(rows(rows_of), None).await.unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["aw", "bw", "a", "b"].iter().all(|id| active(&loader, id))
                && ["a_work", "b_work", "a_svc", "b_svc"]
                    .iter()
                    .all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;

    // No false positive: a nested call back on the caller's chain
    // (a → b → a, both runtimes waiting).
    assert_eq!(
        answer(&call(&fixture.root, "a_svc", "chain"), "a → b → a"),
        json!("a_svc ← b_work relays a_work echoes")
    );

    // Both wait at the gate until both arrived: then each calls the other.
    let a_go = call(&fixture.root, "a_svc", "go");
    let b_go = call(&fixture.root, "b_svc", "go");
    let mut answers = [answer(&a_go, "a.go"), answer(&b_go, "b.go")]
        .map(|answer| answer.as_str().unwrap().to_owned());
    answers.sort();
    let refused: Vec<&String> = answers
        .iter()
        .filter(|answer| answer.ends_with(": SyncWaitCycle"))
        .collect();
    assert_eq!(refused.len(), 1, "exactly one call is refused: {answers:?}");
    assert!(
        answers.contains(&"a_svc got b_work works".to_owned())
            || answers.contains(&"b_svc got a_work works".to_owned()),
        "the other completes: {answers:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// Counterexample 2: one Node and two Python runtimes. Node calls `s1` in
/// p2, which calls `slow` in p1; while `slow` holds, p1 runs `kick`, which
/// calls `s2` in p2, nested above `s1`; `s2` calls Node back, which waits
/// for `s1` and puts the call aside. `s1` is buried under `s2`: without the
/// stack-order edges the cycle goes unnoticed. The host refuses `s2`'s call
/// back, and everything else completes.
/// risk: P10, C6
/// guarantee: bridge/sync-wait-cycle-refused
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_node_and_two_python_runtimes() {
    let dir = tempfile::tempdir().unwrap();
    let go = write(dir.path(), "go.mjs", NODE_GO);
    let back = write(dir.path(), "back.mjs", NODE_BACK);
    std::fs::write(dir.path().join("p_one.py"), PY_ONE).unwrap();
    std::fs::write(dir.path().join("p_two.py"), PY_TWO).unwrap();
    std::fs::write(dir.path().join("p_two_back.py"), PY_TWO_BACK).unwrap();
    let (gate, arrivals) = Gate::new(0);
    let opener = gate.clone();
    let services = ["n_svc", "n_back", "p1_svc", "p2_svc", "p2_back"];
    let fixture = fixture(
        vec![
            (node("node"), true),
            (python("py", dir.path()), false),
            (python("pb", dir.path()), false),
        ],
        false,
        false,
        &services,
        gate,
    )
    .await;
    fixture
        .loader
        .reconcile(
            rows(vec![
                json!({ "id": "n", "name": go.to_string_lossy() }),
                json!({ "id": "nb", "name": back.to_string_lossy() }),
                json!({ "id": "p1", "name": "py:p_one" }),
                json!({ "id": "p2", "name": "pb:p_two" }),
                json!({ "id": "p2b", "name": "pb:p_two_back" }),
            ]),
            None,
        )
        .await
        .unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["n", "nb", "p1", "p2", "p2b"]
                .iter()
                .all(|id| active(&loader, id))
                && services.iter().all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;

    let go = call(&fixture.root, "n_svc", "go");
    // `slow` holds at the gate: Node waits for s1, s1 for slow.
    assert_eq!(recv_within("the slow call at the gate", &arrivals), "slow");
    let kick = call(&fixture.root, "p1_svc", "kick");
    let kicked = answer(&kick, "p1.kick");
    let kicked = kicked.as_str().unwrap();
    assert_eq!(
        kicked, "p1 kicked s2: SyncWaitCycle: n_back.back would wait on itself: p2b → nb → n → p2 → p1 → p2b (synchronous calls)",
        "s2's call back is refused, the rows named (§8.1 row 9)"
    );
    opener.open("slow");
    assert_eq!(
        answer(&go, "n.go"),
        json!("n got s1 after p1 slow"),
        "the rest completes"
    );
    fixture.root.shutdown().await.unwrap();
}

/// Two Node runtimes waiting on each other through Go: `a` calls Go, which
/// calls `b_work` in the other runtime, while `b` calls `a_work` back. Go
/// runs every call on a goroutine of its own and puts none aside, but its
/// synchronous calls say they wait (`sync-wait`, #385): the host sees the
/// cycle and refuses the call that closes it, wherever that is, and the
/// rest completes.
/// risk: P10, C6
/// guarantee: bridge/sync-wait-cycle-refused
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_node_runtimes_waiting_on_each_other_through_go() {
    let dir = tempfile::tempdir().unwrap();
    let work = |me: &str| NODE_WORK.replace("SELF", me).replace("INJECT", "");
    let go = |me: &str, other: &str| NODE_GO_PEER.replace("SELF", me).replace("OTHER", other);
    let files = [
        ("aw", "node", write(dir.path(), "aw.mjs", &work("a_work"))),
        ("bw", "nb", write(dir.path(), "bw.mjs", &work("b_work"))),
        (
            "a",
            "node",
            write(dir.path(), "a.mjs", &go("a_svc", "g_relay")),
        ),
        (
            "b",
            "nb",
            write(dir.path(), "b.mjs", &go("b_svc", "a_work")),
        ),
    ];
    let (gate, _arrivals) = Gate::new(2);
    let services = ["a_work", "b_work", "a_svc", "b_svc", "g_relay"];
    let fixture = fixture(
        vec![(node("node"), true), (node("nb"), false)],
        true,
        false,
        &services,
        gate,
    )
    .await;
    let mut rows_of: Vec<Value> = files
        .iter()
        .map(|(id, runtime, file)| match *runtime {
            "node" => json!({ "id": id, "name": file.to_string_lossy() }),
            runtime => json!({ "id": id, "name": format!("{runtime}:{}", file.display()) }),
        })
        .collect();
    rows_of.push(json!({ "id": "g", "name": "go:go_relay" }));
    fixture.loader.reconcile(rows(rows_of), None).await.unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["aw", "bw", "a", "b", "g"]
                .iter()
                .all(|id| active(&loader, id))
                && services.iter().all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;

    // Both wait at the gate until both arrived: then a calls b through Go,
    // and b calls a.
    let a_go = call(&fixture.root, "a_svc", "go");
    let b_go = call(&fixture.root, "b_svc", "go");
    let answers = [answer(&a_go, "a.go"), answer(&b_go, "b.go")]
        .map(|answer| answer.as_str().unwrap().to_owned());
    assert!(
        answers
            == [
                "a_svc got g_relay relays b_work works",
                "b_svc: SyncWaitCycle"
            ]
            || answers == ["a_svc got g_relay: SyncWaitCycle", "b_svc got a_work works"],
        "exactly one call is refused, and the other completes: {answers:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// As `two_node_runtimes_waiting_on_each_other_through_go`, with Go calling
/// `b_work`'s object (a struct of blocking function fields, #687) instead of
/// the service: `a` calls Go, which calls the object in the other runtime,
/// while `b` calls `a_work` back. Go
/// runs every call on a goroutine of its own and puts none aside, but its
/// synchronous calls say they wait (`sync-wait`, #385): the host sees the
/// cycle and refuses the call that closes it, wherever that is, and the
/// rest completes.
/// risk: P10, C6
/// guarantee: bridge/sync-wait-cycle-refused
/// objects-case(go): sync-wait-cycle
/// guarantee: bridge/object-refs
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_node_runtimes_waiting_on_each_other_through_a_go_object() {
    let dir = tempfile::tempdir().unwrap();
    let work = |me: &str| NODE_WORK.replace("SELF", me).replace("INJECT", "");
    let go = |me: &str, other: &str| NODE_GO_PEER.replace("SELF", me).replace("OTHER", other);
    let files = [
        ("aw", "node", write(dir.path(), "aw.mjs", &work("a_work"))),
        ("bw", "nb", write(dir.path(), "bw.mjs", &work("b_work"))),
        (
            "a",
            "node",
            write(dir.path(), "a.mjs", &go("a_svc", "g_obj_relay")),
        ),
        (
            "b",
            "nb",
            write(dir.path(), "b.mjs", &go("b_svc", "a_work")),
        ),
    ];
    let (gate, _arrivals) = Gate::new(2);
    let services = ["a_work", "b_work", "a_svc", "b_svc", "g_obj_relay"];
    let fixture = fixture(
        vec![(node("node"), true), (node("nb"), false)],
        true,
        false,
        &services,
        gate,
    )
    .await;
    let mut rows_of: Vec<Value> = files
        .iter()
        .map(|(id, runtime, file)| match *runtime {
            "node" => json!({ "id": id, "name": file.to_string_lossy() }),
            runtime => json!({ "id": id, "name": format!("{runtime}:{}", file.display()) }),
        })
        .collect();
    rows_of.push(json!({ "id": "g", "name": "go:go_obj_relay" }));
    fixture.loader.reconcile(rows(rows_of), None).await.unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["aw", "bw", "a", "b", "g"]
                .iter()
                .all(|id| active(&loader, id))
                && services.iter().all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;

    // Both wait at the gate until both arrived: then a calls b through Go,
    // and b calls a.
    let a_go = call(&fixture.root, "a_svc", "go");
    let b_go = call(&fixture.root, "b_svc", "go");
    let answers = [answer(&a_go, "a.go"), answer(&b_go, "b.go")]
        .map(|answer| answer.as_str().unwrap().to_owned());
    assert!(
        answers
            == [
                "a_svc got g_obj_relay relays b_work works",
                "b_svc: SyncWaitCycle"
            ]
            || answers
                == [
                    "a_svc got g_obj_relay: SyncWaitCycle",
                    "b_svc got a_work works"
                ],
        "exactly one call is refused, and the other completes: {answers:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// Two Node runtimes waiting on each other through Swift: as through Go,
/// a Swift runtime runs every call in a Task of its own and its synchronous
/// calls say they wait (`sync-wait`): the host sees the cycle and refuses
/// the call that closes it, and the rest completes.
/// guarantee: bridge/sync-wait-cycle-refused
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_node_runtimes_waiting_on_each_other_through_swift() {
    let dir = tempfile::tempdir().unwrap();
    let work = |me: &str| NODE_WORK.replace("SELF", me).replace("INJECT", "");
    let go = |me: &str, other: &str| NODE_GO_PEER.replace("SELF", me).replace("OTHER", other);
    let files = [
        ("aw", "node", write(dir.path(), "aw.mjs", &work("a_work"))),
        ("bw", "nb", write(dir.path(), "bw.mjs", &work("b_work"))),
        (
            "a",
            "node",
            write(dir.path(), "a.mjs", &go("a_svc", "s_relay")),
        ),
        (
            "b",
            "nb",
            write(dir.path(), "b.mjs", &go("b_svc", "a_work")),
        ),
    ];
    let (gate, _arrivals) = Gate::new(2);
    let services = ["a_work", "b_work", "a_svc", "b_svc", "s_relay"];
    let fixture = fixture(
        vec![(node("node"), true), (node("nb"), false)],
        false,
        true,
        &services,
        gate,
    )
    .await;
    let mut rows_of: Vec<Value> = files
        .iter()
        .map(|(id, runtime, file)| match *runtime {
            "node" => json!({ "id": id, "name": file.to_string_lossy() }),
            runtime => json!({ "id": id, "name": format!("{runtime}:{}", file.display()) }),
        })
        .collect();
    rows_of.push(json!({ "id": "s", "name": "swift:swift_relay" }));
    fixture.loader.reconcile(rows(rows_of), None).await.unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["aw", "bw", "a", "b", "s"]
                .iter()
                .all(|id| active(&loader, id))
                && services.iter().all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;

    let a_go = call(&fixture.root, "a_svc", "go");
    let b_go = call(&fixture.root, "b_svc", "go");
    let answers = [answer(&a_go, "a.go"), answer(&b_go, "b.go")]
        .map(|answer| answer.as_str().unwrap().to_owned());
    assert!(
        answers
            == [
                "a_svc got s_relay relays b_work works",
                "b_svc: SyncWaitCycle"
            ]
            || answers == ["a_svc got s_relay: SyncWaitCycle", "b_svc got a_work works"],
        "exactly one call is refused, and the other completes: {answers:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

// ── An emit's listener run while a call waits ──────────────────

/// In Node: `go` calls `x` in A synchronously.
const NODE_C: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['a_svc'],
  provides: { c_svc: { go: 'sync' } },
  apply(ctx) {
    ctx.provide('c_svc', { go: () => `c got ${ctx.use('a_svc').x()}` })
  },
})
"#;

/// In the same Node runtime: `y`, what A's listener calls back.
const NODE_C_BACK: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { c_back: { y: 'sync' } },
  apply(ctx) {
    ctx.provide('c_back', { y: () => 'c y' })
  },
})
"#;

/// In another Node runtime: `fire` emits `t/n`, whose listener is A's.
/// `reports` is what the runtime reported (its standard error) since: an
/// emit that fails is reported there, the API having returned at once.
const NODE_EMITTER: &str = r#"
import { definePlugin } from 'PLUGIN'
const reported = []
const write = process.stderr.write.bind(process.stderr)
process.stderr.write = (chunk, ...rest) => {
  reported.push(String(chunk))
  return write(chunk, ...rest)
}
export default definePlugin({
  provides: { e_svc: { fire: 'sync', reports: 'sync' } },
  apply(ctx) {
    ctx.provide('e_svc', {
      fire: () => { ctx.emit('t/n'); return 'fired' },
      reports: () => reported.join(''),
    })
  },
})
"#;

/// A in Node (not reentrant): `x` holds at the gate; the listener of
/// `t/n` calls C's `y` and reports what it got at the gate.
const NODE_A: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['gate', 'c_back'],
  provides: { a_svc: { x: 'sync' } },
  apply(ctx) {
    const gate = ctx.use('gate')
    ctx.provide('a_svc', { x: () => { gate.arrive('x'); return 'x' } })
    ctx.on('t/n', () => {
      let got
      try { got = ctx.use('c_back').y() } catch (error) { got = error.name }
      gate.arrive(`listener ${got}`)
    })
  },
})
"#;

/// A in Bun (reentrant), the same.
#[cfg(feature = "bun")]
const BUN_A: &str = r#"
export const inject = ['gate', 'c_back']
export const provides = { a_svc: { x: 'sync' } }
export function apply(ctx) {
  const gate = ctx.use('gate')
  ctx.provide('a_svc', { x: () => { gate.arrive('x'); return 'x' } })
  ctx.on('t/n', () => {
    let got
    try { got = ctx.use('c_back').y() } catch (error) { got = error.name }
    gate.arrive(`listener ${got}`)
  })
}
"#;

/// A in Python (reentrant), the same.
const PY_A: &str = r#"
from rutis.advanced import SyncWaitCycle

inject = ["gate", "c_back"]


class A:
    def __init__(self, ctx=None):
        self.ctx = ctx

    def x(self):
        self.ctx.use("gate").arrive("x")
        return "x"


provides = {"a_svc": A}


def apply(ctx, config):
    ctx.provide("a_svc", A(ctx))

    def heard(*args):
        try:
            got = ctx.use("c_back").y()
        except SyncWaitCycle:
            got = "SyncWaitCycle"
        ctx.use("gate").arrive(f"listener {got}")

    ctx.on("t/n", heard)
"#;

/// C (Node, not reentrant) calls `x` in A, which holds; meanwhile another
/// runtime emits `t/n`, and the host calls A's listener, which calls C
/// back. A reentrant A (Python, Bun) runs the listener on top of `x`: C waits
/// for `x`, `x` for the listener, the listener for C. Nobody waits for the
/// listener's call (an entry, not a hop), but stack order holds for it: the
/// host refuses the call back with `SyncWaitCycle`, and the rest completes.
/// A Node A puts the listener's call aside until `x` returns (it is not
/// reentrant): it runs after, and its call back succeeds.
/// guarantee: bridge/sync-wait-cycle-refused
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_emit_listener_run_while_a_call_waits() {
    let mut kinds = vec!["python", "node"];
    if cfg!(feature = "bun") {
        kinds.push("bun");
    }
    for kind in kinds {
        let reentrant = kind != "node";
        let dir = tempfile::tempdir().unwrap();
        let c = write(dir.path(), "c.mjs", NODE_C);
        let back = write(dir.path(), "c_back.mjs", NODE_C_BACK);
        let e = write(dir.path(), "e.mjs", NODE_EMITTER);
        let a = match kind {
            "python" => {
                std::fs::write(dir.path().join("a_py.py"), PY_A).unwrap();
                "py:a_py".to_owned()
            }
            "node" => format!("na:{}", write(dir.path(), "a.mjs", NODE_A).display()),
            _ => "bn:./a.ts".to_owned(),
        };
        let (gate, arrivals) = Gate::new(0);
        let opener = gate.clone();
        let services = ["c_svc", "c_back", "e_svc", "a_svc"];
        let mut runtimes = vec![(node("node"), true), (node("ne"), false)];
        runtimes.push(match kind {
            "python" => (python("py", dir.path()), false),
            "node" => (node("na"), false),
            _ => (bun("bn", dir.path()), false),
        });
        let fixture = fixture(runtimes, false, false, &services, gate).await;
        fixture
            .loader
            .reconcile(
                rows(vec![
                    json!({ "id": "c", "name": c.to_string_lossy() }),
                    json!({ "id": "cb", "name": back.to_string_lossy() }),
                    json!({ "id": "e", "name": format!("ne:{}", e.display()) }),
                    json!({ "id": "a", "name": a }),
                ]),
                None,
            )
            .await
            .unwrap();
        let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
        COLD.until(
            || {
                ["c", "cb", "e", "a"].iter().all(|id| active(&loader, id))
                    && services.iter().all(|name| service(&root, name).is_some())
            },
            "the rows and their services",
        )
        .await;

        let go = call(&fixture.root, "c_svc", "go");
        // `x` holds at the gate: C waits for it.
        assert_eq!(recv_within("x at the gate", &arrivals), "x");
        assert_eq!(
            answer(&call(&fixture.root, "e_svc", "fire"), "e.fire"),
            json!("fired")
        );
        if reentrant {
            let heard = recv_within("the listener's report", &arrivals);
            assert_eq!(
                heard, "listener SyncWaitCycle",
                "its call back is refused ({kind})"
            );
            opener.open(&heard);
            opener.open("x");
        } else {
            opener.open("x");
            let heard = recv_within("the listener's report", &arrivals);
            assert_eq!(heard, "listener c y", "it runs once x returned ({kind})");
            opener.open(&heard);
        }
        assert_eq!(
            answer(&go, "c.go"),
            json!("c got x"),
            "the rest completes ({kind})"
        );
        // The emit itself succeeded: `fire` called it on an application
        // thread, which ran the dispatch while it waited (#567).
        assert_eq!(
            answer(&call(&fixture.root, "e_svc", "reports"), "e.reports"),
            json!(""),
            "the emit was answered without a failure ({kind})"
        );
        fixture.root.shutdown().await.unwrap();
    }
}

// Function references relayed between two Node runtimes.
const NODE_REF_WORK: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { SELF: { fn: 'sync' } },
  apply(ctx) {
    ctx.provide('SELF', { fn: () => () => 'SELF works' })
  },
})
"#;
const NODE_REF_GO: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['gate', 'OTHER'],
  provides: { SELF: { prepare: 'sync', go: 'sync' } },
  apply(ctx) {
    const gate = ctx.use('gate')
    let f
    ctx.provide('SELF', {
      prepare() { f = ctx.use('OTHER').fn(); return 'ready' },
      go() {
        gate.arrive('SELF')
        try { return `SELF got ${f()}` } catch (error) { return `SELF: ${error.name}` }
      },
    })
  },
})
"#;

/// A function reference taken from the other runtime, called while both
/// runtimes wait: the relayed call is a hop, so one of the two is refused.
/// guarantee: bridge/sync-wait-cycle-refused
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn function_references_calling_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let work = |me: &str| NODE_REF_WORK.replace("SELF", me);
    let go = |me: &str, other: &str| NODE_REF_GO.replace("SELF", me).replace("OTHER", other);
    let files = [
        ("aw", "node", write(dir.path(), "aw.mjs", &work("a_work"))),
        ("bw", "nb", write(dir.path(), "bw.mjs", &work("b_work"))),
        (
            "a",
            "node",
            write(dir.path(), "a.mjs", &go("a_svc", "b_work")),
        ),
        (
            "b",
            "nb",
            write(dir.path(), "b.mjs", &go("b_svc", "a_work")),
        ),
    ];
    let (gate, _arrivals) = Gate::new(2);
    let fixture = fixture(
        vec![(node("node"), true), (node("nb"), false)],
        false,
        false,
        &["a_work", "b_work", "a_svc", "b_svc"],
        gate,
    )
    .await;
    let rows_of = files
        .iter()
        .map(|(id, runtime, file)| match *runtime {
            "node" => json!({ "id": id, "name": file.to_string_lossy() }),
            runtime => json!({ "id": id, "name": format!("{runtime}:{}", file.display()) }),
        })
        .collect();
    fixture.loader.reconcile(rows(rows_of), None).await.unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["aw", "bw", "a", "b"].iter().all(|id| active(&loader, id))
                && ["a_work", "b_work", "a_svc", "b_svc"]
                    .iter()
                    .all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;
    answer(&call(&fixture.root, "a_svc", "prepare"), "a.prepare");
    answer(&call(&fixture.root, "b_svc", "prepare"), "b.prepare");
    let a_go = call(&fixture.root, "a_svc", "go");
    let b_go = call(&fixture.root, "b_svc", "go");
    let answers = [answer(&a_go, "a.go"), answer(&b_go, "b.go")];
    let refused = answers
        .iter()
        .filter(|a| a.as_str().unwrap().ends_with(": SyncWaitCycle"))
        .count();
    assert_eq!(refused, 1, "{answers:?}");
    fixture.root.shutdown().await.unwrap();
}

// ── An emit whose filter answers later ──────────────────────────

/// In another Node runtime: `fire` emits `t/n` through a filter, a call
/// back into this runtime: the dispatch suspends before any listener is
/// called, and calls them once the filter answered.
const NODE_FILTERED_EMITTER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { e_svc: { fire: 'sync' } },
  apply(ctx) {
    ctx.provide('e_svc', { fire: () => { ctx.filter(() => true).emit('t/n'); return 'fired' } })
  },
})
"#;

/// As [`an_emit_listener_run_while_a_call_waits`] with a reentrant A, the
/// emit going through a filter (#537): its listeners are called after the
/// dispatch suspended, not when it started. C (Node) waits for `x` in A
/// (Python); A runs the listener on top of `x`, and its call back to C
/// closes the cycle: refused with `SyncWaitCycle`, and the rest completes.
/// guarantee: bridge/sync-wait-cycle-refused
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_emit_listener_called_after_its_filter_answered() {
    let dir = tempfile::tempdir().unwrap();
    let c = write(dir.path(), "c.mjs", NODE_C);
    let back = write(dir.path(), "c_back.mjs", NODE_C_BACK);
    let e = write(dir.path(), "e.mjs", NODE_FILTERED_EMITTER);
    std::fs::write(dir.path().join("a_py.py"), PY_A).unwrap();
    let (gate, arrivals) = Gate::new(0);
    let opener = gate.clone();
    let services = ["c_svc", "c_back", "e_svc", "a_svc"];
    let fixture = fixture(
        vec![
            (node("node"), true),
            (node("ne"), false),
            (python("py", dir.path()), false),
        ],
        false,
        false,
        &services,
        gate,
    )
    .await;
    fixture
        .loader
        .reconcile(
            rows(vec![
                json!({ "id": "c", "name": c.to_string_lossy() }),
                json!({ "id": "cb", "name": back.to_string_lossy() }),
                json!({ "id": "e", "name": format!("ne:{}", e.display()) }),
                json!({ "id": "a", "name": "py:a_py" }),
            ]),
            None,
        )
        .await
        .unwrap();
    let (loader, root) = (fixture.loader.clone(), fixture.root.clone());
    COLD.until(
        || {
            ["c", "cb", "e", "a"].iter().all(|id| active(&loader, id))
                && services.iter().all(|name| service(&root, name).is_some())
        },
        "the rows and their services",
    )
    .await;

    let go = call(&fixture.root, "c_svc", "go");
    // `x` holds at the gate: C waits for it.
    assert_eq!(recv_within("x at the gate", &arrivals), "x");
    assert_eq!(
        answer(&call(&fixture.root, "e_svc", "fire"), "e.fire"),
        json!("fired")
    );
    let heard = recv_within("the listener's report", &arrivals);
    assert_eq!(heard, "listener SyncWaitCycle", "its call back is refused");
    opener.open(&heard);
    opener.open("x");
    assert_eq!(answer(&go, "c.go"), json!("c got x"), "the rest completes");
    fixture.root.shutdown().await.unwrap();
}
