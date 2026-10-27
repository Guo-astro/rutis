//! What a developer reads when a call goes wrong names only what the
//! developer wrote (#677, #680; the diagnostics design §4.5): a method
//! that does not exist, a wrong argument, a service not declared in
//! `inject`, an error the method throws, between rows in Node, Python and
//! Go and a Rust service. No message holds the words of the platform's
//! inside: `rutis`, `Cordis`, `binding`, `session`, internal ids (`row:`,
//! a NUL), `WireValue`, `runtime-rows`, `<root>`, `<leaf>`.
#![cfg(all(feature = "node", feature = "python", feature = "go"))]

mod common;

use std::sync::Arc;

use common::runtimes::{go_binary, layer, repo, Probe};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, settle, HostDispatch, Value as RpcValue};
use rutis_loader::runtime::go::{GoBinaries, GoResolver, GoRuntimes};
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

/// The words no message may hold (§4.5).
const FORBIDDEN: &[&str] = &[
    "rutis",
    "Cordis",
    "binding",
    "session",
    "row:",
    "\0",
    "WireValue",
    "runtime-rows",
    "<root>",
    "<leaf>",
];

/// A Rust service with typed arguments, as a host provides one.
pub struct Adder;

#[rutis::service]
impl Adder {
    pub fn add(&self, a: u32, label: String) -> String {
        format!("{label}{a}")
    }
}

/// A Rust service the test withdraws while a reference to it is kept.
pub struct Spare;

#[rutis::service]
impl Spare {
    pub fn ping(&self) -> &'static str {
        "spare"
    }
}

const PY_SVC: &str = r#"
class Service:
    def boom(self):
        raise KeyError("region")

    def ping(self):
        return "py ok"


provides = {"py_svc": Service}


def apply(ctx, config):
    ctx.provide("py_svc", Service())
"#;

const PY_CALLER: &str = r#"
inject = ["probe", "adder", "js_svc"]


import inspect


async def apply(ctx, config):
    probe = ctx.use("probe")
    adder = ctx.use("adder")
    js = ctx.use("js_svc")

    async def record(what, run):
        try:
            value = run()
            if inspect.isawaitable(value):
                value = await value
            probe.record(f"py {what} returned: {value}")
        except Exception as error:
            probe.record(f"py {what}: {error}")

    await record("wrong argument", lambda: adder.add("x", "n"))
    await record("missing argument", lambda: adder.add(1))
    await record("too many arguments", lambda: adder.add(1, "n", 3))
    await record("thrown", lambda: js.boom())
    await record("undeclared", lambda: ctx.use("go_taker").ping())
    probe.record("py done")
"#;

const JS_SVC: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  provides: { js_svc: { boom: 'sync', ping: 'sync' } },
  apply(ctx) {
    ctx.provide('js_svc', {
      boom: () => { throw new Error('region') },
      ping: () => 'js ok',
    })
  },
})
"#;

