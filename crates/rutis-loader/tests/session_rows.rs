//! Rows `<connection row>:<plugin>` on the far end of a connection row's
//! session (`docs/design-remote-session-2026-10-28.md` §4.3, §七): they
//! wait for the session (`waiting for gpu`), run once it is there, wait
//! again when it goes; a plugin the far end does not contain fails its row
//! alone, `Unavailable`. The far end is a second rutis kernel with a
//! catalog, over a memory channel.
#![cfg(feature = "runtimes")]

use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView, PluginFactory};
use rutis::Ctx;
use rutis_bridge::session::{call_service, host_key, HostDispatch, Reply, Value as RpcValue};
use rutis_bridge::{
    catalog_key, Channel, Installed, LocalId, PeerId, PluginCatalog, Session, StaticCatalog,
};
use rutis_loader::edit::{Editable, Version};
use rutis_loader::report::{EntryStatus, Unavailable};
use rutis_loader::runtime::SessionResolver;
use rutis_loader::status::describe_waits;
use rutis_loader::{
    Builtins, Chain, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog,
};
use rutis_test_support::{eventually, eventually_with, within};
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// A kernel greeting as `name`.
fn kernel(name: &str) -> Ctx {
    let root = Ctx::root().unwrap();
    root.provide(LocalId(PeerId::new(name).unwrap())).unwrap();
    root
}

// ── the far end ─────────────────────────────────────────────────

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

/// The plugin `contacts`: provides `contacts`.
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

/// The far kernel `office`, which can run `contacts`.
fn office() -> Ctx {
    let office = kernel("office");
    let catalog: Arc<dyn PluginCatalog> =
        Arc::new(StaticCatalog::new().with("contacts", Installed::new(ContactsFactory)));
    office.provide_at(catalog_key(), catalog).unwrap();
    office
}

// ── the connection row ──────────────────────────────────────────

/// The plugin `connection`: mounts a session on each channel it is handed,
/// one at a time, below its row.
struct Connection {
    channels: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Channel>>>,
    current: Current,
}

/// The session a [`Connection`] runs now.
type Current = Arc<Mutex<Option<FiberView>>>;

