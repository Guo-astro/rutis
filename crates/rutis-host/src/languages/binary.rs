//! What the compiled languages (Go, Swift) share: `runtimes.<key>` names
//! binaries, one runtime each, started when a row uses them (or with the
//! host), their rows `<key>:<plugin>` and `<key>-<binary>:<plugin>`.
//!
//! Such a language overrides [`Language::start`] with [`start`]: the
//! binaries' resolver joins the chain now, their runtimes mount once the
//! loader runs ([`super::Later`]), and the host keeps a [`BinaryHost`] for
//! `check`, `dev` and the status lines. `rutis-host dev` rebuilds a project
//! of it with its own build ([`DevBuild`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rutis::host::BoxFuture;
use rutis::Ctx;
use rutis_bridge::runtime::{RuntimeState, Stdio};
use rutis_loader::runtime::binary::{
    Binaries, BinaryFile, BinaryLanguage, BinaryResolver, BinaryRuntimeInfo, BinaryRuntimes,
    BinaryRuntimesHandle, Spawn, Spawned,
};
use rutis_loader::ServiceCatalog;

use super::{Language, Mount, LANGUAGES};
use crate::config::{BinaryRuntime, BinaryStart, HostConfig, Restart};
use crate::connect::SpawnPlugin;
use crate::host::Error;
use crate::restart::Starts;

/// A compiled language's binaries and runtimes in a running host.
#[derive(Clone)]
pub struct BinaryHost {
    /// Its key under `runtimes` (`go`, `swift`).
    pub key: &'static str,
    /// What `runtimes.<key>.restart` says, for each of its runtimes.
    pub restart: Restart,
    /// Each start of its runtimes, for supervising them.
    pub starts: Starts,
    side: Arc<dyn Side>,
}

impl BinaryHost {
    /// The language's name in messages (`Go`, `Swift`).
    pub fn title(&self) -> &'static str {
        self.side.title()
    }

    /// The plugin API its binaries may declare at most.
    pub fn plugin_api(&self) -> u64 {
        self.side.plugin_api()
    }

    /// Every binary, with its manifest or why it has none.
    pub fn binaries(&self) -> Vec<BinaryFile> {
        self.side.binaries()
    }

    /// The candidate files skipped, and why.
    pub fn diagnostics(&self) -> Vec<String> {
        self.side.diagnostics()
    }

    /// The runtime `runtime` runs `path` from its next start on.
    pub fn replace(&self, runtime: &str, path: &Path) {
        self.side.replace(runtime, path)
    }

    /// Stop the runtime `runtime` and start it again from its binary.
    pub fn restart_runtime<'a>(&'a self, runtime: &'a str) -> BoxFuture<'a, Result<(), String>> {
        self.side.restart(runtime)
    }

    /// The state of each runtime started and not stopped since, by name.
    pub fn states(&self) -> Vec<(String, RuntimeState)> {
        self.side.states()
    }

    /// [`BinaryHost::states`], each with a number that grows whenever the
    /// runtime starts again.
    pub fn states_started(&self) -> Vec<(String, RuntimeState, u64)> {
        self.side.states_started()
    }

    /// Every runtime: those that run, and the binaries not started.
    pub fn runtimes(&self) -> Vec<BinaryRuntimeInfo> {
        self.side.runtimes()
    }
}

/// [`BinaryHost`]'s language-independent face of a [`BinaryResolver`] and
/// its [`BinaryRuntimesHandle`].
trait Side: Send + Sync {
    fn title(&self) -> &'static str;
    fn plugin_api(&self) -> u64;
    fn binaries(&self) -> Vec<BinaryFile>;
    fn diagnostics(&self) -> Vec<String>;
    fn replace(&self, runtime: &str, path: &Path);
    fn restart<'a>(&'a self, runtime: &'a str) -> BoxFuture<'a, Result<(), String>>;
    fn states(&self) -> Vec<(String, RuntimeState)>;
    fn states_started(&self) -> Vec<(String, RuntimeState, u64)>;
    fn runtimes(&self) -> Vec<BinaryRuntimeInfo>;
}

struct Of<L: BinaryLanguage> {
    resolver: Arc<BinaryResolver<L>>,
    runtimes: BinaryRuntimesHandle<L>,
}

impl<L: BinaryLanguage> Side for Of<L> {
    fn title(&self) -> &'static str {
        L::TITLE
    }

    fn plugin_api(&self) -> u64 {
        L::PLUGIN_API
    }

    fn binaries(&self) -> Vec<BinaryFile> {
        self.resolver.binaries()
    }

    fn diagnostics(&self) -> Vec<String> {
        self.resolver.diagnostics()
    }

    fn replace(&self, runtime: &str, path: &Path) {
        self.resolver.replace(runtime, path)
    }

    fn restart<'a>(&'a self, runtime: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(self.runtimes.restart(runtime))
    }

    fn states(&self) -> Vec<(String, RuntimeState)> {
        self.runtimes.states()
    }

    fn states_started(&self) -> Vec<(String, RuntimeState, u64)> {
        self.runtimes.states_started()
    }

    fn runtimes(&self) -> Vec<BinaryRuntimeInfo> {
        self.runtimes.runtimes()
    }
}

