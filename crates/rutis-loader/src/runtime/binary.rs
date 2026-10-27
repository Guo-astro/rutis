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

use rutis::host::{
    shutdown_deadline, BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberView, TypeKey,
};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::{
    LocalRuntime, RowSchema, RuntimeHandle, RuntimeState, Stdio as RuntimeStdio,
};
use rutis_bridge::transport::local::{isolate_manifest_command, ManifestChild, Processes};
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

/// Added to the shutdown deadline when the loader waits for a runtime to
/// stop: the runtime's own cleanups have that deadline too, and one that
/// uses all of it is not killed just as it finishes.
const STOP_MARGIN: Duration = Duration::from_secs(2);

/// How long a runtime's process may take to be gone once it was disposed
/// (the local transport kills it 2 s after closing its channel), or once
/// it was killed, before the loader goes on without it.
const STOP_EXIT: Duration = Duration::from_secs(5);

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

/// The manifest format this host reads. A binary may print a newer one;
/// it is refused whole rather than half-read.
#[doc(hidden)]
pub const MANIFEST_FORMAT: u64 = 1;

fn default_manifest_format() -> u64 {
    MANIFEST_FORMAT
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

/// The format line of a manifest, read with the manifest itself; absent
/// means [`MANIFEST_FORMAT`]. It is not part of the public manifest.
#[derive(Deserialize)]
struct ManifestWire {
    #[serde(rename = "manifest", default = "default_manifest_format")]
    manifest: u64,
    #[serde(flatten)]
    rest: BinaryManifest,
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
#[doc(hidden)]
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
    read_manifest_with_timeout(path, MANIFEST_TIMEOUT)
}

/// Run `path --rutis-manifest` and read what it prints, within `timeout`.
/// The process runs in a group of its own and holds none of this process's
/// descriptors ([`isolate_manifest_command`]); if it, or something it
/// started that still holds its output, outlives `timeout`, the group is
/// killed. Its output is read by threads that send what they read, never
/// joined: a descendant holding the pipes would hold a join forever.
fn read_manifest_with_timeout(path: &Path, timeout: Duration) -> Result<BinaryManifest, String> {
    let mut command = Command::new(path);
    command
        .arg("--rutis-manifest")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_manifest_command(&mut command);
    let mut child = retry_busy(|| command.spawn()).map_err(|error| match error.raw_os_error() {
        // ENOEXEC (Unix), ERROR_BAD_EXE_FORMAT (Windows).
        Some(8) if cfg!(unix) => wrong_platform(),
        Some(193) if cfg!(windows) => wrong_platform(),
        _ => format!("cannot run it: {error}"),
    })?;
    let mut guard = Some(ManifestChild::new(child.id()));
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout
            .by_ref()
            .take(MANIFEST_OUTPUT + 1)
            .read_to_end(&mut out);
        let _ = out_tx.send(out);
    });
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stderr
            .by_ref()
            .take(MANIFEST_ERRORS + 1)
            .read_to_end(&mut out);
        // Read the rest and drop it: closing the pipe would kill a binary
        // that writes more than we keep.
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        let _ = err_tx.send(out);
    });
    let started = Instant::now();
    let deadline = started + timeout;
    let mut out = None;
    let mut err = None;
    // How long the readers get to deliver what the child wrote after it
    // ended, before a pipe still open means something it started holds it.
    const ORPHAN_GRACE: Duration = Duration::from_secs(1);
    let mut ended_at = None;
    let mut orphaned = false;
    // On Unix the process is observed with `waitid(WNOWAIT)`, not reaped:
    // its pid is also its group id, and it must not be reused while the
    // group is killed to release the pipes a descendant still holds.
    #[cfg(not(unix))]
    let mut exit = None;
    let mut ended = false;
    let status = loop {
        if !ended {
            #[cfg(unix)]
            match exited_without_reaping(&child) {
                Ok(true) => {
                    ended = true;
                    ended_at = Some(Instant::now());
                }
                Ok(false) => {}
                Err(error) => {
                    stop_manifest(&mut child, &mut guard, false);
                    return Err(error.to_string());
                }
            }
            #[cfg(not(unix))]
            match child.try_wait() {
                Ok(Some(waited)) => {
                    exit = Some(waited);
                    ended = true;
                    ended_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(error) => {
                    stop_manifest(&mut child, &mut guard, false);
                    return Err(error.to_string());
                }
            }
            if !ended && started.elapsed() > timeout {
                stop_manifest(&mut child, &mut guard, false);
                return Err(format!(
                    "it did not print its manifest within {} s",
                    timeout.as_secs_f32()
                ));
            }
        }
        receive(&out_rx, &mut out, deadline);
        receive(&err_rx, &mut err, deadline);
        if ended {
            // It is gone, but something it started may still hold its
            // output. Give the readers a moment for what it wrote, then
            // kill that group while its id is still this child's (not
            // reaped on Unix), and only then reap it.
            if out.is_none() || err.is_none() {
                let waited = ended_at.unwrap_or_else(Instant::now).elapsed();
                if waited < ORPHAN_GRACE && started.elapsed() <= timeout {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                orphaned = true;
                if let Some(guard) = guard.as_ref() {
                    guard.kill();
                }
            }
            #[cfg(unix)]
            {
                let status = child.wait().map_err(|error| error.to_string())?;
                // Reaped: never kill by its pid again. A group that still
                // holds the pipes was killed just above, while it was not
                // reaped; anything else is left as on a successful read.
                if let Some(guard) = guard.as_mut() {
                    guard.disarm();
                }
                break status;
            }
            #[cfg(not(unix))]
            {
                if let Some(guard) = guard.as_mut() {
                    guard.disarm();
                }
                break exit.expect("the manifest process ended");
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Its output readers may still be finishing what it wrote before it
    // ended; the deadline still applies.
    while out.is_none() || err.is_none() {
        if started.elapsed() > timeout {
            stop_manifest(&mut child, &mut guard, true);
            return Err(format!(
                "its output was still open {} s after it started",
                timeout.as_secs_f32()
            ));
        }
        receive(&out_rx, &mut out, deadline);
        receive(&err_rx, &mut err, deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = out.unwrap_or_default();
    let err = err.unwrap_or_default();
    if out.len() as u64 > MANIFEST_OUTPUT {
        stop_manifest(&mut child, &mut guard, true);
        return Err(format!(
            "it printed more than {} MiB of manifest",
            MANIFEST_OUTPUT / (1024 * 1024)
        ));
    }
    if orphaned {
        return Err(
            "it ended, but its output was still open: something it started holds it".into(),
        );
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
    let wire: ManifestWire =
        serde_json::from_slice(&out).map_err(|error| format!("it printed no manifest: {error}"))?;
    match wire.manifest {
        0 => Err(
            "it prints a manifest of format 0, this host reads format 1: upgrade the SDK".into(),
        ),
        format if format > MANIFEST_FORMAT => Err(format!(
            "it prints a manifest of format {format}, this host reads up to {MANIFEST_FORMAT}: upgrade rutis-host"
        )),
        _ => {
            // Only a manifest that is used leaves what it started alone.
            if let Some(guard) = guard.as_mut() {
                guard.disarm();
            }
            Ok(wire.rest)
        }
    }
}

/// Whether the manifest process `child` ended, without reaping it: it stays
/// a child process until [`Child::wait`](std::process::Child::wait), so the
/// process group named by its pid cannot be another's when it is killed to
/// release a descendant's pipes.
#[cfg(unix)]
fn exited_without_reaping(child: &std::process::Child) -> std::io::Result<bool> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    match result {
        0 => Ok(unsafe { info.si_pid() } != 0),
        _ => Err(std::io::Error::last_os_error()),
    }
}

/// Take what a reader thread sent, if it is there by `deadline`: whether
/// its output is known now.
fn receive(
    from: &std::sync::mpsc::Receiver<Vec<u8>>,
    into: &mut Option<Vec<u8>>,
    deadline: Instant,
) -> bool {
    use std::sync::mpsc::RecvTimeoutError;
    if into.is_some() {
        return true;
    }
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return false;
    }
    match from.recv_timeout(left.min(Duration::from_millis(50))) {
        Ok(read) => {
            *into = Some(read);
            true
        }
        // Not yet; the caller looks again (and checks its own deadline).
        Err(RecvTimeoutError::Timeout) => false,
        // It panicked before sending: nothing was read.
        Err(RecvTimeoutError::Disconnected) => {
            *into = Some(Vec::new());
            true
        }
    }
}

/// Kill a manifest process's group (what it started with it) and reap it.
/// With `reaped`, it was already waited for: never kill by its pid then,
/// which may name another process; only stop guarding it.
fn stop_manifest(child: &mut std::process::Child, guard: &mut Option<ManifestChild>, reaped: bool) {
    if !reaped {
        if let Some(guard) = guard.as_ref() {
            guard.kill();
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Some(guard) = guard.as_mut() {
        guard.disarm();
    }
}

/// Read `path`'s manifest together with the stamp of the file it describes:
/// the stamp before and after must be the same, or what was printed may be
/// another file's (a rebuild in between). A change is tried once more; one
/// that changes again is not read at all. A file that is gone after printing
/// its manifest leaves that manifest without a stamp: the start will not
/// find it either, and reports it gone then.
fn read_manifest_stamped(path: &Path) -> (Result<BinaryManifest, String>, Option<Stamp>) {
    let mut tried_again = false;
    loop {
        let Some(before) = stamp(path) else {
            return (Err("there is no such file".into()), None);
        };
        let manifest = read_manifest(path);
        match stamp(path) {
            Some(after) if after == before => return (manifest, Some(after)),
            Some(_) if !tried_again => tried_again = true,
            Some(after) => {
                return (
                    Err("the file changed while its manifest was read".into()),
                    Some(after),
                );
            }
            None => {
                return (
                    match manifest {
                        Ok(manifest) => Ok(manifest),
                        // It could not be read, and its file is gone: that
                        // is the report that explains why.
                        Err(_) => Err("there is no such file".into()),
                    },
                    None,
                );
            }
        }
    }
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
                        let (manifest, observed) = read_manifest_stamped(&path);
                        Binary {
                            path: path.clone(),
                            runtime: runtime.clone(),
                            listed,
                            // The file as it was when its manifest was read,
                            // not before it (#539).
                            stamp: observed.unwrap_or(current),
                            manifest: manifest.map(Arc::new),
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

    /// Read the binary of `runtime` again, for a start: its path and what
    /// it starts with. Nothing is recorded until the start
    /// ([`BinaryResolver::launched`]): a runtime still running keeps its
    /// own meanwhile. A failure names the file it failed on, or the file
    /// the runtime last had when it is gone.
    fn read(&self, runtime: &str) -> Result<(PathBuf, Launched), StartFailure> {
        let last = self
            .state
            .lock()
            .unwrap()
            .scanned
            .values()
            .find(|binary| binary.runtime == runtime)
            .map(|binary| binary.path.clone());
        let binary = self
            .scan(Force::Runtime(runtime))
            .into_iter()
            .find(|binary| binary.runtime == runtime)
            .ok_or_else(|| (last, format!("there is no {} runtime {runtime}", L::TITLE)))?;
        let path = Some(binary.path.clone());
        let manifest = binary.manifest.clone().map_err(|why| (path, why))?;
        let launched = Launched {
            stamp: binary.stamp,
            manifest,
        };
        Ok((binary.path, launched))
    }

    /// Record `launched` as the manifest the runtime `runtime` started
    /// with; the rows resolved to it resolve again.
    fn launched(&self, runtime: &str, launched: Launched) {
        let mut state = self.state.lock().unwrap();
        state.launched.insert(runtime.to_owned(), launched);
        let resolved = state.resolved.get(runtime).cloned().unwrap_or_default();
        state
            .stale
            .entry(runtime.to_owned())
            .or_default()
            .extend(resolved);
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
                        )));
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
    /// Its process, to wait for it to be gone, or kill it.
    processes: Processes,
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

/// A start that failed before there was a slot to watch: the file it
/// failed on (`None` when it was already gone) and why. It stays Stopped
/// until that file changes or [`BinaryRuntimesHandle::restart`] asks again.
struct FailedStart {
    binary: Option<PathBuf>,
    stamp: Option<Stamp>,
    why: String,
}

impl FailedStart {
    fn of(binary: Option<PathBuf>, why: String) -> Self {
        let stamp = binary.as_deref().and_then(stamp);
        Self { binary, stamp, why }
    }

    /// Whether the file this failed on changed (or came back).
    fn retry(&self) -> bool {
        self.binary.as_deref().and_then(stamp) != self.stamp
    }
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

type StartFailure = (Option<PathBuf>, String);
type Prepared<L> = Result<(Ctx, BinaryRuntime<L>, PathBuf, Launched), StartFailure>;

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
    /// Whether each line of that output starts with `[<runtime>] `.
    prefix_output: bool,
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
    /// Starts that failed, and the file they failed on: a failed start is
    /// not retried on every sweep (every 100 ms), only when the file
    /// changes or a caller asks for a restart.
    failed: HashMap<String, FailedStart>,
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
                    prefix_output: false,
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

    /// Start each line a runtime process writes with `[<runtime>] `
    /// (`[go-weather] `), when its output is passed on a line at a time
    /// ([`RuntimeStdio::Lines`]): one plugin runs in each process, so the
    /// runtime's name says which wrote it. Off by default.
    pub fn prefix_output(self, prefix: bool) -> Self {
        self.inner.settings.lock().unwrap().prefix_output = prefix;
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
        // Asked for by hand: a failure kept for the same file is tried again.
        self.inner.slots.lock().unwrap().failed.remove(name);
        // In a task: dropped half way, the restart would free the name while
        // the old runtime is still stopping, and the next start would run a
        // second process under it meanwhile (#307).
        // Its error is logged there too: a caller that gave up never sees it.
        let inner = self.inner.clone();
        let runtime = name.to_owned();
        tokio::spawn(async move {
            let result = inner.restart_now(&runtime, busy).await;
            if let Err(error) = &result {
                eprintln!("runtime {runtime}: cannot restart: {error}");
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
                    None => match slots.failed.get(&binary.runtime) {
                        Some(failed) => (
                            BinaryRuntimeState::Stopped(failed.why.clone()),
                            Some(binary.stamp.clone()) != failed.stamp,
                        ),
                        None => (BinaryRuntimeState::NotStarted, false),
                    },
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
        states.extend(
            slots
                .failed
                .iter()
                .filter(|(name, _)| !slots.running.contains_key(*name))
                .map(|(name, failed)| (name.clone(), RuntimeState::Down(failed.why.clone()))),
        );
        states.sort_by(|a, b| a.0.cmp(&b.0));
        states
    }

    /// [`BinaryRuntimesHandle::states`], each with a number that grows
    /// whenever the runtime starts again (0 for one that did not start): a
    /// runtime seen down twice, with a larger number the second time, came
    /// back and went down again, however short it ran.
    pub fn states_started(&self) -> Vec<(String, RuntimeState, u64)> {
        let slots = self.inner.slots.lock().unwrap();
        let mut states: Vec<(String, RuntimeState, u64)> = slots
            .running
            .iter()
            .map(|(name, slot)| (name.clone(), slot.handle.state(), slot.generation))
            .collect();
        states.extend(
            slots
                .failed
                .iter()
                .filter(|(name, _)| !slots.running.contains_key(*name))
                .map(|(name, failed)| (name.clone(), RuntimeState::Down(failed.why.clone()), 0)),
        );
        states.sort_by(|a, b| a.0.cmp(&b.0));
        states
    }
}

/// Stop a runtime taken out of the slots, and return once its process is
/// gone: another one may start under its name then (#307). Its cleanups
/// get the shutdown deadline and a margin; a runtime still stopping by
/// then is killed, and the rest of its stop, which no longer waits for the
/// process, is waited for, so it withdraws nothing of the next one's.
async fn stop(runtime: &str, slot: Slot) {
    let limit = shutdown_deadline() + STOP_MARGIN;
    let disposed = slot.view.dispose_with_timeout(limit).await;
    let timed_out = matches!(
        disposed,
        Err(rutis::host::DisposeWaitError::TimedOut { .. })
    );
    if let Err(error) = &disposed {
        eprintln!("runtime {runtime}: stopping: {error}");
    }
    // Disposed, the process ends once its channel closes, or the transport
    // kills it; here it is killed at the latest.
    let exited = !timed_out
        && tokio::time::timeout(STOP_EXIT, slot.processes.ended())
            .await
            .is_ok();
    if exited {
        return;
    }
    slot.processes.kill();
    if tokio::time::timeout(STOP_EXIT, slot.processes.ended())
        .await
        .is_err()
    {
        eprintln!("runtime {runtime}: its process still runs after it was killed");
    }
    eprintln!(
        "runtime {runtime}: did not stop within {:.1}s; killed it",
        limit.as_secs_f64()
    );
    if timed_out {
        // Its process gone, its calls into it fail, and its stop ends.
        if let Err(error) = slot.view.dispose_with_timeout(STOP_EXIT).await {
            eprintln!("runtime {runtime}: stopping after killing it: {error}");
        }
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
            let quiet = slots
                .failed
                .get(runtime)
                .is_some_and(|failed| !failed.retry());
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
                        // Running: it keeps its process until restarted
                        // (`replace`, `restart`).
                        None => None,
                        // Crashed: only a new binary is worth another try
                        // here; its restart policy may start it again. A
                        // binary that disappeared is not "changed".
                        Some(_) => {
                            let now = stamp(&slot.binary);
                            (now.is_some() && now != slot.stamp && !quiet)
                                .then_some(Transition::Restart)
                        }
                    }
                }
                None => {
                    if let Some(failed) = slots.failed.get(runtime) {
                        if !failed.retry() {
                            return;
                        }
                        slots.failed.remove(runtime);
                    }
                    Some(Transition::Start)
                }
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
                if let Err((binary, why)) = self.start(runtime) {
                    self.note_failed(runtime, binary, why.clone());
                    eprintln!("runtime {runtime}: cannot start: {why}");
                }
            }
            Some(Transition::Restart) => {
                let busy = Busy::held(self, runtime);
                let inner = self.clone();
                let runtime = runtime.to_owned();
                tokio::spawn(async move {
                    if let Err(error) = inner.restart_now(&runtime, busy).await {
                        eprintln!("runtime {runtime}: cannot restart: {error}");
                    }
                });
            }
        }
    }

    /// Remember a failed start until its file changes or a caller asks
    /// again, and show it in [`BinaryRuntimesHandle::states`].
    fn note_failed(&self, runtime: &str, binary: Option<PathBuf>, why: String) {
        self.slots
            .lock()
            .unwrap()
            .failed
            .insert(runtime.to_owned(), FailedStart::of(binary, why));
    }

    /// Read the binary and make its runtime, before anything of an old
    /// runtime is dropped: a restart that cannot start keeps the old one.
    fn prepare(&self, runtime: &str) -> Prepared<L> {
        let ctx = self
            .ctx
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| (None, format!("the {} runtimes are not running", L::TITLE)))?;
        let (binary, launched) = self.resolver.read(runtime)?;
        // Read once: a guard taken in the literal below lives to its end.
        let settings = *self.settings.lock().unwrap();
        let mut runtime_plugin = BinaryRuntime::<L> {
            label: format!("{runtime} ({})", L::PREFIX),
            runtime: runtime.to_owned(),
            binary: binary.clone(),
            stamp: Some(launched.stamp.clone()),
            project: self.project.clone(),
            output: settings.output,
            prefix_output: settings.prefix_output,
            resolver: self.resolver.clone(),
            first: Mutex::new(None),
        };
        let local = runtime_plugin.local();
        runtime_plugin.first = Mutex::new(Some(local));
        Ok((ctx, runtime_plugin, binary, launched))
    }

    fn install(
        &self,
        runtime: &str,
        ctx: Ctx,
        runtime_plugin: BinaryRuntime<L>,
        binary: PathBuf,
        launched: Launched,
    ) {
        let stamp = Some(launched.stamp.clone());
        self.resolver.launched(runtime, launched);
        let (handle, processes) = {
            let first = runtime_plugin.first.lock().unwrap();
            let local = first.as_ref().expect("the first runtime was just made");
            (local.handle(), local.processes())
        };
        let view = ctx.plugin(runtime_plugin);
        let mut slots = self.slots.lock().unwrap();
        slots.generation += 1;
        let generation = slots.generation;
        slots.failed.remove(runtime);
        slots.running.insert(
            runtime.to_owned(),
            Slot {
                view,
                handle,
                processes,
                stamp,
                binary,
                last_used: Instant::now(),
                stopped: None,
                started: tokio::time::Instant::now(),
                generation,
            },
        );
    }

    /// Stop `runtime` and start it again, holding its busy mark. The binary
    /// is read before the old runtime is stopped: one that cannot start
    /// leaves the old runtime running. Both processes use the runtime's
    /// name, so the new one starts only once the old one is gone, and
    /// records what it started with only then.
    async fn restart_now(self: Arc<Self>, runtime: &str, _busy: Busy<L>) -> Result<(), String> {
        let (ctx, plugin, binary, launched) = self.prepare(runtime).map_err(|(binary, why)| {
            self.note_failed(runtime, binary, why.clone());
            why
        })?;
        let previous = self.slots.lock().unwrap().running.remove(runtime);
        if let Some(previous) = previous {
            stop(runtime, previous).await;
        }
        self.resolver.stopped(runtime);
        self.install(runtime, ctx, plugin, binary, launched);
        Ok(())
    }

    fn start(&self, runtime: &str) -> Result<(), StartFailure> {
        let (ctx, plugin, binary, launched) = self.prepare(runtime)?;
        self.install(runtime, ctx, plugin, binary, launched);
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
        // The policy may have changed while this restart waited.
        if supervision.policy != RestartPolicy::OnFailure {
            return;
        }
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
                eprintln!("runtime {runtime}: cannot restart: {error}");
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
                    stop(&name, previous).await;
                }
                inner.resolver.stopped(&name);
            });
        }
    }
}

impl<L: BinaryLanguage> DynPlugin for BinaryRuntimes<L> {
    fn name(&self) -> &str {
        &self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
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
                async move {
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
                }
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
    /// The binary as it was when its manifest was read: it must still be
    /// that file when its process starts (#539).
    stamp: Option<Stamp>,
    project: PathBuf,
    /// Its process's standard output and error.
    output: (RuntimeStdio, RuntimeStdio),
    /// Whether each line of that output starts with its name.
    prefix_output: bool,
    resolver: Arc<BinaryResolver<L>>,
    /// The runtime whose handle the slot watches, for the first start.
    first: Mutex<Option<LocalRuntime>>,
}

impl<L: BinaryLanguage> BinaryRuntime<L> {
    fn local(&self) -> LocalRuntime {
        let local = L::launch(&self.binary, &self.project)
            .named(self.runtime.clone())
            .stdout(self.output.0)
            .stderr(self.output.1);
        match self.prefix_output {
            true => local.output_prefix(&self.runtime),
            false => local,
        }
    }
}

impl<L: BinaryLanguage> DynPlugin for BinaryRuntime<L> {
    fn name(&self) -> &str {
        &self.label
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            // The manifest was read from this file as it was; a rebuild
            // since would start a process whose rows are another's (#539).
            if stamp(&self.binary) != self.stamp {
                return Err(rutis::Error::from(ErrorKind::PluginFailed(
                    std::io::Error::other(format!(
                        "{} changed after its manifest was read",
                        self.binary.display()
                    ))
                    .into(),
                )));
            }
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

    /// A failed start is retried only when its file changes, not on every
    /// sweep.
    #[test]
    fn a_failed_start_waits_for_its_file_to_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime");
        std::fs::write(&path, b"one").unwrap();
        let failed = FailedStart::of(Some(path.clone()), "cannot start".into());
        assert!(!failed.retry());
        std::fs::write(&path, b"another").unwrap();
        assert!(failed.retry());
        std::fs::remove_file(&path).unwrap();
        assert!(failed.retry());
    }

    /// A failed start whose file was already gone stays stopped until a
    /// caller asks again.
    #[test]
    fn a_failed_start_without_a_file_is_not_retried() {
        let failed = FailedStart::of(None, "there is no runtime".into());
        assert!(!failed.retry());
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

    /// Whether `pid` is still a process.
    #[cfg(unix)]
    fn running(pid: i32) -> bool {
        // SAFETY: kill(pid, 0) checks that the process exists and signals
        // nothing.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// The pid a script wrote, once it is there.
    #[cfg(unix)]
    fn written_pid(path: &Path) -> i32 {
        for _ in 0..500 {
            if let Ok(pid) = std::fs::read_to_string(path)
                .unwrap_or_default()
                .trim()
                .parse()
            {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("{}: no pid was written", path.display());
    }

    /// A manifest that leaves a child holding its output is not waited for
    /// forever: the read ends at its deadline and kills what the manifest
    /// started (#538).
    #[cfg(unix)]
    #[test]
    fn a_manifest_that_leaves_its_output_open_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let path = manifest_script(
            dir.path(),
            &format!(
                "sleep 300 &\necho $! > '{}'\nprintf '{{}}'",
                pidfile.display()
            ),
        );
        let started = Instant::now();
        let error = read_manifest_with_timeout(&path, Duration::from_millis(300)).unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "took {:?}",
            started.elapsed()
        );
        assert!(
            error.contains("output was still open") || error.contains("did not print"),
            "{error}"
        );
        let pid = written_pid(&pidfile);
        for _ in 0..100 {
            if !running(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the background process {pid} survived the manifest read");
    }

    /// The process a manifest is read in has no descriptor above 2 of
    /// this one's: fd 3, opened here, is not there (#538).
    ///
    /// The check runs in a process of its own, started here with fd 3 open:
    /// swapping this process's fd 3 would reach into the other tests sharing
    /// it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_manifest_does_not_inherit_fd_three() {
        const CHILD: &str = "RUTIS_TEST_FD3_CHILD";
        const SCRIPT: &str = "RUTIS_TEST_FD3_SCRIPT";
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_script(
            dir.path(),
            "if [ -e /proc/self/fd/3 ]; then printf 'fd 3 survived'; else printf '{}'; fi",
        );
        if std::env::var_os(CHILD).is_some() {
            let path = PathBuf::from(std::env::var_os(SCRIPT).unwrap());
            assert!(
                std::fs::metadata("/proc/self/fd/3").is_ok(),
                "the process the check runs in has no fd 3"
            );
            assert_eq!(read_manifest(&path).unwrap().plugin_api, 0);
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let status = Command::new("/bin/sh")
            .arg("-c")
            // fd 3 is the script (inheritable), then the test binary runs
            // this test again with fd 3 still open.
            .arg(
                "exec 3<&- 2>/dev/null; exec 3<\"$1\" || exit 1; \
                 exec \"$2\" --exact runtime::binary::tests::a_manifest_does_not_inherit_fd_three \
                 --nocapture",
            )
            .arg("sh")
            .arg(&path)
            .arg(&exe)
            .env(CHILD, "1")
            .env(SCRIPT, &path)
            .status()
            .unwrap();
        assert!(status.success(), "the check in its own process failed");
    }

    /// A format this host does not read is refused whole, not half-read.
    #[cfg(unix)]
    #[test]
    fn a_manifest_of_a_newer_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_script(dir.path(), "printf '{\"manifest\": 2}'");
        let error = read_manifest(&path).unwrap_err();
        assert!(error.contains("format 2"), "{error}");
        assert!(error.contains("reads up to 1"), "{error}");
        assert!(error.contains("upgrade rutis-host"), "{error}");
    }

    /// Format 0 is no format: refused too.
    #[cfg(unix)]
    #[test]
    fn a_manifest_of_format_zero_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_script(dir.path(), "printf '{\"manifest\": 0}'");
        let error = read_manifest(&path).unwrap_err();
        assert!(error.contains("format 0"), "{error}");
    }

    /// Format 1 and a manifest without a format are read.
    #[cfg(unix)]
    #[test]
    fn a_manifest_of_format_one_or_none_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_script(dir.path(), "printf '{\"manifest\": 1, \"pluginApi\": 2}'");
        let manifest = read_manifest(&path).unwrap();
        assert_eq!(manifest.plugin_api, 2);
        let path = manifest_script(dir.path(), "printf '{\"pluginApi\": 3}'");
        let manifest = read_manifest(&path).unwrap();
        assert_eq!(manifest.plugin_api, 3);
    }

    /// A file that changes while its manifest is read is read once more; a
    /// file that changes again is reported, not run (#539).
    #[cfg(unix)]
    #[test]
    fn a_file_that_changes_while_its_manifest_is_read_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        // Touching the script changes its stamp without changing what it
        // prints, so every read sees a file that moved under it.
        let path = manifest_script(dir.path(), "printf '{}'\ntouch \"$0\"");
        let (manifest, observed) = read_manifest_stamped(&path);
        let error = manifest.unwrap_err();
        assert!(
            error.contains("changed while its manifest was read"),
            "{error}"
        );
        assert!(observed.is_some());
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
