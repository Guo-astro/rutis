//! Objects with methods between Node, Bun and Python, through a real host
//! (#687):
//! the owner's row hands its object to the holder's row in the other
//! runtime, which calls its methods (the owner's state changes), gets it in
//! an array and as an event argument, passes it back (the owner gets the
//! original) and on to a Rust host service (a third language). The object
//! stays callable after the owner's row unloads; once the owner's runtime
//! is gone, calls on it fail with `peer-gone`, saying where it came from.
//! Run both ways between Node and Bun, Node and Python, Node and Go, and,
//! with their features, Node and Swift (macOS), Node and Kotlin. A Go or
//! Swift runtime starts on demand: once its owner's row unloads, it stops
//! when idle, and calls on its objects say so.
//!
//! Node and Python are always there; the Bun, Go, Swift and Kotlin cases
//! run with their features (CI runs the Swift ones in the Swift job, the
//! Kotlin ones in the Kotlin job).
#![cfg(all(unix, feature = "node", feature = "python"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{layer, repo, row_state, rows_stage, Probe, COLD};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::pipe::Processes;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, settle, HostDispatch, Reply, Value as RpcValue};
use rutis_loader::runtime::RuntimeResolver;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// The owner: makes a counter and drives the holder with it.
const OWNER: &str = r#"
import { definePlugin } from 'SDK'
class Counter { n = 0; inc() { return ++this.n } value() { return this.n } }
const made = new Counter()
export default definePlugin({
  inject: ['holder'],
  provides: { owner: { run: 'async', announce: 'async', wrongs: 'async' } },
  apply(ctx) {
    const holder = ctx.use('holder')
    ctx.provide('owner', {
      async run() {
        await holder.take(made)
        await holder.inc(); await holder.inc()
        const listed = await holder.listed([{ data: 1 }, made])
        const home = (await holder.back()) === made
        const viaRust = await holder.rust()
        const missing = await holder.nope()
        const plain = await holder.plain({ ping: () => 'pong' })
        return `${made.n} ${listed} ${home} ${viaRust} ${plain} | ${missing}`
      },
      announce() { ctx.emit('counter', made); return 'sent' },
      // What a Go holder's parameters that cannot take an object say.
      async wrongs() {
        const said = []
        for (const method of ['wrong', 'text']) {
          try { await holder[method](made); said.push('taken') } catch (e) { said.push(`${e.code}: ${e.message}`) }
        }
        return said.join(' | ')
      },
    })
  },
})
"#;

/// The holder: keeps the proxy it is given and calls it.
const HOLDER: &str = r#"
import { definePlugin } from 'SDK'
let kept
export default definePlugin({
  inject: ['poker', 'probe'],
  provides: { holder: { take: 'async', inc: 'async', listed: 'async', back: 'async', rust: 'async', nope: 'async', plain: 'async', after: 'async' } },
  apply(ctx) {
    const probe = ctx.use('probe'), poker = ctx.use('poker')
    ctx.on('counter', async counter => { probe.record(`event: ${counter === kept} ${await counter.value()}`) })
    ctx.provide('holder', {
      take(counter) { kept = counter; return String(counter) },
      inc() { return kept.inc() },
      listed([data, counter]) { return `${JSON.stringify(data)} ${counter === kept}` },
      back() { return kept },
      rust() { return poker.poke(kept) },
      async nope() { try { await kept.nope(); return 'answered' } catch (e) { return `${e.code}: ${e.message}` } },
      plain(object) { return object.ping() },
      async after() { try { return String(await kept.inc()) } catch (e) { return `${e.code}: ${e.message}` } },
    })
  },
})
"#;

/// The owner in Python (a class instance with a method, where JavaScript
/// passes a plain object with a function).
const PY_OWNER: &str = r#"
inject = ["holder"]


class Counter:
    def __init__(self):
        self.n = 0

    def inc(self):
        self.n += 1
        return self.n

    def value(self):
        return self.n


class Pinger:
    def ping(self):
        return "pong"


made = Counter()


