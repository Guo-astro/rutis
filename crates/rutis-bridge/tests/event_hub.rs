//! The host as the hub of the logical bus (events protocol §2–§8), against
//! a fake runtime speaking the protocol over a memory session: what every
//! real runtime does is checked by the event expectation file; this checks
//! the host's side of the wire on its own.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{ListenerInfo, Scope};
use rutis::{BoxFuture, Call, CordisError, Ctx, Event, EventKey, Listener, SyncCall, SyncListener};
use rutis_bridge::channel::PeerId;
use rutis_bridge::events::{ArgValue, Args};
use rutis_bridge::runtime::{Mount, Process};
use rutis_bridge::session::{settle, Connection, Dispatch, Endpoint, Error, Format, Reply, Value};
use rutis_test_support::within;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// What a fake listener does when called: its answer.
type Behavior = Arc<dyn Fn(&BTreeMap<String, Value>) -> Reply + Send + Sync>;

/// A runtime that mounts nothing: it records `events.elsewhere` and the
/// `events.call`s it gets, answering each by its listener's behavior.
#[derive(Default)]
struct Fake {
    elsewhere: Mutex<Vec<Json>>,
    calls: Mutex<Vec<BTreeMap<String, Value>>>,
    listeners: Mutex<BTreeMap<String, Behavior>>,
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
            ("", "rows.load") | ("", "rows.unload") => Ok(Value::Undefined),
            ("", "ping") => Ok(Value::Data(json!("pong"))),
            ("", "events.elsewhere") => {
                self.elsewhere.lock().unwrap().push(first()?.json()?);
                Ok(Value::Undefined)
            }
            ("", "events.call") => {
                let Value::Record(fields) = first()? else {
                    return Err(Error::Value("events.call needs a record".into()));
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
            _ => Err(Error::Value(format!("no method {method}"))),
        }
    }
}

/// The host (`root`'s bus) and a fake runtime `rt` declaring `capabilities`.
struct Rig {
    root: Ctx,
    process: Arc<Process>,
    fake: Arc<Fake>,
    far: Connection,
}

async fn rig_with(root: Ctx, endpoint: Endpoint) -> Result<Rig, Error> {
    let (a, b) = rutis_bridge::transport::memory::pair();
    let fake = Arc::new(Fake::default());
    let far = Connection::open_with(b, fake.clone(), Format::Endpoint(endpoint)).unwrap();
    let process = Process::attach(
        a,
        Mount {
            ctx: Some(root.clone()),
            runtime: Some("rt".into()),
            ..Mount::default()
        },
        Format::Endpoint(Endpoint::rust(id("host"))),
    )
    .await?;
    Ok(Rig {
        root,
        process,
        fake,
        far,
    })
}

async fn rig() -> Rig {
    rig_with(Ctx::root().unwrap(), Endpoint::rust(id("rt")))
        .await
        .unwrap()
}

impl Rig {
    async fn send(&self, method: &str, argument: Value) -> Result<Value, Error> {
        let reply = self
            .far
            .invoke_async("", method, Value::List(vec![argument]))
            .await?;
        settle(reply).await
    }

    /// Register listener `id` of `name` for `row` with `behavior`.
    async fn on(&self, id: &str, name: &str, row: Option<&str>, kind: &str, behavior: Behavior) {
        self.fake
            .listeners
            .lock()
            .unwrap()
            .insert(id.into(), behavior);
        self.send(
            "events.on",
            Value::Data(json!({
                "id": id, "name": name, "row": row, "plugin": "fake",
                "kind": kind, "prepend": false, "global": false,
            })),
        )
        .await
        .unwrap();
    }

    async fn load(&self, row: &str, scope: &[Scope]) {
        self.process
            .load_row(
                row,
                std::path::Path::new("fake"),
                json!({}),
                &[],
                &[],
                scope,
            )
            .await
            .unwrap();
    }

