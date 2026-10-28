//! The current state in one snapshot ([`Loader::status`]): each row's state
//! and why, each service and who provides it, each runtime, and the calls
//! into rows that have not returned. Only names: no internal ids, no
//! internal plugins. What `rutis-host status` prints.

use std::collections::HashMap;
use std::time::Duration;

use rutis::host::diagnostics::PluginDiagnostics;
use rutis::host::{CtxHost, FiberState, PluginId, Snapshot};

use super::waits::{key_name, own_runtime, row_of};
use super::{EntryInfo, EntryStatus, Loader, Wait, WaitReason};

/// A snapshot of the loader's rows, the services, the runtimes and the
/// calls in progress ([`Loader::status`]). Its parts are read one after
/// another, not at one instant. `Display` prints it for people, one item a
/// line.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Status {
    pub rows: Vec<Row>,
    pub services: Vec<Service>,
    pub runtimes: Vec<Runtime>,
    pub calls: Vec<Call>,
}

/// A row ([`Status::rows`]); a row inside instanced groups once per
/// instance it runs in.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Row {
    pub id: String,
    /// The instance a copy runs in: `<group> #<number>`.
    pub instance: Option<String>,
    /// The row's status, as [`Loader::entries`] gives it, except that a
    /// row waiting for its runtime is `Running` with a `Pending` snapshot.
    pub state: EntryStatus,
    /// Why it failed (`failed in apply: …`, `failed: …`), or what it waits
    /// for (`waiting for …`), as one line says it; for a stopped row, why.
    pub reason: Option<String>,
}

impl Row {
    pub fn new(
        id: impl Into<String>,
        instance: Option<&str>,
        state: EntryStatus,
        reason: Option<&str>,
    ) -> Self {
        Self {
            id: id.into(),
            instance: instance.map(str::to_owned),
            state,
            reason: reason.map(str::to_owned),
        }
    }
}

/// A service by name ([`Status::services`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Service {
    pub name: String,
    /// The row providing it; `None` when the host does (a Rust service).
    pub provider: Option<String>,
}

impl Service {
    pub fn new(name: impl Into<String>, provider: Option<&str>) -> Self {
        Self {
            name: name.into(),
            provider: provider.map(str::to_owned),
        }
    }
}

/// A runtime ([`Status::runtimes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Runtime {
    pub name: String,
    pub state: RuntimeState,
    /// Why it is down: how its process ended, or why it could not start.
    pub reason: Option<String>,
    /// Its process id: a runtime on this machine, while it runs.
    pub pid: Option<u32>,
}

/// A runtime's state in a [`Status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeState {
    Running,
    Starting,
    /// Its process ended, or it could not start.
    Down,
    /// Not running, and not meant to: stopped, or not started yet.
    Stopped,
}

/// A call into a row's service that has not returned ([`Status::calls`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Call {
    /// The calling row; `None` for the host (a call no row made).
    pub from: Option<String>,
    /// The row called, when known.
    pub to: Option<String>,
    pub service: String,
    pub method: String,
    pub elapsed: Duration,
}

