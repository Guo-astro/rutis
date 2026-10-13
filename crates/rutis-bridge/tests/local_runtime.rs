//! A local runtime that cannot start, or ends before it greets, fails with
//! how its process ended, and its handle says so. Every handover: an
//! inherited socket by default, loopback on Windows (and with
//! `RUTIS_LOCAL_HANDOVER=loopback`).
use rutis::{Ctx, FiberState};
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::runtime::{Launcher, RuntimeState};

async fn failed_with(launcher: Launcher) -> String {
    let dir = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::launcher("broken", launcher, dir.path());
    let handle = runtime.handle();
    let root = Ctx::root().unwrap();
    let view = root.plugin(runtime);
    let _ = (&view).await;
    assert_eq!(view.state().state, FiberState::Failed);
    let reason = match handle.state() {
        RuntimeState::Down(reason) => reason,
        _ => panic!("the runtime should be down"),
    };
    root.shutdown().await.unwrap();
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

#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn local_runtime_events_follow_leaf_lifecycle() {
    use rutis_bridge::runtime::row_projection;
    use rutis_test_support::{eventually, within};
    use serde_json::json;

    within("runtime event declarations survive restart", async {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let package = repo.join("node/rutis-runtime");
        let entry = repo.join("crates/rutis-bridge/tests/fixtures/leaf-events.mjs");
        let runtime =
            LocalRuntime::node(&package, package.join("package.json")).emits(["leaf/changed"]);
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
                )
                .await
                .unwrap();
            let probe = eventually(|| process.service("probe"), "the observer").await;
            process
                .load_row("listener", &entry, json!({}), &[], &[])
                .await
                .unwrap();
            process
                .emit("leaf/changed", vec![json!(generation).into()])
                .await
                .unwrap();
            let expected = json!([generation]);
            assert_eq!(
                process.call_async(&probe, "seen", json!([])).await.unwrap(),
                expected
            );
            process.unload_row("listener").await.unwrap();
            process.emit("leaf/changed", vec![]).await.unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_events_refuses_emits_before_rows_load() {
    use std::sync::Arc;

    use rutis_bridge::channel::PeerId;
    use rutis_bridge::runtime::{runtime_session_key, RuntimePlugin, RuntimeSession};
    use rutis_bridge::session::{Connection, Dispatch, Endpoint, Error, Format, Reply, Value};
    use rutis_test_support::within;
    use serde_json::json;

    // This old runtime has the row contract but not the event capability.
    struct OldRuntime;
    impl Dispatch for OldRuntime {
        fn invoke(&self, _: &Connection, target: &str, method: &str, _: Value) -> Reply {
            assert_eq!((target, method), ("", "mount"));
            Ok(json!({ "services": {}, "features": ["rows.v2", "hosts"] }).into())
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
            // No rows load, so this runtime makes no calls back to its host.
            Ok(Box::new(()))
        }
    }
    within("the old runtime refusing declared events", async {
        let (a, b) = rutis_bridge::transport::memory::pair();
        let endpoint = |name| Format::Endpoint(Endpoint::rust(PeerId::new(name).unwrap()));
        let old = Connection::open_with(b, Arc::new(OldRuntime), endpoint("old")).unwrap();
        let session = Connection::open_with(a, Arc::new(OldRuntime), endpoint("main")).unwrap();
        session.ready().await.unwrap();
        let root = Ctx::root().unwrap();
        root.provide_as::<dyn RuntimeSession>(
            runtime_session_key("old"),
            Arc::new(Session(session.clone())),
        )
        .unwrap();
        let runtime = RuntimePlugin::remote("old").emits(["leaf/changed"]);
        let handle = runtime.handle();
        let view = root.plugin(runtime);
        assert!((&view).await.is_err());
        let RuntimeState::Down(message) = handle.state() else {
            panic!("the runtime should refuse declared events before rows load")
        };
        assert!(message.contains("lacks events"), "{message}");
        root.shutdown().await.unwrap();
        session.close(Error::Transport("test finished".into()));
        old.closed().await;
    })
    .await;
}
