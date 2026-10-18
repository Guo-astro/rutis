// The Rust participant of the events conformance suite: the plugin
// `conformance-events` of the host (location `host`) and of the second
// node (`node`), the host's test hooks, and the node. Written with the
// Rust events API (docs/design-events-protocol-2026-10-18.md §12.1:
// `ctx.events()`, `Call`, `SyncCall`, `EventOptions`, `rutis_bridge::events::Args`)
// and the host bus's test hooks, `rutis_bridge::events::testing::Tap`:
// `dispatches()` (events.dispatch received), `hold(n)` (keep the next n
// dispatches until all arrived) and `reported()` (the §5.6 lines of the
// host's error sink).

use std::sync::Arc;
use std::time::Duration;

use rutis::events::{Call, EventOptions, SyncCall};
use rutis::{BoxFuture, CordisError, Ctx, Effect, Event, EventKey, Plugin, PluginFactory};
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

fn thrown(spec: &Json) -> CordisError {
    CordisError::from(Error::Remote {
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
fn perform<'a>(
    ctx: &'a Ctx,
    log: &'a Arc<EventsLog>,
    config: &'a Json,
    does: &'a Json,
    args: &'a Args,
    call: Call<'a, ArgValue>,
) -> BoxFuture<'a, Result<Option<ArgValue>, CordisError>> {
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
            return perform(ctx, log, config, &does["then"], args, call).await;
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
            struct Cancelled(Arc<EventsLog>, String);
            impl Drop for Cancelled {
                fn drop(&mut self) {
                    self.0.add(format!("{}:cancelled", self.1));
                }
            }
            let _guard = Cancelled(log.clone(), config["label"].as_str().unwrap_or_default().into());
            std::future::pending::<()>().await;
        }
        if does.get("calls").is_some() {
            let Some(Value::Reference(function)) = args.0.first().cloned() else {
                return Err(thrown(&json!({"name": "TypeError", "message": "not a function"})));
            };
            let result = function
                .call(Value::List(vec![json!("x").into()]))
                .map_err(CordisError::from)?;
            return Ok(Some(ArgValue(settle(result).await.map_err(CordisError::from)?)));
        }
        if let Some(spec) = does.get("emits") {
            let result = dispatch(ctx, log, spec).await.map_err(CordisError::from)?;
            return Ok(Some(ArgValue(result)));
        }
        Err(thrown(&json!({ "name": "Unknown", "message": format!("does {does}") })))
    })
}

/// `does`, synchronously (`kind: sync`).
fn perform_sync(
    log: &Arc<EventsLog>,
    does: &Json,
    args: &Args,
    call: SyncCall<'_, ArgValue>,
) -> Result<Option<ArgValue>, CordisError> {
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
        return perform_sync(log, &does["then"], args, call);
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
}

impl Record {
    fn add(&self, first: &Json) {
        if let Some(entry) = line(&self.config, first) {
            self.log.add(entry);
        }
    }
}

/// A typed listener (`typed`): echoes its event, or returns `does.returns`
/// for the unit event.
struct TypedListener(Record);

impl rutis::Listener<City> for TypedListener {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a City,
        _: Call<'a, City>,
    ) -> BoxFuture<'a, Result<Option<City>, CordisError>> {
        self.0.add(&json!(event.city));
        Box::pin(std::future::ready(Ok(Some(event.clone()))))
    }
}

impl rutis::Listener<Pair> for TypedListener {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a Pair,
        _: Call<'a, Pair>,
    ) -> BoxFuture<'a, Result<Option<Pair>, CordisError>> {
        self.0.add(&json!(event.0));
        Box::pin(std::future::ready(Ok(Some(event.clone()))))
    }
}

impl rutis::Listener<Unit> for TypedListener {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: &'a Unit,
        _: Call<'a, String>,
    ) -> BoxFuture<'a, Result<Option<String>, CordisError>> {
        self.0.add(&Json::Null);
        let returns = self.0.config["does"]["returns"].as_str().map(str::to_owned);
        Box::pin(std::future::ready(Ok(returns)))
    }
}

/// An async listener of the case's name.
struct AsyncListener(Record);

impl rutis::Listener<Args> for AsyncListener {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        args: &'a Args,
        call: Call<'a, ArgValue>,
    ) -> BoxFuture<'a, Result<Option<ArgValue>, CordisError>> {
        self.0.add(&first_json(args));
        let Record { log, config } = &self.0;
        perform(ctx, log, config, &config["does"], args, call)
    }
}

/// A synchronous listener (`kind: sync`).
struct SyncListener(Record);

impl rutis::SyncListener<Args> for SyncListener {
    fn call(
        &self,
        _: &Ctx,
        args: &Args,
        call: SyncCall<'_, ArgValue>,
    ) -> Result<Option<ArgValue>, CordisError> {
        self.0.add(&first_json(args));
        perform_sync(&self.0.log, &self.0.config["does"], args, call)
    }
}

