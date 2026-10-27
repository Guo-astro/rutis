//! What the host prints: each row's state as it changes, the state of
//! each peer row's link as it changes, and a local runtime that went down
//! or came back (`docs/guide/rutis-host.md`, "What `run` prints").

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rutis::host::FiberState;

use crate::errln;
use rutis_bridge::runtime::{RuntimeHandle, RuntimeState};
use rutis_bridge::{Failure, LinkState};
use rutis_loader::peer::PeerResolver;
use rutis_loader::report::{EntryInfo, EntryStatus, Wait, WaitReason};
use rutis_loader::Loader;

use crate::languages::BinaryHost;

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
            // A headline already says what failed (`failed in apply: …`,
            // `call to forecast.tomorrow failed: …`).
            FiberState::Failed => match &snapshot.error {
                Some(error) if rutis_bridge::report::headline(error).is_some() => {
                    rutis_bridge::report::report(error)
                }
                Some(error) => format!("failed: {}", rutis_bridge::report::report(error)),
                None => "failed".into(),
            },
            FiberState::Disposed => "stopped".into(),
            _ => format!("{:?}", snapshot.state).to_lowercase(),
        },
        status => format!("{status:?}").to_lowercase(),
    }
}

/// What a waiting row's line says: `waiting for runtime <name> (…)` while
/// its own runtime is not there ([`own_runtime_line`]); otherwise
/// `waiting for <service> (<why>)`, one per service it waits for, joined
/// by `; `; `waiting for its services` when rutis cannot say which.
/// `runtime(name)`: the state of the local runtime `name`.
pub fn describe_waits(waits: &[Wait], runtime: &dyn Fn(&str) -> Option<RuntimeState>) -> String {
    if let Some(own) = waits
        .iter()
        .find(|wait| matches!(wait.reason, WaitReason::OwnRuntime))
    {
        return own_runtime_line(&own.service, runtime(&own.service).as_ref());
    }
    if waits.is_empty() {
        return "waiting for its services".into();
    }
    let each: Vec<String> = waits
        .iter()
        .map(|wait| format!("{} ({})", wait.service, why(&wait.reason, runtime)))
        .collect();
    format!("waiting for {}", each.join("; "))
}

/// A row waiting for its own runtime `name`, in the state `state` (`None`:
/// one the host does not run, a remote one): starting, down and how its
/// process ended, stopped, or running while its rows are resolved again.
pub fn own_runtime_line(name: &str, state: Option<&RuntimeState>) -> String {
    match state {
        Some(RuntimeState::Starting) => format!("waiting for runtime {name} (starting)"),
        Some(RuntimeState::Down(reason)) => {
            format!(
                "waiting for runtime {name} (down: {})",
                how_it_ended(reason)
            )
        }
        Some(RuntimeState::Idle) => format!("waiting for runtime {name} (stopped)"),
        _ => format!("waiting for runtime {name}"),
    }
}

