//! `rutis-host test` (`docs/design-platform-cleanup-2026-10-28.md` §7.5): a
//! host with no rows of its own, one session with the process that started
//! it, and the plugin `rutis-host/test`.
//!
//! - The session: on the command's standard input and output, framed as a
//!   local runtime's channel is (one JSON message per line; [`stdio`]), or,
//!   with `--listen`, the connection of a `rutis-host/listen` row on a
//!   listener of that address. Its connection row is [`ROW`]: the rows
//!   `test:<plugin>` run `<plugin>` at its far end (`run`).
//! - The plugin `rutis-host/test` uses the loader's public API alone and
//!   provides the service `rutis-host/test` ([`Harness`]): `load(name,
//!   config)`, `update(id, config)`, `unload(id)`, `reported(id)`.
//!
//! On standard input and output, the plugin is the connection row itself
//! (`test`); it says when the session ended, and the host stops.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, DynPlugin, Effect, ErrorKind, FiberState, PluginFactory};
use rutis::{Ctx, Error};
use rutis_bridge::Channel;
use rutis_loader::edit::{Editable, NewEntry, Version};
use rutis_loader::report::{EntryInfo, EntryStatus};
use rutis_loader::{Builtins, Layer, Loader, LoaderError, Patch};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::connect::session;

pub const NAME: &str = "rutis-host/test";
/// The connection row: the plugins loaded run at its far end.
pub const ROW: &str = "test";
/// The listener `--listen` adds.
pub const LISTENER: &str = "test";
/// The row of `rutis-host/test` when the connection row is a listen row.
pub const HARNESS_ROW: &str = "harness";

/// Where the session is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// Not greeted yet.
    Starting,
    /// The far end greeted.
    Connected,
    /// It ended, or its handshake failed: why, when it failed.
    Ended(Option<String>),
}

/// What `rutis-host test` and its plugin share.
#[derive(Clone)]
pub struct Test {
    channel: Arc<Mutex<Option<Channel>>>,
    session: watch::Sender<Session>,
    state: Arc<State>,
}

#[derive(Default)]
struct State {
    /// The rows `load` added and has not unloaded.
    loaded: Mutex<HashSet<String>>,
    /// What the host reported of each row: (row, line).
    reported: Mutex<Vec<(String, String)>>,
}

impl Test {
    /// On `channel`, or (`None`) on a listen row's connection.
    pub fn new(channel: Option<Channel>) -> Self {
        Self {
            channel: Arc::new(Mutex::new(channel)),
            session: watch::Sender::new(Session::Starting),
            state: Arc::default(),
        }
    }

    /// Register `rutis-host/test`.
    pub fn register(&self, builtins: &mut Builtins) {
        builtins.register_raw(
            NAME,
            self.clone(),
            Some(json!({ "type": "object", "additionalProperties": false })),
        );
    }

    /// Note an error the host reports: a listener of a row that failed, or
    /// a filter of a row's `emit`, for `reported`.
    pub fn observe(&self, error: &ErrorKind) {
        let row = match error {
            ErrorKind::ListenerFailed { listener: row, .. }
            | ErrorKind::FilterFailed { emitter: row, .. } => row.strip_prefix("row "),
            _ => None,
        };
        let (Some(row), Some(line)) = (row, rutis_bridge::events::report_line(error)) else {
            return;
        };
        self.state
            .reported
            .lock()
            .unwrap()
            .push((row.to_owned(), line));
    }

    /// The session, as it changes.
    pub fn session(&self) -> watch::Receiver<Session> {
        self.session.subscribe()
    }

    /// Load `name` at the far end ([`Harness::load`]).
    pub async fn load(&self, loader: &Loader, name: &str, config: Value) -> Result<String, String> {
        load(loader, &self.state, name, config).await
    }
}

impl PluginFactory<Value> for Test {
    fn name(&self) -> &str {
        NAME
    }

    fn validate_config(&self, config: &Value) -> Result<(), Error> {
        match config {
            Value::Null => Ok(()),
            Value::Object(fields) if fields.is_empty() => Ok(()),
            _ => Err(Error::from(ErrorKind::PluginFailed(
                format!("{NAME} takes no config").into(),
            ))),
        }
    }

    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        self.validate_config(config)?;
        Ok(Box::new(TestPlugin(self.clone())))
    }
}

struct TestPlugin(Test);