impl Loader {
    /// The current state, as [`Status`] says: each row's state and why it
    /// fails or waits, each service by name and the row (or the host)
    /// providing it, each runtime the resolvers run (state, why it is
    /// down, its process id), and the calls into rows' services not
    /// returned yet, with how long they have taken. A row whose runtime
    /// is down or starting waits for it at once, whatever its plugin's
    /// state says yet. Best effort: the parts are read one after another.
    pub fn status(&self) -> Status {
        let runtimes = self.runtimes_now();
        let runtime_of = |name: &str| {
            runtimes
                .iter()
                .find(|runtime| runtime.name == name)
                .map(|runtime| (runtime.state, runtime.reason.clone()))
        };
        let entries = self.entries();
        let (instances, records, root) = {
            let state = self.inner.state.lock().unwrap();
            let instances: HashMap<PluginId, String> = state
                .instances
                .iter()
                .map(|(number, record)| (record.plugin, format!("{} #{number}", record.group)))
                .collect();
            let records: HashMap<PluginId, String> = state
                .running
                .iter()
                .map(|(slot, running)| (running.view.id, slot.row.clone()))
                .collect();
            (instances, records, state.last_root.clone())
        };
        let diagnostics = root.as_ref().map(|root| root.diagnostics());
        let fibers: HashMap<PluginId, &PluginDiagnostics> = diagnostics
            .iter()
            .flat_map(|diagnostics| diagnostics.plugins.iter())
            .map(|fiber| (fiber.id, fiber))
            .collect();
        // The runtime a row's plugin runs in: the one it waits for.
        let runtime_name = |plugin: Option<PluginId>| -> Option<String> {
            let fiber = fibers.get(&plugin?)?;
            fiber
                .injects
                .iter()
                .find(|inject| own_runtime(&inject.key))
                .and_then(|inject| inject.key.name().map(str::to_owned))
        };

        // Rows: entries that are themselves copies, or rows outside
        // instanced groups (the row itself of one inside is listed by its
        // copies).
        let listed: Vec<&EntryInfo> = entries
            .iter()
            .filter(|entry| {
                entry.instance.is_some()
                    || !entries
                        .iter()
                        .any(|other| other.id == entry.id && other.instance.is_some())
            })
            .filter(|entry| !is_group(entry))
            .collect();
        let pending = |entry: &EntryInfo| matches!(&entry.status, EntryStatus::Running(snapshot) if snapshot.state == FiberState::Pending);
        let waiting: Vec<&EntryInfo> = listed.iter().copied().filter(|e| pending(e)).collect();
        let mut waits = match waiting.is_empty() {
            true => Vec::new(),
            false => self.waits_of(&waiting),
        }
        .into_iter();
        let mut rows = Vec::new();
        for entry in listed {
            let instance = entry
                .instance
                .as_ref()
                .and_then(|instance| instances.get(&instance.plugin).cloned());
            let (state, reason) = match pending(entry) {
                true => (
                    entry.status.clone(),
                    Some(describe_waits(
                        &waits.next().unwrap_or_default(),
                        &runtime_of,
                    )),
                ),
                false => row_state(&entry.status),
            };
            // A row whose runtime is not running waits for it now, before
            // its plugin is told (#681).
            let (state, reason) = match (&state, runtime_name(entry.plugin)) {
                (EntryStatus::Running(snapshot), Some(name))
                    if matches!(snapshot.state, FiberState::Active | FiberState::Loading) =>
                {
                    match runtime_of(&name) {
                        Some((runtime @ (RuntimeState::Down | RuntimeState::Starting), why)) => (
                            EntryStatus::Running(Snapshot {
                                state: FiberState::Pending,
                                error: None,
                                ..snapshot.clone()
                            }),
                            Some(own_runtime_line(&name, Some((runtime, why)))),
                        ),
                        _ => (state, reason),
                    }
                }
                _ => (state, reason),
            };
            rows.push(Row {
                id: entry.id.clone(),
                instance,
                state,
                reason,
            });
        }

        let mut services = Vec::new();
        for binding in diagnostics.iter().flat_map(|d| d.bindings.iter()) {
            if binding.removing || own_runtime(&binding.key) {
                continue;
            }
            let Some(name) = key_name(&self.inner.catalog, &binding.key) else {
                continue;
            };
            let provider = row_of(binding.provider, &records, &fibers);
            let service = Service { name, provider };
            if !services.contains(&service) {
                services.push(service);
            }
        }
        services.sort_by(|a, b| a.name.cmp(&b.name).then(a.provider.cmp(&b.provider)));

        Status {
            rows,
            services,
            calls: self.calls_now(),
            runtimes,
        }
    }

    /// The runtime each copy of `entries` (of [`Loader::entries`]) runs
    /// in, by name, in order, from one diagnostics snapshot; `None` for a
    /// row that runs in no runtime (a Rust plugin) or has no fiber.
    pub fn runtimes_of(&self, entries: &[&EntryInfo]) -> Vec<Option<String>> {
        let root = self.inner.state.lock().unwrap().last_root.clone();
        let (Some(root), true) = (root, entries.iter().any(|entry| entry.plugin.is_some())) else {
            return vec![None; entries.len()];
        };
        let diagnostics = root.diagnostics();
        let fibers: HashMap<PluginId, &PluginDiagnostics> = diagnostics
            .plugins
            .iter()
            .map(|fiber| (fiber.id, fiber))
            .collect();
        entries
            .iter()
            .map(|entry| {
                let fiber = fibers.get(&entry.plugin?)?;
                fiber
                    .injects
                    .iter()
                    .find(|inject| own_runtime(&inject.key))
                    .and_then(|inject| inject.key.name().map(str::to_owned))
            })
            .collect()
    }

