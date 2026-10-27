//! A value that cannot cross between languages, or a message over the
//! limit (16 MiB), fails only that one call, with an error naming the
//! call, the argument (or the result) and why: the sessions, the runtime
//! processes, other calls and other rows go on (#675). Each case of the
//! issue, between rows in Node, Python and Go: an object with methods or a
//! class instance (TS → Python, TS → a Go `any`, Python → TS, either way
//! as a result), an array with empty slots, a message over the limit
//! either way, and a JavaScript `throw 'a string'` relayed by a Go service
//! (`return err`).
#![cfg(all(feature = "node", feature = "python", feature = "go"))]

mod common;

use std::sync::Arc;

use common::runtimes::{go_binary, layer, repo, row_state, Probe};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::host_key;
use rutis_loader::runtime::go::{GoBinaries, GoResolver, GoRuntimes};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

const PY_TAKER: &str = r#"
class Counter:
    def __init__(self):
        self.n = 0

    def inc(self):
        self.n += 1
        return self.n


class Taker:
    def take(self, value):
        if isinstance(value, (list, str)):
            return f"{type(value).__name__} of {len(value)}"
        return type(value).__name__

    def big(self, size):
        return "x" * size

    def obj(self):
        return Counter()

    def ping(self):
        return "py ok"


provides = {"py_taker": Taker}


def apply(ctx, config):
    ctx.provide("py_taker", Taker())
"#;

const PY_CALLER: &str = r#"
BIG = 17 * 1024 * 1024
inject = ["js_taker", "probe"]


class Counter:
    def __init__(self):
        self.n = 0

    def inc(self):
        self.n += 1
        return self.n


def apply(ctx, config):
    probe = ctx.use("probe")
    js = ctx.use("js_taker")

    def record(what, run):
        try:
            probe.record(f"py {what}: {run()}")
        except Exception as error:
            probe.record(f"py {what} failed: {error}")

    record("object to js", lambda: js.take(Counter()))
    record("big to js", lambda: js.take("x" * BIG))
    record("big from js", lambda: js.big(BIG))
    record("object from js", lambda: js.obj())
    probe.record(f"py after: {js.ping()}")
"#;

const JS_TAKER: &str = r#"
import { definePlugin } from 'PLUGIN'
class Counter { n = 0; inc() { return ++this.n } }
export default definePlugin({
  provides: { js_taker: { take: 'sync', big: 'sync', obj: 'sync', ping: 'sync' } },
  apply(ctx) {
    ctx.provide('js_taker', {
      take: value => typeof value === 'string' ? `string of ${value.length}` : typeof value,
      big: size => 'x'.repeat(size),
      obj: () => new Counter(),
      ping: () => 'js ok',
    })
  },
})
"#;

const JS_CALLER: &str = r#"
import { definePlugin } from 'PLUGIN'
class Counter { n = 0; inc() { return ++this.n } }
const BIG = 17 * 1024 * 1024
export default definePlugin({
  inject: ['py_taker', 'go_taker', 'probe'],
  apply(ctx) {
    const probe = ctx.use('probe'), py = ctx.use('py_taker'), go = ctx.use('go_taker')
    const record = (what, run) => {
      try { probe.record(`js ${what}: ${run()}`) } catch (error) { probe.record(`js ${what} failed: ${error?.message ?? error}`) }
    }
    record('object to py', () => py.take(new Counter()))
    record('object to go', () => go.take(new Counter()))
    // eslint-disable-next-line no-sparse-arrays
    record('sparse to py', () => py.take([1, , 3]))
    record('big to py', () => py.take('x'.repeat(BIG)))
    record('big from py', () => py.big(BIG))
    record('object from py', () => py.obj())
    try {
      go.relay(() => { throw 'a string' })
      probe.record('js relay: returned')
    } catch (thrown) {
      probe.record(`js relay threw ${typeof thrown}: ${thrown}`)
    }
    probe.record(`js after: ${py.ping()} / ${go.ping()}`)
  },
})
"#;

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    js: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let py = dir.path().join("py");
    let js = dir.path().join("js");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::create_dir_all(&js).unwrap();
    std::fs::write(py.join("py_taker.py"), PY_TAKER).unwrap();
    std::fs::write(py.join("py_caller.py"), PY_CALLER).unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    std::fs::write(js.join("js_taker.mjs"), JS_TAKER.replace("PLUGIN", &sdk)).unwrap();
    std::fs::write(js.join("js_caller.mjs"), JS_CALLER.replace("PLUGIN", &sdk)).unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in ["probe", "py_taker", "js_taker", "go_taker"] {
        catalog.register_shared(name);
    }
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let python = LocalRuntime::python(&py).python_path(repo().join("python/rutis"));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    let go = Arc::new(
        GoResolver::new(GoBinaries::new().file(go_binary()))
            .with_catalog(&catalog)
            .reserve(["node", "py"]),
    );
    root.plugin(node);
    root.plugin(python);
    let plugin = LoaderPlugin::new(
        Chain::new()
            .with_shared(go.clone())
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
    root.plugin(GoRuntimes::new(go, dir.path()).idle(None))
        .await
        .unwrap();
    Fixture {
        root,
        loader,
        probe,
        js,
        _dir: dir,
    }
}

