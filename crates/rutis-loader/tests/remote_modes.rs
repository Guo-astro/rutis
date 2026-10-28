//! One set of rows on a remote runtime, whichever way it connects: rutis
//! dials it (a link dialing out), it dials rutis (a link listening), or it
//! comes in many times, one instance per connection (an access point).
//! Discovery and checks are the same code in all three, so a plugin the
//! runtime does not contain fails its row the same way: `Unavailable`, the
//! row alone, and an edit adding such a row is not refused
//! (`docs/design-remote-runtime-consistency-2026-10-26.md` §五, §八).
//! The runtime is a Rust stand-in speaking the row contract in-process.
#![cfg(feature = "runtimes")]

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use common::runtimes::id;
use rutis::host::{BoxFuture, CtxHost, FiberState};
use rutis::internal::CtxInternal;
use rutis::Ctx;
use rutis_bridge::memory::{pair, MemoryPlugin, MemoryTransport};
use rutis_bridge::runtime::{RuntimeAccessPlugin, RuntimePlugin};
use rutis_bridge::session::{
    host_key, Connection, Dispatch, Endpoint, Error, Format, HostDispatch, Reply, Value as RpcValue,
};
use rutis_bridge::{
    protocol, transport_key, AccessPointConfig, AccessPointPlugin, AdmissionPlugin, Channel,
    ConnectError, Credential, Dial, IdentityPlugin, LinkConfig, LinkPlugin, SharedToken,
    StaticIdentity, Transport,
};
use rutis_loader::edit::{Editable, NewEntry, Version};
use rutis_loader::report::{EntryStatus, Unavailable};
use rutis_loader::runtime::{RuntimeInstances, RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::ServiceCatalog;
use rutis_loader::{Chain, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch};
use rutis_test_support::{eventually, within};
use serde_json::{json, Value};

// ── The runtime ─────────────────────────────────────────────────

/// A runtime `rt` with the plugin `contacts`, which provides `contacts`
/// whose `whoami()` answers `rt`. It lists its plugins in the `mount` reply
/// when `lists`; otherwise rutis asks for each (`rows.schema`).
struct Runtime {
    lists: bool,
    tokio: tokio::runtime::Handle,
    version: AtomicU64,
    exports: Mutex<Vec<(String, String)>>,
}

const PLUGINS: [&str; 1] = ["contacts"];

fn schema(plugin: &str) -> Value {
    json!({ "inject": [], "provides": { plugin: { "whoami": "async" } } })
}

impl Runtime {
    fn announce(&self, peer: &Connection, name: &str, handle: Option<String>) {
        let version = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        let (peer, args) = (peer.clone(), json!([name, handle, version]));
        self.tokio.spawn(async move {
            let _ = peer.invoke_async("", "service", args.into()).await;
        });
    }
}

impl Dispatch for Runtime {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: RpcValue) -> Reply {
        let args = args.json().unwrap_or(Value::Null);
        match (target, method) {
            ("", "mount") => {
                let mut reply = json!({
                    "services": {},
                    "features": ["rows.v2", "hosts", "leaf"]
                });
                if self.lists {
                    let plugins: serde_json::Map<String, Value> = PLUGINS
                        .iter()
                        .map(|plugin| (plugin.to_string(), schema(plugin)))
                        .collect();
                    reply["plugins"] = Value::Object(plugins);
                }
                Ok(reply.into())
            }
            ("", "rows.schema") => {
                let plugin = args[0].as_str().unwrap_or_default();
                match PLUGINS.contains(&plugin) {
                    true => Ok(schema(plugin).into()),
                    false => Err(Error::Remote {
                        name: "NotFound".into(),
                        message: format!("no plugin {plugin}"),
                        graph: None,
                    }),
                }
            }
            ("", "rows.load") => {
                let key = args[0].as_str().unwrap().to_owned();
                let names: Vec<String> = args[5]
                    .as_object()
                    .map(|exports| exports.keys().cloned().collect())
                    .unwrap_or_default();
                for name in names {
                    self.announce(peer, &name, Some(format!("{name}@{key}")));
                    self.exports.lock().unwrap().push((key.clone(), name));
                }
                Ok(RpcValue::Undefined)
            }
            ("", "rows.unload") => {
                let key = args[0].as_str().unwrap();
                let mut exports = self.exports.lock().unwrap();
                for (_, name) in exports.iter().filter(|(row, _)| row == key) {
                    self.announce(peer, name, None);
                }
                exports.retain(|(row, _)| row != key);
                Ok(RpcValue::Undefined)
            }
            // `release`, `hosts.provide`, `hosts.withdraw`.
            ("", _) => Ok(RpcValue::Undefined),
            (_, "whoami") => Ok(json!("rt").into()),
            (target, method) => Err(Error::Value(format!("no {target}.{method}"))),
        }
    }
}

/// The runtime's end of `channel`, its session open.
fn serve(channel: Channel, lists: bool) -> Connection {
    let mut endpoint = Endpoint::rust(id("rt")).expect(id("main"));
    endpoint.capabilities.push("runtime".into());
    let runtime = Runtime {
        lists,
        tokio: tokio::runtime::Handle::current(),
        version: AtomicU64::new(0),
        exports: Mutex::default(),
    };
    Connection::open_with(channel, Arc::new(runtime), Format::Endpoint(endpoint)).unwrap()
}