impl DynPlugin for TestPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let loader = ctx.get::<Loader>().ok_or_else(|| {
                Error::from(ErrorKind::PluginFailed(
                    format!("{NAME} runs as a row of the loader").into(),
                ))
            })?;
            ctx.provide(Harness {
                loader: (*loader).clone(),
                state: self.0.state.clone(),
            })?;
            let Some(channel) = self.0.channel.lock().unwrap().take() else {
                return Ok(Effect::Done);
            };
            let row = crate::connect::row_id(ctx, NAME)?;
            if row != ROW {
                return Err(Error::from(ErrorKind::PluginFailed(
                    format!("{NAME} on standard input and output is the row {ROW}").into(),
                )));
            }
            let (own, said) = (ctx.clone(), self.0.session.clone());
            let task = tokio::spawn(async move {
                let ended = match session::handshake(&own, channel).await {
                    Err(failed) => Some(failed.reason),
                    Ok(greeted) => {
                        let installed = session::install(&own, greeted);
                        said.send_replace(Session::Connected);
                        installed.unloaded().await;
                        None
                    }
                };
                said.send_replace(Session::Ended(ended));
            });
            ctx.effect(move || async move {
                // The session, a child of this row, unloads with it.
                task.abort();
                let _ = task.await;
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// The service `rutis-host/test`: rows of plugins run at the far end of
/// the session, through the loader.
pub struct Harness {
    loader: Loader,
    state: Arc<State>,
}

#[rutis::service(name = "rutis-host/test")]
impl Harness {
    /// Run `name` (as the far end knows it: a file, a package, a module)
    /// at the far end, with `config`, as a row of its own; its id. Fails,
    /// adding no row, when the row cannot start.
    pub async fn load(&self, name: String, config: Option<Value>) -> Result<String, String> {
        load(
            &self.loader,
            &self.state,
            &name,
            config.unwrap_or(Value::Null),
        )
        .await
    }

    /// Give the row `id` that `load` added `config`, as a row's config
    /// changes. Fails when the plugin refuses it (the row keeps its config)
    /// or fails with it.
    pub async fn update(&self, id: String, config: Value) -> Result<(), String> {
        loaded(&self.state, &id)?;
        self.loader
            .update(&id, config)
            .await
            .map_err(|error| failed(&id, &error))?;
        let info = settled(&self.loader, &id).await?;
        if let Some(rejected) = &info.rejected {
            return Err(format!("{id}: {rejected}"));
        }
        refused(&info).map_or(Ok(()), Err)
    }

    /// Remove the row `id` that `load` added: its plugin unloads at the far
    /// end. Returns once it unloaded.
    pub async fn unload(&self, id: String) -> Result<(), String> {
        loaded(&self.state, &id)?;
        self.loader
            .remove(&id)
            .await
            .map_err(|error| failed(&id, &error))?;
        self.state.loaded.lock().unwrap().remove(&id);
        Ok(())
    }

    /// What the host reported of the row `id`, in order: each listener of
    /// it that failed, each filter of its `emit` that failed, as the host
    /// prints them.
    pub fn reported(&self, id: String) -> Vec<String> {
        self.state
            .reported
            .lock()
            .unwrap()
            .iter()
            .filter(|(row, _)| *row == id)
            .map(|(_, line)| line.clone())
            .collect()
    }
}

/// The rows `rutis-host test` starts with: the connection row (and, on a
/// listen row, `rutis-host/test` next to it). The layer is editable, for
/// `load`.
pub async fn start(loader: &Loader, listening: bool) -> Result<(), String> {
    let rows = match listening {
        false => vec![json!({ "id": ROW, "name": NAME })],
        true => vec![
            json!({ "id": ROW, "name": crate::connect::LISTEN, "config": { "listener": LISTENER } }),
            json!({ "id": HARNESS_ROW, "name": NAME }),
        ],
    };
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }]))
        .map_err(|error| format!("rows: {error}"))?;
    let report = loader
        .reconcile(
            vec![Layer::new(ROW, patches)],
            Some(Editable::new(ROW, Version(String::new()))),
        )
        .await
        .map_err(|error| error.to_string())?;
    // A row of these failing (a listen row without its token) fails it.
    match report.failures.is_empty() {
        true => Ok(()),
        false => Err(report
            .failures
            .iter()
            .map(|failure| failure.error.clone())
            .collect::<Vec<_>>()
            .join("\n")),
    }
}

async fn load(loader: &Loader, state: &State, name: &str, config: Value) -> Result<String, String> {
    if name.is_empty() {
        return Err("load needs the name of a plugin".into());
    }
    let id = free_id(loader, name);
    let entry = NewEntry {
        id: Some(id.clone()),
        name: format!("{ROW}:{name}"),
        config,
        ..NewEntry::default()
    };
    loader
        .create(entry, None, None)
        .await
        .map_err(|error| failed(&id, &error))?;
    let info = settled(loader, &id).await?;
    if let Some(why) = refused(&info) {
        let _ = loader.remove(&id).await;
        return Err(why);
    }
    state.loaded.lock().unwrap().insert(id.clone());
    Ok(id)
}