    /// `events.dispatch` from `row`.
    async fn dispatch(
        &self,
        row: Option<&str>,
        name: &str,
        mode: &str,
        args: Vec<Value>,
        filter: Option<Value>,
    ) -> Result<Value, Error> {
        let mut fields = BTreeMap::new();
        fields.insert("row".into(), Value::Data(json!(row)));
        fields.insert("name".into(), Value::Data(json!(name)));
        fields.insert("mode".into(), Value::Data(json!(mode)));
        fields.insert("args".into(), Value::List(args));
        if let Some(filter) = filter {
            fields.insert("filter".into(), filter);
        }
        self.send("events.dispatch", Value::Record(fields)).await
    }

    fn elsewhere(&self) -> Vec<Json> {
        self.fake.elsewhere.lock().unwrap().clone()
    }
}

fn answers(answer: Json) -> Behavior {
    Arc::new(move |_| Ok(Value::Data(answer.clone())))
}

fn key(name: &str) -> EventKey<Args> {
    EventKey::dynamic(name.to_owned())
}

fn args(values: &[Json]) -> Arc<Args> {
    Arc::new(Args(values.iter().cloned().map(Value::Data).collect()))
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

async fn eventually(mut check: impl FnMut() -> bool, what: &str) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Answers a fixed value; counts its calls.
struct Answer(Option<&'static str>, Arc<Mutex<Vec<Vec<Value>>>>);
impl Listener<Args> for Answer {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a Args,
        _: Call<'a, ArgValue>,
    ) -> BoxFuture<'a, Result<Option<ArgValue>, CordisError>> {
        Box::pin(async move {
            self.1.lock().unwrap().push(event.0.clone());
            Ok(self.0.map(|answer| ArgValue(Value::Data(json!(answer)))))
        })
    }
}

/// guarantee: events/v2-required
///
/// A far end that does not declare `events-v2` is refused right after the
/// handshake, with the message of events protocol §2.2 naming it.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_events_v2_is_refused() {
    within("refused", async {
        let mut old = Endpoint::rust(id("rt"));
        old.capabilities
            .retain(|capability| capability != "events-v2");
        old.implementation.name = "old-runtime".into();
        old.implementation.version = "0.12.1".into();
        let error = rig_with(Ctx::root().unwrap(), old)
            .await
            .err()
            .expect("refused");
        assert!(
            error.to_string().contains(
                "incompatible session: the far end (old-runtime 0.12.1) does not declare \
                 events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later"
            ),
            "{error}"
        );
    })
    .await;
}

/// guarantee: events/presence
///
/// The runtime hears the names with listeners elsewhere first (before
/// `mount`), then each change: a host listener added and withdrawn, a
/// prefix entry; its own entries are never elsewhere to it.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_hears_what_is_elsewhere() {
    within("presence", async {
        let root = Ctx::root().unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        root.events()
            .on(&root, &key("t/before"), Answer(None, heard.clone()))
            .unwrap();
        let rig = rig_with(root.clone(), Endpoint::rust(id("rt")))
            .await
            .unwrap();
        assert_eq!(rig.elsewhere()[0], json!({ "add": ["t/before"] }));

        let later = root
            .events()
            .on(&root, &key("t/later"), Answer(None, heard.clone()))
            .unwrap();
        // Queued before `on` returned.
        eventually(|| rig.elsewhere().len() == 2, "t/later").await;
        assert_eq!(rig.elsewhere()[1], json!({ "add": ["t/later"] }));
        later.dispose().await.unwrap();
        eventually(|| rig.elsewhere().len() == 3, "t/later gone").await;
        assert_eq!(rig.elsewhere()[2], json!({ "remove": ["t/later"] }));

        // Its own listener is not elsewhere to it.
        rig.on("1", "t/own", None, "async", answers(json!(null)))
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(rig.elsewhere().len(), 3);
    })
    .await;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Created(String, u32);
impl Event for Created {
    const NAME: &'static str = "t/created";
    type Value = String;
}

struct Typed(Arc<Mutex<Vec<Created>>>);
impl Listener<Created> for Typed {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a Created,
        _: Call<'a, String>,
    ) -> BoxFuture<'a, Result<Option<String>, CordisError>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(event.clone());
            Ok(Some(format!("rust saw {}", event.0)))
        })
    }
}

