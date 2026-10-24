//! The 0.17 author API (#617): what a plugin author writes compiles and
//! behaves as the design says (docs/design-rust-api-convergence-2026-10-24.md,
//! sections 2–3 and 5). Typed and object-safe plugins depending on each
//! other is `typed_plugin.rs`'s `typed_and_untyped_plugins_depend_on_each_other`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rutis::events::Mode;
use rutis::host::{CtxHost, ErrorKind};
use rutis::{Ctx, Error, Event, Plugin};

#[derive(serde::Serialize, serde::Deserialize)]
struct Ping(u32);

impl Event for Ping {
    const NAME: &'static str = "test::AuthorPing";
    type Value = u32;
}

/// `?` takes an `io::Error` (any `std::error::Error + Send + Sync`) in
/// plugin code; the host sees it inside `PluginFailed`, and `kind()` says
/// it is the plugin's own.
#[tokio::test]
async fn question_mark_takes_an_io_error() {
    struct Reads;
    impl Plugin for Reads {
        type Deps = ();
        async fn apply(&self, _: &Ctx, (): ()) -> rutis::Result {
            std::fs::read_to_string("/nonexistent/rutis/author-api")?;
            Ok(())
        }
    }

    fn read() -> rutis::Result<String> {
        Ok(std::fs::read_to_string("/nonexistent/rutis/author-api")?)
    }
    let error = read().unwrap_err();
    assert!(
        error.kind().is_none(),
        "a plugin's own error has no rutis kind"
    );
    match &*error {
        ErrorKind::PluginFailed(source) => assert!(source.is::<std::io::Error>()),
        other => panic!("expected PluginFailed, got {other:?}"),
    }

    let root = Ctx::root().unwrap();
    let view = root.plugin(Reads);
    let failed = (&view).await.unwrap_err();
    assert!(
        matches!(&*failed, ErrorKind::PluginFailed(_)),
        "unexpected: {failed:?}"
    );
    root.shutdown().await.unwrap();
}

/// A rutis error keeps its kind through `?`.
#[test]
fn a_rutis_error_keeps_its_kind() {
    fn closed() -> rutis::Result {
        Err(ErrorKind::Closed)?
    }
    assert!(matches!(
        closed().unwrap_err().kind(),
        Some(ErrorKind::Closed)
    ));
    assert!(Error::msg("boom").kind().is_none());
}

/// Listener closures need no type annotations, async or sync, and `?`
/// works inside them; an event goes in as a value or an `Arc`.
#[tokio::test]
async fn listener_closures_need_no_annotations() {
    let root = Ctx::root().unwrap();
    let modes = Arc::new(Mutex::new(Vec::new()));
    let seen = modes.clone();
    root.on::<Ping>(move |event, call| {
        let seen = seen.clone();
        async move {
            seen.lock().unwrap().push(call.mode());
            let parsed: u32 = "1".parse()?;
            Ok(Some(event.0 + parsed))
        }
    })
    .unwrap();
    assert_eq!(root.serial(Ping(1)).await.unwrap(), Some(2));
    assert_eq!(root.serial(Arc::new(Ping(2))).await.unwrap(), Some(3));
    assert_eq!(*modes.lock().unwrap(), [Mode::Serial, Mode::Serial]);

    let other = Ctx::root().unwrap();
    other
        .on_sync::<Ping>(|event, call| {
            if call.mode() == Mode::WaterfallSync {
                return call.next();
            }
            Ok(Some(event.0 * 10))
        })
        .unwrap();
    assert_eq!(other.bail(Ping(4)).unwrap(), Some(40));
    other.emit(Arc::new(Ping(5))).unwrap();
    root.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
}

