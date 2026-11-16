//! A mount plugin whose `injects` name a rutis host service (#622's
//! `MNT-5-7`, mount design §5): the mount waits for the service natively, and
//! withdrawing it stops the mount and closes its Node process with it; the
//! service is not replaced in place under a running process, and providing it
//! again starts a new process.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, TypeKey};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::{Host, Mount, Process};
use rutis_bridge::session::{Error as SessionError, HostDispatch, Value};
use rutis_test_support::{scaled, within};
use serde_json::json;

/// The host service the mounted plugin depends on.
trait Greeter: Send + Sync + 'static {
    fn hello(&self) -> String;
}

struct Words;

impl Greeter for Words {
    fn hello(&self) -> String {
        "hello from rust".into()
    }
}

/// Serves calls from the Cordis side to the rutis [`Greeter`].
struct GreeterDispatch(Arc<dyn Greeter>);

impl HostDispatch for GreeterDispatch {
    fn invoke(&self, method: &str, _: Value) -> rutis_bridge::session::Reply {
        match method {
            "hello" => Ok(json!(self.0.hello()).into()),
            other => Err(SessionError::value(format!(
                "greeter has no method {other}"
            ))),
        }
    }
}

/// The generated mount's `apply`, hand-written: it names the host service in
/// `injects` (so the kernel gates the mount on it), mounts the runtime with
/// the service registered, and closes the process as the mount's cleanup.
struct Mounted {
    entry: std::path::PathBuf,
    /// Every process the mount started, in order.
    started: Arc<Mutex<Vec<Arc<Process>>>>,
}

impl DynPlugin for Mounted {
    fn name(&self) -> &str {
        "cordis:greeter"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<dyn Greeter>()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let host = CtxHost::require::<dyn Greeter>(ctx)?;
            let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
            let process = Process::mount(
                &package,
                Mount {
                    plugins: vec![(self.entry.as_path(), json!({}))],
                    services: json!({}),
                    hosts: vec![Host {
                        name: "greeter".into(),
                        methods: json!({ "hello": { "sync": true } }),
                        dispatch: Arc::new(GreeterDispatch(host)),
                    }],
                    // The plugins' listeners join this context's bus.
                    ctx: Some(ctx.clone()),
                    ..Mount::default()
                },
            )
            .await?;
            self.started.lock().unwrap().push(process.clone());
            let owner = process.clone();
            ctx.effect(move || async move { owner.dispose().await.map_err(Into::into) })?;
            Ok(Effect::Done)
        })
    }
}

/// The mounted plugin: it injects the host service at the Cordis level too, so
/// its `apply` calls into rutis.
const PLUGIN: &str = r#"
export const inject = ['greeter']
export function apply(ctx) {
  ctx.greeter.hello()
}
"#;

/// `MNT-5-7`: withdrawing the host service stops the mount and ends its Node
/// process; providing it again starts a new one, never replacing the service
/// under the old process.
///
/// guarantee: core/D14
#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_a_host_service_closes_the_mounts_process() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let mut plugin = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    plugin.write_all(PLUGIN.as_bytes()).unwrap();

    let ctx = Ctx::root().unwrap();
    let started: Arc<Mutex<Vec<Arc<Process>>>> = Arc::default();
    let view = ctx.plugin(Mounted {
        entry: plugin.path().to_owned(),
        started: started.clone(),
    });
    (&view).await.unwrap();
    // The host service is not provided yet: the mount waits, and starts nothing.
    assert_eq!(view.state().state, rutis::host::FiberState::Pending);
    assert!(started.lock().unwrap().is_empty());

    let host = ctx.provide_as::<dyn Greeter>(Arc::new(Words)).unwrap();
    within("the mount to run once its host service is there", async {
        while view.state().state != rutis::host::FiberState::Active {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    let first = started.lock().unwrap()[0].clone();
    assert!(!first.ended(), "the mount's process is not running");

    // Withdrawing the host service stops the mount natively, and the mount's
    // cleanup closes its process: the service is not replaced under it.
    host.dispose().await.unwrap();
    (&view).await.unwrap();
    assert_eq!(view.state().state, rutis::host::FiberState::Pending);
    assert_eq!(
        started.lock().unwrap().len(),
        1,
        "a process was replaced in place"
    );
    within("the mount's process to end with the withdrawal", async {
        let deadline = std::time::Instant::now() + scaled(std::time::Duration::from_secs(10));
        while !first.ended() {
            assert!(
                std::time::Instant::now() < deadline,
                "the Node process outlived the mount that started it"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;

    // Providing it again starts the mount again: a new process, and the old
    // one stays ended.
    let host = ctx.provide_as::<dyn Greeter>(Arc::new(Words)).unwrap();
    within("the mount to run again", async {
        while view.state().state != rutis::host::FiberState::Active {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    let second = started.lock().unwrap()[1].clone();
    assert!(
        !Arc::ptr_eq(&first, &second),
        "the mount reused the process it ended"
    );
    assert!(first.ended());
    assert!(!second.ended());

    view.dispose().await.unwrap();
    within("the second process to end with the mount", async {
        let deadline = std::time::Instant::now() + scaled(std::time::Duration::from_secs(10));
        while !second.ended() {
            assert!(
                std::time::Instant::now() < deadline,
                "the process outlived the view"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    drop(host);
    ctx.shutdown().await.unwrap();
}