/// guarantee: events/hub-dispatch
///
/// A host emission reaches the runtime's listener as one `events.call`
/// (a typed event as one argument per tuple field); a runtime's dispatch
/// through the host reaches the host's typed listener and the runtime's own
/// listener (`own`), in registration order, `serial` taking the first
/// result.
#[tokio::test(flavor = "multi_thread")]
async fn dispatches_cross_between_host_and_runtime() {
    within("dispatch", async {
        let rig = rig().await;
        rig.load("chat", &[]).await;
        rig.on(
            "1",
            "t/created",
            Some("chat"),
            "async",
            answers(json!(false)),
        )
        .await;
        // Host -> runtime: typed, two arguments.
        let result = rig
            .root
            .events()
            .serial(
                &rig.root,
                &EventKey::<Created>::of(),
                Arc::new(Created("x".into(), 3)),
            )
            .await
            .unwrap();
        assert_eq!(result, None, "false is no result");
        let call = rig.fake.calls.lock().unwrap()[0].clone();
        assert_eq!(call["listener"], Value::Data(json!("1")));
        assert_eq!(call["mode"], Value::Data(json!("serial")));
        assert_eq!(call["own"], Value::Data(json!(false)));
        assert_eq!(
            call["args"],
            Value::List(vec![Value::Data(json!("x")), Value::Data(json!(3))])
        );
        assert_eq!(call["signal"], Value::Signal);

        // Runtime -> host -> both: the runtime's first, then the host's.
        let typed = Arc::new(Mutex::new(Vec::new()));
        rig.root
            .events()
            .on(&rig.root, &EventKey::<Created>::of(), Typed(typed.clone()))
            .unwrap();
        let result = rig
            .dispatch(
                Some("chat"),
                "t/created",
                "serial",
                vec![json!("y").into(), json!(4).into()],
                None,
            )
            .await
            .unwrap();
        assert_eq!(result, Value::Data(json!("rust saw y")));
        assert_eq!(typed.lock().unwrap()[0], Created("y".into(), 4));
        let own = rig.fake.calls.lock().unwrap()[1].clone();
        assert_eq!(own["own"], Value::Data(json!(true)));
    })
    .await;
}

/// guarantee: events/hub-scope
///
/// Scope: a runtime's dispatch is emitted at its row's scope, never one it
/// names; a listener hears its scope and below. A row not loaded on the
/// session can neither register nor dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn rows_dispatch_and_hear_at_their_scopes() {
    within("scope", async {
        let rig = rig().await;
        let t1 = scope(&[("17", "tenant")]);
        let t2 = scope(&[("18", "tenant")]);
        rig.load("a", &t1).await;
        rig.load("b", &t2).await;
        rig.load("root", &[]).await;
        rig.on("in-t2", "t/x", Some("b"), "async", answers(json!("t2")))
            .await;
        rig.on(
            "at-root",
            "t/x",
            Some("root"),
            "async",
            answers(json!(null)),
        )
        .await;
        let heard = Arc::new(Mutex::new(Vec::new()));
        rig.root
            .events()
            .on(&rig.root, &key("t/x"), Answer(Some("host"), heard.clone()))
            .unwrap();
        // From t1: the root listeners hear it, t2's does not.
        let result = rig
            .dispatch(Some("a"), "t/x", "parallel", vec![], None)
            .await
            .unwrap();
        assert_eq!(result, Value::Undefined);
        let listeners: Vec<Json> = rig
            .fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call["listener"].clone().json().unwrap())
            .collect();
        assert_eq!(listeners, vec![json!("at-root")]);
        assert_eq!(heard.lock().unwrap().len(), 1);

        let error = rig
            .dispatch(Some("ghost"), "t/x", "emit", vec![], None)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("row ghost is not loaded in this runtime"),
            "{error}"
        );
    })
    .await;
}

