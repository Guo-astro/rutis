//! The node conformance suite (feature `testing`): what a full framework
//! node must do on a link, as checks run from a rutis node against it.
//!
//! The far node, linked to `main` as `peer`, must:
//!
//! - export `calendar`: `today()` → `"monday"` (sync), `later()` →
//!   `"tuesday"` (async);
//! - import `probe` (`record(line)`, sync), which `main` exports;
//! - host the installed plugin `conformance-greeter`, which records
//!   `greeter: <text>` through `probe` when it starts and
//!   `greeter gone: <text>` when it stops (`text` from its config); with
//!   `fail` in its config, its apply fails with that message, and loading
//!   it fails with it, with or without an `inject` list (#584);
//! - take the event `tick` from `main` and, while handling it, forward
//!   `tock` with `["pong", <number of tick arguments>]` back.
//!
//! [`fixture`] is a rutis node doing so; `node/rutis-runtime/test/fixtures/
//! cordis-node.mjs` is a Cordis one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::channel::PeerId;
use crate::events::Args;
use crate::session::rpc::{settle, Reply, Value};
use crate::session::{host_key, Error, HostDispatch};
use rutis::events::{Call, Listener};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, PluginFactory};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};
use serde_json::{json, Value as Json};

use crate::{
    peer_key, Described, ExportPlugin, ImportPlugin, LinkEventsPlugin, Peer, StaticCatalog,
};

/// The node event `name`, its arguments as they come.
/// What `main` hears through `probe`.
#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<String>>>);
impl HostDispatch for Probe {
    fn invoke(&self, _method: &str, args: Value) -> Reply {
        let [line]: [String; 1] = crate::session::decode_value(args)?;
        self.0.lock().unwrap().push(line);
        Ok(Value::Undefined)
    }
    fn methods(&self) -> Option<Json> {
        Some(json!({ "record": "sync" }))
    }
}

/// Records the arguments of each event it hears.
fn tocks(tocks: Arc<Mutex<Vec<Json>>>) -> impl Listener<Args> {
    move |event: Arc<Args>, _: Call<Args>| {
        let args = Value::List(event.0.clone()).json().unwrap_or(Json::Null);
        tocks.lock().unwrap().push(args);
        std::future::ready(Ok(None))
    }
}

async fn eventually<T>(mut check: impl FnMut() -> Option<T>, what: &str) -> T {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(found) = check() {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

async fn call(peer: &Peer, method: &str, args: Json) -> Result<Json, Error> {
    let reply = peer
        .connection()
        .invoke_async("", method, args.into())
        .await?;
    settle(reply).await?.json()
}

/// Run the node checks against `peer`, a far node `main` (this
/// application, `root`) has a link to. Mounts what `main` needs for them.
pub async fn node(root: &Ctx, peer: PeerId) {
    let probe = Probe::default();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .expect("probe provided");
    root.plugin(ImportPlugin::new(peer.clone(), ["calendar"]));
    root.plugin(ExportPlugin::new(peer.clone(), ["probe"]));
    // This node's `tick` reaches the peer; the peer's `tock` reaches here.
    root.plugin(LinkEventsPlugin::new(peer.clone(), ["tick"], ["tock"]));
    let tocks = Arc::new(Mutex::new(Vec::new()));
    root.listen::<Args>()
        .name("tock")
        .on(self::tocks(tocks.clone()))
        .expect("tock listener");
    let heard = |line: String| {
        let probe = probe.clone();
        move || probe.0.lock().unwrap().contains(&line).then_some(())
    };

    // Its service, here: sync and async.
    let calendar = eventually(
        || root.get_at::<dyn HostDispatch>(&host_key("calendar")),
        "calendar imported",
    )
    .await;
    let sync = calendar.clone();
    let today = tokio::task::spawn_blocking(move || sync.invoke("today", Value::List(vec![])))
        .await
        .unwrap()
        .expect("calendar.today");
    assert_eq!(
        today.json().unwrap(),
        json!("monday"),
        "a sync call into the node's service"
    );
    let later = settle(calendar.invoke("later", Value::List(vec![])).unwrap())
        .await
        .expect("calendar.later");
    assert_eq!(
        later.json().unwrap(),
        json!("tuesday"),
        "an async call into the node's service"
    );
    drop(calendar);

    // Its host: describe, load (which uses our probe there), update, unload.
    let at = eventually(|| root.get_at::<Peer>(&peer_key(&peer)), "the peer").await;
    let mut offers = at.offers();
    eventually(
        || {
            offers
                .borrow_and_update()
                .families
                .contains("plugins")
                .then_some(())
        },
        "the host offered",
    )
    .await;
    let described = call(&at, "plugins.describe", json!(["conformance-greeter"]))
        .await
        .expect("plugins.describe");
    assert!(
        described.get("schema").is_some(),
        "describe answers with a schema field"
    );
    match call(&at, "plugins.describe", json!(["not-installed-anywhere"])).await {
        Err(Error::Remote { name, .. }) => {
            assert_eq!(name, "NotFound", "a missing plugin is NotFound")
        }
        other => panic!("describing a missing plugin must fail with NotFound, got {other:?}"),
    }
    assert!(
        call(&at, "plugins.load", json!(["f", "/etc/plugin", {}]))
            .await
            .is_err(),
        "a host refuses files"
    );
    call(
        &at,
        "plugins.load",
        json!(["g", "conformance-greeter", { "text": "one" }]),
    )
    .await
    .expect("plugins.load");
    eventually(heard("greeter: one".into()), "the hosted plugin started").await;
    call(&at, "plugins.update", json!(["g", { "text": "two" }]))
        .await
        .expect("plugins.update");
    eventually(heard("greeter: two".into()), "the hosted plugin restarted").await;
    call(&at, "plugins.unload", json!(["g"]))
        .await
        .expect("plugins.unload");
    eventually(
        heard("greeter gone: two".into()),
        "the hosted plugin stopped",
    )
    .await;
    // A plugin whose apply fails fails its load with that error, also
    // behind the gate of an `inject` list (#584).
    for (key, inject) in [("x", json!([])), ("y", json!(["probe"]))] {
        let fail = format!("greeter {key} refused");
        match call(
            &at,
            "plugins.load",
            json!([key, "conformance-greeter", { "fail": fail }, [], inject]),
        )
        .await
        {
            Err(error) => assert!(
                error.to_string().contains(&fail),
                "loading {key} fails with its apply's error, got {error}"
            ),
            Ok(answer) => panic!("loading {key}, whose apply fails, must fail, got {answer}"),
        }
        let _ = call(&at, "plugins.unload", json!([key])).await;
    }

    // Events: tick there; tock back before tick's parallel ends. The
    // peer's listener reaches this bus once the link announced it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        root.named("tick")
            .parallel(Args(vec![Value::Data(json!("ping"))]))
            .await
            .expect("tick dispatched");
        if !tocks.lock().unwrap().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the peer's tick listener"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        *tocks.lock().unwrap(),
        vec![json!(["pong", 1])],
        "tock came back while tick was handled"
    );
}

/// `conformance-greeter`, for a rutis node.
struct Greeter([rutis::host::TypeKey; 1]);
impl PluginFactory<Json> for Greeter {
    // A factory's fiber is gated on what the factory declares.
    fn injects(&self) -> Vec<rutis::host::TypeKey> {
        self.0.to_vec()
    }

    fn build(&self, config: &Json) -> Result<Box<dyn DynPlugin>, RutisError> {
        Ok(Box::new(Greeting(
            config["text"].as_str().unwrap_or("").to_owned(),
            config["fail"].as_str().map(str::to_owned),
        )))
    }
}
struct Greeting(String, Option<String>);
impl DynPlugin for Greeting {
    fn name(&self) -> &str {
        "conformance-greeter"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            if let Some(fail) = &self.1 {
                return Err(rutis::Error::from(ErrorKind::PluginFailed(
                    fail.clone().into(),
                )));
            }
            let probe = ctx.require_at::<dyn HostDispatch>(&host_key("probe"))?;
            record(probe.clone(), format!("greeter: {}", self.0)).await;
            let gone = format!("greeter gone: {}", self.0);
            Ok(Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    record(probe, gone).await;
                    Ok(())
                })
            })))
        })
    }
}

