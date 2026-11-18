//! Wire-level conformance for the event rules the tracking table still marks
//! partly or not verified (`docs/formal-trace/rules/protocol-events.md`): a
//! host on a memory session with a far end scripted frame by frame, so what
//! is checked is the wire itself, not an internal helper.
//!
//! The far end is a fake runtime: it registers listeners, dispatches, and
//! answers `events.call` as the test tells it to. Rules that name another
//! implementation (Node's Cordis `internal/dispatch`, Go's filter errors, a
//! JavaScript listener's signal), need a real runtime, or need a linked node
//! stay open; the PR body lists them. Rules the 0.19 flat bus withdrew
//! (`events.elsewhere` and the cross-session guarantee built on it, §4.4
//! steps 3 and 5 and §4.5) are not checked here either: the tracking
//! table's wording predates the bus.
//!
//! rule: EVP-3-2
//! rule: EVP-3-5
//! rule: EVP-3-10
//! rule: EVP-4.1-3
//! rule: EVP-4.1-6
//! rule: EVP-4.2-1
//! rule: EVP-4.2-2
//! rule: EVP-4.2-4
//! rule: EVP-4.2-5
//! rule: EVP-4.2-10
//! rule: EVP-4.3-1
//! rule: EVP-4.3-2
//! rule: EVP-4.3-3
//! rule: EVP-5.1-1
//! rule: EVP-5.1-3
//! rule: EVP-5.3-2
//! rule: EVP-5.3-6
//! rule: EVP-5.3-7
//! rule: EVP-5.4-1
//! rule: EVP-5.4-2
//! rule: EVP-5.4-7
//! rule: EVP-5.4-14
//! rule: EVP-5.5-5
//! rule: EVP-5.5-6
//! rule: EVP-5.6-5
//! rule: EVP-5.7-4
//! rule: EVP-6-6
//! rule: EVP-6-10
//! rule: EVP-7.1-1
//! rule: EVP-7.1-2
//! rule: EVP-8-3
//! rule: EVP-8-7
//! rule: EVP-10-1
//! rule: EVP-10-2
//! rule: EVP-11-3
//! rule: EVP-11-4
//! rule: EVP-11-7
//! rule: EVP-11-8
//! rule: EVP-12.2-9
//!
//! guarantee: events/hub-reserved
//! guarantee: events/hub-flat
//! guarantee: events/hub-filter
//! guarantee: events/hub-dispatch
//! guarantee: events/hub-lifecycle
//! guarantee: events/remote
//! guarantee: events/framework
//! guarantee: events/order
//! guarantee: events/waterfall
//! guarantee: events/emit

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rutis::events::{Call, Scope, SyncCall};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Event};
use rutis_bridge::channel::PeerId;
use rutis_bridge::events::{ArgValue, Args};
use rutis_bridge::runtime::Process;
use rutis_bridge::session::{
    host_key, settle, Connection, Dispatch, Endpoint, Error, HostDispatch, Reference, Reply, Value,
};
use rutis_test_support::{eventually, until, within};
use serde_json::{json, Value as Json};

mod common;

fn id(name: &str) -> PeerId {
    PeerId::new(name).unwrap()
}

/// What a fake listener does when called.
type Behavior = Arc<dyn Fn(&BTreeMap<String, Value>) -> Reply + Send + Sync>;

/// A runtime that mounts nothing, scripted by the test: it records the rows
/// it is told to load and every `events.call` it gets, and answers by its
/// listener's behavior.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<BTreeMap<String, Value>>>,
    listeners: Mutex<BTreeMap<String, Behavior>>,
    loads: Mutex<Vec<Vec<Value>>>,
}

impl Dispatch for Fake {
    fn invoke(&self, _: &Connection, target: &str, method: &str, args: Value) -> Reply {
        let first = || -> Result<Value, Error> {
            Ok(args
                .clone()
                .list()?
                .into_iter()
                .next()
                .unwrap_or(Value::Undefined))
        };
        match (target, method) {
            ("", "mount") => Ok(Value::Data(
                json!({ "services": {}, "features": ["rows.v2", "hosts"] }),
            )),
            ("", "rows.load") => {
                self.loads.lock().unwrap().push(args.list()?);
                Ok(Value::Undefined)
            }
            ("", "rows.unload") => Ok(Value::Undefined),
            ("", "ping") => Ok(Value::Data(json!("pong"))),
            ("", "events.call") => {
                let Value::Record(fields) = first()? else {
                    return Err(Error::value("events.call needs a record"));
                };
                let listener = fields["listener"].clone().json()?;
                let behavior = self
                    .listeners
                    .lock()
                    .unwrap()
                    .get(listener.as_str().unwrap())
                    .cloned();
                self.calls.lock().unwrap().push(fields.clone());
                match behavior {
                    Some(behavior) => behavior(&fields),
                    None => Err(Error::Remote {
                        name: "BindingError".into(),
                        message: format!("listener {listener} is withdrawn"),
                        graph: None,
                    }),
                }
            }
            _ => Err(Error::value(format!("no method {method}"))),
        }
    }
}

/// The far end's channel, the frames it reads recorded in order: the wire
/// itself, before any plugin code runs.
struct Recorded {
    inner: Box<dyn rutis_bridge::channel::Receiver>,
    frames: Arc<Mutex<Vec<Json>>>,
}

impl rutis_bridge::channel::Receiver for Recorded {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, rutis_bridge::channel::ChannelError> {
        let message = self.inner.recv()?;
        if let Some(message) = &message {
            if let Ok(frame) = serde_json::from_slice::<Json>(message) {
                self.frames.lock().unwrap().push(frame);
            }
        }
        Ok(message)
    }
}

/// The host (`root`'s bus) and the fake runtime `name` mounted on it.
struct Rig {
    root: Ctx,
    process: Arc<Process>,
    fake: Arc<Fake>,
    far: Connection,
    /// Every frame the host wrote to `far`, in order.
    frames: Arc<Mutex<Vec<Json>>>,
}

async fn rig_on(root: Ctx, name: &str) -> Rig {
    rig_endpoint(root, name, Endpoint::rust(id(name))).await
}

async fn rig_endpoint(root: Ctx, name: &str, mut endpoint: Endpoint) -> Rig {
    let (a, b) = rutis_bridge::memory::pair();
    let fake = Arc::new(Fake::default());
    let frames: Arc<Mutex<Vec<Json>>> = Arc::default();
    let far = rutis_bridge::Channel {
        sender: b.sender,
        receiver: Box::new(Recorded {
            inner: b.receiver,
            frames: frames.clone(),
        }),
        closer: b.closer,
    };
    // A language runtime, which the session mounts a process on.
    endpoint.capabilities.push("runtime".into());
    let far = Connection::open(far, fake.clone(), endpoint).unwrap();
    let process = common::runtime::attached(&root, name, a)
        .await
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    Rig {
        root,
        process,
        fake,
        far,
        frames,
    }
}

/// A root whose error sink records what a host reports, for the §5.6 lines.
fn capturing_root() -> (Ctx, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let errors: Arc<Mutex<Vec<Arc<ErrorKind>>>> = Arc::default();
    let into = errors.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |error| into.lock().unwrap().push(error)),
    );
    common::runtime::greet_as_main(&root);
    (root, errors)
}

async fn rig() -> Rig {
    rig_on(common::runtime::root(), "rt").await
}

impl Rig {
    async fn send(&self, method: &str, argument: Value) -> Result<Value, Error> {
        let reply = self
            .far
            .invoke_async("", method, Value::List(vec![argument]))
            .await?;
        settle(reply).await
    }

    /// Queue the frame now and hand back its reply: dropping the reply
    /// cancels the call, which is what an emitter that gives up does.
    fn started(
        &self,
        method: &str,
        argument: Value,
    ) -> Result<impl Future<Output = Reply> + Send + 'static, Error> {
        self.far
            .invoke_started("", method, Value::List(vec![argument]))
    }

    /// Load row `key`, named `id` (its configuration's id) when given, in
    /// `scope`.
    async fn load(&self, key: &str, id: Option<&str>, scope: &[Scope]) {
        self.load_with(key, id, &[], scope).await;
    }

    /// [`Rig::load`], with the `isolate` labels a row keeps its services to.
    async fn load_with(
        &self,
        key: &str,
        id: Option<&str>,
        isolate: &[(&str, &str)],
        scope: &[Scope],
    ) {
        if let Some(id) = id {
            self.process.name_row(key, id);
        }
        let isolate: Vec<(String, String)> = isolate
            .iter()
            .map(|(name, label)| ((*name).into(), (*label).into()))
            .collect();
        self.process
            .load_row(key, Path::new("fake"), json!({}), &isolate, &[], scope)
            .await
            .unwrap();
    }

    /// Register listener `id` of `name` from row `row` for the fake.
    async fn register(
        &self,
        id: &str,
        name: &str,
        row: Option<&str>,
        plugin: &str,
        kind: &str,
    ) -> Result<Value, Error> {
        self.register_with(id, name, row, plugin, kind, quiet())
            .await
    }

    async fn register_with(
        &self,
        id: &str,
        name: &str,
        row: Option<&str>,
        plugin: &str,
        kind: &str,
        behavior: Behavior,
    ) -> Result<Value, Error> {
        self.register_full(id, name, row, plugin, kind, false, behavior)
            .await
    }

    /// Register the quiet listener `id` of `name` from row `row`, before
    /// the entries already in the table (`prepend`, events protocol §4.3).
    async fn register_prepend(
        &self,
        id: &str,
        name: &str,
        row: Option<&str>,
        plugin: &str,
        kind: &str,
    ) -> Result<Value, Error> {
        self.register_full(id, name, row, plugin, kind, true, quiet())
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn register_full(
        &self,
        id: &str,
        name: &str,
        row: Option<&str>,
        plugin: &str,
        kind: &str,
        prepend: bool,
        behavior: Behavior,
    ) -> Result<Value, Error> {
        self.fake
            .listeners
            .lock()
            .unwrap()
            .insert(id.into(), behavior);
        self.send(
            "events.on",
            Value::Data(json!({
                "id": id, "name": name, "row": row, "plugin": plugin,
                "kind": kind, "prepend": prepend, "global": false,
            })),
        )
        .await
    }

    /// `events.dispatch` from `row`, waiting for the answer.
    async fn dispatch(
        &self,
        row: Option<&str>,
        name: &str,
        mode: &str,
        args: Vec<Value>,
    ) -> Result<Value, Error> {
        let fields = request(row, name, mode, args);
        self.send("events.dispatch", Value::Record(fields)).await
    }

    fn dispatch_started(
        &self,
        row: Option<&str>,
        name: &str,
        mode: &str,
        args: Vec<Value>,
        filter: Option<Value>,
    ) -> Result<impl Future<Output = Reply> + Send + 'static, Error> {
        self.dispatch_with(row, name, mode, args, filter, None)
    }

    /// [`Rig::dispatch_started`], a waterfall's `terminal` included.
    fn dispatch_with(
        &self,
        row: Option<&str>,
        name: &str,
        mode: &str,
        args: Vec<Value>,
        filter: Option<Value>,
        terminal: Option<Value>,
    ) -> Result<impl Future<Output = Reply> + Send + 'static, Error> {
        let mut fields = request(row, name, mode, args);
        if let Some(filter) = filter {
            fields.insert("filter".into(), filter);
        }
        if let Some(terminal) = terminal {
            fields.insert("terminal".into(), terminal);
        }
        self.started("events.dispatch", Value::Record(fields))
    }

    /// [`Rig::dispatch_with`], its reply settled on a task of its own: a
    /// dispatch that is not an `emit` does its work while its reply is
    /// settled (it is started as a task, events protocol §4.1), so a test
    /// that watches it run must settle it meanwhile.
    fn dispatch_running(
        &self,
        row: Option<&str>,
        name: &str,
        mode: &str,
        args: Vec<Value>,
        filter: Option<Value>,
        terminal: Option<Value>,
    ) -> tokio::task::JoinHandle<Reply> {
        let started = self
            .dispatch_with(row, name, mode, args, filter, terminal)
            .unwrap();
        tokio::spawn(async move { settle(started.await?).await })
    }
}

