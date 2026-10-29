//! Regressions for service projection and export lifecycle (PR #73 review of
//! 908da2c): withdrawal/re-registration ordering, handles held across
//! reentrant replacement, cycles after close, stable identity of Service
//! instances, Service.check gating, effects created by service methods, and
//! slot refresh after a throwing method.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, Key, TypeKey};
use rutis::{Ctx, Error as RutisError};
use rutis_bridge::runtime::{Process, Projection};
use rutis_bridge::session::{decode, decode_value, Error};
use rutis_test_support::{until, within};
use serde_json::{json, Value};

fn package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime")
}

// What the generator emits for one exported service.
struct Counter {
    process: Arc<Process>,
    handle: String,
    /// Called once, by the first proxy dropped after it is set.
    on_drop: OnDrop,
}

type OnDrop = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;
impl Counter {
    fn call(&self, method: &str, args: Value) -> Result<Value, Error> {
        self.process.call(&self.handle, method, args)
    }
    fn current(&self) -> Result<f64, Error> {
        decode(self.call("current", json!([]))?)
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        let hook = self.on_drop.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        self.process.release(&self.handle);
    }
}

const COUNTER: &str = r#"
export function apply(ctx) {
  const make = value => ({
    current() { return value },
    swap(next) { ctx.set('counter', make(next)) },
    swapThenFail(next) { ctx.set('counter', make(next)); throw new Error('after swap') },
  })
  let withdraw = ctx.provide('counter', make(1))
  ctx.provide('control', {
    cycle(value) { withdraw(); withdraw = ctx.provide('counter', make(value)) },
  })
}
"#;

type Shared = Arc<Mutex<Option<Arc<Process>>>>;

struct Mount {
    plugin: PathBuf,
    process: Shared,
    on_drop: OnDrop,
}

impl DynPlugin for Mount {
    fn name(&self) -> &str {
        "projection-lifecycle"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let projection = Projection::new();
            let on_drop = self.on_drop.clone();
            projection.service_keyed("counter", Key::<Counter>::of(), move |process, handle| {
                Arc::new(Counter {
                    process,
                    handle,
                    on_drop: on_drop.clone(),
                })
            });
            let process = Process::launch_observed(
                &package(),
                &self.plugin,
                json!({}),
                json!({ "counter": ["current", "swap", "swapThenFail"], "control": ["cycle"] }),
                Some(projection.clone()),
            )
            .await?;
            let owner = process.clone();
            let followed = projection.clone();
            ctx.effect(move || async move {
                followed.close();
                owner.dispose().await.map_err(Into::into)
            })?;
            *self.process.lock().unwrap() = Some(process.clone());
            projection.attach(ctx, process)?;
            Ok(Effect::Done)
        })
    }
}

async fn mounted() -> (tempfile::NamedTempFile, Ctx, rutis::host::FiberView, Shared) {
    mounted_with(OnDrop::default()).await
}

async fn mounted_with(
    on_drop: OnDrop,
) -> (tempfile::NamedTempFile, Ctx, rutis::host::FiberView, Shared) {
    let mut file = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    file.write_all(COUNTER.as_bytes()).unwrap();
    let ctx = Ctx::root().unwrap();
    let shared = Shared::default();
    let view = ctx.plugin(Mount {
        plugin: file.path().to_owned(),
        process: shared.clone(),
        on_drop,
    });
    (&view).await.unwrap();
    (file, ctx, view, shared)
}