/// guarantee: events/hub-filter
///
/// A dispatch's filter is the emitter's selector, called once with every
/// candidate's descriptor (the scope rule applied first), answering one
/// boolean each; a wrong count fails the dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn the_filter_runs_in_the_emitters_process() {
    within("filter", async {
        let rig = rig().await;
        rig.load("chat", &[]).await;
        let heard = Arc::new(Mutex::new(Vec::new()));
        rig.root
            .events()
            .on(&rig.root, &key("t/f"), Answer(Some("kept"), heard.clone()))
            .unwrap();
        rig.root
            .events()
            .on(
                &rig.root,
                &key("t/f"),
                Answer(Some("dropped"), heard.clone()),
            )
            .unwrap();
        let seen: Arc<Mutex<Vec<Json>>> = Arc::default();
        let recorded = seen.clone();
        let selector = Value::callback(move |args| {
            let descriptors = args.list()?.remove(0).json()?;
            recorded.lock().unwrap().push(descriptors.clone());
            let keep: Vec<bool> = descriptors
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, _)| i == 1)
                .collect();
            Ok(Value::Data(json!(keep)))
        });
        let result = rig
            .dispatch(Some("chat"), "t/f", "serial", vec![], Some(selector))
            .await
            .unwrap();
        assert_eq!(result, Value::Data(json!("dropped")));
        let descriptors = seen.lock().unwrap()[0].clone();
        let first: ListenerInfo = serde_json::from_value(descriptors[0].clone()).unwrap();
        assert_eq!(first.name, "t/f");
        assert_eq!(first.runtime, None);
        assert_eq!(first.scope, Vec::new());

        let wrong = Value::callback(|_| Ok(Value::Data(json!([true]))));
        let error = rig
            .dispatch(Some("chat"), "t/f", "serial", vec![], Some(wrong))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("filter of t/f returned 1 answers for 2 listeners"),
            "{error}"
        );
    })
    .await;
}