class Owner:
    def __init__(self, ctx):
        self.ctx = ctx

    async def run(self):
        holder = self.ctx.use("holder")
        await holder.take(made)
        await holder.inc()
        await holder.inc()
        listed = await holder.listed([{"data": 1}, made])
        home = "true" if (await holder.back()) is made else "false"
        via_rust = await holder.rust()
        missing = await holder.nope()
        plain = await holder.plain(Pinger())
        return f"{made.n} {listed} {home} {via_rust} {plain} | {missing}"

    def announce(self):
        self.ctx.emit("counter", made)
        return "sent"


provides = {"owner": Owner}


def apply(ctx, config):
    ctx.provide("owner", Owner(ctx))
"#;

/// The holder in Python.
const PY_HOLDER: &str = r#"
import json

inject = ["poker", "probe"]
kept = None


def said(error):
    return f"{getattr(error, 'code', None)}: {getattr(error, 'message', error)}"


class Holder:
    def __init__(self, ctx):
        self.ctx = ctx

    def take(self, counter):
        global kept
        kept = counter
        return repr(counter)

    async def inc(self):
        return await kept.inc()

    def listed(self, items):
        data, counter = items
        return f"{json.dumps(data, separators=(',', ':'))} {'true' if counter is kept else 'false'}"

    def back(self):
        return kept

    async def rust(self):
        return await self.ctx.use("poker").poke(kept)

    async def nope(self):
        try:
            await kept.nope()
            return "answered"
        except Exception as error:
            return said(error)

    async def plain(self, thing):
        return await thing.ping()

    async def after(self):
        try:
            return str(await kept.inc())
        except Exception as error:
            return said(error)


provides = {"holder": Holder}


def apply(ctx, config):
    probe = ctx.use("probe")

    async def heard(counter):
        probe.record(f"event: {'true' if counter is kept else 'false'} {await counter.value()}")

    ctx.on("counter", heard)
    ctx.provide("holder", Holder(ctx))
"#;

/// The host service `poker`: calls `inc` on the object it is given, as a
/// Rust host's code does (a third language).
struct Poker;