/// The listeners of the `events.call` frames for `name` the far end read,
/// in the order they arrived.
fn calls_for(frames: &Arc<Mutex<Vec<Json>>>, name: &str) -> Vec<String> {
    frames
        .lock()
        .unwrap()
        .iter()
        .filter(|frame| {
            frame["op"] == json!("invoke")
                && frame["method"] == json!("events.call")
                && frame["args"]["value"][0]["value"]["name"]["value"] == json!(name)
        })
        .map(called)
        .collect()
}

/// The listeners of the calls the fake got, in order.
fn listeners(fake: &Fake) -> Vec<String> {
    fake.calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| {
            call["listener"]
                .clone()
                .json()
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn request(row: Option<&str>, name: &str, mode: &str, args: Vec<Value>) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    fields.insert("row".into(), Value::Data(json!(row)));
    fields.insert("name".into(), Value::Data(json!(name)));
    fields.insert("mode".into(), Value::Data(json!(mode)));
    fields.insert("args".into(), Value::List(args));
    fields
}

/// A listener that answers nothing.
fn quiet() -> Behavior {
    Arc::new(|_| Ok(Value::Undefined))
}

fn scope(chain: &[(&str, &str)]) -> Vec<Scope> {
    chain
        .iter()
        .map(|(id, group)| Scope {
            id: (*id).into(),
            group: (*group).into(),
        })
        .collect()
}

/// The listener id an `events.call` frame names.
fn called(frame: &Json) -> String {
    frame["args"]["value"][0]["value"]["listener"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("an events.call frame: {frame:#?}"))
        .to_owned()
}

/// The number of a dispatch id (`d<number>`), or `None`.
fn dispatch_number(value: &Value) -> Option<u64> {
    match value {
        Value::Data(Json::String(id)) => id.strip_prefix('d')?.parse().ok(),
        _ => None,
    }
}

/// The `name` and `message` of a `BindingError` the host threw.
fn binding(error: &Error) -> (&str, &str) {
    match error {
        Error::Remote { name, message, .. } => (name, message),
        other => panic!("expected a BindingError, got {other:?}"),
    }
}

/// A typed event of this host, for a dispatch that does not come from a
/// session.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Happened(String);

impl Event for Happened {
    const NAME: &'static str = "t/id";
    type Value = ();
}

/// A host listener that answers nothing.
async fn hear(_: Arc<Args>, _: Call<Args>) -> Result<Option<ArgValue>, rutis::Error> {
    Ok(None)
}

/// EVP-4.2-2, EVP-8-7
///
/// `internal/*` belongs to the runtime that names it: the host refuses to
/// register such a listener, and refuses a dispatch of such a name, with
/// the protocol's one message. The far end never gets an entry into the
/// host's table for one.
#[tokio::test(flavor = "multi_thread")]
async fn internal_names_are_refused_and_never_registered() {
    within("internal names", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        let refused = rig
            .register("1", "internal/state", Some("chat"), "fake", "any")
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            (
                "BindingError",
                "event internal/state is internal to its runtime"
            )
        );
        let refused = rig
            .dispatch(Some("chat"), "internal/state", "serial", vec![])
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            (
                "BindingError",
                "event internal/state is internal to its runtime"
            )
        );
        assert!(
            rig.fake.calls.lock().unwrap().is_empty(),
            "no listener of the name is on the bus"
        );
    })
    .await;
}

/// EVP-3-5
///
/// The host tells every row its scope chain and its id in the seventh
/// `rows.load` argument: seven arguments always, `exports` as `{}` when the
/// row exports none, and the configured id (the key when it has none).
#[tokio::test(flavor = "multi_thread")]
async fn rows_load_carries_seven_arguments_with_the_scope_and_the_id() {
    within("rows.load", async {
        let rig = rig().await;
        rig.load("plain", None, &[]).await;
        let chain = scope(&[("17", "tenant"), ("42", "session")]);
        rig.load("chat", Some("chat-ui"), &chain).await;
        let loads = rig.fake.loads.lock().unwrap().clone();
        assert_eq!(loads.len(), 2, "{loads:?}");
        let plain = &loads[0];
        assert_eq!(plain.len(), 7, "{plain:?}");
        assert_eq!(plain[0], Value::Data(json!("plain")));
        assert_eq!(plain[3], Value::Data(json!([])), "no isolate: {plain:?}");
        assert_eq!(plain[4], Value::Data(json!([])), "no inject: {plain:?}");
        assert_eq!(plain[5], Value::Data(json!({})), "no exports: {plain:?}");
        assert_eq!(
            plain[6],
            Value::Data(json!({ "scope": [], "id": "plain" })),
            "{plain:?}"
        );
        let named = &loads[1];
        assert_eq!(named.len(), 7, "{named:?}");
        assert_eq!(
            named[6],
            Value::Data(json!({
                "scope": [
                    { "id": "17", "group": "tenant" },
                    { "id": "42", "group": "session" },
                ],
                "id": "chat-ui",
            })),
            "{named:?}"
        );
    })
    .await;
}

/// EVP-3-2, EVP-4.2-5, EVP-4.2-10
///
/// A filter sees each candidate's descriptor: the event name, whether it is
/// a prefix entry, the registering plugin's name, the row's configured id
/// (not its key), the runtime's name (the host's own Rust listener has
/// none), the scope chain outer first and `[]` at the root, and `global`.
/// The emitter's own entry also carries its listener id; a prefix entry of
/// this host is `prefix: true`. A `global` entry is filtered out before the
/// filter is called (§6), so its descriptor never reaches one: this checks
/// `global: false` for the entries that do.
#[tokio::test(flavor = "multi_thread")]
async fn descriptors_carry_the_registration_and_the_scope() {
    within("descriptors", async {
        let rig = rig().await;
        let chain = scope(&[("17", "tenant"), ("42", "session")]);
        rig.load("chat", Some("chat-ui"), &chain).await;
        rig.register("7", "t/desc", Some("chat"), "chat-plugin", "any")
            .await
            .unwrap();
        // A Rust listener of this host, a prefix entry, at the root.
        rig.root.listen::<Args>().prefix("t/").on(hear).unwrap();
        let seen: Arc<Mutex<Vec<Json>>> = Arc::default();
        let recorded = seen.clone();
        let selector = Value::callback(move |argument| {
            let descriptors = argument.list()?.remove(0).json()?;
            recorded.lock().unwrap().push(descriptors.clone());
            let answers = descriptors.as_array().unwrap().len();
            Ok(Value::Data(json!(vec![false; answers])))
        });
        let result = rig
            .dispatch_started(Some("chat"), "t/desc", "serial", vec![], Some(selector))
            .unwrap()
            .await;
        let result = settle(result.unwrap()).await.unwrap();
        assert_eq!(result, Value::Undefined);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "{seen:?}");
        let described = &seen[0];
        assert_eq!(described[0]["name"], json!("t/desc"));
        assert_eq!(described[0]["prefix"], json!(false));
        assert_eq!(described[0]["plugin"], json!("chat-plugin"));
        assert_eq!(described[0]["row"], json!("chat-ui"), "the row id");
        assert_eq!(described[0]["runtime"], json!("rt"));
        assert_eq!(described[0]["node"], Json::Null);
        assert_eq!(
            described[0]["scope"],
            json!([
                { "id": "17", "group": "tenant" },
                { "id": "42", "group": "session" },
            ])
        );
        assert_eq!(described[0]["global"], json!(false));
        assert_eq!(described[0]["listener"], json!("7"));
        // The host's own: no runtime, no row, the root's scope; a prefix.
        assert_eq!(described[1]["name"], json!("t/"));
        assert_eq!(described[1]["prefix"], json!(true));
        assert_eq!(described[1]["runtime"], Json::Null);
        assert_eq!(described[1]["node"], Json::Null);
        assert_eq!(described[1]["row"], Json::Null);
        assert_eq!(described[1]["scope"], json!([]));
        assert_eq!(described[1]["global"], json!(false));
    })
    .await;
}

