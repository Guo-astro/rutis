use std::future::IntoFuture;
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, EffectPhase, ErrorKind};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error, Event};
use tokio::sync::oneshot;

#[tokio::test]
async fn named_effect_reports_actual_nested_cleanup_and_disappears_after_disposal() {
    let ctx = Ctx::root().unwrap();
    let view = ctx.root_view().unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let leaf = |index, order: Arc<Mutex<Vec<u32>>>| {
        Effect::Disposer(Box::new(move || {
            order.lock().unwrap().push(index);
            Ok(())
        }))
    };
    let disposer = ctx
        .effect_named("top", || {
            Effect::Many(vec![
                leaf(1, order.clone()),
                Effect::Many(vec![leaf(2, order.clone()), leaf(3, order.clone())]),
            ])
        })
        .unwrap();
    let effects = view.effects();
    let top = effects.iter().find(|effect| effect.label == "top").unwrap();
    assert_eq!(top.phase, EffectPhase::Live);
    assert_eq!(top.children.len(), 2);
    assert_eq!(top.children[0].label, "0: disposer");
    assert_eq!(top.children[1].label, "1: many");
    assert_eq!(top.children[1].children.len(), 2);
    assert!(top
        .children
        .iter()
        .all(|child| child.phase == EffectPhase::Live));
    disposer.dispose().await.unwrap();
    assert_eq!(order.lock().unwrap().as_slice(), [3, 2, 1]);
    assert!(view.effects().iter().all(|effect| effect.label != "top"));

    let anonymous = ctx.effect(|| async { Ok(()) }).unwrap();
    assert!(view
        .effects()
        .iter()
        .any(|effect| effect.label == "anonymous"));
    anonymous.dispose().await.unwrap();
}

#[tokio::test]
async fn draining_record_remains_visible_until_cleanup_finishes() {
    let ctx = Ctx::root().unwrap();
    let view = ctx.root_view().unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let disposer = ctx
        .effect_named("slow", || {
            Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    entered_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    Ok(())
                })
            }))
        })
        .unwrap();
    let drain = tokio::spawn(disposer.dispose());
    entered_rx.await.unwrap();
    let effect = view
        .effects()
        .into_iter()
        .find(|e| e.label == "slow")
        .unwrap();
    assert_eq!(effect.phase, EffectPhase::Draining);
    release_tx.send(()).unwrap();
    drain.await.unwrap().unwrap();
    assert!(view.effects().iter().all(|effect| effect.label != "slow"));
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Ping;

impl Event for Ping {
    const NAME: &'static str = "effect-tree-ping";
    type Value = ();
}

struct Nop;

impl Nop {
    async fn on_event(
        &self,
        _ctx: &rutis::Ctx,
        _event: &Ping,
        _call: rutis::events::Call<Ping>,
    ) -> Result<Option<()>, Error> {
        Ok(None)
    }
}

struct NamedPlugin;

impl DynPlugin for NamedPlugin {
    fn name(&self) -> &str {
        "named-plugin"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.effect_named("child-owned", || Effect::Done)?;
            Ok(Effect::Done)
        })
    }
}

#[tokio::test]
async fn framework_labels_and_child_ownership_follow_lifecycle() {
    let root = Ctx::root().unwrap();
    let root_view = root.root_view().unwrap();
    let service = root.provide(7u64).unwrap();
    let listener = root
        .on::<Ping>({
            let __listener = std::sync::Arc::new(Nop);
            move |event, call| {
                let __listener = __listener.clone();
                async move {
                    let __ctx = call.ctx().clone();
                    __listener.on_event(&__ctx, &event, call).await
                }
            }
        })
        .unwrap();
    let effects = root_view.effects();
    assert!(effects
        .iter()
        .any(|e| e.label.contains("service provide:") && e.label.contains("u64")));
    assert!(effects
        .iter()
        .any(|e| e.label.contains("event listener:") && e.label.contains("effect-tree-ping")));

    let child = root.plugin(NamedPlugin);
    (&child).into_future().await.unwrap();
    assert!(root_view
        .effects()
        .iter()
        .any(|e| e.label.contains("plugin mount: named-plugin")));
    assert!(child.effects().iter().any(|e| e.label == "child-owned"));
    assert!(child
        .effects()
        .iter()
        .any(|e| e.label == "plugin apply: named-plugin"));
    child.restart().await.unwrap();
    assert_eq!(
        child
            .effects()
            .iter()
            .filter(|e| e.label == "child-owned")
            .count(),
        1
    );
    child.shutdown().await.unwrap();
    assert!(child.effects().is_empty());
    assert!(root_view
        .effects()
        .iter()
        .all(|e| !e.label.contains("plugin mount: named-plugin")));
    listener.dispose().await.unwrap();
    service.dispose().await.unwrap();
    assert!(root_view.effects().is_empty());
}

struct FailingPlugin;

impl DynPlugin for FailingPlugin {
    fn name(&self) -> &str {
        "failing-plugin"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.effect_named("rollback", || Effect::Done)?;
            Err(rutis::Error::from(ErrorKind::PluginFailed(
                "failure".into(),
            )))
        })
    }
}

#[tokio::test]
async fn failed_apply_rolls_back_metadata() {
    let root = Ctx::root().unwrap();
    let child = root.plugin(FailingPlugin);
    assert!((&child).into_future().await.is_err());
    assert!(child.effects().is_empty());
    child.shutdown().await.unwrap_err();
    assert!(child.effects().is_empty());
}
