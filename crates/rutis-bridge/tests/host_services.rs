//! rutis services provided to mounted Cordis plugins: the plugins inject them
//! natively, call them synchronously or asynchronously, and disposers the
//! host returns run when Cordis cleans up.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rutis_bridge::runtime::{Host, Mount, Process};
use rutis_bridge::session::{Error, HostDispatch};
use rutis_bridge::session::{Reply, Value};
use serde_json::json;

const CONSUMER: &str = r#"
export const inject = ['clock']
export function apply(ctx) {
  // The host returns the disposer, as Cordis services commonly do.
  ctx.effect(() => ctx.clock.watch())
  ctx.provide('reader', {
    now() { return ctx.clock.now() },
    async later() { return await ctx.clock.later() },
    missing() { return ctx.clock.rewind() },
  })
}
"#;

struct Clock {
    watching: Arc<AtomicUsize>,
}

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: Value) -> Reply {
        match method {
            "now" => Ok(json!(42).into()),
            "later" => Ok(Value::future(async { Ok(json!(7).into()) })),
            "watch" => {
                self.watching.fetch_add(1, Ordering::SeqCst);
                let watching = self.watching.clone();
                Ok(Value::callback(move |_| {
                    watching.fetch_sub(1, Ordering::SeqCst);
                    Ok(Value::Undefined)
                }))
            }
            _ => Err(Error::Value(format!("clock.{method} is not implemented"))),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_inject_and_call_host_services() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let mut consumer = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    consumer.write_all(CONSUMER.as_bytes()).unwrap();
    let watching = Arc::new(AtomicUsize::new(0));
    let process = Process::mount(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime"),
        Mount {
            plugins: vec![(consumer.path(), json!({}))],
            services: json!({ "reader": ["now", "later", "missing"] }),
            hosts: vec![Host {
                name: "clock".into(),
                methods: json!({ "now": "sync", "later": "async", "watch": "sync" }),
                dispatch: Arc::new(Clock {
                    watching: watching.clone(),
                }),
            }],
            ..Mount::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(watching.load(Ordering::SeqCst), 1);
    let reader = process.service("reader").unwrap();
    assert_eq!(process.call(&reader, "now", json!([])).unwrap(), json!(42));
    assert_eq!(
        process
            .call_async(&reader, "later", json!([]))
            .await
            .unwrap(),
        json!(7)
    );
    let error = process.call(&reader, "missing", json!([])).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("clock has no method rewind (methods: later, now, watch)"),
        "{error}"
    );
    process.dispose().await.unwrap();
    // Cordis cleanup called the disposer the host returned.
    assert_eq!(watching.load(Ordering::SeqCst), 0);
}