/// EVP-4.2-4
///
/// A listener belongs to a row loaded on this session, by its key; the
/// mount's own plugins write `row: null`, and their descriptor has no row.
/// A row this session does not have is refused with the protocol's message.
#[tokio::test(flavor = "multi_thread")]
async fn a_listener_belongs_to_a_row_of_this_session() {
    within("listener rows", async {
        let rig = rig().await;
        let refused = rig
            .register("1", "t/row", Some("elsewhere"), "fake", "any")
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            (
                "BindingError",
                "the calling row is not loaded in this runtime"
            )
        );
        // The mount's own plugins: `row: null`.
        rig.register("2", "t/row", None, "fake", "any")
            .await
            .unwrap();
        let seen: Arc<Mutex<Vec<Json>>> = Arc::default();
        let recorded = seen.clone();
        let selector = Value::callback(move |argument| {
            let descriptors = argument.list()?.remove(0).json()?;
            recorded.lock().unwrap().push(descriptors.clone());
            Ok(Value::Data(json!([false])))
        });
        let result = rig
            .dispatch_started(None, "t/row", "serial", vec![], Some(selector))
            .unwrap()
            .await;
        assert_eq!(settle(result.unwrap()).await.unwrap(), Value::Undefined);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0][0]["row"], Json::Null, "{seen:?}");
        assert_eq!(seen[0][0]["listener"], json!("2"), "{seen:?}");
    })
    .await;
}

/// EVP-5.3-6, EVP-5.3-7
///
/// One dispatch names its calls alike: the same id for every listener,
/// number in one host and never reused; another dispatch gets another. The
/// same id reaches listeners of two sessions, and `own` tells them apart:
/// the dispatching session's entry is its own, the other session's and the
/// host's own emissions are not.
#[tokio::test(flavor = "multi_thread")]
async fn a_dispatch_names_its_calls_and_marks_its_own_session() {
    within("dispatch identity", async {
        let root = common::runtime::root();
        let one = rig_on(root.clone(), "rt1").await;
        let two = rig_on(root.clone(), "rt2").await;
        one.load("chat", None, &[]).await;
        two.load("chat", None, &[]).await;
        one.register("11", "t/id", Some("chat"), "one", "any")
            .await
            .unwrap();
        two.register("22", "t/id", Some("chat"), "two", "any")
            .await
            .unwrap();
        let result = one
            .dispatch(Some("chat"), "t/id", "serial", vec![json!("x").into()])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        let first = one.fake.calls.lock().unwrap()[0].clone();
        let second = two.fake.calls.lock().unwrap()[0].clone();
        assert_eq!(first["own"], Value::Data(json!(true)), "{first:?}");
        assert_eq!(second["own"], Value::Data(json!(false)), "{second:?}");
        assert_eq!(
            second["dispatch"], first["dispatch"],
            "{first:?} {second:?}"
        );
        let number = dispatch_number(&first["dispatch"]).expect("d<number>");
        // Another dispatch: another id.
        one.dispatch(Some("chat"), "t/id", "serial", vec![])
            .await
            .unwrap();
        let again = one.fake.calls.lock().unwrap()[1].clone();
        assert_ne!(again["dispatch"], first["dispatch"]);
        assert_ne!(dispatch_number(&again["dispatch"]).unwrap(), number);
        // A host emission is no session's own. It is a dispatch of its own,
        // so its call is found by what it carries: a count of the calls here
        // is either the second dispatch's call, or skipped past when the
        // host's lands with it (an `emit` starts its calls before it
        // answers, §5.4).
        root.emit(Happened("host".into())).unwrap();
        let host = Value::List(vec![json!("host").into()]);
        let heard = eventually(
            || {
                two.fake
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|call| call.get("args") == Some(&host))
                    .cloned()
            },
            "the host's emit at rt2",
        )
        .await;
        assert_eq!(heard["own"], Value::Data(json!(false)), "{heard:?}");
        assert_ne!(
            heard["dispatch"], again["dispatch"],
            "the host's emission is a dispatch of its own: {heard:?}"
        );
    })
    .await;
}

/// EVP-5.4-2
///
/// An `emit` answers once every one of its calls has started, in
/// registration order, and with the plain `undefined`: the frames the far
/// end reads are the calls first, then the reply.
#[tokio::test(flavor = "multi_thread")]
async fn an_emit_answers_after_all_its_calls_started() {
    within("emit reply", async {
        let rig = rig().await;
        rig.load("a", None, &[]).await;
        rig.load("b", None, &[]).await;
        rig.register("1", "t/emit", Some("a"), "one", "any")
            .await
            .unwrap();
        rig.register("2", "t/emit", Some("b"), "two", "any")
            .await
            .unwrap();
        let result = rig
            .dispatch(Some("a"), "t/emit", "emit", vec![json!(1).into()])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        // The frames the far end read, in order: this dispatch ends with
        // its two calls and then the answer, the calls first.
        let frames = rig.frames.lock().unwrap().clone();
        let tail = &frames[frames.len() - 3..];
        assert_eq!(tail[2]["op"], json!("return"), "{frames:#?}");
        assert_eq!(
            tail[2]["value"],
            json!({ "type": "undefined" }),
            "the plain undefined, not a future: {frames:#?}"
        );
        assert_eq!(called(&tail[0]), "1", "{frames:#?}");
        assert_eq!(called(&tail[1]), "2", "{frames:#?}");
    })
    .await;
}

/// EVP-5.7-4
///
/// An emitter that gives up on its `events.dispatch` (`cancel`) does not
/// cancel an `emit`: this one was queued behind another, still starts its
/// calls, and the session's emits go on after it.
///
/// This is the race that used to lose the emit (#1121): when `cancel` reaches
/// the session before its ordered task takes the queued `events.dispatch`, the
/// emit must still start. Deterministically covered in
/// `crates/rutis-bridge/src/session/rpc/tests.rs::a_cancelled_emit_dispatch_stays_queued`.
#[tokio::test(flavor = "multi_thread")]
async fn an_emit_is_not_cancelled() {
    within("emit cancel", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/e", Some("chat"), "one", "any")
            .await
            .unwrap();
        // The first emit waits in its filter, so the second one is queued.
        let held: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<Reply>>>> = Arc::default();
        let filtering = Arc::new(AtomicBool::new(false));
        let (release, receiver) = tokio::sync::oneshot::channel::<Reply>();
        *held.lock().unwrap() = Some(receiver);
        let filter = {
            let held = held.clone();
            let filtering = filtering.clone();
            Value::callback(move |_| {
                let waiting = held.lock().unwrap().take();
                match waiting {
                    Some(receiver) => {
                        filtering.store(true, Ordering::SeqCst);
                        Ok(Value::future(async move {
                            receiver
                                .await
                                .unwrap_or_else(|_| Ok(Value::Data(json!([false]))))
                        }))
                    }
                    None => Ok(Value::Data(json!([true]))),
                }
            })
        };
        let first = rig
            .dispatch_started(Some("chat"), "t/e", "emit", vec![], Some(filter))
            .unwrap();
        until(
            || filtering.load(Ordering::SeqCst),
            "the first emit's filter",
        )
        .await;
        let second = rig
            .dispatch_started(Some("chat"), "t/e", "emit", vec![], None)
            .unwrap();
        drop(second);
        // The first keeps no listener; the second is next in line.
        release.send(Ok(Value::Data(json!([false])))).unwrap();
        let answered = settle(first.await.unwrap()).await.unwrap();
        assert_eq!(answered, Value::Undefined);
        let third = rig
            .dispatch(Some("chat"), "t/e", "emit", vec![])
            .await
            .unwrap();
        assert_eq!(third, Value::Undefined);
        until(
            || rig.fake.calls.lock().unwrap().len() == 2,
            "one call each from the second and third emits",
        )
        .await;
        let calls = rig.fake.calls.lock().unwrap().clone();
        for call in &calls {
            assert_eq!(call["listener"], Value::Data(json!("1")), "{calls:?}");
        }
        assert_ne!(calls[0]["dispatch"], calls[1]["dispatch"], "{calls:?}");
    })
    .await;
}

