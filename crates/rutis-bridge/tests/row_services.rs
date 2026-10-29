//! Rows exporting services to rutis and using host services leased one by
//! one: what rutis-loader builds JavaScript rows on.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::task::Poll;

use rutis::events::{Call, Listener};
use rutis::host::diagnostics::{ServiceChange, ServiceChanged};
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::{
    row_projection, row_projection_of, Mount, Process, Projection, RowService,
};
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{settle, Reply, Value as RpcValue};
use rutis_test_support::{recv_within, until, within};
use serde_json::{json, Value};

const WEATHER: &str = r#"
export const name = 'weather'
export const inject = ['clock']
export const Config = {
  type: 'object', meta: {},
  dict: { city: { type: 'string', meta: { default: 'Paris' } } },
  '~standard': { validate: value => ({ value }) },
}
export function apply(ctx, config) {
  ctx.provide('weather', {
    today() { return `${config.city} at ${ctx.clock.now()}` },
    async later() { return `${config.city} later` },
  })
}
"#;

// Reports whether `clock` is registered in the Context, read from outside
// any plugin's inject.
const INSPECTOR: &str = r#"
export function apply(ctx) {
  ctx.provide('inspector', { has(name) { return ctx.get(name, false) !== undefined } })
}
"#;

struct Clock(Arc<AtomicUsize>);

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "now");
        Ok(json!(self.0.fetch_add(1, Ordering::SeqCst)).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "now": "sync" }))
    }
}

fn package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime")
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

