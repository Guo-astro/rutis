//! A Cordis service slot projected into rutis follows replacement and
//! withdrawal through public rutis API, while earlier snapshots keep
//! addressing their original object.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::{Ctx, Error as RutisError};
use rutis_bridge::runtime::{Mount as RuntimeMount, Process, Projection};
use rutis_bridge::session::{decode, Error};
use rutis_test_support::until;
use serde_json::json;

// What the generator emits for one exported service.
struct Counter {
    process: Arc<Process>,
    handle: String,
}
impl Counter {
    fn current(&self) -> Result<f64, Error> {
        decode(self.process.call(&self.handle, "current", json!([]))?)
    }
    fn swap(&self, value: f64) -> Result<(), Error> {
        self.process.call(&self.handle, "swap", json!([value]))?;
        Ok(())
    }
    fn assign(&self, value: f64) -> Result<(), Error> {
        self.process.call(&self.handle, "assign", json!([value]))?;
        Ok(())
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        self.process.release(&self.handle);
    }
}

const PLUGIN: &[u8] = br#"
export function apply(ctx) {
  const make = value => ({
    current() { return value },
    swap(next) { ctx.set('counter', make(next)) },
    assign(next) { ctx.counter = make(next) },
  })
  let withdraw = ctx.provide('counter', make(1))
  ctx.provide('control', {
    withdraw() { withdraw(); withdraw = undefined },
    restore(value) { withdraw = ctx.provide('counter', make(value)) },
  })
}
"#;

struct Mount {
    plugin: std::path::PathBuf,
    process: Arc<std::sync::Mutex<Option<Arc<Process>>>>,
}

impl DynPlugin for Mount {
    fn name(&self) -> &str {
        "projection-test"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let projection = Projection::new();
            projection.service::<Counter>("counter", |process, handle| Counter { process, handle });
            let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
            let process = Process::mount(
                &package,
                RuntimeMount {
                    plugins: vec![(&self.plugin, json!({}))],
                    services: json!({ "counter": ["current", "swap", "assign"], "control": ["withdraw", "restore"] }),
                    observer: Some(projection.clone()),
                    ..RuntimeMount::default()
                },
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

/// guarantee: boundary/5.1
#[tokio::test(flavor = "current_thread")]
async fn projected_service_follows_replacement_and_withdrawal() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let mut file = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    file.write_all(PLUGIN).unwrap();
    let ctx = Ctx::root().unwrap();
    let slot = Arc::default();
    let view = ctx.plugin(Mount {
        plugin: file.path().to_owned(),
        process: Arc::clone(&slot),
    });
    (&view).await.unwrap();
    let process = slot.lock().unwrap().clone().unwrap();

    let first = ctx.get::<Counter>().unwrap();
    assert_eq!(first.current().unwrap(), 1.0);

    // ctx.set emits nothing in Cordis; the slot is re-read after the call.
    first.swap(2.0).unwrap();
    let second = ctx.get::<Counter>().unwrap();
    assert_eq!(second.current().unwrap(), 2.0);
    assert_eq!(first.current().unwrap(), 1.0, "a snapshot keeps its object");

    // Property assignment goes through internal/set.
    second.assign(3.0).unwrap();
    assert_eq!(ctx.get::<Counter>().unwrap().current().unwrap(), 3.0);

    // A released, replaced object is dropped on the Cordis side.
    let released = first.handle.clone();
    drop(first);
    until(
        || process.call(&released, "current", json!([])).is_err(),
        "the released object dropped",
    )
    .await;

    // Withdrawal removes the native binding; a new provide publishes again.
    process.call("control", "withdraw", json!([])).unwrap();
    until(|| ctx.get::<Counter>().is_none(), "the binding withdrawn").await;
    process.call("control", "restore", json!([5])).unwrap();
    until(
        || {
            ctx.get::<Counter>()
                .is_some_and(|counter| counter.current().ok() == Some(5.0))
        },
        "the counter at 5.0",
    )
    .await;
    assert_eq!(second.current().unwrap(), 2.0);

    drop(second);
    view.dispose().await.unwrap();
    assert!(ctx.get::<Counter>().is_none());
    ctx.shutdown().await.unwrap();
}
