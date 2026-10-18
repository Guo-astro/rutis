//! The host as the hub of the logical bus (events protocol §4, §5, §7):
//! each runtime session's listeners are remote entries of the kernel's one
//! table, its dispatches run over that table, and it hears which names have
//! listeners elsewhere.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

use rutis::events::{
    BatchFilter, Dispatch, EventOptions, FilterAnswer, ListenerInfo, ListenerKind, Mode,
    PayloadRef, RawTerminal, RemoteCall, RemoteEntry, RemoteListener, RemoteNext, RemoteSyncCall,
    RemoteSyncNext, Scope, TableChange, TableEntry,
};
use rutis::{BoxFuture, CordisError, Ctx, Disposer};
use serde_json::{json, Value as Json};

use super::args::{from_wire, result_from_wire, result_to_wire, to_wire, SessionTranscoder};
use super::errors::{binding, to_cordis, to_session};
use crate::session::rpc::with_caller;
use crate::session::{settle, Connection, Error, Reply, Value, WeakConnection};

/// A table entry's key for presence: its name (or prefix) and whether it is
/// a prefix.
type Key = (String, bool);

/// The logical bus of one kernel bus, as the sessions on it see it.
pub(crate) struct Hub {
    /// The bus's root: holds the bus (and its address) while the hub lives.
    #[allow(dead_code)]
    ctx: Ctx,
    state: Mutex<HubState>,
    watching: Mutex<Option<Disposer>>,
    /// The listener turning `ServiceChanged` into `rutis/service`.
    services: Mutex<Option<Disposer>>,
    /// The test hooks of the bus, when tapped.
    #[cfg(feature = "testing")]
    tap: Mutex<Option<Arc<super::testing::TapState>>>,
}

#[derive(Default)]
struct HubState {
    /// The table snapshot is applied: changes are applied as they come.
    ready: bool,
    /// Changes seen before the snapshot was applied, in order.
    queued: Vec<TableChange>,
    /// How many entries of each key each origin has (`None`: this host's
    /// Rust listeners).
    presence: HashMap<Key, HashMap<Option<u64>, usize>>,
    /// The runtime sessions told what is elsewhere, by origin.
    sessions: HashMap<u64, Weak<SessionEvents>>,
}

/// The hubs, by the address of their kernel bus. A hub lives while a
/// session uses it, and keeps its bus (its context) alive meanwhile, so an
/// address is never another bus's while its hub lives.
static HUBS: Mutex<Vec<(usize, Weak<Hub>)>> = Mutex::new(Vec::new());

/// Origins of entries, unique in this process (sessions and links of every
/// bus): `0` is never one.
static ORIGINS: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_origin() -> u64 {
    ORIGINS.fetch_add(1, Ordering::Relaxed)
}

impl Hub {
    /// The hub of `ctx`'s bus, made when none lives.
    pub(crate) fn of(ctx: &Ctx) -> Result<Arc<Hub>, Error> {
        // The bus's root: the watcher lasts as long as the bus.
        let ctx = &ctx.root_ctx();
        let address = ctx.events() as *const _ as usize;
        let mut hubs = HUBS.lock().unwrap();
        hubs.retain(|(_, hub)| hub.strong_count() > 0);
        if let Some(hub) = hubs
            .iter()
            .find(|(at, _)| *at == address)
            .and_then(|(_, hub)| hub.upgrade())
        {
            return Ok(hub);
        }
        let hub = Arc::new(Hub {
            ctx: ctx.clone(),
            state: Mutex::default(),
            watching: Mutex::new(None),
            services: Mutex::new(None),
            #[cfg(feature = "testing")]
            tap: Mutex::new(None),
        });
        ctx.events().set_transcoder(Arc::new(SessionTranscoder));
        let weak = Arc::downgrade(&hub);
        let (snapshot, watching) = ctx
            .events()
            .watch_table(ctx, move |change: &TableChange| {
                if let Some(hub) = weak.upgrade() {
                    hub.changed(change);
                }
            })
            .map_err(|error| to_session(&error))?;
        *hub.watching.lock().unwrap() = Some(watching);
        let services = ctx
            .events()
            .on(
                ctx,
                &rutis::EventKey::<rutis::ServiceChanged>::of(),
                super::framework::ServiceEvents,
            )
            .map_err(|error| to_session(&error))?;
        *hub.services.lock().unwrap() = Some(services);
        {
            let mut state = hub.state.lock().unwrap();
            for entry in &snapshot {
                count(&mut state.presence, entry, true);
            }
            let queued = std::mem::take(&mut state.queued);
            state.ready = true;
            drop(state);
            for change in &queued {
                hub.changed(change);
            }
        }
        hubs.push((address, Arc::downgrade(&hub)));
        Ok(hub)
    }

