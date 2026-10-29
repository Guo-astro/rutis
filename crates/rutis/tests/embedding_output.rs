//! What an embedding sees without reading any result (#678, diagnostics
//! design §6.2, §七, §5.3): a plugin that fails to start reaches the error
//! sink by its name, a shutdown that runs out of time says which plugin
//! and which cleanup it still waits for, a `#[rutis::service]` type served
//! by no name says so once, and the diagnostics print readably. rutis's own
//! plugins (`DynPlugin::platform`) stay out of all of it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{
    BoxFuture, CtxHost, DisposeWaitError, DynPlugin, Effect, ErrorKind, ErrorSink, TypeKey,
};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error, Plugin};
use rutis_test_support::within;
use tokio::sync::Notify;

/// A root whose error sink records what it is sent.
fn root() -> (Ctx, Arc<Mutex<Vec<Arc<ErrorKind>>>>) {
    let sunk = Arc::new(Mutex::new(Vec::new()));
    let sink: ErrorSink = {
        let sunk = sunk.clone();
        Arc::new(move |error| sunk.lock().unwrap().push(error))
    };
    (
        Ctx::root_with_sink(tokio::runtime::Handle::current(), sink),
        sunk,
    )
}

fn texts(sunk: &Mutex<Vec<Arc<ErrorKind>>>) -> Vec<String> {
    sunk.lock()
        .unwrap()
        .iter()
        .map(|error| error.to_string())
        .collect()
}

/// A plugin of rutis itself, or not, that fails to start or waits for
/// `key`.
struct Probe {
    name: &'static str,
    platform: bool,
    injects: Vec<TypeKey>,
    fails: bool,
}

impl DynPlugin for Probe {
    fn name(&self) -> &str {
        self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn platform(&self) -> bool {
        self.platform
    }

    fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if self.fails {
                return Err(Error::msg("no API key"));
            }
            Ok(Effect::Done)
        })
    }
}

/// A plugin that fails to start is reported to the error sink once, by
/// its name, besides the error its awaiter gets; a plugin of rutis itself
/// is not (it reports its own failures).
/// guarantee: core/features.default-output
#[tokio::test]
async fn a_failed_start_reaches_the_error_sink_by_the_plugins_name() {
    let (ctx, sunk) = root();
    let weather = ctx.plugin(Probe {
        name: "weather",
        platform: false,
        injects: Vec::new(),
        fails: true,
    });
    assert!((&weather).await.is_err());
    let link = ctx.plugin(Probe {
        name: "rutis-bridge/session",
        platform: true,
        injects: Vec::new(),
        fails: true,
    });
    assert!((&link).await.is_err());
    assert_eq!(texts(&sunk), ["weather: failed: no API key"]);
    ctx.shutdown().await.unwrap();
}

struct Missing;

/// A plugin of rutis itself waiting long (a link waiting for its peer) is
/// not reported; one of the developer's is.
/// guarantee: core/features.still-waiting
#[tokio::test(flavor = "multi_thread")]
async fn a_platform_plugin_waiting_long_is_not_reported() {
    let (ctx, sunk) = root();
    ctx.report_waiting_after(Some(Duration::from_millis(50)));
    let _link = ctx.plugin(Probe {
        name: "rutis-bridge/import#main",
        platform: true,
        injects: vec![TypeKey::of::<Missing>()],
        fails: false,
    });
    let _chat = ctx.plugin(Probe {
        name: "chat",
        platform: false,
        injects: vec![TypeKey::of::<Missing>()],
        fails: false,
    });
    within("chat's wait to be reported", async {
        while texts(&sunk).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let said = texts(&sunk);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].starts_with("chat: still waiting after 0.05s for "),
        "{said:?}"
    );
    ctx.shutdown().await.unwrap();
}

/// Holds its cleanup, registered with `ctx.effect`, until released.
struct Slow {
    release: Arc<Notify>,
    registered_at: Arc<Mutex<u32>>,
}