/// `effect` runs its closure now and registers the future it returns; once
/// the plugin has stopped, it is `InactiveEffect` and the closure does not
/// run, so nothing is acquired that would not be released.
#[tokio::test]
async fn effect_after_stop_is_inactive_and_does_not_run() {
    struct Keeps(Arc<Mutex<Option<Ctx>>>, Arc<AtomicBool>);
    impl Plugin for Keeps {
        type Deps = ();
        async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
            let cleaned = self.1.clone();
            ctx.effect(|| async move {
                cleaned.store(true, Ordering::SeqCst);
                Ok(())
            })?;
            *self.0.lock().unwrap() = Some(ctx.clone());
            Ok(())
        }
    }

    let root = Ctx::root().unwrap();
    let kept = Arc::new(Mutex::new(None));
    let cleaned = Arc::new(AtomicBool::new(false));
    let view = root.plugin(Keeps(kept.clone(), cleaned.clone()));
    (&view).await.unwrap();
    view.dispose().await.unwrap();
    assert!(cleaned.load(Ordering::SeqCst), "the cleanup ran at stop");

    let ctx = kept.lock().unwrap().take().unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let refused = ctx
        .effect(move || {
            flag.store(true, Ordering::SeqCst);
            async { Ok(()) }
        })
        .unwrap_err();
    assert!(
        matches!(refused.kind(), Some(ErrorKind::InactiveEffect)),
        "unexpected: {refused:?}"
    );
    assert!(!ran.load(Ordering::SeqCst), "the closure must not run");
    root.shutdown().await.unwrap();
}

// ── the error table of section 3 ─────────────────────────────────

/// A host returning `Box<dyn Error>` or `Box<dyn Error + Send + Sync>`
/// uses `?` on rutis's errors, the shared ones included.
#[tokio::test]
async fn hosts_returning_box_dyn_error_use_question_mark() {
    struct Fails;
    impl Plugin for Fails {
        type Deps = ();
        async fn apply(&self, _: &Ctx, (): ()) -> rutis::Result {
            Err(Error::msg("fails"))
        }
    }

    async fn host() -> Result<(), Box<dyn std::error::Error>> {
        let root = Ctx::root()?;
        let view = root.plugin(Fails);
        let result = (&view).await;
        root.shutdown().await?;
        result?;
        Ok(())
    }
    async fn sendable() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(Error::msg("sendable"))?;
        Ok(())
    }
    assert!(host().await.unwrap_err().to_string().contains("fails"));
    assert!(sendable()
        .await
        .unwrap_err()
        .to_string()
        .contains("sendable"));
}

/// An application's thiserror type holds a rutis error, transparently or
/// as the source; plugin code uses `?` on its own thiserror errors.
#[test]
fn thiserror_types_wrap_rutis_errors_and_convert_into_them() {
    #[derive(Debug, thiserror::Error)]
    enum AppError {
        #[error(transparent)]
        Rutis(#[from] rutis::Error),
    }
    #[derive(Debug, thiserror::Error)]
    #[error("starting failed")]
    struct Starting {
        #[source]
        cause: rutis::Error,
    }
    #[derive(Debug, thiserror::Error)]
    #[error("quota exceeded")]
    struct Quota;

    fn app() -> Result<(), AppError> {
        Err(Error::from(ErrorKind::Closed))?;
        Ok(())
    }
    let app = app().unwrap_err();
    assert_eq!(app.to_string(), ErrorKind::Closed.to_string());

    let starting = Starting {
        cause: Error::msg("no runtime"),
    };
    let source = std::error::Error::source(&starting).expect("a source");
    assert!(source.to_string().contains("no runtime"));

    fn plugin_code() -> rutis::Result {
        Err(Quota)?;
        Ok(())
    }
    let error = plugin_code().unwrap_err();
    assert!(error.kind().is_none());
    assert!(error.to_string().contains("quota exceeded"));
}

/// With the `anyhow` feature, a host returning `anyhow::Result` uses `?`,
/// and plugin code turns an `anyhow::Error` into a rutis one.
#[cfg(feature = "anyhow")]
#[tokio::test]
async fn anyhow_hosts_and_plugin_code() {
    async fn host() -> anyhow::Result<()> {
        let root = Ctx::root()?;
        root.shutdown().await?;
        Err(Error::msg("from rutis"))?;
        Ok(())
    }
    assert!(host().await.unwrap_err().to_string().contains("from rutis"));

    fn plugin_code() -> rutis::Result {
        let failing: anyhow::Result<()> = Err(anyhow::anyhow!("from anyhow"));
        failing.map_err(Error::from_anyhow)?;
        Ok(())
    }
    let error = plugin_code().unwrap_err();
    assert!(error.kind().is_none());
    assert!(error.to_string().contains("from anyhow"));
}
