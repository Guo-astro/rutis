// The Rust participant of the events conformance suite: the plugin
// `conformance-events` of the host (location `host`) and of the second
// node (`node`), the host's test hooks, and the node. Written with the
// Rust events API (docs/design-events-protocol-2026-10-18.md §12.1:
// `ctx.events()`, `Call`, `SyncCall`, `EventOptions`, `rutis_bridge::events::Args`)
// and the host bus's test hooks, `rutis_bridge::events::testing::Tap`:
// `dispatches()` (events.dispatch received), `hold(n)` (keep the next n
// dispatches until all arrived) and `reported()` (the §5.6 lines of the
// host's error sink).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rutis::events::{Call, Listen, SyncCall};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, PluginFactory};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError, Event};
use rutis_bridge::events::testing::Tap;
use rutis_bridge::events::{ArgValue, Args};
use rutis_bridge::runtime::testing::events::{EventsLog, Row};
use rutis_bridge::session::{host_key, settle, Error, HostDispatch, Reply, Value};
use rutis_loader::Builtins;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

// ── Typed events (`typed`) ──────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct City {
    city: String,
}
impl Event for City {
    const NAME: &'static str = "t/typed";
    type Value = City;
}

#[derive(Serialize, Deserialize, Clone)]
struct Pair(String, i64);
impl Event for Pair {
    const NAME: &'static str = "t/typed-pair";
    type Value = Pair;
}

#[derive(Serialize, Deserialize, Clone)]
struct Unit;
impl Event for Unit {
    const NAME: &'static str = "t/typed-unit";
    type Value = String;
}

// ── What a listener does ────────────────────────────────────────

fn thrown(spec: &Json) -> RutisError {
    RutisError::from(Error::Remote {
        name: spec["name"].as_str().unwrap_or("Error").to_owned(),
        message: spec["message"].as_str().unwrap_or_default().to_owned(),
        graph: None,
    })
}

fn fmt(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The line a call logs (`log`), from its first argument as JSON.
fn line(config: &Json, first: &Json) -> Option<String> {
    let label = config["label"].as_str().unwrap_or_default();
    match &config["log"] {
        Json::Bool(false) => None,
        Json::String(how) if how == "arg" => Some(format!("{label}:{}", fmt(first))),
        Json::Array(fields) => Some(format!(
            "{label}:{}",
            fields
                .iter()
                .map(|f| fmt(&first[f.as_str().unwrap_or_default()]))
                .collect::<Vec<_>>()
                .join(",")
        )),
        _ => Some(label.to_owned()),
    }
}

fn no_result(value: &Option<ArgValue>) -> bool {
    matches!(value, None | Some(ArgValue(Value::Undefined)))
        || matches!(value, Some(ArgValue(Value::Data(Json::Null | Json::Bool(false)))))
}

fn text(value: Option<ArgValue>) -> String {
    match value {
        Some(ArgValue(Value::Data(Json::String(text)))) => text,
        _ => String::new(),
    }
}

fn data(value: &Json) -> ArgValue {
    ArgValue(Value::Data(value.clone()))
}

/// `does`, asynchronously.
#[allow(clippy::too_many_arguments)]
fn perform<'a>(
    ctx: &'a Ctx,
    log: &'a Arc<EventsLog>,
    alive: &'a Arc<AtomicBool>,
    config: &'a Json,
    does: &'a Json,
    args: &'a Args,
    call: Call<Args>,
) -> BoxFuture<'a, Result<Option<ArgValue>, RutisError>> {
    Box::pin(async move {
        if let Some(v) = does.get("returns") {
            return Ok(Some(data(v)));
        }
        if does.get("returnsNothing").is_some() {
            return Ok(None);
        }
        if does.get("echo").is_some() {
            return Ok(args.0.first().cloned().map(ArgValue));
        }
        if let Some(spec) = does.get("throws") {
            return Err(thrown(spec));
        }
        if let Some(ms) = does.get("after").and_then(Json::as_u64) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            return perform(ctx, log, alive, config, &does["then"], args, call).await;
        }
        if let Some(prefix) = does.get("next").and_then(Json::as_str) {
            let rest = call.next().await?;
            let rest = if no_result(&rest) { String::new() } else { text(rest) };
            return Ok(Some(ArgValue(json!(format!("{prefix}{rest}")).into())));
        }
        if let Some(v) = does.get("veto") {
            return Ok(Some(data(v)));
        }
        if does.get("waits").is_some() {
            // Dropped when the call is cancelled: the guard logs it.
            // (Not when its process was killed: a dead process logs nothing.)
            struct Cancelled(Arc<EventsLog>, String, Arc<AtomicBool>);
            impl Drop for Cancelled {
                fn drop(&mut self) {
                    if self.2.load(Ordering::SeqCst) {
                        self.0.add(format!("{}:cancelled", self.1));
                    }
                }
            }
            let label = config["label"].as_str().unwrap_or_default().into();
            let _guard = Cancelled(log.clone(), label, alive.clone());
            // Until the call is dropped (cancelled) or its plugin stops.
            ctx.cancelled().await;
            return Ok(None);
        }
        if does.get("calls").is_some() {
            let Some(Value::Reference(function)) = args.0.first().cloned() else {
                return Err(thrown(&json!({"name": "TypeError", "message": "not a function"})));
            };
            let result = function
                .call(Value::List(vec![json!("x").into()]))
                .map_err(RutisError::from)?;
            return Ok(Some(ArgValue(settle(result).await.map_err(RutisError::from)?)));
        }
        if let Some(spec) = does.get("emits") {
            let result = dispatch(ctx, log, spec, None).await.map_err(RutisError::from)?;
            return Ok(Some(ArgValue(result)));
        }
        Err(thrown(&json!({ "name": "Unknown", "message": format!("does {does}") })))
    })
}

