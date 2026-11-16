//! What `restart()` and `update()` report when a terminal dispose wins the
//! race between their check and their post (#622's doubt).
//!
//! The check and the pre-cancel run under the transition lock; the post runs
//! after it. A dispose registered in between is taken up first, and the
//! restart's intent is either drained by the driver's exit or posted after
//! the driver stopped. Reporting the dispose's (usually clean) result as the
//! restart's would say the call succeeded although no generation started.

use super::*;
use crate::host::CtxHost;
use crate::{BoxFuture, Effect, Error};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

struct Noop;

impl crate::DynPlugin for Noop {
    fn name(&self) -> &str {
        "noop"
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

/// `apply` holds the driver until the test releases it, so intents posted
/// meanwhile queue behind it in a fixed order.
struct Gate {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl crate::DynPlugin for Gate {
    fn name(&self) -> &str {
        "gate"
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async {
            self.started.notify_one();
            self.release.notified().await;
            Ok(Effect::Done)
        })
    }
}

fn assert_lost_to_dispose(result: Result<(), Arc<ErrorKind>>) {
    let error = result.expect_err("the call reported success although nothing restarted");
    assert!(
        matches!(&*error, ErrorKind::InactiveEffect),
        "the call reported {error:?}, not InactiveEffect"
    );
}

/// `restart()` checks, then a dispose registers and completes before the
/// restart posts: the dispose's task is the fiber's terminal result, and the
/// restart must not join it as its own success.
///
/// guarantee: core/features.errors
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_that_loses_to_a_dispose_reports_it() {
    let ctx = Ctx::root().expect("runtime in scope");
    let view = ctx.plugin(Noop);
    (&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Active);

    let disposed = Arc::new(Mutex::new(None));
    test_hooks::on_restart_before_post({
        let view = view.clone();
        let disposed = disposed.clone();
        move || {
            let handle = tokio::runtime::Handle::current();
            let result = tokio::task::block_in_place(|| handle.block_on(view.dispose()));
            *disposed.lock().unwrap() = Some(result);
        }
    });
    let result = view.restart().await;
    let dispose = disposed.lock().unwrap().take().expect("the hook ran");
    assert!(dispose.is_ok(), "the dispose reported {dispose:?}");
    assert_lost_to_dispose(result);
    assert_eq!(
        view.state().state,
        FiberState::Disposed,
        "the fiber did not end disposed"
    );
}

/// The same window in `update()`: the dry run and the config store happen
/// before the post, so a dispose registered after the second check wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_loses_to_a_dispose_reports_it() {
    struct Factory;

    impl crate::host::PluginFactory<u32> for Factory {
        fn validate_config(&self, _config: &u32) -> Result<(), Error> {
            Ok(())
        }

        fn build(&self, _config: &u32) -> Result<Box<dyn crate::DynPlugin>, Error> {
            Ok(Box::new(Noop))
        }
    }

    let ctx = Ctx::root().expect("runtime in scope");
    let view = ctx.plugin_with(Factory, 1u32);
    (&view).await.unwrap();
    assert_eq!(view.state().state, FiberState::Active);

    let disposed = Arc::new(Mutex::new(None));
    test_hooks::on_restart_before_post({
        let view = view.clone();
        let disposed = disposed.clone();
        move || {
            let handle = tokio::runtime::Handle::current();
            let result = tokio::task::block_in_place(|| handle.block_on(view.dispose()));
            *disposed.lock().unwrap() = Some(result);
        }
    });
    let result = view.update(2u32).await;
    let dispose = disposed.lock().unwrap().take().expect("the hook ran");
    assert!(dispose.is_ok(), "the dispose reported {dispose:?}");
    assert_lost_to_dispose(result);
    assert_eq!(view.state().state, FiberState::Disposed);
}

/// The intent was already posted when the driver took the dispose up: the
/// `Dispose` arm drains the mailbox after the unload, and the drained restart
/// must not be completed with the dispose's clean result.
#[tokio::test]
async fn a_restart_drained_behind_a_dispose_reports_it() {
    let ctx = Ctx::root().expect("runtime in scope");
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let view = ctx.plugin(Gate {
        started: started.clone(),
        release: release.clone(),
    });
    // The driver is inside `apply`; a dispose and a restart posted now queue
    // behind it, in this order.
    started.notified().await;
    let fiber = view.inner.clone();
    let dispose = view.dispose();
    // What `restart()` leaves before its post: the generation is
    // pre-cancelled and the count is taken.
    {
        let mut tr = fiber.transition.lock().unwrap();
        tr.pending_restarts += 1;
        tr.pre_cancelled = true;
        fiber.cancel_current();
    }
    let task = TransitionTask::new();
    assert!(
        fiber.post_join_restart(task.clone()),
        "the driver stopped before the restart was posted"
    );
    release.notify_one();

    let dispose = dispose.await;
    assert!(dispose.is_ok(), "the dispose reported {dispose:?}");
    assert_lost_to_dispose(join_task(&task).await);
    assert_eq!(view.state().state, FiberState::Disposed);
}

/// The driver already stopped when the restart posted: the late post
/// completes at once, and `alive == false` alone must not read as success.
#[tokio::test]
async fn a_restart_posted_after_the_driver_stopped_reports_it() {
    let ctx = Ctx::root().expect("runtime in scope");
    let view = ctx.plugin(Noop);
    (&view).await.unwrap();
    view.dispose().await.unwrap();
    let task = TransitionTask::new();
    assert!(
        !view.inner.post_join_restart(task.clone()),
        "the driver was still alive after the dispose completed"
    );
    assert_lost_to_dispose(join_task(&task).await);
}