/// EVP-5.4-14
///
/// A waterfall's `next` runs the rest once: a second call in the same
/// waterfall fails with `WaterfallNext` ("already called"), and a call made
/// after the dispatch ended fails with it too ("after its waterfall
/// ended"), even on a handle that was never called.
#[tokio::test(flavor = "multi_thread")]
async fn next_runs_once_and_not_after_the_waterfall_ended() {
    within("next", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        // The listener of `t/w` calls `next` twice; the host answers a
        // `next` call with a future, so each answer is settled.
        let misuse: Arc<Mutex<Option<(String, String)>>> = Arc::default();
        let twice = {
            let misuse = misuse.clone();
            Arc::new(move |fields: &BTreeMap<String, Value>| -> Reply {
                let next = fields["next"].clone().reference()?;
                let misuse = misuse.clone();
                Ok(Value::future(async move {
                    let first = settle(next.call(Value::List(vec![]))?).await?;
                    assert_eq!(first, Value::Undefined, "the rest is empty");
                    let refused = settle(next.call(Value::List(vec![]))?).await.unwrap_err();
                    match &refused {
                        Error::Remote { name, message, .. } => {
                            *misuse.lock().unwrap() = Some((name.clone(), message.clone()))
                        }
                        other => panic!("expected WaterfallNext, got {other:?}"),
                    }
                    Ok(Value::Undefined)
                }))
            })
        };
        rig.register_with("1", "t/w", Some("chat"), "fake", "async", twice)
            .await
            .unwrap();
        let result = rig
            .dispatch(Some("chat"), "t/w", "waterfall", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        assert_eq!(
            misuse.lock().unwrap().take().unwrap(),
            (
                "WaterfallNext".to_owned(),
                "next of t/w was already called".to_owned()
            )
        );

        // Another waterfall's listener keeps its handle, never calls it, and
        // the test calls it once the dispatch ended.
        let kept: Arc<Mutex<Option<Reference>>> = Arc::default();
        let keeping = {
            let kept = kept.clone();
            Arc::new(move |fields: &BTreeMap<String, Value>| -> Reply {
                *kept.lock().unwrap() = Some(fields["next"].clone().reference()?);
                Ok(Value::Undefined)
            })
        };
        rig.register_with("2", "t/w2", Some("chat"), "fake", "async", keeping)
            .await
            .unwrap();
        let result = rig
            .dispatch(Some("chat"), "t/w2", "waterfall", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        let next = kept.lock().unwrap().take().unwrap();
        let refused = settle(next.call(Value::List(vec![])).unwrap())
            .await
            .unwrap_err();
        match refused {
            Error::Remote { name, message, .. } => {
                assert_eq!(name, "WaterfallNext");
                assert_eq!(message, "next of t/w2 was called after its waterfall ended");
            }
            other => panic!("expected WaterfallNext, got {other:?}"),
        }
    })
    .await;
}

/// EVP-6-6
///
/// A filter's answers must be one boolean per candidate: a shorter list, or
/// one with a non-boolean, fails the whole dispatch with the protocol's
/// `BindingError` and starts no call.
#[tokio::test(flavor = "multi_thread")]
async fn filter_answers_must_match_the_candidates() {
    within("filter answers", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/f", Some("chat"), "one", "any")
            .await
            .unwrap();
        rig.register("2", "t/f", Some("chat"), "two", "any")
            .await
            .unwrap();
        let short = Value::callback(|_| Ok(Value::Data(json!([true]))));
        let answered = rig
            .dispatch_started(Some("chat"), "t/f", "serial", vec![], Some(short))
            .unwrap()
            .await
            .unwrap();
        let refused = settle(answered).await.unwrap_err();
        assert_eq!(
            binding(&refused),
            (
                "BindingError",
                "filter of t/f returned 1 answers for 2 listeners"
            )
        );
        // Not a boolean: still refused, and the message counts the answers.
        let not_boolean = Value::callback(|_| Ok(Value::Data(json!([true, "yes"]))));
        let answered = rig
            .dispatch_started(Some("chat"), "t/f", "serial", vec![], Some(not_boolean))
            .unwrap()
            .await
            .unwrap();
        let refused = settle(answered).await.unwrap_err();
        assert_eq!(
            binding(&refused),
            (
                "BindingError",
                "filter of t/f returned 2 answers for 2 listeners"
            )
        );
        assert!(rig.fake.calls.lock().unwrap().is_empty());
    })
    .await;
}

/// EVP-4.1-6
///
/// `events.off` answers once the entries have left the table: a dispatch
/// queued right after it does not call them. Ids that are not there are
/// ignored, not refused.
#[tokio::test(flavor = "multi_thread")]
async fn events_off_answers_after_the_entries_left() {
    within("events.off", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/off", Some("chat"), "one", "any")
            .await
            .unwrap();
        let result = rig
            .dispatch(Some("chat"), "t/off", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        assert_eq!(rig.fake.calls.lock().unwrap().len(), 1);
        // The off's frame is queued before the dispatch's: the entry is
        // gone by the time the dispatch runs.
        let off = rig
            .started("events.off", Value::Data(json!(["1", "no-such-listener"])))
            .unwrap();
        let next = rig
            .dispatch_started(Some("chat"), "t/off", "serial", vec![], None)
            .unwrap();
        let answered = settle(off.await.unwrap()).await.unwrap();
        assert_eq!(answered, Value::Undefined, "an unknown id is ignored");
        let result = settle(next.await.unwrap()).await.unwrap();
        assert_eq!(result, Value::Undefined);
        assert_eq!(
            rig.fake.calls.lock().unwrap().len(),
            1,
            "the withdrawn listener was not called again"
        );
    })
    .await;
}

/// A host service, provided by a plugin row.
struct Clock;

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _: Value) -> Reply {
        Err(Error::value(format!("no method {method}")))
    }
}

/// A plugin that provides the host service `service` as row `row`.
struct Provides {
    row: &'static str,
    service: &'static str,
}

