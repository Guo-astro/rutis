//! A plugin that waits a long time for services nothing provides is
//! reported once to the error sink, naming them (#676): an embedding
//! application otherwise sees nothing, the plugin staying `Pending`.
//!
//! One thread keeps the time for every root, not the runtime's timer: the
//! kernel runs on whatever runtime it is given, timers enabled or not. When
//! a wait is due, the check runs on the fiber's runtime.

use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use super::{FiberInner, FiberState};
use crate::error::ErrorKind;

/// How long a plugin waits before it is reported, unless the root says
/// otherwise (`CtxHost::report_waiting_after`).
pub(crate) const DEFAULT_REPORT_AFTER: Duration = Duration::from_secs(10);

/// One fiber's wait: due at `due`, for its generation `generation`.
struct Watch {
    due: Instant,
    after: Duration,
    generation: u64,
    fiber: Weak<FiberInner>,
}

struct Watches {
    pending: Mutex<Vec<Watch>>,
    changed: Condvar,
}

fn watches() -> &'static Arc<Watches> {
    static WATCHES: OnceLock<Arc<Watches>> = OnceLock::new();
    WATCHES.get_or_init(|| {
        let watches = Arc::new(Watches {
            pending: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        });
        let keeper = watches.clone();
        let started = std::thread::Builder::new()
            .name("rutis-waiting".into())
            .spawn(move || keep(&keeper));
        if let Err(error) = started {
            eprintln!("cannot report plugins that wait long: {error}");
        }
        watches
    })
}

/// The thread: sleep until the next wait is due, then check each due one
/// on its fiber's runtime.
fn keep(watches: &Watches) {
    let mut pending = watches.pending.lock().unwrap();
    loop {
        let now = Instant::now();
        let (due, later): (Vec<Watch>, Vec<Watch>) = std::mem::take(&mut *pending)
            .into_iter()
            .partition(|watch| watch.due <= now);
        *pending = later;
        if !due.is_empty() {
            drop(pending);
            for watch in due {
                if let Some(fiber) = watch.fiber.upgrade() {
                    let handle = crate::host::CtxHost::handle(&fiber.ctx).clone();
                    handle.spawn(async move { report(&fiber, watch.generation, watch.after) });
                }
            }
            pending = watches.pending.lock().unwrap();
            continue;
        }
        pending = match pending.iter().map(|watch| watch.due).min() {
            None => watches.changed.wait(pending).unwrap(),
            Some(next) => {
                watches
                    .changed
                    .wait_timeout(pending, next.saturating_duration_since(now))
                    .unwrap()
                    .0
            }
        };
    }
}

/// `fiber` waits for services nothing provides, in its generation
/// `generation`: report it once its root's delay has passed, if it still
/// waits then.
pub(crate) fn watch(fiber: &Arc<FiberInner>, generation: u64) {
    // rutis's own plugins (a link waiting for its peer, a runtime's rows)
    // are not reported: their state says what they wait for, and their
    // names mean nothing to the developer (#678).
    if fiber.platform {
        return;
    }
    let Some(after) = *fiber.ctx.shared().waiting_report.lock().unwrap() else {
        return;
    };
    let watches = watches();
    {
        let mut pending = watches.pending.lock().unwrap();
        // A fiber that is gone (its instance closed, say) is never
        // reported, but its watch keeps the fiber's memory until it is due.
        // Drop those now and then, so plugins started and stopped many
        // times within the delay do not hold their memory that long.
        if pending.len() % PURGE_EVERY == PURGE_EVERY - 1 {
            pending.retain(|watch| watch.fiber.strong_count() > 0);
        }
        pending.push(Watch {
            due: Instant::now() + after,
            after,
            generation,
            fiber: Arc::downgrade(fiber),
        });
    }
    watches.changed.notify_one();
}

/// How many watches may be added between two purges of those whose fiber
/// is gone.
const PURGE_EVERY: usize = 32;

/// How reports name keys: by the names a namer gives them (a loader's
/// services, a row's runtime), else described.
pub(crate) fn key_names(shared: &crate::ctx::Shared) -> impl Fn(&crate::TypeKey) -> String {
    let namers = namers(shared);
    move |key| {
        namers
            .iter()
            .find_map(|namer| namer(key))
            .unwrap_or_else(|| key.describe())
    }
}

/// The key namers added to the root (`CtxHost::name_keys_with`), still
/// held by whoever added them.
pub(crate) fn namers(shared: &crate::ctx::Shared) -> Vec<Arc<crate::host::KeyNamer>> {
    shared
        .key_namers
        .lock()
        .unwrap()
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .collect()
}

/// Report `fiber` to its error sink when it still waits in `generation`
/// for services nothing provides.
fn report(fiber: &Arc<FiberInner>, generation: u64, after: Duration) {
    let shared = fiber.ctx.shared();
    if fiber.closing.load(std::sync::atomic::Ordering::SeqCst)
        || shared.closing.load(std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    {
        let transition = fiber.transition.lock().unwrap();
        if transition.generation != generation || transition.state != FiberState::Pending {
            return;
        }
    }
    let (_, missing) = fiber.resolve_deps();
    if missing.is_empty() {
        return;
    }
    let name = key_names(shared);
    let error = ErrorKind::StillWaiting {
        plugin: fiber.display_name(),
        seconds: after.as_secs_f64(),
        missing: missing.iter().map(name).collect(),
    };
    (shared.error_sink)(Arc::new(error));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::host::CtxHost;
    use crate::{Ctx, Plugin};

    struct Missing;

    struct Waiter;

    impl Plugin for Waiter {
        type Deps = (Arc<Missing>,);

        async fn apply(&self, _: &Ctx, _: Self::Deps) -> crate::Result {
            Ok(())
        }
    }

    /// Plugins that wait and go, many times within the delay: the watches
    /// of those gone are dropped as new ones come instead of when they are
    /// due, which would hold each fiber's memory that long (seen as the
    /// memory of tenants created and closed over and over, 0.17.2).
    #[tokio::test(flavor = "multi_thread")]
    async fn the_watches_of_fibers_gone_do_not_pile_up() {
        let root = Ctx::root().unwrap();
        root.report_waiting_after(Some(Duration::from_secs(600)));
        for _ in 0..300 {
            let view = root.plugin(Waiter);
            (&view).await.unwrap();
            view.dispose().await.unwrap();
        }
        let gone = super::watches()
            .pending
            .lock()
            .unwrap()
            .iter()
            .filter(|watch| watch.fiber.strong_count() == 0)
            .count();
        assert!(
            gone < 100,
            "{gone} watches of fibers that are gone are held"
        );
        root.shutdown().await.unwrap();
    }
}
