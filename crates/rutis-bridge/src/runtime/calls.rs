//! The calls into a runtime's rows that have not returned yet
//! ([`Process::calls`](crate::runtime::Process::calls)), for a status
//! snapshot: who calls which row's service, which method, since when. Only
//! recorded; a call is never changed by it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A call that has not returned yet.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InFlight {
    /// The calling row; `None` for a call that no row made (from Rust, from
    /// a peer node, or from a runtime outside any row).
    pub from: Option<String>,
    /// The row whose service is called, when it is known.
    pub to: Option<String>,
    pub service: String,
    pub method: String,
    /// How long ago it was made.
    pub elapsed: Duration,
}

struct Recorded {
    from: Option<String>,
    to: Option<String>,
    service: String,
    method: String,
    since: Instant,
}

/// The calls of one runtime process.
#[derive(Default)]
pub(crate) struct Calls {
    next: AtomicU64,
    live: Mutex<BTreeMap<u64, Recorded>>,
}

/// Removes its call once dropped: when the call returned, or its future
/// was dropped (the caller gave up).
pub(crate) struct Ongoing {
    calls: Arc<Calls>,
    number: u64,
}

impl Drop for Ongoing {
    fn drop(&mut self) {
        self.calls.live.lock().unwrap().remove(&self.number);
    }
}

impl Calls {
    /// Record a call from `from` to `to`'s `service.method`, for as long as
    /// the returned guard lives.
    pub(crate) fn begin(
        self: &Arc<Self>,
        from: Option<String>,
        to: Option<String>,
        service: &str,
        method: &str,
    ) -> Ongoing {
        let number = self.next.fetch_add(1, Ordering::Relaxed);
        self.live.lock().unwrap().insert(
            number,
            Recorded {
                from,
                to,
                service: service.to_owned(),
                method: method.to_owned(),
                since: Instant::now(),
            },
        );
        Ongoing {
            calls: self.clone(),
            number,
        }
    }

    /// The calls not returned yet, oldest first.
    pub(crate) fn snapshot(&self) -> Vec<InFlight> {
        let now = Instant::now();
        self.live
            .lock()
            .unwrap()
            .values()
            .map(|call| InFlight {
                from: call.from.clone(),
                to: call.to.clone(),
                service: call.service.clone(),
                method: call.method.clone(),
                elapsed: now.saturating_duration_since(call.since),
            })
            .collect()
    }
}

/// The host's own services' calls that have not returned yet (#937): a call
/// a runtime makes into a service the host provides itself (a
/// `#[rutis::service]` type, a service the embedding registered) never
/// passes a [`Process`](crate::runtime::Process), so nothing else records
/// it.
static HOST_CALLS: std::sync::OnceLock<Arc<Calls>> = std::sync::OnceLock::new();

fn host_registry() -> &'static Arc<Calls> {
    HOST_CALLS.get_or_init(Arc::default)
}

/// Record a call of `method` on the host's service `service`, from the row
/// `from` (`None`: no row, from Rust), for as long as the guard lives.
pub(crate) fn begin_host(from: Option<String>, service: &str, method: &str) -> Ongoing {
    host_registry().begin(from, None, service, method)
}

/// The calls into the host's own services that have not returned yet, oldest
/// first (`Loader::status` merges them with each process's).
pub fn host_calls() -> Vec<InFlight> {
    host_registry().snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// guarantee: loader/status
    /// A call is listed while its guard lives, oldest first, and gone once
    /// the guard is dropped.
    #[test]
    fn a_call_is_listed_until_it_returns() {
        let calls = Arc::new(Calls::default());
        let first = calls.begin(
            Some("chat".into()),
            Some("weather".into()),
            "weather",
            "tomorrow",
        );
        let second = calls.begin(None, Some("weather".into()), "weather", "today");
        let listed = calls.snapshot();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].from.as_deref(), Some("chat"));
        assert_eq!(listed[0].method, "tomorrow");
        assert_eq!(listed[1].from, None);
        drop(first);
        assert_eq!(calls.snapshot().len(), 1);
        drop(second);
        assert!(calls.snapshot().is_empty());
    }
}