/// A transport whose every dial reaches a new stand-in runtime: what rutis
/// dials out to.
struct ToRuntime {
    lists: bool,
    sessions: Mutex<Vec<Connection>>,
}

impl Transport for ToRuntime {
    fn kind(&self) -> &str {
        "to-runtime"
    }

    fn dial<'a>(&'a self, _dial: &'a Dial) -> BoxFuture<'a, Result<Channel, ConnectError>> {
        Box::pin(async move {
            let (ours, theirs) = pair();
            self.sessions
                .lock()
                .unwrap()
                .push(serve(theirs, self.lists));
            Ok(ours)
        })
    }
}

impl ToRuntime {
    fn close(&self) {
        for session in self.sessions.lock().unwrap().drain(..) {
            session.close(Error::Transport("gone".into()));
        }
    }
}

// ── The three ways ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
enum Way {
    /// rutis dials the runtime: a link dialing out.
    Dial,
    /// The runtime dials rutis: a link listening.
    Listen,
    /// The runtime comes in many times: an access point, one instance of
    /// the `rt` group per connection.
    Instances,
}

struct Main {
    root: Ctx,
    loader: Loader,
    way: Way,
    /// The runtime's sessions, to end them.
    end: Box<dyn Fn() + Send + Sync>,
}

/// rutis with the rows `rt:contacts` and `rt:missing`, and the runtime
/// connected the way `way` says.
async fn main(way: Way, lists: bool) -> Main {
    let root = Ctx::root().unwrap();
    let rows = json!([
        { "id": "contacts", "name": "rt:contacts" },
        { "id": "missing", "name": "rt:missing" }
    ]);
    let mut catalog = ServiceCatalog::new();
    catalog.share_by_name();
    let (chain, rows, single) = match way {
        Way::Instances => {
            let instances = RuntimeInstances::new("rt", "rt", &catalog);
            let group = json!([{ "id": "rt", "group": true, "instanced": true, "config": rows }]);
            (Chain::new().with(instances.clone()), group, Err(instances))
        }
        Way::Dial | Way::Listen => {
            let runtime = RuntimePlugin::remote("rt");
            let resolver =
                Arc::new(RuntimeResolver::modules(runtime.handle()).with_catalog(&catalog));
            root.plugin(runtime);
            (
                Chain::new().with_shared(resolver.clone()),
                rows,
                Ok(resolver),
            )
        }
    };
    let plugin = LoaderPlugin::new(
        chain,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    (&root.plugin(plugin)).await.unwrap();
    if let Ok(resolver) = &single {
        root.plugin(RuntimeRowsPlugin::new(resolver.clone()));
    }
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    loader
        .reconcile(
            vec![Layer::new("rows", patches)],
            Some(Editable::new("rows", Version::default())),
        )
        .await
        .unwrap();

    let end: Box<dyn Fn() + Send + Sync> = match way {
        Way::Dial => {
            let transport = Arc::new(ToRuntime {
                lists,
                sessions: Mutex::default(),
            });
            let provided: Arc<dyn Transport> = transport.clone();
            root.provide_at(transport_key("to-runtime"), provided)
                .unwrap();
            root.plugin(IdentityPlugin::new(
                "main",
                StaticIdentity::new(id("main"))
                    .present(id("rt"), Credential::Bearer("rt-token".into())),
            ));
            root.plugin(LinkPlugin::new(
                LinkConfig::dial(id("rt"), "to-runtime", "main", "rt").require("runtime"),
            ));
            root.plugin(RuntimeAccessPlugin::new(id("rt"), "rt"));
            Box::new(move || transport.close())
        }
        Way::Listen | Way::Instances => {
            let memory = Arc::new(MemoryTransport::default());
            memory.endpoint("main-in", id("main"));
            (&root.plugin(MemoryPlugin::with_transport(memory.clone())))
                .await
                .unwrap();
            match (way, single) {
                (Way::Instances, Err(instances)) => {
                    (&root.plugin(AdmissionPlugin::new(
                        "rt",
                        SharedToken::new(id("main"), "rt-token"),
                    )))
                        .await
                        .unwrap();
                    let access = AccessPointPlugin::new(
                        AccessPointConfig::new(id("rt"), "memory", "rt", "main-in")
                            .require("runtime"),
                    )
                    .accept(instances.accept(loader.clone(), root.clone()));
                    (&root.plugin(access)).await.unwrap();
                }
                _ => {
                    root.plugin(IdentityPlugin::new(
                        "main",
                        StaticIdentity::new(id("main")).accept_token("rt-token", id("rt")),
                    ));
                    root.plugin(LinkPlugin::new(
                        LinkConfig::listen(id("rt"), "memory", "main", "main-in")
                            .require("runtime"),
                    ));
                    root.plugin(RuntimeAccessPlugin::new(id("rt"), "rt"));
                }
            }
            let session = dial_in(&memory, lists).await;
            Box::new(move || session.close(Error::Transport("gone".into())))
        }
    };
    Main {
        root,
        loader,
        way,
        end,
    }
}

/// The runtime dials rutis at `main-in`, once rutis listens for it.
async fn dial_in(memory: &Arc<MemoryTransport>, lists: bool) -> Connection {
    let identity = Arc::new(
        StaticIdentity::new(id("rt")).present(id("main"), Credential::Bearer("rt-token".into())),
    );
    let dial = Dial::address("main-in")
        .peer(id("main"))
        .identity(identity)
        .protocol(protocol());
    let channel = within("rutis listening", async {
        loop {
            match memory.dial(&dial).await {
                Ok(channel) => break channel,
                Err(ConnectError::Retryable { .. }) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await
                }
                Err(error) => panic!("refused: {error}"),
            }
        }
    })
    .await;
    let session = serve(channel, lists);
    within("the runtime's session", session.ready())
        .await
        .unwrap();
    session
}

