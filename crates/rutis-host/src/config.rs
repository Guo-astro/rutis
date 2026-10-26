//! `rutis.json`: what a host runs.
//!
//! ```json
//! {
//!   "format": 1,
//!   "id": "main",
//!   "runtimes": {
//!     "node": { "project": "." },
//!     "py": { "project": ".", "python": ".venv/bin/python", "restart": "on-failure" },
//!     "go": { "dir": "plugins/go", "start": "on-demand", "idle": 60 },
//!     "swift": { "dir": "plugins/swift", "restart": "on-failure" }
//!   },
//!   "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
//!   "rows": [
//!     { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" } },
//!     { "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis" } }
//!   ]
//! }
//! ```
//!
//! Paths are relative to the file. Rows are rutis-loader rows; every service
//! name crosses between languages and nodes by name. A field the host does
//! not know is an error; in a row, a warning (the row runs without it), so
//! a misspelt one is not dropped without a word.
//!
//! `format` is the file's format, 1 when absent (no file before 0.10 has
//! one). A file of a newer format than this host reads is refused before
//! anything else in it is read, saying what to upgrade: its other fields
//! may mean what this host does not know.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::languages::LANGUAGES;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// The file's format (1 when absent); at most [`FORMAT`].
    #[serde(default = "default_format", deserialize_with = "format")]
    pub format: u64,
    /// This node's endpoint id, which linked peers see.
    #[serde(default = "default_id")]
    pub id: String,
    #[serde(default)]
    pub runtimes: Runtimes,
    /// WebSocket listeners links can register on (`"listen": "<name>"` in a
    /// `rutis-bridge/peer` row).
    #[serde(default)]
    pub listen: Vec<Listener>,
    #[serde(default)]
    pub rows: Vec<Value>,
    /// What reading the file found wrong but went on without: the host
    /// shows these before it starts.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

fn default_id() -> String {
    "host".into()
}

/// The newest `rutis.json` format this host reads. Formats start at 1.
pub const FORMAT: u64 = 1;

fn default_format() -> u64 {
    1
}

/// `format`, refused when it is not one this host reads.
fn format<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let format = u64::deserialize(deserializer)?;
    check_format(format).map_err(serde::de::Error::custom)?;
    Ok(format)
}

fn check_format(format: u64) -> Result<(), String> {
    match format {
        0 => Err(format!(
            "format 0 is not a rutis.json format: formats start at 1, and this rutis-host reads up to {FORMAT}"
        )),
        format if format > FORMAT => Err(format!(
            "this file is format {format}, this rutis-host reads up to {FORMAT}: upgrade rutis-host"
        )),
        _ => Ok(()),
    }
}

