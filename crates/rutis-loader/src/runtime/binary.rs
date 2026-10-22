//! Compiled programs as runtimes, whatever their language: one runtime per
//! binary built with a language's SDK (Go's, later Swift's), several side
//! by side, each started when a row uses it. A language is a
//! [`BinaryLanguage`] (its row prefix, its SDK's marker, how its binaries
//! start); everything else is here, and the language's module (`go.rs`)
//! only names its types.
//!
//! A binary says what it serves without being started: `<binary>
//! --rutis-manifest` prints its plugins, what they inject and provide, their
//! config schemas and versions. [`BinaryResolver`] reads the manifests of
//! the binaries it is given (files, and every binary of the language in
//! some directories: those containing its SDK's marker) and resolves
//! `<prefix>:<plugin>` to the binary that has the plugin, or
//! `<runtime>:<plugin>` to a named one; [`BinaryRuntimes`] starts a
//! binary's runtime when a row resolves to it, stops it once idle, and
//! starts it again after its process ended as a [`RestartPolicy`] says.
//!
//! A runtime is named after its file ([`BinaryLanguage::runtime_name`],
//! `go-<name>`): that is the name rows, [`BinaryRuntimesHandle::restart`]
//! and diagnostics use. While a runtime runs, rows resolve from the
//! manifest it started with; a binary replaced on disk takes effect when
//! the runtime restarts. A binary whose session was refused (an old SDK)
//! stays refused until its file changes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use rutis::{shutdown_deadline, BoxFuture, CordisError, Ctx, Effect, FiberView, Plugin, TypeKey};
use rutis_bridge::runtime::{
    LocalRuntime, RowSchema, RuntimeHandle, RuntimeState, Stdio as RuntimeStdio,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{leaf_resolved, RowsSource, RuntimeRowsPlugin};
use crate::backoff::{Backoff, LIMIT, WINDOW};
use crate::{EntryStatus, Loader, LoaderError, Resolved, Resolver, ServiceCatalog};

/// A language whose plugins are compiled into programs, one runtime per
/// program: what differs between Go, Swift and the like. A zero-sized type
/// implements it; the shared types take it as their parameter
/// (`BinaryResolver<Go>`).
pub trait BinaryLanguage: Send + Sync + 'static {
    /// Its rows' prefix (`go`: rows `go:<plugin>`), the start of its
    /// runtimes' names (`go-<file>`), and the `source` in its rows' meta.
    const PREFIX: &'static str;
    /// As messages name it (`Go`).
    const TITLE: &'static str;
    /// In every binary built with its SDK (`rutis-go-runtime:1`): only
    /// files in a directory containing it are run to read their manifest.
    const MARKER: &'static [u8];
    /// The newest plugin API of its binaries this host runs, read from
    /// their manifests.
    const PLUGIN_API: u64;
    /// The runtime the binary at `path` runs as: `<PREFIX>-<file>`.
    fn runtime_name(path: &Path) -> String;
    /// The local runtime that runs `binary` in `project` (its channel is
    /// the local transport's: an inherited fd, or loopback).
    fn launch(binary: &Path, project: &Path) -> LocalRuntime;
}

/// How often [`BinaryRuntimes`] looks for runtimes to start or stop.
const SWEEP: Duration = Duration::from_millis(100);

/// How long a binary may take to print its manifest. Generous: the first
/// run of a new binary on macOS waits for the system to assess it, which
/// takes seconds when several start at once.
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The most a manifest may print, and the most of its errors kept: a
/// binary that floods its stdout is stopped at this size, not buffered
/// without bound. Errors beyond the kept part are read and dropped, so a
/// chatty binary keeps running.
const MANIFEST_OUTPUT: u64 = 16 * 1024 * 1024;
const MANIFEST_ERRORS: u64 = 64 * 1024;

/// Wait for a runtime whose process was killed to report that it is down.
async fn wait_down(handle: &RuntimeHandle, limit: Duration) {
    let _ = tokio::time::timeout(limit, async {
        while !matches!(handle.state(), RuntimeState::Down(_)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

/// How long a failed manifest read stands before a row asking for a plugin
/// no binary has gets it read again: a failure may pass, but a binary that
/// keeps failing (or hangs until [`MANIFEST_TIMEOUT`]) is not run again on
/// every lookup of a missing name.
const FAILED_READ_STANDS: Duration = Duration::from_secs(5);

/// How often starting a binary is tried while it is busy (ETXTBSY), and
/// the first pause between tries, doubled after each.
const BUSY_TRIES: u32 = 5;
const BUSY_PAUSE: Duration = Duration::from_millis(10);

// ── Binaries ─────────────────────────────────────────────────────

/// Where a language's binaries are: files, and directories whose binaries
/// of the language all count. A directory is trusted as a whole: the files
/// in it that contain the SDK's marker are run to read their manifests.
#[derive(Clone, Debug, Default)]
pub struct Binaries {
    files: Vec<(PathBuf, Option<String>)>,
    dirs: Vec<PathBuf>,
}

impl Binaries {
    pub fn new() -> Self {
        Self::default()
    }

    /// The binary at `path`, named after its file.
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.push((path.into(), None));
        self
    }

    /// The binary at `path`, as the runtime `name` (a development build
    /// whose file name changes with every build).
    pub fn file_named(mut self, path: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        self.files.push((path.into(), Some(name.into())));
        self
    }

    /// Every binary of the language in `dir`.
    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dirs.push(dir.into());
        self
    }
}

/// What a binary's manifest says.
#[derive(Clone, Debug, Deserialize)]
pub struct BinaryManifest {
    /// The SDK version it was built with.
    #[serde(default)]
    pub sdk: String,
    /// The plugin API its plugins need.
    #[serde(rename = "pluginApi", default)]
    pub plugin_api: u64,
    /// Its plugins, by name.
    #[serde(default)]
    pub plugins: BTreeMap<String, RowSchema>,
    /// The platform it was built for (`linux/amd64`), when it says.
    #[serde(default)]
    pub platform: Option<String>,
}

/// What can tell that a file changed: its size, modification time and (on
/// Unix) change time, which a copy that keeps the others still changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    changed: Option<(i64, i64)>,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.ctime(), metadata.ctime_nsec()))
    };
    #[cfg(not(unix))]
    let changed = None;
    Some(Stamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        changed,
    })
}

/// A binary as last read.
#[derive(Clone, Debug)]
struct Binary {
    path: PathBuf,
    runtime: String,
    listed: bool,
    stamp: Stamp,
    manifest: Result<Arc<BinaryManifest>, String>,
    /// When the manifest was read (on the async runtime's clock).
    read_at: tokio::time::Instant,
}

/// A binary as [`BinaryResolver::binaries`] reports it.
#[derive(Clone, Debug)]
pub struct BinaryFile {
    pub path: PathBuf,
    /// Its runtime name.
    pub runtime: String,
    /// Listed explicitly (a file), not found in a directory.
    pub listed: bool,
    /// Its manifest, or why it could not be read.
    pub manifest: Result<Arc<BinaryManifest>, String>,
}

/// What the binary at `path` serves, whatever its language: it runs `path
/// --rutis-manifest` (within 30 s).
pub fn read_binary_manifest(path: &Path) -> Result<BinaryManifest, String> {
    read_manifest(path)
}

/// Run `spawn` again while it fails because the file is busy: on Linux a
/// file just written cannot be run (ETXTBSY) while a process another
/// thread is starting still holds it open for writing, until that process
/// runs its own program a moment later (Go's os/exec and cargo retry too).
fn retry_busy<T>(mut spawn: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    // ETXTBSY on Linux and macOS.
    const ETXTBSY: i32 = 26;
    let mut pause = BUSY_PAUSE;
    let mut tries = 1;
    loop {
        match spawn() {
            Err(error)
                if cfg!(unix) && error.raw_os_error() == Some(ETXTBSY) && tries < BUSY_TRIES =>
            {
                std::thread::sleep(pause);
                pause *= 2;
                tries += 1;
            }
            result => return result,
        }
    }
}