impl DynPlugin for Connection {
    fn name(&self) -> &str {
        "connection"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, rutis::Error>> {
        Box::pin(async move {
            let (channels, at) = (self.channels.clone(), ctx.clone());
            let current = self.current.clone();
            let task = tokio::spawn(async move {
                let mut channels = channels.lock().await;
                while let Some(channel) = channels.recv().await {
                    let session = at.plugin(Session::new(channel));
                    *current.lock().unwrap() = Some(session.clone());
                    disposed(&session).await;
                }
            });
            let task = Mutex::new(Some(task));
            ctx.effect(move || async move {
                if let Some(task) = task.lock().unwrap().take() {
                    task.abort();
                }
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

struct ConnectionFactory {
    channels: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Channel>>>,
    current: Current,
}

impl PluginFactory<Value> for ConnectionFactory {
    fn name(&self) -> &str {
        "connection"
    }

    fn build(&self, _: &Value) -> Result<Box<dyn DynPlugin>, rutis::Error> {
        Ok(Box::new(Connection {
            channels: self.channels.clone(),
            current: self.current.clone(),
        }))
    }
}

async fn disposed(view: &FiberView) {
    let mut states = view.watch();
    while states.borrow_and_update().state != FiberState::Disposed {
        if states.changed().await.is_err() {
            return;
        }
    }
}

// ── rutis ───────────────────────────────────────────────────────

struct Main {
    root: Ctx,
    loader: Loader,
    connect: mpsc::UnboundedSender<Channel>,
    session: Current,
}

/// rutis `main` with the connection row `gpu` and `rows`, the session
/// resolver last in its chain.
async fn main(rows: Value) -> Main {
    let root = kernel("main");
    let catalog = ServiceCatalog::new();
    let (connect, channels) = mpsc::unbounded_channel();
    let session = Current::default();
    let mut builtins = Builtins::new();
    builtins.register_raw(
        "connection",
        ConnectionFactory {
            channels: Arc::new(tokio::sync::Mutex::new(channels)),
            current: session.clone(),
        },
        None,
    );
    let sessions = Arc::new(SessionResolver::new(&catalog));
    let plugin = LoaderPlugin::new(
        Chain::new().with(builtins).with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    sessions.attach(&loader);
    (&root.plugin(plugin)).await.unwrap();
    let mut all = vec![json!({ "id": "gpu", "name": "connection" })];
    all.extend(rows.as_array().unwrap().iter().cloned());
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": all }])).unwrap();
    loader
        .reconcile(
            vec![Layer::new("rows", patches)],
            Some(Editable::new("rows", Version::default())),
        )
        .await
        .unwrap();
    Main {
        root,
        loader,
        connect,
        session,
    }
}

impl Main {
    /// Connect `gpu` to `office`: the far end's session.
    fn connect(&self, office: &Ctx) -> FiberView {
        let (ours, theirs) = rutis_bridge::memory::pair();
        let far = office.plugin(Session::new(theirs));
        self.connect.send(ours).unwrap();
        far
    }

    fn status(&self, id: &str) -> Option<EntryStatus> {
        self.loader.get(id).map(|entry| entry.status)
    }

    /// The line a waiting row `id` shows, once it waits.
    async fn waiting(&self, id: &str) -> String {
        eventually_with(
            || {
                let entry = self.loader.get(id)?;
                match &entry.status {
                    EntryStatus::Running(snapshot) if snapshot.state == FiberState::Pending => {}
                    _ => return None,
                }
                let waits = self.loader.waits(&entry);
                (!waits.is_empty()).then(|| describe_waits(&waits, &|_| None))
            },
            &format!("{id} waiting"),
            || self.state(),
        )
        .await
    }

    /// Every row and fiber, for a failure.
    fn state(&self) -> String {
        let rows = self.loader.entries().into_iter().map(|entry| {
            let waits = self.loader.waits(&entry);
            format!("{}: {:?} {waits:?}", entry.id, entry.status)
        });
        let plugins = self.root.diagnostics().plugins.into_iter().map(|plugin| {
            format!(
                "plugin {} ({:?}): {:?} {:?} {:?}",
                plugin.name, plugin.row, plugin.state, plugin.error, plugin.injects
            )
        });
        rows.chain(plugins).collect::<Vec<_>>().join("\n")
    }

    async fn running(&self, id: &str) {
        eventually(
            || match self.status(id)? {
                EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active => Some(()),
                _ => None,
            },
            &format!("{id} running"),
        )
        .await
    }

    async fn failed(&self, id: &str) -> LoaderError {
        eventually(
            || match self.status(id)? {
                EntryStatus::Unresolved(error) => Some(error),
                _ => None,
            },
            &format!("{id} failing"),
        )
        .await
    }

    fn contacts(&self) -> Option<Arc<dyn HostDispatch>> {
        self.root.get_at(&host_key("contacts"))
    }
}

/// guarantee: loader/peer-rows, loader/remote-runtime-rows
/// A row on the connection waits for its session, runs once it is there,
/// stops and waits again when it goes, and runs again on the next one; a
/// plugin the far end does not contain fails its row alone.
#[tokio::test(flavor = "multi_thread")]
async fn rows_run_on_the_session_of_their_connection_row() {
    let main = main(json!([
        { "id": "contacts", "name": "gpu:contacts" },
        { "id": "missing", "name": "gpu:missing" }
    ]))
    .await;
    assert_eq!(main.waiting("contacts").await, "waiting for gpu");
    assert_eq!(main.waiting("missing").await, "waiting for gpu");

    let office = office();
    let far = main.connect(&office);
    main.running("contacts").await;
    // Its services come as the far end announces them.
    eventually(|| main.contacts(), "contacts at main").await;
    let answer: String = within("the call", call_service(&main.root, "contacts", "get", ()))
        .await
        .unwrap();
    assert_eq!(answer, "office");
    let missing = main.failed("missing").await;
    assert!(Unavailable::is(&missing), "{missing:?}");
    assert!(
        missing
            .to_string()
            .contains("runtime gpu: this connection does not contain plugin missing"),
        "{missing}"
    );

    // The session unloads: the row stops, its service goes, and it waits
    // again.
    let session = main.session.lock().unwrap().clone().unwrap();
    within("the session unloading", session.dispose())
        .await
        .unwrap();
    drop(far);
    assert_eq!(main.waiting("contacts").await, "waiting for gpu");
    eventually(
        || main.contacts().is_none().then_some(()),
        "contacts gone at main",
    )
    .await;

    // The next session runs it again.
    let _far = main.connect(&office);
    main.running("contacts").await;
    // Its services come as the far end announces them.
    eventually(|| main.contacts(), "contacts at main").await;
    let answer: String = within("the call", call_service(&main.root, "contacts", "get", ()))
        .await
        .unwrap();
    assert_eq!(answer, "office");
    main.root.shutdown().await.unwrap();
}

/// The far end's session unloads first: its rows there end with it, which
/// is a disconnection here, not the row ending on its own: the row waits
/// again and runs on the next session.
#[tokio::test(flavor = "multi_thread")]
async fn a_far_session_unloading_first_leaves_the_row_waiting() {
    let main = main(json!([{ "id": "contacts", "name": "gpu:contacts" }])).await;
    let office = office();
    let far = main.connect(&office);
    main.running("contacts").await;
    eventually(|| main.contacts(), "contacts at main").await;
    within("the far session unloading", far.dispose())
        .await
        .unwrap();
    assert_eq!(main.waiting("contacts").await, "waiting for gpu");
    let _far = main.connect(&office);
    main.running("contacts").await;
    main.root.shutdown().await.unwrap();
}

/// A row whose connection row is not there (yet: it may come in the same
/// batch or later) waits for it, as for a connection not connected.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_whose_connection_row_is_not_there_waits_for_it() {
    let main = main(json!([{ "id": "contacts", "name": "cpu:contacts" }])).await;
    assert_eq!(main.waiting("contacts").await, "waiting for cpu");
    main.root.shutdown().await.unwrap();
}
