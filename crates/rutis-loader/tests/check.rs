//! `Loader::check`: rows that resolve and pass the dry run, and whether
//! what each waits for in rutis is provided, before anything runs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin};
use rutis_loader::{
    Builtins, Declared, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, Provider, RowStatus,
    ServiceCatalog, Supply, Unknown,
};
use serde::Deserialize;
use serde_json::{json, Value};

/// Counts its applies: a check applies nothing.
struct Counted(Arc<AtomicUsize>);

impl Plugin for Counted {
    fn name(&self) -> &str {
        "counted"
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Sized {
    size: u32,
}

/// Plugins that declare from their config what they require and provide
/// (`{ "requires": [...], "provides": [...] }`), `sized` (whose config needs
/// a number), `vague` (which declares nothing) and `remote` (whose
/// declarations are not known); a catalog where the application registered
/// `clock` for a service of its own.
async fn loader(applied: &Arc<AtomicUsize>) -> (Ctx, Loader) {
    let mut builtins = Builtins::new();
    let counted = |applied: &Arc<AtomicUsize>| {
        let applied = applied.clone();
        move |_: &Value| -> Result<Box<dyn Plugin>, CordisError> {
            Ok(Box::new(Counted(applied.clone())))
        }
    };
    builtins.register_fn::<Value, _>("plain", counted(applied));
    builtins.register_fn::<Value, _>("vague", counted(applied));
    builtins.register_fn::<Value, _>("remote", counted(applied));
    let sized = applied.clone();
    builtins.register_fn::<Sized, _>("sized", move |_| Ok(Box::new(Counted(sized.clone()))));
    let names = |config: &Value, field: &str| -> Vec<String> {
        config[field]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect()
    };
    builtins.declare(
        "plain",
        Arc::new(move |config: &Value| {
            Ok(Declared {
                requires: names(config, "requires"),
                provides: names(config, "provides"),
            })
        }),
    );
    builtins.declare("sized", Arc::new(|_: &Value| Ok(Declared::default())));
    builtins.declare(
        "remote",
        Arc::new(|_: &Value| Err("it runs on gpu, which is not connected".to_owned())),
    );
    let mut catalog = ServiceCatalog::new();
    catalog.register::<String>("clock");
    let plugin = LoaderPlugin::new(
        builtins,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    let root = Ctx::root().unwrap();
    root.plugin(plugin).await.unwrap();
    (root, loader)
}

fn layers(rows: Value) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

/// Each row's requirements, as (service, supply).
fn supplies(report: &rutis_loader::CheckReport, id: &str) -> Vec<(String, Supply)> {
    let row = report.rows.iter().find(|row| row.id == id).expect(id);
    match &row.status {
        RowStatus::Checked { requires, .. } => requires
            .iter()
            .map(|requirement| (requirement.service.clone(), requirement.supply.clone()))
            .collect(),
        other => panic!("{id}: {other:?}"),
    }
}

/// What a row waits for is provided by another row, by the application,
/// or by nothing; a disabled provider is named; the row's own `inject`
/// counts (`clock`), and a row does not provide for itself.
/// risk: A6 (#258)
#[tokio::test(flavor = "multi_thread")]
async fn each_service_a_row_waits_for_has_a_provider() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "llm", "name": "plain", "config": { "provides": ["llm"] } },
            { "id": "chat", "name": "plain", "config": { "requires": ["llm", "search", "store"] }, "inject": ["clock"] },
            { "id": "search", "name": "plain", "config": { "provides": ["search"] }, "disabled": true },
            { "id": "loop", "name": "plain", "config": { "requires": ["echo"], "provides": ["echo"] } },
        ])))
        .await;
    assert_eq!(
        supplies(&report, "chat"),
        [
            ("clock".into(), Supply::Provided(vec![Provider::Host])),
            (
                "llm".into(),
                Supply::Provided(vec![Provider::Row("llm".into())])
            ),
            (
                "search".into(),
                Supply::Missing {
                    disabled: vec!["search".into()]
                }
            ),
            ("store".into(), Supply::Missing { disabled: vec![] }),
        ]
    );
    assert_eq!(
        supplies(&report, "loop"),
        [("echo".into(), Supply::Missing { disabled: vec![] })]
    );
    assert!(matches!(
        report
            .rows
            .iter()
            .find(|row| row.id == "search")
            .unwrap()
            .status,
        RowStatus::Disabled
    ));
    let failing: Vec<&str> = report.failing().map(|row| row.id.as_str()).collect();
    assert_eq!(failing, ["chat", "loop"]);
    // Nothing ran, and the check changed nothing.
    assert_eq!(applied.load(Ordering::SeqCst), 0);
    assert!(loader.entries().is_empty());
}

/// A row whose declarations are not known may provide anything: a
/// service nothing known provides is then not known to be missing.
/// risk: A6 (#258)
#[tokio::test(flavor = "multi_thread")]
async fn a_row_that_may_provide_it_makes_it_unknown_not_missing() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "chat", "name": "plain", "config": { "requires": ["llm"] } },
            { "id": "gpu", "name": "remote" },
            { "id": "native", "name": "vague" },
        ])))
        .await;
    assert_eq!(
        supplies(&report, "chat"),
        [(
            "llm".into(),
            Supply::Unknown(vec![
                Unknown {
                    row: "gpu".into(),
                    why: "it runs on gpu, which is not connected".into()
                },
                Unknown {
                    row: "native".into(),
                    why: "its plugin does not say".into()
                },
            ])
        )]
    );
    assert_eq!(report.failing().count(), 0);
}

/// A row that cannot start fails, and provides nothing to the others: its
/// module does not resolve, or its config fails the dry run.
/// risk: A6
#[tokio::test(flavor = "multi_thread")]
async fn rows_that_cannot_start_fail_and_provide_nothing() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "missing", "name": "not-installed" },
            { "id": "sized", "name": "sized", "config": { "size": "large" } },
            { "id": "chat", "name": "plain", "config": { "requires": ["sized"] } },
            { "id": "ok", "name": "sized", "config": { "size": 3 } },
        ])))
        .await;
    let status = |id: &str| {
        &report
            .rows
            .iter()
            .find(|row| row.id == id)
            .expect(id)
            .status
    };
    let RowStatus::Failed(error) = status("missing") else {
        panic!("{:?}", status("missing"))
    };
    assert_eq!(error.to_string(), "no plugin named \"not-installed\"");
    let RowStatus::Failed(error) = status("sized") else {
        panic!("{:?}", status("sized"))
    };
    assert!(error.to_string().contains("size"), "{error}");
    assert!(matches!(status("ok"), RowStatus::Checked { .. }));
    let failing: Vec<&str> = report.failing().map(|row| row.id.as_str()).collect();
    assert_eq!(failing, ["missing", "sized", "chat"]);
    assert_eq!(applied.load(Ordering::SeqCst), 0);
}