/// `does`, synchronously (`kind: sync`).
fn perform_sync(
    ctx: &Ctx,
    log: &Arc<EventsLog>,
    does: &Json,
    args: &Args,
    call: SyncCall<'_, Args>,
) -> Result<Option<ArgValue>, RutisError> {
    if let Some(spec) = does.get("emits") {
        // A synchronous dispatch (`bail`) from here: it
        // runs to its end inline, never waiting.
        let mut dispatched = std::pin::pin!(dispatch(ctx, log, spec, None));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        return match std::future::Future::poll(dispatched.as_mut(), &mut context) {
            std::task::Poll::Ready(result) => {
                Ok(Some(ArgValue(result.map_err(RutisError::from)?)))
            }
            std::task::Poll::Pending => Err(thrown(&json!({
                "name": "Unknown",
                "message": "a synchronous listener emits only bail",
            }))),
        };
    }
    if let Some(v) = does.get("returns").or_else(|| does.get("veto")) {
        return Ok(Some(data(v)));
    }
    if does.get("returnsNothing").is_some() {
        return Ok(None);
    }
    if does.get("echo").is_some() {
        return Ok(args.0.first().cloned().map(ArgValue));
    }
    if let Some(spec) = does.get("throws") {
        return Err(thrown(spec));
    }
    if let Some(ms) = does.get("after").and_then(Json::as_u64) {
        std::thread::sleep(Duration::from_millis(ms));
        return perform_sync(ctx, log, &does["then"], args, call);
    }
    if let Some(prefix) = does.get("next").and_then(Json::as_str) {
        let rest = call.next()?;
        let rest = if no_result(&rest) { String::new() } else { text(rest) };
        return Ok(Some(ArgValue(json!(format!("{prefix}{rest}")).into())));
    }
    Err(thrown(&json!({ "name": "Unknown", "message": format!("sync does {does}") })))
}

fn first_json(args: &Args) -> Json {
    args.0.first().cloned().and_then(|v| v.json().ok()).unwrap_or(Json::Null)
}

/// Logs the line of a call (`log`).
#[derive(Clone)]
struct Record {
    log: Arc<EventsLog>,
    config: Arc<Json>,
    alive: Arc<AtomicBool>,
}

impl Record {
    fn add(&self, first: &Json) {
        if !self.alive.load(Ordering::SeqCst) {
            return;
        }
        if let Some(entry) = line(&self.config, first) {
            self.log.add(entry);
        }
    }
}

/// The listener of `config` with its options (`prepend`, `global`).
fn listener<'c, E: Event>(ctx: &'c Ctx, config: &Json) -> Listen<'c, E> {
    let mut listen = ctx.listen::<E>();
    if config["options"]["prepend"].as_bool().unwrap_or(false) {
        listen = listen.prepend();
    }
    if config["options"]["global"].as_bool().unwrap_or(false) {
        listen = listen.global();
    }
    listen
}

