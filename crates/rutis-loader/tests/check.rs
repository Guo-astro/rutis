//! `Loader::check`: rows that resolve and pass the dry run, and whether
//! what each waits for in rutis is provided, before anything runs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, PluginFactory};
use rutis::{Ctx, Error};
use rutis_loader::report::{CheckReport, Provider, RowStatus, Supply, Unknown};
use rutis_loader::resolve::{Declared, Resolved};
use rutis_loader::{
    Builtins, Chain, Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch, Resolver,
    ServiceCatalog,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// Counts its applies: a check applies nothing.
struct Counted(Arc<AtomicUsize>);

impl rutis::Plugin for Counted {
    type Deps = ();

    fn name(&self) -> &str {
        "counted"
    }

    async fn apply(&self, _ctx: &Ctx, (): ()) -> rutis::Result {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct Sized {
    size: u32,
}

struct CountedFactory(Arc<AtomicUsize>);

impl PluginFactory<Value> for CountedFactory {
    fn build(&self, _config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Counted(self.0.clone())))
    }
}

/// `foreign`: a plugin that handles its row's `isolate` and `inject`
/// itself (as a runtime's), declaring nothing it waits for.
struct Foreign(Arc<Resolved>);

impl Resolver for Foreign {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        let found = (name == "foreign").then(|| self.0.clone());
        Box::pin(async move {
            found.ok_or_else(|| LoaderError::NotFound {
                name: name.to_owned(),
            })
        })
    }
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
        move |_: &Value| Ok(Counted(applied.clone()))
    };
    builtins.register("plain", counted(applied));
    builtins.register("vague", counted(applied));
    builtins.register("remote", counted(applied));
    let sized = applied.clone();
    builtins.register("sized", move |_: &Sized| Ok(Counted(sized.clone())));
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
                announces: config["announces"] == true,
            })
        }),
    );
    builtins.declare("sized", Arc::new(|_: &Value| Ok(Declared::default())));
    builtins.declare(
        "remote",
        Arc::new(|_: &Value| Err("it runs on gpu, which is not connected".to_owned())),
    );
    // Every name is shared by name but `clock`, registered as the
    // application's own.
    let mut catalog = ServiceCatalog::new();
    catalog.register::<String>("clock");
    let foreign = Foreign(Arc::new(
        Resolved::new(Arc::new(CountedFactory(applied.clone())))
            .with_foreign_scope()
            .with_declared(Declared::default()),
    ));
    let plugin = LoaderPlugin::new(
        Chain::new().with(builtins).with(foreign),
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
fn supplies(report: &CheckReport, id: &str) -> Vec<(String, Supply)> {
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
/// guarantee: loader/check
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
/// guarantee: loader/check
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

/// A service no row declares, but rows that provide services they do not
/// declare may (a published Cordis plugin's, announced when it runs), may
/// be provided at runtime: not missing, and named with those rows. A
/// declared provider still wins, a disabled one does not count, and a row
/// does not provide for itself.
/// risk: A6 (#428)
/// guarantee: loader/check
#[tokio::test(flavor = "multi_thread")]
async fn rows_that_announce_what_they_provide_may_provide_it() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "storage", "name": "plain", "config": { "announces": true } },
            { "id": "domain", "name": "plain", "config": { "requires": ["storage"], "announces": true } },
            { "id": "workspace", "name": "plain", "config": { "requires": ["storageDomain", "llm"] } },
            { "id": "llm", "name": "plain", "config": { "provides": ["llm"] } },
            { "id": "off", "name": "plain", "config": { "announces": true }, "disabled": true },
        ])))
        .await;
    assert_eq!(
        supplies(&report, "domain"),
        [("storage".into(), Supply::Announced(vec!["storage".into()]))]
    );
    assert_eq!(
        supplies(&report, "workspace"),
        [
            (
                "llm".into(),
                Supply::Provided(vec![Provider::Row("llm".into())])
            ),
            (
                "storageDomain".into(),
                Supply::Announced(vec!["storage".into(), "domain".into()])
            ),
        ]
    );
    assert_eq!(report.failing().count(), 0);
}

/// A row that cannot start fails, and provides nothing to the others: its
/// module does not resolve, or its config fails the dry run.
/// risk: A6
/// guarantee: loader/check
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

/// A row whose plugin handles its `inject` itself waits in rutis only for
/// the names rutis shares; its runtime resolves the others natively, so
/// they are not checked here.
/// risk: A6 (#258)
/// guarantee: loader/check
#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_rows_own_inject_counts_only_names_rutis_shares() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "cordis", "name": "foreign", "inject": ["shared", "clock"] },
        ])))
        .await;
    // `clock` is the application's own service, not shared by name, so not
    // checked. With runtimes or peers, `shared` is shared by name and
    // nothing provides it; without them, rutis shares nothing.
    #[cfg(any(feature = "runtimes", feature = "peer"))]
    assert_eq!(
        supplies(&report, "cordis"),
        [(
            "shared".to_owned(),
            Supply::Missing {
                disabled: Vec::new()
            }
        )]
    );
    #[cfg(not(any(feature = "runtimes", feature = "peer")))]
    assert_eq!(supplies(&report, "cordis"), []);
}

