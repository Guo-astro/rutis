//! An unload waits for the event calls still running on the fiber at most
//! the shutdown deadline (`RUTIS_SHUTDOWN_TIMEOUT`, events protocol §7.1),
//! then says so and goes on. Its own test binary: it sets the variable.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, ErrorSink};
use rutis::{Ctx, Error, Event};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Hang;
impl Event for Hang {
    const NAME: &'static str = "t/hang";
    type Value = ();
}

/// Registers a listener of `t/hang` that never ends, not even when its
/// fiber is cancelled.
struct Stuck;
impl DynPlugin for Stuck {
    fn name(&self) -> &str {
        "stuck"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.on::<Hang>(|_, _| std::future::pending())?;
            Ok(Effect::Done)
        })
    }
}

/// guarantee: events/lifecycle
#[tokio::test(flavor = "multi_thread")]
async fn an_unload_gives_up_on_running_listeners_at_the_deadline() {
    std::env::set_var("RUTIS_SHUTDOWN_TIMEOUT", "0.3");
    let errors: Arc<Mutex<Vec<Arc<ErrorKind>>>> = Arc::default();
    let into = errors.clone();
    let sink: ErrorSink = Arc::new(move |error| into.lock().unwrap().push(error));
    let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
    let view = root.plugin(Stuck);
    (&view).await.unwrap();
    // A call of the listener, under way.
    let emitter = root.clone();
    let running = tokio::spawn(async move {
        let _ = emitter.parallel(Hang).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(5), view.dispose())
        .await
        .expect("the unload went on past the deadline")
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(250));
    let lines: Vec<String> = errors
        .lock()
        .unwrap()
        .iter()
        .map(|error| error.to_string())
        .collect();
    assert!(
        lines.contains(
            &"listeners of plugin stuck still running after 0.3s; unloading anyway".to_owned()
        ),
        "{lines:?}"
    );
    running.abort();
    drop(root);
}