    /// A change of the table: the sessions for which a name or prefix
    /// starts or stops being elsewhere are told, in order, before the
    /// change's registration returns (events protocol §4.4).
    fn changed(&self, change: &TableChange) {
        let mut state = self.state.lock().unwrap();
        if !state.ready {
            state.queued.push(change.clone());
            return;
        }
        let (entry, added) = match change {
            TableChange::Added(entry) => (entry, true),
            TableChange::Removed(entry) => (entry, false),
        };
        let key = (entry.name.clone(), entry.prefix);
        let before = state.presence.get(&key).cloned().unwrap_or_default();
        count(&mut state.presence, entry, added);
        let after = state.presence.get(&key).cloned().unwrap_or_default();
        let sessions: Vec<Arc<SessionEvents>> =
            state.sessions.values().filter_map(Weak::upgrade).collect();
        for session in sessions {
            let was = elsewhere(&before, session.origin);
            let is = elsewhere(&after, session.origin);
            if was != is {
                session.push_elsewhere(Presence::change(&key, is));
            }
        }
    }

    /// `session` hears what is elsewhere from now on, starting with the
    /// whole of it.
    fn join(&self, session: &Arc<SessionEvents>) {
        let mut state = self.state.lock().unwrap();
        let mut presence = Presence::default();
        for (key, counts) in &state.presence {
            if elsewhere(counts, session.origin) {
                presence.add(key);
            }
        }
        session.push_elsewhere(presence);
        state
            .sessions
            .insert(session.origin, Arc::downgrade(session));
    }

    fn leave(&self, origin: u64) {
        self.state.lock().unwrap().sessions.remove(&origin);
    }

    #[cfg(feature = "testing")]
    pub(crate) fn tap(&self, tap: Arc<super::testing::TapState>) {
        *self.tap.lock().unwrap() = Some(tap);
    }
}

/// The kernel's process-local diagnostic events (`rutis::ServiceChanged`,
/// `rutis::FiberStatusChanged`): they name Rust types and cannot be decoded
/// elsewhere, so their listeners are never announced to runtimes or nodes.
pub(crate) fn process_local(name: &str) -> bool {
    name.starts_with("rutis::")
}

fn count(
    presence: &mut HashMap<Key, HashMap<Option<u64>, usize>>,
    entry: &TableEntry,
    added: bool,
) {
    if process_local(&entry.name) {
        return;
    }
    let key = (entry.name.clone(), entry.prefix);
    let counts = presence.entry(key.clone()).or_default();
    let n = counts.entry(entry.origin).or_default();
    if added {
        *n += 1;
    } else {
        *n = n.saturating_sub(1);
        if *n == 0 {
            counts.remove(&entry.origin);
        }
        if counts.is_empty() {
            presence.remove(&key);
        }
    }
}

/// Whether some entry of these counts is not `origin`'s.
fn elsewhere(counts: &HashMap<Option<u64>, usize>, origin: u64) -> bool {
    counts.iter().any(|(at, n)| *n > 0 && *at != Some(origin))
}

/// One `events.elsewhere` message.
#[derive(Default)]
struct Presence {
    add: Vec<String>,
    remove: Vec<String>,
    add_prefixes: Vec<String>,
    remove_prefixes: Vec<String>,
}

