//! A service that emits events from its methods keeps a clone of its
//! plugin's `Ctx` (#667): it emits as that plugin. Once the plugin has
//! stopped the service is withdrawn and registering on the kept ctx is
//! refused (what docs/guide/rust-plugin.md says).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::CtxHost;
use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Created(pub String);

impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = ();
}

/// The service: its methods emit with the plugin's `Ctx`.
pub struct Sessions {
    ctx: Ctx,
}

impl Sessions {
    pub fn create(&self, id: &str) -> Result {
        self.ctx.emit(Created(id.to_owned()))
    }

    pub async fn create_and_wait(&self, id: &str) -> Result {
        self.ctx.parallel(Created(id.to_owned())).await
    }
}

pub struct SessionsPlugin;

impl Plugin for SessionsPlugin {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Sessions { ctx: ctx.clone() })?;
        Ok(())
    }
}

/// The emits of a service holding its plugin's ctx, before and after the stop.
#[tokio::test]
async fn a_service_emits_with_the_ctx_it_kept() {
    let root = Ctx::root().unwrap();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let log = heard.clone();
    root.on_sync::<Created>(move |event, _| {
        log.lock().unwrap().push(event.0.clone());
        Ok(None)
    })
    .unwrap();
    let plugin = root.plugin(SessionsPlugin);
    plugin.running().await.unwrap();
    let sessions = root.get::<Sessions>().unwrap();
    sessions.create_and_wait("a").await.unwrap();
    // `emit` returns at once; its listeners run after.
    sessions.create("b").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while heard.lock().unwrap().len() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*heard.lock().unwrap(), ["a", "b"]);

    // Stopped: the service is withdrawn, and registering on the ctx the
    // service kept is refused. (Whether emitting from it still reaches
    // listeners is not promised.)
    plugin.dispose().await.unwrap();
    assert!(root.get::<Sessions>().is_none());
    let refused = sessions
        .ctx
        .on_sync::<Created>(|_, _| Ok(None))
        .unwrap_err();
    assert!(
        matches!(refused.kind(), Some(rutis::host::ErrorKind::InactiveEffect)),
        "{refused}"
    );
    root.shutdown().await.unwrap();
}