/// A `#[rutis::service]` type of the application nothing has provided is
/// not reported as provided by the host: it serves its name once provided.
#[cfg(any(feature = "runtimes", feature = "peer"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_marked_type_serves_its_name_once_provided() {
    pub struct Weather;
    #[rutis::service]
    impl Weather {
        pub fn today(&self) -> u32 {
            1
        }
    }
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "user", "name": "plain", "config": { "requires": ["weather"] } },
        ])))
        .await;
    let supplies = supplies(&report, "user");
    let [(service, Supply::Provided(by))] = supplies.as_slice() else {
        panic!("{supplies:?}");
    };
    assert_eq!(service, "weather");
    assert!(
        matches!(by.as_slice(), [Provider::Marked(name)] if name.ends_with("Weather")),
        "{by:?}"
    );
}

/// Rows that inject each other's services make a cycle: the check names
/// it, link by link, once; a row waiting on the cycle is not part of it.
/// guarantee: loader/check
#[tokio::test(flavor = "multi_thread")]
async fn rows_that_inject_each_other_are_a_cycle() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "a", "name": "plain", "config": { "requires": ["bsvc"], "provides": ["asvc"] } },
            { "id": "b", "name": "plain", "config": { "requires": ["csvc"], "provides": ["bsvc"] } },
            { "id": "c", "name": "plain", "config": { "requires": ["asvc"], "provides": ["csvc"] } },
            { "id": "user", "name": "plain", "config": { "requires": ["asvc"] } },
            { "id": "clock-user", "name": "plain", "config": { "requires": ["clock"] } },
            // `p` may start from `q2`, which needs nothing: no cycle.
            { "id": "p", "name": "plain", "config": { "requires": ["qsvc"], "provides": ["psvc"] } },
            { "id": "q", "name": "plain", "config": { "requires": ["psvc"], "provides": ["qsvc"] } },
            { "id": "q2", "name": "plain", "config": { "provides": ["qsvc"] } },
        ])))
        .await;
    let cycles: Vec<Vec<(String, String, String)>> = report
        .cycles
        .iter()
        .map(|cycle| {
            cycle
                .iter()
                .map(|link| {
                    (
                        link.row.clone(),
                        link.service.clone(),
                        link.provider.clone(),
                    )
                })
                .collect()
        })
        .collect();
    let link = |row: &str, service: &str, provider: &str| {
        (row.to_owned(), service.to_owned(), provider.to_owned())
    };
    assert_eq!(
        cycles,
        [vec![
            link("a", "bsvc", "b"),
            link("b", "csvc", "c"),
            link("c", "asvc", "a")
        ]]
    );
    // A warning: nothing more fails than before.
    assert_eq!(report.failing().count(), 0);
}

/// Two rows declaring one service in one scope clash; in scopes of their
/// own (`isolate`) they do not.
/// guarantee: loader/check
#[tokio::test(flavor = "multi_thread")]
async fn rows_declaring_one_service_in_one_scope_clash() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "one", "name": "plain", "config": { "provides": ["llm"] } },
            { "id": "two", "name": "plain", "config": { "provides": ["llm", "clock"] } },
            { "id": "own", "name": "plain", "isolate": { "clock": true }, "config": { "provides": ["clock"] } },
            { "id": "off", "name": "plain", "disabled": true, "config": { "provides": ["llm"] } },
            // Waits for what nothing provides: it never starts.
            { "id": "stuck", "name": "plain", "config": { "provides": ["llm"], "requires": ["nowhere"] } },
        ])))
        .await;
    let clashes: Vec<(String, Vec<String>)> = report
        .clashes
        .iter()
        .map(|clash| (clash.service.clone(), clash.rows.clone()))
        .collect();
    assert_eq!(
        clashes,
        [("llm".to_owned(), vec!["one".to_owned(), "two".to_owned()])]
    );
    // Only `stuck` fails: nothing provides what it waits for.
    let failing: Vec<&str> = report.failing().map(|row| row.id.as_str()).collect();
    assert_eq!(failing, ["stuck"]);
}

/// The only row declaring a service is disabled while rows that declare
/// nothing may provide it: that row is named (#666), not those; a
/// misspelt name gets the one it meant.
/// guarantee: loader/check
#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_provider_is_named_and_a_misspelt_name_corrected() {
    let applied = Arc::new(AtomicUsize::new(0));
    let (_root, loader) = loader(&applied).await;
    let report = loader
        .check(&layers(json!([
            { "id": "llm", "name": "plain", "disabled": true, "config": { "provides": ["llm"] } },
            { "id": "probe", "name": "plain", "config": { "announces": true } },
            { "id": "weather", "name": "plain", "config": { "requires": ["llm", "lmm"] } },
        ])))
        .await;
    assert_eq!(
        supplies(&report, "weather"),
        [
            (
                "llm".into(),
                Supply::OnlyDisabled {
                    disabled: vec!["llm".into()],
                    announced: vec!["probe".into()],
                }
            ),
            ("lmm".into(), Supply::Announced(vec!["probe".into()])),
        ]
    );
    let row = report.rows.iter().find(|row| row.id == "weather").unwrap();
    let RowStatus::Checked { requires, .. } = &row.status else {
        panic!("{:?}", row.status)
    };
    let likes: Vec<Option<&str>> = requires
        .iter()
        .map(|requirement| requirement.like.as_deref())
        .collect();
    assert_eq!(likes, [None, Some("llm")]);
    // As before: a warning, not a failure.
    assert_eq!(report.failing().count(), 0);
}