/// Register the listener of `config` on `ctx`'s bus: a typed one
/// (`typed`) echoes its event, or returns `does.returns` for the unit
/// event; otherwise an async or a synchronous (`kind: sync`) listener of
/// the case's name.
fn listen(
    ctx: &Ctx,
    log: &Arc<EventsLog>,
    alive: &Arc<AtomicBool>,
    config: &Json,
) -> Result<(), RutisError> {
    let name = config["name"].as_str().unwrap_or_default().to_owned();
    let record = Record {
        log: log.clone(),
        config: Arc::new(config.clone()),
        alive: alive.clone(),
    };
    match config["typed"].as_str() {
        Some("struct") => {
            listener::<City>(ctx, config).on(move |event, _| {
                record.add(&json!(event.city));
                std::future::ready(Ok(Some((*event).clone())))
            })?;
        }
        Some("tuple") => {
            listener::<Pair>(ctx, config).on(move |event, _| {
                record.add(&json!(event.0));
                std::future::ready(Ok(Some((*event).clone())))
            })?;
        }
        Some(_) => {
            listener::<Unit>(ctx, config).on(move |_, _| {
                record.add(&Json::Null);
                let returns = record.config["does"]["returns"].as_str().map(str::to_owned);
                std::future::ready(Ok(returns))
            })?;
        }
        None if config["kind"] == "sync" => {
            listener::<Args>(ctx, config)
                .name(name)
                .on_sync(move |args, call| {
                    record.add(&first_json(&args));
                    let ctx = call.ctx().clone();
                    perform_sync(&ctx, &record.log, &record.config["does"], &args, call)
                })?;
        }
        None => {
            listener::<Args>(ctx, config).name(name).on(move |args, call| {
                record.add(&first_json(&args));
                let record = record.clone();
                async move {
                    let ctx = call.ctx().clone();
                    let Record { log, config, alive } = &record;
                    perform(&ctx, log, alive, config, &config["does"], &args, call).await
                }
            })?;
        }
    }
    Ok(())
}

// ── Emitting ────────────────────────────────────────────────────

/// Emit `spec` from `ctx` with the Rust API and return the result.
/// `args`: the arguments as they came (functions included), else the
/// spec's written ones. With `cancel`, the emitter gives up (drops the
/// dispatch) once that listener has logged its start (§5.7).
async fn dispatch(ctx: &Ctx, log: &Arc<EventsLog>, spec: &Json, args: Option<Vec<Value>>) -> Reply {
    let Some(when) = spec["cancel"]["when"].as_str() else {
        return dispatch_now(ctx, log, spec, args).await;
    };
    tokio::select! {
        result = dispatch_now(ctx, log, spec, args) => result,
        waited = log.wait(when, Duration::from_secs(30)) => {
            waited.map_err(Error::Value)?;
            Err(Error::Remote {
                name: "Cancelled".into(),
                message: "the emitter gave up the dispatch".into(),
                graph: None,
            })
        }
    }
}

