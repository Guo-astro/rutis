//! A local runtime that cannot start, or ends before it greets, fails with
//! how its process ended, and its handle says so. Every handover: an
//! inherited socket by default, loopback on Windows (and with
//! `RUTIS_LOCAL_HANDOVER=loopback`).
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::runtime::{Launcher, RuntimeState};

/// Also checks what the error sink heard (stderr in an embedding, by
/// default): the runtime down, once, by its name, and no failed start of
/// rutis's own plugins.
async fn failed_with(launcher: Launcher) -> String {
    use rutis::host::ErrorKind;
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::launcher("broken", launcher, dir.path());
    let handle = runtime.handle();
    let sunk: Arc<Mutex<Vec<Arc<ErrorKind>>>> = Arc::default();
    let into = sunk.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |error| into.lock().unwrap().push(error)),
    );
    let view = root.plugin(runtime);
    let _ = (&view).await;
    assert_eq!(view.state().state, FiberState::Failed);
    let reason = match handle.state() {
        RuntimeState::Down(reason) => reason,
        _ => panic!("the runtime should be down"),
    };
    rutis_test_support::eventually(
        || (!sunk.lock().unwrap().is_empty()).then_some(()),
        "the report",
    )
    .await;
    root.shutdown().await.unwrap();
    let heard: Vec<String> = sunk.lock().unwrap().iter().map(|e| e.to_string()).collect();
    assert_eq!(heard, [format!("runtime broken: down ({reason})")]);
    reason
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_exits_before_greeting_says_how() {
    #[cfg(unix)]
    let (launcher, status) = (
        Launcher::new("sh").arg("-c").arg("exit 17"),
        "exited with exit status: 17",
    );
    #[cfg(windows)]
    let (launcher, status) = (
        Launcher::new("cmd").arg("/C").arg("exit 17"),
        "exited with exit code: 17",
    );
    let reason = failed_with(launcher.inherit_fd()).await;
    assert!(reason.contains(status), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_cannot_start_says_why() {
    let reason = failed_with(Launcher::new("/nonexistent/runtime").inherit_fd()).await;
    assert!(
        reason.contains("cannot start /nonexistent/runtime"),
        "{reason}"
    );
}

// guarantee: sdk/leaf-events (a leaf listener is on the host's bus, across restarts)
#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn local_runtime_events_follow_leaf_lifecycle() {
    use rutis::internal::CtxInternal;
    use rutis_bridge::runtime::row_projection;
    use rutis_test_support::{eventually, within};
    use serde_json::json;

    within("runtime event declarations survive restart", async {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let package = repo.join("node/rutis-runtime");
        let entry = repo.join("crates/rutis-bridge/tests/fixtures/leaf-events.mjs");
        let runtime = LocalRuntime::node(&package, package.join("package.json"));
        let changed = "leaf/changed";
        let emit = |args: Vec<rutis_bridge::session::Value>| {
            std::sync::Arc::new(rutis_bridge::events::Args(args))
        };
        let handle = runtime.handle();
        let root = Ctx::root().unwrap();
        let view = root.plugin(runtime);
        (&view).await.unwrap();
        for generation in 1..=2 {
            let process = handle.ready().await.unwrap();
            let exports = process.describe_row(&entry).await.unwrap().provides;
            process
                .load_row_exporting(
                    "observer",
                    &entry,
                    json!({ "observe": true }),
                    &[],
                    &[],
                    &exports,
                    row_projection(&exports),
                    &[],
                )
                .await
                .unwrap();
            let probe = eventually(|| process.service("probe"), "the observer").await;
            process
                .load_row("listener", &entry, json!({}), &[], &[], &[])
                .await
                .unwrap();
            root.named(changed)
                .parallel(emit(vec![json!(generation).into()]))
                .await
                .unwrap();
            let expected = json!([generation]);
            assert_eq!(
                process.call_async(&probe, "seen", json!([])).await.unwrap(),
                expected
            );
            process.unload_row("listener").await.unwrap();
            root.named(changed).parallel(emit(vec![])).await.unwrap();
            assert_eq!(
                process.call_async(&probe, "seen", json!([])).await.unwrap(),
                expected
            );
            if generation == 1 {
                view.restart().await.unwrap();
                process.closed().await;
            }
        }
        root.shutdown().await.unwrap();
    })
    .await;
}

// guarantee: events/framework (rutis/runtime)
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_announces_running_and_down() {
    use std::sync::{Arc, Mutex};

    use rutis_bridge::channel::PeerId;
    use rutis_bridge::events::{RuntimeEvent, RuntimeEventState};
    use rutis_bridge::runtime::{runtime_session_key, RuntimePlugin, RuntimeSession};
    use rutis_bridge::session::{Connection, Dispatch, Endpoint, Error, Format, Reply, Value};
    use rutis_test_support::{eventually, within};
    use serde_json::json;

    /// Mounts, and takes what the host says of elsewhere.
    struct Fake;
    impl Dispatch for Fake {
        fn invoke(&self, _: &Connection, target: &str, method: &str, _: Value) -> Reply {
            match (target, method) {
                ("", "mount") => {
                    Ok(json!({ "services": {}, "features": ["rows.v2", "hosts"] }).into())
                }
                ("", "events.elsewhere") => Ok(Value::Undefined),
                _ => Err(Error::Value(format!("no {method}"))),
            }
        }
    }
    struct Session(Connection);
    impl RuntimeSession for Session {
        fn connection(&self) -> Connection {
            self.0.clone()
        }
        fn route(
            &self,
            _: Arc<dyn Dispatch>,
        ) -> Result<Box<dyn std::any::Any + Send + Sync>, Error> {
            Ok(Box::new(()))
        }
    }
    within("rutis/runtime", async {
        let (a, b) = rutis_bridge::transport::memory::pair();
        let endpoint = |name| Format::Endpoint(Endpoint::rust(PeerId::new(name).unwrap()));
        let far = Connection::open_with(b, Arc::new(Fake), endpoint("fake")).unwrap();
        let session = Connection::open_with(a, Arc::new(Fake), endpoint("main")).unwrap();
        session.ready().await.unwrap();
        let root = Ctx::root().unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = heard.clone();
        root.on::<RuntimeEvent>(move |event, _| {
            into.lock().unwrap().push((*event).clone());
            std::future::ready(Ok(None))
        })
        .unwrap();
        root.provide_at(
            runtime_session_key("fake"),
            Arc::new(Session(session.clone())),
        )
        .unwrap();
        let view = root.plugin(RuntimePlugin::remote("fake"));
        (&view).await.unwrap();
        eventually(
            || (!heard.lock().unwrap().is_empty()).then_some(()),
            "running",
        )
        .await;
        assert_eq!(
            heard.lock().unwrap()[0],
            RuntimeEvent {
                runtime: "fake".into(),
                state: RuntimeEventState::Running,
                reason: None,
            }
        );
        far.close(Error::Transport("the runtime went".into()));
        eventually(|| (heard.lock().unwrap().len() == 2).then_some(()), "down").await;
        let down = heard.lock().unwrap()[1].clone();
        assert_eq!(down.state, RuntimeEventState::Down);
        assert!(down.reason.is_some());
        root.shutdown().await.unwrap();
    })
    .await;
}