impl HostDispatch for Poker {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        assert_eq!(method, "poke");
        let object = args.list()?.remove(0).reference()?;
        Ok(RpcValue::future(async move {
            object
                .call_method_async("inc", RpcValue::List(vec![]))
                .await
        }))
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "poke": "async" }))
    }
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    node: Processes,
    #[cfg(feature = "bun")]
    bun: Processes,
    py: Processes,
    #[cfg(feature = "kotlin")]
    kt: Processes,
    dir: PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    // Python's modules in a directory of their own, its project.
    let py = dir.path().join("py");
    std::fs::create_dir_all(&py).unwrap();
    for (name, text) in [("owner", PY_OWNER), ("holder", PY_HOLDER)] {
        std::fs::write(py.join(format!("{name}.py")), text).unwrap();
    }
    for (name, text) in [("owner", OWNER), ("holder", HOLDER)] {
        for extension in ["mjs", "ts"] {
            std::fs::write(
                dir.path().join(format!("{name}.{extension}")),
                text.replace("SDK", &sdk),
            )
            .unwrap();
        }
    }
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    root.provide_at(host_key("poker"), Arc::new(Poker)).unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in ["probe", "poker", "owner", "holder"] {
        catalog.register_shared(name);
    }
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let python = LocalRuntime::python(&py).python_path(repo().join("python/rutis"));
    let (node_processes, py_processes) = (node.processes(), python.processes());
    let chain = Chain::new();
    // Bun, rows `bun:<module>`.
    #[cfg(feature = "bun")]
    let bun = LocalRuntime::bun(repo().join("bun/rutis-bun"), dir.path());
    #[cfg(feature = "bun")]
    let bun_processes = bun.processes();
    #[cfg(feature = "bun")]
    let bun_rows = Arc::new(RuntimeResolver::modules(bun.handle()).with_catalog(&catalog));
    #[cfg(feature = "bun")]
    root.plugin(bun);
    // Go: the multilang binary's plugins, rows `go:<plugin>`.
    #[cfg(feature = "go")]
    let go = Arc::new(
        rutis_loader::runtime::go::GoResolver::new(
            rutis_loader::runtime::go::GoBinaries::new().file(common::runtimes::go_binary()),
        )
        .with_catalog(&catalog)
        .reserve(["node", "py", "bun", "kt"]),
    );
    #[cfg(feature = "go")]
    let chain = chain.with_shared(go.clone());
    // Kotlin: the conformance jar's plugins, rows `kt:<plugin>`.
    #[cfg(feature = "kotlin")]
    let kotlin = {
        let project = dir.path().join("kt");
        std::fs::create_dir_all(&project).unwrap();
        LocalRuntime::launcher("kt", common::runtimes::kotlin_launcher(&project), &project)
    };
    #[cfg(feature = "kotlin")]
    let kt_processes = kotlin.processes();
    #[cfg(feature = "kotlin")]
    let kt_rows = Arc::new(RuntimeResolver::modules(kotlin.handle()).with_catalog(&catalog));
    #[cfg(feature = "kotlin")]
    root.plugin(kotlin);
    // Swift: the multilang binary's plugins, rows `swift:<plugin>`, in the
    // runtime `swift-multilang`.
    #[cfg(all(feature = "swift", target_os = "macos"))]
    let swift = Arc::new(
        rutis_loader::runtime::swift::SwiftResolver::new(
            rutis_loader::runtime::swift::SwiftBinaries::new().file(
                common::runtimes::swift_binary("RutisMultilang", "multilang"),
            ),
        )
        .with_catalog(&catalog)
        .reserve(["node", "py", "bun", "kt"]),
    );
    let py_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    root.plugin(python);
    root.plugin(node);
    #[cfg(all(feature = "swift", target_os = "macos"))]
    let chain = chain.with_shared(swift.clone());
    #[cfg(feature = "kotlin")]
    let chain = chain.with_shared(kt_rows.clone());
    #[cfg(feature = "bun")]
    let chain = chain.with_shared(bun_rows.clone());
    let chain = chain
        .with_shared(py_rows.clone())
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
    for rows in [py_rows, node_rows] {
        rows_stage(&root, rows).await;
    }
    #[cfg(feature = "bun")]
    rows_stage(&root, bun_rows).await;
    #[cfg(feature = "kotlin")]
    rows_stage(&root, kt_rows).await;
    // Stops two seconds after its last row is gone.
    #[cfg(feature = "go")]
    root.plugin(
        rutis_loader::runtime::go::GoRuntimes::new(go, dir.path())
            .idle(Some(std::time::Duration::from_secs(2))),
    )
    .await
    .unwrap();
    #[cfg(all(feature = "swift", target_os = "macos"))]
    root.plugin(
        rutis_loader::runtime::swift::SwiftRuntimes::new(swift, dir.path())
            .idle(Some(std::time::Duration::from_secs(2))),
    )
    .await
    .unwrap();
    Fixture {
        root,
        loader,
        probe,
        node: node_processes,
        #[cfg(feature = "bun")]
        bun: bun_processes,
        py: py_processes,
        #[cfg(feature = "kotlin")]
        kt: kt_processes,
        dir: dir.path().to_owned(),
        _dir: dir,
    }
}

/// Call `service.method` from the host, as Rust code does: its result.
/// The service is waited for: a row's service is there once the row runs,
/// which may be just after `reconcile` returns.
async fn call(root: &Ctx, service: &str, method: &str) -> Value {
    let dispatch = COLD
        .eventually(
            || root.get_at::<dyn HostDispatch>(&host_key(service)),
            &format!("the service {service}"),
        )
        .await;
    let reply = dispatch.invoke(method, RpcValue::List(vec![])).unwrap();
    let value = settle(reply).await.unwrap();
    rutis_bridge::session::decode_value(value).unwrap()
}

/// The rows: the owner in the runtime `owner`, the holder in `holder`
/// (`node`, `bun`, `py`, `go`, `swift` or `kt`).
fn rows(fixture: &Fixture, owner: &str, holder: &str, with_owner: bool) -> Vec<Value> {
    let row = |runtime: &str, name: &str| match runtime {
        "node" => fixture
            .dir
            .join(format!("{name}.mjs"))
            .to_string_lossy()
            .into_owned(),
        "py" => format!("py:{name}"),
        "go" => format!("go:go_{name}"),
        "swift" => format!("swift:swift_{name}"),
        "kt" => format!("kt:kt_{name}"),
        _ => format!("bun:./{name}.ts"),
    };
    let mut rows = vec![json!({ "id": "holder", "name": row(holder, "holder") })];
    if with_owner {
        rows.push(json!({ "id": "owner", "name": row(owner, "owner") }));
    }
    rows
}