fn read_manifest(path: &Path) -> Result<BinaryManifest, String> {
    let mut child = retry_busy(|| {
        Command::new(path)
            .arg("--rutis-manifest")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    })
    .map_err(|error| match error.raw_os_error() {
        // ENOEXEC (Unix), ERROR_BAD_EXE_FORMAT (Windows).
        Some(8) if cfg!(unix) => wrong_platform(),
        Some(193) if cfg!(windows) => wrong_platform(),
        _ => format!("cannot run it: {error}"),
    })?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let reading = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout
            .by_ref()
            .take(MANIFEST_OUTPUT + 1)
            .read_to_end(&mut out);
        out
    });
    let errors = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stderr
            .by_ref()
            .take(MANIFEST_ERRORS + 1)
            .read_to_end(&mut out);
        // Read the rest and drop it: closing the pipe would kill a binary
        // that writes more than we keep.
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        out
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > MANIFEST_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "it did not print its manifest within {} s",
                    MANIFEST_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    };
    let out = reading.join().unwrap_or_default();
    let err = errors.join().unwrap_or_default();
    if out.len() as u64 > MANIFEST_OUTPUT {
        return Err(format!(
            "it printed more than {} MiB of manifest",
            MANIFEST_OUTPUT / (1024 * 1024)
        ));
    }
    if !status.success() {
        let err = String::from_utf8_lossy(&err);
        let tail: String = err
            .trim()
            .lines()
            .last()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        return Err(format!("--rutis-manifest exited with {status}: {tail}"));
    }
    serde_json::from_slice(&out).map_err(|error| format!("it printed no manifest: {error}"))
}