/// guarantee: events/hub-lifecycle
///
/// A row's entries leave the bus when it unloads, every entry of a session
/// when it ends; later dispatches no longer reach them.
#[tokio::test(flavor = "multi_thread")]
async fn entries_leave_with_their_row_and_their_session() {
    within("lifecycle", async {
        let rig = rig().await;
        rig.load("chat", &[]).await;
        rig.load("other", &[]).await;
        rig.on("1", "t/l", Some("chat"), "async", answers(json!("chat")))
            .await;
        rig.on("2", "t/l", Some("other"), "async", answers(json!("other")))
            .await;
        let serial = || async {
            rig.root
                .events()
                .serial(&rig.root, &key("t/l"), args(&[]))
                .await
                .map(|value| value.map(|value| value.0))
        };
        assert_eq!(serial().await.unwrap(), Some(Value::Data(json!("chat"))));
        rig.process.unload_row("chat").await.unwrap();
        assert_eq!(serial().await.unwrap(), Some(Value::Data(json!("other"))));
        rig.far.close(Error::Transport("gone".into()));
        eventually(
            || rig.process.connection().close_reason().is_some(),
            "the session's end",
        )
        .await;
        let mut gone = false;
        for _ in 0..200 {
            // A call already on its way to the ended session fails once
            // (events protocol §7.2); then the entries are gone.
            if matches!(serial().await, Ok(None)) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(gone, "the session's entries left with it");
    })
    .await;
}

/// guarantee: events/reserved
///
/// A runtime cannot emit `rutis/*` (`ReservedEvent`) nor register
/// `internal/*`.
#[tokio::test(flavor = "multi_thread")]
async fn reserved_and_internal_names_are_refused() {
    within("reserved", async {
        let rig = rig().await;
        let error = rig
            .dispatch(None, "rutis/row", "emit", vec![], None)
            .await
            .unwrap_err();
        match error {
            Error::Remote { name, message, .. } => {
                assert_eq!(name, "ReservedEvent");
                assert_eq!(
                    message,
                    "rutis/* events are emitted by rutis only: rutis/row"
                );
            }
            other => panic!("{other:?}"),
        }
        let error = rig
            .send(
                "events.on",
                Value::Data(json!({
                    "id": "9", "name": "internal/dispatch", "row": null,
                    "plugin": "fake", "kind": "async",
                })),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("event internal/dispatch is internal to its runtime"),
            "{error}"
        );
    })
    .await;
}

struct SyncAnswer;
impl SyncListener<Args> for SyncAnswer {
    fn call(
        &self,
        _: &Ctx,
        _: &Args,
        _: SyncCall<'_, ArgValue>,
    ) -> Result<Option<ArgValue>, CordisError> {
        Ok(Some(ArgValue(Value::Data(json!("sync host")))))
    }
}

/// guarantee: events/hub-bail
///
/// `bail` runs every listener synchronously: a runtime listener known to
/// be asynchronous fails it (`AsyncListener`); a synchronous one answers;
/// the runtime's own `bail` through the host gets a host listener's answer.
#[tokio::test(flavor = "multi_thread")]
async fn bail_is_synchronous_everywhere() {
    within("bail", async {
        let rig = rig().await;
        rig.load("chat", &[]).await;
        rig.on("s", "t/b", Some("chat"), "sync", answers(json!(null)))
            .await;
        let root = rig.root.clone();
        let result = tokio::task::spawn_blocking(move || {
            root.events().bail_sync(&root, &key("t/b"), &Args(vec![]))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, None);
        assert_eq!(
            rig.fake.calls.lock().unwrap()[0]["mode"],
            Value::Data(json!("bail"))
        );

        rig.on("a", "t/b", Some("chat"), "async", answers(json!(null)))
            .await;
        let root = rig.root.clone();
        let error = tokio::task::spawn_blocking(move || {
            root.events().bail_sync(&root, &key("t/b"), &Args(vec![]))
        })
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.wire_name(), Some("AsyncListener"), "{error}");

        rig.process.unload_row("chat").await.unwrap();
        rig.load("chat", &[]).await;
        rig.root
            .events()
            .on_sync(&rig.root, &key("t/b2"), SyncAnswer)
            .unwrap();
        let result = rig
            .dispatch(Some("chat"), "t/b2", "bail", vec![], None)
            .await
            .unwrap();
        assert_eq!(result, Value::Data(json!("sync host")));
    })
    .await;
}

/// guarantee: events/hub-errors
///
/// `parallel` gathers every failure in registration order as one
/// `AggregateError`; a value the runtime does not take fails only that
/// invocation.
#[tokio::test(flavor = "multi_thread")]
async fn parallel_failures_are_aggregated_in_registration_order() {
    within("errors", async {
        let mut endpoint = Endpoint::rust(id("rt"));
        endpoint
            .capabilities
            .retain(|capability| capability != "value-bytes");
        let rig = rig_with(Ctx::root().unwrap(), endpoint).await.unwrap();
        rig.load("chat", &[]).await;
        rig.on(
            "1",
            "t/p",
            Some("chat"),
            "async",
            Arc::new(|_| {
                Err(Error::Remote {
                    name: "Busy".into(),
                    message: "first".into(),
                    graph: None,
                })
            }),
        )
        .await;
        rig.on("2", "t/p", Some("chat"), "async", answers(json!(null)))
            .await;
        let error = rig
            .root
            .events()
            .parallel(&rig.root, &key("t/p"), args(&[json!(1)]))
            .await
            .unwrap_err();
        let CordisError::ListenersFailed { name, errors } = &error else {
            panic!("{error:?}");
        };
        assert_eq!(name, "t/p");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].to_string().contains("first"), "{error}");

        // Bytes: the runtime does not take them; each invocation fails.
        let error = rig
            .root
            .events()
            .parallel(
                &rig.root,
                &key("t/p"),
                Arc::new(Args(vec![Value::Bytes(b"x".to_vec())])),
            )
            .await
            .unwrap_err();
        let CordisError::ListenersFailed { errors, .. } = &error else {
            panic!("{error:?}");
        };
        assert_eq!(errors.len(), 2);
        for error in errors {
            assert!(error.to_string().contains("value-bytes"), "{error}");
        }
    })
    .await;
}