const JS_CALLER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['probe', 'adder', 'py_svc'],
  async apply(ctx) {
    const probe = ctx.use('probe'), adder = ctx.use('adder'), py = ctx.use('py_svc')
    const record = async (what, run) => {
      try { probe.record(`js ${what} returned: ${await run()}`) } catch (error) { probe.record(`js ${what}: ${error?.message ?? error}`) }
    }
    await record('wrong argument', () => adder.add('x', 'n'))
    await record('missing argument', () => adder.add(1))
    await record('too many arguments', () => adder.add(1, 'n', 3))
    await record('thrown', () => py.boom())
    await record('undeclared', () => ctx.use('go_taker').ping())
    probe.record('js done')
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
    std::fs::write(py.join("py_svc.py"), PY_SVC).unwrap();
    std::fs::write(py.join("py_caller.py"), PY_CALLER).unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    std::fs::write(js.join("js_svc.mjs"), JS_SVC.replace("PLUGIN", &sdk)).unwrap();
    std::fs::write(js.join("js_caller.mjs"), JS_CALLER.replace("PLUGIN", &sdk)).unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    root.provide(Adder).unwrap();
    let mut catalog = ServiceCatalog::new();
    for name in ["probe", "adder", "py_svc", "js_svc", "go_taker"] {
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

    /// Waits for the line `line`; on a timeout, says what was recorded and
    /// where every row is.
    async fn wait(&self, line: &str) {
        Guard(HANG_GUARD * 3)
            .eventually_with(
                || {
                    self.probe
                        .lines()
                        .iter()
                        .any(|seen| seen == line)
                        .then_some(())
                },
                &format!("the line {line:?}"),
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

    /// The service `name` as the host reaches it, once it is provided.
    async fn service(&self, name: &str) -> Arc<dyn HostDispatch> {
        let key = host_key(name);
        Guard(HANG_GUARD * 3)
            .eventually(
                || self.root.get_at::<dyn HostDispatch>(&key),
                &format!("the service {name}"),
            )
            .await
    }
}

/// What a call to `service.method` with `args` fails with, as text.
async fn failure(service: &Arc<dyn HostDispatch>, method: &str, args: Value) -> String {
    error(service, method, args).await.to_string()
}

/// What a call to `service.method` with `args` fails with.
async fn error(
    service: &Arc<dyn HostDispatch>,
    method: &str,
    args: Value,
) -> rutis_bridge::session::Error {
    let reply = match service.invoke(method, RpcValue::Data(args)) {
        Ok(value) => settle(value).await,
        Err(error) => Err(error),
    };
    reply.expect_err("the call fails")
}

/// The words of `FORBIDDEN` in `text`.
fn forbidden(text: &str) -> Vec<&'static str> {
    FORBIDDEN
        .iter()
        .copied()
        .filter(|word| text.contains(word))
        .collect()
}

/// guarantee: sdk/error-texts
///
/// Each fault of the list, through Node, Python, Go and a Rust service:
/// the messages name the service, the method and the argument, and hold
/// none of the forbidden words.
#[tokio::test(flavor = "multi_thread")]
async fn fault_messages_hold_none_of_the_platforms_words() {
    let fixture = fixture().await;
    let rows = vec![
        named("ps", "py:py_svc"),
        fixture.js("js", "js_svc"),
        named("gt", "go:go_taker"),
        fixture.js("jc", "js_caller"),
        named("pc", "py:py_caller"),
    ];
    fixture.loader.reconcile(layer(rows), None).await.unwrap();

    // From the host: each language's service, called wrongly.
    let mut messages: Vec<(String, String)> = Vec::new();
    for (name, thrown) in [
        ("py_svc", Some("boom")),
        ("js_svc", Some("boom")),
        ("go_taker", None),
    ] {
        let service = fixture.service(name).await;
        let missing = failure(&service, "nope", json!([])).await;
        assert!(
            missing.starts_with(&format!("{name} has no method nope (methods: ")),
            "{missing}"
        );
        messages.push((format!("{name} no method"), missing));
        if let Some(method) = thrown {
            messages.push((
                format!("{name} thrown"),
                failure(&service, method, json!([])).await,
            ));
        }
    }
    let go = fixture.service("go_taker").await;
    messages.push((
        "go_taker too many arguments".into(),
        failure(&go, "ping", json!([1, 2])).await,
    ));
    let adder = fixture.service("adder").await;
    for (what, args, text) in [
        (
            "wrong argument",
            json!(["x", "n"]),
            "adder.add: argument 1 should be an integer, got \"x\"",
        ),
        (
            "missing argument",
            json!([1]),
            "adder.add: argument 2 is missing",
        ),
        (
            "too many arguments",
            json!([1, "n", 3]),
            "adder.add takes 2 arguments, got 3",
        ),
    ] {
        let message = failure(&adder, "add", args).await;
        assert_eq!(message, text);
        messages.push((format!("adder {what}"), message));
    }

    // A service withdrawn while a caller still holds it: `unavailable`.
    let spare = fixture.root.provide(Spare).unwrap();
    let kept = fixture.service("spare").await;
    spare.dispose().await.unwrap();
    let unavailable = error(&kept, "ping", json!([])).await;
    assert_eq!(unavailable.code(), Some("unavailable"));
    let unavailable = unavailable.to_string();
    assert!(
        unavailable.starts_with("service spare is not available"),
        "{unavailable}"
    );
    messages.push(("spare unavailable".into(), unavailable));

    // From the rows: what each language's caller sees.
    fixture.wait("js done").await;
    fixture.wait("py done").await;
    for line in fixture.probe.lines() {
        if line.starts_with("js ") || line.starts_with("py ") {
            messages.push(("row".into(), line));
        }
    }

    let offending: Vec<String> = messages
        .iter()
        .filter(|(_, message)| !forbidden(message).is_empty())
        .map(|(what, message)| format!("{what}: {message:?} holds {:?}", forbidden(message)))
        .collect();
    assert!(
        offending.is_empty(),
        "messages holding forbidden words:\n{}\nall: {messages:#?}",
        offending.join("\n")
    );
    fixture.root.shutdown().await.unwrap();
}
