use rutis::events::Call;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, TypeKey};
use rutis::{Ctx, Error, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, Notify};

#[derive(serde::Serialize, serde::Deserialize)]
struct Ping;
impl Event for Ping {
    const NAME: &'static str = "ping";
    type Value = ();
}
fn listener(_: Arc<Ping>, _: Call<Ping>) -> std::future::Ready<Result<Option<()>, Error>> {
    std::future::ready(Ok(None))
}

struct Probe {
    seen: Arc<Mutex<Vec<Ctx>>>,
    gate: Arc<Notify>,
    late: Arc<Mutex<Option<oneshot::Sender<Error>>>>,
    injects: Vec<TypeKey>,
}
impl DynPlugin for Probe {
    fn name(&self) -> &str {
        "generation-probe"
    }
    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let old = ctx.clone();
            let first = {
                let mut seen = self.seen.lock().unwrap();
                seen.push(old.clone());
                seen.len() == 1
            };
            if first {
                let gate = self.gate.clone();
                let sender = self.late.lock().unwrap().take().unwrap();
                tokio::spawn(async move {
                    gate.notified().await;
                    let result = old.provide(77_u64).unwrap_err();
                    let _ = sender.send(result);
                });
            }
            Ok(Effect::Done)
        })
    }
}

async fn exercise() {
    let root = Ctx::root().unwrap();
    let provider = root.provide(1_u8).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Notify::new());
    let (tx, rx) = oneshot::channel();
    let view = root.plugin(Probe {
        seen: seen.clone(),
        gate: gate.clone(),
        late: Arc::new(Mutex::new(Some(tx))),
        injects: vec![TypeKey::of::<u8>()],
    });
    (&view).await.unwrap();
    let first = seen.lock().unwrap()[0].clone();
    let isolated = first.isolate(TypeKey::of::<u64>(), "old-generation");
    provider.dispose().await.unwrap();
    assert_eq!(view.state().state, FiberState::Pending);
    assert!(matches!(
        first.effect(|| async { Ok(()) }).map_err(Error::into_kind),
        Err(ErrorKind::InactiveGeneration { .. })
    ));
    assert!(matches!(
        first.provide(1_u64).map_err(Error::into_kind),
        Err(ErrorKind::InactiveGeneration { .. })
    ));

    let replacement = root.provide(2_u8).unwrap();
    (&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Active);
    assert_eq!(seen.lock().unwrap().len(), 2);
    gate.notify_one();
    assert!(matches!(
        &*rx.await.unwrap(),
        ErrorKind::StaleGeneration { .. }
    ));
    assert!(matches!(
        first.effect(|| async { Ok(()) }).map_err(Error::into_kind),
        Err(ErrorKind::StaleGeneration { .. })
    ));
    assert!(matches!(
        isolated
            .effect(|| async { Ok(()) })
            .map_err(Error::into_kind),
        Err(ErrorKind::StaleGeneration { .. })
    ));
    assert!(isolated.cancellation_token().is_cancelled());
    assert!(matches!(
        first.on::<Ping>(listener).map_err(Error::into_kind),
        Err(ErrorKind::StaleGeneration { .. })
    ));
    let child = first.plugin(Noop);
    child.dispose().await.unwrap();
    assert_eq!(child.state().state, FiberState::Disposed);
    let current = seen.lock().unwrap()[1].clone();
    current.provide(55_u64).unwrap();
    replacement.dispose().await.unwrap();
    view.dispose().await.unwrap();
    root.shutdown().await.unwrap();
}

struct Noop;
impl DynPlugin for Noop {
    fn name(&self) -> &str {
        "noop"
    }
    fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

struct Fails {
    seen: Arc<Mutex<Option<Ctx>>>,
}
impl DynPlugin for Fails {
    fn name(&self) -> &str {
        "fails"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        *self.seen.lock().unwrap() = Some(ctx.clone());
        Box::pin(async {
            Err(Error::from(ErrorKind::PluginFailed(
                "intentional failure".into(),
            )))
        })
    }
}

#[tokio::test]
async fn rejects_registration_after_failed_apply() {
    let root = Ctx::root().unwrap();
    let seen = Arc::new(Mutex::new(None));
    let view = root.plugin(Fails { seen: seen.clone() });
    assert!((&view).await.is_err());
    assert_eq!(view.state().state, FiberState::Failed);
    let old = seen.lock().unwrap().clone().unwrap();
    assert!(matches!(
        old.provide(1_u64).map_err(Error::into_kind),
        Err(ErrorKind::InactiveGeneration {
            state: FiberState::Failed,
            ..
        })
    ));
    view.dispose().await.unwrap();
    root.shutdown().await.unwrap();
}

#[test]
fn rejects_late_registration_current_thread() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(exercise());
}

#[test]
fn rejects_late_registration_multi_thread() {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(exercise());
}