    /// Each runtime the resolvers run, with its state now.
    fn runtimes_now(&self) -> Vec<Runtime> {
        #[cfg(feature = "runtimes")]
        {
            use rutis_bridge::runtime::RuntimeState as Bridge;
            let mut runtimes: Vec<Runtime> = self
                .inner
                .resolver
                .runtimes()
                .into_iter()
                .map(|handle| {
                    let (state, reason) = match handle.state() {
                        Bridge::Ready(_) => (RuntimeState::Running, None),
                        Bridge::Starting => (RuntimeState::Starting, None),
                        Bridge::Down(why) => {
                            (RuntimeState::Down, Some(how_it_ended(&why).to_owned()))
                        }
                        Bridge::Idle => (RuntimeState::Stopped, None),
                    };
                    Runtime {
                        name: handle.name().to_owned(),
                        state,
                        reason,
                        pid: handle.pid(),
                    }
                })
                .collect();
            runtimes.sort_by(|a, b| a.name.cmp(&b.name));
            runtimes.dedup_by(|a, b| a.name == b.name);
            runtimes
        }
        #[cfg(not(feature = "runtimes"))]
        Vec::new()
    }

    /// The calls into rows of the resolvers' running runtimes, oldest first.
    fn calls_now(&self) -> Vec<Call> {
        #[cfg(feature = "runtimes")]
        {
            let mut calls: Vec<Call> = self
                .inner
                .resolver
                .runtimes()
                .into_iter()
                .filter_map(|handle| match handle.state() {
                    rutis_bridge::runtime::RuntimeState::Ready(process) => Some(process.calls()),
                    _ => None,
                })
                .flatten()
                .map(|call| Call {
                    from: call.from,
                    to: call.to,
                    service: call.service,
                    method: call.method,
                    elapsed: call.elapsed,
                })
                .collect();
            calls.sort_by_key(|call| std::cmp::Reverse(call.elapsed));
            calls
        }
        #[cfg(not(feature = "runtimes"))]
        Vec::new()
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// A group row: listed by the rows inside it.
fn is_group(entry: &EntryInfo) -> bool {
    !matches!(
        entry.options.get("group"),
        None | Some(serde_json::Value::Null | serde_json::Value::Bool(false))
    )
}

/// A row's reason from its status, when it does not wait.
fn row_state(status: &EntryStatus) -> (EntryStatus, Option<String>) {
    let reason = match status {
        EntryStatus::Inactive => Some("a group around it is not running".into()),
        EntryStatus::Unresolved(error) => {
            Some(format!("unresolved: {}", first_line(&error.to_string())))
        }
        EntryStatus::Stopped => Some("stopped in its instance".into()),
        EntryStatus::Running(snapshot) if snapshot.state == FiberState::Failed => {
            snapshot.error.as_ref().map(|error| failure(error))
        }
        _ => None,
    };
    (status.clone(), reason)
}

/// The word a row's status prints as: `waiting`, `starting`, `running`,
/// `failed`, `stopped` or `disabled`.
fn state_word(status: &EntryStatus) -> &'static str {
    match status {
        EntryStatus::Disabled => "disabled",
        EntryStatus::Unresolved(_) => "failed",
        EntryStatus::Running(snapshot) => match snapshot.state {
            FiberState::Pending => "waiting",
            FiberState::Loading => "starting",
            FiberState::Active => "running",
            FiberState::Failed => "failed",
            _ => "stopped",
        },
        _ => "stopped",
    }
}

/// A row's failure in one line: what failed (`failed in apply: …`,
/// `call to forecast.tomorrow failed: …`) when the error says, else
/// `failed: …`.
fn failure(error: &rutis::host::ErrorKind) -> String {
    #[cfg(any(feature = "runtimes", feature = "peer"))]
    let said = match rutis_bridge::report::headline(error) {
        Some(_) => rutis_bridge::report::report(error),
        None => format!("failed: {}", rutis_bridge::report::report(error)),
    };
    #[cfg(not(any(feature = "runtimes", feature = "peer")))]
    let said = format!("failed: {error}");
    said.lines().next().unwrap_or_default().to_owned()
}

/// The state of the runtime named, and why it is down; `None` for one not
/// known (a remote one): what [`describe_waits`] asks.
pub type RuntimeLookup<'a> = dyn Fn(&str) -> Option<(RuntimeState, Option<String>)> + 'a;

/// What a waiting row's line says: `waiting for runtime <name> (…)` while
/// its own runtime is not there ([`own_runtime_line`]); otherwise
/// `waiting for <service> (<why>)`, one per service it waits for, joined
/// by `; `; `waiting for its services` when rutis cannot say which.
/// `runtime(name)`: the state of the runtime `name` and why it is down,
/// `None` for one not known (a remote one).
pub fn describe_waits(waits: &[Wait], runtime: &RuntimeLookup<'_>) -> String {
    if let Some(own) = waits
        .iter()
        .find(|wait| matches!(wait.reason, WaitReason::OwnRuntime))
    {
        return own_runtime_line(&own.service, runtime(&own.service));
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
/// one not known, a remote one): starting, down and how its process ended,
/// stopped, or running while its rows are resolved again.
pub fn own_runtime_line(name: &str, state: Option<(RuntimeState, Option<String>)>) -> String {
    match state {
        Some((RuntimeState::Starting, _)) => format!("waiting for runtime {name} (starting)"),
        Some((RuntimeState::Down, why)) => format!(
            "waiting for runtime {name} (down: {})",
            how_it_ended(why.as_deref().unwrap_or("it ended"))
        ),
        Some((RuntimeState::Stopped, _)) => format!("waiting for runtime {name} (stopped)"),
        _ => format!("waiting for runtime {name}"),
    }
}

/// Why a service a row waits for is not there, in a few words.
fn why(reason: &WaitReason, runtime: &RuntimeLookup<'_>) -> String {
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
            Some((RuntimeState::Down, _)) => format!("row {row}'s runtime {name} is down"),
            Some(_) => format!("row {row}'s runtime {name} is not running yet"),
            // One not known (a remote one): its state unknown.
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

/// A duration as a status prints it: `850ms` under a second, `42s` under a
/// minute, then `3m05s`.
fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0 => format!("{}ms", duration.as_millis()),
        1..=59 => format!("{seconds}s"),
        _ => format!("{}m{:02}s", seconds / 60, seconds % 60),
    }
}

impl std::fmt::Display for RuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RuntimeState::Running => "running",
            RuntimeState::Starting => "starting",
            RuntimeState::Down => "down",
            RuntimeState::Stopped => "stopped",
        })
    }
}