async fn dispatch_now(
    ctx: &Ctx,
    log: &Arc<EventsLog>,
    spec: &Json,
    args: Option<Vec<Value>>,
) -> Reply {
    let name = spec["name"].as_str().unwrap_or_default().to_owned();
    let args = Args(args.unwrap_or_else(|| {
        spec["args"]
            .as_array()
            .map(|args| args.iter().map(rutis_bridge::runtime::testing::events::argument).collect())
            .unwrap_or_default()
    }));
    let filter = spec.get("filter").cloned();
    let log2 = log.clone();
    let filtered = filter.map(|filter| {
        ctx.try_filter_named(name.clone(), move |info| {
            if let Some(spec) = filter.get("throws") {
                return Err(thrown(spec));
            }
            log2.filter_description(serde_json::to_value(info).unwrap());
            let row = info.row.clone().unwrap_or_default();
            Ok(filter["keep"].as_array().into_iter().flatten().any(|k| k.as_str() == Some(&row)))
        })
    });
    // A text the terminal returns, or `{echo: true}`: its first argument.
    let terminal = match &spec["terminal"] {
        Json::String(text) => Some(TerminalDoes::Text(text.clone())),
        Json::Object(echo) if echo.get("echo") == Some(&Json::Bool(true)) => {
            Some(TerminalDoes::Echo)
        }
        _ => None,
    };
    let value = |v: Option<ArgValue>| v.map(|v| v.0).unwrap_or(Value::Undefined);
    let result: Result<Value, RutisError> = match (spec["mode"].as_str().unwrap_or_default(), &filtered) {
        // As `emit` returns in the other languages: once every call started
        // (the erased dispatch says when; `EventBus::emit` returns before).
        ("emit", None) => {
            let dispatch = rutis::internal::events::Dispatch::new(
                name,
                rutis::events::Mode::Emit,
                rutis::internal::events::Payload::new(Arc::new(args)),
            );
            ctx.events()
                .dispatch(ctx, dispatch)
                .await
                .map(|_| Value::Undefined)
        }
        ("emit", Some(f)) => f.emit(args).map(|()| Value::Undefined),
        ("parallel", None) => ctx.named(name).parallel(args).await.map(|()| Value::Undefined),
        ("parallel", Some(f)) => f.parallel(args).await.map(|()| Value::Undefined),
        ("serial", None) => ctx.named(name).serial(args).await.map(value),
        ("serial", Some(f)) => f.serial(args).await.map(value),
        ("waterfall", _) => {
            let log = log.clone();
            ctx.named(name)
                .waterfall(args)
                .terminal(move |event| {
                    let result = terminal.as_ref().map(|t| {
                        log.add("terminal");
                        t.result(&event)
                    });
                    std::future::ready(Ok(result))
                })
                .await
                .map(value)
        }
        ("bail", None) => ctx.named(name).bail(args).map(value),
        ("bail", Some(f)) => f.bail(args).map(value),
        (mode, _) => return Err(Error::Value(format!("unknown mode {mode}"))),
    };
    result.map_err(|e| rutis_bridge::events::to_session_error(&e))
}

/// What a terminal returns: its text, or its first argument (`echo`).
enum TerminalDoes {
    Text(String),
    Echo,
}

impl TerminalDoes {
    fn result(&self, event: &Args) -> ArgValue {
        match self {
            Self::Text(text) => ArgValue(json!(text).into()),
            Self::Echo => ArgValue(event.0.first().cloned().unwrap_or(Value::Undefined)),
        }
    }
}

/// The service `emitter-<label>`.
struct Emitter {
    ctx: Ctx,
    log: Arc<EventsLog>,
}

impl HostDispatch for Emitter {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let mut spec = args.list()?.into_iter().next().unwrap_or(Value::Undefined);
        // The arguments stay as they came: a function among them is one.
        let arguments = match &mut spec {
            Value::Record(fields) => match fields.remove("args") {
                Some(Value::List(arguments)) => Some(arguments),
                _ => None,
            },
            _ => None,
        };
        let spec = rutis_bridge::runtime::testing::events::written(spec)?;
        let (ctx, log) = (self.ctx.clone(), self.log.clone());
        match method {
            "dispatch" => Ok(Value::future(async move {
                dispatch(&ctx, &log, &spec, arguments).await
            })),
            _ => tokio::runtime::Handle::current().block_on(dispatch(&ctx, &log, &spec, arguments)),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "dispatch": "async", "dispatch_sync": "sync" }))
    }
}

// ── The plugin ──────────────────────────────────────────────────

struct Participant {
    config: Json,
    log: Arc<EventsLog>,
    /// The row its fiber is, on the node (the host's loader marks its rows).
    row: Option<String>,
    /// Its process is alive (a killed node's listeners log nothing).
    alive: Arc<AtomicBool>,
}

impl DynPlugin for Participant {
    fn name(&self) -> &str {
        "conformance-events"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            if let Some(row) = &self.row {
                ctx.mark_row(row)?;
            }
            if let Some(fails) = self.config.get("fails") {
                return Err(thrown(fails));
            }
            if let Some(name) = self.config["provides"].as_str() {
                ctx.provide_at(host_key(name), Arc::new(Nothing))?;
            }
            if self.config["emitter"] == true {
                let label = self.config["label"].as_str().unwrap_or_default();
                ctx.provide_at(
                    host_key(&format!("emitter-{label}")),
                    Arc::new(Emitter { ctx: ctx.clone(), log: self.log.clone() }),
                )?;
            }
            if self.config.get("name").is_some() {
                listen(ctx, &self.log, &self.alive, &self.config)?;
            }
            Ok(Effect::Done)
        })
    }
}

