//! Synchronous calls between runtimes that would wait on each other for
//! ever: the host refuses the call that closes the cycle with
//! `SyncWaitCycle`, and everything else completes (#228; multi-language
//! design §9, policy B; `specs/cross-runtime-sync`). A gate, a host service
//! the test opens, fixes the order of the calls: no sleeps.
#![cfg(all(feature = "node", feature = "python"))]

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use rutis::{Ctx, FiberState};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{Reply, Value as RpcValue};
use rutis_loader::{
    Chain, EntryStatus, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, RuntimeResolver,
    RuntimeRowsPlugin, ServiceCatalog,
};
use serde_json::{json, Value};

/// The hang guard of every wait (Q7.1): a cycle the host misses hangs.
/// The gate and each synchronous forward hold a worker of the test's
/// runtime while they wait; the tests use 8, twice what they hold at most.
const GUARD: Duration = Duration::from_secs(20);

// ── Two Node runtimes calling each other ────────────────────────
// Services of different rows, so no two rows wait for each other's.

/// What each runtime does for the other: `work`, and `echo`.
const NODE_WORK: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: [INJECT],
  provides: { SELF: { work: 'sync', echo: 'sync', relay: 'sync' } },
  apply(ctx) {
    ctx.provide('SELF', {
      work: () => 'SELF works',
      echo: () => 'SELF echoes',
      relay: () => `SELF relays ${ctx.use('a_work').echo()}`,
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
from rutis import SyncWaitCycle

inject = ["n_back"]


class Back:
    def __init__(self, ctx=None):
        self.ctx = ctx

    def s2(self):
        try:
            return f"s2 got {self.ctx.use('n_back').back()}"
        except SyncWaitCycle:
            return "s2: SyncWaitCycle"


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
            .wait_timeout_while(state, GUARD, |state| {
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

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
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

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    tokio::time::timeout(GUARD, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

fn service(root: &Ctx, name: &str) -> Option<Arc<dyn HostDispatch>> {
    root.get_as::<dyn HostDispatch>(host_key(name))
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
        .recv_timeout(GUARD)
        .unwrap_or_else(|_| panic!("{what} hangs: the runtimes wait on each other"))
        .unwrap_or_else(|error| panic!("{what} failed: {error}"))
}

/// Runtimes, one loader over them, and the gate.
struct Fixture {
    root: Ctx,
    loader: Loader,
    _dir: tempfile::TempDir,
}

async fn fixture(runtimes: Vec<(LocalRuntime, bool)>, services: &[&str], gate: Gate) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("gate"), Arc::new(gate))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in std::iter::once(&"gate").chain(services) {
        catalog.register_shared(*name);
    }
    let mut chain = Chain::new();
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
        root.plugin(RuntimeRowsPlugin::new(resolver));
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
    until("the rows and their services", || {
        ["aw", "bw", "a", "b"].iter().all(|id| active(&loader, id))
            && ["a_work", "b_work", "a_svc", "b_svc"]
                .iter()
                .all(|name| service(&root, name).is_some())
    })
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
    until("the rows and their services", || {
        ["n", "nb", "p1", "p2", "p2b"]
            .iter()
            .all(|id| active(&loader, id))
            && services.iter().all(|name| service(&root, name).is_some())
    })
    .await;

    let go = call(&fixture.root, "n_svc", "go");
    // `slow` holds at the gate: Node waits for s1, s1 for slow.
    assert_eq!(arrivals.recv_timeout(GUARD).unwrap(), "slow");
    let kick = call(&fixture.root, "p1_svc", "kick");
    let kicked = answer(&kick, "p1.kick");
    let kicked = kicked.as_str().unwrap();
    assert!(
        kicked == "p1 kicked s2: SyncWaitCycle",
        "s2's call back is refused: {kicked}"
    );
    opener.open("slow");
    assert_eq!(
        answer(&go, "n.go"),
        json!("n got s1 after p1 slow"),
        "the rest completes"
    );
    fixture.root.shutdown().await.unwrap();
}