/// The file's `format`, checked before the rest of the file: a newer
/// format's other fields are unknown here, and the error to show is the
/// format's. Anything else wrong (not JSON, a `format` that is not a
/// whole number) is left to the full read, which says where.
fn check_newer_format(text: &str) -> Result<(), String> {
    #[derive(Deserialize)]
    struct Format {
        format: Option<Value>,
    }
    match serde_json::from_str::<Format>(text) {
        Ok(Format {
            format: Some(Value::Number(format)),
        }) => format.as_u64().map_or(Ok(()), check_format),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtimes {
    pub node: Option<NodeRuntime>,
    pub py: Option<PythonRuntime>,
    pub bun: Option<BunRuntime>,
    /// Go binaries, one runtime each, started when a row uses them.
    pub go: Option<GoRuntime>,
    /// Swift binaries (macOS), one runtime each, started when a row uses
    /// them: the same settings as Go's.
    pub swift: Option<SwiftRuntime>,
    /// Runtimes on other machines: reached through a `rutis-bridge/peer`
    /// row with `"runtime": "<name>"`, or dialing in to a listener, each
    /// connection an instance of a group (`listen` and `instances`).
    #[serde(default)]
    pub remote: Vec<RemoteRuntime>,
    /// The local runtimes (by key) that rows enabled with their defaults,
    /// the file not declaring them ([`HostConfig::enable_used_runtimes`]):
    /// one that cannot start is a warning, its rows fail, the host runs.
    #[serde(skip)]
    pub implicit: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRuntime {
    pub name: String,
    /// `python`: rows `<name>:<module>`; `go`: rows `<name>:<plugin>`;
    /// `node`: npm names, resolved there when no runtime before it has them.
    pub language: String,
    /// The listener (of `listen`) its runtimes dial in to, with the token
    /// `RUTIS_TOKEN_<NAME>` (else `RUTIS_TOKEN`) and an id of their own.
    /// Goes with `instances`.
    #[serde(default)]
    pub listen: Option<String>,
    /// The top-level `instanced` group each connection is an instance of:
    /// its rows `<name>:<plugin>` run on that connection. Goes with
    /// `listen`.
    #[serde(default)]
    pub instances: Option<String>,
}

impl RemoteRuntime {
    /// The listener and group of a runtime that dials in, `None` for one
    /// reached through a link; one without the other is an error.
    pub fn dials_in(&self) -> Result<Option<(&str, &str)>, String> {
        let name = &self.name;
        match (&self.listen, &self.instances) {
            (Some(listen), Some(instances)) => Ok(Some((listen, instances))),
            (None, None) => Ok(None),
            (Some(_), None) => Err(format!(
                "remote runtime {name}: listen needs instances: name the instanced group \
                 each connection is an instance of"
            )),
            (None, Some(_)) => Err(format!(
                "remote runtime {name}: instances needs listen: name the listener its \
                 runtimes dial in to"
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRuntime {
    /// The Node project plugins are installed in (its package.json).
    #[serde(default = "here")]
    pub project: PathBuf,
    /// The `@arcships/rutis-runtime` package; by default the project's.
    #[serde(default)]
    pub runtime: Option<PathBuf>,
    /// What the host does when the runtime's process ends on its own
    /// (`no`, the default; `on-failure`; `exit`).
    #[serde(default)]
    pub restart: Restart,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BunRuntime {
    /// The project plugins are installed in (its package.json and
    /// node_modules); rows `bun:<module>` resolve from here.
    #[serde(default = "here")]
    pub project: PathBuf,
    /// The `@arcships/rutis-bun` package; by default the project's.
    #[serde(default)]
    pub runtime: Option<PathBuf>,
    /// The Bun executable; by default `bun` on `PATH`.
    #[serde(default)]
    pub program: Option<PathBuf>,
    /// What the host does when the runtime's process ends on its own
    /// (`no`, the default; `on-failure`; `exit`).
    #[serde(default)]
    pub restart: Restart,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonRuntime {
    /// Where plugin modules that are not installed are found.
    #[serde(default = "here")]
    pub project: PathBuf,
    /// The interpreter; by default `$VIRTUAL_ENV/bin/python`, then the
    /// project's `.venv/bin/python`, then `python3` (on Windows,
    /// `Scripts\python.exe` in those environments, then `python`).
    #[serde(default)]
    pub python: Option<PathBuf>,
    /// What the host does when the runtime's process ends on its own
    /// (`no`, the default; `on-failure`; `exit`).
    #[serde(default)]
    pub restart: Restart,
}

/// `runtimes.go` and `runtimes.swift`: the binaries of a compiled language,
/// one runtime each.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryRuntime {
    /// Every binary of the language in this directory (trusted as a whole:
    /// its files that contain the SDK's marker are run to read their
    /// manifests).
    #[serde(default)]
    pub dir: Option<PathBuf>,
    /// Binaries, each a runtime named after its file (`go-<name>`,
    /// `swift-<name>`).
    #[serde(default)]
    pub binaries: Vec<PathBuf>,
    /// `on-demand` (the default): a runtime starts when a row uses it;
    /// `eager`: every one starts with the host and keeps running.
    #[serde(default)]
    pub start: BinaryStart,
    /// Seconds an on-demand runtime stays idle before it stops (60).
    #[serde(default)]
    pub idle: Option<u64>,
    /// The directory the runtimes run in.
    #[serde(default = "here")]
    pub project: PathBuf,
    /// What the host does when a runtime's process ends on its own (`no`,
    /// the default; `on-failure`; `exit`), each runtime on its own.
    #[serde(default)]
    pub restart: Restart,
    /// A development build and its runtime's name (`rutis-host dev`).
    #[serde(skip)]
    pub dev: Option<(PathBuf, String)>,
}

/// `runtimes.go`: Go binaries.
pub type GoRuntime = BinaryRuntime;

/// `runtimes.swift`: Swift binaries.
pub type SwiftRuntime = BinaryRuntime;

/// What the host does when a local runtime's process ends on its own
/// (`runtimes.<node|py|bun|go|swift>.restart`; `docs/guide/rutis-host.md`, "When
/// a runtime ends").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    /// Nothing: its rows wait, the host runs on.
    #[default]
    No,
    /// Start it again after a backoff, up to a limit
    /// ([`crate::restart::Backoff`]).
    OnFailure,
    /// Stop the host, which exits 3, for its process manager to restart.
    Exit,
}

impl From<Restart> for rutis_loader::runtime::binary::RestartPolicy {
    fn from(restart: Restart) -> Self {
        match restart {
            Restart::No => Self::No,
            Restart::OnFailure => Self::OnFailure,
            Restart::Exit => Self::Exit,
        }
    }
}

impl Restart {
    /// As `rutis.json` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Restart::No => "no",
            Restart::OnFailure => "on-failure",
            Restart::Exit => "exit",
        }
    }
}

/// When a compiled language's runtimes start (`runtimes.<go|swift>.start`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BinaryStart {
    #[default]
    OnDemand,
    Eager,
}

/// `runtimes.go.start`.
pub type GoStart = BinaryStart;

impl BinaryRuntime {
    /// Refuse settings that cannot work together, as `runtimes.<key>`.
    pub fn validate(&self, key: &str) -> Result<(), String> {
        if self.dir.is_none() && self.binaries.is_empty() && self.dev.is_none() {
            return Err(format!("runtimes.{key} needs a dir or binaries"));
        }
        if self.start == BinaryStart::Eager && self.idle.is_some() {
            return Err(format!(
                "runtimes.{key}: eager runtimes keep running, so idle does not apply to them"
            ));
        }
        Ok(())
    }
}

/// The interpreter of the virtual environment `venv`.
pub fn venv_python(venv: &Path) -> PathBuf {
    match cfg!(windows) {
        true => venv.join("Scripts").join("python.exe"),
        false => venv.join("bin").join("python"),
    }
}

/// The interpreter used when no environment names one.
pub const DEFAULT_PYTHON: &str = if cfg!(windows) { "python" } else { "python3" };

/// `path` as a `file:` URL (percent-encoded, with a drive letter on
/// Windows); a relative path, which has none, as `file://<path>`.
pub fn file_url(path: &Path) -> String {
    match url::Url::from_file_path(path) {
        Ok(url) => url.into(),
        Err(()) => format!("file://{}", path.display()),
    }
}

fn here() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listener {
    pub name: String,
    pub address: SocketAddr,
    /// TLS certificate and key (PEM); `RUTIS_CERT` / `RUTIS_KEY` otherwise.
    #[serde(default)]
    pub cert: Option<PathBuf>,
    #[serde(default)]
    pub key: Option<PathBuf>,
}

impl HostConfig {
    /// Read `path`; relative paths in it become relative to its directory.
    pub fn read(path: &Path) -> Result<Self, String> {
        Self::parse(&read_text(path)?, path)
    }

    /// The configuration `text`, read from `path` ([`HostConfig::read`]).
    pub fn parse(text: &str, path: &Path) -> Result<Self, String> {
        check_newer_format(text).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut config: Self =
            serde_json::from_str(text).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut ignored = Vec::new();
        check_rows(&config.rows, "rows", &mut ignored)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        ignored.extend(config.enable_used_runtimes());
        config.warnings = ignored
            .into_iter()
            .map(|warning| format!("{}: {warning}", path.display()))
            .collect();
        let base = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        config.rebase(base);
        // A language whose runtime this machine cannot run is refused now,
        // not once its binary starts: Swift's binaries run on macOS only.
        for language in LANGUAGES {
            if language.configured(&config.runtimes) {
                language
                    .available()
                    .map_err(|error| format!("{}: {error}", path.display()))?;
            }
        }
        Ok(config)
    }

    /// Enable, with its defaults, each local runtime that enabled rows use
    /// but `runtimes` does not declare (a `py:` row without `runtimes.py`),
    /// as if it declared `"py": {}` (#666); one line each, saying so.
    /// Before [`HostConfig::rebase`]: the defaults are relative to the file.
    pub fn enable_used_runtimes(&mut self) -> Vec<String> {
        let mut names = Vec::new();
        rows_named(&self.rows, &mut names);
        let mut said = Vec::new();
        for language in LANGUAGES {
            let key = language.key();
            // A remote runtime of that name takes its rows.
            let remote =
                self.runtimes.remote.iter().any(|remote| {
                    remote.name == key || (key == "node" && remote.language == "node")
                });
            if language.configured(&self.runtimes) || remote {
                continue;
            }
            let rows: Vec<&str> = names
                .iter()
                .filter(|name| language.runs_by_default(name))
                .map(String::as_str)
                .collect();
            if let Some(first) = rows.first() {
                language.enable_default(&mut self.runtimes);
                self.runtimes.implicit.push(key.to_owned());
                said.push(format!(
                    "rows use the {key} runtime ({first}{}), which runtimes does not declare: \
                     it runs with its defaults; declare \"runtimes\": {{ \"{key}\": {{}} }} to say so",
                    match rows.len() {
                        1 => String::new(),
                        n => format!(" and {} more", n - 1),
                    }
                ));
            }
        }
        said
    }

    /// Make the relative paths in this configuration relative to `base`.
    /// `base` is made absolute first, so a plugin file's row is a `file:` URL
    /// even when `base` was relative to the working directory (`rutis-host
    /// run rutis.json`, the usual way): `file://./plugin.mjs` names no file.
    pub fn rebase(&mut self, base: &Path) {
        // `Path::parent` of a bare file name is empty: the working directory.
        let base = match base.as_os_str().is_empty() {
            true => Path::new("."),
            false => base,
        };
        let base = &std::path::absolute(base).unwrap_or_else(|_| base.to_owned());
        let at = |path: &mut PathBuf| {
            if path.is_relative() {
                // Without the `.` components (`<base>/.` for the default
                // project), which errors would show.
                *path = base.join(&*path).components().collect();
            }
        };
        for language in LANGUAGES {
            language.rebase(&mut self.runtimes, &at);
        }
        for listener in &mut self.listen {
            for path in [&mut listener.cert, &mut listener.key]
                .into_iter()
                .flatten()
            {
                at(path);
            }
        }
        // A plugin file named relative to the configuration. `Url` percent-
        // encodes spaces and the like, which a bare `display()` would not.
        for row in &mut self.rows {
            if let Some(name) = row["name"].as_str() {
                let relative = ["./", "../"]
                    .iter()
                    .chain(if cfg!(windows) {
                        &[".\\", "..\\"][..]
                    } else {
                        &[]
                    })
                    .any(|prefix| name.starts_with(prefix));
                if relative {
                    row["name"] = json!(file_url(&base.join(name)));
                }
            }
        }
    }

    /// Add `other`'s runtimes (where this has none), listeners and rows.
    pub fn merge(&mut self, mut other: HostConfig) {
        for language in LANGUAGES {
            let taken = !language.configured(&self.runtimes);
            language.merge(&mut self.runtimes, &mut other.runtimes);
            // Taken as rows of the other enabled it: still implicit.
            let key = language.key();
            if taken
                && other
                    .runtimes
                    .implicit
                    .iter()
                    .any(|implicit| implicit == key)
            {
                self.runtimes.implicit.push(key.to_owned());
            }
        }
        self.runtimes.remote.extend(other.runtimes.remote);
        self.listen.extend(other.listen);
        self.rows.extend(other.rows);
        self.warnings.extend(other.warnings);
    }

    /// Each runtime's name, which is also the prefix of its rows
    /// (`py:<module>`, `bun:<module>`, `<remote>:<module>`), names one
    /// runtime only.
    pub fn check_runtime_names(&self) -> Result<(), String> {
        let mut seen: Vec<&str> = Vec::new();
        for remote in &self.runtimes.remote {
            let name = remote.name.as_str();
            if !rutis_loader::internal::is_runtime_name(name) {
                return Err(format!(
                    "remote runtime {name:?}: a runtime name is two or more of a-z, 0-9 and -, and not file"
                ));
            }
            // A local language's own (`py`), or its rows' prefix (`go-<binary>`).
            let local = LANGUAGES
                .iter()
                .filter(|language| language.configured(&self.runtimes))
                .find_map(|language| language.claims(name));
            if let Some(error) = local {
                return Err(error);
            }
            if seen.contains(&name) {
                return Err(crate::languages::taken(name));
            }
            seen.push(name);
        }
        Ok(())
    }

    /// Each runtime that dials in names a listener of `listen` and a
    /// top-level `instanced` group of `rows`.
    pub fn check_dialing_runtimes(&self) -> Result<(), String> {
        for remote in &self.runtimes.remote {
            let Some((listen, group)) = remote.dials_in()? else {
                continue;
            };
            let name = &remote.name;
            if !self.listen.iter().any(|listener| listener.name == listen) {
                return Err(format!(
                    "remote runtime {name}: no listener named {listen}: add it to listen"
                ));
            }
            let row = self.rows.iter().find(|row| row["id"] == group);
            let Some(row) = row else {
                return Err(format!(
                    "remote runtime {name}: no top-level row {group}: instances names a group \
                     with \"instanced\": true"
                ));
            };
            if row["group"] != true || row["instanced"] != true {
                return Err(format!(
                    "remote runtime {name}: row {group} is not an instanced group: give it \
                     \"group\": true and \"instanced\": true"
                ));
            }
        }
        Ok(())
    }

    /// The rows as the loader takes them: `rutis-bridge/peer` rows get the
    /// transport their address needs and this host's identity.
    pub fn rows(&self) -> Vec<Value> {
        self.rows
            .iter()
            .map(|row| {
                let mut row = row.clone();
                if row["name"] == "rutis-bridge/peer" {
                    let config = &mut row["config"];
                    if config.is_null() {
                        *config = json!({});
                    }
                    if config.get("transport").is_none() {
                        let dial = config["dial"].as_str().unwrap_or("");
                        let network = dial.starts_with("ws://")
                            || dial.starts_with("wss://")
                            || config.get("listen").is_some();
                        config["transport"] = json!(if network { "websocket" } else { "local" });
                    }
                    if config.get("identity").is_none() {
                        config["identity"] = json!("host");
                    }
                }
                row
            })
            .collect()
    }

    /// The peers rows link to: (peer, dials).
    pub fn peers(&self) -> Vec<(String, bool)> {
        self.rows
            .iter()
            .filter(|row| row["name"] == "rutis-bridge/peer")
            .filter_map(|row| {
                let peer = row["config"]["peer"].as_str()?.to_owned();
                Some((peer, row["config"].get("dial").is_some()))
            })
            .collect()
    }
}

/// The text of the configuration file `path`.
pub fn read_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!(
            "{}: {error}: name the configuration, as `rutis-host run path/to/rutis.json`",
            path.display()
        ),
        _ => format!("{}: {error}", path.display()),
    })
}

/// The fields of a row (rutis-loader's; `config` holds a group's rows).
const ROW_FIELDS: &[&str] = &[
    "id",
    "name",
    "config",
    "inject",
    "isolate",
    "disabled",
    "group",
    "instanced",
];

/// Each row field the loader does not know, in `rows` and in groups, as a
/// warning in `ignored` (the loader ignores it); a row that is not an
/// object is an error.
fn check_rows(rows: &[Value], at: &str, ignored: &mut Vec<String>) -> Result<(), String> {
    for (index, row) in rows.iter().enumerate() {
        let Some(fields) = row.as_object() else {
            return Err(format!("{at}[{index}]: a row is an object"));
        };
        let place = match row["id"].as_str() {
            Some(id) => format!("{at}[{index}] (id {id:?})"),
            None => format!("{at}[{index}]"),
        };
        for unknown in fields
            .keys()
            .filter(|key| !ROW_FIELDS.contains(&key.as_str()))
        {
            ignored.push(format!(
                "{place}: unknown field `{unknown}` is ignored; a row has {}",
                ROW_FIELDS
                    .iter()
                    .map(|field| format!("`{field}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // A 0.12 link's `events: { out, in }`: refused here, so `check`
        // says so, not only `run` once the loader builds the link.
        if row["name"] == "rutis-bridge/peer" && row["config"].get("events").is_some() {
            return Err(format!(
                "{place}: {}",
                rutis_loader::internal::REMOVED_EVENTS
            ));
        }
        let group = !matches!(
            fields.get("group"),
            None | Some(Value::Null | Value::Bool(false))
        );
        if let (true, Some(children)) = (group, row["config"].as_array()) {
            check_rows(children, &format!("{place}.config"), ignored)?;
        }
    }
    Ok(())
}

/// The names of the rows in `rows` and the groups in it that are not
/// disabled (`"disabled": true`), in order.
fn rows_named(rows: &[Value], names: &mut Vec<String>) {
    for row in rows {
        if row["disabled"] == true {
            continue;
        }
        if let Some(name) = row["name"].as_str() {
            names.push(name.to_owned());
        }
        let group = !matches!(
            row.get("group"),
            None | Some(Value::Null | Value::Bool(false))
        );
        if let (true, Some(children)) = (group, row["config"].as_array()) {
            rows_named(children, names);
        }
    }
}

/// The token presented to, or accepted from, `peer`:
/// `RUTIS_TOKEN_<PEER>` (upper case, `-` as `_`), else `RUTIS_TOKEN`.
pub fn token(peer: &str) -> Option<String> {
    let specific = format!("RUTIS_TOKEN_{}", peer.to_uppercase().replace('-', "_"));
    std::env::var(specific)
        .or_else(|_| std::env::var("RUTIS_TOKEN"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_rows_get_their_transport_and_identity() {
        let config: HostConfig = serde_json::from_value(json!({
            "rows": [
                { "id": "a", "name": "rutis-bridge/peer", "config": { "peer": "a", "dial": "wss://a/rutis" } },
                { "id": "b", "name": "rutis-bridge/peer", "config": { "peer": "b", "listen": "public" } },
                { "id": "c", "name": "rutis-bridge/peer", "config": { "peer": "c", "dial": "/tmp/c.sock" } },
                { "id": "w", "name": "weather" }
            ]
        }))
        .unwrap();
        let rows = config.rows();
        assert_eq!(rows[0]["config"]["transport"], "websocket");
        assert_eq!(rows[1]["config"]["transport"], "websocket");
        assert_eq!(rows[2]["config"]["transport"], "local");
        assert_eq!(rows[0]["config"]["identity"], "host");
        assert_eq!(rows[3], json!({ "id": "w", "name": "weather" }));
        assert_eq!(
            config.peers(),
            vec![("a".into(), true), ("b".into(), false), ("c".into(), true)]
        );
    }

    /// guarantee: host/config-paths-and-defaults
    #[test]
    #[cfg(unix)]
    fn paths_are_relative_to_the_file() {
        let mut config: HostConfig = serde_json::from_value(json!({
            "runtimes": { "node": {}, "py": { "python": ".venv/bin/python" } }
        }))
        .unwrap();
        config.rows = vec![
            json!({ "id": "llm", "name": "./dev/fake.ts" }),
            json!({ "id": "w", "name": "weather" }),
        ];
        config.rebase(Path::new("/srv/app"));
        assert_eq!(config.rows[0]["name"], "file:///srv/app/dev/fake.ts");
        assert_eq!(config.rows[1]["name"], "weather");
        assert_eq!(
            config.runtimes.node.unwrap().project,
            Path::new("/srv/app/.")
        );
        assert_eq!(
            config.runtimes.py.unwrap().python.unwrap(),
            Path::new("/srv/app/.venv/bin/python")
        );
        assert!(serde_json::from_value::<HostConfig>(json!({ "unknown": 1 })).is_err());
    }

    /// Rows of a language whose runtime `runtimes` does not declare enable
    /// it with its defaults, saying so once (#666); a declared runtime, a
    /// remote one of that name, a disabled row or an npm name do not.
    /// guarantee: host/config-paths-and-defaults
    #[test]
    #[cfg(unix)]
    fn rows_enable_the_runtime_they_use() {
        let text = json!({
            "rows": [
                { "id": "llm", "name": "py:llmbox" },
                { "id": "g", "group": true, "config": [
                    { "id": "tool", "name": "py:tool" },
                    { "id": "ts", "name": "./plugins/fake.ts" },
                ] },
                { "id": "off", "name": "bun:off", "disabled": true },
                { "id": "w", "name": "weather" },
            ]
        })
        .to_string();
        let config = HostConfig::parse(&text, Path::new("/srv/app/rutis.json")).unwrap();
        assert_eq!(
            config.runtimes.py.as_ref().unwrap().project,
            Path::new("/srv/app/.")
        );
        assert!(config.runtimes.node.is_some());
        assert!(config.runtimes.bun.is_none());
        assert_eq!(
            config.warnings,
            [
                "/srv/app/rutis.json: rows use the node runtime (./plugins/fake.ts), which runtimes does not declare: it runs with its defaults; declare \"runtimes\": { \"node\": {} } to say so",
                "/srv/app/rutis.json: rows use the py runtime (py:llmbox and 1 more), which runtimes does not declare: it runs with its defaults; declare \"runtimes\": { \"py\": {} } to say so",
            ]
        );
        // Declared, or a remote runtime's: nothing added, nothing said.
        let declared = json!({
            "runtimes": { "py": { "python": "/usr/bin/python3" }, "remote": [{ "name": "bun", "language": "bun" }] },
            "rows": [{ "id": "a", "name": "py:a" }, { "id": "b", "name": "bun:b" }],
        })
        .to_string();
        let config = HostConfig::parse(&declared, Path::new("/srv/app/rutis.json")).unwrap();
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert!(config.runtimes.bun.is_none());
        assert!(config.runtimes.implicit.is_empty());
        assert_eq!(
            config.runtimes.py.unwrap().python.unwrap(),
            Path::new("/usr/bin/python3")
        );
        // A `bun:` row enables Bun; what rows enable is recorded as such.
        let bun = json!({ "rows": [{ "id": "b", "name": "bun:b" }] }).to_string();
        let config = HostConfig::parse(&bun, Path::new("/srv/app/rutis.json")).unwrap();
        assert_eq!(
            config.runtimes.bun.as_ref().unwrap().project,
            Path::new("/srv/app/.")
        );
        assert_eq!(config.runtimes.implicit, ["bun"]);
    }

    #[test]
    fn go_runtimes_are_checked() {
        let parse =
            |value: serde_json::Value| -> GoRuntime { serde_json::from_value(value).unwrap() };
        assert!(parse(json!({ "dir": "plugins/go" })).validate("go").is_ok());
        assert!(
            parse(json!({ "binaries": ["bin/netkit"], "start": "eager" }))
                .validate("go")
                .is_ok()
        );
        let none = parse(json!({})).validate("swift").unwrap_err();
        assert!(
            none.contains("runtimes.swift needs a dir or binaries"),
            "{none}"
        );
        let both = parse(json!({ "dir": "d", "start": "eager", "idle": 30 }))
            .validate("go")
            .unwrap_err();
        assert!(both.contains("idle does not apply"), "{both}");
        let mut config: HostConfig = serde_json::from_value(json!({
            "runtimes": { "go": { "dir": "plugins/go", "binaries": ["bin/netkit"] } }
        }))
        .unwrap();
        // Absolute on every platform: on Windows, `/srv/host` gets a drive.
        let base = std::path::absolute("/srv/host").unwrap();
        config.rebase(&base);
        let go = config.runtimes.go.unwrap();
        assert_eq!(go.dir.unwrap(), base.join("plugins/go"));
        assert_eq!(go.binaries[0], base.join("bin/netkit"));
        assert_eq!(go.project, base.join("."));
    }
}