fn named(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "config": {} })
}

impl Fixture {
    fn js(&self, id: &str, file: &str) -> Value {
        named(id, &self.js.join(format!("{file}.mjs")).to_string_lossy())
    }

    /// The line recorded that starts with `start`; on a timeout, what was
    /// recorded and where every row is.
    async fn line(&self, start: &str) -> String {
        Guard(HANG_GUARD * 3)
            .eventually_with(
                || {
                    self.probe
                        .lines()
                        .into_iter()
                        .find(|line| line.starts_with(start))
                },
                &format!("a line {start:?}"),
                || {
                    let rows: Vec<String> = self
                        .loader
                        .entries()
                        .into_iter()
                        .map(|entry| format!("{}: {:?}", entry.id, entry.status))
                        .collect();
                    format!("recorded: {:#?}\nrows: {rows:#?}", self.probe.lines())
                },
            )
            .await
    }
}

/// The line starting with `start` holds every one of `parts`.
async fn expect(fixture: &Fixture, start: &str, parts: &[&str]) {
    let line = fixture.line(start).await;
    for part in parts {
        assert!(line.contains(part), "{line:?} lacks {part:?}");
    }
}

const OVER: &str = "over the limit of 16777216 bytes (16 MiB)";
/// What the host says of a far end that takes no objects (#687 §八).
const OLDER: &str = "does not take objects (older than rutis 0.18); upgrade it";

/// guarantee: bridge/bad-values
///
/// Each bad value of #675 fails only its own call, between rows in Node,
/// Python and Go, with an error naming the call, the argument or the
/// result and why; every call after them is answered, every row keeps
/// running (no runtime process ended), and a value the far end can take
/// (an array with empty slots) crosses.
#[tokio::test(flavor = "multi_thread")]
async fn a_bad_value_fails_only_its_call_between_node_python_and_go() {
    let fixture = fixture().await;
    let rows = vec![
        named("pt", "py:py_taker"),
        fixture.js("jt", "js_taker"),
        named("gt", "go:go_taker"),
        fixture.js("jc", "js_caller"),
        named("pc", "py:py_caller"),
    ];
    let report = fixture.loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");

    // TS → Python, TS → Go `any`, Python → TS: refused on the way, by the
    // host, which knows the far end takes no objects.
    let object_to = |service: &str, from: &str, far: &str| {
        [
            format!("{service}.take: argument 1 cannot be sent: an object from {from} cannot cross to {far}"),
            format!("{far} does not take objects (older than rutis 0.18); upgrade it"),
        ]
    };
    let parts = object_to("py_taker", "node", "py");
    expect(
        &fixture,
        "js object to py failed: ",
        &[&parts[0], &parts[1]],
    )
    .await;
    let parts = object_to("go_taker", "node", "go-multilang");
    expect(
        &fixture,
        "js object to go failed: ",
        &[&parts[0], &parts[1]],
    )
    .await;
    let parts = object_to("js_taker", "py", "node");
    expect(
        &fixture,
        "py object to js failed: ",
        &[&parts[0], &parts[1]],
    )
    .await;
    // As results, either way.
    expect(
        &fixture,
        "js object from py failed: ",
        &[
            "py_taker.obj: the result cannot be sent: an object from py cannot cross to node",
            OLDER,
        ],
    )
    .await;
    expect(
        &fixture,
        "py object from js failed: ",
        &[
            "js_taker.obj: the result cannot be sent: an object from node cannot cross to py",
            OLDER,
        ],
    )
    .await;
    // An empty slot is undefined: the array crosses whole.
    expect(&fixture, "js sparse to py: list of 3", &[]).await;
    // Over the limit, refused by the side that would send it, either way.
    expect(
        &fixture,
        "js big to py failed: ",
        &[
            "py_taker.take: argument 1 cannot be sent: the message would be ",
            OVER,
        ],
    )
    .await;
    expect(
        &fixture,
        "js big from py failed: ",
        &[
            "py_taker.big: the result cannot be sent: the message would be ",
            OVER,
        ],
    )
    .await;
    expect(
        &fixture,
        "py big to js failed: ",
        &[
            "js_taker.take: argument 1 cannot be sent: the message would be ",
            OVER,
        ],
    )
    .await;
    expect(
        &fixture,
        "py big from js failed: ",
        &[
            "js_taker.big: the result cannot be sent: the message would be ",
            OVER,
        ],
    )
    .await;
    // A thrown string, relayed by Go's `return err`, is the string again.
    expect(&fixture, "js relay threw string: a string", &[]).await;
    // And everything after them goes on.
    expect(&fixture, "js after: py ok / go ok", &[]).await;
    expect(&fixture, "py after: js ok", &[]).await;
    // The callers record their last line from inside `apply`, so a row may
    // still be Loading for a moment after it: wait for Active rather than
    // read the state at once (that raced on a loaded CI runner).
    for id in ["pt", "jt", "gt", "jc", "pc"] {
        Guard(HANG_GUARD * 3)
            .until(
                || row_state(&fixture.loader, id) == Some(FiberState::Active),
                &format!("{id} keeps running: {:#?}", fixture.probe.lines()),
            )
            .await;
    }
    fixture.root.shutdown().await.unwrap();
}