async fn objects_cross(owner: &str, holder: &str) {
    let fixture = fixture().await;
    let report = fixture
        .loader
        .reconcile(layer(rows(&fixture, owner, holder, true)), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    // Both rows run before the owner is called.
    COLD.eventually_with(
        || {
            ["owner", "holder"]
                .iter()
                .all(|id| row_state(&fixture.loader, id) == Some(rutis::host::FiberState::Active))
                .then_some(())
        },
        "both rows running",
        || {
            format!(
                "{:#?}",
                fixture
                    .loader
                    .entries()
                    .into_iter()
                    .map(|entry| format!("{entry:?}"))
                    .collect::<Vec<_>>()
            )
        },
    )
    .await;
    // Two calls of the holder's, one of Rust's: 3; listed in an array, the
    // same proxy; passed back, the original; a missing method is the
    // owner's to answer; a plain object with a function goes by reference.
    assert_eq!(
        call(&fixture.root, "owner", "run").await,
        json!(
            "3 {\"data\":1} true true 3 pong | no-method: Counter has no method nope (methods: inc, value)"
        )
    );
    // As an event argument: the same proxy.
    call(&fixture.root, "owner", "announce").await;
    fixture.probe.wait_for("event: true 3").await;
    // The owner's row unloads: the object is still callable.
    fixture
        .loader
        .reconcile(layer(rows(&fixture, owner, holder, false)), None)
        .await
        .unwrap();
    assert_eq!(call(&fixture.root, "holder", "after").await, json!("4"));
    // The owner's runtime is gone: peer-gone, saying where from.
    // A Go or Swift runtime stops by itself, idle; another is killed.
    let said = match owner {
        "go" | "swift" => format!("peer-gone: a call on an object from {owner}-multilang failed: runtime {owner}-multilang stopped because it was idle"),
        _ => {
            match owner {
                "node" => fixture.node.kill(),
                "py" => fixture.py.kill(),
                #[cfg(feature = "kotlin")]
                "kt" => fixture.kt.kill(),
                #[cfg(feature = "bun")]
                "bun" => fixture.bun.kill(),
                other => panic!("no runtime {other}"),
            }
            format!("peer-gone: a call on an object from {owner} failed: runtime {owner} exited")
        }
    };
    let mut answer = Value::Null;
    for _ in 0..200 {
        answer = call(&fixture.root, "holder", "after").await;
        if answer == json!(said) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(answer, json!(said));
    fixture.root.shutdown().await.unwrap();
}

/// objects-case(node): send-call-state
/// objects-case(node): same-proxy-home
/// objects-case(node): third-language
/// objects-case(node): return-array-event
/// objects-case(node): outlives-its-row
/// objects-case(node): owner-killed-peer-gone
/// objects-case(rust): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn a_node_object_crosses_to_bun() {
    objects_cross("node", "bun").await;
}

/// objects-case(bun): send-call-state
/// objects-case(bun): same-proxy-home
/// objects-case(bun): third-language
/// objects-case(bun): return-array-event
/// objects-case(bun): outlives-its-row
/// objects-case(bun): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[cfg(feature = "bun")]
#[tokio::test(flavor = "multi_thread")]
async fn a_bun_object_crosses_to_node() {
    objects_cross("bun", "node").await;
}

/// objects-case(python): send-call-state
/// objects-case(python): same-proxy-home
/// objects-case(python): third-language
/// objects-case(python): return-array-event
/// objects-case(python): outlives-its-row
/// objects-case(python): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread")]
async fn a_python_object_crosses_to_node() {
    objects_cross("py", "node").await;
}

/// objects-case(python): send-call-state
/// objects-case(python): same-proxy-home
/// objects-case(python): return-array-event
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread")]
async fn a_node_object_crosses_to_python() {
    objects_cross("node", "py").await;
}

/// objects-case(go): send-call-state
/// objects-case(go): same-proxy-home
/// objects-case(go): third-language
/// objects-case(go): return-array-event
/// objects-case(go): outlives-its-row
/// objects-case(go): idle-stop-peer-gone
/// objects-case(rust): idle-stop-peer-gone
/// guarantee: bridge/object-refs
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn a_go_object_crosses_to_node() {
    objects_cross("go", "node").await;
}

/// objects-case(go): send-call-state
/// objects-case(go): same-proxy-home
/// objects-case(go): third-language
/// objects-case(go): return-array-event
/// guarantee: bridge/object-refs
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn a_node_object_crosses_to_go() {
    objects_cross("node", "go").await;
}

/// A Go parameter that cannot take an object fails the call with
/// `bad-argument`, saying what it takes (#687 §八).
/// objects-case(go): declared-type-refuses
/// guarantee: bridge/object-refs
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn a_go_parameter_that_takes_no_object_says_so() {
    let fixture = fixture().await;
    fixture
        .loader
        .reconcile(layer(rows(&fixture, "node", "go", true)), None)
        .await
        .unwrap();
    assert_eq!(
        call(&fixture.root, "owner", "wrongs").await,
        json!(
            "bad-argument: holder.wrong: argument 1 should be a Tally, got an object (an object is received only as a struct of function fields or any) | \
             bad-argument: holder.text: argument 1 should be a string, got an object"
        )
    );
    fixture.root.shutdown().await.unwrap();
}

/// A Go runtime holding an object another runtime called crashes: calls on
/// its objects fail with `peer-gone`, saying it exited.
/// objects-case(go): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[cfg(feature = "go")]
#[tokio::test(flavor = "multi_thread")]
async fn a_go_object_whose_runtime_exited_is_peer_gone() {
    runtime_exited("go").await;
}

/// The owner's compiled runtime (`go` or `swift`) crashes after its object
/// was called: calls on the object fail with `peer-gone`, saying it exited.
#[cfg(any(feature = "go", all(feature = "swift", target_os = "macos")))]
async fn runtime_exited(owner: &str) {
    let fixture = fixture().await;
    fixture
        .loader
        .reconcile(layer(rows(&fixture, owner, "node", true)), None)
        .await
        .unwrap();
    call(&fixture.root, "owner", "run").await;
    assert_eq!(call(&fixture.root, "owner", "die").await, json!("dying"));
    let said = format!(
        "peer-gone: a call on an object from {owner}-multilang failed: runtime {owner}-multilang exited"
    );
    let mut answer = Value::Null;
    for _ in 0..200 {
        answer = call(&fixture.root, "holder", "after").await;
        if answer == json!(said) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(answer, json!(said));
    fixture.root.shutdown().await.unwrap();
}

/// objects-case(swift): send-call-state
/// objects-case(swift): same-proxy-home
/// objects-case(swift): third-language
/// objects-case(swift): return-array-event
/// objects-case(swift): outlives-its-row
/// objects-case(swift): idle-stop-peer-gone
/// guarantee: bridge/object-refs
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_swift_object_crosses_to_node() {
    objects_cross("swift", "node").await;
}

/// objects-case(swift): send-call-state
/// objects-case(swift): same-proxy-home
/// objects-case(swift): third-language
/// objects-case(swift): return-array-event
/// guarantee: bridge/object-refs
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_node_object_crosses_to_swift() {
    objects_cross("node", "swift").await;
}

/// A Swift runtime holding an object another runtime called crashes.
/// objects-case(swift): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[cfg(all(feature = "swift", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_swift_object_whose_runtime_exited_is_peer_gone() {
    runtime_exited("swift").await;
}

/// objects-case(kotlin): send-call-state
/// objects-case(kotlin): same-proxy-home
/// objects-case(kotlin): third-language
/// objects-case(kotlin): return-array-event
/// objects-case(kotlin): outlives-its-row
/// objects-case(kotlin): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn a_kotlin_object_crosses_to_node() {
    objects_cross("kt", "node").await;
}

/// objects-case(kotlin): send-call-state
/// objects-case(kotlin): same-proxy-home
/// objects-case(kotlin): third-language
/// objects-case(kotlin): return-array-event
/// guarantee: bridge/object-refs
#[cfg(feature = "kotlin")]
#[tokio::test(flavor = "multi_thread")]
async fn a_node_object_crosses_to_kotlin() {
    objects_cross("node", "kt").await;
}
