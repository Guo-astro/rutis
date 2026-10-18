//! The published releases a scenario meets the current code with (#209,
//! quality standard Q9.3.2.4): their runtime packages, rutis-host and Go
//! SDK, from the registries, cached.
//!
//! Which releases: `RUTIS_E2E_PREVIOUS` (versions separated by commas;
//! set, it must name at least one), else derived from npm's list of `@arcships/rutis-runtime` versions
//! (every package is released at the same version) and this checkout's
//! version (`node/rutis-host/package.json`): the latest release older than
//! the checkout's version — the previous release — and the latest release
//! that is not newer, when it differs (the checkout's version once it is
//! released, while main has not moved on to the next one). Nothing to edit
//! after a release: with main at 0.11.0 that is 0.10.0 and 0.11.0; once
//! main says 0.12.0, 0.11.0. `RUTIS_E2E_ONLY_OLDER` (set, not empty)
//! leaves the checkout's own version out: the release gate sets it, as a
//! release run again after failing halfway may find it published in part.
//!
//! What a release brings, each installed once per machine into
//! `RUTIS_E2E_CACHE` (else `target/rutis-e2e-previous` in this checkout),
//! one directory per version and kind, made whole before it is used
//! (installed beside it, then renamed):
//!
//! - `npm/`: `npm install --save-exact` of `@arcships/rutis-runtime`,
//!   `@arcships/rutis` (the SDK), `@arcships/rutis-bun` and
//!   `@arcships/rutis-host` (its binary comes in the platform package) at
//!   that exact version, with npm's lockfile beside them;
//! - `python/`: `pip install --target` of `rutis[network]==<version>`, for
//!   `PYTHONPATH` (`RUTIS_PYTHON_PATH` of a host);
//! - Go: no directory; a scenario builds against the tagged module
//!   `github.com/arcships/rutis/go/rutis v<version>` from the Go proxy into
//!   the caller's module cache ([`Release::go_require`]).
//!
//! Offline: without the registry, [`releases`] says why and returns none,
//! so the scenarios pass without running; in CI (`CI` set) that fails
//! instead, and so does an install that fails.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::repo;

/// A published release.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
}

/// The releases to meet ([module docs](self)); none when the registry is
/// out of reach outside CI.
pub fn releases() -> Vec<Release> {
    static RELEASES: OnceLock<Vec<Release>> = OnceLock::new();
    RELEASES
        .get_or_init(|| match resolve() {
            Ok(versions) => {
                let met = format!(
                    "cross-version: the current code meets {} (this checkout is {})",
                    versions.join(", "),
                    current()
                );
                eprintln!("{met}");
                summary(&met);
                versions
                    .into_iter()
                    .map(|version| Release { version })
                    .collect()
            }
            Err(why) if std::env::var_os("CI").is_some() => {
                panic!("cross-version: {why}")
            }
            Err(why) => {
                eprintln!("cross-version: skipped, {why}");
                Vec::new()
            }
        })
        .clone()
}

/// Add `line` to the GitHub Actions job summary, when there is one (CI):
/// which releases a run met, without `--nocapture`.
fn summary(line: &str) {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return;
    };
    use std::io::Write as _;
    let written = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(error) = written {
        eprintln!("cross-version: the job summary: {error}");
    }
}

/// This checkout's version: the one every package is released at.
pub fn current() -> String {
    let manifest: Value =
        serde_json::from_str(include_str!("../../../node/rutis-host/package.json")).unwrap();
    manifest["version"].as_str().unwrap().to_owned()
}

