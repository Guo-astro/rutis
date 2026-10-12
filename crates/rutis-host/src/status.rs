//! What the host prints: each row's state as it changes, and the state
//! of each peer row's link as it changes (`docs/guide/rutis-host.md`,
//! "What `run` prints").

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rutis::FiberState;

use crate::outln;
use rutis_bridge::{Failure, LinkState};
use rutis_loader::{EntryStatus, Loader, PeerResolver};

/// One line for a row's state.
pub fn describe(status: &EntryStatus) -> String {
    match status {
        EntryStatus::Disabled => "disabled".into(),
        EntryStatus::Inactive => "inactive".into(),
        EntryStatus::Unresolved(error) => format!("unresolved: {error}"),
        EntryStatus::Stopped => "stopped in its instance".into(),
        EntryStatus::Running(snapshot) => match snapshot.state {
            FiberState::Pending => "waiting for its services".into(),
            FiberState::Loading => "starting".into(),
            FiberState::Active => "running".into(),
            FiberState::Failed => match &snapshot.error {
                Some(error) => format!("failed: {error}"),
                None => "failed".into(),
            },
            FiberState::Disposed => "stopped".into(),
            _ => format!("{:?}", snapshot.state).to_lowercase(),
        },
        status => format!("{status:?}").to_lowercase(),
    }
}

/// The category of a link failure, as printed: from [`Failure`], never
/// from the error's text.
pub fn category(failure: Failure) -> &'static str {
    match failure {
        Failure::Retryable => "unreachable",
        Failure::AuthRejected => "auth-rejected",
        Failure::Incompatible => "incompatible",
    }
}

/// A wait as printed: whole seconds as `30s`, otherwise to a tenth,
/// `2.1s`.
fn seconds(wait: Duration) -> String {
    let tenths = (wait.as_millis() + 50) / 100;
    match tenths % 10 {
        0 => format!("{}s", tenths / 10),
        tenth => format!("{}.{tenth}s", tenths / 10),
    }
}

/// One line for a link's state, after `was_ready` (whether the state
/// last seen was `Ready`); `None` for states not printed (not started,
/// dialing, a listener waiting for its first connection). The detail in
/// parentheses is the error the link reports, which names no credentials:
/// the transports never put a token in a reason.
pub fn describe_link(state: &LinkState, was_ready: bool) -> Option<String> {
    let retry = |wait: &Duration| match wait.is_zero() {
        // A listening link does not retry: the far end connects again.
        true => String::new(),
        false => format!("; retrying in {}", seconds(*wait)),
    };
    Some(match state {
        LinkState::Idle => return None,
        // A listening link whose session ended waits for the far end.
        LinkState::Connecting if was_ready => "link down (the session ended)".into(),
        LinkState::Connecting => return None,
        LinkState::Ready { .. } => "link connected".into(),
        LinkState::Waiting {
            failure: Failure::Retryable,
            error,
            retry_in,
        } => format!("link down ({error}){}", retry(retry_in)),
        LinkState::Waiting {
            failure,
            error,
            retry_in,
        } => format!(
            "link refused: {} ({error}){}",
            category(*failure),
            retry(retry_in)
        ),
        LinkState::Stopped {
            failure: Failure::Retryable,
            error,
        } => format!("link stopped ({error})"),
        LinkState::Stopped { failure, error } => {
            format!("link stopped: {} ({error})", category(*failure))
        }
    })
}

/// What [`follow`] has printed, to print only changes.
#[derive(Default)]
pub struct Shown {
    rows: BTreeMap<String, String>,
    /// Per peer row: whether its link was last seen ready, and the last
    /// line printed for it.
    links: BTreeMap<String, (bool, Option<String>)>,
}