fn wrong_platform() -> String {
    format!(
        "it is not built for {}/{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Whether `path` could be a binary of the SDK whose marker is `marker`:
/// an executable file not starting with `.` that contains it.
fn candidate(path: &Path, marker: &[u8]) -> bool {
    let named = path
        .file_name()
        .map(|name| !name.to_string_lossy().starts_with('.'))
        .unwrap_or(false);
    if !named {
        return false;
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    #[cfg(windows)]
    {
        let exe = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
        if !exe {
            return false;
        }
    }
    contains_marker(path, marker)
}

fn contains_marker(path: &Path, marker: &[u8]) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buffer = vec![0u8; 1 << 20];
    let mut carried = 0usize;
    loop {
        let Ok(read) = file.read(&mut buffer[carried..]) else {
            return false;
        };
        if read == 0 {
            return false;
        }
        let filled = carried + read;
        let window = &buffer[..filled];
        let first = marker[0];
        let mut at = 0;
        while let Some(offset) = window[at..].iter().position(|&byte| byte == first) {
            let start = at + offset;
            if window.len() - start < marker.len() {
                break;
            }
            if &window[start..start + marker.len()] == marker {
                return true;
            }
            at = start + 1;
        }
        // Keep the tail, in case the marker straddles two reads.
        let keep = (marker.len() - 1).min(filled);
        buffer.copy_within(filled - keep..filled, 0);
        carried = keep;
    }
}

/// Run `work`, which blocks, without stalling the async runtime.
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

// ── BinaryResolver ───────────────────────────────────────────────

#[derive(Default)]
struct State {
    /// The binaries read so far, by path.
    scanned: BTreeMap<PathBuf, Binary>,
    /// The manifest each running runtime started with.
    launched: HashMap<String, Launched>,
    /// The row names resolved to each runtime.
    resolved: HashMap<String, HashSet<String>>,
    /// The row names of each runtime to resolve again before they start.
    stale: HashMap<String, HashSet<String>>,
    /// Files skipped, and why.
    diagnostics: Vec<String>,
    /// Files in directories found not to be Go binaries, as they were then:
    /// not read again while unchanged.
    rejected: HashMap<PathBuf, Stamp>,
    /// The runtimes whose binary the session refused (events protocol
    /// §2.2: it lacks `events-v2`), the binary as it was then and why: its
    /// rows fail with why, and it is not started again while unchanged.
    refused: HashMap<String, (Option<Stamp>, String)>,
}

/// Which manifests a scan reads again although their files did not change.
#[derive(Clone, Copy)]
enum Force<'a> {
    None,
    All,
    Runtime(&'a str),
    /// Those whose last read failed at least [`FAILED_READ_STANDS`] ago:
    /// the failure may have passed. (#306 most likely was ETXTBSY, which
    /// `read_manifest` retries; that is inferred, not seen: any one-off
    /// failure stayed cached while the file did not change.)
    Failed,
}

#[derive(Clone)]
struct Launched {
    stamp: Stamp,
    manifest: Arc<BinaryManifest>,
}

/// Resolves `<prefix>:<plugin>` and `<runtime>:<plugin>` rows to the
/// language's binaries. Give the loader a shared reference
/// (`Chain::with_shared`) and the same one to [`BinaryRuntimes`].
pub struct BinaryResolver<L: BinaryLanguage> {
    binaries: Mutex<Binaries>,
    catalog: ServiceCatalog,
    reserved: HashSet<String>,
    state: Mutex<State>,
    runtimes: Mutex<Weak<Inner<L>>>,
    language: PhantomData<fn() -> L>,
}

impl<L: BinaryLanguage> BinaryResolver<L> {
    pub fn new(binaries: Binaries) -> Self {
        Self {
            binaries: Mutex::new(binaries),
            catalog: ServiceCatalog::default(),
            reserved: HashSet::new(),
            state: Mutex::default(),
            runtimes: Mutex::new(Weak::new()),
            language: PhantomData,
        }
    }

    /// The catalog the loader uses (`LoaderOptions::catalog`).
    pub fn with_catalog(mut self, catalog: &ServiceCatalog) -> Self {
        self.catalog = catalog.clone();
        self
    }

    /// The names of the application's other runtimes (`node`, `py`, remote
    /// ones): a runtime of this language may not take one.
    pub fn reserve(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.reserved.extend(names.into_iter().map(Into::into));
        self
    }

    /// Use `path` as the binary of the runtime `runtime` from now on (a new
    /// development build). The running runtime keeps its process until it
    /// restarts.
    pub fn replace(&self, runtime: &str, path: impl Into<PathBuf>) {
        let path = path.into();
        let mut binaries = self.binaries.lock().unwrap();
        for (file, name) in &mut binaries.files {
            let current = name.clone().unwrap_or_else(|| L::runtime_name(file));
            if current == runtime {
                *file = path.clone();
                *name = Some(runtime.to_owned());
                return;
            }
        }
        binaries.files.push((path, Some(runtime.to_owned())));
    }

    /// Every binary, read again (no cache): for diagnostics.
    pub fn binaries(&self) -> Vec<BinaryFile> {
        self.scan(Force::All)
            .into_iter()
            .map(|binary| BinaryFile {
                path: binary.path,
                runtime: binary.runtime,
                listed: binary.listed,
                manifest: binary.manifest,
            })
            .collect()
    }

    /// Files found in directories that were skipped, and why.
    pub fn diagnostics(&self) -> Vec<String> {
        self.state.lock().unwrap().diagnostics.clone()
    }

    /// Read the binaries: a manifest is read again when its file changed,
    /// or when `force` says so.
    fn scan(&self, force: Force) -> Vec<Binary> {
        let binaries = self.binaries.lock().unwrap().clone();
        blocking(|| {
            let mut found: Vec<(PathBuf, String, bool)> = Vec::new();
            let mut diagnostics = Vec::new();
            for (path, name) in &binaries.files {
                let runtime = name.clone().unwrap_or_else(|| L::runtime_name(path));
                found.push((path.clone(), runtime, true));
            }
            for dir in &binaries.dirs {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    diagnostics.push(format!("{}: cannot read the directory", dir.display()));
                    continue;
                };
                let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
                paths.sort();
                for path in paths {
                    if found.iter().any(|(listed, _, _)| *listed == path) {
                        continue;
                    }
                    let (cached, rejected) = {
                        let state = self.state.lock().unwrap();
                        let current = stamp(&path);
                        let rejected =
                            current.is_some() && state.rejected.get(&path) == current.as_ref();
                        (state.scanned.contains_key(&path), rejected)
                    };
                    if rejected {
                        continue;
                    }
                    if cached || candidate(&path, L::MARKER) {
                        let runtime = L::runtime_name(&path);
                        found.push((path, runtime, false));
                    } else if let Some(current) = stamp(&path) {
                        self.state.lock().unwrap().rejected.insert(path, current);
                    }
                }
            }
            let mut out = Vec::new();
            let mut seen: HashMap<String, PathBuf> = HashMap::new();
            for (path, runtime, listed) in found {
                let Some(current) = stamp(&path) else {
                    if listed {
                        out.push(Binary {
                            path: path.clone(),
                            runtime,
                            listed,
                            stamp: Stamp {
                                len: 0,
                                modified: None,
                                changed: None,
                            },
                            manifest: Err("there is no such file".into()),
                            read_at: tokio::time::Instant::now(),
                        });
                    }
                    self.state.lock().unwrap().scanned.remove(&path);
                    continue;
                };
                let previous = self.state.lock().unwrap().scanned.get(&path).cloned();
                let forced = match force {
                    Force::None => false,
                    Force::All => true,
                    Force::Runtime(name) => name == runtime,
                    Force::Failed => previous.as_ref().is_some_and(|previous| {
                        previous.manifest.is_err()
                            && previous.read_at.elapsed() >= FAILED_READ_STANDS
                    }),
                };
                let mut binary = match previous {
                    Some(previous) if !forced && previous.stamp == current => previous,
                    _ => {
                        let manifest = read_manifest(&path).map(Arc::new);
                        Binary {
                            path: path.clone(),
                            runtime: runtime.clone(),
                            listed,
                            stamp: current,
                            manifest,
                            read_at: tokio::time::Instant::now(),
                        }
                    }
                };
                binary.runtime = runtime.clone();
                binary.listed = listed;
                self.state
                    .lock()
                    .unwrap()
                    .scanned
                    .insert(path.clone(), binary.clone());
                if let (false, Err(error)) = (listed, &binary.manifest) {
                    diagnostics.push(format!("{}: {error}", path.display()));
                    continue;
                }
                let clash = if self.reserved.contains(&runtime) {
                    Some(format!("the runtime name {runtime} is another runtime's"))
                } else {
                    seen.get(&runtime).map(|other| {
                        format!(
                            "{} and {} are both the runtime {runtime}",
                            other.display(),
                            path.display()
                        )
                    })
                };
                if let Some(clash) = clash {
                    if listed {
                        binary.manifest = Err(format!("configuration error: {clash}"));
                        out.push(binary);
                    } else {
                        diagnostics.push(format!("{}: skipped: {clash}", path.display()));
                    }
                    continue;
                }
                seen.insert(runtime, path);
                out.push(binary);
            }
            self.state.lock().unwrap().diagnostics = diagnostics;
            out
        })
    }

    /// The manifest rows of `binary` resolve from: the one its runtime
    /// started with while it runs, else the file's.
    fn manifest_of(&self, binary: &Binary) -> Result<(Arc<BinaryManifest>, bool), String> {
        let launched = self
            .state
            .lock()
            .unwrap()
            .launched
            .get(&binary.runtime)
            .cloned();
        match launched {
            Some(launched) => Ok((launched.manifest, launched.stamp != binary.stamp)),
            None => binary.manifest.clone().map(|manifest| (manifest, false)),
        }
    }

    /// Read the binary of `runtime` again and record its manifest as the one
    /// its runtime starts with; the rows resolved to it resolve again.
    fn launch(&self, runtime: &str) -> Result<PathBuf, String> {
        let binary = self
            .scan(Force::Runtime(runtime))
            .into_iter()
            .find(|binary| binary.runtime == runtime)
            .ok_or_else(|| format!("there is no {} runtime {runtime}", L::TITLE))?;
        let manifest = binary.manifest.clone()?;
        let mut state = self.state.lock().unwrap();
        state.launched.insert(
            runtime.to_owned(),
            Launched {
                stamp: binary.stamp.clone(),
                manifest,
            },
        );
        let resolved = state.resolved.get(runtime).cloned().unwrap_or_default();
        state
            .stale
            .entry(runtime.to_owned())
            .or_default()
            .extend(resolved);
        Ok(binary.path)
    }

    fn stopped(&self, runtime: &str) {
        self.state.lock().unwrap().launched.remove(runtime);
    }

    /// The session refused the binary of `runtime` (`stamp`, as it started)
    /// for `why`: record it, unless it was already. `true` when it is new.
    fn refuse(&self, runtime: &str, stamp: Option<Stamp>, why: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        if state
            .refused
            .get(runtime)
            .is_some_and(|(at, _)| *at == stamp)
        {
            return false;
        }
        state
            .refused
            .insert(runtime.to_owned(), (stamp, why.to_owned()));
        true
    }

    /// Why the binary of `runtime` is refused, while it is the file the
    /// session refused; a changed file is tried again.
    fn refusal(&self, runtime: &str, stamp: &Stamp) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        let (at, why) = state.refused.get(runtime)?;
        if at.as_ref() == Some(stamp) {
            return Some(why.clone());
        }
        state.refused.remove(runtime);
        None
    }

    fn take_stale(&self, runtime: &str) -> HashSet<String> {
        self.state
            .lock()
            .unwrap()
            .stale
            .get(runtime)
            .cloned()
            .unwrap_or_default()
    }

    fn resolve_now(&self, name: &str) -> Result<Resolved, LoaderError> {
        let not_found = || LoaderError::NotFound {
            name: name.to_owned(),
        };
        let failed = |message: String| LoaderError::Resolve {
            name: name.to_owned(),
            message,
        };
        let (named, plugin) = match name
            .strip_prefix(L::PREFIX)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            Some(plugin) => (None, plugin),
            None => {
                let (runtime, plugin) = name.split_once(':').ok_or_else(not_found)?;
                let ours = runtime
                    .strip_prefix(L::PREFIX)
                    .is_some_and(|rest| rest.starts_with('-'));
                if !ours {
                    return Err(not_found());
                }
                (Some(runtime), plugin)
            }
        };
        if plugin.is_empty() || plugin.contains('/') {
            return Err(not_found());
        }
        let mut binaries = self.scan(Force::None);
        let mut rescanned = false;
        loop {
            if let Some(runtime) = named {
                if let Some(binary) = binaries.iter().find(|binary| binary.runtime == runtime) {
                    let (manifest, replaced) = match self.manifest_of(binary) {
                        Ok(read) => read,
                        // A listed binary whose read failed: the rescan
                        // below may read it again; it says why otherwise.
                        Err(_) if !rescanned => {
                            rescanned = true;
                            binaries = self.scan(Force::Failed);
                            continue;
                        }
                        Err(error) => return Err(failed(error)),
                    };
                    if manifest.plugins.contains_key(plugin) {
                        return self.build(name, binary, &manifest, plugin);
                    }
                    let mut message = format!(
                        "{runtime} has no plugin {plugin}; it has: {}",
                        manifest
                            .plugins
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    if replaced {
                        message.push_str(&format!(
                            "; its binary was replaced, which takes effect after restarting it ({}Runtimes restart {runtime:?})",
                            L::TITLE
                        ));
                    }
                    return Err(failed(message));
                }
            } else {
                let mut having = Vec::new();
                let mut replaced_having = Vec::new();
                for binary in &binaries {
                    let Ok((manifest, replaced)) = self.manifest_of(binary) else {
                        continue;
                    };
                    if manifest.plugins.contains_key(plugin) {
                        having.push((binary, manifest));
                    } else if replaced
                        && binary
                            .manifest
                            .as_ref()
                            .is_ok_and(|disk| disk.plugins.contains_key(plugin))
                    {
                        replaced_having.push(binary.runtime.clone());
                    }
                }
                match having.len() {
                    1 => {
                        let (binary, manifest) = &having[0];
                        return self.build(name, binary, manifest, plugin);
                    }
                    0 if !replaced_having.is_empty() => {
                        return Err(failed(format!(
                            "the binary of {} was replaced; {plugin} takes effect after restarting it ({}Runtimes restart {:?})",
                            replaced_having[0],
                            L::TITLE,
                            replaced_having[0]
                        )))
                    }
                    0 => {}
                    _ => {
                        let runtimes: Vec<String> = having
                            .iter()
                            .map(|(binary, _)| binary.runtime.clone())
                            .collect();
                        return Err(failed(format!(
                            "{plugin} is in several {} binaries ({}): name one, as {}:{plugin}",
                            L::TITLE,
                            runtimes.join(", "),
                            runtimes[0]
                        )));
                    }
                }
            }
            if rescanned {
                break;
            }
            // Not found: a binary may have been dropped into a directory,
            // or a read that failed a while ago may succeed now (#306).
            rescanned = true;
            binaries = self.scan(Force::Failed);
        }
        if let Some(runtime) = named {
            // A listed binary whose manifest failed says why.
            if let Some(binary) = self
                .state
                .lock()
                .unwrap()
                .scanned
                .values()
                .find(|binary| binary.runtime == runtime && binary.listed)
            {
                if let Err(error) = &binary.manifest {
                    return Err(failed(format!("{}: {error}", binary.path.display())));
                }
            }
        }
        Err(not_found())
    }

    fn build(
        &self,
        name: &str,
        binary: &Binary,
        manifest: &BinaryManifest,
        plugin: &str,
    ) -> Result<Resolved, LoaderError> {
        if let Some(why) = self.refusal(&binary.runtime, &binary.stamp) {
            return Err(LoaderError::Resolve {
                name: name.to_owned(),
                message: why,
            });
        }
        if manifest.plugin_api > L::PLUGIN_API {
            return Err(LoaderError::Resolve {
                name: name.to_owned(),
                message: format!(
                    "{plugin} needs plugin API {}; this host supports {}: upgrade the host",
                    manifest.plugin_api,
                    L::PLUGIN_API
                ),
            });
        }
        let described = &manifest.plugins[plugin];
        // A leaf runtime: every service the plugin injects gates it in rutis.
        let gated = described.inject.clone();
        let meta = json!({
            "source": L::PREFIX,
            "binary": binary.path,
            "runtime": binary.runtime,
            "entry": plugin,
            "version": described.version,
            "sdk": manifest.sdk,
            "inject": described.inject,
            "provides": described.provides,
        });
        let resolved = leaf_resolved(
            name,
            &binary.runtime,
            PathBuf::from(plugin),
            gated,
            described,
            &self.catalog,
            meta,
        );
        {
            let mut state = self.state.lock().unwrap();
            state
                .resolved
                .entry(binary.runtime.clone())
                .or_default()
                .insert(name.to_owned());
            if let Some(stale) = state.stale.get_mut(&binary.runtime) {
                stale.remove(name);
            }
        }
        let runtimes = self.runtimes.lock().unwrap().upgrade();
        if let Some(runtimes) = runtimes {
            runtimes.want(&binary.runtime);
        }
        Ok(resolved)
    }
}