/// Captures the context it is applied in.
struct Owner(Mutex<Option<tokio::sync::oneshot::Sender<Ctx>>>);
impl rutis::Plugin for Owner {
    fn name(&self) -> &str {
        "owner"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<rutis::Effect, CordisError>> {
        Box::pin(async move {
            if let Some(sender) = self.0.lock().unwrap().take() {
                let _ = sender.send(ctx.clone());
            }
            Ok(rutis::Effect::Done)
        })
    }
}

/// guarantee: events/hub-lifecycle
///
/// The plugin a runtime's entries belong to unloads while a call of one of
/// them waits for the runtime's answer, which never comes: the unload does
/// not wait for it (it would end the session only after its wait); the call
/// fails once with the session's end (events protocol §7.2), and nothing is
/// left running past the shutdown deadline.
#[tokio::test(flavor = "multi_thread")]
async fn an_unload_does_not_wait_for_a_call_the_runtime_never_answers() {
    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    let into = errors.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |error: Arc<CordisError>| into.lock().unwrap().push(error.to_string())),
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let view = root.plugin(Owner(Mutex::new(Some(sender))));
    (&view).await.unwrap();
    let owner = receiver.await.unwrap();
    let rig = rig_with(owner, Endpoint::rust(id("rt"))).await.unwrap();
    rig.on(
        "1",
        "t/hang",
        None,
        "async",
        Arc::new(|_| Ok(Value::future(std::future::pending::<Reply>()))),
    )
    .await;
    let emitter = root.clone();
    let parallel = tokio::spawn(async move {
        emitter
            .events()
            .parallel(&emitter, &key("t/hang"), args(&[]))
            .await
    });
    eventually(
        || !rig.fake.calls.lock().unwrap().is_empty(),
        "the call reached the runtime",
    )
    .await;
    within("the unload", view.dispose()).await.unwrap();
    let failed = within("the call", parallel).await.unwrap().unwrap_err();
    assert!(
        failed.to_string().contains("1 listeners of t/hang failed"),
        "{failed}"
    );
    let errors = errors.lock().unwrap().clone();
    assert!(
        !errors.iter().any(|line| line.contains("still running")),
        "{errors:?}"
    );
}

/// guarantee: events/hub-dispatch
///
/// A session's event operations never run on its reader: a dispatch
/// observer (plugin code) seeing a runtime's `emit` calls that runtime
/// synchronously over the same session, and gets its answer, which only
/// that reader can read.
#[tokio::test(flavor = "multi_thread")]
async fn plugin_code_seeing_a_runtime_s_emit_can_call_that_runtime() {
    let rig = rig().await;
    let answers: Arc<Mutex<Vec<Json>>> = Arc::default();
    let into = answers.clone();
    let connection = rig.process.connection().clone();
    rig.root
        .events()
        .observe_dispatch(&rig.root, move |attempt| {
            if attempt.name == "t/probe" {
                let answer = connection
                    .invoke("", "ping", Value::List(vec![]))
                    .and_then(Value::json);
                into.lock().unwrap().push(answer.unwrap_or(Json::Null));
            }
        })
        .unwrap();
    within(
        "the emit",
        rig.dispatch(None, "t/probe", "emit", vec![], None),
    )
    .await
    .unwrap();
    eventually(
        || !answers.lock().unwrap().is_empty(),
        "the observer's call",
    )
    .await;
    assert_eq!(*answers.lock().unwrap(), [json!("pong")]);
}

/// `EventKey::<Args>::of()` names no event: it fails clearly instead of
/// dispatching under a made-up name (events protocol §12.1).
#[tokio::test(flavor = "multi_thread")]
async fn args_events_need_a_name() {
    let root = Ctx::root().unwrap();
    let message = "Args events need a name: EventKey::dynamic(name)";
    let emitted = root
        .events()
        .emit(&root, &EventKey::<Args>::of(), args(&[]))
        .unwrap_err();
    assert_eq!(emitted.to_string(), message);
    let refused = root
        .events()
        .on(&root, &EventKey::<Args>::of(), Answer(None, Arc::default()))
        .err()
        .unwrap();
    assert_eq!(refused.to_string(), message);
    // The session error a runtime gets: `to_session_error` is exported only
    // with the `testing` feature.
    #[cfg(feature = "testing")]
    {
        let session = rutis_bridge::events::to_session_error(&refused);
        assert!(
            matches!(&session, Error::Remote { name, .. } if name == "BindingError"),
            "{session:?}"
        );
    }
}