impl Plugin for Slow {
    type Deps = ();

    fn name(&self) -> &str {
        "slow"
    }

    async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
        let release = self.release.clone();
        *self.registered_at.lock().unwrap() = line!() + 1;
        ctx.effect(move || async move {
            release.notified().await;
            Ok(())
        })?;
        Ok(())
    }
}

/// The time limit is the host's: `shutdown_with_timeout` running out names
/// the plugin and where its stuck cleanup was registered, not the root;
/// the shutdown goes on and finishes once the cleanup does.
/// guarantee: core/features.shutdown-timeout-names-stuck
#[tokio::test]
async fn a_shutdown_that_runs_out_of_time_names_the_stuck_cleanup() {
    let (ctx, _) = root();
    let release = Arc::new(Notify::new());
    let registered_at = Arc::new(Mutex::new(0));
    (&ctx.plugin(Slow {
        release: release.clone(),
        registered_at: registered_at.clone(),
    }))
        .await
        .unwrap();
    let error = ctx
        .shutdown_with_timeout(Duration::from_millis(50))
        .await
        .unwrap_err();
    let DisposeWaitError::TimedOut { stuck, .. } = &error else {
        panic!("{error}");
    };
    let line = *registered_at.lock().unwrap();
    let at = format!("tests/embedding_output.rs:{line}:");
    assert_eq!(stuck.len(), 1, "{stuck:?}");
    assert_eq!(stuck[0].plugin, "slow");
    assert!(
        stuck[0]
            .doing
            .starts_with("still running the cleanup registered at ")
            && stuck[0].doing.contains(&at),
        "{stuck:?}"
    );
    let text = error.to_string();
    assert!(text.starts_with("not finished after "), "{text}");
    assert!(
        text.contains(": slow: still running the cleanup registered at "),
        "{text}"
    );
    assert!(!text.contains("PluginId"), "{text}");
    release.notify_one();
    within("the shutdown to finish", ctx.shutdown())
        .await
        .unwrap();
}

/// A row whose cleanup (the one its `apply` returned) is stuck, and a
/// plugin still withdrawing a service its consumer holds: each is named by
/// its row or plugin name, with what it is doing.
/// guarantee: core/features.shutdown-timeout-names-stuck
#[tokio::test]
async fn a_stuck_row_cleanup_and_a_withdrawal_are_named() {
    struct Forecast;
    struct Provider;
    impl Plugin for Provider {
        type Deps = ();
        fn name(&self) -> &str {
            "provider"
        }
        async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
            ctx.provide(Forecast)?;
            Ok(())
        }
    }
    struct Row(Arc<Notify>);
    impl DynPlugin for Row {
        fn name(&self) -> &str {
            "weather.py"
        }
        fn injects(&self) -> Vec<TypeKey> {
            vec![TypeKey::of::<Forecast>()]
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            let release = self.0.clone();
            Box::pin(async move {
                ctx.mark_row("weather")?;
                Ok(Effect::AsyncDisposer(Box::new(move || {
                    Box::pin(async move {
                        release.notified().await;
                        Ok(())
                    })
                })))
            })
        }
    }
    let (ctx, _) = root();
    let release = Arc::new(Notify::new());
    let provider = ctx.plugin(Provider);
    (&provider).await.unwrap();
    (&ctx.plugin(Row(release.clone()))).await.unwrap();
    let error = provider
        .dispose_with_timeout(Duration::from_millis(50))
        .await
        .unwrap_err();
    let DisposeWaitError::TimedOut { stuck, .. } = &error else {
        panic!("{error}");
    };
    let stuck: Vec<String> = stuck.iter().map(ToString::to_string).collect();
    assert!(
        stuck
            .iter()
            .any(|s| s.starts_with("provider: still withdrawing ") && s.contains("Forecast")),
        "{stuck:?}"
    );
    // The consumer is not below the provider: the provider's dispose names
    // only itself.
    assert_eq!(stuck.len(), 1, "{stuck:?}");
    let error = ctx
        .shutdown_with_timeout(Duration::from_millis(50))
        .await
        .unwrap_err();
    let DisposeWaitError::TimedOut { stuck, .. } = &error else {
        panic!("{error}");
    };
    let stuck: Vec<String> = stuck.iter().map(ToString::to_string).collect();
    assert!(
        stuck.contains(&"weather: still running the cleanup its apply returned".to_owned()),
        "{stuck:?}"
    );
    release.notify_one();
    within("the shutdown to finish", ctx.shutdown())
        .await
        .unwrap();
}