struct Nothing;
impl HostDispatch for Nothing {
    fn invoke(&self, _: &str, _: Value) -> Reply {
        Ok(Value::Undefined)
    }
}

struct Factory(Arc<EventsLog>);
impl PluginFactory<Json> for Factory {
    fn name(&self) -> &str {
        "conformance-events"
    }
    fn build(&self, config: &Json) -> Result<Box<dyn DynPlugin>, RutisError> {
        Ok(Box::new(Participant {
            config: config.clone(),
            log: self.0.clone(),
            row: None,
            alive: Arc::new(AtomicBool::new(true)),
        }))
    }
}

// ── The host's hooks ────────────────────────────────────────────

/// The host's test hooks of its bus.
pub struct Hooks(Tap);

impl Hooks {
    pub fn dispatches(&self) -> u64 {
        self.0.dispatches()
    }
    pub async fn answered(&self) -> Result<(), String> {
        self.0
            .answered(rutis_bridge::runtime::testing::events::scaled(
                rutis_bridge::runtime::testing::events::SETTLE,
            ))
            .await
    }
    pub fn hold(&self, n: usize) -> Result<(), String> {
        self.0.hold(n);
        Ok(())
    }
    pub fn reported(&self) -> Vec<String> {
        self.0.reported()
    }
}

/// Register the Rust `conformance-events` with the host's builtins; `tap`
/// is the host bus's.
pub fn register(builtins: &mut Builtins, log: Arc<EventsLog>, tap: Tap) -> Hooks {
    builtins.register_raw(
        "conformance-events",
        Factory(log),
        crate::common::schema_of::<Json>(),
    );
    Hooks(tap)
}

// ── The second node ─────────────────────────────────────────────

/// A second rutis node linked to the host over the memory transport; both
/// export and import `t/*` and `rutis/*` (not `n/*`). Its participants are
/// Rust plugins at its root.
pub struct Node {
    host: Ctx,
    node: std::sync::Mutex<Option<Ctx>>,
    transport: Arc<rutis_bridge::transport::memory::MemoryTransport>,
    /// Whether the node's process is alive: a killed one logs nothing more.
    alive: std::sync::Mutex<Arc<AtomicBool>>,
    log: Arc<EventsLog>,
    rows: std::sync::Mutex<std::collections::BTreeMap<String, rutis::host::FiberView>>,
    /// The §5.6 lines the node's error sink received, over every process
    /// of it (a restarted node appends).
    reported: Arc<std::sync::Mutex<Vec<String>>>,
}

fn peer_id(s: &str) -> rutis_bridge::PeerId {
    rutis_bridge::PeerId::new(s).unwrap()
}

fn shared() -> Vec<String> {
    vec!["t/*".into(), "rutis/*".into()]
}

impl Node {
    pub async fn start(host: &Ctx, log: Arc<EventsLog>) -> Node {
        let node = Node {
            host: host.clone(),
            node: Default::default(),
            transport: Node::host_side(host).await,
            alive: Default::default(),
            log,
            rows: Default::default(),
            reported: Default::default(),
        };
        node.link().await;
        node
    }

    /// The host's end, once: its transport, identity and listening link.
    async fn host_side(host: &Ctx) -> Arc<rutis_bridge::transport::memory::MemoryTransport> {
        use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
        use rutis_bridge::{Features, IdentityPlugin, LinkConfig, PeerPlugin, StaticIdentity};
        let transport = Arc::new(MemoryTransport::default());
        transport.endpoint("host-in", peer_id("host"));
        (&host.plugin(MemoryPlugin::with_transport(transport.clone()))).await.unwrap();
        host.plugin(IdentityPlugin::new(
            "host",
            StaticIdentity::new(peer_id("host")).accept_token("node-token", peer_id("node")),
        ));
        let features = Features { export: shared(), import: shared(), ..Features::default() };
        host.plugin(
            PeerPlugin::new(LinkConfig::listen(peer_id("node"), "memory", "host", "host-in"), features).unwrap(),
        );
        transport
    }

