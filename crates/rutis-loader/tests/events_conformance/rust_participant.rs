// The Rust participant of the events conformance suite: the plugin
// `conformance-events` of the host (location `host`) and of the second
// node (`node`), the host's test hooks, and the node. Written against the
// events-v2 Rust API (docs/design-events-protocol-2026-10-18.md §12.1:
// `ctx.events()`, `Call`, `SyncCall`, `EventOptions`, `rutis_bridge::events::Args`)
// and the host's test hooks; built with `--cfg rutis_events_v2` until both
// are on the branch. The hooks it assumes of rutis-bridge (events/host):
// `rutis_bridge::events::testing::Tap` on the host's root context, with
// `dispatches() -> u64` (events.dispatch received), `hold(n)` (keep the next
// n dispatches until all arrived) and `reported() -> Vec<String>` (the §5.6
// lines of the host's error sink).

use std::sync::Arc;
use std::time::Duration;

use rutis::events::{Call, EventOptions, SyncCall};
use rutis::{BoxFuture, CordisError, Ctx, Effect, Event, EventKey, Plugin, PluginFactory};
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
            let function = args.0.first().cloned().ok_or_else(|| thrown(&json!({"name": "TypeError"})))?;
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

/// Register the listener of `config` on `ctx`'s bus.
fn listen(ctx: &Ctx, log: &Arc<EventsLog>, config: &Json) -> Result<(), CordisError> {
    let options = EventOptions::default()
        .prepend(config["options"]["prepend"].as_bool().unwrap_or(false))
        .global(config["options"]["global"].as_bool().unwrap_or(false));
    let name = config["name"].as_str().unwrap_or_default().to_owned();
    let record = {
        let (log, config) = (log.clone(), config.clone());
        move |first: &Json| {
            if let Some(entry) = line(&config, first) {
                log.add(entry);
            }
        }
    };
    match config["typed"].as_str() {
        Some(typed) => {
            // A typed listener echoes its event (`does: echo`) or returns
            // what `does` says, as its type.
            let returns = config["does"]["returns"].clone();
            match typed {
                "struct" => ctx.events().on(ctx, &EventKey::<City>::of(), move |_: &Ctx, e: &City, _| {
                    record(&json!(e.city));
                    let e = e.clone();
                    Box::pin(async move { Ok(Some(e)) })
                })?,
                "tuple" => ctx.events().on(ctx, &EventKey::<Pair>::of(), move |_: &Ctx, e: &Pair, _| {
                    record(&json!(e.0));
                    let e = e.clone();
                    Box::pin(async move { Ok(Some(e)) })
                })?,
                _ => ctx.events().on(ctx, &EventKey::<Unit>::of(), move |_: &Ctx, _: &Unit, _| {
                    record(&Json::Null);
                    let returns = returns.as_str().map(str::to_owned);
                    Box::pin(async move { Ok(returns) })
                })?,
            };
        }
        None if config["kind"] == "sync" => {
            let (log, does) = (log.clone(), config["does"].clone());
            ctx.events().on_sync_opt(
                ctx,
                &EventKey::<Args>::dynamic(name),
                move |_: &Ctx, args: &Args, call: SyncCall<'_, ArgValue>| {
                    record(&first_json(args));
                    perform_sync(&log, &does, args, call)
                },
                options,
            )?;
        }
        None => {
            let (log, config) = (log.clone(), Arc::new(config.clone()));
            ctx.events().on_opt(
                ctx,
                &EventKey::<Args>::dynamic(name),
                move |ctx: &Ctx, args: &Args, call: Call<'_, ArgValue>| {
                    record(&first_json(args));
                    let (log, config) = (log.clone(), config.clone());
                    Box::pin(async move {
                        perform(ctx, &log, &config, &config["does"], args, call).await
                    })
                },
                options,
            )?;
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
            let log = log.clone();
            let terminal = move |_: &Ctx, _: &Args| -> BoxFuture<'_, Result<Option<ArgValue>, CordisError>> {
                let terminal = terminal.clone();
                let log = log.clone();
                Box::pin(async move {
                    Ok(terminal.map(|t| {
                        log.add("terminal");
                        ArgValue(json!(t).into())
                    }))
                })
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
    result.map_err(|e| Error::Remote {
        name: e.name().to_owned(),
        message: e.to_string(),
        graph: None,
    })
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
}

impl Plugin for Participant {
    fn name(&self) -> &str {
        "conformance-events"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
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
        Ok(Box::new(Participant { config: config.clone(), log: self.0.clone() }))
    }
}

// ── The host's hooks ────────────────────────────────────────────

/// The host's test hooks of its bus (events/host).
pub struct Hooks(rutis_bridge::events::testing::Tap);

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

/// Register the Rust `conformance-events` with the host's builtins.
pub fn register(builtins: &mut Builtins, log: Arc<EventsLog>) -> Hooks {
    builtins.register::<Json, _>("conformance-events", Factory(log));
    Hooks(rutis_bridge::events::testing::Tap::new())
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
        ctx.mark_row(&row.label).map_err(|e| e.to_string())?;
        let view = ctx.plugin(Participant { config: row.config.clone(), log: self.log.clone() });
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

