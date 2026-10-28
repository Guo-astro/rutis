//! Release of dependency declarations: they are unregistered when the
//! driver exits, so keyed declarations (a unique qualifier per instance) do
//! not accumulate in `inject_index` under a long-lived root.

use super::*;
use crate::ctx::Ctx;
use crate::host::CtxHost;
use crate::{BoxFuture, DependencyStatus, Effect, Error};
use std::time::Duration;

struct Declares {
    keys: Vec<TypeKey>,
}

impl crate::DynPlugin for Declares {
    fn name(&self) -> &str {
        "declares"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.keys.to_vec()
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test]
async fn churn_injects_release_index() {
    let ctx = Ctx::root().expect("runtime in scope");
    for i in 0..25 {
        let key = TypeKey::keyed_dynamic::<u32>(format!("dep/{i}"));
        // 依赖永不满足:保持 Pending,直接 dispose 进终态
        let view = ctx.plugin(Declares { keys: vec![key] });
        view.dispose().await.unwrap();
        drop(view);
    }
    for _ in 0..5_000 {
        let empty = ctx
            .shared()
            .registry
            .inject_index
            .lock()
            .unwrap()
            .is_empty();
        if empty {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let index = ctx.shared().registry.inject_index.lock().unwrap();
    assert!(
        index.is_empty(),
        "inject declarations must unregister on terminal exit"
    );
}

#[tokio::test]
async fn foreign_instance_dependency_never_enters_notification_index() {
    let root = Ctx::root().unwrap();
    let owner = root.plugin(Declares { keys: vec![] });
    (&owner).await.unwrap();
    let foreign = TypeKey::instance::<u32>(owner.inner.instance);
    let pending = root.plugin(Declares {
        keys: vec![foreign.clone()],
    });
    (&pending).await.unwrap();
    assert_eq!(
        root.diagnostics()
            .plugins
            .iter()
            .find(|plugin| plugin.id == pending.id)
            .unwrap()
            .injects[0]
            .status,
        DependencyStatus::OutOfScope
    );
    assert!(!root
        .shared()
        .registry
        .inject_index
        .lock()
        .unwrap()
        .contains_key(&foreign));
    root.shutdown().await.unwrap();
}