/// `rows:`, `services:`, `runtimes:`, `calls:`, each item on a line of its
/// own below its heading (a part with nothing says `none`):
///
/// ```text
/// rows:
///   chat: running
///   weather: waiting for llm (row model failed: …)
/// services:
///   llm: model
///   tools: host
/// runtimes:
///   py: running (pid 4242)
/// calls:
///   chat → weather.tomorrow  42s
/// ```
impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "rows:")?;
        if self.rows.is_empty() {
            writeln!(f, "  none")?;
        }
        for row in &self.rows {
            let name = match &row.instance {
                Some(instance) => format!("{} in {instance}", row.id),
                None => row.id.clone(),
            };
            match (state_word(&row.state), &row.reason) {
                // A waiting or failed row's reason says so itself.
                ("waiting" | "failed", Some(reason)) => writeln!(f, "  {name}: {reason}")?,
                (state, Some(reason)) => writeln!(f, "  {name}: {state} ({reason})")?,
                (state, None) => writeln!(f, "  {name}: {state}")?,
            }
        }
        writeln!(f, "services:")?;
        if self.services.is_empty() {
            writeln!(f, "  none")?;
        }
        for service in &self.services {
            let provider = service.provider.as_deref().unwrap_or("host");
            writeln!(f, "  {}: {provider}", service.name)?;
        }
        writeln!(f, "runtimes:")?;
        if self.runtimes.is_empty() {
            writeln!(f, "  none")?;
        }
        for runtime in &self.runtimes {
            let mut detail = Vec::new();
            if let Some(reason) = &runtime.reason {
                detail.push(reason.clone());
            }
            if let Some(pid) = runtime.pid {
                detail.push(format!("pid {pid}"));
            }
            match detail.is_empty() {
                true => writeln!(f, "  {}: {}", runtime.name, runtime.state)?,
                false => writeln!(
                    f,
                    "  {}: {} ({})",
                    runtime.name,
                    runtime.state,
                    detail.join(", ")
                )?,
            }
        }
        writeln!(f, "calls:")?;
        if self.calls.is_empty() {
            writeln!(f, "  none")?;
        }
        for call in &self.calls {
            writeln!(f, "  {call}")?;
        }
        Ok(())
    }
}