    /// The node itself, dialing the host: a new process each time.
    async fn link(&self) {
        use rutis_bridge::{Credential, Features, IdentityPlugin, LinkConfig, PeerPlugin, StaticIdentity};
        // Its error sink prints as rutis-host does and records the event
        // failures' lines (`reported`).
        let reported = self.reported.clone();
        let sink: rutis::host::ErrorSink = Arc::new(move |error| {
            match rutis_bridge::events::report_line(&error) {
                Some(line) => {
                    eprintln!("node: {line}");
                    reported.lock().unwrap().push(line);
                }
                None => rutis::host::default_sink()(error),
            }
        });
        let node = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
        // The host's transport, provided as is: the node's end must not
        // close it (`MemoryPlugin` closes every channel and listener of the
        // shared transport when it goes, the host's too).
        node.provide_at(rutis_bridge::transport_key("memory"), self.transport.clone())
            .unwrap();
        node.plugin(IdentityPlugin::new(
            "node",
            StaticIdentity::new(peer_id("node")).present(peer_id("host"), Credential::Bearer("node-token".into())),
        ));
        let features = Features { export: shared(), import: shared(), ..Features::default() };
        node.plugin(
            PeerPlugin::new(LinkConfig::dial(peer_id("host"), "memory", "node", "host-in"), features).unwrap(),
        );
        // A new process: alive until killed.
        let alive = Arc::new(AtomicBool::new(true));
        *self.alive.lock().unwrap() = alive;
        *self.node.lock().unwrap() = Some(node);
    }

    /// The §5.6 lines the node reported so far.
    pub fn reported(&self) -> Vec<String> {
        self.reported.lock().unwrap().clone()
    }

    pub fn ctx(&self) -> Result<Ctx, String> {
        self.node.lock().unwrap().clone().ok_or_else(|| "the node is killed".into())
    }

    pub async fn load(&self, row: &Row) -> Result<(), String> {
        let ctx = self.ctx()?;
        let view = ctx.plugin(Participant {
            config: row.config.clone(),
            log: self.log.clone(),
            row: Some(row.label.clone()),
            alive: self.alive.lock().unwrap().clone(),
        });
        (&view).await.map_err(|e| e.to_string())?;
        self.rows.lock().unwrap().insert(row.label.clone(), view);
        Ok(())
    }

    pub async fn unload(&self, label: &str) -> Result<(), String> {
        let view = self.rows.lock().unwrap().remove(label).ok_or("no such node row")?;
        view.dispose().await.map_err(|e| e.to_string())
    }

    pub async fn update(&self, row: &Row) -> Result<(), String> {
        self.unload(&row.label).await?;
        self.load(row).await
    }

    pub fn emitter(&self, label: &str) -> Option<Arc<dyn HostDispatch>> {
        self.ctx().ok()?.get_at(&host_key(&format!("emitter-{label}")))
    }

    pub async fn kill(&self) -> Result<(), String> {
        let node = self.node.lock().unwrap().take().ok_or("already killed")?;
        self.rows.lock().unwrap().clear();
        self.alive
            .lock()
            .unwrap()
            .store(false, Ordering::SeqCst);
        // As a process that dies: its session goes first, under the calls
        // in flight (the host sees them fail); then the node's plugins. A
        // listener that does not stop fails the case rather than hanging
        // the run.
        let session = node
            .get_at(&rutis_bridge::peer_key(&peer_id("host")))
            .map(|peer| peer.connection().clone());
        if let Some(session) = session {
            session.close(Error::Transport("the node was killed".into()));
        }
        tokio::time::timeout(Duration::from_secs(60), node.shutdown())
            .await
            .map_err(|_| "the node did not shut down within 60 s".to_owned())?
            .map_err(|e| e.to_string())
    }

    pub async fn restart(&self) -> Result<(), String> {
        self.link().await;
        // Linked again: both ends have the session.
        let linked = |ctx: &Ctx, peer: &str| {
            ctx.get_at(&rutis_bridge::peer_key(&peer_id(peer)))
                .is_some()
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let node = self.ctx()?;
            if linked(&node, "host") && linked(&self.host, "node") {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!(
                    "the node did not link again (node side {}, host side {})",
                    linked(&node, "host"),
                    linked(&self.host, "node")
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn reset(&self) -> Result<(), String> {
        let labels: Vec<String> = self.rows.lock().unwrap().keys().cloned().collect();
        for label in labels {
            self.unload(&label).await?;
        }
        if self.node.lock().unwrap().is_none() {
            self.restart().await?;
        }
        Ok(())
    }
}

