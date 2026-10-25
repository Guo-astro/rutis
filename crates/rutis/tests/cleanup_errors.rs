use std::sync::Arc;

use rutis::host::{CtxHost, ErrorKind};
use rutis::{Ctx, Error};
use tokio::sync::{oneshot, Semaphore};

#[tokio::test]
async fn taking_early_cleanup_errors_releases_them_and_excludes_unload() {
    let ctx = Ctx::root().unwrap();
    for _ in 0..128 {
        let disposer = ctx
            .effect(|| async {
                Err(Error::from(ErrorKind::ServiceNotFound(
                    "early cleanup".into(),
                )))
            })
            .unwrap();
        let immediate = disposer.dispose().await.unwrap_err();
        let weak = Arc::downgrade(&immediate);
        let taken = ctx.take_cleanup_errors();
        assert_eq!(taken.len(), 1);
        assert!(Arc::ptr_eq(&immediate, &taken[0]));
        assert!(ctx.take_cleanup_errors().is_empty());
        drop(immediate);
        drop(taken);
        assert!(weak.upgrade().is_none());
    }
    ctx.shutdown().await.unwrap();
}

#[tokio::test]
async fn unconsumed_early_cleanup_error_reaches_unload_with_same_identity() {
    let ctx = Ctx::root().unwrap();
    let disposer = ctx
        .effect(|| async { Err(Error::from(ErrorKind::ServiceNotFound("unconsumed".into()))) })
        .unwrap();
    let immediate = disposer.dispose().await.unwrap_err();
    let final_error = ctx.shutdown().await.unwrap_err();
    assert!(Arc::ptr_eq(&immediate, &final_error));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_cleanup_and_owner_shutdown_assign_error_once() {
    for _ in 0..32 {
        let ctx = Ctx::root().unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let release = Arc::new(Semaphore::new(0));
        let disposer = ctx
            .effect({
                let release = release.clone();
                move || async move {
                    let _ = started_tx.send(());
                    let permit = release.acquire().await.unwrap();
                    permit.forget();
                    Err(Error::from(ErrorKind::ServiceNotFound(
                        "racing cleanup".into(),
                    )))
                }
            })
            .unwrap();
        let immediate = tokio::spawn(disposer.dispose());
        started_rx.await.unwrap();
        let shutdown = ctx.shutdown();
        release.add_permits(1);
        let immediate = immediate.await.unwrap().unwrap_err();
        let taken = ctx.take_cleanup_errors();
        let final_result = shutdown.await;
        let observed = taken.len() + usize::from(final_result.is_err());
        assert_eq!(observed, 1, "error must have exactly one owner");
        if let Some(taken) = taken.first() {
            assert!(Arc::ptr_eq(&immediate, taken));
        }
        if let Err(final_error) = final_result {
            assert!(Arc::ptr_eq(&immediate, &final_error));
        }
    }
}
