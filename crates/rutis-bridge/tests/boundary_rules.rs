//! The boundary rules for Cordis plugins (requirements-protocol-plugins §5)
//! that concern events and host services: rules 2, 3, 4, 5 and 7.
//!
//! Each test mounts `node/rutis-runtime/test/fixtures/boundary-rules.mjs`
//! in the Node runtime, with listeners on the rutis side, and runs the same
//! operations in native Cordis (`boundary-native.mjs`, where the rutis
//! listeners are ordinary Cordis listeners registered at the same points in
//! time). Every listener, on either side, writes to one log: on the Cordis
//! side through the `log` service, which rutis provides (rule 7). The test
//! then states what differs from native Cordis, and that the difference is
//! exactly what the rule allows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::{BoxFuture, CordisError, Ctx, Event, EventKey};
use rutis_bridge::runtime::{EmitToCordis, EventSink, Events, Host, Mount, Process};
use rutis_bridge::session::{Error, HostDispatch, Reply, Value};
use rutis_test_support::within;
use serde_json::{json, Value as Json};

fn package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime")
}

fn fixture(name: &str) -> PathBuf {
    package().join("test/fixtures").join(name)
}

/// The native Cordis reference: `{ log, result }` of one scenario.
fn native(scenario: &str) -> Json {
    let output = std::process::Command::new("node")
        .arg(fixture("boundary-native.mjs"))
        .arg(scenario)
        .current_dir(package())
        .output()
        .expect("run node");
    assert!(
        output.status.success(),
        "native {scenario} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the reference prints JSON")
}

/// One log for both sides, in the order the listeners ran.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn mark(&self, tag: &str) -> usize {
        let mut entries = self.0.lock().unwrap();
        entries.push(tag.to_owned());
        entries.len()
    }
    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// The `log` service rutis provides to the plugin.
impl HostDispatch for Log {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        match method {
            "push" => {
                let (tag,): (String,) = serde_json::from_value(args.json()?)
                    .map_err(|error| Error::Value(error.to_string()))?;
                Ok(json!(self.mark(&tag)).into())
            }
            _ => Err(Error::Value(format!("log.{method} is not implemented"))),
        }
    }
}

macro_rules! events {
    ($($name:ident = $cordis:literal;)*) => {$(
        struct $name;
        impl Event for $name {
            const NAME: &'static str = $cordis;
            type Value = ();
        }
    )*};
}
events! {
    Order = "boundary/order";
    Back = "boundary/back";
    Chain = "boundary/chain";
    Link = "boundary/link";
    Waterfall = "boundary/wf";
    WaterfallAsync = "boundary/wf-async";
}

/// A rutis listener: writes `tag` to the log; with `split`, writes
/// `<tag>:start`, yields to the runtime and writes `<tag>:end`; with
/// `bail`, short-circuits a serial dispatch.
struct Mark {
    log: Log,
    tag: &'static str,
    split: bool,
    bail: bool,
}

fn mark(log: &Log, tag: &'static str) -> Mark {
    Mark {
        log: log.clone(),
        tag,
        split: false,
        bail: false,
    }
}

impl<E: Event<Value = ()>> rutis::Listener<E> for Mark {
    fn call<'a>(&'a self, _: &'a Ctx, _: &'a E) -> BoxFuture<'a, Result<Option<()>, CordisError>> {
        Box::pin(async move {
            if self.split {
                self.log.mark(&format!("{}:start", self.tag));
                tokio::task::yield_now().await;
                self.log.mark(&format!("{}:end", self.tag));
            } else {
                self.log.mark(self.tag);
            }
            Ok(self.bail.then_some(()))
        })
    }
}

/// A mounted `boundary-rules.mjs`, its `probe` service, and the log.
struct Mounted {
    process: Arc<Process>,
    probe: String,
}