impl DynPlugin for Provides {
    fn name(&self) -> &str {
        "provider"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, rutis::Error>> {
        Box::pin(async move {
            ctx.mark_row(self.row)?;
            ctx.provide_at(host_key(self.service), Arc::new(Clock))?;
            Ok(Effect::Done)
        })
    }
}

/// EVP-8-3
///
/// `rutis/service` records the service's name, the scope chain, whether it
/// is available, and the providing row: `true` with the new row when the
/// provider is replaced (after the old one's `false`), and `false` with the
/// last provider's row when it goes.
#[tokio::test(flavor = "multi_thread")]
async fn a_service_provider_is_announced_replaced_and_withdrawn() {
    within("rutis/service", async {
        let rig = rig().await;
        rig.register("svc", "rutis/service", None, "fake", "any")
            .await
            .unwrap();
        let first = rig.root.plugin(Provides {
            row: "weather",
            service: "weather",
        });
        until(
            || rig.fake.calls.lock().unwrap().len() == 1,
            "the service's arrival",
        )
        .await;
        assert_eq!(
            service(&rig.fake.calls.lock().unwrap()[0]),
            (
                "weather".to_owned(),
                Vec::new(),
                true,
                Some("weather".to_owned())
            )
        );
        first.dispose().await.unwrap();
        until(
            || rig.fake.calls.lock().unwrap().len() == 2,
            "the service's withdrawal",
        )
        .await;
        assert_eq!(
            service(&rig.fake.calls.lock().unwrap()[1]),
            (
                "weather".to_owned(),
                Vec::new(),
                false,
                Some("weather".to_owned())
            )
        );
        let second = rig.root.plugin(Provides {
            row: "weather-2",
            service: "weather",
        });
        until(
            || rig.fake.calls.lock().unwrap().len() == 3,
            "the new provider",
        )
        .await;
        assert_eq!(
            service(&rig.fake.calls.lock().unwrap()[2]),
            (
                "weather".to_owned(),
                Vec::new(),
                true,
                Some("weather-2".to_owned())
            )
        );
        second.dispose().await.unwrap();
        until(
            || rig.fake.calls.lock().unwrap().len() == 4,
            "the new provider's withdrawal",
        )
        .await;
        assert_eq!(
            service(&rig.fake.calls.lock().unwrap()[3]),
            (
                "weather".to_owned(),
                Vec::new(),
                false,
                Some("weather-2".to_owned())
            )
        );
    })
    .await;
}

/// The record a `rutis/service` call carries: `(name, scope, available, row)`.
fn service(
    call: &BTreeMap<String, Value>,
) -> (String, Vec<(String, String)>, bool, Option<String>) {
    assert_eq!(
        call["name"],
        Value::Data(json!("rutis/service")),
        "{call:?}"
    );
    assert_eq!(call["mode"], Value::Data(json!("emit")), "{call:?}");
    let Value::List(arguments) = call["args"].clone() else {
        panic!("one argument: {call:?}");
    };
    assert_eq!(arguments.len(), 1, "{call:?}");
    let Json::Object(fields) = arguments[0].clone().json().unwrap() else {
        panic!("a record: {call:?}");
    };
    let name = fields["name"].as_str().unwrap().to_owned();
    let chain: Vec<(String, String)> = fields["scope"]
        .as_array()
        .unwrap()
        .iter()
        .map(|scope| {
            (
                scope["id"].as_str().unwrap().to_owned(),
                scope["group"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let available = fields["available"].as_bool().unwrap();
    let row = fields["row"].as_str().map(str::to_owned);
    (name, chain, available, row)
}

/// rule: EVP-4.3-1
/// rule: EVP-4.3-2
/// rule: EVP-5.1-1
///
/// The table keeps the entries in the order their `events.on` frames
/// arrive, and a `prepend` entry goes before every one of them; a dispatch
/// takes the matching entries in that order: the calls of one `serial`
/// dispatch start prepended first, then by arrival.
#[tokio::test(flavor = "multi_thread")]
async fn entries_are_taken_in_arrival_order_and_prepend_first() {
    within("positions", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        for id in ["1", "2", "3"] {
            rig.register(id, "t/order", Some("chat"), "fake", "any")
                .await
                .unwrap();
        }
        rig.register_prepend("p", "t/order", Some("chat"), "fake", "any")
            .await
            .unwrap();
        let result = rig
            .dispatch(Some("chat"), "t/order", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined, "no entry answers");
        assert_eq!(listeners(&rig.fake), ["p", "1", "2", "3"]);
    })
    .await;
}

/// rule: EVP-4.3-3
///
/// A dispatch takes the entries that match the name: an exact entry of that
/// name, and a prefix entry the name starts with (`t/ev` takes `t/ev`
/// itself and `t/eviction`), and nothing else. The entries are taken in
/// registration order across both kinds: the exact entry registered first
/// is called first, and the prefix entry registered after it answers the
/// `serial` dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn a_prefix_entry_takes_the_names_that_start_with_it() {
    within("prefix names", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        for (id, name) in [("1", "t/ev"), ("2", "t/eviction"), ("3", "t/else")] {
            rig.register(id, name, Some("chat"), "fake", "any")
                .await
                .unwrap();
        }
        // The host's own prefix entry, registered last, which hears the
        // dispatched name and answers it.
        let heard: Arc<Mutex<Vec<String>>> = Arc::default();
        let recording = heard.clone();
        rig.root
            .listen::<Args>()
            .prefix("t/ev")
            .on(move |_: Arc<Args>, call: Call<Args>| {
                let heard = recording.clone();
                async move {
                    heard.lock().unwrap().push(call.name().to_owned());
                    Ok(Some(ArgValue(Value::Data(json!("prefix")))))
                }
            })
            .unwrap();
        // `t/ev`: the exact entry of the same name first, the prefix after.
        let result = rig
            .dispatch(Some("chat"), "t/ev", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Data(json!("prefix")));
        assert_eq!(listeners(&rig.fake), ["1"]);
        // `t/eviction` starts with `t/ev`: its own entry, then the prefix.
        let result = rig
            .dispatch(Some("chat"), "t/eviction", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(
            result,
            Value::Data(json!("prefix")),
            "the exact entry (no result) came before the prefix entry"
        );
        assert_eq!(listeners(&rig.fake), ["1", "2"]);
        assert_eq!(*heard.lock().unwrap(), ["t/ev", "t/eviction"]);
        // `t/other` does not start with `t/ev`, and no entry has its name.
        let result = rig
            .dispatch(Some("chat"), "t/other", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined, "no entry takes t/other");
        assert_eq!(listeners(&rig.fake), ["1", "2"]);
        assert_eq!(heard.lock().unwrap().len(), 2);
        // Nothing at all: no call, and the dispatch answers as usual.
        let result = rig
            .dispatch(Some("chat"), "t/nothing", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        assert_eq!(listeners(&rig.fake), ["1", "2"]);
        assert_eq!(heard.lock().unwrap().len(), 2);
    })
    .await;
}

/// rule: EVP-5.1-3
///
/// Every call checks that its entry is still there before it starts: one
/// withdrawn after the snapshot (here while the filter was still waiting)
/// is skipped, as if it had never been selected.
#[tokio::test(flavor = "multi_thread")]
async fn an_entry_withdrawn_after_the_snapshot_is_skipped() {
    within("withdrawn after the snapshot", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/drop", Some("chat"), "one", "any")
            .await
            .unwrap();
        rig.register("2", "t/drop", Some("chat"), "two", "any")
            .await
            .unwrap();
        let (release, answer) = tokio::sync::oneshot::channel::<Reply>();
        let filtering = Arc::new(AtomicBool::new(false));
        let awaiting: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<Reply>>>> = Arc::default();
        *awaiting.lock().unwrap() = Some(answer);
        let filter = {
            let waiting = filtering.clone();
            let awaiting = awaiting.clone();
            Value::callback(move |_| {
                waiting.store(true, Ordering::SeqCst);
                let answer = awaiting.lock().unwrap().take();
                match answer {
                    Some(answer) => Ok(Value::future(async move {
                        answer
                            .await
                            .unwrap_or_else(|_| Ok(Value::Data(json!([true, true]))))
                    })),
                    None => Ok(Value::Data(json!([true, true]))),
                }
            })
        };
        let held =
            rig.dispatch_running(Some("chat"), "t/drop", "serial", vec![], Some(filter), None);
        until(|| filtering.load(Ordering::SeqCst), "the dispatch's filter").await;
        // The snapshot is taken and the filter has answered nothing yet:
        // the entry leaves the table in the meantime.
        let gone = rig
            .send("events.off", Value::Data(json!(["2"])))
            .await
            .unwrap();
        assert_eq!(gone, Value::Undefined);
        release.send(Ok(Value::Data(json!([true, true])))).unwrap();
        assert_eq!(held.await.unwrap().unwrap(), Value::Undefined);
        assert_eq!(
            listeners(&rig.fake),
            ["1"],
            "the withdrawn entry is skipped"
        );
    })
    .await;
}

/// rule: EVP-5.4-7
///
/// A waterfall calls the first entry with `next`; `next` calls the next
/// entry still there (a withdrawn one is skipped) and, after the last, the
/// terminal with the event's arguments, whose value the reply carries; the
/// outermost value is returned as it is. With no entry at all the terminal
/// is called straight away, and with no terminal the reply is `undefined`.
#[tokio::test(flavor = "multi_thread")]
async fn a_waterfall_skips_the_withdrawn_and_ends_at_its_terminal() {
    within("waterfall chain", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        // The first entry calls `next` and answers its value.
        let rest: Arc<Mutex<Vec<Value>>> = Arc::default();
        let chaining = {
            let rest = rest.clone();
            Arc::new(move |fields: &BTreeMap<String, Value>| -> Reply {
                let next = fields["next"].clone().reference()?;
                let rest = rest.clone();
                Ok(Value::future(async move {
                    let out = settle(next.call(Value::List(vec![]))?).await?;
                    rest.lock().unwrap().push(out.clone());
                    Ok(out)
                }))
            })
        };
        rig.register_with("1", "t/w", Some("chat"), "one", "async", chaining)
            .await
            .unwrap();
        rig.register("2", "t/w", Some("chat"), "two", "any")
            .await
            .unwrap();
        let terminal_seen: Arc<Mutex<Vec<Vec<Value>>>> = Arc::default();
        let recording = terminal_seen.clone();
        let terminal = Value::callback(move |argument| {
            let arguments = argument.list()?;
            recording.lock().unwrap().push(arguments.clone());
            Ok(Value::Data(json!("end")))
        });
        let (release, answer) = tokio::sync::oneshot::channel::<Reply>();
        let filtering = Arc::new(AtomicBool::new(false));
        let awaiting: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<Reply>>>> = Arc::default();
        *awaiting.lock().unwrap() = Some(answer);
        let filter = {
            let waiting = filtering.clone();
            let awaiting = awaiting.clone();
            Value::callback(move |_| {
                waiting.store(true, Ordering::SeqCst);
                let answer = awaiting.lock().unwrap().take();
                match answer {
                    Some(answer) => Ok(Value::future(async move {
                        answer
                            .await
                            .unwrap_or_else(|_| Ok(Value::Data(json!([true, true]))))
                    })),
                    None => Ok(Value::Data(json!([true, true]))),
                }
            })
        };
        let held = rig.dispatch_running(
            Some("chat"),
            "t/w",
            "waterfall",
            vec![json!(1).into(), json!("x").into()],
            Some(filter),
            Some(terminal),
        );
        until(
            || filtering.load(Ordering::SeqCst),
            "the waterfall's filter",
        )
        .await;
        let gone = rig
            .send("events.off", Value::Data(json!(["2"])))
            .await
            .unwrap();
        assert_eq!(gone, Value::Undefined);
        release.send(Ok(Value::Data(json!([true, true])))).unwrap();
        let reply = held.await.unwrap().unwrap();
        assert_eq!(
            reply,
            Value::Data(json!("end")),
            "`next` skipped the withdrawn entry and reached the terminal"
        );
        assert_eq!(listeners(&rig.fake), ["1"]);
        assert_eq!(*rest.lock().unwrap(), vec![Value::Data(json!("end"))]);
        assert_eq!(
            *terminal_seen.lock().unwrap(),
            vec![vec![Value::Data(json!(1)), Value::Data(json!("x"))]],
            "the terminal takes the event's arguments"
        );

        // No entry at all: the terminal answers, and its value is the reply.
        let empty_seen: Arc<Mutex<Vec<Vec<Value>>>> = Arc::default();
        let recording = empty_seen.clone();
        let empty_terminal = Value::callback(move |argument| {
            recording.lock().unwrap().push(argument.list()?);
            Ok(Value::Data(json!(false)))
        });
        let answered = rig
            .dispatch_with(
                Some("chat"),
                "t/w2",
                "waterfall",
                vec![json!(2).into()],
                None,
                Some(empty_terminal),
            )
            .unwrap()
            .await
            .unwrap();
        assert_eq!(
            settle(answered).await.unwrap(),
            Value::Data(json!(false)),
            "a waterfall returns its value as it is"
        );
        assert_eq!(
            *empty_seen.lock().unwrap(),
            vec![vec![Value::Data(json!(2))]]
        );
        // No entry, no terminal: `undefined`.
        let answered = rig
            .dispatch_with(Some("chat"), "t/w3", "waterfall", vec![], None, None)
            .unwrap()
            .await
            .unwrap();
        assert_eq!(settle(answered).await.unwrap(), Value::Undefined);
    })
    .await;
}

/// rule: EVP-4.2-1
///
/// The runtime picks the id a listener registers under; the host keeps the
/// ids of a session apart: a second registration under an id still there
/// is refused, and the entry already in the table is the one called. (The
/// refusal's message is the host's own: the protocol's error table does not
/// list it, `docs/formal-trace/rules/protocol-events.md`.)
#[tokio::test(flavor = "multi_thread")]
async fn a_second_listener_of_a_live_id_is_refused() {
    within("duplicate ids", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/one", Some("chat"), "one", "any")
            .await
            .unwrap();
        let refused = rig
            .register("1", "t/two", Some("chat"), "two", "any")
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            ("BindingError", "listener 1 is already registered")
        );
        let result = rig
            .dispatch(Some("chat"), "t/two", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        assert!(
            listeners(&rig.fake).is_empty(),
            "the refused registration brought no entry"
        );
        rig.dispatch(Some("chat"), "t/one", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(listeners(&rig.fake), ["1"]);
    })
    .await;
}

/// rule: EVP-7.1-1
/// rule: EVP-7.1-2
///
/// `rows.ended` is a table change in the session's frame order: an
/// `events.on` before it enters the table and leaves with the row (a
/// dispatch between the two calls it), one after it is refused for a row
/// that is gone, and a dispatch after it reaches no entry of that row. The
/// runtime's own `events.off` for an id the host took with the row is
/// ignored, not refused.
#[tokio::test(flavor = "multi_thread")]
async fn rows_ended_is_a_table_change_in_frame_order() {
    within("rows.ended", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/end", Some("chat"), "one", "any")
            .await
            .unwrap();
        rig.dispatch(Some("chat"), "t/end", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(listeners(&rig.fake), ["1"], "registered before the end");
        let ended = rig
            .send("rows.ended", Value::Data(json!("chat")))
            .await
            .unwrap();
        assert_eq!(ended, Value::Undefined);
        // The row's entries are gone: no call, and no new registration.
        // (Its own dispatch is refused, the row no longer being loaded:
        // the mount's still runs over the table.)
        rig.dispatch(None, "t/end", "serial", vec![]).await.unwrap();
        assert_eq!(listeners(&rig.fake), ["1"]);
        let refused = rig
            .register("2", "t/end", Some("chat"), "two", "any")
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            (
                "BindingError",
                "the calling row is not loaded in this runtime"
            )
        );
        // Its own `events.off` for the id the host took is harmless.
        let gone = rig
            .send("events.off", Value::Data(json!(["1"])))
            .await
            .unwrap();
        assert_eq!(gone, Value::Undefined);
    })
    .await;
}

/// rule: EVP-5.5-5
/// rule: EVP-5.5-6
///
/// A row's end lets go of the emits that are its own: the one waiting for
/// its filter is cancelled, the ones queued behind it are dropped, and
/// neither starts a call. Both `events.dispatch` calls still get their
/// answers, and the session's other emits (another row's) go on in their
/// turn.
///
/// The three emits name one event, and row `a` and row `b` both listen to
/// it: a call of "1" after `rows.ended` says row `a`'s entry outlived the
/// row (§7.1 step 1), which is what the emit that row `b` queued used to
/// be offered when the row's emits were let go before its entries were out
/// (#1133). `rows.ended` is a table change in the session's frame order
/// (§4.1), so its reply says the two emits before it were queued.
#[tokio::test(flavor = "multi_thread")]
async fn a_rows_end_lets_go_of_its_queued_and_running_emits() {
    within("emit queue and rows.ended", async {
        let rig = rig().await;
        rig.load("a", None, &[]).await;
        rig.load("b", None, &[]).await;
        // Both rows listen to the one name; row `a`'s running emit reaches
        // its own listener and row `b`'s, so its filter is called for two.
        rig.register("1", "t/ended", Some("a"), "one", "any")
            .await
            .unwrap();
        rig.register("2", "t/ended", Some("b"), "two", "any")
            .await
            .unwrap();
        let (release, answer) = tokio::sync::oneshot::channel::<Reply>();
        let filtering = Arc::new(AtomicBool::new(false));
        let awaiting: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<Reply>>>> = Arc::default();
        *awaiting.lock().unwrap() = Some(answer);
        let filter = {
            let waiting = filtering.clone();
            let awaiting = awaiting.clone();
            Value::callback(move |_| {
                waiting.store(true, Ordering::SeqCst);
                let answer = awaiting.lock().unwrap().take();
                match answer {
                    Some(answer) => Ok(Value::future(async move {
                        answer
                            .await
                            .unwrap_or_else(|_| Ok(Value::Data(json!([true, true]))))
                    })),
                    None => Ok(Value::Data(json!([true, true]))),
                }
            })
        };
        let first = rig
            .dispatch_started(Some("a"), "t/ended", "emit", vec![], Some(filter))
            .unwrap();
        until(
            || filtering.load(Ordering::SeqCst),
            "the first emit's filter",
        )
        .await;
        // Queued behind it: another of row `a`, then one of row `b`.
        let second = rig
            .dispatch_started(Some("a"), "t/ended", "emit", vec![], None)
            .unwrap();
        let third = rig
            .dispatch_started(Some("b"), "t/ended", "emit", vec![], None)
            .unwrap();
        // Row `a` ends: its running emit is cancelled, its queued one
        // dropped. The filter's future goes with the first.
        let ended = rig
            .send("rows.ended", Value::Data(json!("a")))
            .await
            .unwrap();
        assert_eq!(ended, Value::Undefined);
        until(|| release.is_closed(), "the first emit's filter dropped").await;
        assert_eq!(
            settle(first.await.unwrap()).await.unwrap(),
            Value::Undefined
        );
        assert_eq!(
            settle(second.await.unwrap()).await.unwrap(),
            Value::Undefined
        );
        // The other row's emit runs: its calls are on the wire before its
        // reply, and row `a`'s listener is not among them.
        assert_eq!(
            settle(third.await.unwrap()).await.unwrap(),
            Value::Undefined
        );
        assert_eq!(
            calls_for(&rig.frames, "t/ended"),
            ["2"],
            "the row that ended was still offered to the other row's emit"
        );
        until(
            || listeners(&rig.fake) == ["2"],
            "the third emit's only call",
        )
        .await;
        assert_eq!(
            listeners(&rig.fake),
            ["2"],
            "neither the cancelled nor the dropped emit called anyone, and the \
             row that ended still heard the other row's"
        );
    })
    .await;
}

/// rule: EVP-5.4-1
///
/// An `emit` starts the calls of a name in registration order and waits
/// for none of them: the first listener here does not answer until the
/// test lets it, and the second is called and the dispatch answered all
/// the same.
#[tokio::test(flavor = "multi_thread")]
async fn an_emit_starts_every_call_and_waits_for_none() {
    within("emit starts", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        let (release, answer) = tokio::sync::oneshot::channel::<Reply>();
        let awaiting: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<Reply>>>> = Arc::default();
        *awaiting.lock().unwrap() = Some(answer);
        rig.register_with(
            "1",
            "t/s",
            Some("chat"),
            "one",
            "any",
            Arc::new(move |_| {
                let answer = awaiting.lock().unwrap().take();
                Ok(Value::future(async move {
                    match answer {
                        Some(answer) => answer.await.unwrap_or(Ok(Value::Undefined)),
                        None => Ok(Value::Undefined),
                    }
                }))
            }),
        )
        .await
        .unwrap();
        rig.register("2", "t/s", Some("chat"), "two", "any")
            .await
            .unwrap();
        let result = rig
            .dispatch(Some("chat"), "t/s", "emit", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        until(
            || calls_for(&rig.frames, "t/s").len() == 2,
            "both calls of the emit",
        )
        .await;
        assert_eq!(
            calls_for(&rig.frames, "t/s"),
            ["1", "2"],
            "both started, in registration order, the first still unanswered"
        );
        release.send(Ok(Value::Undefined)).unwrap();
    })
    .await;
}

/// rule: EVP-4.1-3
///
/// Table changes and `emit` dispatches take their turn in the session's
/// frame stream while another dispatch runs: a `serial` dispatch held in
/// its listener does not stop an `events.on` from entering the table or an
/// `emit` from starting its calls.
#[tokio::test(flavor = "multi_thread")]
async fn a_running_dispatch_does_not_hold_the_frame_stream() {
    within("ordered operations", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        let (release, answer) = tokio::sync::oneshot::channel::<Reply>();
        let held = Arc::new(AtomicBool::new(false));
        let called = held.clone();
        let waiting: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<Reply>>>> = Arc::default();
        *waiting.lock().unwrap() = Some(answer);
        rig.register_with(
            "slow",
            "t/slow",
            Some("chat"),
            "one",
            "any",
            Arc::new(move |_| {
                called.store(true, Ordering::SeqCst);
                let answer = waiting.lock().unwrap().take();
                Ok(Value::future(async move {
                    match answer {
                        Some(answer) => answer.await.unwrap_or(Ok(Value::Undefined)),
                        None => Ok(Value::Undefined),
                    }
                }))
            }),
        )
        .await
        .unwrap();
        let held_dispatch =
            rig.dispatch_running(Some("chat"), "t/slow", "serial", vec![], None, None);
        until(|| held.load(Ordering::SeqCst), "the held listener").await;
        // Both answer while the dispatch above is still in its listener.
        rig.register("fast", "t/fast", Some("chat"), "two", "any")
            .await
            .unwrap();
        let result = rig
            .dispatch(Some("chat"), "t/fast", "emit", vec![])
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        until(|| listeners(&rig.fake) == ["slow", "fast"], "both calls").await;
        release.send(Ok(Value::Undefined)).unwrap();
        assert_eq!(held_dispatch.await.unwrap().unwrap(), Value::Undefined);
    })
    .await;
}