impl Shown {
    /// The lines for what changed since the last call: rows' states, then
    /// links' states.
    pub fn changes(&mut self, loader: &Loader, peers: &PeerResolver) -> Vec<String> {
        let mut lines = Vec::new();
        let mut rows = BTreeMap::new();
        let mut links = BTreeMap::new();
        for entry in loader.entries() {
            let key = match &entry.instance {
                Some(instance) => format!("{} in {:?}", entry.id, instance.plugin),
                None => entry.id.clone(),
            };
            let link = match (&entry.status, &entry.view) {
                (EntryStatus::Running(_), Some(view)) => peers.link_state(view.instance()),
                _ => None,
            };
            if let Some(link) = link {
                let state = link.borrow().clone();
                let (was_ready, printed) = self.links.remove(&key).unwrap_or_default();
                let line = describe_link(&state, was_ready);
                let printed = match line {
                    Some(line) if printed.as_ref() != Some(&line) => {
                        lines.push((key.clone(), line.clone()));
                        Some(line)
                    }
                    _ => printed,
                };
                let ready = matches!(state, LinkState::Ready { .. });
                links.insert(key.clone(), (ready, printed));
            }
            rows.insert(key, describe(&entry.status));
        }
        let mut out = Vec::new();
        for (id, line) in &rows {
            if self.rows.get(id) != Some(line) {
                out.push(format!("{id}: {line}"));
            }
        }
        for id in self.rows.keys().filter(|id| !rows.contains_key(*id)) {
            out.push(format!("{id}: removed"));
        }
        out.extend(lines.into_iter().map(|(id, line)| format!("{id}: {line}")));
        self.rows = rows;
        self.links = links;
        out
    }
}

/// Print every change of a row's state, and of a peer row's link, for as
/// long as the host runs. Checked every 200 ms: a state that lasts less
/// may not be printed.
pub fn follow(loader: Loader, peers: Arc<PeerResolver>) {
    tokio::spawn(async move {
        let mut shown = Shown::default();
        loop {
            for line in shown.changes(&loader, &peers) {
                outln!("{line}");
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
}

/// The lines `run` and `dev` print once the host started: each listener's
/// bound address.
pub fn listening(listening: &[(String, std::net::SocketAddr)]) -> Vec<String> {
    listening
        .iter()
        .map(|(name, address)| format!("rutis-host: listening on {name} {address}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(failure: Failure, error: &str, retry_in: Duration) -> LinkState {
        LinkState::Waiting {
            failure,
            error: error.into(),
            retry_in,
        }
    }

    /// guarantee: host/link-state-output
    #[test]
    fn each_link_state_has_its_line() {
        let cases = [
            (LinkState::Idle, false, None),
            (LinkState::Connecting, false, None),
            (
                LinkState::Connecting,
                true,
                Some("link down (the session ended)"),
            ),
            (LinkState::Ready { generation: 3 }, false, Some("link connected")),
            (
                waiting(
                    Failure::Retryable,
                    "127.0.0.1:1: refused",
                    Duration::from_millis(2_140),
                ),
                true,
                Some("link down (127.0.0.1:1: refused); retrying in 2.1s"),
            ),
            (
                waiting(
                    Failure::AuthRejected,
                    "gpu: upgrade refused with 401 Unauthorized",
                    Duration::from_secs(30),
                ),
                false,
                Some("link refused: auth-rejected (gpu: upgrade refused with 401 Unauthorized); retrying in 30s"),
            ),
            (
                waiting(Failure::AuthRejected, "identity mismatch", Duration::ZERO),
                false,
                Some("link refused: auth-rejected (identity mismatch)"),
            ),
            (
                waiting(Failure::Retryable, "did not greet", Duration::ZERO),
                false,
                Some("link down (did not greet)"),
            ),
            (
                LinkState::Stopped {
                    failure: Failure::Incompatible,
                    error: "gpu does not declare runtime".into(),
                },
                false,
                Some("link stopped: incompatible (gpu does not declare runtime)"),
            ),
            (
                LinkState::Stopped {
                    failure: Failure::Retryable,
                    error: "the process exited".into(),
                },
                true,
                Some("link stopped (the process exited)"),
            ),
        ];
        for (state, was_ready, line) in cases {
            assert_eq!(
                describe_link(&state, was_ready).as_deref(),
                line,
                "{state:?}"
            );
        }
    }

    #[test]
    fn waits_are_printed_in_whole_seconds_or_tenths() {
        assert_eq!(seconds(Duration::from_secs(30)), "30s");
        assert_eq!(seconds(Duration::from_millis(29_990)), "30s");
        assert_eq!(seconds(Duration::from_millis(480)), "0.5s");
        assert_eq!(seconds(Duration::from_millis(2_140)), "2.1s");
    }

    #[test]
    fn each_listener_is_printed_with_its_address() {
        let listening = [("public".to_owned(), "127.0.0.1:7443".parse().unwrap())];
        assert_eq!(
            super::listening(&listening),
            ["rutis-host: listening on public 127.0.0.1:7443"]
        );
    }
}