impl<L: BinaryLanguage> Resolver for BinaryResolver<L> {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move { self.resolve_now(name).map(Arc::new) })
    }
}

/// The rows of one runtime, for its [`RuntimeRowsPlugin`].
struct BinaryRows<L: BinaryLanguage> {
    resolver: Arc<BinaryResolver<L>>,
    runtime: String,
}

impl<L: BinaryLanguage> RowsSource for BinaryRows<L> {
    fn runtime_name(&self) -> &str {
        &self.runtime
    }

    fn take_stale(&self) -> HashSet<String> {
        self.resolver.take_stale(&self.runtime)
    }
}

/// Whether a runtime is down because its session was refused (events
/// protocol §2.2: `incompatible session: …`), which starting the same
/// binary again would only repeat.
fn refused(why: &str) -> bool {
    why.starts_with("incompatible session: ")
}

// ── Restarts ─────────────────────────────────────────────────────

/// What [`BinaryRuntimes`] does when a runtime's process ends on its own (a
/// crash, `kill -9`, a plugin making it exit), once supervising
/// ([`BinaryRuntimesHandle::supervise`]). A runtime the session refused
/// (an old SDK) did not end on its own: it stays down, whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Nothing: its rows wait. A new binary, or
    /// [`BinaryRuntimesHandle::restart`], starts it again.
    #[default]
    No,
    /// Start it again after the [`Backoff`]'s wait, up to its limit
    /// ([`RuntimeEnd::Restarting`], [`RuntimeEnd::GaveUp`]).
    OnFailure,
    /// Nothing here: [`RuntimeEnd::Exit`] says so, for the application to
    /// stop.
    Exit,
}

/// What supervising did about a runtime whose process ended, as
/// [`BinaryRuntimesHandle::supervise`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeEnd {
    /// `on-failure`: its wait is over, and it starts again now.
    Restarting { runtime: String },
    /// `on-failure`: it was restarted [`LIMIT`] times within [`WINDOW`]
    /// and ended again; it stays down.
    GaveUp { runtime: String },
    /// `exit`: it ended, and why.
    Exit { runtime: String, why: String },
}

impl RuntimeEnd {
    /// The line a host prints for it, if any: `runtime <name>: restarting`,
    /// `runtime <name>: gave up after 5 restarts within 5 minutes`
    /// (rutis-host's, for every local runtime).
    pub fn line(&self) -> Option<String> {
        match self {
            RuntimeEnd::Restarting { runtime } => Some(format!("runtime {runtime}: restarting")),
            RuntimeEnd::GaveUp { runtime } => Some(format!(
                "runtime {runtime}: gave up after {LIMIT} restarts within {} minutes",
                WINDOW.as_secs() / 60
            )),
            RuntimeEnd::Exit { .. } => None,
        }
    }
}

type Report = Arc<dyn Fn(RuntimeEnd) + Send + Sync>;

/// A [`RestartPolicy`] in force, and where its ends go.
#[derive(Clone)]
struct Supervision {
    policy: RestartPolicy,
    report: Report,
}

// ── BinaryRuntimes ───────────────────────────────────────────────

/// Where a runtime is.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BinaryRuntimeState {
    NotStarted,
    Starting,
    Running,
    /// It stopped, and why (a crash, an idle stop).
    Stopped(String),
}

