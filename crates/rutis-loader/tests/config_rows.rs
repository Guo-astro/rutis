//! Rust rows (the rows the loader fills itself): their config is filled
//! with the schema's defaults and validated before `build`, through the
//! loader's one config step, at every place it is checked (config schema
//! design §2.1, §5.3).

use std::sync::{Arc, Mutex};

use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_loader::edit::Editable;
use rutis_loader::report::{EntryStatus, WaitReason};
use rutis_loader::{Builtins, Layer, LoaderOptions, LoaderPlugin, Patch};
use rutis_test_support::until;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// What each built copy got.
type Seen = Arc<Mutex<Vec<Weather>>>;

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq)]
struct Weather {
    city: String,
    #[serde(default = "metric")]
    units: String,
    #[serde(default)]
    days: Option<u32>,
}

fn metric() -> String {
    "metric".into()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Strict {
    name: String,
}

struct Noop;

impl rutis::Plugin for Noop {
    type Deps = ();

    fn name(&self) -> &str {
        "noop"
    }

    async fn apply(&self, _ctx: &Ctx, (): ()) -> rutis::Result {
        Ok(())
    }
}

/// Provides `forecast`.
struct Forecast;

impl rutis::Plugin for Forecast {
    type Deps = ();

    fn name(&self) -> &str {
        "forecast"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.provide(7u64)?;
        Ok(())
    }
}

struct Harness {
    loader: rutis_loader::Loader,
    seen: Seen,
    warnings: Arc<Mutex<Vec<String>>>,
    _root: Ctx,
}

async fn harness() -> Harness {
    let seen: Seen = Arc::default();
    let mut builtins = Builtins::new();
    let log = seen.clone();
    builtins.register("weather", move |config: &Weather| {
        log.lock().unwrap().push(config.clone());
        Ok(Noop)
    });
    builtins.register("strict", |_: &Strict| Ok(Noop));
    builtins.register("forecast", |_: &Weather| Ok(Forecast));
    builtins.declare(
        "forecast",
        Arc::new(|_: &Value| {
            Ok(rutis_loader::resolve::Declared {
                provides: vec!["forecast".into()],
                ..Default::default()
            })
        }),
    );
    let mut catalog = rutis_loader::ServiceCatalog::new();
    catalog.register::<u64>("forecast");
    let plugin = LoaderPlugin::new(
        builtins,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    let warnings: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = warnings.clone();
    loader.on_config_warning(Arc::new(move |row: &str, issue: &rutis::ConfigIssue| {
        sink.lock()
            .unwrap()
            .push(format!("{row}: config: {}", issue.message))
    }));
    let root = Ctx::root().unwrap();
    root.plugin(plugin).await.unwrap();
    Harness {
        loader,
        seen,
        warnings,
        _root: root,
    }
}

fn layers(rows: Value) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches), Layer::new("user", Vec::new())]
}

fn error_of(loader: &rutis_loader::Loader, id: &str) -> Option<String> {
    match loader.get(id)?.status {
        EntryStatus::Unresolved(error) => Some(error.to_string()),
        EntryStatus::Running(snapshot) => snapshot.error.map(|e| e.to_string()),
        _ => None,
    }
}

fn running(loader: &rutis_loader::Loader, id: &str) -> bool {
    matches!(loader.get(id).map(|e| e.status), Some(EntryStatus::Running(s)) if s.state == FiberState::Active)
}

/// A wrong config fails its row with `config: …` (the schema catches the
/// type error before serde does); a right one gets the schema's defaults.
/// guarantee: sdk/config-schema
#[tokio::test]
async fn a_rust_row_is_filled_and_validated() {
    let h = harness().await;
    let report = h
        .loader
        .reconcile(
            layers(json!([
                { "id": "bad", "name": "weather", "config": { "city": 42, "days": 1.5 } },
                { "id": "good", "name": "weather", "config": { "city": "Oslo", "days": 3.0 } },
                { "id": "strict", "name": "strict", "config": { "name": "a", "nmae": "b" } }
            ])),
            None,
        )
        .await
        .unwrap();
    let failures: Vec<String> = report.failures.iter().map(ToString::to_string).collect();
    assert_eq!(
        failures,
        [
            "bad: config: city should be a string, got 42\nconfig: days should be an integer or null, got 1.5",
            "strict: config: nmae is not a config key (did you mean name?)",
        ]
    );
    assert_eq!(
        error_of(&h.loader, "bad").as_deref(),
        Some("config: city should be a string, got 42\nconfig: days should be an integer or null, got 1.5")
    );
    assert!(running(&h.loader, "good"));
    // `1.0`-style integers decode, defaults are filled before serde.
    assert_eq!(
        *h.seen.lock().unwrap(),
        [Weather {
            city: "Oslo".into(),
            units: "metric".into(),
            days: Some(3)
        }]
    );
    assert!(h.warnings.lock().unwrap().is_empty());
}

/// An extra key is warned about once per row and value; the row runs.
#[tokio::test]
async fn extra_keys_are_warned_about_once() {
    let h = harness().await;
    let rows = json!([{ "id": "w", "name": "weather", "config": { "city": "Oslo", "cty": "x" } }]);
    h.loader
        .reconcile(layers(rows.clone()), None)
        .await
        .unwrap();
    assert!(running(&h.loader, "w"));
    h.loader.reconcile(layers(rows), None).await.unwrap();
    h.loader.reload("w").await.unwrap();
    assert_eq!(
        *h.warnings.lock().unwrap(),
        ["w: config: cty is not a config key (did you mean city?)"]
    );
}

/// An edit with a wrong config is refused by `commit`'s dry run, and the
/// row runs on; a row depending on a failed one says why it waits.
#[tokio::test]
async fn edits_and_waits_see_config_errors() {
    let h = harness().await;
    h.loader
        .reconcile(
            layers(json!([
                { "id": "w", "name": "weather", "config": { "city": "Oslo" } },
                { "id": "f", "name": "forecast", "config": { "city": 1 } },
                { "id": "user", "name": "weather", "config": { "city": "Bergen" }, "inject": ["forecast"] }
            ])),
            Some(Editable::new("user", Default::default())),
        )
        .await
        .unwrap();
    let error = h
        .loader
        .update("w", json!({ "city": 3 }))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "config: city should be a string, got 3");
    assert!(running(&h.loader, "w"));

    until(
        || {
            let entry = h.loader.get("user").unwrap();
            let waits = h.loader.waits_of(&[&entry]);
            waits.first().is_some_and(|waits| {
                waits.iter().any(|wait| {
                    matches!(&wait.reason, WaitReason::ProviderFailed { row, error }
                    if row == "f" && error == "config: city should be a string, got 1")
                })
            })
        },
        "user waits for forecast",
    )
    .await;
}