/// rule: EVP-5.3-2
///
/// A dispatch's `filter` and `terminal` are function references: data
/// where one is required fails the dispatch itself, and no listener is
/// called.
#[tokio::test(flavor = "multi_thread")]
async fn a_dispatchs_filter_and_terminal_must_be_functions() {
    within("dispatch arguments", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/a", Some("chat"), "one", "any")
            .await
            .unwrap();
        let mut fields = request(Some("chat"), "t/a", "serial", vec![]);
        fields.insert("filter".into(), Value::Data(json!("not a function")));
        let refused = rig
            .send("events.dispatch", Value::Record(fields))
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            ("BindingError", "events.dispatch: filter is not a function")
        );
        let mut fields = request(Some("chat"), "t/a", "waterfall", vec![]);
        fields.insert("terminal".into(), Value::Data(json!(7)));
        let refused = rig
            .send("events.dispatch", Value::Record(fields))
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            (
                "BindingError",
                "events.dispatch: terminal is not a function"
            )
        );
        assert!(listeners(&rig.fake).is_empty(), "no listener was called");
    })
    .await;
}

/// The name and message of an error a far end read.
fn thrown(error: &Error) -> (&str, &str) {
    match error {
        Error::Remote { name, message, .. } => (name, message),
        other => panic!("expected a remote error, got {other:?}"),
    }
}

/// A typed event a host filter is used on: a name in the table.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Filtered(String);

impl Event for Filtered {
    const NAME: &'static str = "t/f";
    type Value = ();
}