/// A runtime as [`BinaryRuntimesHandle::runtimes`] reports it.
#[derive(Clone, Debug)]
pub struct BinaryRuntimeInfo {
    pub name: String,
    pub binary: PathBuf,
    pub state: BinaryRuntimeState,
    /// Its binary was replaced since it started: restart it for the new one.
    pub replaced: bool,
    /// Its manifest (the one it runs with, when running), or why it could
    /// not be read.
    pub manifest: Result<Arc<BinaryManifest>, String>,
}

struct Slot {
    view: FiberView,
    handle: RuntimeHandle,
    /// The file it started from, to tell a changed binary after a crash.
    stamp: Option<Stamp>,
    binary: PathBuf,
    last_used: Instant,
    stopped: Option<String>,
    /// When it started (on the async runtime's clock), for the backoff.
    started: tokio::time::Instant,
    /// Which start it is: a restart decided for one start does not restart
    /// a later one.
    generation: u64,
}

impl Slot {
    /// Note that its process ended, if it did and that is news: why.
    fn ended(&mut self) -> Option<String> {
        if self.stopped.is_some() {
            return None;
        }
        let RuntimeState::Down(reason) = self.handle.state() else {
            return None;
        };
        self.stopped = Some(reason.clone());
        Some(reason)
    }
}

/// A runtime whose process ended, as noted: for [`Inner::ended`].
struct Ended {
    runtime: String,
    why: String,
    ran: Duration,
    generation: u64,
}

struct Inner<L: BinaryLanguage> {
    resolver: Arc<BinaryResolver<L>>,
    project: PathBuf,
    /// Set by the builder methods before the plugin runs.
    settings: Mutex<Settings>,
    ctx: Mutex<Option<Ctx>>,
    slots: Mutex<Slots>,
    /// Wanted before the plugin ran.
    early: Mutex<HashSet<String>>,
    /// The restart policy in force, while supervising.
    supervision: Mutex<Option<Supervision>>,
    /// The ends noted before supervising first started (a runtime that
    /// crashed as it started, while the application was still starting),
    /// for [`BinaryRuntimesHandle::supervise`] to handle: the latest of
    /// each runtime, so at most one per runtime. `None` once it did, or
    /// once [`BinaryRuntimesHandle::unsupervise`] said it will not come.
    /// Locked under `supervision`.
    unreported: Mutex<Option<HashMap<String, Ended>>>,
    /// Each runtime's restarts so far, kept across its starts and stops.
    backoffs: Mutex<HashMap<String, Backoff>>,
    /// The restarts waiting out their backoff: aborted when supervising
    /// stops.
    pending: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

#[derive(Clone, Copy)]
struct Settings {
    idle: Option<Duration>,
    eager: bool,
    /// Each runtime process's standard output and error.
    output: (RuntimeStdio, RuntimeStdio),
}

#[derive(Default)]
struct Slots {
    running: HashMap<String, Slot>,
    /// Runtimes being started, restarted or stopped: nothing else starts
    /// one of them meanwhile, so no two processes run under one name.
    busy: HashSet<String>,
    /// Runtimes a row asked for while they were busy: wanted again once
    /// that is over (a stop in progress is followed by a start).
    rewanted: HashSet<String>,
    /// The last start's generation.
    generation: u64,
}

/// A runtime marked busy. The mark goes when this does, on every way out
/// (a future dropped half way too); a runtime wanted meanwhile is wanted
/// again then.
struct Busy<L: BinaryLanguage> {
    inner: Arc<Inner<L>>,
    runtime: String,
}

impl<L: BinaryLanguage> Busy<L> {
    /// Mark `runtime` busy, unless it is already.
    fn take(inner: &Arc<Inner<L>>, runtime: &str) -> Option<Self> {
        let marked = inner.slots.lock().unwrap().busy.insert(runtime.to_owned());
        marked.then(|| Self::held(inner, runtime))
    }

    /// The guard of a mark already set under the slots' lock.
    fn held(inner: &Arc<Inner<L>>, runtime: &str) -> Self {
        Self {
            inner: inner.clone(),
            runtime: runtime.to_owned(),
        }
    }
}

impl<L: BinaryLanguage> Drop for Busy<L> {
    fn drop(&mut self) {
        let again = {
            let mut slots = self.inner.slots.lock().unwrap();
            slots.busy.remove(&self.runtime);
            slots.rewanted.remove(&self.runtime)
        };
        if again {
            let (inner, runtime) = (self.inner.clone(), self.runtime.clone());
            // Not here: starting reads a manifest, which blocks.
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn_blocking(move || inner.want(&runtime));
            }
        }
    }
}

/// What [`Inner::want`] decided to do for a runtime.
enum Transition {
    Start,
    Restart,
}

/// Starts the runtimes rows use, one per binary, and stops them once idle.
/// Mount it after the loader, with the [`BinaryResolver`] the loader uses.
pub struct BinaryRuntimes<L: BinaryLanguage> {
    inner: Arc<Inner<L>>,
    name: String,
    injects: [TypeKey; 1],
}

/// Controls the runtimes from outside the plugin.
pub struct BinaryRuntimesHandle<L: BinaryLanguage> {
    inner: Arc<Inner<L>>,
}

impl<L: BinaryLanguage> Clone for BinaryRuntimesHandle<L> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<L: BinaryLanguage> BinaryRuntimes<L> {
    /// Runtimes for the binaries `resolver` knows, run in `project`; they
    /// stop 60 s after their last row is gone.
    pub fn new(resolver: Arc<BinaryResolver<L>>, project: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                resolver,
                project: project.into(),
                settings: Mutex::new(Settings {
                    idle: Some(Duration::from_secs(60)),
                    eager: false,
                    output: (RuntimeStdio::Inherit, RuntimeStdio::Inherit),
                }),
                ctx: Mutex::new(None),
                slots: Mutex::default(),
                early: Mutex::default(),
                supervision: Mutex::new(None),
                unreported: Mutex::new(Some(HashMap::new())),
                backoffs: Mutex::default(),
                pending: Mutex::default(),
            }),
            name: format!("{}-runtimes", L::PREFIX),
            injects: [TypeKey::of::<Loader>()],
        }
    }

    /// Stop a runtime once `idle` passed without a row using it; `None`
    /// never stops one.
    pub fn idle(self, idle: Option<Duration>) -> Self {
        self.inner.settings.lock().unwrap().idle = idle;
        self
    }

    /// Start every binary's runtime when mounted, and keep them running.
    pub fn eager(self) -> Self {
        let mut settings = self.inner.settings.lock().unwrap();
        settings.idle = None;
        settings.eager = true;
        drop(settings);
        self
    }

    /// Where each runtime process's standard output and error go: this
    /// process's by default ([`RuntimeStdio::Inherit`]), as
    /// [`LocalRuntime::stdout`] and [`LocalRuntime::stderr`].
    pub fn output(self, stdout: RuntimeStdio, stderr: RuntimeStdio) -> Self {
        self.inner.settings.lock().unwrap().output = (stdout, stderr);
        self
    }

    pub fn handle(&self) -> BinaryRuntimesHandle<L> {
        BinaryRuntimesHandle {
            inner: self.inner.clone(),
        }
    }
}

