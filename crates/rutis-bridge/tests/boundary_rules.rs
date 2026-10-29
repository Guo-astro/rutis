//! The boundary rule for Cordis plugins (requirements-protocol-plugins §5)
//! that concerns host services: rule 7. The event rules (2–5) are gone
//! since rutis 0.13: events follow one model wherever their listeners are
//! (`docs/design-events-2026-10-18.md`), checked by the event expectation
//! file and `event_hub.rs`.
//!
//! The test mounts `node/rutis-runtime/test/fixtures/boundary-rules.mjs` in
//! the Node runtime and runs the same operation in native Cordis
//! (`boundary-native.mjs`). Every step writes to one log: on the Cordis
//! side through the `log` service, which rutis provides (rule 7). The test
//! then states what differs from native Cordis, and that the difference is
//! exactly what the rule allows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis_bridge::runtime::{Host, Mount, Process};
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

/// A mounted `boundary-rules.mjs`, its `probe` service, and the log.
struct Mounted {
    process: Arc<Process>,
    probe: String,
}

impl Mounted {
    async fn new(log: &Log) -> Self {
        let plugin = fixture("boundary-rules.mjs");
        let process = Process::mount(
            &package(),
            Mount {
                plugins: vec![(plugin.as_path(), json!({}))],
                services: json!({ "probe": ["host"] }),
                hosts: vec![Host {
                    name: "log".into(),
                    methods: json!({ "push": "sync" }),
                    dispatch: Arc::new(log.clone()),
                }],
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

fn strings(value: &Json) -> Vec<String> {
    serde_json::from_value(value.clone()).unwrap()
}

/// guarantee: boundary/5.7
///
/// The `log` service rutis provides is, on the Cordis side, a proxy built
/// from the declared methods: calling it through its interface works as
/// natively (same type, same result), but it is not an instance of the
/// class the plugin declares, which natively it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_service_is_a_proxy_not_an_instance_of_the_declared_class() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let log = Log::default();
    let mounted = Mounted::new(&log).await;
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