impl Presence {
    fn change(key: &Key, present: bool) -> Self {
        let mut presence = Self::default();
        match present {
            true => presence.add(key),
            false => match key.1 {
                true => presence.remove_prefixes.push(key.0.clone()),
                false => presence.remove.push(key.0.clone()),
            },
        }
        presence
    }

    fn add(&mut self, key: &Key) {
        match key.1 {
            true => self.add_prefixes.push(key.0.clone()),
            false => self.add.push(key.0.clone()),
        }
    }

    /// The message's argument; empty fields left out.
    fn argument(&self) -> Json {
        let mut fields = serde_json::Map::new();
        for (field, names) in [
            ("add", &self.add),
            ("remove", &self.remove),
            ("addPrefixes", &self.add_prefixes),
            ("removePrefixes", &self.remove_prefixes),
        ] {
            if !names.is_empty() {
                fields.insert(field.into(), json!(names));
            }
        }
        Json::Object(fields)
    }
}

/// Withdraw an entry now: the kernel takes it out of the table when its
/// disposer runs; what is left of the disposal finishes on its own.
pub(crate) fn dispose_now(disposer: Disposer) {
    let mut disposal = disposer.dispose();
    let mut context = Context::from_waker(std::task::Waker::noop());
    if disposal.as_mut().poll(&mut context).is_pending() {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = disposal.await;
            });
        }
    }
}

/// One runtime session's side of the logical bus: its entries, its rows'
/// scopes, its dispatches (events protocol §3–§7).
pub(crate) struct SessionEvents {
    /// The hub; `None` when the runtime has no bus to join (a process
    /// mounted without a context), whose events then stay in its process.
    hub: Option<Arc<Hub>>,
    /// The context its entries are registered and its dispatches emitted
    /// through (the runtime plugin's): they go with it.
    ctx: Option<Ctx>,
    origin: u64,
    connection: WeakConnection,
    runtime: String,
    /// The scope of the mount's own plugins (`row: null`).
    mount_scope: Vec<Scope>,
    /// The scope chain and the id of each row loaded on the session, by
    /// key.
    rows: Mutex<HashMap<String, (Vec<Scope>, String)>>,
    /// The session's entries, by the runtime's id.
    entries: Mutex<HashMap<String, Registered>>,
    /// `emit` dispatches waiting for those before them to start (§5.5).
    emits: Mutex<Emits>,
}

struct Registered {
    row: Option<String>,
    disposer: Disposer,
}

#[derive(Default)]
struct Emits {
    busy: bool,
    queue: VecDeque<BoxFuture<'static, ()>>,
}

/// `events.on`'s argument (events protocol §4.2).
#[derive(serde::Deserialize)]
struct Registration {
    id: String,
    name: String,
    row: Option<String>,
    plugin: String,
    kind: ListenerKind,
    #[serde(default)]
    prepend: bool,
    #[serde(default)]
    global: bool,
}