/// As data, for plugins of other languages (the service `loader`'s
/// `status()`): `{rows: [{id, instance?, state, reason?}], services:
/// [{name, provider}], runtimes: [{name, state, reason?, pid?}], calls:
/// [{from, to, service, method, ms}]}`. A state is the word `Display`
/// prints (`running`, `waiting`, `starting`, `failed`, `disabled`,
/// `stopped`); a `provider` or `from` of `null` is the host.
impl serde::Serialize for Status {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde_json::{json, Map, Value};
        let some = |object: &mut Map<String, Value>, key: &str, value: Option<Value>| {
            if let Some(value) = value {
                object.insert(key.into(), value);
            }
        };
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|row| {
                let mut object = Map::new();
                object.insert("id".into(), json!(row.id));
                some(
                    &mut object,
                    "instance",
                    row.instance.as_ref().map(|i| json!(i)),
                );
                object.insert("state".into(), json!(state_word(&row.state)));
                some(&mut object, "reason", row.reason.as_ref().map(|r| json!(r)));
                Value::Object(object)
            })
            .collect();
        let services: Vec<Value> = self
            .services
            .iter()
            .map(|service| json!({ "name": service.name, "provider": service.provider }))
            .collect();
        let runtimes: Vec<Value> = self
            .runtimes
            .iter()
            .map(|runtime| {
                let mut object = Map::new();
                object.insert("name".into(), json!(runtime.name));
                object.insert("state".into(), json!(runtime.state.to_string()));
                some(
                    &mut object,
                    "reason",
                    runtime.reason.as_ref().map(|r| json!(r)),
                );
                some(&mut object, "pid", runtime.pid.map(|pid| json!(pid)));
                Value::Object(object)
            })
            .collect();
        let calls: Vec<Value> = self
            .calls
            .iter()
            .map(|call| {
                json!({
                    "from": call.from,
                    "to": call.to,
                    "service": call.service,
                    "method": call.method,
                    "ms": call.elapsed.as_millis() as u64,
                })
            })
            .collect();
        json!({ "rows": rows, "services": services, "runtimes": runtimes, "calls": calls })
            .serialize(serializer)
    }
}

/// `chat → forecast.tomorrow (row weather)  42s`: the caller (`host` for
/// none), the service and method, the row called when its name is not the
/// service's, how long it has taken.
impl std::fmt::Display for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let from = self.from.as_deref().unwrap_or("host");
        write!(f, "{from} → {}.{}", self.service, self.method)?;
        if let Some(to) = self.to.as_deref().filter(|to| *to != self.service) {
            write!(f, " (row {to})")?;
        }
        write!(f, "  {}", elapsed(self.elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(state: FiberState) -> EntryStatus {
        EntryStatus::Running(Snapshot {
            generation: 1,
            state,
            error: None,
        })
    }

    /// One item a line under each heading; a waiting row's line is its
    /// reason; a call names the row called when it is not the service's
    /// name; empty parts say `none`.
    #[test]
    fn a_status_prints_one_item_a_line() {
        let status = Status {
            rows: vec![
                Row {
                    id: "chat".into(),
                    instance: None,
                    state: running(FiberState::Active),
                    reason: None,
                },
                Row {
                    id: "weather".into(),
                    instance: None,
                    state: running(FiberState::Pending),
                    reason: Some("waiting for runtime py (down: exited with status 1)".into()),
                },
                Row {
                    id: "contacts".into(),
                    instance: Some("phone #1".into()),
                    state: running(FiberState::Failed),
                    reason: Some("failed in apply: KeyError: 'region'".into()),
                },
            ],
            services: vec![
                Service {
                    name: "forecast".into(),
                    provider: Some("weather".into()),
                },
                Service {
                    name: "tools".into(),
                    provider: None,
                },
            ],
            runtimes: vec![
                Runtime {
                    name: "node".into(),
                    state: RuntimeState::Running,
                    reason: None,
                    pid: Some(4242),
                },
                Runtime {
                    name: "py".into(),
                    state: RuntimeState::Down,
                    reason: Some("exited with status 1".into()),
                    pid: None,
                },
            ],
            calls: vec![Call {
                from: Some("chat".into()),
                to: Some("weather".into()),
                service: "forecast".into(),
                method: "tomorrow".into(),
                elapsed: Duration::from_secs(42),
            }],
        };
        assert_eq!(
            status.to_string(),
            "rows:\n  \
             chat: running\n  \
             weather: waiting for runtime py (down: exited with status 1)\n  \
             contacts in phone #1: failed in apply: KeyError: 'region'\n\
             services:\n  \
             forecast: weather\n  \
             tools: host\n\
             runtimes:\n  \
             node: running (pid 4242)\n  \
             py: down (exited with status 1)\n\
             calls:\n  \
             chat → forecast.tomorrow (row weather)  42s\n"
        );
        assert_eq!(
            Status::default().to_string(),
            "rows:\n  none\nservices:\n  none\nruntimes:\n  none\ncalls:\n  none\n"
        );
    }

    #[test]
    fn elapsed_times_read_as_people_say_them() {
        assert_eq!(elapsed(Duration::from_millis(850)), "850ms");
        assert_eq!(elapsed(Duration::from_secs(42)), "42s");
        assert_eq!(elapsed(Duration::from_secs(185)), "3m05s");
    }
}