/// Register the listener of `config` on `ctx`'s bus.
fn listen(ctx: &Ctx, log: &Arc<EventsLog>, config: &Json) -> Result<(), CordisError> {
    let options = EventOptions::default()
        .prepend(config["options"]["prepend"].as_bool().unwrap_or(false))
        .global(config["options"]["global"].as_bool().unwrap_or(false));
    let name = config["name"].as_str().unwrap_or_default().to_owned();
    let record = Record {
        log: log.clone(),
        config: Arc::new(config.clone()),
    };
    let bus = ctx.events();
    match config["typed"].as_str() {
        Some("struct") => {
            bus.on_opt(ctx, &EventKey::<City>::of(), TypedListener(record), options)?;
        }
        Some("tuple") => {
            bus.on_opt(ctx, &EventKey::<Pair>::of(), TypedListener(record), options)?;
        }
        Some(_) => {
            bus.on_opt(ctx, &EventKey::<Unit>::of(), TypedListener(record), options)?;
        }
        None if config["kind"] == "sync" => {
            bus.on_sync_opt(ctx, &EventKey::<Args>::dynamic(name), SyncListener(record), options)?;
        }
        None => {
            bus.on_opt(ctx, &EventKey::<Args>::dynamic(name), AsyncListener(record), options)?;
        }
    }
    Ok(())
}

// ── Emitting ────────────────────────────────────────────────────

/// Emit `spec` from `ctx` with the Rust API and return the result.
async fn dispatch(ctx: &Ctx, log: &Arc<EventsLog>, spec: &Json) -> Reply {
    let name = spec["name"].as_str().unwrap_or_default().to_owned();
    let key = EventKey::<Args>::dynamic(name);
    let args = Args(
        spec["args"]
            .as_array()
            .map(|args| args.iter().map(rutis_bridge::runtime::testing::events::argument).collect())
            .unwrap_or_default(),
    );
    let filter = spec.get("filter").cloned();
    let log2 = log.clone();
    let bus = ctx.events();
    let filtered = filter.map(|filter| {
        bus.filter(move |info| {
            if filter.get("throws").is_some() {
                panic!("a Rust filter cannot throw");
            }
            log2.filter_description(serde_json::to_value(info).unwrap());
            let row = info.row.clone().unwrap_or_default();
            filter["keep"].as_array().into_iter().flatten().any(|k| k.as_str() == Some(&row))
        })
    });
    let terminal = spec["terminal"].as_str().map(str::to_owned);
    let value = |v: Option<ArgValue>| v.map(|v| v.0).unwrap_or(Value::Undefined);
    let result: Result<Value, CordisError> = match (spec["mode"].as_str().unwrap_or_default(), &filtered) {
        ("emit", None) => bus.emit(ctx, &key, Arc::new(args)).map(|()| Value::Undefined),
        ("emit", Some(f)) => f.emit(ctx, &key, Arc::new(args)).map(|()| Value::Undefined),
        ("parallel", None) => bus.parallel(ctx, &key, Arc::new(args)).await.map(|()| Value::Undefined),
        ("parallel", Some(f)) => f.parallel(ctx, &key, Arc::new(args)).await.map(|()| Value::Undefined),
        ("serial", None) => bus.serial(ctx, &key, Arc::new(args)).await.map(value),
        ("serial", Some(f)) => f.serial(ctx, &key, Arc::new(args)).await.map(value),
        ("waterfall", _) => {
            let terminal = TerminalOf {
                log: log.clone(),
                terminal,
            };
            bus.waterfall(ctx, &key, &args, terminal).await.map(value)
        }
        ("bail", None) => bus.bail_sync(ctx, &key, &args).map(value),
        ("bail", Some(f)) => f.bail_sync(ctx, &key, &args).map(value),
        ("waterfall-sync", _) => bus
            .waterfall_sync(ctx, &key, &args, |_, _| {
                Ok(terminal.map(|t| {
                    log.add("terminal");
                    ArgValue(json!(t).into())
                }))
            })
            .map(value),
        (mode, _) => return Err(Error::Value(format!("unknown mode {mode}"))),
    };
    result.map_err(|e| rutis_bridge::events::to_session_error(&e))
}

/// The end of a waterfall (`terminal`), logged when reached.
struct TerminalOf {
    log: Arc<EventsLog>,
    terminal: Option<String>,
}

impl rutis::events::Terminal<Args> for TerminalOf {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        _: &'a Args,
    ) -> BoxFuture<'a, Result<Option<ArgValue>, CordisError>> {
        let result = self.terminal.as_ref().map(|t| {
            self.log.add("terminal");
            ArgValue(json!(t).into())
        });
        Box::pin(std::future::ready(Ok(result)))
    }
}

/// The service `emitter-<label>`.
struct Emitter {
    ctx: Ctx,
    log: Arc<EventsLog>,
}