impl SessionEvents {
    /// The runtime on `connection`, named `runtime`, whose mount's plugins
    /// are at `scope`, joins `ctx`'s bus: it is told right away what has
    /// listeners elsewhere (events protocol §4.5). It leaves the bus when
    /// the session ends or this is dropped.
    pub(crate) fn join(
        ctx: Option<&Ctx>,
        connection: Connection,
        runtime: String,
        scope: Vec<Scope>,
    ) -> Result<Arc<Self>, Error> {
        let hub = ctx.map(Hub::of).transpose()?;
        let session = Arc::new(Self {
            hub,
            ctx: ctx.cloned(),
            origin: next_origin(),
            connection: connection.downgrade(),
            runtime,
            mount_scope: scope,
            rows: Mutex::default(),
            entries: Mutex::default(),
            emits: Mutex::default(),
        });
        match &session.hub {
            Some(hub) => hub.join(&session),
            None => session.push_elsewhere(Presence::default()),
        }
        let weak = Arc::downgrade(&session);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                connection.closed().await;
                if let Some(session) = weak.upgrade() {
                    session.leave();
                }
            });
        }
        Ok(session)
    }

    /// Tell the runtime what changed elsewhere. Written now, in order; the
    /// answer is only awaited.
    fn push_elsewhere(&self, presence: Presence) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        // A notification: whatever call changed the table, it is not part
        // of it (another session's chain must not reach this one).
        let sent = crate::session::rpc::unchained(|| {
            connection.invoke_started(
                "",
                "events.elsewhere",
                Value::List(vec![Value::Data(presence.argument())]),
            )
        });
        if let (Ok(answer), Ok(runtime)) = (sent, tokio::runtime::Handle::try_current()) {
            runtime.spawn(async move {
                let _ = answer.await;
            });
        }
    }

    /// Withdraw everything of the session (its end, events protocol §7.2).
    fn leave(&self) {
        if let Some(hub) = &self.hub {
            hub.leave(self.origin);
        }
        self.rows.lock().unwrap().clear();
        let entries: Vec<Registered> = self
            .entries
            .lock()
            .unwrap()
            .drain()
            .map(|(_, e)| e)
            .collect();
        for entry in entries {
            dispose_now(entry.disposer);
        }
    }

    /// Row `key` is being loaded in `scope`.
    pub(crate) fn row_loading(&self, key: &str, scope: Vec<Scope>, id: String) {
        self.rows
            .lock()
            .unwrap()
            .insert(key.to_owned(), (scope, id));
    }

    /// Row `key` is unloading or ended: its entries leave the table now
    /// (events protocol §7.1).
    pub(crate) fn row_ended(&self, key: &str) {
        self.rows.lock().unwrap().remove(key);
        let gone: Vec<Registered> = {
            let mut entries = self.entries.lock().unwrap();
            let ids: Vec<String> = entries
                .iter()
                .filter(|(_, entry)| entry.row.as_deref() == Some(key))
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| entries.remove(&id))
                .collect()
        };
        for entry in gone {
            dispose_now(entry.disposer);
        }
    }

    /// The `events.*` operations a runtime sends.
    pub(crate) fn invoke(
        self: &Arc<Self>,
        connection: &Connection,
        method: &str,
        args: Value,
    ) -> Reply {
        let first = args.list()?.into_iter().next().unwrap_or(Value::Undefined);
        match method {
            "events.on" => self.on(first),
            "events.off" => self.off(first),
            "events.dispatch" => {
                #[cfg(feature = "testing")]
                if let Some(tap) = self
                    .hub
                    .as_ref()
                    .and_then(|hub| hub.tap.lock().unwrap().clone())
                {
                    tap.arrived();
                }
                self.dispatch(connection, first)
            }
            _ => Err(binding(format!("{method} is not offered here"))),
        }
    }

    /// The scope and the id of row `key` on this session, or the mount's
    /// scope for none.
    fn row_of(&self, key: Option<&str>) -> Result<(Vec<Scope>, Option<String>), Error> {
        match key {
            None => Ok((self.mount_scope.clone(), None)),
            Some(key) => self
                .rows
                .lock()
                .unwrap()
                .get(key)
                .map(|(scope, id)| (scope.clone(), Some(id.clone())))
                .ok_or_else(|| binding(format!("row {key} is not loaded in this runtime"))),
        }
    }

    fn on(self: &Arc<Self>, registration: Value) -> Reply {
        let registration: Registration = crate::session::decode(registration.json()?)?;
        if registration.name.starts_with("internal/") {
            return Err(binding(format!(
                "event {} is internal to its runtime",
                registration.name
            )));
        }
        let (scope, id) = self.row_of(registration.row.as_deref())?;
        let Some(ctx) = &self.ctx else {
            return Ok(Value::Undefined);
        };
        if self.entries.lock().unwrap().contains_key(&registration.id) {
            return Err(binding(format!(
                "listener {} is already registered",
                registration.id
            )));
        }
        let info = ListenerInfo {
            name: registration.name.clone(),
            prefix: false,
            plugin: registration.plugin,
            row: id,
            runtime: Some(self.runtime.clone()),
            node: None,
            scope,
            global: registration.global,
        };
        let entry = RemoteEntry {
            name: registration.name.clone(),
            prefix: false,
            info,
            options: EventOptions::default()
                .prepend(registration.prepend)
                .global(registration.global),
            origin: self.origin,
        };
        let listener = Arc::new(WireEntry {
            connection: self.connection.clone(),
            id: registration.id.clone(),
            kind: registration.kind,
            origin: self.origin,
        });
        let disposer = ctx
            .events()
            .on_remote(ctx, entry, listener)
            .map_err(|error| to_session(&error))?;
        self.entries.lock().unwrap().insert(
            registration.id,
            Registered {
                row: registration.row,
                disposer,
            },
        );
        Ok(Value::Undefined)
    }

    fn off(&self, ids: Value) -> Reply {
        let ids: Vec<String> = crate::session::decode(ids.json()?)?;
        let gone: Vec<Registered> = {
            let mut entries = self.entries.lock().unwrap();
            ids.iter().filter_map(|id| entries.remove(id)).collect()
        };
        for entry in gone {
            dispose_now(entry.disposer);
        }
        Ok(Value::Undefined)
    }

    /// A dispatch from the runtime on `source`: what it calls in other
    /// sessions is forwarded from `source` (§5.3), so their call chains
    /// stay apart.
    fn dispatch(self: &Arc<Self>, source: &Connection, request: Value) -> Reply {
        let mut fields = record(request)?;
        let name = text(&mut fields, "name")?;
        let mode = match text(&mut fields, "mode")?.as_str() {
            "emit" => Mode::Emit,
            "parallel" => Mode::Parallel,
            "serial" => Mode::Serial,
            "waterfall" => Mode::Waterfall,
            "bail" => Mode::Bail,
            other => return Err(binding(format!("events.dispatch: unknown mode {other}"))),
        };
        let row = match fields.remove("row") {
            None | Some(Value::Undefined) | Some(Value::Data(Json::Null)) => None,
            Some(row) => Some(crate::session::decode::<String>(row.json()?)?),
        };
        if name.starts_with(rutis::events::RESERVED_PREFIX) {
            return Err(Error::Remote {
                name: "ReservedEvent".into(),
                message: format!("rutis/* events are emitted by rutis only: {name}"),
                graph: None,
            });
        }
        if name.starts_with("internal/") {
            return Err(binding(format!("event {name} is internal to its runtime")));
        }
        let (scope, row) = self.row_of(row.as_deref())?;
        let args = match fields.remove("args") {
            None | Some(Value::Undefined) => Vec::new(),
            Some(args) => args.list()?,
        };
        let filter = reference(&mut fields, "filter")?;
        let terminal = reference(&mut fields, "terminal")?;
        let Some(ctx) = self.ctx.clone() else {
            // No bus: nothing is elsewhere, and a runtime never sends this.
            return Ok(Value::Undefined);
        };
        let mut dispatch = Dispatch::new(name.clone(), mode, from_wire(args));
        dispatch.scope = Some(scope);
        dispatch.origin = Some(self.origin);
        dispatch.filter = filter.map(|filter| selector(filter, name.clone(), mode.is_sync()));
        dispatch.terminal = terminal.map(terminal_of);

        match mode {
            Mode::Bail => {
                let result = ctx
                    .events()
                    .dispatch_sync(&ctx, dispatch)
                    .map_err(|error| to_session(&error))?;
                result_to_wire(result)
            }
            Mode::Emit => {
                let reporter = ctx.clone();
                let (started, answered) = tokio::sync::oneshot::channel();
                let row = row.clone();
                let run: BoxFuture<'static, ()> =
                    Box::pin(with_caller(source.clone(), async move {
                        let dispatched = ctx.events().dispatch(&ctx, dispatch).await;
                        if let Err(error) = dispatched {
                            report_filter(&reporter, &name, row.as_deref(), &error);
                        }
                        let _ = started.send(());
                    }));
                match self.run_emit(run) {
                    true => Ok(Value::Undefined),
                    false => Ok(Value::future(async move {
                        let _ = answered.await;
                        Ok(Value::Undefined)
                    })),
                }
            }
            // Cancelled when the emitter gives up (§5.7): the dispatch stops
            // and cancels its calls under way.
            _ => Ok(Value::cancellable_future(with_caller(
                source.clone(),
                async move {
                    let result = ctx
                        .events()
                        .dispatch(&ctx, dispatch)
                        .await
                        .map_err(|error| to_session(&error))?;
                    result_to_wire(result)
                },
            ))),
        }
    }

    /// Start an `emit` dispatch after those of the session before it
    /// started (events protocol §5.5): here, when none waits and it starts
    /// at once (`true`); otherwise in turn, on a task.
    fn run_emit(self: &Arc<Self>, mut run: BoxFuture<'static, ()>) -> bool {
        {
            let mut emits = self.emits.lock().unwrap();
            if emits.busy {
                emits.queue.push_back(run);
                return false;
            }
            emits.busy = true;
        }
        let mut context = Context::from_waker(std::task::Waker::noop());
        if run.as_mut().poll(&mut context).is_ready() {
            let mut emits = self.emits.lock().unwrap();
            if emits.queue.is_empty() {
                emits.busy = false;
                return true;
            }
            run = emits.queue.pop_front().expect("a queued emit");
        }
        let session = self.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    let mut run = run;
                    loop {
                        run.await;
                        let mut emits = session.emits.lock().unwrap();
                        match emits.queue.pop_front() {
                            Some(next) => run = next,
                            None => {
                                emits.busy = false;
                                return;
                            }
                        }
                    }
                });
            }
            Err(_) => self.emits.lock().unwrap().busy = false,
        }
        false
    }
}