impl Main {
    /// The status of row `id`: its one copy (in the connection's instance
    /// for [`Way::Instances`]).
    fn status(&self, id: &str) -> Option<EntryStatus> {
        let instanced = matches!(self.way, Way::Instances);
        self.loader
            .entries()
            .into_iter()
            .find(|entry| entry.id == id && entry.instance.is_some() == instanced)
            .map(|entry| entry.status)
    }

    async fn running(&self, id: &str) {
        eventually(
            || match self.status(id)? {
                EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active => Some(()),
                _ => None,
            },
            &format!("{id} running ({:?})", self.way),
        )
        .await
    }

    /// Wait for row `id` to fail, and say why.
    async fn failed(&self, id: &str) -> LoaderError {
        eventually(
            || match self.status(id)? {
                EntryStatus::Unresolved(error) => Some(error),
                _ => None,
            },
            &format!("{id} failing ({:?})", self.way),
        )
        .await
    }

    /// The service `contacts` the row of `rt:contacts` provides, where
    /// rutis can call it: at the root, for one runtime. A runtime that comes
    /// in many times provides it in each instance alone
    /// (`runtime_instances.rs` uses it there).
    fn contacts(&self) -> Option<Arc<dyn HostDispatch>> {
        self.root
            .get_key::<dyn HostDispatch>(host_key("contacts").erase())
    }

    /// Whether the runtime's row runs: its service is there, or, inside
    /// instances, its copy.
    fn connected(&self) -> bool {
        match self.way {
            Way::Instances => self.status("contacts").is_some(),
            _ => self.contacts().is_some(),
        }
    }
}

/// What happens to the rows over `way`: the missing plugin's error, and the
/// one an edit added.
async fn rows_over(way: Way, lists: bool) -> (String, String) {
    let main = main(way, lists).await;
    main.running("contacts").await;
    if !matches!(way, Way::Instances) {
        let contacts = eventually(|| main.contacts(), &format!("the service ({way:?})")).await;
        let call = contacts.invoke("whoami", RpcValue::List(vec![])).unwrap();
        let whoami = rutis_bridge::session::settle(call).await.unwrap();
        assert_eq!(whoami.json().unwrap(), json!("rt"));
    }

    let missing = main.failed("missing").await;
    assert!(Unavailable::is(&missing), "{way:?}: {missing:?}");

    // An edit adding a row the runtime does not contain is not refused.
    main.loader
        .create(
            NewEntry {
                id: Some("extra".into()),
                name: "rt:extra".into(),
                ..NewEntry::default()
            },
            matches!(way, Way::Instances).then_some("rt"),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("{way:?}: the edit was refused: {error}"));
    let extra = main.failed("extra").await;
    assert!(Unavailable::is(&extra), "{way:?}: {extra:?}");
    main.running("contacts").await;

    // The runtime goes: its row stops, and its service with it.
    (main.end)();
    eventually(
        || (!main.connected()).then_some(()),
        &format!("the row stopped ({way:?})"),
    )
    .await;
    main.root.shutdown().await.unwrap();
    (missing.to_string(), extra.to_string())
}

/// The missing plugin fails its row the same way over every way, whether
/// the runtime lists its plugins in its `mount` reply or is asked.
/// guarantee: loader/remote-runtime-rows
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_plugin_fails_the_same_way_however_the_runtime_connects() {
    for lists in [true, false] {
        let mut seen = Vec::new();
        for way in [Way::Dial, Way::Listen, Way::Instances] {
            let (missing, extra) = rows_over(way, lists).await;
            assert!(
                missing.contains("runtime rt: this connection does not contain plugin missing"),
                "{way:?}: {missing}"
            );
            assert!(
                extra.contains("runtime rt: this connection does not contain plugin extra"),
                "{way:?}: {extra}"
            );
            seen.push((way, missing, extra));
        }
        let (_, first_missing, first_extra) = &seen[0];
        for (way, missing, extra) in &seen {
            assert_eq!(missing, first_missing, "{way:?}");
            assert_eq!(extra, first_extra, "{way:?}");
        }
    }
}