/// An apply that fails because it was stopped (a dispose, a shutdown) is
/// no failed start: nothing reaches the error sink.
/// guarantee: core/features.default-output
#[tokio::test]
async fn an_apply_failing_because_it_was_stopped_is_not_reported() {
    struct GivesUp(Arc<Notify>);
    impl Plugin for GivesUp {
        type Deps = ();
        fn name(&self) -> &str {
            "gives-up"
        }
        async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
            self.0.notify_one();
            ctx.cancelled().await;
            Err(Error::msg("stopped while starting"))
        }
    }
    let (ctx, sunk) = root();
    let started = Arc::new(Notify::new());
    let view = ctx.plugin(GivesUp(started.clone()));
    within("apply to start", started.notified()).await;
    let _ = within("the dispose", view.dispose()).await;
    let other = ctx.plugin(GivesUp(started.clone()));
    within("apply to start", started.notified()).await;
    within("the shutdown", ctx.shutdown()).await.unwrap();
    drop(other);
    assert!(texts(&sunk).is_empty(), "{:?}", texts(&sunk));
}

/// Only the developer's plugins are named when they are stuck: the host's
/// own cleanups only by where they were registered (as `the host`), and
/// rutis's own plugins left out while one of the developer's is listed.
/// guarantee: core/features.shutdown-timeout-names-stuck
#[tokio::test]
async fn a_stuck_shutdown_names_neither_the_root_nor_rutis_plugins_beside_the_developers() {
    struct Holds(Arc<Notify>, &'static str, bool);
    impl DynPlugin for Holds {
        fn name(&self) -> &str {
            self.1
        }
        fn platform(&self) -> bool {
            self.2
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            let release = self.0.clone();
            Box::pin(async move {
                ctx.effect(move || async move {
                    release.notified().await;
                    Ok(())
                })?;
                Ok(Effect::Done)
            })
        }
    }
    let (ctx, _) = root();
    let release = Arc::new(Notify::new());
    (&ctx.plugin(Holds(release.clone(), "rutis-bridge/import#main", true)))
        .await
        .unwrap();
    (&ctx.plugin(Holds(release.clone(), "weather", false)))
        .await
        .unwrap();
    let error = ctx
        .shutdown_with_timeout(Duration::from_millis(50))
        .await
        .unwrap_err();
    let DisposeWaitError::TimedOut { stuck, .. } = &error else {
        panic!("{error}");
    };
    let names: Vec<&str> = stuck.iter().map(|s| s.plugin.as_str()).collect();
    assert_eq!(names, ["weather"], "{error}");
    // weather's cleanup ends; now rutis's own plugin is the only lead.
    release.notify_one();
    let error = within(
        "the second wait",
        ctx.shutdown_with_timeout(Duration::from_millis(200)),
    )
    .await
    .unwrap_err();
    let DisposeWaitError::TimedOut { stuck, .. } = &error else {
        panic!("{error}");
    };
    let names: Vec<&str> = stuck.iter().map(|s| s.plugin.as_str()).collect();
    assert_eq!(names, ["rutis-bridge/import#main"], "{error}");
    release.notify_one();
    within("the shutdown to finish", ctx.shutdown())
        .await
        .unwrap();
}

/// An apply that does not return holds a shutdown: it says so.
/// guarantee: core/features.shutdown-timeout-names-stuck
#[tokio::test]
async fn an_apply_that_does_not_return_is_named() {
    struct Hangs(Arc<Notify>, Arc<Notify>);
    impl Plugin for Hangs {
        type Deps = ();
        fn name(&self) -> &str {
            "hangs"
        }
        async fn apply(&self, _: &Ctx, _: ()) -> rutis::Result {
            self.0.notify_one();
            self.1.notified().await;
            Ok(())
        }
    }
    let (ctx, _) = root();
    let (started, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let _view = ctx.plugin(Hangs(started.clone(), release.clone()));
    within("apply to start", started.notified()).await;
    let error = ctx
        .shutdown_with_timeout(Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with(": hangs: its apply has not returned"),
        "{error}"
    );
    release.notify_one();
    within("the shutdown to finish", ctx.shutdown())
        .await
        .unwrap();
}

/// A calendar the kernel knows as a marked service.
pub struct Calendar;

#[rutis::service]
impl Calendar {
    pub fn today(&self) -> String {
        "monday".into()
    }
}

/// With no rutis-bridge linked into this test, a marked service is served
/// by type only: providing it says so once, however often it is provided.
/// guarantee: core/features.not-served-by-name
#[tokio::test]
async fn a_marked_service_without_the_bridge_says_once_it_is_served_by_type_only() {
    let (ctx, sunk) = root();
    let first = ctx.provide(Calendar).unwrap();
    first.dispose().await.unwrap();
    ctx.provide(Calendar).unwrap();
    assert_eq!(
        texts(&sunk),
        [
            "service calendar is provided by type but not registered by name: depend on \
          rutis-bridge to use it by name and to check clashes"
        ]
    );
    assert!(ctx.get::<Calendar>().is_some());
    ctx.shutdown().await.unwrap();
}

/// `diagnostics()` prints one item a line by names: plugins by their row
/// or plugin name with their state and what they wait for, services with
/// their providers; rutis's own plugins and the root are left out, `{:#}`
/// lists them.
/// guarantee: core/features.readable-diagnostics
#[tokio::test]
async fn diagnostics_print_by_name_without_rutis_own_plugins() {
    struct Greeting;
    struct Greeter;
    impl Plugin for Greeter {
        type Deps = ();
        fn name(&self) -> &str {
            "greeter"
        }
        async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
            ctx.mark_row("hello")?;
            ctx.provide(Greeting)?;
            Ok(())
        }
    }
    let (ctx, _) = root();
    ctx.report_waiting_after(None);
    (&ctx.plugin(Greeter)).await.unwrap();
    let _waiting = ctx.plugin(Probe {
        name: "chat",
        platform: false,
        injects: vec![TypeKey::of::<Missing>()],
        fails: false,
    });
    let broken = ctx.plugin(Probe {
        name: "broken",
        platform: false,
        injects: Vec::new(),
        fails: true,
    });
    let _ = (&broken).await;
    let link = ctx.plugin(Probe {
        name: "rutis-bridge/session",
        platform: true,
        injects: Vec::new(),
        fails: false,
    });
    (&link).await.unwrap();
    let missing = std::any::type_name::<Missing>();
    let greeting = std::any::type_name::<Greeting>();
    let text = ctx.diagnostics().to_string();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "plugins:", "{text}");
    for line in [
        "  hello: running".to_owned(),
        format!("  chat: waiting for {missing} (service missing)"),
        "  broken: failed: no API key".to_owned(),
        "services:".to_owned(),
        format!("  {greeting}: provided by hello"),
    ] {
        assert!(lines.contains(&line.as_str()), "{line:?} in\n{text}");
    }
    assert!(!text.contains("rutis-bridge"), "{text}");
    assert!(!text.contains("root"), "{text}");
    assert!(!text.contains("PluginId"), "{text}");
    let all = format!("{:#}", ctx.diagnostics());
    assert!(all.contains("  rutis-bridge/session: running"), "{all}");
    ctx.shutdown().await.unwrap();
}