// Review #1: the new registration waits for the withdrawal to complete, and a
// failed publication is retried instead of being recorded as applied.
#[tokio::test(flavor = "current_thread")]
async fn withdrawal_and_immediate_reregistration_keep_the_service() {
    let (_file, ctx, view, shared) = mounted().await;
    let process = shared.lock().unwrap().clone().unwrap();
    process.call("control", "cycle", json!([9])).unwrap();
    until(
        || {
            ctx.get::<Counter>()
                .is_some_and(|counter| counter.current().ok() == Some(9.0))
        },
        "the counter at 9.0",
    )
    .await;
    drop(process);
    view.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

// Review #3: a handle whose proxy is being published is not released when a
// reentrant change replaces it before publication finishes. Publishing the
// value-2 proxy drops the value-1 one inside the write; its Drop takes the
// new proxy and replaces the service again before the publication returns.
#[tokio::test(flavor = "current_thread")]
async fn a_proxy_held_during_reentrant_replacement_stays_callable() {
    let on_drop = OnDrop::default();
    let (_file, ctx, view, shared) = mounted_with(on_drop.clone()).await;
    let held: Arc<Mutex<Option<Arc<Counter>>>> = Arc::default();
    let (into, reader) = (held.clone(), ctx.clone());
    *on_drop.lock().unwrap() = Some(Box::new(move || {
        let candidate = reader.get::<Counter>().unwrap();
        *into.lock().unwrap() = Some(candidate.clone());
        // Replace again while the candidate is still being published.
        candidate.call("swap", json!([3])).unwrap();
    }));
    // The registry holds the only value-1 proxy: swap through the raw handle.
    let process = shared.lock().unwrap().clone().unwrap();
    let handle = process.service("counter").unwrap();
    process.call(&handle, "swap", json!([2])).unwrap();
    until(
        || {
            held.lock().unwrap().is_some()
                && ctx
                    .get::<Counter>()
                    .is_some_and(|counter| counter.current().ok() == Some(3.0))
        },
        "the reentrant replacement published",
    )
    .await;
    let held = held.lock().unwrap().take().unwrap();
    assert_eq!(held.current().unwrap(), 2.0);
    assert_eq!(ctx.get::<Counter>().unwrap().current().unwrap(), 3.0);
    drop((held, process));
    view.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

// Review #5: closing breaks the Process -> Projection -> writer -> proxy ->
// Process cycle, so a disposed mount frees its process.
#[tokio::test(flavor = "current_thread")]
async fn a_disposed_mount_frees_its_process() {
    let (_file, ctx, view, shared) = mounted().await;
    ctx.get::<Counter>().unwrap().current().unwrap();
    let weak: Weak<Process> = Arc::downgrade(shared.lock().unwrap().as_ref().unwrap());
    shared.lock().unwrap().take();
    view.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
    until(|| weak.upgrade().is_none(), "the process freed").await;
}

// Review #7: a method that replaces the service and then throws still
// refreshes the slot.
#[tokio::test(flavor = "current_thread")]
async fn a_throwing_method_still_refreshes_the_slot() {
    let (_file, ctx, view, _shared) = mounted().await;
    let error = ctx
        .get::<Counter>()
        .unwrap()
        .call("swapThenFail", json!([9]))
        .unwrap_err();
    assert!(error.to_string().contains("after swap"));
    until(
        || {
            ctx.get::<Counter>()
                .is_some_and(|counter| counter.current().ok() == Some(9.0))
        },
        "the counter at 9.0",
    )
    .await;
    view.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

fn ticker() -> PathBuf {
    package().join("test/fixtures/ticker.mjs")
}

// Review #2 and #6: effects that Service methods create through their context
// are disposed with the mount, and reading a Service instance repeatedly does
// not look like a replacement.
#[tokio::test(flavor = "current_thread")]
async fn service_method_effects_end_with_the_mount_and_handles_stay_stable() {
    let process = Process::launch(
        &package(),
        &ticker(),
        json!({}),
        json!({ "ticker": ["start", "count"] }),
    )
    .await
    .unwrap();
    let handle = process.service("ticker").unwrap();
    assert_eq!(
        process.call(&handle, "start", json!([])).unwrap(),
        json!(true)
    );
    for _ in 0..3 {
        process.call(&handle, "count", json!([])).unwrap();
    }
    assert_eq!(process.service("ticker").as_deref(), Some("ticker"));
    // The interval registered by start() must not keep Node alive.
    within(
        "disposal, which must not wait for an effect nothing cleans up",
        process.dispose(),
    )
    .await
    .unwrap();
}

// Review #4: Service.check gates export like native injection.
#[tokio::test(flavor = "current_thread")]
async fn a_service_failing_its_check_is_not_exported() {
    let process = Process::launch(
        &package(),
        &ticker(),
        json!({ "ready": false }),
        json!({ "ticker": ["start", "count"] }),
    )
    .await
    .unwrap();
    assert_eq!(process.service("ticker"), None);
    process.dispose().await.unwrap();
}

// PR #73 review of 85ae6e7 / 6794a3a: a publication that failed and is
// retried keeps its handle; calls on exported objects refresh the slots.
const PAIR: &str = r#"
export function apply(ctx) {
  const make = value => ({ current() { return value } })
  ctx.provide('counter', make(1))
  ctx.provide('other', make(0))
  ctx.provide('control', {
    swap(value) { ctx.set('counter', make(value)) },
    bump(value) { ctx.set('other', make(value)) },
    // A live object whose method replaces the counter.
    remote() { return { swap(value) { ctx.set('counter', make(value)) } } },
  })
}
"#;

struct Other {
    process: Arc<Process>,
    handle: String,
}
impl Drop for Other {
    fn drop(&mut self) {
        self.process.release(&self.handle);
    }
}

struct PairMount {
    plugin: PathBuf,
    process: Shared,
}

impl DynPlugin for PairMount {
    fn name(&self) -> &str {
        "projection-pair"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let projection = Projection::new();
            projection.service::<Counter>("counter", |process, handle| Counter {
                process,
                handle,
                on_drop: OnDrop::default(),
            });
            projection.service::<Other>("other", |process, handle| Other { process, handle });
            let process = Process::launch_observed(
                &package(),
                &self.plugin,
                json!({}),
                json!({ "counter": ["current"], "other": ["current"], "control": ["swap", "bump", "remote"] }),
                Some(projection.clone()),
            )
            .await?;
            let owner = process.clone();
            let followed = projection.clone();
            ctx.effect(move || async move {
                followed.close();
                owner.dispose().await.map_err(Into::into)
            })?;
            *self.process.lock().unwrap() = Some(process.clone());
            projection.attach(ctx, process)?;
            Ok(Effect::Done)
        })
    }
}

async fn pair() -> (
    tempfile::NamedTempFile,
    Ctx,
    rutis::host::FiberView,
    Arc<Process>,
) {
    let mut file = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    file.write_all(PAIR.as_bytes()).unwrap();
    let ctx = Ctx::root().unwrap();
    let shared = Shared::default();
    let view = ctx.plugin(PairMount {
        plugin: file.path().to_owned(),
        process: shared.clone(),
    });
    (&view).await.unwrap();
    let process = shared.lock().unwrap().clone().unwrap();
    (file, ctx, view, process)
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_on_an_exported_object_refreshes_the_slots() {
    let (_file, ctx, view, process) = pair().await;
    let remote: rutis_bridge::session::ObjectRef =
        decode_value(process.invoke("control", "remote", vec![]).unwrap()).unwrap();
    remote
        .call("swap", vec![rutis_bridge::session::arg(&9).unwrap()])
        .unwrap();
    until(
        || {
            ctx.get::<Counter>()
                .is_some_and(|counter| counter.current().ok() == Some(9.0))
        },
        "the counter at 9.0",
    )
    .await;
    let handle = process.service("counter").unwrap();
    assert_eq!(
        process.call(&handle, "current", json!([])).unwrap(),
        json!(9)
    );
    drop((remote, process));
    view.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

// Experiment follow-up: a Node process that goes away withdraws the
// services, so consumers stop by native gating, and errors say how it ended.
struct Consumer {
    stopped: Arc<Mutex<bool>>,
    injects: [TypeKey; 1],
}

impl DynPlugin for Consumer {
    fn name(&self) -> &str {
        "consumer"
    }
    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        let stopped = self.stopped.clone();
        Box::pin(async move {
            ctx.effect(move || async move {
                *stopped.lock().unwrap() = true;
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_crashed_process_withdraws_its_services() {
    let mut file = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    file.write_all(
        br#"
export function apply(ctx) {
  ctx.provide('counter', { current() { return 1 } })
  ctx.provide('control', { cycle() { process.exit(17) } })
}
"#,
    )
    .unwrap();
    let ctx = Ctx::root().unwrap();
    let shared = Shared::default();
    let view = ctx.plugin(Mount {
        plugin: file.path().to_owned(),
        process: shared.clone(),
        on_drop: OnDrop::default(),
    });
    (&view).await.unwrap();
    let stopped = Arc::new(Mutex::new(false));
    let consumer = ctx.plugin(Consumer {
        stopped: stopped.clone(),
        injects: [TypeKey::of::<Counter>()],
    });
    (&consumer).await.unwrap();
    let process = shared.lock().unwrap().clone().unwrap();

    let error = process.call("control", "cycle", json!([])).unwrap_err();
    // `exit status: 17` on Unix, `exit code: 17` on Windows.
    let error = error.to_string();
    assert!(
        error.contains("exited with exit ") && error.ends_with(" 17"),
        "{error}"
    );
    until(
        || ctx.get::<Counter>().is_none() && *stopped.lock().unwrap(),
        "the service withdrawn and its consumer stopped",
    )
    .await;
    let status = process.exit_status().unwrap();
    assert!(
        status.starts_with("exited with exit ") && status.ends_with(" 17"),
        "{status}"
    );
    drop(process);
    let disposed = view.dispose().await.unwrap_err().to_string();
    assert!(disposed.contains(&status), "{disposed}");
    ctx.shutdown().await.unwrap();
}