/// A `WaterfallNext` misuse a Rust listener recorded: its message and its
/// wire name.
type Misuse = Arc<Mutex<Option<(String, Option<&'static str>)>>>;

/// rule: EVP-3-10
///
/// The `isolate` labels a row is loaded with (a service it keeps to a scope
/// of its own, a lease among them) are no event scope: its listeners'
/// descriptors carry the `rows.load` scope chain, `[]` when the row has
/// none, and nothing of the labels.
#[tokio::test(flavor = "multi_thread")]
async fn isolate_labels_are_not_event_scopes() {
    within("isolate labels", async {
        let rig = rig().await;
        let chain = scope(&[("17", "tenant")]);
        // A row keeps a service and a lease to itself; another only a
        // service. The labels are `(service, label)` pairs of `rows.load`.
        rig.load_with(
            "iso",
            Some("iso-row"),
            &[("weather", "private"), ("lease", "\u{1}iso")],
            &chain,
        )
        .await;
        rig.load_with("plain", None, &[("weather", "private")], &[])
            .await;
        rig.register("1", "t/iso", Some("iso"), "one", "any")
            .await
            .unwrap();
        rig.register("2", "t/iso", Some("plain"), "two", "any")
            .await
            .unwrap();
        let seen: Arc<Mutex<Vec<Json>>> = Arc::default();
        let recorded = seen.clone();
        let selector = Value::callback(move |argument| {
            let descriptors = argument.list()?.remove(0).json()?;
            recorded.lock().unwrap().push(descriptors.clone());
            let answers = descriptors.as_array().unwrap().len();
            Ok(Value::Data(json!(vec![false; answers])))
        });
        let answered = rig
            .dispatch_started(Some("iso"), "t/iso", "serial", vec![], Some(selector))
            .unwrap()
            .await
            .unwrap();
        assert_eq!(settle(answered).await.unwrap(), Value::Undefined);
        let seen = seen.lock().unwrap().clone();
        let described = &seen[0];
        // The isolated row: its id and chain; the plain row: an empty one.
        assert_eq!(described[0]["row"], json!("iso-row"), "{described:#?}");
        assert_eq!(
            described[0]["scope"],
            json!([{ "id": "17", "group": "tenant" }]),
            "{described:#?}"
        );
        assert_eq!(described[1]["row"], json!("plain"), "{described:#?}");
        assert_eq!(described[1]["scope"], json!([]), "{described:#?}");
        // A label is not part of a descriptor at all: neither the service's
        // nor the lease's (whose U+0001 shows escaped in JSON text).
        let text = described.to_string();
        assert!(!text.contains("private"), "{text}");
        assert!(!text.contains("0001"), "{text}");
    })
    .await;
}

/// rule: EVP-5.6-5
///
/// A call that cannot be delivered (here the runtime answers that its
/// listener is withdrawn) is reported by the dispatching side, once, in the
/// §5.6 words for a listener that is no row's (`in runtime <r>`); the
/// emitter still gets its `undefined` and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn an_emit_reports_a_listener_that_is_gone() {
    within("withdrawn listener report", async {
        let (root, errors) = capturing_root();
        let rig = rig_on(root, "rt").await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/gone", None, "one", "any")
            .await
            .unwrap();
        // The runtime no longer has listener 1: the call reaches it and is
        // refused there.
        rig.fake.listeners.lock().unwrap().remove("1");
        let answered = rig
            .dispatch(Some("chat"), "t/gone", "emit", vec![])
            .await
            .unwrap();
        assert_eq!(
            answered,
            Value::Undefined,
            "an emit answers its emitter as usual"
        );
        until(
            || {
                errors.lock().unwrap().iter().any(|error| {
                    rutis_bridge::events::report_line(error)
                        .is_some_and(|line| line.contains("t/gone"))
                })
            },
            "the report",
        )
        .await;
        let lines: Vec<String> = errors
            .lock()
            .unwrap()
            .iter()
            .filter_map(|error| rutis_bridge::events::report_line(error))
            .filter(|line| line.contains("t/gone"))
            .collect();
        assert_eq!(
            lines,
            ["listener of t/gone in runtime rt failed: BindingError: listener \"1\" is withdrawn"],
            "{lines:?}"
        );
        // The session goes on.
        let answered = rig
            .dispatch(Some("chat"), "t/gone", "emit", vec![])
            .await
            .unwrap();
        assert_eq!(answered, Value::Undefined);
    })
    .await;
}

/// rule: EVP-6-10
///
/// A filter's error is the dispatch's failure as it is, and a plain `bool`
/// filter (0.22's `filter`; the rule's `try_filter`, whose error is now the
/// selector's) can only fail by panicking: the panic, like the error, fails
/// the whole dispatch before any call.
#[tokio::test(flavor = "multi_thread")]
async fn a_filter_error_fails_the_dispatch_and_a_panic_is_it() {
    within("filter errors", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/f", Some("chat"), "one", "any")
            .await
            .unwrap();
        // The emitter's selector throws: the dispatch fails with that
        // error, as it is.
        let throwing = Value::callback(|_| {
            Err(Error::Remote {
                name: "StopFilter".into(),
                message: "the predicate said no".into(),
                graph: None,
            })
        });
        let refused = rig
            .dispatch_started(Some("chat"), "t/f", "serial", vec![], Some(throwing))
            .unwrap()
            .await
            .unwrap();
        assert_eq!(
            thrown(&settle(refused).await.unwrap_err()),
            ("StopFilter", "the predicate said no")
        );
        assert!(
            rig.fake.calls.lock().unwrap().is_empty(),
            "no listener is called"
        );
        // A Rust filter that panics: the whole dispatch fails with the
        // panic's error.
        let failed = rig
            .root
            .filter(|_| -> bool { panic!("filter broke") })
            .serial(Filtered("x".into()))
            .await
            .unwrap_err();
        assert_eq!(failed.to_string(), "filter broke");
        assert!(
            rig.fake.calls.lock().unwrap().is_empty(),
            "no listener is called"
        );
    })
    .await;
}

/// rule: EVP-10-1
///
/// A signal is a session value on the event path both ways: an argument a
/// listener in another session receives as one, and a result it answers
/// with, which the dispatch hands on as it is. A result nested deeper than
/// the 64-level limit fails the call that answers it, not the session.
#[tokio::test(flavor = "multi_thread")]
async fn signals_cross_the_event_path_and_a_deep_result_is_refused() {
    within("event values", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        // The listener records the arguments it was called with.
        let received: Arc<Mutex<Vec<Vec<Value>>>> = Arc::default();
        let recording = received.clone();
        rig.register_with(
            "1",
            "t/sig",
            Some("chat"),
            "one",
            "any",
            Arc::new(move |fields| {
                if let Value::List(arguments) = fields["args"].clone() {
                    recording.lock().unwrap().push(arguments);
                }
                Ok(Value::Signal)
            }),
        )
        .await
        .unwrap();
        let answered = rig
            .dispatch(Some("chat"), "t/sig", "serial", vec![Value::Signal])
            .await
            .unwrap();
        assert_eq!(
            answered,
            Value::Signal,
            "a signal result reaches the emitter"
        );
        assert_eq!(
            *received.lock().unwrap(),
            vec![vec![Value::Signal]],
            "a signal argument reaches the listener"
        );
        // A waterfall's value too.
        let answered = rig
            .dispatch(Some("chat"), "t/sig", "waterfall", vec![Value::Signal])
            .await
            .unwrap();
        assert_eq!(answered, Value::Signal);
        // A result nested deeper than the protocol's 64 levels fails its
        // call with the `BindingError`; the session goes on.
        let deep = {
            let mut value = Value::Data(json!("bottom"));
            for _ in 0..65 {
                value = Value::List(vec![value]);
            }
            value
        };
        rig.root
            .listen::<Args>()
            .name("t/deep")
            .on(move |_: Arc<Args>, _: Call<Args>| {
                let deep = deep.clone();
                async move { Ok(Some(ArgValue(deep))) }
            })
            .unwrap();
        let refused = rig
            .dispatch(Some("chat"), "t/deep", "serial", vec![])
            .await
            .unwrap_err();
        assert_eq!(
            thrown(&refused),
            (
                "BindingError",
                "events.dispatch: the result cannot be sent: the value nests deeper than 64 \
                 levels"
            )
        );
        // The session is still usable.
        let answered = rig
            .dispatch(Some("chat"), "t/sig", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(answered, Value::Signal);
    })
    .await;
}

/// rule: EVP-10-2
///
/// A value the listener's runtime does not take is refused by the sender's
/// side, and only that call fails: an `emit` reports one line, a `serial`
/// dispatch's reply is that `BindingError`, and a `parallel`'s is an
/// `AggregateError` whose child is it. Nothing of the call reaches the
/// listener: it is refused before its frame is made.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_the_far_end_does_not_take_fails_only_its_call() {
    within("value capability", async {
        let (root, errors) = capturing_root();
        let mut endpoint = Endpoint::rust(id("rt"));
        endpoint
            .capabilities
            .retain(|capability| capability != "value-bytes");
        let rig = rig_endpoint(root, "rt", endpoint).await;
        rig.load("chat", None, &[]).await;
        rig.register("1", "t/cap", None, "one", "any")
            .await
            .unwrap();
        let bytes = || Value::Bytes(b"x".to_vec());
        let refused_at = "events.call: argument 1 cannot be sent: cannot send bytes: the far end \
                          does not take bytes values (it did not declare the capability \
                          value-bytes)";
        // emit: a line, and the emitter hears `undefined`.
        let answered = rig
            .dispatch(Some("chat"), "t/cap", "emit", vec![bytes()])
            .await
            .unwrap();
        assert_eq!(answered, Value::Undefined);
        until(
            || {
                errors.lock().unwrap().iter().any(|error| {
                    rutis_bridge::events::report_line(error)
                        .is_some_and(|line| line.contains("t/cap"))
                })
            },
            "the emit's report",
        )
        .await;
        let lines: Vec<String> = errors
            .lock()
            .unwrap()
            .iter()
            .filter_map(|error| rutis_bridge::events::report_line(error))
            .filter(|line| line.contains("t/cap"))
            .collect();
        assert_eq!(
            lines,
            [format!(
                "listener of t/cap in runtime rt failed: BindingError: {refused_at}"
            )]
        );
        // serial: the dispatch's reply is the failure.
        let refused = rig
            .dispatch(Some("chat"), "t/cap", "serial", vec![bytes()])
            .await
            .unwrap_err();
        assert_eq!(thrown(&refused), ("BindingError", refused_at));
        // parallel: one `AggregateError`, its child that same failure.
        let refused = rig
            .dispatch(Some("chat"), "t/cap", "parallel", vec![bytes()])
            .await
            .unwrap_err();
        let Error::Remote {
            name,
            message,
            graph,
        } = &refused
        else {
            panic!("expected AggregateError, got {refused:?}");
        };
        assert_eq!(name, "AggregateError");
        assert_eq!(message, "1 listener of t/cap failed");
        let graph = graph.as_ref().expect("the children's graph");
        let nodes = graph["nodes"].as_array().unwrap();
        assert_eq!(nodes[0]["name"], json!("AggregateError"), "{graph:#?}");
        assert_eq!(nodes[1]["name"], json!("BindingError"), "{graph:#?}");
        assert_eq!(nodes[1]["message"], json!(refused_at), "{graph:#?}");
        assert!(
            rig.fake.calls.lock().unwrap().is_empty(),
            "no call reached the listener"
        );
    })
    .await;
}