impl Mounted {
    async fn new(
        log: &Log,
        events: Option<(Vec<String>, Arc<dyn EventSink>)>,
        emits: &[&str],
    ) -> Self {
        let plugin = fixture("boundary-rules.mjs");
        let process = Process::mount(
            &package(),
            Mount {
                plugins: vec![(plugin.as_path(), json!({}))],
                services: json!({
                    "probe": ["late", "order", "serial", "bail", "waterfall", "waterfallAsync", "announce", "internal", "host"],
                }),
                hosts: vec![Host {
                    name: "log".into(),
                    methods: json!({ "push": "sync" }),
                    dispatch: Arc::new(log.clone()),
                }],
                events,
                emits: emits.iter().map(|name| (*name).to_owned()).collect(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let probe = process.service("probe").unwrap();
        Self { process, probe }
    }

    async fn call(&self, method: &str) -> Result<Json, Error> {
        within(
            method,
            self.process.call_async(&self.probe, method, json!([])),
        )
        .await
    }
}

/// `entries` restricted to one side.
fn side(entries: &[String], rutis: impl Fn(&str) -> bool) -> Vec<String> {
    entries.iter().filter(|tag| rutis(tag)).cloned().collect()
}

/// Where the entries of one side stand: one contiguous run, or a failure.
fn run_of(entries: &[String], member: impl Fn(&str) -> bool) -> std::ops::Range<usize> {
    let first = entries.iter().position(|tag| member(tag)).expect("a group");
    let last = entries.iter().rposition(|tag| member(tag)).unwrap();
    assert!(
        entries[first..=last].iter().all(|tag| member(tag)),
        "the group is not contiguous: {entries:?}"
    );
    first..last + 1
}

fn strings(value: &Json) -> Vec<String> {
    serde_json::from_value(value.clone()).unwrap()
}

/// guarantee: boundary/5.3 (Cordis -> rutis)
///
/// A Cordis `serial` reaches the rutis listeners as one group, at the place
/// where the runtime registered its forwarding listener: after the
/// listeners the plugins registered while starting, before the ones they
/// register later. In native Cordis the same listeners run in registration
/// order, the rutis ones interleaved with the Cordis ones.
#[tokio::test(flavor = "multi_thread")]
async fn cordis_events_reach_rutis_listeners_as_one_group() {
    let log = Log::default();
    let ctx = Ctx::root().unwrap();
    // R1 before the mount, R2 after it, as in the reference.
    ctx.events()
        .on(&ctx, &EventKey::<Order>::of(), mark(&log, "R1"))
        .unwrap();
    let events = Events::new();
    events.forward::<Order>("boundary/order", |_| Ok(Order));
    let mounted = Mounted::new(&log, Some((events.names(), events.clone())), &[]).await;
    events.attach(&ctx);
    ctx.events()
        .on(&ctx, &EventKey::<Order>::of(), mark(&log, "R2"))
        .unwrap();
    mounted.call("late").await.unwrap();
    mounted.call("order").await.unwrap();

    let reference = native("order");
    let native = strings(&reference["log"]);
    let bridged = log.entries();
    assert_eq!(native, ["R1", "A", "B", "R2", "C"]);
    let rutis = |tag: &str| tag.starts_with('R');
    // Each side keeps its native order (a rutis parallel has none of its
    // own between R1 and R2).
    assert_eq!(
        side(&bridged, |tag| !rutis(tag)),
        side(&native, |tag| !rutis(tag))
    );
    let mut group = side(&bridged, rutis);
    group.sort();
    assert_eq!(group, side(&native, rutis));
    // The rutis listeners run as one group, after A and B (registered while
    // the plugin started) and before C (registered later).
    assert_eq!(run_of(&bridged, rutis), 2..4, "{bridged:?}");

    events.close();
    mounted.process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// guarantee: boundary/5.3 (rutis -> Cordis)
///
/// A rutis `serial` reaches the Cordis listeners as one group, at the place
/// of the rutis listener that re-emits it into Cordis; the Cordis listeners
/// keep their order within it, including one registered after later rutis
/// listeners. In native Cordis, b runs after R2.
#[tokio::test(flavor = "multi_thread")]
async fn rutis_events_reach_cordis_listeners_as_one_group() {
    let log = Log::default();
    let ctx = Ctx::root().unwrap();
    ctx.events()
        .on(&ctx, &EventKey::<Back>::of(), mark(&log, "R1"))
        .unwrap();
    let mounted = Mounted::new(&log, None, &["boundary/back"]).await;
    ctx.events()
        .on(
            &ctx,
            &EventKey::<Back>::of(),
            EmitToCordis::new(mounted.process.clone(), "boundary/back", |_| Ok(vec![])),
        )
        .unwrap();
    ctx.events()
        .on(&ctx, &EventKey::<Back>::of(), mark(&log, "R2"))
        .unwrap();
    mounted.call("late").await.unwrap();
    let dispatched = ctx.events().serial(&ctx, &EventKey::<Back>::of(), &Back);
    assert_eq!(within("rutis serial", dispatched).await.unwrap(), None);

    let reference = native("back");
    assert_eq!(strings(&reference["log"]), ["R1", "a", "R2", "b"]);
    // Same per side: R1 before R2, a before b; the Cordis pair is one group
    // where the re-emitting listener stands, between R1 and R2.
    assert_eq!(log.entries(), ["R1", "a", "b", "R2"]);

    mounted.process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// guarantee: boundary/5.4 (Cordis -> rutis)
///
/// In a Cordis `serial` chain the rutis listeners are one link: the chain
/// waits for all of them (R:end comes before C) and goes on to the next
/// Cordis listener, which bails as natively; D never runs on either side.
#[tokio::test(flavor = "multi_thread")]
async fn a_cordis_serial_chain_takes_the_rutis_side_as_one_link() {
    let log = Log::default();
    let ctx = Ctx::root().unwrap();
    ctx.events()
        .on(
            &ctx,
            &EventKey::<Chain>::of(),
            Mark {
                split: true,
                ..mark(&log, "R")
            },
        )
        .unwrap();
    let events = Events::new();
    events.forward::<Chain>("boundary/chain", |_| Ok(Chain));
    let mounted = Mounted::new(&log, Some((events.names(), events.clone())), &[]).await;
    events.attach(&ctx);
    mounted.call("late").await.unwrap();
    let result = mounted.call("serial").await.unwrap();

    let reference = native("serial");
    assert_eq!(strings(&reference["log"]), ["R:start", "R:end", "A", "C"]);
    assert_eq!(reference["result"], json!("C"));
    // The same bail value; the rutis link moved after A, whole.
    assert_eq!(result, reference["result"]);
    assert_eq!(log.entries(), ["A", "R:start", "R:end", "C"]);

    events.close();
    mounted.process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// A rutis `serial` over R1, the listener re-emitting into Cordis, R2
/// (bails) and R3, with two async Cordis listeners a and b: the result and
/// the log.
async fn rutis_serial_over_cordis() -> (Option<()>, Vec<String>) {
    let log = Log::default();
    let ctx = Ctx::root().unwrap();
    ctx.events()
        .on(&ctx, &EventKey::<Link>::of(), mark(&log, "R1"))
        .unwrap();
    let mounted = Mounted::new(&log, None, &["boundary/link"]).await;
    ctx.events()
        .on(
            &ctx,
            &EventKey::<Link>::of(),
            EmitToCordis::new(mounted.process.clone(), "boundary/link", |_| Ok(vec![])),
        )
        .unwrap();
    ctx.events()
        .on(
            &ctx,
            &EventKey::<Link>::of(),
            Mark {
                bail: true,
                ..mark(&log, "R2")
            },
        )
        .unwrap();
    ctx.events()
        .on(&ctx, &EventKey::<Link>::of(), mark(&log, "R3"))
        .unwrap();
    let dispatched = ctx.events().serial(&ctx, &EventKey::<Link>::of(), &Link);
    let result = within("rutis serial", dispatched).await.unwrap();
    mounted.process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
    (result, log.entries())
}

/// guarantee: boundary/5.4 (rutis -> Cordis)
///
/// In a rutis `serial` chain the Cordis listeners are one link: the chain
/// waits for the whole Cordis group before R2, which bails, so R3 never
/// runs, as natively.
#[tokio::test(flavor = "multi_thread")]
async fn a_rutis_serial_chain_takes_the_cordis_side_as_one_link() {
    let (result, bridged) = rutis_serial_over_cordis().await;

    let reference = native("link");
    let native = strings(&reference["log"]);
    assert_eq!(native, ["R1", "a:start", "a:end", "b:start", "b:end", "R2"]);
    assert_eq!(reference["result"], json!("R2"));
    assert_eq!(result, Some(()));
    let cordis = |tag: &str| !tag.starts_with('R');
    assert_eq!(side(&bridged, |tag| !cordis(tag)), ["R1", "R2"]);
    assert_eq!(run_of(&bridged, cordis), 1..5, "{bridged:?}");
    let mut group = side(&bridged, cordis);
    group.sort();
    let mut expected = side(&native, cordis);
    expected.sort();
    assert_eq!(group, expected);
}

/// guarantee: boundary/5.3
///
/// Whatever the rutis dispatch, the Cordis group runs as a Cordis
/// `parallel` (rule 3): the rutis listener re-emitting into Cordis does not
/// know how it was dispatched. So inside the Cordis link of a rutis
/// `serial`, the two async Cordis listeners start in registration order and
/// overlap, as a native Cordis `parallel` runs them; natively, where they
/// are listeners of the one serial, a finishes before b starts (#281).
#[tokio::test(flavor = "multi_thread")]
async fn a_rutis_serial_runs_the_cordis_group_as_a_cordis_parallel() {
    let (_, bridged) = rutis_serial_over_cordis().await;
    let reference = native("link");
    let cordis = |tag: &str| !tag.starts_with('R');
    assert_eq!(
        side(&strings(&reference["log"]), cordis),
        ["a:start", "a:end", "b:start", "b:end"]
    );
    assert_eq!(
        side(&bridged, cordis),
        ["a:start", "b:start", "a:end", "b:end"]
    );
}

/// guarantee: boundary/5.4
///
/// A synchronous Cordis `bail` cannot wait for the rutis side (rule 4), so
/// reaching the rutis group fails the `bail` with an error saying so: A has
/// run, the rutis listener and C have not. Natively it runs R, A, C and
/// bails with "C". Before #281 the forwarding listener's Promise was taken
/// as the bail value.
#[tokio::test(flavor = "multi_thread")]
async fn a_cordis_bail_across_the_boundary_fails() {
    let log = Log::default();
    let ctx = Ctx::root().unwrap();
    ctx.events()
        .on(&ctx, &EventKey::<Chain>::of(), mark(&log, "R"))
        .unwrap();
    let events = Events::new();
    events.forward::<Chain>("boundary/chain", |_| Ok(Chain));
    let mounted = Mounted::new(&log, Some((events.names(), events.clone())), &[]).await;
    events.attach(&ctx);
    mounted.call("late").await.unwrap();
    let result = mounted.call("bail").await;

    let reference = native("bail");
    assert_eq!(strings(&reference["log"]), ["R", "A", "C"]);
    assert_eq!(reference["result"], json!({ "value": "C" }));
    match result {
        Err(Error::Remote { message, .. }) => assert_eq!(
            message,
            "event boundary/chain is forwarded to rutis, whose listeners run \
             asynchronously: a synchronous bail cannot wait for them (use serial)"
        ),
        other => panic!("the bail did not fail: {other:?}"),
    }
    assert_eq!(log.entries(), ["A"]);

    events.close();
    mounted.process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// guarantee: boundary/5.4
///
/// In a Cordis `waterfall` the rutis listeners are one asynchronous link
/// (rule 4): when they are done, the link calls `next` once, and the chain
/// goes on to C and the final step. The waterfall therefore returns a
/// Promise, as natively when a middleware is async, which resolves to the
/// native result "end". Natively, with every middleware synchronous, the
/// result is "end" itself. Before #281 the chain stopped at the rutis side.
#[tokio::test(flavor = "multi_thread")]
async fn a_cordis_waterfall_takes_the_rutis_side_as_one_async_link() {
    let log = Log::default();
    let ctx = Ctx::root().unwrap();
    ctx.events()
        .on(&ctx, &EventKey::<Waterfall>::of(), mark(&log, "R"))
        .unwrap();
    let events = Events::new();
    events.forward::<Waterfall>("boundary/wf", |_| Ok(Waterfall));
    let mounted = Mounted::new(&log, Some((events.names(), events.clone())), &[]).await;
    events.attach(&ctx);
    mounted.call("late").await.unwrap();
    let result = mounted.call("waterfall").await.unwrap();

    let reference = native("waterfall");
    assert_eq!(strings(&reference["log"]), ["R", "A", "C"]);
    assert_eq!(reference["result"], json!({ "value": "end" }));
    assert_eq!(result, json!({ "promise": true, "value": "end" }));
    // The rutis link moved after A, whole (rule 3).
    assert_eq!(log.entries(), ["A", "R", "C"]);

    events.close();
    mounted.process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// guarantee: boundary/5.4
///
/// A waterfall whose Cordis middleware is async reaches the rutis link
/// after an await; the link still calls `next` once, and the result is the
/// native one: a Promise of "end" (#281).
#[tokio::test(flavor = "multi_thread")]
async fn an_async_cordis_waterfall_across_the_boundary_behaves_natively() {
    let log = Log::default();
    let ctx = Ctx::root().unwrap();
    ctx.events()
        .on(&ctx, &EventKey::<WaterfallAsync>::of(), mark(&log, "R"))
        .unwrap();
    let events = Events::new();
    events.forward::<WaterfallAsync>("boundary/wf-async", |_| Ok(WaterfallAsync));
    let mounted = Mounted::new(&log, Some((events.names(), events.clone())), &[]).await;
    events.attach(&ctx);
    mounted.call("late").await.unwrap();
    let result = mounted.call("waterfallAsync").await.unwrap();

    let reference = native("waterfall-async");
    assert_eq!(strings(&reference["log"]), ["R", "A", "C"]);
    assert_eq!(
        reference["result"],
        json!({ "promise": true, "value": "end" })
    );
    assert_eq!(result, reference["result"]);
    assert_eq!(log.entries(), ["A", "R", "C"]);

    events.close();
    mounted.process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// Every Cordis event name that reaches the rutis side, in order.
#[derive(Default)]
struct Names(Mutex<Vec<String>>);

impl EventSink for Names {
    fn event(&self, name: &str, _: Vec<Value>) -> Reply {
        self.0.lock().unwrap().push(name.to_owned());
        Ok(Value::Undefined)
    }
}

/// guarantee: boundary/5.5
///
/// Cordis emits `internal/service` when the plugin provides a service; the
/// Cordis side hears it as natively, while the rutis side, forwarding an
/// ordinary event, receives that event only. Natively a listener of the
/// application hears both provides.
#[tokio::test(flavor = "multi_thread")]
async fn internal_events_stay_on_the_cordis_side() {
    let log = Log::default();
    let names = Arc::new(Names::default());
    let mounted = Mounted::new(
        &log,
        Some((vec!["boundary/order".into()], names.clone())),
        &[],
    )
    .await;
    mounted.call("announce").await.unwrap();
    mounted.call("order").await.unwrap();
    let heard = mounted.call("internal").await.unwrap();

    let reference = native("internal");
    assert_eq!(
        strings(&reference["log"]),
        ["R:probe", "R:boundary-extra"],
        "natively the application hears internal/service"
    );
    assert_eq!(heard, reference["result"]);
    assert_eq!(*names.0.lock().unwrap(), ["boundary/order"]);

    mounted.process.dispose().await.unwrap();
}

/// guarantee: boundary/5.5
///
/// Asking the runtime to carry an `internal/*` event, in either direction,
/// is a violation it can detect, so the mount fails with an error naming
/// the event: on the rutis side before the process starts, and in the Node
/// runtime for a controller that sends such a mount anyway (#282).
#[tokio::test(flavor = "multi_thread")]
// guarantee: sdk/leaf-events (internal/* cannot be declared)
async fn a_mount_listing_an_internal_event_fails() {
    let plugin = fixture("boundary-rules.mjs");
    let mut accepted = Vec::new();
    for (events, emits) in [
        (vec!["internal/service".to_owned()], vec![]),
        (vec![], vec!["internal/service".to_owned()]),
    ] {
        let names: Arc<dyn EventSink> = Arc::new(Names::default());
        let mounted = Process::mount(
            &package(),
            Mount {
                plugins: vec![(plugin.as_path(), json!({}))],
                services: json!({ "probe": ["announce"] }),
                hosts: vec![Host {
                    name: "log".into(),
                    methods: json!({ "push": "sync" }),
                    dispatch: Arc::new(Log::default()),
                }],
                events: Some((events.clone(), names)),
                emits: emits.clone(),
                ..Default::default()
            },
        )
        .await;
        let field = if events.is_empty() { "emits" } else { "events" };
        match mounted {
            Err(Error::Value(message)) => assert_eq!(
                message,
                format!(
                    "framework-internal events do not cross the boundary \
                     (mount {field}): internal/service"
                )
            ),
            Err(error) => panic!("{error}"),
            Ok(process) => {
                process.dispose().await.unwrap();
                accepted.push(format!("forwarding {events:?} / emitting {emits:?}"));
            }
        }
    }
    assert!(accepted.is_empty(), "mounts succeeded: {accepted:?}");

    // The Node runtime checks the names before anything else in a mount.
    let log = Log::default();
    let mounted = Mounted::new(&log, None, &[]).await;
    for field in ["events", "emits"] {
        let refused = mounted
            .process
            .call_async(
                "",
                "mount",
                json!({ field: ["boundary/order", "internal/service"] }),
            )
            .await;
        match refused {
            Err(Error::Remote { message, .. }) => assert_eq!(
                message,
                if field == "emits" {
                    "event internal/service is internal to the runtime and cannot be emitted from rutis".into()
                } else {
                    format!(
                        "framework-internal events do not cross the boundary \
                     (mount {field}): internal/service"
                    )
                }
            ),
            other => panic!("the runtime took the mount: {other:?}"),
        }
    }
    mounted.process.dispose().await.unwrap();
}

/// guarantee: boundary/5.7
///
/// The `log` service rutis provides is, on the Cordis side, a proxy built
/// from the declared methods: calling it through its interface works as
/// natively (same type, same result), but it is not an instance of the
/// class the plugin declares, which natively it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_service_is_a_proxy_not_an_instance_of_the_declared_class() {
    let log = Log::default();
    let mounted = Mounted::new(&log, None, &[]).await;
    let bridged = mounted.call("host").await.unwrap();

    let reference = native("host");
    assert_eq!(
        reference["result"],
        json!({ "instance": true, "push": "function", "pushed": 1 })
    );
    assert_eq!(bridged["instance"], json!(false));
    assert_eq!(bridged["push"], reference["result"]["push"]);
    assert_eq!(bridged["pushed"], reference["result"]["pushed"]);
    assert_eq!(log.entries(), strings(&reference["log"]));

    mounted.process.dispose().await.unwrap();
}