impl<L: BinaryLanguage> BinaryRuntimesHandle<L> {
    /// Stop the runtime `name` and start it again from its binary as it is
    /// now: its rows stop, resolve from the new manifest, and start again.
    /// A caller that stops waiting (a timeout around it) does not stop the
    /// restart half way: once it began, it goes on to the end.
    pub async fn restart(&self, name: &str) -> Result<(), String> {
        // Wait for a start, restart or stop in progress to finish.
        let busy = loop {
            if let Some(busy) = Busy::take(&self.inner, name) {
                break busy;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        // In a task: dropped half way, the restart would free the name while
        // the old runtime is still stopping, and the next start would run a
        // second process under it meanwhile (#307).
        // Its error is logged there too: a caller that gave up never sees it.
        let inner = self.inner.clone();
        let runtime = name.to_owned();
        tokio::spawn(async move {
            let result = inner.restart_now(&runtime, busy).await;
            if let Err(error) = &result {
                eprintln!(
                    "rutis-loader: cannot restart the {} runtime {runtime}: {error}",
                    L::TITLE
                );
            }
            result
        })
        .await
        .map_err(|error| match error.is_panic() {
            true => format!("the restart of {name} panicked"),
            false => format!("the restart of {name} was cancelled: the async runtime stopped"),
        })?
    }

    /// From now on, when a runtime's process ends on its own, do what
    /// `policy` says, telling `report` (on whatever thread noticed): with
    /// [`RestartPolicy::OnFailure`], start it again after the [`Backoff`]'s
    /// wait (1 s, doubling up to 30 s; one that ran a minute starts over
    /// from 1 s), and give up once it was restarted [`LIMIT`] times within
    /// [`WINDOW`]; with [`RestartPolicy::Exit`], only report it. Replaces
    /// an earlier call's policy and report. A runtime stopped (idle, a
    /// restart, the plugin disposed) did not end on its own. The first
    /// call also handles the runtimes that ended before it and are still
    /// down (one that crashed as it started, while the application was
    /// starting).
    pub fn supervise(
        &self,
        policy: RestartPolicy,
        report: impl Fn(RuntimeEnd) + Send + Sync + 'static,
    ) {
        let earlier = {
            let mut supervision = self.inner.supervision.lock().unwrap();
            *supervision = Some(Supervision {
                policy,
                report: Arc::new(report),
            });
            self.inner.unreported.lock().unwrap().take()
        };
        for ended in earlier.unwrap_or_default().into_values() {
            // Unless something started or stopped it since.
            let still = self
                .inner
                .slots
                .lock()
                .unwrap()
                .running
                .get(&ended.runtime)
                .is_some_and(|slot| slot.generation == ended.generation && slot.stopped.is_some());
            if still {
                self.inner.ended(ended);
            }
        }
    }

    /// Stop supervising: a restart waiting out its backoff does not happen
    /// (the application is stopping). One already starting goes on. Called
    /// before any [`BinaryRuntimesHandle::supervise`], it says none is
    /// coming (restart `no`): the ends noted so far are dropped, and no
    /// more are kept for it.
    pub fn unsupervise(&self) {
        self.inner.unsupervise();
    }

    /// Every runtime: those that run, and the binaries not started.
    pub fn runtimes(&self) -> Vec<BinaryRuntimeInfo> {
        let binaries = self.inner.resolver.scan(Force::None);
        let slots = self.inner.slots.lock().unwrap();
        binaries
            .into_iter()
            .map(|binary| {
                let (state, replaced) = match slots.running.get(&binary.runtime) {
                    None if slots.busy.contains(&binary.runtime) => {
                        (BinaryRuntimeState::Starting, false)
                    }
                    None => (BinaryRuntimeState::NotStarted, false),
                    Some(slot) => {
                        let state = match (&slot.stopped, slot.handle.state()) {
                            (Some(reason), _) => BinaryRuntimeState::Stopped(reason.clone()),
                            (None, RuntimeState::Ready(_)) => BinaryRuntimeState::Running,
                            (None, RuntimeState::Down(reason)) => {
                                BinaryRuntimeState::Stopped(reason)
                            }
                            (None, _) => BinaryRuntimeState::Starting,
                        };
                        (state, slot.stamp.as_ref() != Some(&binary.stamp))
                    }
                };
                let manifest = self
                    .inner
                    .resolver
                    .manifest_of(&binary)
                    .map(|(manifest, _)| manifest);
                BinaryRuntimeInfo {
                    name: binary.runtime,
                    binary: binary.path,
                    state,
                    replaced,
                    manifest,
                }
            })
            .collect()
    }

    /// The state of each runtime started and not stopped since (a crashed
    /// one included), by name. Unlike [`BinaryRuntimesHandle::runtimes`]
    /// it reads no binary: for a status checked often.
    pub fn states(&self) -> Vec<(String, RuntimeState)> {
        let slots = self.inner.slots.lock().unwrap();
        let mut states: Vec<(String, RuntimeState)> = slots
            .running
            .iter()
            .map(|(name, slot)| (name.clone(), slot.handle.state()))
            .collect();
        states.sort_by(|a, b| a.0.cmp(&b.0));
        states
    }
}

impl<L: BinaryLanguage> Inner<L> {
    /// A row resolved to `runtime`, or waits for it: start it unless it
    /// runs, or stopped after a crash with its binary unchanged.
    fn want(self: &Arc<Self>, runtime: &str) {
        if self.ctx.lock().unwrap().is_none() {
            self.early.lock().unwrap().insert(runtime.to_owned());
            return;
        }
        let mut ended = None;
        let transition = {
            let mut slots = self.slots.lock().unwrap();
            if slots.busy.contains(runtime) {
                // Once the start, restart or stop in progress is over.
                slots.rewanted.insert(runtime.to_owned());
                return;
            }
            let transition = match slots.running.get_mut(runtime) {
                Some(slot) => {
                    slot.last_used = Instant::now();
                    if let Some(why) = slot.ended() {
                        // Its process is gone: rows resolve from the file.
                        self.resolver.stopped(runtime);
                        ended = Some(Ended {
                            runtime: runtime.to_owned(),
                            why,
                            ran: slot.started.elapsed(),
                            generation: slot.generation,
                        });
                    }
                    match &slot.stopped {
                        None => None,
                        // Crashed: only a new binary is worth another try
                        // here; its restart policy may start it again.
                        Some(_) if stamp(&slot.binary) != slot.stamp => Some(Transition::Restart),
                        Some(_) => None,
                    }
                }
                None => Some(Transition::Start),
            };
            if transition.is_some() {
                slots.busy.insert(runtime.to_owned());
            }
            transition
        };
        match transition {
            None => {
                if let Some(ended) = ended {
                    self.ended(ended);
                }
            }
            Some(Transition::Start) => {
                let _busy = Busy::held(self, runtime);
                if let Err(error) = self.start(runtime) {
                    eprintln!(
                        "rutis-loader: cannot start the {} runtime {runtime}: {error}",
                        L::TITLE
                    );
                }
            }
            Some(Transition::Restart) => {
                let busy = Busy::held(self, runtime);
                let inner = self.clone();
                let runtime = runtime.to_owned();
                tokio::spawn(async move {
                    if let Err(error) = inner.restart_now(&runtime, busy).await {
                        eprintln!(
                            "rutis-loader: cannot restart the {} runtime {runtime}: {error}",
                            L::TITLE
                        );
                    }
                });
            }
        }
    }

    /// Stop `runtime` and start it again, holding its busy mark. The new
    /// process starts only once the old one is gone: both use the
    /// runtime's name.
    async fn restart_now(self: Arc<Self>, runtime: &str, _busy: Busy<L>) -> Result<(), String> {
        let previous = self.slots.lock().unwrap().running.remove(runtime);
        if let Some(previous) = previous {
            let limit = shutdown_deadline();
            if let Err(error) = previous.view.dispose_with_timeout(limit).await {
                eprintln!(
                    "rutis-loader: stopping the {} runtime {runtime}: {error}",
                    L::TITLE
                );
                if matches!(&error, rutis::DisposeWaitError::TimedOut { .. }) {
                    previous.handle.kill();
                    wait_down(&previous.handle, limit).await;
                    eprintln!(
                        "rutis-loader: the {} runtime {runtime} did not stop in time; killed it",
                        L::TITLE
                    );
                }
            }
        }
        self.resolver.stopped(runtime);
        self.start(runtime)
    }

    fn start(&self, runtime: &str) -> Result<(), String> {
        let ctx = self
            .ctx
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| format!("the {} runtimes are not running", L::TITLE))?;
        let binary = self.resolver.launch(runtime)?;
        let mut runtime_plugin = BinaryRuntime::<L> {
            label: format!("{runtime} ({})", L::PREFIX),
            runtime: runtime.to_owned(),
            binary: binary.clone(),
            project: self.project.clone(),
            output: self.settings.lock().unwrap().output,
            resolver: self.resolver.clone(),
            first: Mutex::new(None),
        };
        let local = runtime_plugin.local();
        let handle = local.handle();
        runtime_plugin.first = Mutex::new(Some(local));
        let view = ctx.plugin(runtime_plugin);
        let mut slots = self.slots.lock().unwrap();
        slots.generation += 1;
        let generation = slots.generation;
        slots.running.insert(
            runtime.to_owned(),
            Slot {
                view,
                handle,
                stamp: stamp(&binary),
                binary,
                last_used: Instant::now(),
                stopped: None,
                started: tokio::time::Instant::now(),
                generation,
            },
        );
        Ok(())
    }

    fn unsupervise(&self) {
        let mut supervision = self.supervision.lock().unwrap();
        *supervision = None;
        *self.unreported.lock().unwrap() = None;
        drop(supervision);
        for pending in self.pending.lock().unwrap().drain(..) {
            pending.abort();
        }
    }

    /// The process of a runtime ended on its own: do what the restart
    /// policy says, if supervising.
    fn ended(self: &Arc<Self>, ended: Ended) {
        // Starting the same binary again would only be refused again.
        if refused(&ended.why) {
            return;
        }
        let supervision = {
            let supervision = self.supervision.lock().unwrap();
            match &*supervision {
                Some(supervision) => supervision.clone(),
                None => {
                    // Not yet supervising: for `supervise` to handle.
                    // An earlier end of the same runtime is over: a later
                    // start replaced it.
                    if let Some(unreported) = &mut *self.unreported.lock().unwrap() {
                        unreported.insert(ended.runtime.clone(), ended);
                    }
                    return;
                }
            }
        };
        let Ended {
            runtime,
            why,
            ran,
            generation,
        } = ended;
        match supervision.policy {
            RestartPolicy::No => {}
            RestartPolicy::Exit => (supervision.report)(RuntimeEnd::Exit { runtime, why }),
            RestartPolicy::OnFailure => {
                let wait = self
                    .backoffs
                    .lock()
                    .unwrap()
                    .entry(runtime.clone())
                    .or_default()
                    .next(tokio::time::Instant::now(), ran);
                let Some(wait) = wait else {
                    (supervision.report)(RuntimeEnd::GaveUp { runtime });
                    return;
                };
                let Ok(tokio) = tokio::runtime::Handle::try_current() else {
                    return;
                };
                let inner = self.clone();
                let task = tokio.spawn(async move {
                    tokio::time::sleep(wait).await;
                    inner.restart_after_end(runtime, generation).await;
                });
                let mut pending = self.pending.lock().unwrap();
                pending.retain(|task| !task.is_finished());
                pending.push(task);
            }
        }
    }

    /// The backoff of `runtime`'s start `generation` is over: start it
    /// again, unless supervising stopped, or something else started or
    /// stopped it meanwhile (a new binary, a restart, an idle stop).
    async fn restart_after_end(self: Arc<Self>, runtime: String, generation: u64) {
        let Some(supervision) = self.supervision.lock().unwrap().clone() else {
            return;
        };
        let Some(busy) = Busy::take(&self, &runtime) else {
            return;
        };
        let still = self
            .slots
            .lock()
            .unwrap()
            .running
            .get(&runtime)
            .is_some_and(|slot| slot.generation == generation && slot.stopped.is_some());
        if !still {
            return;
        }
        (supervision.report)(RuntimeEnd::Restarting {
            runtime: runtime.clone(),
        });
        // In a task of its own: aborted half way (supervising stopped),
        // the restart would free the name while the old runtime is still
        // stopping (#307).
        let inner = self.clone();
        let restart = tokio::spawn(async move {
            if let Err(error) = inner.restart_now(&runtime, busy).await {
                eprintln!(
                    "rutis-loader: cannot restart the {} runtime {runtime}: {error}",
                    L::TITLE
                );
            }
        });
        let _ = restart.await;
    }

    /// The runtimes the session refused (events protocol §2.2), as a local
    /// runtime refused is: their binaries are marked refused, and their
    /// rows, which would wait for them for ever, resolve again and fail
    /// with why.
    fn fail_refused(&self, loader: &Loader) {
        let refused: Vec<String> = {
            let slots = self.slots.lock().unwrap();
            slots
                .running
                .iter()
                .filter_map(|(name, slot)| {
                    let why = slot.stopped.as_deref().filter(|why| refused(why))?;
                    self.resolver
                        .refuse(name, slot.stamp.clone(), why)
                        .then(|| name.clone())
                })
                .collect()
        };
        if refused.is_empty() {
            return;
        }
        let rows: Vec<String> = loader
            .entries()
            .into_iter()
            .filter(|entry| entry.meta.get("source").and_then(Value::as_str) == Some(L::PREFIX))
            .filter(|entry| {
                entry
                    .meta
                    .get("runtime")
                    .and_then(Value::as_str)
                    .is_some_and(|runtime| refused.iter().any(|name| name == runtime))
            })
            .map(|entry| entry.id)
            .collect();
        let loader = loader.clone();
        // Not here: resolving again takes the loader's lock.
        tokio::spawn(async move {
            for id in rows {
                // It fails, and its status says why.
                let _ = loader.refresh(&id).await;
            }
        });
    }

    /// Start the runtimes rows wait for (a row whose resolution the loader
    /// reused asks no resolver), note the ones that crashed, and stop those
    /// no row has used for `idle`. A runtime is in use while an entry that
    /// is not disabled resolves to it.
    fn sweep(self: &Arc<Self>, loader: &Loader, idle: Option<Duration>) {
        let used: HashSet<String> = loader
            .entries()
            .into_iter()
            .filter(|entry| !matches!(entry.status, EntryStatus::Disabled))
            .filter(|entry| entry.meta.get("source").and_then(Value::as_str) == Some(L::PREFIX))
            .filter_map(|entry| {
                entry
                    .meta
                    .get("runtime")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        for runtime in &used {
            let running = self
                .slots
                .lock()
                .unwrap()
                .running
                .get(runtime)
                .is_some_and(|slot| slot.stopped.is_none());
            if !running {
                self.want(runtime);
            }
        }
        let mut stopping = Vec::new();
        let mut crashed = Vec::new();
        {
            let mut slots = self.slots.lock().unwrap();
            let now = Instant::now();
            let Slots { running, busy, .. } = &mut *slots;
            for (name, slot) in running.iter_mut() {
                if let Some(why) = slot.ended() {
                    crashed.push(Ended {
                        runtime: name.clone(),
                        why,
                        ran: slot.started.elapsed(),
                        generation: slot.generation,
                    });
                }
                if used.contains(name) {
                    slot.last_used = now;
                } else if slot.stopped.is_none()
                    && !busy.contains(name)
                    && idle.is_some_and(|idle| now.duration_since(slot.last_used) >= idle)
                {
                    stopping.push(name.clone());
                }
            }
            for name in &stopping {
                busy.insert(name.clone());
            }
        }
        for ended in crashed {
            // Its process is gone: rows resolve from the file.
            self.resolver.stopped(&ended.runtime);
            self.ended(ended);
        }
        self.fail_refused(loader);
        for name in stopping {
            let busy = Busy::held(self, &name);
            let inner = self.clone();
            tokio::spawn(async move {
                let _busy = busy;
                let previous = inner.slots.lock().unwrap().running.remove(&name);
                if let Some(previous) = previous {
                    let limit = shutdown_deadline();
                    if let Err(error) = previous.view.dispose_with_timeout(limit).await {
                        eprintln!(
                            "rutis-loader: stopping the {} runtime {name}: {error}",
                            L::TITLE
                        );
                        if matches!(&error, rutis::DisposeWaitError::TimedOut { .. }) {
                            previous.handle.kill();
                            wait_down(&previous.handle, limit).await;
                            eprintln!(
                                "rutis-loader: the {} runtime {name} did not stop in time; killed it",
                                L::TITLE
                            );
                        }
                    }
                }
                inner.resolver.stopped(&name);
            });
        }
    }
}

impl<L: BinaryLanguage> Plugin for BinaryRuntimes<L> {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let loader = ctx.require::<Loader>()?;
            let inner = self.inner.clone();
            let settings = *inner.settings.lock().unwrap();
            *inner.ctx.lock().unwrap() = Some(ctx.clone());
            *inner.resolver.runtimes.lock().unwrap() = Arc::downgrade(&inner);
            let mut wanted: Vec<String> = inner.early.lock().unwrap().drain().collect();
            if settings.eager {
                for binary in inner.resolver.scan(Force::None) {
                    if binary.manifest.is_ok() && !wanted.contains(&binary.runtime) {
                        wanted.push(binary.runtime);
                    }
                }
            }
            for runtime in wanted {
                inner.want(&runtime);
            }
            let sweeper = {
                let inner = inner.clone();
                let loader = loader.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(SWEEP).await;
                        let inner = inner.clone();
                        let loader = loader.clone();
                        // Starting reads a manifest, which blocks.
                        let _ = tokio::task::spawn_blocking(move || {
                            inner.sweep(&loader, settings.idle)
                        })
                        .await;
                    }
                })
            };
            let inner = self.inner.clone();
            ctx.effect(move || {
                Effect::Disposer(Box::new(move || {
                    sweeper.abort();
                    // Stopping its runtimes is no end of theirs.
                    for pending in inner.pending.lock().unwrap().drain(..) {
                        pending.abort();
                    }
                    *inner.ctx.lock().unwrap() = None;
                    *inner.resolver.runtimes.lock().unwrap() = Weak::new();
                    let names: Vec<String> = inner
                        .slots
                        .lock()
                        .unwrap()
                        .running
                        .drain()
                        .map(|(name, _)| name)
                        .collect();
                    for name in names {
                        inner.resolver.stopped(&name);
                    }
                    Ok(())
                }))
            })?;
            Ok(Effect::Done)
        })
    }
}