fn resolve() -> Result<Vec<String>, String> {
    if let Ok(list) = std::env::var("RUTIS_E2E_PREVIOUS") {
        let versions: Vec<String> = list
            .split(',')
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .map(str::to_owned)
            .collect();
        // Set wrong (empty, or not versions): a mistake, offline or not,
        // never a request to meet nothing.
        assert!(
            !versions.is_empty(),
            "RUTIS_E2E_PREVIOUS={list:?} names no version"
        );
        for version in &versions {
            assert!(
                parse(version).is_some(),
                "RUTIS_E2E_PREVIOUS: {version} is not a version"
            );
        }
        return Ok(versions);
    }
    let output = Command::new(npm())
        .args([
            "view",
            "@arcships/rutis-runtime",
            "versions",
            "--json",
            "--fetch-retries=0",
            "--fetch-timeout=20000",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("npm cannot run ({error})"))?;
    if !output.status.success() {
        return Err(format!(
            "the npm registry cannot be reached: npm view: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let published: Vec<String> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("npm view printed no list of versions: {error}"))?;
    let only_older =
        std::env::var_os("RUTIS_E2E_ONLY_OLDER").is_some_and(|value| !value.is_empty());
    let versions = choose(&current(), &published, only_older);
    if versions.is_empty() {
        return Err(format!(
            "no release before {} is published: {published:?}",
            current()
        ));
    }
    Ok(versions)
}

/// The releases to meet, of `published`, for a checkout at `current`: the
/// latest release older than it, then the latest one not newer than it
/// when that is another. `only_older` (`RUTIS_E2E_ONLY_OLDER`, set by the
/// release gate) leaves out the checkout's own version: a release run
/// again after it failed halfway may find it published in part.
fn choose(current: &str, published: &[String], only_older: bool) -> Vec<String> {
    let current = parse(current).expect("this checkout's version");
    let mut releases: Vec<(u64, u64, u64)> = published
        .iter()
        .filter_map(|version| parse(version))
        .collect();
    releases.sort();
    let previous = releases.iter().rev().find(|version| **version < current);
    let latest = releases.iter().rev().find(|version| **version <= current);
    let mut chosen: Vec<(u64, u64, u64)> = previous.into_iter().copied().collect();
    if let Some(latest) = latest.filter(|latest| !(only_older && **latest == current)) {
        if Some(latest) != previous {
            chosen.push(*latest);
        }
    }
    chosen
        .into_iter()
        .map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
        .collect()
}

/// `x.y.z`, no pre-release.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let found = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(found)
}

impl Release {
    /// Whether this release is older than `version` (`x.y.z`).
    pub fn older_than(&self, version: &str) -> bool {
        parse(&self.version) < parse(version)
    }

    /// Its `@arcships/rutis-runtime` package directory.
    pub fn node_runtime(&self) -> PathBuf {
        self.npm().join("node_modules/@arcships/rutis-runtime")
    }

    /// Its `@arcships/rutis` (the plugin SDK) package directory.
    pub fn node_sdk(&self) -> PathBuf {
        self.npm().join("node_modules/@arcships/rutis")
    }

    /// Its `@arcships/rutis-bun` package directory.
    pub fn bun_runtime(&self) -> PathBuf {
        self.npm().join("node_modules/@arcships/rutis-bun")
    }

    /// Its `rutis-host` program, as npm installs it for this platform.
    pub fn host(&self) -> PathBuf {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let cpu = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        };
        let program = if cfg!(windows) {
            "rutis-host.exe"
        } else {
            "rutis-host"
        };
        let path = self
            .npm()
            .join(format!("node_modules/@arcships/rutis-host-{os}-{cpu}/bin"))
            .join(program);
        assert!(
            path.exists(),
            "rutis-host {} has no binary for {os}-{cpu}: {}",
            self.version,
            path.display()
        );
        path
    }

    /// The directory with its Python package `rutis` (and websockets), for
    /// `PYTHONPATH`.
    pub fn python_path(&self) -> PathBuf {
        self.install("python", |dir| {
            let requirement = format!("rutis[network]=={}", self.version);
            let mut command = Command::new(python());
            command.args([
                "-m",
                "pip",
                "install",
                "--quiet",
                "--disable-pip-version-check",
            ]);
            command.arg("--target").arg(dir).arg(requirement);
            command
        })
    }

    /// The `require` line of a Go module built against its Go SDK.
    pub fn go_require(&self) -> String {
        format!(
            "require github.com/arcships/rutis/go/rutis v{}",
            self.version
        )
    }

    fn npm(&self) -> PathBuf {
        self.install("npm", |dir| {
            let manifest = r#"{ "name": "rutis-previous", "private": true }"#;
            std::fs::write(dir.join("package.json"), manifest).expect("package.json");
            let mut command = Command::new(npm());
            command
                .args([
                    "install",
                    "--save-exact",
                    "--no-audit",
                    "--no-fund",
                    "--loglevel=error",
                ])
                .args(
                    ["rutis-runtime", "rutis", "rutis-bun", "rutis-host"]
                        .map(|name| format!("@arcships/{name}@{}", self.version)),
                )
                .current_dir(dir);
            command
        })
    }

    /// `<cache>/<version>/<kind>`, installed by `command` (given the
    /// directory to fill) unless it is there already.
    fn install(&self, kind: &str, command: impl FnOnce(&Path) -> Command) -> PathBuf {
        static LOCK: Mutex<()> = Mutex::new(());
        let _one_at_a_time = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = cache().join(&self.version).join(kind);
        if dir.exists() {
            return dir;
        }
        let part = cache()
            .join(&self.version)
            .join(format!("{kind}.part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&part);
        std::fs::create_dir_all(&part).expect("the cache directory");
        let mut command = command(&part);
        let what = format!("{command:?}");
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        // A child of this process, but no scenario's residue.
        crate::residue::started(child.id());
        let output = child
            .wait_with_output()
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        assert!(
            output.status.success(),
            "installing {kind} of release {}: {what}: {}\n{}{}",
            self.version,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if let Err(error) = std::fs::rename(&part, &dir) {
            // Another test process installed it meanwhile.
            assert!(dir.exists(), "{}: {error}", dir.display());
            let _ = std::fs::remove_dir_all(&part);
        }
        dir
    }
}

/// Where releases are installed: `RUTIS_E2E_CACHE`, else
/// `target/rutis-e2e-previous` in this checkout.
fn cache() -> PathBuf {
    match std::env::var_os("RUTIS_E2E_CACHE") {
        Some(dir) => PathBuf::from(dir),
        None => repo().join("target/rutis-e2e-previous"),
    }
}

fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// The Python on PATH: `python3`, `python` on Windows.
pub fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

#[cfg(test)]
mod tests {
    use super::choose;

    fn versions(list: &[&str]) -> Vec<String> {
        list.iter().map(|version| (*version).to_owned()).collect()
    }

    #[test]
    fn the_previous_release_and_the_checkouts_once_released() {
        let published = versions(&["0.9.1", "0.10.0", "0.11.0", "0.12.0-rc.1"]);
        // main at the released version: the one before, and that one.
        assert_eq!(choose("0.11.0", &published, false), ["0.10.0", "0.11.0"]);
        // main moved on to the next version: the latest release only.
        assert_eq!(choose("0.12.0", &published, false), ["0.11.0"]);
        assert_eq!(choose("0.10.0", &published, false), ["0.9.1", "0.10.0"]);
        assert!(choose("0.9.0", &published, false).is_empty());
    }

    /// The release gate (`RUTIS_E2E_ONLY_OLDER`): never the version being
    /// released, published in part or not; older ones as before.
    #[test]
    fn only_older_releases_leave_the_checkouts_out() {
        let published = versions(&["0.9.1", "0.10.0", "0.11.0"]);
        assert_eq!(choose("0.11.0", &published, true), ["0.10.0"]);
        assert_eq!(choose("0.12.0", &published, true), ["0.11.0"]);
        assert!(choose("0.9.1", &published, true).is_empty());
    }
}
