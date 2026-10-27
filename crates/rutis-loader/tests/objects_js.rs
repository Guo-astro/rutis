//! Objects with methods between Node and Bun, through a real host (#687):
//! the owner's row hands its object to the holder's row in the other
//! runtime, which calls its methods (the owner's state changes), gets it in
//! an array and as an event argument, passes it back (the owner gets the
//! original) and on to a Rust host service (a third language). The object
//! stays callable after the owner's row unloads; once the owner's runtime
//! is gone, calls on it fail with `peer-gone`, saying where it came from.
//! Run both ways: Node owns and Bun holds, and the reverse.
#![cfg(all(unix, feature = "bun", feature = "node"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{layer, repo, Probe};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, settle, HostDispatch, Reply, Value as RpcValue};
use rutis_bridge::transport::local::Processes;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// The owner: makes a counter and drives the holder with it.
const OWNER: &str = r#"
import { definePlugin } from 'SDK'
class Counter { n = 0; inc() { return ++this.n } value() { return this.n } }
const made = new Counter()
export default definePlugin({
  inject: ['holder'],
  provides: { owner: { run: 'async', announce: 'async' } },
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
    bun: Processes,
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
    let bun = LocalRuntime::bun(repo().join("bun/rutis-bun"), dir.path());
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let (node_processes, bun_processes) = (node.processes(), bun.processes());
    let bun_rows = Arc::new(RuntimeResolver::modules(bun.handle()).with_catalog(&catalog));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    root.plugin(bun);
    root.plugin(node);
    let chain = Chain::new()
        .with_shared(bun_rows.clone())
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
    for rows in [bun_rows, node_rows] {
        root.plugin(RuntimeRowsPlugin::new(rows));
    }
    Fixture {
        root,
        loader,
        probe,
        node: node_processes,
        bun: bun_processes,
        dir: dir.path().to_owned(),
        _dir: dir,
    }
}

/// Call `service.method` from the host, as Rust code does: its result.
async fn call(root: &Ctx, service: &str, method: &str) -> Value {
    let dispatch = root.get_at::<dyn HostDispatch>(&host_key(service)).unwrap();
    let reply = dispatch.invoke(method, RpcValue::List(vec![])).unwrap();
    let value = settle(reply).await.unwrap();
    rutis_bridge::session::decode_value(value).unwrap()
}

/// The rows: the owner in `owner` (`node` or `bun`), the holder in the
/// other runtime.
fn rows(fixture: &Fixture, owner: &str, with_owner: bool) -> Vec<Value> {
    let row = |runtime: &str, name: &str| match runtime {
        "node" => fixture
            .dir
            .join(format!("{name}.mjs"))
            .to_string_lossy()
            .into_owned(),
        _ => format!("bun:./{name}.ts"),
    };
    let holder = if owner == "node" { "bun" } else { "node" };
    let mut rows = vec![json!({ "id": "holder", "name": row(holder, "holder") })];
    if with_owner {
        rows.push(json!({ "id": "owner", "name": row(owner, "owner") }));
    }
    rows
}

async fn objects_cross(owner: &str) {
    let fixture = fixture().await;
    let report = fixture
        .loader
        .reconcile(layer(rows(&fixture, owner, true)), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
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
        .reconcile(layer(rows(&fixture, owner, false)), None)
        .await
        .unwrap();
    assert_eq!(call(&fixture.root, "holder", "after").await, json!("4"));
    // The owner's runtime is gone: peer-gone, saying where from.
    match owner {
        "node" => fixture.node.kill(),
        _ => fixture.bun.kill(),
    }
    let said = format!(
        "peer-gone: a call on an object from {owner} failed: runtime {owner} exited"
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

/// objects-case(node): send-call-state
/// objects-case(node): same-proxy-home
/// objects-case(node): third-language
/// objects-case(node): return-array-event
/// objects-case(node): outlives-its-row
/// objects-case(node): owner-killed-peer-gone
/// objects-case(rust): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread")]
async fn a_node_object_crosses_to_bun() {
    objects_cross("node").await;
}

/// objects-case(bun): send-call-state
/// objects-case(bun): same-proxy-home
/// objects-case(bun): third-language
/// objects-case(bun): return-array-event
/// objects-case(bun): outlives-its-row
/// objects-case(bun): owner-killed-peer-gone
/// guarantee: bridge/object-refs
#[tokio::test(flavor = "multi_thread")]
async fn a_bun_object_crosses_to_node() {
    objects_cross("bun").await;
}