/// The logical bus of a context's tree, held while this lives
/// ([`join_bus`]).
pub struct BusHandle(#[allow(dead_code)] Arc<Hub>);

/// Keep the logical bus of `ctx`'s tree (its root's) for as long as the
/// handle lives: the framework events rutis-bridge emits (`rutis/service`
/// for every host service, Rust or not) follow its table from now on. A
/// runtime session or node link holds it too; a host whose services are
/// all Rust holds it with this.
pub fn join_bus(ctx: &Ctx) -> Result<BusHandle, Error> {
    Hub::of(ctx).map(BusHandle)
}

impl Drop for Hub {
    /// Its listeners leave the bus with it: a later hub of the same bus
    /// registers its own.
    fn drop(&mut self) {
        for disposer in [
            self.watching.get_mut().unwrap().take(),
            self.services.get_mut().unwrap().take(),
        ]
        .into_iter()
        .flatten()
        {
            dispose_now(disposer);
        }
    }
}

impl Drop for SessionEvents {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Report a failed `emit` dispatch (its filter, events protocol §5.6) to
/// the host's error sink.
fn report_filter(ctx: &Ctx, name: &str, row: Option<&str>, error: &CordisError) {
    let error = to_session(error);
    let (kind, message) = match &error {
        Error::Remote { name, message, .. } => (name.clone(), message.clone()),
        other => ("BindingError".to_owned(), other.to_string()),
    };
    let from = match row {
        Some(row) => format!("row {row}"),
        None => "the mount".to_owned(),
    };
    let line = format!("filter of {name} from {from} failed: {kind}: {message}");
    (ctx.error_sink())(Arc::new(CordisError::PluginFailed(line.into())));
}

/// A request's fields.
fn record(value: Value) -> Result<BTreeMap<String, Value>, Error> {
    match value {
        Value::Record(fields) => Ok(fields),
        Value::Data(Json::Object(fields)) => Ok(fields
            .into_iter()
            .map(|(key, value)| (key, Value::Data(value)))
            .collect()),
        _ => Err(binding("events.dispatch needs its dispatch")),
    }
}

fn text(fields: &mut BTreeMap<String, Value>, field: &str) -> Result<String, Error> {
    match fields.remove(field) {
        Some(Value::Data(Json::String(text))) => Ok(text),
        _ => Err(binding(format!("events.dispatch needs a {field}"))),
    }
}

fn reference(
    fields: &mut BTreeMap<String, Value>,
    field: &str,
) -> Result<Option<crate::session::Reference>, Error> {
    match fields.remove(field) {
        None | Some(Value::Undefined) | Some(Value::Data(Json::Null)) => Ok(None),
        Some(Value::Reference(reference)) if reference.is_function() => Ok(Some(reference)),
        Some(_) => Err(binding(format!(
            "events.dispatch: {field} is not a function"
        ))),
    }
}

/// A listener's descriptor as a session value (events protocol §4.2).
pub(crate) fn descriptor(info: &ListenerInfo) -> Value {
    Value::Data(serde_json::to_value(info).unwrap_or(Json::Null))
}

/// The batch filter that calls the emitter's selector (events protocol §6):
/// once, with every candidate's descriptor; synchronously in `bail`.
fn selector(selector: crate::session::Reference, name: String, sync: bool) -> BatchFilter {
    Box::new(move |infos: Vec<ListenerInfo>| {
        let wanted = infos.len();
        let argument = Value::List(vec![Value::List(infos.iter().map(descriptor).collect())]);
        let answers = move |reply: Value| -> Result<Vec<bool>, CordisError> {
            let wrong = |got: usize| {
                to_cordis(binding(format!(
                    "filter of {name} returned {got} answers for {wanted} listeners"
                )))
            };
            let answers = match reply.json() {
                Ok(Json::Array(answers)) => answers,
                _ => return Err(wrong(0)),
            };
            if answers.len() != wanted {
                return Err(wrong(answers.len()));
            }
            answers
                .iter()
                .map(|answer| answer.as_bool().ok_or_else(|| wrong(answers.len())))
                .collect()
        };
        match sync {
            true => {
                FilterAnswer::Ready(selector.call(argument).map_err(to_cordis).and_then(answers))
            }
            false => FilterAnswer::Pending(Box::pin(async move {
                let reply = selector.call_async(argument).await.map_err(to_cordis)?;
                let reply = settle(reply).await.map_err(to_cordis)?;
                answers(reply)
            })),
        }
    })
}

/// A waterfall's terminal in the emitter's process: called with the event
/// arguments, its value as is.
fn terminal_of(terminal: crate::session::Reference) -> RawTerminal {
    RawTerminal::Async(Box::new(move |event: PayloadRef<'_>| {
        let args = to_wire(event);
        Box::pin(async move {
            let args = args.map_err(to_cordis)?;
            let reply = terminal
                .call_async(Value::List(args))
                .await
                .map_err(to_cordis)?;
            let value = settle(reply).await.map_err(to_cordis)?;
            Ok(result_from_wire(value, Mode::Waterfall))
        })
    }))
}

/// An entry whose listener is across a session: a runtime's (its
/// `events.on` id) or a linked node's (the id it announced). Each call is
/// one `events.call` (events protocol §5.3).
pub(crate) struct WireEntry {
    pub(crate) connection: WeakConnection,
    pub(crate) id: String,
    pub(crate) kind: ListenerKind,
    /// The origin of the session it came over.
    pub(crate) origin: u64,
}

/// What an `events.call` carries.
struct Invocation<'a> {
    name: &'a str,
    mode: Mode,
    dispatch: u64,
    own: bool,
    event: PayloadRef<'a>,
    next: Option<Value>,
}

