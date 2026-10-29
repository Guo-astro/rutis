//! What the host prints: each row's state as it changes, and a local
//! runtime that went down or came back (`docs/guide/rutis-host.md`, "What
//! `run` prints"). Connection rows print their own lines (`connect`).

use std::collections::BTreeMap;
use std::time::Duration;

use rutis::host::FiberState;

use crate::errln;
use rutis_bridge::runtime::{RuntimeHandle, RuntimeState};
use rutis_loader::report::{EntryInfo, EntryStatus, Wait};
use rutis_loader::Loader;

use crate::languages::BinaryHost;

/// One line for a row's state.
pub fn describe(status: &EntryStatus) -> String {
    match status {
        EntryStatus::Disabled => "disabled".into(),
        EntryStatus::Inactive => "inactive".into(),
        // A config that does not match its schema: `config: …`, a line
        // per problem.
        EntryStatus::Unresolved(error) if error.config().is_some() => error.to_string(),
        EntryStatus::Unresolved(error) => format!("unresolved: {error}"),
        EntryStatus::Stopped => "stopped in its instance".into(),
        EntryStatus::Running(snapshot) => match snapshot.state {
            FiberState::Pending => "waiting for its services".into(),
            FiberState::Loading => "starting".into(),
            FiberState::Active => "running".into(),
            // A headline already says what failed (`failed in apply: …`,
            // `call to forecast.tomorrow failed: …`).
            FiberState::Failed => match &snapshot.error {
                Some(error) if rutis::ConfigError::of(error).is_some() => error.to_string(),
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

/// What a waiting row's line says ([`rutis_loader::status::describe_waits`]),
/// `runtime(name)` giving the state of the local runtime `name`.
pub fn describe_waits(waits: &[Wait], runtime: &dyn Fn(&str) -> Option<RuntimeState>) -> String {
    rutis_loader::status::describe_waits(waits, &|name| runtime(name).as_ref().map(phase))
}

/// A row waiting for its own runtime `name`, in the state `state`
/// ([`rutis_loader::status::own_runtime_line`]).
pub fn own_runtime_line(name: &str, state: Option<&RuntimeState>) -> String {
    rutis_loader::status::own_runtime_line(name, state.map(phase))
}

/// A runtime's state as the loader's status says it, and why it is down.
fn phase(state: &RuntimeState) -> (rutis_loader::status::RuntimeState, Option<String>) {
    use rutis_loader::status::RuntimeState as Phase;
    match state {
        RuntimeState::Ready(_) => (Phase::Running, None),
        RuntimeState::Starting => (Phase::Starting, None),
        RuntimeState::Down(why) => (Phase::Down, Some(why.clone())),
        RuntimeState::Idle => (Phase::Stopped, None),
    }
}

pub use rutis_loader::status::how_it_ended;

/// A wait as printed: whole seconds as `30s`, otherwise to a tenth,
/// `2.1s`.
pub(crate) fn seconds(wait: Duration) -> String {
    let tenths = (wait.as_millis() + 50) / 100;
    match tenths % 10 {
        0 => format!("{}s", tenths / 10),
        tenth => format!("{}.{tenth}s", tenths / 10),
    }
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

/// The connection rows' part of `rutis-host status`, `connections:` with a
/// line per row, the far ends connected and the remote plugins each
/// registered; nothing when there is no connection row:
///
/// ```text
/// connections:
///   gpu: connected (gpu:embedder)
///   phone: phone-1, phone-2
/// ```
pub fn connections(rows: &[crate::connect::Row]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let far = |connected: &crate::connect::Connected, name: &str| match connected.plugins.is_empty()
    {
        true => name.to_owned(),
        false => format!("{name} ({})", connected.plugins.join(", ")),
    };
    let mut out = String::from("connections:\n");
    for row in rows {
        let line = match (row.dials, row.connected.as_slice()) {
            (true, []) => "disconnected".to_owned(),
            (true, [connected, ..]) => far(connected, "connected"),
            (false, []) => "none connected".to_owned(),
            (false, connected) => connected
                .iter()
                .map(|connected| far(connected, &connected.id))
                .collect::<Vec<_>>()
                .join(", "),
        };
        out.push_str(&format!("  {}: {line}\n", row.id));
    }
    out
}

/// What [`follow`] watches: the rows and the runtimes.
#[derive(Clone)]
pub struct Followed {
    pub loader: Loader,
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
    /// Per runtime: the last line printed for it, and how many times it
    /// had started then.
    runtimes: BTreeMap<String, (String, u64)>,
}

impl Shown {
    /// The lines for what changed since the last call: runtimes' states,
    /// then rows' states.
    pub fn changes(&mut self, followed: &Followed) -> Vec<String> {
        let states = followed.runtime_states();
        let mut out = self.runtime_changes(&states);
        let runtime = |name: &str| {
            states
                .iter()
                .find(|(runtime, _, _)| runtime == name)
                .map(|(_, state, _)| state.clone())
        };
        // Only then is it worth asking which runtime each row runs in.
        let any_away = states
            .iter()
            .any(|(_, state, _)| matches!(state, RuntimeState::Down(_) | RuntimeState::Starting));
        out.extend(self.row_changes(&followed.loader, &runtime, any_away));
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
        runtime: &dyn Fn(&str) -> Option<RuntimeState>,
        any_away: bool,
    ) -> Vec<String> {
        let mut rows = BTreeMap::new();
        let entries = loader.entries();
        // What every waiting row waits for, from one snapshot per look.
        let pending = |entry: &EntryInfo| matches!(&entry.status, EntryStatus::Running(snapshot) if snapshot.state == FiberState::Pending);
        let waiting: Vec<&EntryInfo> = entries.iter().filter(|entry| pending(entry)).collect();
        let mut waits = match waiting.is_empty() {
            true => Vec::new(),
            false => loader.waits_of(&waiting),
        }
        .into_iter();
        let mut runtimes = match any_away {
            true => loader.runtimes_of(&entries.iter().collect::<Vec<_>>()),
            false => vec![None; entries.len()],
        }
        .into_iter();
        for entry in &entries {
            let key = match &entry.instance {
                Some(instance) => format!("{} in {:?}", entry.id, instance.plugin),
                None => entry.id.clone(),
            };
            let line = match pending(entry) {
                true => describe_waits(&waits.next().unwrap_or_default(), runtime),
                false => describe(&entry.status),
            };
            // A running row whose runtime went down waits for it from now
            // on, before its plugin is told (#681).
            let line = match (&entry.status, runtimes.next().flatten()) {
                (EntryStatus::Running(snapshot), Some(name))
                    if matches!(snapshot.state, FiberState::Active | FiberState::Loading) =>
                {
                    match runtime(&name) {
                        Some(state @ (RuntimeState::Down(_) | RuntimeState::Starting)) => {
                            own_runtime_line(&name, Some(&state))
                        }
                        _ => line,
                    }
                }
                _ => line,
            };
            rows.insert(key, line);
        }
        let mut out = Vec::new();
        for (id, line) in &rows {
            if self.rows.get(id) != Some(line) {
                out.extend(prefixed(id, line));
            }
        }
        for id in self.rows.keys().filter(|id| !rows.contains_key(*id)) {
            out.push(format!("{id}: removed"));
        }
        self.rows = rows;
        out
    }
}

/// A row's line, `<id>: <line>`; each further line of a config error
/// (`config: …`, one problem each) gets the prefix too, other lines (where
/// an error came from) are left as they are.
fn prefixed(id: &str, line: &str) -> Vec<String> {
    let mut lines = line.split('\n');
    let mut out = vec![format!("{id}: {}", lines.next().unwrap_or_default())];
    for more in lines {
        match more.starts_with("config: ") {
            false => out
                .last_mut()
                .expect("a first line")
                .push_str(&format!("\n{more}")),
            true => out.push(format!("{id}: {more}")),
        }
    }
    out
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
    use rutis_loader::report::WaitReason;

    /// A config error prints one line per problem, each with the row's
    /// name (config schema design §9); where an error came from stays
    /// indented under its line.
    #[test]
    fn a_config_error_prints_a_line_per_problem() {
        let error = rutis_loader::LoaderError::Rejected {
            id: "weather".into(),
            error: std::sync::Arc::new(rutis::host::ErrorKind::PluginFailed(Box::new(
                rutis::config::check(
                    &serde_json::json!({ "properties": { "a": { "type": "integer" } } }),
                    Some(&serde_json::json!({ "a": "x" })),
                )
                .error()
                .unwrap(),
            ))),
        };
        let line = describe(&EntryStatus::Unresolved(error));
        assert_eq!(line, "config: a should be an integer, got \"x\"");
        assert_eq!(
            prefixed("weather", "config: a\nconfig: b"),
            ["weather: config: a", "weather: config: b"]
        );
        assert_eq!(
            prefixed("weather", "failed in apply: x\n    in weather:"),
            ["weather: failed in apply: x\n    in weather:"]
        );
    }

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
        ];
        for (state, line) in cases {
            assert_eq!(own_runtime_line("py", state.as_ref()), line);
        }
        // Not a local runtime: a connection row's far end.
        assert_eq!(own_runtime_line("gpu", None), "waiting for gpu");
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
