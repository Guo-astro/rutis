//! A connection plugin of another language (`docs/design-remote-session-2026-10-28.md`
//! §5.2): a Node row and a Python row `relay` open a session with
//! `sessions.open({ send, close })` over the Rust service `wire`, one end
//! of a memory channel whose other end is a session in the kernel
//! `office`; the row `relay:contacts` then runs `contacts` there.
#![cfg(feature = "runtimes")]

mod common;

use std::sync::{Arc, Mutex};

use common::runtimes::{repo, rows_stage, TestRuntime, COLD};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, PluginFactory};
use rutis::Ctx;
use rutis_bridge::channel::{Channel, Receiver, Sender};
use rutis_bridge::session::{
    call_service, host_key, settle, HostDispatch, Reply, Value as RpcValue,
};
use rutis_bridge::{
    catalog_key, Installed, LocalId, PeerId, PluginCatalog, Session, StaticCatalog,
};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis_test_support::within;
use serde_json::{json, Value};

/// A kernel greeting as `name`.
fn kernel(name: &str) -> Ctx {
    let root = Ctx::root().unwrap();
    root.provide(LocalId(PeerId::new(name).unwrap())).unwrap();
    root
}

// ── the far end: the kernel `office` ────────────────────────────

/// Answers `get` with `office`.
struct Answer;

impl HostDispatch for Answer {
    fn invoke(&self, _method: &str, _: RpcValue) -> Reply {
        Ok(json!("office").into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "get": "async" }))
    }
}

struct Contacts;

impl DynPlugin for Contacts {
    fn name(&self) -> &str {
        "contacts"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, rutis::Error>> {
        Box::pin(async move {
            ctx.provide_at(host_key("contacts"), Arc::new(Answer))?;
            Ok(Effect::Done)
        })
    }
}

struct ContactsFactory;

impl PluginFactory<Value> for ContactsFactory {
    fn name(&self) -> &str {
        "contacts"
    }

    fn build(&self, _: &Value) -> Result<Box<dyn DynPlugin>, rutis::Error> {
        Ok(Box::new(Contacts))
    }
}

/// The kernel `office`, which can run `contacts`, with a session on
/// `channel`.
fn office(channel: Channel) -> Ctx {
    let office = kernel("office");
    let catalog: Arc<dyn PluginCatalog> =
        Arc::new(StaticCatalog::new().with("contacts", Installed::new(ContactsFactory)));
    office.provide_at(catalog_key(), catalog).unwrap();
    office.plugin(Session::new(channel));
    office
}

// ── the service `wire` ──────────────────────────────────────────

/// One end of a channel as a service: `send(bytes)` sends on it,
/// `subscribe(fn)` hands every message it receives to `fn`, in order,
/// each once the previous call settled.
struct Wire {
    sender: Mutex<Box<dyn Sender>>,
    receiver: Mutex<Option<Box<dyn Receiver>>>,
    tokio: tokio::runtime::Handle,
}

impl Wire {
    fn new(channel: Channel) -> Self {
        Self {
            sender: Mutex::new(channel.sender),
            receiver: Mutex::new(Some(channel.receiver)),
            tokio: tokio::runtime::Handle::current(),
        }
    }
}

impl HostDispatch for Wire {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        let first = args.list()?.into_iter().next();
        match (method, first) {
            ("send", Some(RpcValue::Bytes(message))) => {
                self.sender
                    .lock()
                    .unwrap()
                    .send(&message)
                    .map_err(|error| rutis_bridge::session::Error::Value(error.to_string()))?;
                Ok(RpcValue::Undefined)
            }
            ("subscribe", Some(listener)) => {
                let listener = listener.reference()?;
                let mut receiver = self
                    .receiver
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one subscriber");
                let tokio = self.tokio.clone();
                std::thread::spawn(move || {
                    while let Ok(Some(message)) = receiver.recv() {
                        let delivered = tokio.block_on(async {
                            settle(
                                listener
                                    .call_async(RpcValue::List(vec![RpcValue::Bytes(message)]))
                                    .await?,
                            )
                            .await
                        });
                        if delivered.is_err() {
                            return;
                        }
                    }
                });
                Ok(RpcValue::Undefined)
            }
            (method, args) => Err(rutis_bridge::session::Error::Value(format!(
                "wire.{method}({args:?})"
            ))),
        }
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "send": "async", "subscribe": "sync" }))
    }
}

// ── the relays ──────────────────────────────────────────────────

const JS_RELAY: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['sessions', 'wire'],
  async apply(ctx) {
    const sessions = ctx.use('sessions'), wire = ctx.use('wire')
    const near = await sessions.open({
      send: m => wire.send(m),
      close: () => {},
    })
    await wire.subscribe(m => near.receive(m))
  },
})
"#;

