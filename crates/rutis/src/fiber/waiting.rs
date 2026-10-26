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
            eprintln!("[rutis] cannot report plugins that wait long: {error}");
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
    let Some(after) = *fiber.ctx.shared().waiting_report.lock().unwrap() else {
        return;
    };
    let watches = watches();
    watches.pending.lock().unwrap().push(Watch {
        due: Instant::now() + after,
        after,
        generation,
        fiber: Arc::downgrade(fiber),
    });
    watches.changed.notify_one();
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
    let error = ErrorKind::StillWaiting {
        plugin: fiber.name.clone(),
        seconds: after.as_secs_f64(),
        missing: missing.iter().map(|key| key.describe()).collect(),
    };
    (shared.error_sink)(Arc::new(error));
}