/// Why a service a row waits for is not there, in a few words.
fn why(reason: &WaitReason, runtime: &dyn Fn(&str) -> Option<RuntimeState>) -> String {
    // An error's first line: the status line is one line.
    let first = |error: &str| error.lines().next().unwrap_or_default().to_owned();
    match reason {
        WaitReason::NoProvider { like: Some(like) } => {
            format!("no row provides it; did you mean {like}?")
        }
        WaitReason::NoProvider { like: None } => "no row provides it".into(),
        WaitReason::NoEnabledProvider { like } => format!(
            "no enabled row provides it{}; rutis-host check says whether a disabled row does",
            like.as_ref()
                .map(|like| format!(", did you mean {like}?"))
                .unwrap_or_default()
        ),
        WaitReason::Explained(why) => first(why),
        WaitReason::ProviderDisabled { row } => {
            format!("row {row}, which provides it, is disabled")
        }
        WaitReason::ProviderFailed { row, error } => format!("row {row} failed: {}", first(error)),
        WaitReason::ProviderInactive { row } => {
            format!("row {row}, which provides it, is not running")
        }
        WaitReason::ProviderRuntime { row, runtime: name } => match runtime(name) {
            Some(RuntimeState::Down(_)) => format!("row {row}'s runtime {name} is down"),
            Some(_) => format!("row {row}'s runtime {name} is not running yet"),
            // One the host does not run (a remote one): its state unknown.
            None => format!("row {row} waits for its runtime {name}"),
        },
        WaitReason::Cycle { row, service } => {
            format!("row {row} waits for {service} from this row: they inject each other")
        }
        WaitReason::ProviderWaiting { row, services } => {
            format!("row {row} waits too, for {}", services.join(", "))
        }
        WaitReason::ProviderStarting { row } => format!("row {row} is starting"),
        WaitReason::ProviderRunning { row } => format!("row {row} has not provided it"),
        WaitReason::Other(said) => first(said),
        _ => "not there yet".into(),
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
        // A listening link whose connection ended waits for the far end.
        LinkState::Connecting if was_ready => "link down (the connection ended)".into(),
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

/// A runtime's state, as far as its line goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime<'a> {
    Running,
    /// Its process ended, or it could not start: why.
    Down(&'a str),
    /// Starting, or stopped by the host.
    Other,
}

impl<'a> From<&'a RuntimeState> for Runtime<'a> {
    fn from(state: &'a RuntimeState) -> Self {
        match state {
            RuntimeState::Ready(_) => Runtime::Running,
            RuntimeState::Down(reason) => Runtime::Down(reason),
            _ => Runtime::Other,
        }
    }
}

/// One line for a local runtime's state, after `was_down` (whether the
/// line last printed for it was `down`); `None` for states not printed: a
/// runtime starting, running as it has since the host started, or stopped
/// by the host. The detail is how its process ended (`exited with signal:
/// 9 (SIGKILL)`, `exited with exit status: 1`), or why it did not start.
pub fn describe_runtime(state: Runtime<'_>, was_down: bool) -> Option<String> {
    match state {
        Runtime::Down(reason) => Some(format!("down ({})", how_it_ended(reason))),
        Runtime::Running if was_down => Some("running".into()),
        _ => None,
    }
}

/// How a runtime ended, from why it is down: what its plugin and its
/// channel saw says it in more words, and the process's exit is the part
/// worth a line (`exited with signal: 9 (SIGKILL)`).
pub fn how_it_ended(reason: &str) -> &str {
    ["runtime process ", "runtime ended: the process "]
        .iter()
        .find_map(|said| reason.strip_prefix(said))
        .or_else(|| reason.strip_prefix("runtime ended: "))
        .unwrap_or(reason)
}

/// What [`follow`] watches: the rows, their links and the runtimes.
#[derive(Clone)]
pub struct Followed {
    pub loader: Loader,
    pub peers: Arc<PeerResolver>,
    /// The local runtimes (`node`, `py`, `bun`) by name.
    pub runtimes: Vec<(String, RuntimeHandle)>,
    /// The compiled languages' runtimes (Go, Swift), those the
    /// configuration has.
    pub binaries: Vec<BinaryHost>,
}

impl Followed {
    /// Each local runtime's state by name, the compiled languages' among
    /// them, with a number that grows whenever it starts again.
    fn runtime_states(&self) -> Vec<(String, RuntimeState, u64)> {
        let mut states: Vec<(String, RuntimeState, u64)> = self
            .runtimes
            .iter()
            .map(|(name, handle)| (name.clone(), handle.state(), handle.starts()))
            .collect();
        for binaries in &self.binaries {
            states.extend(binaries.states_started());
        }
        states
    }
}

/// What [`follow`] has printed, to print only changes.
#[derive(Default)]
pub struct Shown {
    rows: BTreeMap<String, String>,
    /// Per peer row: whether its link was last seen ready, and the last
    /// line printed for it.
    links: BTreeMap<String, (bool, Option<String>)>,
    /// Per runtime: the last line printed for it, and how many times it
    /// had started then.
    runtimes: BTreeMap<String, (String, u64)>,
}

impl Shown {
    /// The lines for what changed since the last call: runtimes' states,
    /// then rows' states, then links' states.
    pub fn changes(&mut self, followed: &Followed) -> Vec<String> {
        let states = followed.runtime_states();
        let mut out = self.runtime_changes(&states);
        let runtime = |name: &str| {
            states
                .iter()
                .find(|(runtime, _, _)| runtime == name)
                .map(|(_, state, _)| state.clone())
        };
        out.extend(self.row_changes(&followed.loader, &followed.peers, &runtime));
        out
    }

    /// The lines for the runtimes `states` (name, state, how many times it
    /// has started) whose state changed. A runtime down again after it
    /// started again is a change, however short it ran between two looks:
    /// each time it goes down has its line. A runtime gone from `states`
    /// keeps its last line: one of Go or Swift stops when idle, and one the
    /// host stops goes on to `Idle`, neither printed.
    pub fn runtime_changes(&mut self, states: &[(String, RuntimeState, u64)]) -> Vec<String> {
        let mut out = Vec::new();
        for (name, state, starts) in states {
            let printed = self.runtimes.get(name);
            let was_down = printed.is_some_and(|(line, _)| line.starts_with("down"));
            let started_since = printed.is_some_and(|(_, then)| starts > then);
            match describe_runtime(state.into(), was_down) {
                Some(line)
                    if printed.map(|(line, _)| line) != Some(&line)
                        || (line.starts_with("down") && started_since) =>
                {
                    out.push(format!("runtime {name}: {line}"));
                    self.runtimes.insert(name.clone(), (line, *starts));
                }
                _ => {}
            }
        }
        out
    }

    fn row_changes(
        &mut self,
        loader: &Loader,
        peers: &PeerResolver,
        runtime: &dyn Fn(&str) -> Option<RuntimeState>,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let mut rows = BTreeMap::new();
        let mut links = BTreeMap::new();
        let entries = loader.entries();
        // What every waiting row waits for, from one snapshot per look.
        let pending = |entry: &EntryInfo| matches!(&entry.status, EntryStatus::Running(snapshot) if snapshot.state == FiberState::Pending);
        let waiting: Vec<&EntryInfo> = entries.iter().filter(|entry| pending(entry)).collect();
        let mut waits = match waiting.is_empty() {
            true => Vec::new(),
            false => loader.waits_of(&waiting),
        }
        .into_iter();
        for entry in &entries {
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
            let line = match pending(entry) {
                true => describe_waits(&waits.next().unwrap_or_default(), runtime),
                false => describe(&entry.status),
            };
            rows.insert(key, line);
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

/// Print every change of a row's state, of a peer row's link, and of a
/// runtime going down or coming back, for as long as the host runs.
/// Checked every 200 ms: a state that lasts less may not be printed.
pub fn follow(followed: Followed) {
    tokio::spawn(async move {
        let mut shown = Shown::default();
        loop {
            for line in shown.changes(&followed) {
                errln!("{line}");
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

    /// A failed row prints the plugin's own error, then which row and what
    /// it was doing, then the plugin's frames (#677).
    /// guarantee: host/row-failure-output
    #[test]
    fn a_failed_row_prints_where_its_error_came_from() {
        let thrown = rutis_bridge::session::Error::Remote {
            name: "KeyError".into(),
            message: "'region'".into(),
            graph: Some(serde_json::json!({
                "root": { "type": "reference", "value": 0 },
                "nodes": [{
                    "type": "error", "name": "KeyError", "message": "'region'",
                    "stack": "KeyError: 'region'\n  File \"weather.py\", line 12, in apply",
                    "site": { "row": "weather", "doing": "apply" }
                }]
            })),
        };
        let status = EntryStatus::Running(rutis::host::Snapshot {
            generation: 1,
            state: FiberState::Failed,
            error: Some(std::sync::Arc::new(rutis::host::ErrorKind::PluginFailed(
                Box::new(thrown),
            ))),
        });
        assert_eq!(
            describe(&status),
            "failed in apply: KeyError: 'region'\n    in weather:\n    File \"weather.py\", line 12, in apply"
        );
    }

    /// A row whose call failed says which call; an error with no site
    /// prints after `failed: ` (§8.1 rows 2 and 4).
    #[test]
    fn a_failed_call_is_the_state_and_an_error_without_a_site_follows_failed() {
        let failed = |graph: Option<serde_json::Value>| {
            EntryStatus::Running(rutis::host::Snapshot {
                generation: 1,
                state: FiberState::Failed,
                error: Some(std::sync::Arc::new(rutis::host::ErrorKind::PluginFailed(
                    Box::new(rutis_bridge::session::Error::Remote {
                        name: "KeyError".into(),
                        message: "'region'".into(),
                        graph,
                    }),
                ))),
            })
        };
        assert_eq!(
            describe(&failed(Some(serde_json::json!({
                "root": { "type": "reference", "value": 0 },
                "nodes": [{
                    "type": "error", "name": "KeyError", "message": "'region'",
                    "site": { "row": "weather", "doing": "call", "service": "forecast", "method": "tomorrow" }
                }]
            })))),
            "call to forecast.tomorrow failed: KeyError: 'region'\n    in weather:"
        );
        assert_eq!(describe(&failed(None)), "failed: KeyError: 'region'");
    }

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
                Some("link down (the connection ended)"),
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

    /// guarantee: host/runtime-state-lines
    #[test]
    fn a_runtime_has_a_line_when_it_goes_down_and_when_it_comes_back() {
        let killed = "runtime process exited with signal: 9 (SIGKILL)";
        let cases = [
            (Runtime::Other, false, None),
            (Runtime::Running, false, None),
            (
                Runtime::Down(killed),
                false,
                Some("down (exited with signal: 9 (SIGKILL))"),
            ),
            (
                Runtime::Down("runtime ended: the process exited with exit status: 3"),
                false,
                Some("down (exited with exit status: 3)"),
            ),
            (
                Runtime::Down("runtime ended: did not greet"),
                false,
                Some("down (did not greet)"),
            ),
            (
                Runtime::Down("the py runtime lacks rows.v2"),
                false,
                Some("down (the py runtime lacks rows.v2)"),
            ),
            (Runtime::Other, true, None),
            (Runtime::Running, true, Some("running")),
        ];
        for (state, was_down, line) in cases {
            assert_eq!(
                describe_runtime(state, was_down).as_deref(),
                line,
                "{state:?}"
            );
        }
    }

    /// Each change once: a runtime down stays one line however long it is,
    /// a restart that has not run yet prints nothing, and a runtime the
    /// host stops (idle) none.
    /// guarantee: host/runtime-state-lines
    #[test]
    fn a_runtime_line_is_printed_once_per_change() {
        let mut shown = Shown::default();
        let state = |name: &str, state: RuntimeState| vec![(name.to_owned(), state, 1)];
        assert!(shown
            .runtime_changes(&state("py", RuntimeState::Starting))
            .is_empty());
        let down = || RuntimeState::Down("runtime process exited with exit status: 1".into());
        assert_eq!(
            shown.runtime_changes(&state("py", down())),
            ["runtime py: down (exited with exit status: 1)"]
        );
        assert!(shown.runtime_changes(&state("py", down())).is_empty());
        assert!(shown
            .runtime_changes(&state("py", RuntimeState::Starting))
            .is_empty());
        assert_eq!(
            shown.runtime_changes(&state(
                "py",
                RuntimeState::Down("runtime process exited with signal: 9 (SIGKILL)".into())
            )),
            ["runtime py: down (exited with signal: 9 (SIGKILL))"]
        );
        assert!(shown
            .runtime_changes(&state("node", RuntimeState::Idle))
            .is_empty());
    }

    /// A runtime that starts again and goes down the same way between two
    /// looks (it ran less than one) has the line again: each time it goes
    /// down, once (#665).
    /// guarantee: host/runtime-state-lines
    #[test]
    fn a_runtime_down_again_after_it_started_again_has_the_line_again() {
        let mut shown = Shown::default();
        let down = |starts| {
            let why = "runtime process exited with exit status: 1".to_owned();
            vec![("node".to_owned(), RuntimeState::Down(why), starts)]
        };
        let line = ["runtime node: down (exited with exit status: 1)"];
        assert_eq!(shown.runtime_changes(&down(1)), line);
        assert!(shown.runtime_changes(&down(1)).is_empty());
        assert_eq!(shown.runtime_changes(&down(2)), line);
        assert!(shown.runtime_changes(&down(2)).is_empty());
        // Seen starting in between: no line for that, one for the end.
        assert!(shown
            .runtime_changes(&[("node".to_owned(), RuntimeState::Starting, 3)])
            .is_empty());
        assert_eq!(shown.runtime_changes(&down(3)), line);
    }

    /// A waiting row says what it waits for and why (#676).
    /// guarantee: host/row-state-lines
    #[test]
    fn a_waiting_row_says_what_it_waits_for_and_why() {
        // `py` down, `node` starting, others not the host's (remote).
        let down = |name: &str| match name {
            "py" => Some(RuntimeState::Down(
                "runtime process exited with signal: 9 (SIGKILL)".into(),
            )),
            "node" => Some(RuntimeState::Starting),
            _ => None,
        };
        let row = |row: &str| row.to_owned();
        let cases = [
            (vec![], "waiting for its services"),
            (
                vec![Wait::new(
                    "lmm",
                    WaitReason::NoProvider {
                        like: Some("llm".into()),
                    },
                )],
                "waiting for lmm (no row provides it; did you mean llm?)",
            ),
            (
                vec![
                    Wait::new(
                        "llm",
                        WaitReason::ProviderFailed {
                            row: row("model"),
                            error: "Error: boom\n    at apply".into(),
                        },
                    ),
                    Wait::new("blob", WaitReason::NoProvider { like: None }),
                ],
                "waiting for llm (row model failed: Error: boom); blob (no row provides it)",
            ),
            (
                vec![Wait::new(
                    "llm",
                    WaitReason::ProviderRuntime {
                        row: row("model"),
                        runtime: "py".into(),
                    },
                )],
                "waiting for llm (row model's runtime py is down)",
            ),
            (
                vec![Wait::new(
                    "llm",
                    WaitReason::ProviderRuntime {
                        row: row("model"),
                        runtime: "node".into(),
                    },
                )],
                "waiting for llm (row model's runtime node is not running yet)",
            ),
            (
                vec![Wait::new(
                    "llm",
                    WaitReason::ProviderRuntime {
                        row: row("model"),
                        runtime: "gpu".into(),
                    },
                )],
                "waiting for llm (row model waits for its runtime gpu)",
            ),
            (
                vec![Wait::new(
                    "bsvc",
                    WaitReason::Cycle {
                        row: row("b"),
                        service: "asvc".into(),
                    },
                )],
                "waiting for bsvc (row b waits for asvc from this row: they inject each other)",
            ),
            (
                vec![Wait::new(
                    "mail",
                    WaitReason::ProviderDisabled { row: row("mailer") },
                )],
                "waiting for mail (row mailer, which provides it, is disabled)",
            ),
            (
                vec![Wait::new(
                    "mail",
                    WaitReason::NoEnabledProvider { like: None },
                )],
                "waiting for mail (no enabled row provides it; rutis-host check says whether a disabled row does)",
            ),
            (
                vec![Wait::new(
                    "lmm",
                    WaitReason::NoEnabledProvider {
                        like: Some("llm".into()),
                    },
                )],
                "waiting for lmm (no enabled row provides it, did you mean llm?; rutis-host check says whether a disabled row does)",
            ),
            (
                vec![
                    Wait::new("llm", WaitReason::NoProvider { like: None }),
                    Wait::new("py", WaitReason::OwnRuntime),
                ],
                "waiting for runtime py (down: exited with signal: 9 (SIGKILL))",
            ),
        ];
        for (waits, line) in cases {
            assert_eq!(describe_waits(&waits, &down), line, "{waits:?}");
        }
    }

    /// A row waiting for its own runtime names it and its state.
    /// guarantee: host/row-state-lines
    #[test]
    fn a_row_waiting_for_its_runtime_names_it() {
        let cases = [
            (
                Some(RuntimeState::Starting),
                "waiting for runtime py (starting)",
            ),
            (
                Some(RuntimeState::Down(
                    "runtime ended: the process exited with exit status: 1".into(),
                )),
                "waiting for runtime py (down: exited with exit status: 1)",
            ),
            (Some(RuntimeState::Idle), "waiting for runtime py (stopped)"),
            (None, "waiting for runtime py"),
        ];
        for (state, line) in cases {
            assert_eq!(own_runtime_line("py", state.as_ref()), line);
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