const PY_RELAY: &str = r#"
import inspect

inject = ["sessions", "wire"]


async def settled(value):
    if inspect.isawaitable(value):
        return await value
    return value


async def apply(ctx, config):
    sessions = ctx.use("sessions")
    wire = ctx.use("wire")

    async def send(message):
        await settled(wire.send(message))

    async def close(why):
        return None

    near = await settled(sessions.open({"send": send, "close": close}))

    async def receive(message):
        await settled(near["receive"](message))

    await settled(wire.subscribe(receive))
"#;

/// rutis `main` with `sessions`, `wire` and the rows `relay` (named
/// `relay`) and `relay:contacts`, `relay` on the local runtime `runtime`:
/// one session resolver for the runtime's rows and the connection's
/// (with `npm`, the Node names on the runtime too).
async fn main(
    root: &Ctx,
    runtime: TestRuntime,
    npm: bool,
    relay: &str,
    catalog: ServiceCatalog,
) -> Loader {
    root.plugin(Session::service());
    let name = runtime.handle().name().to_owned();
    let sessions = Arc::new(match npm {
        true => SessionResolver::npm(&catalog, &name),
        false => SessionResolver::new(&catalog),
    });
    sessions.runtime(runtime.handle());
    root.plugin(runtime);
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    sessions.attach(&loader);
    (&root.plugin(plugin)).await.unwrap();
    rows_stage(root, &sessions, &name).await;
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
        { "id": "relay", "name": relay, "config": {} },
        { "id": "contacts", "name": "relay:contacts", "config": {} },
    ] }]))
    .unwrap();
    loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    loader
}

fn catalog() -> ServiceCatalog {
    let mut catalog = ServiceCatalog::new();
    for name in ["sessions", "wire"] {
        catalog.register_shared(name);
    }
    catalog
}

/// `main` with `wire` on one end of a channel; `office` on the other.
fn wired() -> (Ctx, Ctx) {
    let (ours, theirs) = rutis_bridge::memory::pair();
    let root = kernel("main");
    root.provide_at(host_key("wire"), Arc::new(Wire::new(ours)))
        .unwrap();
    (root, office(theirs))
}

/// Once `relay:contacts` runs, `contacts` answers from `office`.
async fn contacts_run_there(root: &Ctx, loader: &Loader) {
    COLD.eventually_with(
        || match loader.get("contacts")?.status {
            EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active => Some(()),
            _ => None,
        },
        "relay:contacts running",
        || {
            let rows = loader
                .entries()
                .into_iter()
                .map(|entry| format!("{}: {:?}", entry.id, entry.status));
            let plugins = root.diagnostics().plugins.into_iter().map(|plugin| {
                format!(
                    "plugin {} ({:?}): {:?} {:?}",
                    plugin.name, plugin.row, plugin.state, plugin.error
                )
            });
            rows.chain(plugins).collect::<Vec<_>>().join("\n")
        },
    )
    .await;
    // Its services come as the far end announces them.
    COLD.until(
        || root.get_at(&host_key("contacts")).is_some(),
        "contacts here",
    )
    .await;
    let answer: String = within("the call", call_service(root, "contacts", "get", ()))
        .await
        .unwrap();
    assert_eq!(answer, "office");
}

/// A Node connection plugin opens the session; `relay:contacts` runs on
/// its far end.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_relay_opens_a_session() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    let relay = dir.path().join("relay.mjs");
    std::fs::write(&relay, JS_RELAY.replace("PLUGIN", &sdk)).unwrap();
    let catalog = catalog();
    let (root, office) = wired();
    let node = TestRuntime::node(&repo().join("node/rutis-runtime/package.json"));
    let loader = main(&root, node, true, &relay.to_string_lossy(), catalog).await;
    contacts_run_there(&root, &loader).await;
    root.shutdown().await.unwrap();
    office.shutdown().await.unwrap();
}

/// A Python connection plugin opens the session; `relay:contacts` runs on
/// its far end.
#[tokio::test(flavor = "multi_thread")]
async fn a_python_relay_opens_a_session() {
    if !rutis_test_support::runtimes::enabled("python") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("relay.py"), PY_RELAY).unwrap();
    let catalog = catalog();
    let (root, office) = wired();
    let python = TestRuntime::python(dir.path());
    let loader = main(&root, python, false, "py:relay", catalog).await;
    contacts_run_there(&root, &loader).await;
    root.shutdown().await.unwrap();
    office.shutdown().await.unwrap();
}