impl WireEntry {
    fn connection(&self) -> Result<Connection, CordisError> {
        self.connection
            .upgrade()
            .ok_or_else(|| to_cordis(Error::Transport("session closed".into())))
    }

    fn request(&self, call: Invocation<'_>) -> Result<Value, CordisError> {
        let mut fields = BTreeMap::new();
        fields.insert("listener".into(), Value::Data(json!(self.id)));
        fields.insert(
            "dispatch".into(),
            Value::Data(json!(format!("d{}", call.dispatch))),
        );
        fields.insert("own".into(), Value::Data(json!(call.own)));
        fields.insert("name".into(), Value::Data(json!(call.name)));
        fields.insert("mode".into(), Value::Data(json!(call.mode.as_str())));
        let args = to_wire(call.event).map_err(to_cordis)?;
        fields.insert("args".into(), Value::List(args));
        if let Some(next) = call.next {
            fields.insert("next".into(), next);
        }
        fields.insert("signal".into(), Value::Signal);
        Ok(Value::List(vec![Value::Record(fields)]))
    }

    /// The dispatch came from this entry's own session.
    fn own(&self, origin: Option<u64>) -> bool {
        origin == Some(self.origin)
    }
}

/// The session whose dispatch a call into `connection` serves, if that is
/// another one ([`crate::session::caller`]).
fn caller_of(connection: &Connection) -> Option<Connection> {
    crate::session::caller().filter(|source| source.tag() != connection.tag())
}