/// One runtime and its rows' release: stopping it stops both.
struct BinaryRuntime<L: BinaryLanguage> {
    label: String,
    runtime: String,
    binary: PathBuf,
    project: PathBuf,
    /// Its process's standard output and error.
    output: (RuntimeStdio, RuntimeStdio),
    resolver: Arc<BinaryResolver<L>>,
    /// The runtime whose handle the slot watches, for the first start.
    first: Mutex<Option<LocalRuntime>>,
}

impl<L: BinaryLanguage> BinaryRuntime<L> {
    fn local(&self) -> LocalRuntime {
        L::launch(&self.binary, &self.project)
            .named(self.runtime.clone())
            .stdout(self.output.0)
            .stderr(self.output.1)
    }
}

impl<L: BinaryLanguage> Plugin for BinaryRuntime<L> {
    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let local = self
                .first
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| self.local());
            ctx.plugin(local);
            ctx.plugin(RuntimeRowsPlugin::new(Arc::new(BinaryRows::<L> {
                resolver: self.resolver.clone(),
                runtime: self.runtime.clone(),
            })));
            Ok(Effect::Done)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy() -> std::io::Error {
        std::io::Error::from_raw_os_error(26)
    }

    /// A start that fails while the file is busy is tried again (#306).
    #[cfg(unix)]
    #[test]
    fn a_busy_file_is_started_again() {
        let mut tries = 0;
        let started = retry_busy(|| {
            tries += 1;
            match tries {
                1 | 2 => Err(busy()),
                _ => Ok(tries),
            }
        });
        assert_eq!(started.unwrap(), 3);
    }

    /// Only so often, and other failures not at all.
    #[test]
    fn other_failures_and_a_file_that_stays_busy_are_not_tried_forever() {
        let mut tries = 0;
        let started: std::io::Result<()> = retry_busy(|| {
            tries += 1;
            Err(busy())
        });
        assert_eq!(started.unwrap_err().raw_os_error(), Some(26));
        assert_eq!(tries, if cfg!(unix) { BUSY_TRIES } else { 1 });

        let mut tries = 0;
        let started: std::io::Result<()> = retry_busy(|| {
            tries += 1;
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert!(started.is_err());
        assert_eq!(tries, 1);
    }

    #[cfg(unix)]
    fn manifest_script(dir: &Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("runtime");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    /// A manifest that floods stdout is refused at 16 MiB instead of being
    /// buffered without bound.
    #[cfg(unix)]
    #[test]
    fn a_manifest_that_prints_more_than_the_cap_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_script(
            dir.path(),
            "dd if=/dev/zero bs=1048576 count=17 2>/dev/null",
        );
        let error = read_manifest(&path).unwrap_err();
        assert!(error.contains("more than 16 MiB"), "{error}");
    }

    /// Errors beyond the kept 64 KiB are read and dropped: a binary that
    /// writes them keeps running and its manifest is read.
    #[cfg(unix)]
    #[test]
    fn a_manifest_may_write_more_errors_than_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_script(dir.path(), "printf '{}'\nhead -c 100000 /dev/zero >&2");
        assert_eq!(read_manifest(&path).unwrap().plugin_api, 0);
    }

    #[cfg(feature = "go")]
    fn ended(runtime: &str, generation: u64) -> Ended {
        Ended {
            runtime: runtime.into(),
            why: "crashed".into(),
            ran: Duration::ZERO,
            generation,
        }
    }

    #[cfg(feature = "go")]
    fn kept(runtimes: &crate::GoRuntimesHandle) -> Option<Vec<(String, u64)>> {
        let unreported = runtimes.inner.unreported.lock().unwrap();
        unreported.as_ref().map(|ends| {
            let mut kept: Vec<_> = ends
                .values()
                .map(|end| (end.runtime.clone(), end.generation))
                .collect();
            kept.sort();
            kept
        })
    }

    /// Ends noted before supervising are kept for it, only the latest of
    /// each runtime; once `unsupervise` says supervising will not come
    /// (restart `no`, or an application that never supervises and says
    /// so), none are kept.
    #[cfg(feature = "go")]
    #[test]
    fn ends_kept_for_supervising_are_bounded_and_dropped_when_it_will_not_come() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = Arc::new(crate::GoResolver::new(
            crate::GoBinaries::new().dir(dir.path()),
        ));
        let runtimes = crate::GoRuntimes::new(resolver, dir.path()).handle();
        for generation in 1..=100 {
            runtimes.inner.ended(ended("go-a", generation));
            runtimes.inner.ended(ended("go-b", generation));
        }
        assert_eq!(
            kept(&runtimes),
            Some(vec![("go-a".into(), 100), ("go-b".into(), 100)])
        );
        runtimes.unsupervise();
        assert_eq!(kept(&runtimes), None);
        runtimes.inner.ended(ended("go-a", 101));
        assert_eq!(kept(&runtimes), None);
    }
}
