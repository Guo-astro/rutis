//! The host's limit on calls into one runtime's plugin code
//! (`docs/design-host-plugin-limits-2026-10-27.md` §4.2): a call by name
//! (`RowService`) or of a function passed by reference that has not
//! finished within it is cancelled and fails with code `timeout`; with
//! `kill_on_timeout` the runtime's process is ended too. Counted from the
//! call being sent to its end, a returned future's settling included.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;

/// Run once a call ran out of time ([`CallLimit::ended`]).
pub(crate) type Ended = Arc<dyn Fn(&str) + Send + Sync>;

/// What a host set for calls into one runtime ([`Connection::limit_calls`]).
pub(crate) struct CallLimit {
    /// The runtime's name, as the host named it.
    pub(crate) runtime: String,
    pub(crate) limit: Duration,
    /// Run once a call ran out of time, with what ran out (`a function of
    /// wordcount did not finish within 30s`), before the call fails: ends
    /// the runtime's process when the host said so.
    pub(crate) ended: Option<Ended>,
}

/// The call that ran out of time.
#[derive(Clone)]
pub(crate) enum Called {
    /// `service.method`, called by name.
    Service(String, String),
    /// A function passed by reference.
    Function,
}

/// When a call into a limited runtime must have ended, carried on the
/// future it returned so awaiting that future counts against the same
/// limit.
#[derive(Clone)]
pub(crate) struct Deadline {
    pub(super) at: Instant,
    pub(super) limit: Arc<CallLimit>,
    pub(super) called: Called,
}

impl CallLimit {
    /// The deadline of a call starting now.
    pub(crate) fn deadline(self: &Arc<Self>, called: Called) -> Deadline {
        Deadline {
            // A limit too far away to count is no deadline at all.
            at: Instant::now()
                .checked_add(self.limit)
                .unwrap_or_else(far_future),
            limit: self.clone(),
            called,
        }
    }

    /// `called` ran out of time: the error its caller gets, after ending
    /// the process when the host said so.
    pub(crate) fn ran_out(&self, called: &Called) -> Error {
        let what = match called {
            Called::Service(service, method) => format!("call to {service}.{method}"),
            Called::Function => format!("a function of {}", self.runtime),
        };
        let within = format!("{what} did not finish within {}", spoken(self.limit));
        let mut message = format!(
            "{within} (the host's limit for runtime {}); the call was cancelled",
            self.runtime
        );
        if let Some(ended) = &self.ended {
            ended(&within);
            message.push_str(&format!("; runtime {} was ended", self.runtime));
        }
        let mut data = serde_json::json!({
            "runtime": self.runtime,
            "limit": u64::try_from(self.limit.as_millis()).unwrap_or(u64::MAX),
        });
        if let Called::Service(service, method) = called {
            data["service"] = Json::from(service.as_str());
            data["method"] = Json::from(method.as_str());
        }
        Error::binding(crate::session::code::TIMEOUT, message, Some(data)).graphed()
    }
}

impl Deadline {
    /// Run `call` to its end or this deadline, whichever comes first:
    /// dropped at the deadline, it is cancelled.
    pub(crate) async fn bound<F: Future<Output = Reply>>(&self, call: F) -> Reply {
        let at = tokio::time::Instant::from_std(self.at);
        match tokio::time::timeout_at(at, call).await {
            Ok(reply) => self.carry(reply),
            Err(_) => Err(self.limit.ran_out(&self.called)),
        }
    }

    /// A future the reply holds settles within the same deadline.
    pub(crate) fn carry(&self, reply: Reply) -> Reply {
        if let Ok(Value::Reference(Reference(ReferenceInner::Remote(import)))) = &reply {
            if import.kind == Kind::Future {
                let _ = import.deadline.set(self.clone());
            }
        }
        reply
    }
}

/// `30s`, `1500ms`.
fn spoken(limit: Duration) -> String {
    if limit.subsec_nanos() == 0 && limit.as_secs() > 0 {
        format!("{}s", limit.as_secs())
    } else {
        format!("{}ms", limit.as_millis())
    }
}

/// An instant no call reaches: about thirty years from now.
fn far_future() -> Instant {
    Instant::now() + Duration::from_secs(86_400 * 365 * 30)
}

impl Connection {
    /// Limit the calls into the far end's plugin code (a runtime's): by
    /// name ([`RowService`](crate::runtime::RowService)) and of the
    /// functions it passed by reference. Set once, when the runtime starts.
    pub(crate) fn limit_calls(&self, limit: CallLimit) {
        let _ = self.0.limit.set(Arc::new(limit));
    }

    /// The host's limit on calls into the far end, if it set one.
    pub(crate) fn call_limit(&self) -> Option<Arc<CallLimit>> {
        self.0.limit.get().cloned()
    }

    /// [`Connection::invoke`] within `deadline`: past it the call is
    /// cancelled and fails as [`CallLimit::ran_out`] says.
    pub(crate) fn invoke_within(
        &self,
        target: &str,
        method: &str,
        args: Value,
        deadline: &Deadline,
    ) -> Reply {
        let operation = Operation::Invoke(target.into(), method.into(), args);
        self.request_sync_within(operation, Some(deadline))
    }

    /// A synchronous request, given up at `deadline` (if any): the far end
    /// is told to cancel it and a late reply is discarded.
    pub(super) fn request_sync_within(
        &self,
        call: Operation,
        deadline: Option<&Deadline>,
    ) -> Reply {
        let Some(deadline) = deadline else {
            return self.request_sync(call);
        };
        let (sender, receiver) = mpsc::channel();
        let id = self.send(
            call,
            Waiting::Sync {
                sender,
                origins: Vec::new(),
            },
        )?;
        loop {
            let left = deadline.at.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(left) {
                Ok(Message::Reply(result)) => return deadline.carry(result),
                Ok(Message::Invoke(incoming)) => {
                    let _pumping = Pumping::enter();
                    let _runtime = LentRuntime::enter(&self.0.executor);
                    self.execute(incoming, true)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.cancel(&id);
                    return Err(deadline.limit.ran_out(&deadline.called));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(transport(mpsc::RecvError))
                }
            }
        }
    }
}

impl Reference {
    /// The deadline of a call of this function, when its runtime has a
    /// limit; of an await of this future, the one its call set.
    pub(super) fn deadline(&self, connection: &Connection) -> Option<Deadline> {
        let ReferenceInner::Remote(import) = &self.0 else {
            return None;
        };
        match import.kind {
            Kind::Function => connection
                .call_limit()
                .map(|limit| limit.deadline(Called::Function)),
            Kind::Future => import.deadline.get().cloned(),
            _ => None,
        }
    }
}

impl Import {
    /// As [`Reference::deadline`], for a relayed call or await.
    pub(super) fn relayed_deadline(&self, target: &Connection, call: bool) -> Option<Deadline> {
        match (self.kind, call) {
            (Kind::Function, true) => target
                .call_limit()
                .map(|limit| limit.deadline(Called::Function)),
            (Kind::Future, false) => self.deadline.get().cloned(),
            _ => None,
        }
    }
}