/// Record `line` through the (synchronous) probe, off the async threads.
async fn record(probe: Arc<dyn HostDispatch>, line: String) {
    let _ = tokio::task::spawn_blocking(move || {
        probe.invoke("record", Value::List(vec![Value::Data(json!(line))]))
    })
    .await;
}

/// On `tick`, forward `tock` back.
/// Answers each `tick` with a `tock` carrying the count of its arguments.
fn tick_tock(root: Ctx) -> impl Listener<Args> {
    move |event: Arc<Args>, _: Call<Args>| {
        let root = root.clone();
        async move {
            let count = event.0.len();
            root.named("tock")
                .parallel(Args(vec![
                    Value::Data(json!("pong")),
                    Value::Data(json!(count)),
                ]))
                .await?;
            Ok(None)
        }
    }
}

struct Calendar;
impl HostDispatch for Calendar {
    fn invoke(&self, method: &str, _args: Value) -> Reply {
        match method {
            "today" => Ok(json!("monday").into()),
            "later" => Ok(Value::future(async { Ok(json!("tuesday").into()) })),
            _ => Err(Error::Value(method.into())),
        }
    }
    fn methods(&self) -> Option<Json> {
        Some(json!({ "today": "sync", "later": "async" }))
    }
}

/// Make `root`, linked to `main`, the conformance node: it provides and
/// exports `calendar`, imports `probe`, hosts `conformance-greeter` and
/// answers `tick` with `tock`.
pub fn fixture(root: &Ctx, main: PeerId) {
    root.provide_at(host_key("calendar"), Arc::new(Calendar))
        .expect("calendar provided");
    root.plugin(ExportPlugin::new(main.clone(), ["calendar"]));
    root.plugin(ImportPlugin::new(main.clone(), ["probe"]));
    let catalog = StaticCatalog::new().with(
        "conformance-greeter",
        Described {
            schema: Some(
                json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
            ),
            version: Some("1.0.0".into()),
            integrity: None,
        },
        Greeter([host_key("probe").erase()]),
    );
    root.plugin(crate::HostPlugin::new(main.clone(), Arc::new(catalog)));
    root.plugin(LinkEventsPlugin::new(main, ["tock"], ["tick"]));
    root.listen::<Args>()
        .name("tick")
        .on(tick_tock(root.clone()))
        .expect("tick listener");
}
