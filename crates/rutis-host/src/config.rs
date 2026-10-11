//! `rutis.json`: what a host runs.
//!
//! ```json
//! {
//!   "id": "main",
//!   "runtimes": {
//!     "node": { "project": "." },
//!     "py": { "project": ".", "python": ".venv/bin/python" },
//!     "go": { "dir": "plugins/go", "start": "on-demand", "idle": 60 }
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

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtimes {
    pub node: Option<NodeRuntime>,
    pub py: Option<PythonRuntime>,
    pub bun: Option<BunRuntime>,
    /// Go binaries, one runtime each, started when a row uses them.
    pub go: Option<GoRuntime>,
    /// Runtimes on other machines, reached through a `rutis-bridge/peer` row
    /// with `"runtime": "<name>"`.
    #[serde(default)]
    pub remote: Vec<RemoteRuntime>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRuntime {
    pub name: String,
    /// `python`: rows `<name>:<module>`; `go`: rows `<name>:<plugin>`;
    /// `node`: npm names, resolved there when no runtime before it has them.
    pub language: String,
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoRuntime {
    /// Every Go binary in this directory (trusted as a whole: its files
    /// that contain the Go SDK's marker are run to read their manifests).
    #[serde(default)]
    pub dir: Option<PathBuf>,
    /// Go binaries, each a runtime named after its file (`go-<name>`).
    #[serde(default)]
    pub binaries: Vec<PathBuf>,
    /// `on-demand` (the default): a runtime starts when a row uses it;
    /// `eager`: every one starts with the host and keeps running.
    #[serde(default)]
    pub start: GoStart,
    /// Seconds an on-demand runtime stays idle before it stops (60).
    #[serde(default)]
    pub idle: Option<u64>,
    /// The directory the runtimes run in.
    #[serde(default = "here")]
    pub project: PathBuf,
    /// A development build and its runtime's name (`rutis-host dev`).
    #[serde(skip)]
    pub dev: Option<(PathBuf, String)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GoStart {
    #[default]
    OnDemand,
    Eager,
}

impl GoRuntime {
    /// Refuse settings that cannot work together.
    pub fn validate(&self) -> Result<(), String> {
        if self.dir.is_none() && self.binaries.is_empty() && self.dev.is_none() {
            return Err("runtimes.go needs a dir or binaries".into());
        }
        if self.start == GoStart::Eager && self.idle.is_some() {
            return Err(
                "runtimes.go: eager runtimes keep running, so idle does not apply to them".into(),
            );
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
        let text = std::fs::read_to_string(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{}: {error}: name the configuration, as `rutis-host run path/to/rutis.json`",
                path.display()
            ),
            _ => format!("{}: {error}", path.display()),
        })?;
        let mut config: Self =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut ignored = Vec::new();
        check_rows(&config.rows, "rows", &mut ignored)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        config.warnings = ignored
            .into_iter()
            .map(|warning| format!("{}: {warning}", path.display()))
            .collect();
        let base = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        config.rebase(base);
        Ok(config)
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
        if let Some(node) = &mut self.runtimes.node {
            at(&mut node.project);
            if let Some(runtime) = &mut node.runtime {
                at(runtime);
            }
        }
        if let Some(bun) = &mut self.runtimes.bun {
            at(&mut bun.project);
            if let Some(runtime) = &mut bun.runtime {
                at(runtime);
            }
            // A bare program name is looked up on PATH, not under the base.
            if let Some(program) = &mut bun.program {
                if program.components().count() > 1 {
                    at(program);
                }
            }
        }
        if let Some(py) = &mut self.runtimes.py {
            at(&mut py.project);
            // A bare program name is looked up on PATH, not under the base.
            if let Some(python) = &mut py.python {
                if python.components().count() > 1 {
                    at(python);
                }
            }
        }
        if let Some(go) = &mut self.runtimes.go {
            if let Some(dir) = &mut go.dir {
                at(dir);
            }
            for binary in &mut go.binaries {
                at(binary);
            }
            at(&mut go.project);
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
    pub fn merge(&mut self, other: HostConfig) {
        if self.runtimes.node.is_none() {
            self.runtimes.node = other.runtimes.node;
        }
        if self.runtimes.py.is_none() {
            self.runtimes.py = other.runtimes.py;
        }
        if self.runtimes.bun.is_none() {
            self.runtimes.bun = other.runtimes.bun;
        }
        if self.runtimes.go.is_none() {
            self.runtimes.go = other.runtimes.go;
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
        let local = [
            ("node", self.runtimes.node.is_some()),
            ("py", self.runtimes.py.is_some()),
            ("bun", self.runtimes.bun.is_some()),
        ];
        let mut seen: Vec<&str> = local
            .iter()
            .filter(|(_, present)| *present)
            .map(|(name, _)| *name)
            .collect();
        for remote in &self.runtimes.remote {
            let name = remote.name.as_str();
            if !rutis_loader::is_runtime_name(name) {
                return Err(format!(
                    "remote runtime {name:?}: a runtime name is two or more of a-z, 0-9 and -, and not file"
                ));
            }
            if seen.contains(&name) {
                return Err(format!(
                    "remote runtime {name}: the name is already a runtime's; rows `{name}:<module>` must name one"
                ));
            }
            // `go:<plugin>` and `go-<binary>:<plugin>` are the Go runtimes' rows.
            if self.runtimes.go.is_some() && (name == "go" || name.starts_with("go-")) {
                return Err(format!(
                    "remote runtime {name}: go and names starting with go- are the local Go runtimes'"
                ));
            }
            seen.push(name);
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

    #[test]
    fn go_runtimes_are_checked() {
        let parse =
            |value: serde_json::Value| -> GoRuntime { serde_json::from_value(value).unwrap() };
        assert!(parse(json!({ "dir": "plugins/go" })).validate().is_ok());
        assert!(
            parse(json!({ "binaries": ["bin/netkit"], "start": "eager" }))
                .validate()
                .is_ok()
        );
        let none = parse(json!({})).validate().unwrap_err();
        assert!(none.contains("needs a dir or binaries"), "{none}");
        let both = parse(json!({ "dir": "d", "start": "eager", "idle": 30 }))
            .validate()
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