/// [`Language::start`] of the compiled language `language` (`L`), whose
/// settings are `settings`: the binaries' resolver now, their runtimes once
/// the loader runs.
pub(super) fn start<L: BinaryLanguage>(
    language: &dyn Language,
    settings: &BinaryRuntime,
    config: &HostConfig,
    mount: &mut Mount<'_>,
) -> Result<(), Error> {
    let key = language.key();
    let resolver = Arc::new(resolver::<L>(key, config, settings, mount.catalog)?);
    mount.resolve(resolver.clone());
    let (project, start, idle) = (settings.project.clone(), settings.start, settings.idle);
    let restart = settings.restart;
    // One plugin in each process: the host starts its lines with the
    // runtime's name (#678).
    let prefix_output = mount.prefix_output;
    let starts = Starts::default();
    mount.later.push(Box::new(move |root: Ctx| {
        Box::pin(async move {
            let mut plugin =
                BinaryRuntimes::<L>::new(resolver.clone(), &project, spawn(starts.clone()))
                    .output(Stdio::Lines, Stdio::Lines)
                    .prefix_output(prefix_output);
            plugin = match start {
                BinaryStart::Eager => plugin.eager(),
                BinaryStart::OnDemand => plugin.idle(Some(Duration::from_secs(idle.unwrap_or(60)))),
            };
            let runtimes = plugin.handle();
            (&root.plugin(plugin))
                .await
                .map_err(|error| format!("the {} runtimes: {error}", L::TITLE))?;
            Ok(Some(BinaryHost {
                key,
                restart,
                starts,
                side: Arc::new(Of { resolver, runtimes }),
            }))
        })
    }));
    Ok(())
}

/// What starts a compiled language's runtime: a `rutis-host/spawn` row,
/// named after it, each start told to `starts`.
fn spawn(starts: Starts) -> Spawn {
    Arc::new(move |name: &str, launcher, anchor: &Path| {
        let plugin = SpawnPlugin::new(name, launcher, anchor);
        starts.started(name, plugin.handle());
        Spawned {
            handle: plugin.handle(),
            processes: plugin.processes(),
            plugin: Box::new(plugin),
        }
    })
}

/// The resolver of the binaries `settings` names. A runtime of them may not
/// take another runtime's name: another language's, or a connection row's.
fn resolver<L: BinaryLanguage>(
    key: &str,
    config: &HostConfig,
    settings: &BinaryRuntime,
    catalog: &ServiceCatalog,
) -> Result<BinaryResolver<L>, Error> {
    settings.validate(key)?;
    let mut binaries = Binaries::new();
    if let Some(dir) = &settings.dir {
        binaries = binaries.dir(dir);
    }
    for binary in &settings.binaries {
        binaries = binaries.file(binary);
    }
    if let Some((binary, name)) = &settings.dev {
        binaries = binaries.file_named(binary, name);
    }
    let reserved = LANGUAGES
        .iter()
        .map(|language| language.key())
        .filter(|other| *other != key)
        .map(str::to_owned)
        .chain(config.rows.iter().filter_map(|row| {
            let connection =
                row["name"] == crate::connect::DIAL || row["name"] == crate::connect::LISTEN;
            connection
                .then(|| row["id"].as_str().map(str::to_owned))
                .flatten()
        }));
    Ok(BinaryResolver::<L>::new(binaries)
        .with_catalog(catalog)
        .reserve(reserved))
}

/// A plugin project of a compiled language as `rutis-host dev` builds it:
/// each build to a new file (`.rutis/<key>/<name>-<n>`), which its runtime
/// then restarts on.
pub trait DevBuild: Send + Sync {
    /// The project's name, as the build's lines say it.
    fn name(&self) -> &str;

    /// The runtime its binary runs as (`go-<name>`, `swift-<name>`).
    fn runtime(&self) -> &str;

    /// Build #`n` without blocking: its binary, or the compiler's errors.
    /// Dropped, it kills the build.
    fn rebuild(&self, n: u32) -> BoxFuture<'_, Result<PathBuf, String>>;

    /// Where build #`n` is.
    fn binary(&self, n: u32) -> PathBuf;

    /// Whether `path`, a project file that changed, is a source or package
    /// file of this language. Any change rebuilds the binary; a change to
    /// another file (a plugin of another language the dev file adds, the
    /// dev file itself) also reloads the other rows.
    fn is_source(&self, path: &Path) -> bool;
}

/// Says that `name` is being built, runs `build`, and on success says how
/// long it took: a first build (dependencies, macros) can take minutes, and
/// whoever waits sees that it is working.
pub(super) fn building<T>(
    name: &str,
    build: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    crate::errln!("{name}: building");
    let started = std::time::Instant::now();
    let built = build()?;
    crate::errln!("{name}: built in {}", seconds(started.elapsed()));
    Ok(built)
}

/// `elapsed` as the build lines say it: `0.4s`, `12.3s`.
pub(super) fn seconds(elapsed: Duration) -> String {
    format!("{:.1}s", elapsed.as_secs_f64())
}

/// Run `command` without blocking, killed when dropped: its output.
pub(super) async fn run(command: std::process::Command) -> std::io::Result<std::process::Output> {
    tokio::process::Command::from(command)
        .kill_on_drop(true)
        .output()
        .await
}
