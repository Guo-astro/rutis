//! Two rutis hosts as nodes (`docs/design-remote-session-2026-10-28.md`
//! §2.6): `main` dials `office` with a `rutis-host/dial` row, `office`
//! takes it with a `rutis-host/listen` row, over loopback `ws://`. The
//! services of both are seen on both sides, a name provided on both is
//! refused by kernel rules, events are one bus, and the row
//! `office:scanner` runs `scanner` on office with main's config. Then
//! office goes, and what came from it goes with it.
//!
//! guarantee: loader/peer-rows, events/node-links

// The test holds the environment until it ends, across its awaits: the
// variables it sets are for the hosts it starts.
#![allow(clippy::await_holding_lock)]

use std::sync::Arc;

use rutis::events::Call;
use rutis::host::{CtxHost, FiberState};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Plugin};
use rutis_bridge::events::{ArgValue, Args};
use rutis_bridge::session::{call_service, host_key, HostDispatch, Reply, Value};
use rutis_host::config::HostConfig;
use rutis_host::host::{Embedding, Host};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value as Json};

mod common;
use common::environment;

/// Twice HANG_GUARD: two hosts start, and connect.
const COLD: Guard = Guard(std::time::Duration::from_secs(HANG_GUARD.as_secs() * 2));

/// Answers `get` with its text.
struct Answer(String);

impl HostDispatch for Answer {
    fn invoke(&self, _: &str, _: Value) -> Reply {
        Ok(json!(self.0).into())
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "get": "async" }))
    }
}

/// A row providing `config.service`, whose `get` answers `config.answer`.
struct Provides(Json);

impl Plugin for Provides {
    type Deps = ();

    fn name(&self) -> &str {
        "test/provides"
    }

    async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
        let service = self.0["service"].as_str().unwrap_or_default();
        let answer = self.0["answer"].as_str().unwrap_or_default().to_owned();
        ctx.provide_at(host_key(service), Arc::new(Answer(answer)))?;
        Ok(())
    }
}

/// Rows compiled in: `test/provides`, and `scanner` (the same, for a far
/// end to run by name).
fn embedding() -> Embedding {
    Embedding {
        builtins: Some(Box::new(|builtins| {
            builtins.register::<Json, _, _>("test/provides", |config| Ok(Provides(config.clone())));
            builtins.register::<Json, _, _>("scanner", |config| Ok(Provides(config.clone())));
        })),
        rows_file: None,
        observe: None,
    }
}

async fn start(config: Json) -> Host {
    let config: HostConfig = serde_json::from_value(config).unwrap();
    let host = Host::start_with(&config, embedding()).await.unwrap();
    host.load(config.rows()).await.unwrap();
    host
}

/// What `service`'s `get` answers on `host`, once it is there.
async fn get(host: &Host, service: &str) -> String {
    COLD.eventually(|| host.service(service), &format!("{service} there"))
        .await;
    call_service(&host.root(), service, "get", ())
        .await
        .unwrap()
}

/// The state of the plugin `name` on `host`, and its error.
fn plugin(host: &Host, name: &str) -> Option<(FiberState, Option<String>)> {
    host.root()
        .diagnostics()
        .plugins
        .into_iter()
        .find(|plugin| plugin.name == name)
        .map(|plugin| (plugin.state, plugin.error.map(|error| error.to_string())))
}

/// The first answer of `serial` of `name` on `host`, once a listener
/// answers.
async fn heard(host: &Host, name: &str) -> Json {
    let root = host.root();
    let deadline = tokio::time::Instant::now() + rutis_test_support::scaled(HANG_GUARD);
    loop {
        let answered = root
            .named(name)
            .serial(Args(vec![]))
            .await
            .unwrap()
            .map(|ArgValue(value)| value.json().unwrap());
        if let Some(answer) = answered {
            return answer;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "nobody answered {name}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_hosts_as_nodes_share_services_events_and_run_rows() {
    let _environment = environment();
    // The token main's dial row `office` presents, and office's listen row
    // `main` takes.
    std::env::set_var("RUTIS_TOKEN_OFFICE", "node-token");
    std::env::set_var("RUTIS_TOKEN_MAIN", "node-token");

    let office = start(json!({
        "id": "office",
        "listen": [{ "name": "local", "address": "127.0.0.1:0" }],
        "rows": [
            { "id": "main", "name": "rutis-host/listen", "config": { "listener": "local" } },
            { "id": "clock", "name": "test/provides", "config": { "service": "clock", "answer": "office's clock" } },
            { "id": "weather", "name": "test/provides", "config": { "service": "weather", "answer": "office's weather" } }
        ]
    }))
    .await;
    let address = office.listening[0].1;
    office
        .root()
        .listen::<Args>()
        .name("t/ping")
        .on(|_: Arc<Args>, _: Call<Args>| async {
            Ok(Some(ArgValue(json!("heard at office").into())))
        })
        .unwrap();

    let main = start(json!({
        "id": "main",
        "rows": [
            { "id": "office", "name": "rutis-host/dial", "config": { "address": format!("ws://{address}/rutis") } },
            { "id": "notes", "name": "test/provides", "config": { "service": "notes", "answer": "main's notes" } },
            { "id": "weather", "name": "test/provides", "config": { "service": "weather", "answer": "main's weather" } },
            { "id": "scanner", "name": "office:scanner", "config": { "service": "scanner", "answer": "300 dpi" } }
        ]
    }))
    .await;
    main.root()
        .listen::<Args>()
        .name("t/pong")
        .on(|_: Arc<Args>, _: Call<Args>| async {
            Ok(Some(ArgValue(json!("heard at main").into())))
        })
        .unwrap();

    // Services both ways, without export or import.
    assert_eq!(get(&main, "clock").await, "office's clock");
    assert_eq!(get(&office, "notes").await, "main's notes");

    // A name both provide: each keeps its own; the far end's remote plugin
    // fails, saying why.
    let refused = COLD
        .eventually(
            || {
                plugin(&main, "office:weather")
                    .and_then(|(state, error)| (state == FiberState::Failed).then_some(error))
            },
            "office:weather failing at main",
        )
        .await
        .unwrap();
    assert!(
        refused.contains(
            "office:weather: weather is provided here by row weather: not registered from office"
        ),
        "{refused}"
    );
    assert_eq!(get(&main, "weather").await, "main's weather");
    assert_eq!(get(&office, "weather").await, "office's weather");

    // One bus: each host's listeners hear what the other emits.
    assert_eq!(heard(&main, "t/ping").await, json!("heard at office"));
    assert_eq!(heard(&office, "t/pong").await, json!("heard at main"));

    // The row `office:scanner` runs `scanner` on office, with main's
    // config: its service is seen at main.
    assert_eq!(get(&main, "scanner").await, "300 dpi");

    // Office goes: what came from it goes at main.
    office.shut_down(HANG_GUARD).await.ok();
    COLD.eventually(
        || main.service("clock").is_none().then_some(()),
        "office's clock gone at main",
    )
    .await;
    main.shut_down(HANG_GUARD).await.ok();
}