/// rule: EVP-11-3
///
/// `AsyncListener`'s message names the listener by the first descriptor
/// field with a value: a row's listener is `row <id>` (the configuration's
/// id, not the key), a mount's is `runtime <r>`, and a Rust listener of
/// this host has only its plugin.
#[tokio::test(flavor = "multi_thread")]
async fn an_async_listener_is_named_by_its_first_described_place() {
    within("async listener labels", async {
        let rig = rig().await;
        rig.load("chat", Some("chat-ui"), &[]).await;
        rig.register("1", "t/row", Some("chat"), "chat-plugin", "async")
            .await
            .unwrap();
        rig.register("2", "t/mount", None, "mount-plugin", "async")
            .await
            .unwrap();
        rig.root.listen::<Args>().name("t/host").on(hear).unwrap();
        let refused = rig
            .dispatch(Some("chat"), "t/row", "bail", vec![])
            .await
            .unwrap_err();
        assert_eq!(
            thrown(&refused),
            (
                "AsyncListener",
                "listener of t/row in row chat-ui is asynchronous and cannot run in a synchronous \
                 bail"
            )
        );
        let refused = rig
            .dispatch(Some("chat"), "t/mount", "bail", vec![])
            .await
            .unwrap_err();
        assert_eq!(
            thrown(&refused),
            (
                "AsyncListener",
                "listener of t/mount in runtime rt is asynchronous and cannot run in a \
                 synchronous bail"
            )
        );
        let refused = rig
            .dispatch(Some("chat"), "t/host", "bail", vec![])
            .await
            .unwrap_err();
        assert_eq!(
            thrown(&refused),
            (
                "AsyncListener",
                "listener of t/host in plugin root is asynchronous and cannot run in a \
                 synchronous bail"
            )
        );
    })
    .await;
}

/// rule: EVP-11-4
///
/// `WaterfallNext`'s messages, word for word: `next` outside a waterfall
/// says which mode it was, and a synchronous listener in an asynchronous
/// waterfall is told to register with `on`. (The twice and ended cases are
/// in `next_runs_once_and_not_after_the_waterfall_ended`.)
#[tokio::test(flavor = "multi_thread")]
async fn next_outside_a_waterfall_and_in_an_asynchronous_one() {
    within("next messages", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        let serial_seen: Misuse = Arc::default();
        let seen = serial_seen.clone();
        rig.root
            .listen::<Args>()
            .name("t/ns")
            .on(move |_: Arc<Args>, call: Call<Args>| {
                let seen = seen.clone();
                async move {
                    let refused = call.next().await.unwrap_err();
                    *seen.lock().unwrap() = Some((refused.to_string(), refused.wire_name()));
                    Ok(None)
                }
            })
            .unwrap();
        let answered = rig
            .dispatch(Some("chat"), "t/ns", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(answered, Value::Undefined);
        assert_eq!(
            serial_seen.lock().unwrap().take().unwrap(),
            (
                "next is available only in a waterfall (this is serial)".to_owned(),
                Some("WaterfallNext")
            )
        );
        let bail_seen: Misuse = Arc::default();
        let seen = bail_seen.clone();
        rig.root
            .listen::<Args>()
            .name("t/nb")
            .on_sync(move |_: Arc<Args>, call: SyncCall<'_, Args>| {
                let refused = call.next().unwrap_err();
                *seen.lock().unwrap() = Some((refused.to_string(), refused.wire_name()));
                Ok(None)
            })
            .unwrap();
        let answered = rig
            .dispatch(Some("chat"), "t/nb", "bail", vec![])
            .await
            .unwrap();
        assert_eq!(answered, Value::Undefined);
        assert_eq!(
            bail_seen.lock().unwrap().take().unwrap(),
            (
                "next is available only in a waterfall (this is bail)".to_owned(),
                Some("WaterfallNext")
            )
        );
        let async_seen: Misuse = Arc::default();
        let seen = async_seen.clone();
        rig.root
            .listen::<Args>()
            .name("t/na")
            .on_sync(move |_: Arc<Args>, call: SyncCall<'_, Args>| {
                let refused = call.next().unwrap_err();
                *seen.lock().unwrap() = Some((refused.to_string(), refused.wire_name()));
                Ok(None)
            })
            .unwrap();
        let answered = rig
            .dispatch(Some("chat"), "t/na", "waterfall", vec![])
            .await
            .unwrap();
        assert_eq!(answered, Value::Undefined);
        assert_eq!(
            async_seen.lock().unwrap().take().unwrap(),
            (
                "a synchronous listener cannot call next in an asynchronous waterfall; register \
                 it with on"
                    .to_owned(),
                Some("WaterfallNext")
            )
        );
    })
    .await;
}

/// rule: EVP-11-7
///
/// A frame the host rejects fails with a `BindingError` carrying the
/// message the protocol gives for that operation: a dispatch that is not a
/// record, one without a name or a mode, one with an unknown mode, and an
/// operation the session does not offer. The session goes on afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn rejected_frames_are_binding_errors_with_the_protocol_message() {
    within("rejected frames", async {
        let rig = rig().await;
        let refused = rig
            .send("events.dispatch", Value::Data(json!("not a dispatch")))
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            ("BindingError", "events.dispatch needs its dispatch")
        );
        let refused = rig
            .send("events.dispatch", Value::Record(BTreeMap::new()))
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            ("BindingError", "events.dispatch needs a name")
        );
        let mut fields = BTreeMap::new();
        fields.insert("name".into(), Value::Data(json!("t/x")));
        let refused = rig
            .send("events.dispatch", Value::Record(fields.clone()))
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            ("BindingError", "events.dispatch needs a mode")
        );
        fields.insert("mode".into(), Value::Data(json!("sync")));
        let refused = rig
            .send("events.dispatch", Value::Record(fields))
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            ("BindingError", "events.dispatch: unknown mode sync")
        );
        let refused = rig
            .send("events.reload", Value::Undefined)
            .await
            .unwrap_err();
        assert_eq!(
            binding(&refused),
            ("BindingError", "events.reload is not offered here")
        );
        // Still there.
        rig.load("chat", None, &[]).await;
        let answered = rig
            .dispatch(Some("chat"), "t/none", "serial", vec![])
            .await
            .unwrap();
        assert_eq!(answered, Value::Undefined);
    })
    .await;
}

/// rule: EVP-11-8
///
/// A listener's own error crosses the event path with its name, message and
/// error graph, as a call's error does: `serial`, `waterfall` and `bail`
/// each hand the emitter the very error the listener threw.
#[tokio::test(flavor = "multi_thread")]
async fn a_listener_error_carries_its_graph_across_the_event_path() {
    within("listener error graph", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        let graph = json!({
            "root": { "type": "reference", "value": 1 },
            "nodes": [
                null,
                {
                    "type": "error",
                    "name": "KeyError",
                    "message": "'region'",
                    "cause": { "type": "reference", "value": 2 },
                },
                { "type": "data", "value": { "region": null } },
            ],
        });
        let throwing = {
            let graph = graph.clone();
            Arc::new(move |_: &BTreeMap<String, Value>| -> Reply {
                Err(Error::Remote {
                    name: "KeyError".into(),
                    message: "'region'".into(),
                    graph: Some(graph.clone()),
                })
            })
        };
        rig.register_with("1", "t/err", Some("chat"), "one", "sync", throwing)
            .await
            .unwrap();
        for mode in ["serial", "waterfall", "bail"] {
            let refused = rig
                .dispatch(Some("chat"), "t/err", mode, vec![])
                .await
                .unwrap_err();
            let Error::Remote {
                name,
                message,
                graph: seen,
            } = &refused
            else {
                panic!("{mode}: expected KeyError, got {refused:?}");
            };
            assert_eq!(name, "KeyError", "{mode}");
            assert_eq!(message, "'region'", "{mode}");
            assert_eq!(
                seen.as_ref(),
                Some(&graph),
                "{mode}: the graph crossed as it was"
            );
        }
    })
    .await;
}

/// rule: EVP-12.2-9
///
/// A runtime's plugin hears `rutis/*` (the host emits it), but cannot emit
/// it: a dispatch of such a name in any of the five modes is refused with
/// `ReservedEvent`, and no listener is called. (`internal/*` is covered by
/// `internal_names_are_refused_and_never_registered`.)
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_hears_rutis_events_but_cannot_emit_them() {
    within("rutis/* emitted", async {
        let rig = rig().await;
        rig.load("chat", None, &[]).await;
        // Listening to one is allowed.
        rig.register("1", "rutis/service", None, "one", "any")
            .await
            .unwrap();
        for mode in ["emit", "parallel", "serial", "waterfall", "bail"] {
            let refused = rig
                .dispatch(Some("chat"), "rutis/service", mode, vec![])
                .await
                .unwrap_err();
            assert_eq!(
                thrown(&refused),
                (
                    "ReservedEvent",
                    "rutis/* events are emitted by rutis only: rutis/service"
                ),
                "{mode}"
            );
        }
        assert!(
            rig.fake.calls.lock().unwrap().is_empty(),
            "no listener was called"
        );
    })
    .await;
}