/// A row id for the plugin `name`: its file's name without the extension
/// (`/plugins/weather.mjs` is `weather`), else the name itself; `-2`,
/// `-3`… while one is taken.
fn free_id(loader: &Loader, name: &str) -> String {
    let base = base_id(name);
    let mut id = base.to_owned();
    let mut next = 2;
    while loader.get(&id).is_some() {
        id = format!("{base}-{next}");
        next += 1;
    }
    id
}

fn base_id(name: &str) -> &str {
    let base = match name.rsplit(['/', '\\']).next() {
        Some(file) if file.len() < name.len() => file.split('.').next().unwrap_or(file),
        _ => name,
    };
    if base.is_empty() {
        "plugin"
    } else {
        base
    }
}

fn loaded(state: &State, id: &str) -> Result<(), String> {
    match state.loaded.lock().unwrap().contains(id) {
        true => Ok(()),
        false => Err(format!("no row {id} was loaded")),
    }
}

/// The loader refused an edit of `id`: a row that failed says so as its
/// line does.
fn failed(id: &str, error: &LoaderError) -> String {
    match error {
        LoaderError::ApplyFailed { failures } => failures
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        error => format!("{id}: {error}"),
    }
}

/// The row once it is no longer starting.
async fn settled(loader: &Loader, id: &str) -> Result<EntryInfo, String> {
    loop {
        let info = loader
            .get(id)
            .ok_or_else(|| format!("no row {id} was loaded"))?;
        let starting = matches!(&info.status, EntryStatus::Running(snapshot) if snapshot.state == FiberState::Loading);
        let Some(view) = info.view.clone().filter(|_| starting) else {
            return Ok(info);
        };
        let mut states = view.watch();
        while states.borrow_and_update().state == FiberState::Loading {
            if states.changed().await.is_err() {
                break;
            }
        }
    }
}

/// Why the row cannot run, when it cannot: its line.
fn refused(info: &EntryInfo) -> Option<String> {
    let cannot = match &info.status {
        EntryStatus::Unresolved(_) => true,
        EntryStatus::Running(snapshot) => snapshot.state == FiberState::Failed,
        _ => false,
    };
    cannot.then(|| format!("{}: {}", info.id, crate::status::describe(&info.status)))
}

/// The session's channel on this process's standard input and output, one
/// message per line. From now on what writes to standard output writes to
/// standard error, and standard input reads nothing: only the session uses
/// the two.
#[cfg(unix)]
pub fn stdio() -> std::io::Result<Channel> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let dup = |fd| match unsafe { libc::dup(fd) } {
        -1 => Err(std::io::Error::last_os_error()),
        // SAFETY: a new descriptor, owned by nothing else.
        new => Ok(unsafe { OwnedFd::from_raw_fd(new) }),
    };
    let (read, write, shut) = (dup(0)?, dup(1)?, dup(0)?);
    let null = std::fs::File::open("/dev/null")?;
    // SAFETY: replacing this process's own standard descriptors.
    if unsafe { libc::dup2(2, 1) } == -1 || unsafe { libc::dup2(null.as_raw_fd(), 0) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(rutis_bridge::pipe::framed(
        std::fs::File::from(read),
        std::fs::File::from(write),
        Arc::new(Shut(shut)),
    ))
}

/// Wakes a read blocked on a socket (when the far end gave one socket for
/// both), and ends it both ways. A pipe needs nothing: the channel lets go
/// of its writing end, and the far end, seeing the end, closes its own.
#[cfg(unix)]
struct Shut(std::os::fd::OwnedFd);

#[cfg(unix)]
impl rutis_bridge::channel::Closer for Shut {
    fn close(&self, _: rutis_bridge::Close) {
        use std::os::fd::AsRawFd;
        // SAFETY: a descriptor this owns; ENOTSOCK on a pipe is harmless.
        unsafe { libc::shutdown(self.0.as_raw_fd(), libc::SHUT_RDWR) };
    }
}

/// The session's channel on standard input and output.
#[cfg(not(unix))]
pub fn stdio() -> std::io::Result<Channel> {
    struct Nothing;
    impl rutis_bridge::channel::Closer for Nothing {
        fn close(&self, _: rutis_bridge::Close) {}
    }
    Ok(rutis_bridge::pipe::framed(
        std::io::stdin(),
        std::io::stdout(),
        Arc::new(Nothing),
    ))
}

#[cfg(test)]
mod tests {
    use super::base_id;

    #[test]
    fn a_rows_id_is_its_files_name() {
        assert_eq!(base_id("/plugins/weather.mjs"), "weather");
        assert_eq!(base_id("weather_plugin"), "weather_plugin");
        assert_eq!(base_id("@ana/wordcount"), "wordcount");
    }
}