async fn rows_process(dir: &Path) -> Arc<Process> {
    let anchor = write(dir, "package.json", "{}");
    Process::mount(
        &package(),
        Mount {
            anchor: Some(&anchor),
            ..Mount::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn describe_reports_config_inject_and_provides() {
    let dir = tempfile::tempdir().unwrap();
    let process = rows_process(dir.path()).await;
    assert!(process.supports("rows.v2") && process.supports("hosts"));
    write(
        dir.path(),
        "weather/package.json",
        r#"{ "rutis": { "provides": { "weather": { "today": "sync", "later": "async" } } } }"#,
    );
    let entry = write(dir.path(), "weather/index.mjs", WEATHER);
    let described = process.describe_row(&entry).await.unwrap();
    assert_eq!(described.inject, ["clock"]);
    assert_eq!(
        Value::Object(described.provides),
        json!({ "weather": { "today": "sync", "later": "async" } })
    );
    assert!(!described.announces, "its package says what it provides");
    assert_eq!(
        described.config.unwrap()["properties"]["city"]["default"],
        json!("Paris")
    );
    // A plugin outside any package with `rutis.provides` declares nothing:
    // what it provides is announced when it runs.
    let inspector = write(dir.path(), "inspector.mjs", INSPECTOR);
    let described = process.describe_row(&inspector).await.unwrap();
    assert!(described.inject.is_empty() && described.provides.is_empty());
    assert!(described.announces);
    process.dispose().await.unwrap();
}

/// A published Cordis plugin: it declares nothing, and provides a service
/// with methods of its own and inherited, a field, and a second service
/// the row keeps to itself.
const STORE: &str = r#"
class Base { keys() { return ['a'] } }
class Store extends Base {
  constructor(prefix) { super(); this.prefix = prefix }
  get(key) { return `${this.prefix}${key}` }
}
export function apply(ctx) {
  ctx.provide('store', new Store('v:'))
  ctx.provide('kept', { secret() { return 1 } })
}
"#;

/// #428: the services a row provides without declaring them are announced
/// with the methods each object has (own and inherited, `async`; not its
/// fields), and published before its load returns, each in the scope the
/// projection runs in: one that scope isolates is seen there only;
/// unloading withdraws them.
#[tokio::test(flavor = "multi_thread")]
async fn services_a_row_does_not_declare_are_announced_with_their_methods() {
    let dir = tempfile::tempdir().unwrap();
    let process = rows_process(dir.path()).await;
    assert!(process.supports("rows.announce"));
    let entry = write(dir.path(), "store.mjs", STORE);
    let root = Ctx::root().unwrap();
    // The row keeps `kept` to an isolate of its own, as the loader runs it.
    let ctx = root.isolate(host_key("kept").erase(), "rutis-loader/entry/store");
    let projection = row_projection_of("row store", &serde_json::Map::new(), host_key);
    projection.attach(&ctx, process.clone()).unwrap();
    process
        .load_row_exporting(
            "store",
            &entry,
            json!({}),
            &[],
            &[],
            &serde_json::Map::new(),
            projection.clone(),
            &[],
        )
        .await
        .unwrap();
    let store = ctx
        .get_at::<dyn HostDispatch>(&host_key("store"))
        .expect("announced and published before the load returned");
    assert_eq!(
        store.methods(),
        Some(json!({ "get": "async", "keys": "async" }))
    );
    assert_eq!(store.provider().as_deref(), Some("row store"));
    let got = settle(store.invoke("get", json!(["x"]).into()).unwrap())
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(got, json!("v:x"));
    assert!(
        store.invoke("prefix", json!([]).into()).is_err(),
        "a field is not a method"
    );
    drop(store);
    assert!(ctx.get_at::<dyn HostDispatch>(&host_key("kept")).is_some());
    assert!(root.get_at::<dyn HostDispatch>(&host_key("kept")).is_none());

    projection.withdraw().await;
    process.unload_row("store").await.unwrap();
    assert!(ctx.get_at::<dyn HostDispatch>(&host_key("store")).is_none());
    process.dispose().await.unwrap();
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn row_services_reach_rutis_and_hosts_are_leased() {
    let dir = tempfile::tempdir().unwrap();
    let process = rows_process(dir.path()).await;
    let entry = write(dir.path(), "weather.mjs", WEATHER);
    let inspector = write(dir.path(), "inspector.mjs", INSPECTOR);
    // Exported only to be called here: its projection is never attached.
    let shape = json!({ "inspector": { "has": "sync" } });
    let shape = shape.as_object().unwrap();
    process
        .load_row_exporting(
            "inspector",
            &inspector,
            json!({}),
            &[],
            &[],
            shape,
            row_projection(shape),
            &[],
        )
        .await
        .unwrap();
    until(|| process.service("inspector").is_some(), "the inspector").await;
    let has_clock = || -> bool {
        let handle = process.service("inspector").unwrap();
        rutis_bridge::session::decode(process.call(&handle, "has", json!(["clock"])).unwrap())
            .unwrap()
    };

    assert!(!has_clock());
    // Two leases register `clock` once; it stays until the last is released.
    let ticks = Arc::new(AtomicUsize::new(0));
    let clock: Arc<dyn HostDispatch> = Arc::new(Clock(ticks.clone()));
    let first = process
        .lease_host("clock", clock.clone(), None)
        .await
        .unwrap();
    let second = process
        .lease_host("clock", clock.clone(), None)
        .await
        .unwrap();
    assert!(has_clock());

    let ctx = Ctx::root().unwrap();
    let provides = json!({ "weather": { "today": "sync", "later": "async" } });
    let provides = provides.as_object().unwrap();
    let projection = row_projection(provides);
    projection.attach(&ctx, process.clone()).unwrap();
    process
        .load_row_exporting(
            "weather",
            &entry,
            json!({ "city": "Oslo" }),
            &[],
            &[],
            provides,
            projection.clone(),
            &[],
        )
        .await
        .unwrap();
    let key = host_key("weather");
    until(|| ctx.get_at(&key).is_some(), "the weather service").await;
    let weather = ctx.get_at(&key).unwrap();
    assert_eq!(
        weather
            .invoke("today", json!([]).into())
            .unwrap()
            .json()
            .unwrap(),
        json!("Oslo at 0")
    );
    let later = settle(weather.invoke("later", json!([]).into()).unwrap())
        .await
        .unwrap();
    assert_eq!(later.json().unwrap(), json!("Oslo later"));
    assert_eq!(weather.methods(), Some(provides["weather"].clone()));
    assert_eq!(weather.origin(), Some(process.connection().tag()));
    assert!(weather.invoke("tomorrow", json!([]).into()).is_err());
    drop(weather);

    // A second row cannot export the same name.
    let error = process
        .load_row_exporting(
            "again",
            &entry,
            json!({}),
            &[],
            &[],
            provides,
            projection.clone(),
            &[],
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already exported"), "{error}");

    // Unloading the row withdraws its service from rutis.
    process.unload_row("weather").await.unwrap();
    until(|| ctx.get_at(&key).is_none(), "the withdrawal").await;
    projection.close();

    first.release().await.unwrap();
    assert!(has_clock(), "one lease is left");
    second.release().await.unwrap();
    assert!(!has_clock(), "the last lease withdraws it");
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

const GREETER: &str = r#"
export function apply(ctx) {
  ctx.provide('greeter', { greet() { return 'Hello' } })
}
"#;

/// A row projection for `greeter` whose first proxy is made only once
/// `release` says so, after telling `held`: its publisher stops in the
/// middle of that publication.
fn held_projection() -> (Arc<Projection>, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (tell, held) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let mut first = Some((tell, wait));
    let projection = Projection::new();
    projection.service_keyed("greeter", host_key("greeter"), move |process, handle| {
        if let Some((tell, wait)) = first.take() {
            tell.send(()).unwrap();
            recv_within("the test releases the publication", &wait);
        }
        Arc::new(RowService::new(process, handle, json!({ "greet": "sync" })))
    });
    (projection, held, release)
}

/// Counts the services published in a context.
#[derive(Clone, Default)]
struct Published(Arc<AtomicUsize>);

impl Published {
    /// The listener that counts them.
    fn listener(&self) -> impl Listener<ServiceChanged> {
        let count = self.0.clone();
        move |changed: Arc<ServiceChanged>, _: Call<ServiceChanged>| {
            if changed.change == ServiceChange::Provided {
                count.fetch_add(1, Ordering::SeqCst);
            }
            std::future::ready(Ok(None))
        }
    }
}

/// Load the greeter row exporting to `projection`, and wait until its
/// publisher is held in the middle of publishing the service.
async fn load_held(
    process: &Arc<Process>,
    entry: &Path,
    projection: &Arc<Projection>,
    held: mpsc::Receiver<()>,
) {
    let loading = {
        let process = process.clone();
        let entry = entry.to_owned();
        let projection = projection.clone();
        tokio::spawn(async move {
            let provides = json!({ "greeter": { "greet": "sync" } });
            process
                .load_row_exporting(
                    "greeter",
                    &entry,
                    json!({}),
                    &[],
                    &[],
                    provides.as_object().unwrap(),
                    projection,
                    &[],
                )
                .await
        })
    };
    tokio::task::spawn_blocking(move || recv_within("the publication", &held))
        .await
        .unwrap();
    within("the load", loading).await.unwrap().unwrap();
}

/// #372: the row is unloaded, and its projection closed, while its service
/// is still being published. Unloading reports the withdrawal to the
/// projection, which leaves it to the publication under way; closing used
/// to drop it, and the service stayed in rutis after its row was gone.
#[tokio::test(flavor = "multi_thread")]
async fn closing_keeps_a_withdrawal_reported_during_a_publication() {
    let dir = tempfile::tempdir().unwrap();
    let process = rows_process(dir.path()).await;
    let entry = write(dir.path(), "greeter.mjs", GREETER);
    let ctx = Ctx::root().unwrap();
    let published = Published::default();
    ctx.on(published.listener()).unwrap();
    let (projection, held, release) = held_projection();
    projection.attach(&ctx, process.clone()).unwrap();
    load_held(&process, &entry, &projection, held).await;

    process.unload_row("greeter").await.unwrap();
    projection.close();
    release.send(()).unwrap();
    // The publication completes, then the withdrawal follows.
    until(
        || published.0.load(Ordering::SeqCst) == 1,
        "the publication",
    )
    .await;
    until(
        || {
            ctx.get_at::<dyn HostDispatch>(&host_key("greeter"))
                .is_none()
        },
        "the withdrawal",
    )
    .await;
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// #372: `withdraw` called while a service is being published waits for
/// that publication and withdraws its service too. It used to return at
/// once and leave the service in rutis.
#[tokio::test(flavor = "multi_thread")]
async fn withdraw_waits_for_a_publication_under_way() {
    let dir = tempfile::tempdir().unwrap();
    let process = rows_process(dir.path()).await;
    let entry = write(dir.path(), "greeter.mjs", GREETER);
    let ctx = Ctx::root().unwrap();
    let (projection, held, release) = held_projection();
    projection.attach(&ctx, process.clone()).unwrap();
    load_held(&process, &entry, &projection, held).await;

    let mut withdrawal = std::pin::pin!(projection.withdraw());
    let pending =
        std::future::poll_fn(|cx| Poll::Ready(withdrawal.as_mut().poll(cx).is_pending())).await;
    assert!(pending, "withdraw waits for the publication under way");
    release.send(()).unwrap();
    within("the withdrawal", withdrawal).await;
    assert!(
        ctx.get_at::<dyn HostDispatch>(&host_key("greeter"))
            .is_none(),
        "withdrawn once withdraw returns"
    );
    process.unload_row("greeter").await.unwrap();
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}