/// Run `call`, a call into `connection`, forwarded from the session whose
/// dispatch it serves ([`caller_of`]).
fn forwarded<R>(connection: &Connection, call: impl FnOnce() -> R) -> R {
    match caller_of(connection) {
        Some(source) => connection.forward(&source, call),
        None => call(),
    }
}

/// `next` as the far end gets it: a function taking no arguments,
/// returning the rest of the chain.
pub(crate) fn next_function(next: RemoteNext) -> Value {
    Value::callback(move |_| {
        let next = next.clone();
        let rest = async move {
            let rest = next.call().await.map_err(|error| to_session(&error))?;
            result_to_wire(rest)
        };
        // The rest of the chain runs on behalf of the session calling it.
        Ok(match crate::session::caller() {
            Some(source) => Value::future(with_caller(source, rest)),
            None => Value::future(rest),
        })
    })
}

/// `next` of a synchronous waterfall as the far end gets it.
pub(crate) fn sync_next_function(next: RemoteSyncNext) -> Value {
    Value::callback(move |_| {
        let rest = next.call().map_err(|error| to_session(&error))?;
        result_to_wire(rest)
    })
}

impl RemoteListener for WireEntry {
    fn kind(&self) -> ListenerKind {
        self.kind
    }

    fn call<'a>(
        &'a self,
        call: RemoteCall<'a>,
    ) -> BoxFuture<'a, Result<Option<rutis::events::Value>, CordisError>> {
        let mode = call.mode();
        let own = self.own(call.origin());
        let connection = match self.connection() {
            Ok(connection) => connection,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        // The call, the `next` it hands over, and the wait for what it
        // answers go out with the chain of the session whose dispatch this
        // is, rebased: a function made here records the chain it is made in.
        let source = caller_of(&connection);
        let build = || {
            self.request(Invocation {
                name: call.name(),
                mode,
                dispatch: call.dispatch(),
                own,
                event: call.event(),
                next: call.next().map(next_function),
            })
        };
        let request = match &source {
            Some(source) => connection.forward(source, build),
            None => build(),
        };
        // Sent now, so calls started one after another arrive in that
        // order (an `emit`'s calls, §5.5); only the answer is awaited.
        let send = || {
            request.and_then(|request| {
                connection
                    .invoke_started("", "events.call", request)
                    .map_err(to_cordis)
            })
        };
        let started = match &source {
            Some(source) => connection.forward(source, send),
            None => send(),
        };
        let run = async move {
            let reply = started?.await.map_err(to_cordis)?;
            let value = settle(reply).await.map_err(to_cordis)?;
            Ok(result_from_wire(value, mode))
        };
        match source {
            Some(source) => Box::pin(connection.forward_async(&source, run)),
            None => Box::pin(run),
        }
    }

    fn call_sync(
        &self,
        call: RemoteSyncCall<'_>,
    ) -> Result<Option<rutis::events::Value>, CordisError> {
        let mode = call.mode();
        let own = self.own(call.origin());
        let connection = self.connection()?;
        let reply = forwarded(&connection, || {
            let request = self.request(Invocation {
                name: call.name(),
                mode,
                dispatch: call.dispatch(),
                own,
                event: call.event(),
                next: call.next().map(sync_next_function),
            })?;
            connection
                .invoke("", "events.call", request)
                .map_err(to_cordis)
        })?;
        Ok(result_from_wire(reply, mode))
    }
}

/// Poll `future` to completion on this thread.
#[allow(dead_code)]
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let thread = std::thread::current();
    let waker = std::task::Waker::from(Arc::new(Unpark(thread)));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = Pin::as_mut(&mut future).poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