impl HostDispatch for Emitter {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let spec = args.list()?.into_iter().next().unwrap_or(Value::Undefined);
        let spec = rutis_bridge::runtime::testing::events::written(spec)?;
        let (ctx, log) = (self.ctx.clone(), self.log.clone());
        match method {
            "dispatch" => Ok(Value::future(async move { dispatch(&ctx, &log, &spec).await })),
            _ => tokio::runtime::Handle::current().block_on(dispatch(&ctx, &log, &spec)),
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
}

impl Plugin for Participant {
    fn name(&self) -> &str {
        "conformance-events"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            if let Some(row) = &self.row {
                ctx.mark_row(row)?;
            }
            if let Some(fails) = self.config.get("fails") {
                return Err(thrown(fails));
            }
            if let Some(name) = self.config["provides"].as_str() {
                ctx.provide_as::<dyn HostDispatch>(host_key(name), Arc::new(Nothing))?;
            }
            if self.config["emitter"] == true {
                let label = self.config["label"].as_str().unwrap_or_default();
                ctx.provide_as::<dyn HostDispatch>(
                    host_key(&format!("emitter-{label}")),
                    Arc::new(Emitter { ctx: ctx.clone(), log: self.log.clone() }),
                )?;
            }
            if self.config.get("name").is_some() {
                listen(ctx, &self.log, &self.config)?;
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
    fn build(&self, config: &Json) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(Participant { config: config.clone(), log: self.0.clone(), row: None }))
    }
}

// ── The host's hooks ────────────────────────────────────────────

/// The host's test hooks of its bus.
pub struct Hooks(Tap);

impl Hooks {
    pub fn dispatches(&self) -> u64 {
        self.0.dispatches()
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
    builtins.register::<Json, _>("conformance-events", Factory(log));
    Hooks(tap)
}

// ── The second node ─────────────────────────────────────────────

/// A second rutis node linked to the host over the memory transport; both
/// export and import `t/*` and `rutis/*` (not `n/*`). Its participants are
/// Rust plugins at its root.
pub struct Node {
    host: Ctx,
    node: std::sync::Mutex<Option<Ctx>>,
    log: Arc<EventsLog>,
    rows: std::sync::Mutex<std::collections::BTreeMap<String, rutis::FiberView>>,
}

fn shared() -> Vec<String> {
    vec!["t/*".into(), "rutis/*".into()]
}

impl Node {
    pub async fn start(host: &Ctx, log: Arc<EventsLog>) -> Node {
        let node = Node {
            host: host.clone(),
            node: Default::default(),
            log,
            rows: Default::default(),
        };
        node.link().await;
        node
    }

    async fn link(&self) {
        use rutis_bridge::transport::memory::{MemoryPlugin, MemoryTransport};
        use rutis_bridge::{Credential, Features, IdentityPlugin, LinkConfig, PeerPlugin, StaticIdentity};
        let id = |s: &str| rutis_bridge::PeerId::new(s).unwrap();
        let transport = Arc::new(MemoryTransport::default());
        transport.endpoint("host-in", id("host"));
        (&self.host.plugin(MemoryPlugin::with_transport(transport.clone()))).await.unwrap();
        self.host.plugin(IdentityPlugin::new(
            "host",
            StaticIdentity::new(id("host")).accept_token("node-token", id("node")),
        ));
        let features = Features { export: shared(), import: shared(), ..Features::default() };
        self.host.plugin(
            PeerPlugin::new(LinkConfig::listen(id("node"), "memory", "host", "host-in"), features.clone()).unwrap(),
        );
        let node = Ctx::root().unwrap();
        (&node.plugin(MemoryPlugin::with_transport(transport))).await.unwrap();
        node.plugin(IdentityPlugin::new(
            "node",
            StaticIdentity::new(id("node")).present(id("host"), Credential::Bearer("node-token".into())),
        ));
        node.plugin(PeerPlugin::new(LinkConfig::dial(id("host"), "memory", "node", "host-in"), features).unwrap());
        *self.node.lock().unwrap() = Some(node);
    }

    fn ctx(&self) -> Result<Ctx, String> {
        self.node.lock().unwrap().clone().ok_or_else(|| "the node is killed".into())
    }

    pub async fn load(&self, row: &Row) -> Result<(), String> {
        let ctx = self.ctx()?;
        let view = ctx.plugin(Participant {
            config: row.config.clone(),
            log: self.log.clone(),
            row: Some(row.label.clone()),
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
        self.ctx().ok()?.get_as::<dyn HostDispatch>(host_key(&format!("emitter-{label}")))
    }

    pub async fn kill(&self) -> Result<(), String> {
        let node = self.node.lock().unwrap().take().ok_or("already killed")?;
        self.rows.lock().unwrap().clear();
        node.shutdown().await.map_err(|e| e.to_string())
    }

    pub async fn restart(&self) -> Result<(), String> {
        self.link().await;
        Ok(())
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

