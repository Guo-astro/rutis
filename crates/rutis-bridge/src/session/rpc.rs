//! One ordered, bidirectional connection for invocation, await and owned references.
//! Socket readers admit frames and pin references; they never execute plugin code.
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::thread::ThreadId;

use crate::channel::{Channel, ChannelError, Closer, Sender};
use rutis::BoxFuture;
use serde_json::Value as Json;
use tokio::runtime::{Handle, RuntimeFlavor};
use tokio::sync::{oneshot, watch, Notify};

use crate::channel::PeerId;
pub use crate::session::protocol::Implementation;
use crate::session::protocol::{Frame, Kind, WireValue, VERSION};
use crate::session::values::{
    base64_decode, base64_encode, check_keys, holds_beyond_safe, BigInt, Date, DateTime, Decimal,
    Instant, TimeOfDay, Uuid,
};
use crate::session::{Error, Handshake};

pub type Reply = Result<Value, Error>;
type Callback = dyn Fn(Value) -> Reply + Send + Sync;

/// A value crossing a session: JSON data, references to functions, objects
/// and async results, and the tagged values beyond JSON (points in time,
/// local dates and times, binary data, maps, sets, big integers, decimals,
/// UUIDs; see [`values`](crate::session::values)). `invoke` returns a
/// Future reference; only `wait`/`wait_async` await it.
///
/// A tagged value is sent only to a far end that declared its capability
/// `value-<tag>`; to any other, the call carrying it fails, naming the
/// value and the capability, and the session goes on. More kinds of value
/// may be added: match with a `_` arm.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Value {
    Undefined,
    Data(Json),
    List(Vec<Value>),
    /// A plain object whose fields contain references or tagged values.
    Record(std::collections::BTreeMap<String, Value>),
    Reference(Reference),
    /// As an argument: an AbortSignal the callee receives, aborted when this
    /// call is cancelled (its future dropped). Received by Rust, it only
    /// marks the call as cancellable: a Rust callee is cancelled by the
    /// future it returned being dropped.
    Signal,
    /// A point in time (tag `instant`): JS `Date`, Python `datetime` with a
    /// time zone, Go `time.Time`.
    Instant(Instant),
    /// A local date and time without a time zone (tag `datetime`).
    DateTime(DateTime),
    /// A calendar date (tag `date`).
    Date(Date),
    /// A time of day without a time zone (tag `time`).
    Time(TimeOfDay),
    /// Binary data (tag `bytes`): JS `Uint8Array`, Python `bytes`, Go
    /// `[]byte`.
    Bytes(Vec<u8>),
    /// A map (tag `map`), its entries in order. Keys are strings, finite
    /// numbers, booleans, or bigint, datetime, date, time, decimal and uuid
    /// values, no two equal; a value `Undefined` is sent as null.
    Map(Vec<(Value, Value)>),
    /// A set (tag `set`), its elements in order, of the kinds a map key
    /// may be, no two equal.
    Set(Vec<Value>),
    /// An integer of any size (tag `bigint`). An integer in `Data` beyond
    /// ±(2^53−1) is sent as one too (design V6), and the arrays and
    /// objects holding it as lists and records.
    BigInt(BigInt),
    /// A decimal number, its text kept (tag `decimal`).
    Decimal(Decimal),
    /// A UUID (tag `uuid`).
    Uuid(Uuid),
}
impl Value {
    /// The tag of a value beyond JSON data (`instant`, `map`, ...), whose
    /// capability is `value-<tag>`; `None` for data, lists, records,
    /// references and signals.
    pub fn tag(&self) -> Option<&'static str> {
        Some(match self {
            Self::Instant(_) => "instant",
            Self::DateTime(_) => "datetime",
            Self::Date(_) => "date",
            Self::Time(_) => "time",
            Self::Bytes(_) => "bytes",
            Self::Map(_) => "map",
            Self::Set(_) => "set",
            Self::BigInt(_) => "bigint",
            Self::Decimal(_) => "decimal",
            Self::Uuid(_) => "uuid",
            _ => return None,
        })
    }
    pub fn list(self) -> Result<Vec<Value>, Error> {
        match self {
            Self::List(values) => Ok(values),
            Self::Data(Json::Array(values)) => Ok(values.into_iter().map(Self::Data).collect()),
            _ => Err(Error::Value("expected argument array".into())),
        }
    }
    /// The value as JSON data: lists and records as arrays and objects,
    /// a bigint that fits an `i64` or `u64` as the number. Any other
    /// tagged value, a reference or a signal is an error.
    pub fn json(self) -> Result<Json, Error> {
        match self {
            Self::Undefined => Ok(Json::Null),
            Self::Data(value) => Ok(value),
            Self::List(values) => values
                .into_iter()
                .map(Self::json)
                .collect::<Result<Vec<_>, _>>()
                .map(Json::Array),
            Self::Record(fields) => fields
                .into_iter()
                .map(|(key, value)| value.json().map(|value| (key, value)))
                .collect::<Result<serde_json::Map<_, _>, _>>()
                .map(Json::Object),
            Self::Reference(_) => Err(Error::Value("expected data, received a reference".into())),
            Self::Signal => Err(Error::Value("expected data, received a signal".into())),
            // An integer beyond ±(2^53−1) sent as data comes back a bigint
            // (design V6): as data, it is the number again.
            Self::BigInt(value) => value.to_json_number().map(Json::Number).ok_or_else(|| {
                Error::Value("expected data, received a bigint beyond 64 bits".into())
            }),
            // Not JSON: as text it would be a string the receiver takes for
            // one (decode it with `decode_value` into its type instead).
            other => Err(Error::Value(format!(
                "expected data, received {}",
                crate::session::values::describe(&other)
            ))),
        }
    }
    pub fn reference(self) -> Result<Reference, Error> {
        match self {
            Self::Reference(value) => Ok(value),
            _ => Err(Error::Value("expected a reference".into())),
        }
    }
    pub fn future(future: impl Future<Output = Reply> + Send + 'static) -> Self {
        Self::future_inner(future, true)
    }
    /// [`Value::future`] that stops (is dropped) when every wait for it
    /// is given up before it finished: a far end that cancels its `await`
    /// cancels the work, not only its wait for it.
    pub fn cancellable_future(future: impl Future<Output = Reply> + Send + 'static) -> Self {
        let value = Self::future_inner(future, true);
        if let Self::Reference(Reference(ReferenceInner::Local(object))) = &value {
            if let Body::Future(state) = &object.body {
                state.future.lock().unwrap().cancellable = true;
            }
        }
        value
    }
    #[cfg(all(unix, feature = "cordis"))]
    pub(crate) fn control_future(future: impl Future<Output = Reply> + Send + 'static) -> Self {
        Self::future_inner(future, false)
    }
    fn future_inner(future: impl Future<Output = Reply> + Send + 'static, business: bool) -> Self {
        Self::future_on(future, business, Executor::current())
    }
    fn future_on(
        future: impl Future<Output = Reply> + Send + 'static,
        business: bool,
        executor: Executor,
    ) -> Self {
        Self::Reference(Reference(ReferenceInner::Local(Arc::new(Object {
            executor,
            business,
            origin: current_path(),
            body: Body::Future(AsyncResult {
                future: Mutex::new(FutureState {
                    pending: Some(Box::pin(future)),
                    task: None,
                    cancellable: false,
                    waiters: 0,
                }),
                result: watch::channel(None).0,
            }),
        }))))
    }
    /// The closure runs on its captured runtime, or on the originating sync
    /// caller's thread when that caller is pumping this invocation chain.
    pub fn callback(callback: impl Fn(Value) -> Reply + Send + Sync + 'static) -> Self {
        Self::Reference(Reference(ReferenceInner::Local(Arc::new(Object {
            executor: Executor::current(),
            business: true,
            origin: current_path(),
            body: Body::Function(Arc::new(callback)),
        }))))
    }
}
impl From<Json> for Value {
    fn from(value: Json) -> Self {
        Self::Data(value)
    }
}

#[derive(Clone)]
pub struct Reference(ReferenceInner);
impl std::fmt::Debug for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reference")
            .field("future", &self.is_future())
            .finish_non_exhaustive()
    }
}
/// Identity: the same exported object, or the same import of a remote one
/// (repeated grants of one remote object share their import).
impl PartialEq for Reference {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (ReferenceInner::Local(a), ReferenceInner::Local(b)) => Arc::ptr_eq(a, b),
            (ReferenceInner::Remote(a), ReferenceInner::Remote(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}
#[derive(Clone)]
enum ReferenceInner {
    Local(Arc<Object>),
    Remote(Arc<Import>),
}
impl Reference {
    pub fn is_future(&self) -> bool {
        match &self.0 {
            ReferenceInner::Local(object) => object.kind() == Kind::Future,
            ReferenceInner::Remote(import) => import.kind == Kind::Future,
        }
    }
    pub fn is_function(&self) -> bool {
        match &self.0 {
            ReferenceInner::Local(object) => object.kind() == Kind::Function,
            ReferenceInner::Remote(import) => import.kind == Kind::Function,
        }
    }
    pub fn is_object(&self) -> bool {
        matches!(&self.0, ReferenceInner::Remote(import) if import.kind == Kind::Object)
    }
    pub fn call(&self, args: Value) -> Reply {
        match &self.0 {
            ReferenceInner::Local(object) => object.call(args),
            ReferenceInner::Remote(import) => import
                .connection()?
                .request_sync(Operation::Call(import.id, None, args)),
        }
    }
    pub async fn call_async(&self, args: Value) -> Reply {
        match &self.0 {
            ReferenceInner::Local(object) => object.call(args),
            ReferenceInner::Remote(import) => {
                import
                    .connection()?
                    .request_async(Operation::Call(import.id, None, args))
                    .await
            }
        }
    }
    fn remote_object(&self) -> Result<&Arc<Import>, Error> {
        match &self.0 {
            ReferenceInner::Remote(import) if import.kind == Kind::Object => Ok(import),
            _ => Err(Error::Value("reference is not a remote object".into())),
        }
    }
    /// Call a method of a remote object and wait for it to return.
    pub fn call_method(&self, method: &str, args: Value) -> Reply {
        let import = self.remote_object()?;
        import
            .connection()?
            .request_sync(Operation::Call(import.id, Some(method.into()), args))
    }
    /// Call a method of a remote object without blocking the caller.
    pub async fn call_method_async(&self, method: &str, args: Value) -> Reply {
        let import = self.remote_object()?;
        import
            .connection()?
            .request_async(Operation::Call(import.id, Some(method.into()), args))
            .await
    }
    /// Read a property of a remote object; every read is live.
    pub fn get(&self, property: &str) -> Reply {
        let import = self.remote_object()?;
        import
            .connection()?
            .request_sync(Operation::Get(import.id, property.into()))
    }
    pub async fn wait_async(&self) -> Reply {
        match &self.0 {
            ReferenceInner::Local(object) => object.wait(false, current_path()).await,
            ReferenceInner::Remote(import) => {
                import
                    .connection()?
                    .request_async(Operation::Await(import.id, import.origin.clone()))
                    .await
            }
        }
    }
    pub fn wait(&self) -> Reply {
        match &self.0 {
            ReferenceInner::Remote(import) => import
                .connection()?
                .request_sync(Operation::Await(import.id, import.origin.clone())),
            ReferenceInner::Local(_) => {
                Err(Error::Value("local future requires wait_async".into()))
            }
        }
    }
}

#[derive(Clone)]
struct Executor {
    handle: Handle,
    thread: ThreadId,
}
impl Executor {
    fn current() -> Self {
        Self {
            handle: Handle::current(),
            thread: std::thread::current().id(),
        }
    }
    fn blocked_here(&self) -> bool {
        self.handle.runtime_flavor() == RuntimeFlavor::CurrentThread
            && self.thread == std::thread::current().id()
    }
}
/// Lazily started once per connection. Closing drops the stop sender; the
/// runtime cancels its tasks without blocking the caller or joining itself.
struct Background {
    executor: Executor,
    _stop: oneshot::Sender<()>,
}
impl Background {
    fn start() -> Result<Self, Error> {
        let (ready, started) = mpsc::sync_channel(1);
        let (stop, stopped) = oneshot::channel();
        std::thread::Builder::new()
            .name("rutis-async".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => {
                        let _ = ready.send(Ok(Executor {
                            handle: runtime.handle().clone(),
                            thread: std::thread::current().id(),
                        }));
                        runtime.block_on(async {
                            let _ = stopped.await;
                        });
                        runtime.shutdown_background();
                    }
                    Err(error) => {
                        let _ = ready.send(Err(transport(error)));
                    }
                }
            })
            .map_err(transport)?;
        Ok(Self {
            executor: started.recv().map_err(transport)??,
            _stop: stop,
        })
    }
}
struct Object {
    executor: Executor,
    body: Body,
    business: bool,
    origin: Vec<String>,
}
enum Body {
    Function(Arc<Callback>),
    Future(AsyncResult),
    /// Stands in for a reference imported on another session: calls, reads
    /// and awaits are forwarded to it. Holding the import keeps the original
    /// alive; dropping the relay releases it on its own session.
    Relay(Arc<Import>),
}
struct AsyncResult {
    future: Mutex<FutureState>,
    result: watch::Sender<Option<Reply>>,
}
struct FutureState {
    pending: Option<BoxFuture<'static, Reply>>,
    task: Option<tokio::task::AbortHandle>,
    /// Aborted when its last waiter gives up ([`Value::cancellable_future`]).
    cancellable: bool,
    /// The waits under way.
    waiters: usize,
}

/// One wait for a future: when the last one is given up before the result,
/// a cancellable future's task is aborted.
struct Waiter<'a>(&'a AsyncResult);
impl Drop for Waiter<'_> {
    fn drop(&mut self) {
        let mut state = self.0.future.lock().unwrap();
        state.waiters -= 1;
        if state.waiters == 0 && state.cancellable && self.0.result.borrow().is_none() {
            if let Some(task) = state.task.take() {
                task.abort();
            }
        }
    }
}
impl Drop for AsyncResult {
    fn drop(&mut self) {
        if let Some(task) = self.future.get_mut().unwrap().task.take() {
            task.abort();
        }
    }
}
fn poll_future(future: &mut BoxFuture<'static, Reply>, cx: &mut Context<'_>) -> Poll<Reply> {
    catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx)))
        .unwrap_or_else(|panic| Poll::Ready(Err(panic_error(panic))))
}
impl Object {
    fn kind(&self) -> Kind {
        match &self.body {
            Body::Function(_) => Kind::Function,
            Body::Future(_) => Kind::Future,
            Body::Relay(import) => import.kind,
        }
    }
    fn call(&self, args: Value) -> Reply {
        match &self.body {
            Body::Function(callback) => protected(|| callback(args)),
            _ => Err(Error::Value("reference is not callable".into())),
        }
    }
    fn ready_on_blocked_executor(&self) -> Reply {
        let Body::Future(state) = &self.body else {
            return Err(Error::Value("reference is not awaitable".into()));
        };
        if let Some(result) = state.result.borrow().clone() {
            return result;
        }
        let pending = state.future.lock().unwrap().pending.take();
        if let Some(mut future) = pending {
            let _entered = self.executor.handle.enter();
            let waker = Waker::noop();
            match poll_future(&mut future, &mut Context::from_waker(waker)) {
                Poll::Ready(result) => {
                    state.result.send_replace(Some(result.clone()));
                    return result;
                }
                Poll::Pending => {
                    let result = state.result.clone();
                    let task =
                        self.executor
                            .handle
                            .spawn(ASYNC_PATH.scope(current_path(), async move {
                                let reply =
                                    std::future::poll_fn(|cx| poll_future(&mut future, cx)).await;
                                result.send_replace(Some(reply));
                            }));
                    state.future.lock().unwrap().task = Some(task.abort_handle());
                }
            }
        }
        Err(Error::SyncWaitCycle(
            format!("await needs the Rust current_thread executor occupied by its parent synchronous call; origin: {:?}", self.origin),
        ))
    }
    async fn wait(self: &Arc<Self>, _sync: bool, path: Vec<String>) -> Reply {
        let Body::Future(state) = &self.body else {
            return Err(Error::Value("reference is not awaitable".into()));
        };
        {
            let mut pending = state.future.lock().unwrap();
            if let Some(mut future) = pending.pending.take() {
                let result = state.result.clone();
                let task = self
                    .executor
                    .handle
                    .spawn(ASYNC_PATH.scope(path, async move {
                        let reply = std::future::poll_fn(|cx| poll_future(&mut future, cx)).await;
                        result.send_replace(Some(reply));
                    }));
                pending.task = Some(task.abort_handle());
            }
            pending.waiters += 1;
        }
        let _waiter = Waiter(state);
        let mut result = state.result.subscribe();
        loop {
            if let Some(result) = result.borrow().clone() {
                return result;
            }
            result
                .changed()
                .await
                .map_err(|_| Error::Transport("future executor stopped".into()))?;
        }
    }
}

/// Wait for a returned Promise / Future reference; other values pass through.
pub async fn settle(value: Value) -> Reply {
    match value {
        Value::Reference(reference) if reference.is_future() => reference.wait_async().await,
        value => Ok(value),
    }
}

fn protected(call: impl FnOnce() -> Reply) -> Reply {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|panic| Err(panic_error(panic)))
}
fn panic_error(panic: Box<dyn std::any::Any + Send>) -> Error {
    match panic.downcast::<Error>() {
        Ok(error) => *error,
        Err(panic) => crate::session::native_error(
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "native callback panicked".into()),
        ),
    }
}
tokio::task_local! { static ASYNC_PATH: Vec<String>; }
thread_local! { static SYNC_PATH: RefCell<Option<Vec<String>>> = const { RefCell::new(None) }; }
thread_local! { static CALLER: RefCell<Option<Connection>> = const { RefCell::new(None) }; }
thread_local! {
    /// The invocation this thread runs: its session's tag and its sequence.
    static INVOKING: RefCell<Option<(String, u64)>> = const { RefCell::new(None) };
}

/// The invocation of a host service this thread is running, as its
/// session's tag and its sequence in that session: invocations of one
/// session were sent in sequence order, whatever order they run in.
#[cfg_attr(not(feature = "testing"), allow(dead_code))]
pub(crate) fn invocation() -> Option<(String, u64)> {
    INVOKING.with(|invoking| invoking.borrow().clone())
}
/// The session whose call (an `invoke`, or a call of a function this side
/// exported) is being dispatched on this thread, if any. Code that calls
/// another session on its behalf passes it to [`Connection::forward`].
pub fn caller() -> Option<Connection> {
    CALLER.with(|caller| caller.borrow().clone())
}
/// Run `call` outside any invocation chain: what it sends is not part of
/// the call being handled here (a notification another session's call
/// happens to trigger), so it carries no chain of that session.
pub(crate) fn unchained<R>(call: impl FnOnce() -> R) -> R {
    let _path = PathGuard::enter(Vec::new());
    call()
}
/// Poll `future` with `source` as the [`caller`]: what it calls in other
/// sessions on that session's behalf is forwarded from it (its chain
/// rebased), however long after the call it runs.
pub(crate) async fn with_caller<F: Future>(source: Connection, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        let previous = CALLER.with(|caller| caller.replace(Some(source.clone())));
        let polled = future.as_mut().poll(cx);
        CALLER.with(|caller| caller.replace(previous));
        polled
    })
    .await
}
thread_local! { static PUMPING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
/// Marks a thread that runs an invocation pumped by its own synchronous
/// wait (`request_sync`), so code that relies on running there can check it.
struct Pumping;
impl Pumping {
    fn enter() -> Self {
        PUMPING.with(|depth| depth.set(depth.get() + 1));
        Self
    }
    fn active() -> bool {
        PUMPING.with(|depth| depth.get() > 0)
    }
}
impl Drop for Pumping {
    fn drop(&mut self) {
        PUMPING.with(|depth| depth.set(depth.get() - 1));
    }
}
fn current_path() -> Vec<String> {
    SYNC_PATH
        .with(|p| p.borrow().clone())
        .or_else(|| ASYNC_PATH.try_with(Clone::clone).ok())
        .unwrap_or_default()
}
struct PathGuard(Option<Vec<String>>);
impl PathGuard {
    fn enter(path: Vec<String>) -> Self {
        Self(SYNC_PATH.with(|p| p.replace(Some(path))))
    }
}
impl Drop for PathGuard {
    fn drop(&mut self) {
        SYNC_PATH.with(|p| p.replace(self.0.take()));
    }
}
/// Poll `future` with `path` as the current invocation chain. The path is
/// entered on every poll, so it wins over whatever chain the polling thread
/// or task carries.
async fn with_path<F: Future>(path: Vec<String>, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        let _path = PathGuard::enter(path.clone());
        future.as_mut().poll(cx)
    })
    .await
}

/// Rewrite an invocation chain from session `from` for a call sent to
/// session `to` (both are [`Connection::tag`]s). Call ids are unique only
/// within a session, so entries of other sessions carry their session's tag
/// (`"s1/node:3"`): untagged entries belong to `from` and get its tag;
/// entries tagged `to` return to their original ids, so `to` recognises the
/// calls it is synchronously waiting for; entries of third sessions pass
/// through. A tagged entry never equals a native `node:`/`rust:` id, so no
/// session mistakes it for one of its own calls.
///
/// Chains cross links to other processes unchanged, and may come back, so
/// a tag names its session among all the processes a chain passes, not
/// only in this one: each process tags with a random nonce of its own
/// (#325).
pub fn rebase(path: &[String], from: &str, to: &str) -> Vec<String> {
    let home = format!("{to}/");
    path.iter()
        .map(|entry| match entry.strip_prefix(&home) {
            Some(native) => native.to_owned(),
            None if entry.contains('/') => entry.clone(),
            None => format!("{from}/{entry}"),
        })
        .collect()
}
static NEXT_TAG: AtomicU64 = AtomicU64::new(1);

/// A new session tag: `s`, this process's nonce (15 random decimal digits)
/// and a counter. Only the process whose tag an entry carries restores it;
/// with a counter alone, another process's `s1` was taken for this one's
/// `s1` (#325). Two processes' tags meet only if their nonces are equal
/// (one chance in 10^15 per pair), and never meet the shorter tags of an
/// older rutis. Digits only: runtimes accept an origin entry as
/// `s[0-9]+/<id>`, so nothing on the wire changes.
fn next_tag() -> String {
    static NONCE: std::sync::LazyLock<u64> = std::sync::LazyLock::new(|| {
        let mut bytes = [0u8; 8];
        if getrandom::getrandom(&mut bytes).is_err() {
            // No system randomness: the standard library's hash keys, the
            // process id and the time still differ between processes.
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            if let Ok(now) = std::time::UNIX_EPOCH.elapsed() {
                hasher.write_u128(now.as_nanos());
            }
            bytes = hasher.finish().to_le_bytes();
        }
        u64::from_le_bytes(bytes) % 1_000_000_000_000_000
    });
    format!(
        "s{:015}{}",
        *NONCE,
        NEXT_TAG.fetch_add(1, Ordering::Relaxed)
    )
}

/// The capabilities of the tagged values, `value-<tag>` (#308): a side
/// declaring one takes values of that tag.
const VALUE_CAPABILITIES: [&str; 10] = [
    "value-instant",
    "value-datetime",
    "value-date",
    "value-time",
    "value-bytes",
    "value-map",
    "value-set",
    "value-bigint",
    "value-decimal",
    "value-uuid",
];
/// The event protocol of rutis 0.13 (#464): mandatory, declared by every
/// endpoint in both formats; a far end that does not declare it is refused
/// right after the handshake (`docs/design-events-protocol-2026-10-18.md`
/// §2).
pub const EVENTS_CAPABILITY: &str = "events-v2";

/// What this side declares in a compat-format handshake: it marks its
/// synchronous waits, and takes such marks (#228); it takes every tagged
/// value (#308); it speaks the event protocol of 0.13 (#464).
const COMPAT_CAPABILITIES: [&str; 12] = [
    "sync-wait",
    EVENTS_CAPABILITY,
    "value-instant",
    "value-datetime",
    "value-date",
    "value-time",
    "value-bytes",
    "value-map",
    "value-set",
    "value-bigint",
    "value-decimal",
    "value-uuid",
];
/// What a compat-format handshake may declare that means anything here; any
/// other capability there is ignored, as before the format had any.
const COMPAT_DECLARED: [&str; 14] = [
    "sync-wait",
    EVENTS_CAPABILITY,
    "sync-stack",
    "reentrant-sync",
    "value-instant",
    "value-datetime",
    "value-date",
    "value-time",
    "value-bytes",
    "value-map",
    "value-set",
    "value-bigint",
    "value-decimal",
    "value-uuid",
];

struct Import {
    peer: Weak<SessionState>,
    id: u64,
    kind: Kind,
    grants: Mutex<u64>,
    origin: Vec<String>,
}
impl Import {
    fn connection(&self) -> Result<Connection, Error> {
        self.peer
            .upgrade()
            .map(Connection)
            .ok_or_else(|| Error::Transport("session closed".into()))
    }
}
impl Drop for Import {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.upgrade() {
            {
                let mut imports = peer.imports.lock().unwrap();
                if imports
                    .get(&self.id)
                    .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), self))
                {
                    imports.remove(&self.id);
                }
            }
            let _ = Connection(peer).write(Frame::Release {
                reference: self.id,
                count: *self.grants.get_mut().unwrap(),
            });
        }
    }
}
struct Export {
    object: Arc<Object>,
    grants: u64,
}
#[derive(Default)]
struct Exports {
    next: u64,
    entries: HashMap<u64, Export>,
    identities: HashMap<usize, u64>,
    /// Relays by the import they forward to, so forwarding one import twice
    /// grants the same reference again.
    relays: HashMap<usize, u64>,
}
impl Exports {
    fn remove(&mut self, id: u64) -> Option<Export> {
        let entry = self.entries.remove(&id)?;
        self.identities
            .remove(&(Arc::as_ptr(&entry.object) as usize));
        if let Body::Relay(import) = &entry.object.body {
            self.relays.remove(&(Arc::as_ptr(import) as usize));
        }
        Some(entry)
    }
}
enum Waiting {
    Sync {
        sender: mpsc::Sender<Message>,
        origins: Vec<String>,
    },
    Async(oneshot::Sender<Reply>),
}
enum Message {
    Reply(Reply),
    Invoke(Incoming),
}
enum Operation {
    Invoke(String, String, Value),
    Call(u64, Option<String>, Value),
    Get(u64, String),
    Await(u64, Vec<String>),
}
enum Accepted {
    Invoke(String, String, Value),
    Call(Arc<Object>, Value),
    Await(Arc<Object>),
    /// Method calls and property reads reach relays of remote objects only.
    Method(Arc<Object>, String, Value),
    Get(Arc<Object>, String),
}
struct Awaiting {
    object: Arc<Object>,
    path: Vec<String>,
    _cancel: oneshot::Sender<()>,
}
struct Incoming {
    id: String,
    path: Vec<String>,
    call: Accepted,
    _flight: Option<Flight>,
}
#[derive(Default)]
struct Calls {
    next: u64,
    waiting: HashMap<String, Waiting>,
    incoming: HashMap<String, Incoming>,
    awaiting: HashMap<String, Awaiting>,
    /// Calls this side cancelled; their late replies are discarded.
    cancelled: std::collections::HashSet<String>,
    /// Late replies discarded after a cancellation.
    orphans: u64,
    /// The far end's last call id, without its prefix.
    received: u64,
    closed: Option<Error>,
}
impl Calls {
    /// Take the far end's call `sequence` (its id without the far end's
    /// prefix): ids only grow, so one already taken, by a call answered,
    /// running or refused, is not a new call and closes the session.
    fn admit(&mut self, sequence: Option<&str>) -> Result<(), Error> {
        self.received = sequence
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|n| *n > self.received && *n <= 9_007_199_254_740_991)
            .ok_or_else(|| transport("invalid or repeated invocation identity"))?;
        Ok(())
    }
}

/// A call recorded in the wait graph ([`waits::forward`]) that leaves it
/// unless it was sent.
struct Unsent(Option<(String, String)>);
impl Unsent {
    fn sent(mut self) {
        self.0 = None;
    }
}
impl Drop for Unsent {
    fn drop(&mut self) {
        if let Some((session, id)) = self.0.take() {
            waits::returned(&session, &id);
        }
    }
}

/// Cancels an async call whose future is dropped before it completes.
struct CancelOnDrop {
    peer: Weak<SessionState>,
    id: String,
    done: bool,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if let Some(peer) = self.peer.upgrade() {
            Connection(peer).cancel(&self.id);
        }
    }
}
struct Activity {
    count: Mutex<usize>,
    changed: Notify,
}
struct Flight(Arc<Activity>);
impl Drop for Flight {
    fn drop(&mut self) {
        *self.0.count.lock().unwrap() -= 1;
        self.0.changed.notify_waiters();
    }
}

pub trait Dispatch: Send + Sync + 'static {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply;
}
/// How a session identifies itself and its calls.
#[derive(Clone, Debug, Default)]
pub enum Format {
    /// Protocol 2, as local runtimes speak it: this side calls as `rust:`,
    /// the far end as `node:`, and the handshake carries only the version.
    #[default]
    Compat,
    /// The endpoint format ([`crate::session::ENDPOINT_PROTOCOL`]): each side names
    /// itself in the handshake and calls as `<endpoint id>:<n>`.
    Endpoint(Endpoint),
}

/// This side of an endpoint-format session.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub local: PeerId,
    /// The far end this side expects; the session fails if the handshake
    /// names another.
    pub expected: Option<PeerId>,
    pub implementation: Implementation,
    /// What this side supports: `objects`, `signals`, `reentrant-sync`,
    /// `forwarding`, `sync-wait` (and, for a runtime with one thread and one
    /// stack, `sync-stack`), and `value-<tag>` for each tagged value it
    /// takes. Capabilities grant no permission.
    pub capabilities: Vec<String>,
}

impl Endpoint {
    /// This implementation (rutis-bridge), as `local`, with what it
    /// supports.
    pub fn rust(local: PeerId) -> Self {
        Self {
            local,
            expected: None,
            implementation: Implementation {
                name: "rutis-bridge".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: [
                "objects",
                "signals",
                "reentrant-sync",
                "forwarding",
                "sync-wait",
                EVENTS_CAPABILITY,
            ]
            .into_iter()
            .chain(VALUE_CAPABILITIES)
            .map(String::from)
            .collect(),
        }
    }

    pub fn expect(mut self, peer: PeerId) -> Self {
        self.expected = Some(peer);
        self
    }
}

/// What the far end said of itself in an endpoint-format handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Greeting {
    pub endpoint: PeerId,
    pub implementation: Option<Implementation>,
    pub capabilities: Vec<String>,
}

/// Why a far end's protocol version is refused, saying which end to upgrade
/// (standard Q6.2.4): this side when the far end is newer, naming what to
/// upgrade here; the far end when it is older, naming its implementation
/// when its handshake said.
fn version_mismatch(far: u32, local: u32, implementation: Option<&Implementation>) -> String {
    let mismatch = format!("the far end speaks protocol {far}, this side {local}");
    if far > local {
        format!("{mismatch}: upgrade this side (Rust crates rutis-host / rutis-bridge)")
    } else if far < local {
        match implementation {
            Some(Implementation { name, version }) => {
                format!("{mismatch}: upgrade the far end ({name} {version})")
            }
            None => format!("{mismatch}: upgrade the far end"),
        }
    } else {
        mismatch
    }
}

/// Why a far end that does not declare [`EVENTS_CAPABILITY`] is refused
/// (`docs/design-events-protocol-2026-10-18.md` §2.2): `far` names it.
fn without_events(far: &str) -> String {
    format!(
        "the far end ({far}) does not declare events-v2 (the event protocol of rutis 0.13): \
         upgrade it to rutis 0.13 or later"
    )
}

/// Whether the control operation `method` is handled on the reader, before
/// the next frame (events protocol §4.1, §5.5): the event table changes
/// (`events.on`, `events.off`, `events.elsewhere`), and the `emit`
/// dispatches and invocations, which start in arrival order.
fn inline_event_op(method: &str, args: &Value) -> bool {
    match method {
        "events.on" | "events.off" | "events.elsewhere" => true,
        "events.dispatch" | "events.call" => {
            let first = match args {
                Value::List(items) => items.first(),
                _ => None,
            };
            let mode = match first {
                Some(Value::Record(fields)) => match fields.get("mode") {
                    Some(Value::Data(Json::String(mode))) => Some(mode.as_str()),
                    _ => None,
                },
                Some(Value::Data(Json::Object(fields))) => {
                    fields.get("mode").and_then(Json::as_str)
                }
                _ => match args {
                    Value::Data(Json::Array(items)) => items
                        .first()
                        .and_then(|first| first.get("mode"))
                        .and_then(Json::as_str),
                    _ => None,
                },
            };
            mode == Some("emit")
        }
        _ => false,
    }
}

/// One session's state. The session is reached through [`Connection`];
/// the channel underneath is any [`Channel`].
struct SessionState {
    /// Unique among processes ([`next_tag`]); marks this session's call
    /// ids in chains forwarded to other sessions (see [`rebase`]).
    tag: String,
    format: Format,
    /// The far end as the channel's connector verified it, if it did.
    verified: Option<PeerId>,
    /// This side's call ids: `rust:` or `<endpoint>:`.
    local_prefix: String,
    /// The far end's: `node:`, or `<endpoint>:` once it greeted.
    remote_prefix: std::sync::OnceLock<String>,
    greeting: std::sync::OnceLock<Greeting>,
    /// What the far end declared in a compat-format handshake, if anything.
    compat_capabilities: std::sync::OnceLock<Vec<String>>,
    writer: Mutex<Box<dyn Sender>>,
    closer: Arc<dyn Closer>,
    calls: Mutex<Calls>,
    exports: Mutex<Exports>,
    imports: Mutex<HashMap<u64, Weak<Import>>>,
    executor: Handle,
    background: Mutex<Option<Background>>,
    dispatch: Arc<dyn Dispatch>,
    ready: watch::Sender<Option<Result<(), Error>>>,
    ended: watch::Sender<bool>,
    activity: Arc<Activity>,
    /// The far end's `events.*` operations, run one after another in
    /// arrival order by one task, never on the reader ([`Ordered`]).
    ordered: Mutex<Ordered>,
}

/// The event operations of a session waiting for their turn (events
/// protocol §4.1, §5.5): each is the id of an admitted call, and whether it
/// runs in its turn (table changes, `emit`) or only starts there, as a task
/// of its own (the other modes, which wait for results).
#[derive(Default)]
struct Ordered {
    busy: bool,
    queue: std::collections::VecDeque<(String, bool)>,
}
impl Drop for SessionState {
    fn drop(&mut self) {
        self.closer.close("session dropped");
    }
}

#[derive(Clone)]
pub struct Connection(Arc<SessionState>);

/// A [`Connection`] that does not keep its session alive: for what the
/// session's own dispatch holds (a cycle otherwise).
#[derive(Clone)]
pub struct WeakConnection(Weak<SessionState>);
impl WeakConnection {
    /// The session, while something else keeps it.
    pub fn upgrade(&self) -> Option<Connection> {
        self.0.upgrade().map(Connection)
    }
}

impl Connection {
    /// This session, held weakly.
    pub fn downgrade(&self) -> WeakConnection {
        WeakConnection(Arc::downgrade(&self.0))
    }
    /// Start a session on `channel`: sends `hello` and reads frames on a
    /// thread of its own. When the channel ends, the session closes with
    /// `peer disconnected` (a normal end) or the channel's reason.
    pub fn open(channel: Channel, dispatch: Arc<dyn Dispatch>) -> Result<Self, Error> {
        Self::open_with(channel, dispatch, Format::Compat)
    }

    /// [`Connection::open`] in `format`. In the endpoint format the session
    /// is ready only once the far end greeted with the endpoint the channel
    /// verified (if it verified one) and this side expects (if it expects
    /// one); otherwise [`Connection::ready`] fails with [`Error::Handshake`].
    pub fn open_with(
        channel: Channel,
        dispatch: Arc<dyn Dispatch>,
        format: Format,
    ) -> Result<Self, Error> {
        let Channel {
            sender,
            mut receiver,
            closer,
            info,
        } = channel;
        let label = info.label;
        let (local_prefix, remote_prefix, hello) = match &format {
            Format::Compat => (
                "rust:".to_owned(),
                std::sync::OnceLock::from("node:".to_owned()),
                // Older runtimes ignore capabilities in a compat handshake.
                Frame::Hello {
                    version: VERSION,
                    endpoint: None,
                    implementation: None,
                    capabilities: Some(COMPAT_CAPABILITIES.map(String::from).to_vec()),
                },
            ),
            Format::Endpoint(endpoint) => (
                format!("{}:", endpoint.local),
                std::sync::OnceLock::new(),
                Frame::Hello {
                    version: crate::session::ENDPOINT_PROTOCOL,
                    endpoint: Some(endpoint.local.to_string()),
                    implementation: Some(endpoint.implementation.clone()),
                    capabilities: Some(endpoint.capabilities.clone()),
                },
            ),
        };
        let peer = Self(Arc::new(SessionState {
            tag: next_tag(),
            format,
            verified: info.peer,
            local_prefix,
            remote_prefix,
            greeting: std::sync::OnceLock::new(),
            compat_capabilities: std::sync::OnceLock::new(),
            closer,
            writer: Mutex::new(sender),
            calls: Mutex::new(Calls::default()),
            exports: Mutex::new(Exports::default()),
            imports: Mutex::new(HashMap::new()),
            executor: Handle::current(),
            background: Mutex::new(None),
            dispatch,
            ready: watch::channel(None).0,
            ended: watch::channel(false).0,
            activity: Arc::new(Activity {
                count: Mutex::new(0),
                changed: Notify::new(),
            }),
            ordered: Mutex::default(),
        }));
        // Greet before reading (#239). A side that finds the far end's
        // greeting incompatible closes the channel; had it not greeted yet,
        // the far end would see only the channel end, not which protocol
        // this side speaks. Greeting first, the far end always reads this
        // side's greeting, which the channel delivers before its end.
        //
        // A far end that does not greet first may have greeted and closed
        // before this side greets. Its greeting is still on the channel, so
        // a failed greeting does not end the session here: the reader reads
        // what the far end sent, and `ready` reports its greeting as
        // incompatible, or else the channel end.
        {
            let mut writer = peer.0.writer.lock().unwrap();
            let _ = peer.write_locked(&mut writer, hello);
        }
        let weak = Arc::downgrade(&peer.0);
        std::thread::Builder::new()
            .name("rutis-reader".into())
            // Set, not left to RUST_MIN_STACK: a frame within
            // MAX_FRAME_DEPTH takes about 640 KiB to parse in a debug build.
            .stack_size(READER_STACK)
            .spawn(move || {
                let result: Result<(), Error> = (|| loop {
                    let message = match receiver.recv() {
                        Ok(Some(message)) => message,
                        Ok(None) => return Err(Error::Transport("peer disconnected".into())),
                        Err(error) => return Err(ended(&label, error)),
                    };
                    let Some(peer) = weak.upgrade().map(Self) else {
                        return Ok(());
                    };
                    if deeper_than(&message, MAX_FRAME_DEPTH) {
                        peer.too_deep(&message)?;
                        continue;
                    }
                    peer.receive(parse_frame(&message)?)?;
                })();
                if let (Err(error), Some(peer)) = (result, weak.upgrade()) {
                    Self(peer).close(error);
                }
            })
            .map_err(transport)?;
        Ok(peer)
    }

    /// What ended the session, once it ended.
    pub fn close_reason(&self) -> Option<Error> {
        self.0.calls.lock().unwrap().closed.clone()
    }

    /// What the far end said of itself (endpoint format), once it greeted.
    pub fn greeting(&self) -> Option<&Greeting> {
        self.0.greeting.get()
    }

    /// Whether the far end declared `capability`: in the endpoint format,
    /// any; in the compat format, only those of [`COMPAT_DECLARED`] (older
    /// far ends declare nothing there).
    pub fn supports(&self, capability: &str) -> bool {
        let declared = match self.greeting() {
            Some(greeting) => &greeting.capabilities,
            None => match self.0.compat_capabilities.get() {
                Some(capabilities) => capabilities,
                None => return false,
            },
        };
        declared.iter().any(|c| c == capability)
    }
    /// Whether this side declared `capability` in its handshake.
    fn declares(&self, capability: &str) -> bool {
        match &self.0.format {
            Format::Compat => COMPAT_CAPABILITIES.contains(&capability),
            Format::Endpoint(endpoint) => endpoint.capabilities.iter().any(|c| c == capability),
        }
    }
    /// After the handshake: what the far end does while it waits
    /// synchronously, for the wait-cycle check ([`waits`]).
    fn record_profile(&self) {
        waits::profile(
            &self.0.tag,
            waits::Profile {
                // One thread, one stack: declared, not inferred (a Rust far
                // end marks its waits but runs each call on its own thread).
                stacked: self.supports("sync-wait") && self.supports("sync-stack"),
                // Not declared: taken as non-reentrant (#228).
                reentrant: self.supports("reentrant-sync"),
            },
        );
    }
    pub async fn ready(&self) -> Result<(), Error> {
        let mut ready = self.0.ready.subscribe();
        loop {
            if let Some(result) = ready.borrow().clone() {
                return result;
            }
            ready.changed().await.map_err(transport)?;
        }
    }
    pub async fn closed(&self) {
        let mut ended = self.0.ended.subscribe();
        while !*ended.borrow() {
            if ended.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn close(&self, error: Error) {
        // Interrupt a blocked write before taking the writer lock. Serialize
        // table teardown with encoding/sending so no export can be added after
        // teardown and no partial encoding rollback races the cleared table.
        self.0.closer.close(&error.to_string());
        let writer = self.0.writer.lock().unwrap();
        let (waiting, incoming, awaiting) = {
            let mut calls = self.0.calls.lock().unwrap();
            if calls.closed.is_some() {
                return;
            }
            calls.closed = Some(error.clone());
            waits::ended(&self.0.tag);
            (
                std::mem::take(&mut calls.waiting),
                std::mem::take(&mut calls.incoming),
                std::mem::take(&mut calls.awaiting),
            )
        };
        let exports = std::mem::take(&mut *self.0.exports.lock().unwrap());
        self.0.imports.lock().unwrap().clear();
        drop(writer);
        self.0.ready.send_if_modified(|ready| {
            if ready.is_none() {
                *ready = Some(Err(error.clone()));
                true
            } else {
                false
            }
        });
        self.0.ended.send_replace(true);
        for call in waiting.into_values() {
            finish(call, Err(error.clone()));
        }
        drop(exports);
        drop(incoming);
        drop(awaiting);
        self.0.background.lock().unwrap().take();
    }
    /// Create an owned async result on this connection's shared executor.
    ///
    /// Adapter code must establish that the factory and Future can progress
    /// independently of the caller's executor: `Send` alone is insufficient.
    /// Create timers/tasks inside the factory, not on the caller's runtime.
    /// Arbitrary captured runtime handles or lifecycle tasks are not migrated.
    pub fn independent_future<F, Fut>(&self, factory: F) -> Result<Value, Error>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        Ok(Value::future_on(
            async move { factory().await },
            true,
            self.background()?,
        ))
    }
    fn background(&self) -> Result<Executor, Error> {
        let mut background = self.0.background.lock().unwrap();
        if let Some(error) = &self.0.calls.lock().unwrap().closed {
            return Err(error.clone());
        }
        if background.is_none() {
            *background = Some(Background::start()?);
        }
        Ok(background.as_ref().unwrap().executor.clone())
    }
    /// This session's tag, unique among the processes a call chain crosses
    /// (`s`, a 15-digit random nonce of the process and a counter; see
    /// [`rebase`]).
    pub fn tag(&self) -> &str {
        &self.0.tag
    }
    /// Run `call`, which calls into this session on behalf of a call that
    /// arrived on `source`, with the current invocation chain rebased from
    /// `source` to this session. Without it the ids of the two sessions mix,
    /// and a reverse call can be routed to the wrong waiting thread.
    pub fn forward<R>(&self, source: &Connection, call: impl FnOnce() -> R) -> R {
        let _path = PathGuard::enter(rebase(&current_path(), source.tag(), self.tag()));
        call()
    }
    /// The async form of [`Connection::forward`]: the chain is taken when
    /// this is called and is current whenever `future` is polled.
    pub fn forward_async<F: Future>(
        &self,
        source: &Connection,
        future: F,
    ) -> impl Future<Output = F::Output> {
        with_path(rebase(&current_path(), source.tag(), self.tag()), future)
    }
    pub fn invoke(&self, target: &str, method: &str, args: Value) -> Reply {
        self.request_sync(Operation::Invoke(target.into(), method.into(), args))
    }
    pub async fn invoke_async(&self, target: &str, method: &str, args: Value) -> Reply {
        self.request_async(Operation::Invoke(target.into(), method.into(), args))
            .await
    }
    /// [`Connection::invoke_async`], its frame written before this returns:
    /// calls started one after another reach the far end in that order,
    /// whenever their replies are awaited. Dropping the returned future
    /// cancels the call (`cancel`), as dropping `invoke_async`'s does.
    pub fn invoke_started(
        &self,
        target: &str,
        method: &str,
        args: Value,
    ) -> Result<impl Future<Output = Reply> + Send + 'static, Error> {
        let (sender, receiver) = oneshot::channel();
        let id = self.send(
            Operation::Invoke(target.into(), method.into(), args),
            Waiting::Async(sender),
        )?;
        let guard = CancelOnDrop {
            peer: Arc::downgrade(&self.0),
            id,
            done: false,
        };
        Ok(async move {
            let mut guard = guard;
            let result = receiver.await.map_err(transport);
            guard.done = true;
            result?
        })
    }
    /// [`Connection::invoke_started`], settled: a future the far end answers
    /// with is awaited too. Dropped before that, the call is cancelled,
    /// also after the far end answered with its future (it is still
    /// running it).
    pub fn invoke_settled(
        &self,
        target: &str,
        method: &str,
        args: Value,
    ) -> Result<impl Future<Output = Reply> + Send + 'static, Error> {
        let (sender, receiver) = oneshot::channel();
        let id = self.send(
            Operation::Invoke(target.into(), method.into(), args),
            Waiting::Async(sender),
        )?;
        let guard = CancelOnDrop {
            peer: Arc::downgrade(&self.0),
            id,
            done: false,
        };
        Ok(async move {
            let mut guard = guard;
            let settled = match receiver.await.map_err(transport) {
                Ok(Ok(value)) => settle(value).await,
                Ok(Err(error)) | Err(error) => Err(error),
            };
            guard.done = true;
            settled
        })
    }
    pub async fn drain(&self) {
        loop {
            let changed = self.0.activity.changed.notified();
            if *self.0.activity.count.lock().unwrap() == 0 {
                return;
            }
            changed.await;
        }
    }
    fn write(&self, frame: Frame) -> Result<(), Error> {
        let result = {
            let mut writer = self.0.writer.lock().unwrap();
            self.write_locked(&mut writer, frame)
        };
        if let Err(error) = &result {
            self.close(error.clone());
        }
        result
    }
    fn write_locked(&self, writer: &mut Box<dyn Sender>, frame: Frame) -> Result<(), Error> {
        if let Some(error) = &self.0.calls.lock().unwrap().closed {
            return Err(error.clone());
        }
        let bytes = serde_json::to_vec(&frame).map_err(transport)?;
        writer.send(&bytes).map_err(|error| ended("", error))
    }
    fn send(&self, call: Operation, mut waiting: Waiting) -> Result<String, Error> {
        let mut writer = self.0.writer.lock().unwrap();
        if !matches!(*self.0.ready.borrow(), Some(Ok(()))) {
            return Err(Error::Transport("protocol handshake incomplete".into()));
        }
        let id = {
            let mut calls = self.0.calls.lock().unwrap();
            if let Some(error) = &calls.closed {
                return Err(error.clone());
            }
            calls.next = calls
                .next
                .checked_add(1)
                .ok_or_else(|| transport("call identifiers exhausted"))?;
            (format!("{}{}", self.0.local_prefix, calls.next), calls.next)
        };
        let (id, sequence) = id;
        let mut path = current_path();
        // A call forwarded for a caller that waits synchronously: refused,
        // not sent, if it would close a cycle of synchronous waits (#228).
        let hop = waits::forward(&self.0.tag, &id, sequence, &path).map_err(|cycle| {
            Error::SyncWaitCycle(format!(
                "this synchronous call would wait on itself: {}",
                cycle.join(" -> ")
            ))
        })?;
        // Leaves the wait graph if the call is not sent after all.
        let unsent = Unsent(hop.then(|| (self.0.tag.clone(), id.clone())));
        let sync = matches!(waiting, Waiting::Sync { .. }) && self.supports("sync-wait");
        if let Operation::Await(_, origin) = &call {
            if let Waiting::Sync { origins, .. } = &mut waiting {
                *origins = origin.clone();
            }
            for ancestor in origin {
                if !path.contains(ancestor) {
                    path.push(ancestor.clone());
                }
            }
        }
        let frame = match &call {
            Operation::Invoke(target, method, args) => Frame::Invoke {
                id: id.clone(),
                path,
                target: target.clone(),
                method: method.clone(),
                args: self.encode(args, MAX_DEPTH + 1)?,
                sync,
            },
            Operation::Call(reference, method, args) => Frame::Call {
                id: id.clone(),
                path,
                reference: *reference,
                method: method.clone(),
                args: self.encode(args, MAX_DEPTH + 1)?,
                sync,
            },
            Operation::Get(reference, property) => Frame::Get {
                id: id.clone(),
                path,
                reference: *reference,
                property: property.clone(),
                sync,
            },
            Operation::Await(reference, _) => Frame::Await {
                id: id.clone(),
                path,
                reference: *reference,
                sync,
            },
        };
        let mut blocked = Vec::new();
        {
            let mut calls = self.0.calls.lock().unwrap();
            if let Waiting::Sync { sender, origins } = &waiting {
                for (id, awaiting) in &calls.awaiting {
                    // Forwarded relay calls run on the background executor
                    // and never wait on this one.
                    if matches!(awaiting.object.body, Body::Future(_))
                        && awaiting.object.executor.blocked_here()
                        && origins.iter().any(|origin| awaiting.path.contains(origin))
                    {
                        blocked.push((id.clone(), awaiting.object.clone()));
                    }
                }
                // Claim queued work atomically with registering this waiter.
                // A runtime task may already be scheduled, but has not begun.
                let mut related: Vec<_> = calls
                    .incoming
                    .iter()
                    .filter(|(_, incoming)| {
                        incoming.path.contains(&id)
                            || origins.iter().any(|origin| incoming.path.contains(origin))
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                let remote = self.remote_prefix();
                related.sort_by_key(|id| id.strip_prefix(remote).unwrap().parse::<u64>().unwrap());
                for id in related {
                    let incoming = calls.incoming.remove(&id).unwrap();
                    sender.send(Message::Invoke(incoming)).map_err(transport)?;
                }
            }
            calls.waiting.insert(id.clone(), waiting);
        }
        let result = self.write_locked(&mut writer, frame);
        drop(writer);
        if let Err(error) = result {
            self.close(error.clone());
            return Err(error);
        }
        unsent.sent();
        for (id, object) in blocked {
            self.finish_await(id, object.ready_on_blocked_executor());
        }
        Ok(id)
    }
    fn request_sync(&self, call: Operation) -> Reply {
        let (sender, receiver) = mpsc::channel();
        self.send(
            call,
            Waiting::Sync {
                sender,
                origins: Vec::new(),
            },
        )?;
        loop {
            match receiver.recv().map_err(transport)? {
                Message::Reply(result) => return result,
                Message::Invoke(incoming) => {
                    let _pumping = Pumping::enter();
                    self.execute(incoming, true)
                }
            }
        }
    }
    async fn request_async(&self, call: Operation) -> Reply {
        let (sender, receiver) = oneshot::channel();
        let id = self.send(call, Waiting::Async(sender))?;
        let mut guard = CancelOnDrop {
            peer: Arc::downgrade(&self.0),
            id,
            done: false,
        };
        let result = receiver.await.map_err(transport);
        guard.done = true;
        result?
    }
    /// Give up on an outstanding call: the callee's AbortSignal (if any) is
    /// aborted and a late reply is discarded.
    fn cancel(&self, id: &str) {
        {
            let mut calls = self.0.calls.lock().unwrap();
            if calls.closed.is_some() {
                return;
            }
            // Still waiting: a late reply will be discarded. Already answered
            // but not consumed (e.g. a returned Promise the caller has not yet
            // awaited): the callee may still be running it, so cancel anyway.
            if calls.waiting.remove(id).is_some() {
                calls.cancelled.insert(id.to_owned());
            }
        }
        waits::returned(&self.0.tag, id);
        let _ = self.write(Frame::Cancel { id: id.to_owned() });
    }
    /// Late replies discarded after cancellations.
    pub fn orphans(&self) -> u64 {
        self.0.calls.lock().unwrap().orphans
    }
    /// How many references this side exports (granted and not yet released
    /// by the far end) and imports (received and not yet released), for the
    /// release-count invariant (risk P4).
    #[cfg(feature = "testing")]
    pub fn references(&self) -> (usize, usize) {
        let exports = self.0.exports.lock().unwrap().entries.len();
        let imports = self.0.imports.lock().unwrap().len();
        (exports, imports)
    }
    /// Encode `value`, nested at most `levels` deep: [`MAX_DEPTH`] for a
    /// result, one more for an argument list (the list is not a level).
    fn encode(&self, value: &Value, levels: usize) -> Result<WireValue, Error> {
        let mut grants = Vec::new();
        let result = self.encode_inner(value, levels, &mut grants);
        if result.is_err() {
            let mut exports = self.0.exports.lock().unwrap();
            for id in grants {
                let Some(entry) = exports.entries.get_mut(&id) else {
                    continue;
                };
                entry.grants -= 1;
                if entry.grants == 0 {
                    // The value being encoded still holds the import of a
                    // dropped relay, so no release is written under the lock.
                    exports.remove(id);
                }
            }
        }
        result
    }
    fn encode_inner(
        &self,
        value: &Value,
        levels: usize,
        grants: &mut Vec<u64>,
    ) -> Result<WireValue, Error> {
        if matches!(
            value,
            Value::List(_) | Value::Record(_) | Value::Map(_) | Value::Set(_)
        ) && levels == 0
        {
            return Err(too_deep());
        }
        if let Some(tag) = value.tag() {
            self.sendable(tag, || crate::session::values::describe(value).to_owned())?;
        }
        Ok(match value {
            Value::Instant(value) => WireValue::Instant(value.to_string()),
            Value::DateTime(value) => WireValue::DateTime(value.to_string()),
            Value::Date(value) => WireValue::Date(value.to_string()),
            Value::Time(value) => WireValue::Time(value.to_string()),
            Value::Bytes(value) => WireValue::Bytes(base64_encode(value)),
            Value::BigInt(value) => WireValue::BigInt(value.to_string()),
            Value::Decimal(value) => WireValue::Decimal(value.to_string()),
            Value::Uuid(value) => WireValue::Uuid(value.to_string()),
            Value::Map(entries) => {
                check_keys(entries.iter().map(|(key, _)| key), "map key")?;
                WireValue::Map(
                    entries
                        .iter()
                        .map(|(key, value)| {
                            // Undefined is null in data (design V10).
                            let value = match value {
                                Value::Undefined => WireValue::Data(Json::Null),
                                value => self.encode_inner(value, levels - 1, grants)?,
                            };
                            Ok((self.encode_inner(key, levels - 1, grants)?, value))
                        })
                        .collect::<Result<_, Error>>()?,
                )
            }
            Value::Set(elements) => {
                check_keys(elements, "set element")?;
                WireValue::Set(
                    elements
                        .iter()
                        .map(|element| self.encode_inner(element, levels - 1, grants))
                        .collect::<Result<_, _>>()?,
                )
            }
            Value::Undefined => WireValue::Undefined,
            Value::Data(value) => {
                if !fits(value, levels) {
                    return Err(too_deep());
                }
                self.encode_data(value)?
            }
            Value::List(values) => WireValue::List(
                values
                    .iter()
                    .map(|value| self.encode_inner(value, levels - 1, grants))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Record(fields) => WireValue::Record(
                fields
                    .iter()
                    .map(|(key, value)| {
                        Ok((key.clone(), self.encode_inner(value, levels - 1, grants)?))
                    })
                    .collect::<Result<_, Error>>()?,
            ),
            Value::Signal => WireValue::Signal,
            Value::Reference(Reference(ReferenceInner::Remote(import))) => {
                self.encode_import(import, grants)?
            }
            // Decoding unwraps relays, so Rust code should never hold one;
            // if it does, forward from the innermost import all the same.
            Value::Reference(Reference(ReferenceInner::Local(object))) => match &object.body {
                Body::Relay(import) => self.encode_import(import, grants)?,
                _ => self.encode_local(object, grants)?,
            },
        })
    }
    /// Whether a value tagged `tag` may be sent: only to a far end that
    /// declared `value-<tag>`; `what` names the value for the error.
    fn sendable(&self, tag: &str, what: impl FnOnce() -> String) -> Result<(), Error> {
        let capability = format!("value-{tag}");
        if self.supports(&capability) {
            return Ok(());
        }
        Err(Error::Value(format!(
            "cannot send {}: the far end does not take {tag} values \
             (it did not declare the capability {capability})",
            what()
        )))
    }
    /// JSON data as sent: an integer beyond ±(2^53−1) crosses as a bigint
    /// (design V6), and the arrays and objects holding one as lists and
    /// records.
    fn encode_data(&self, value: &Json) -> Result<WireValue, Error> {
        if !holds_beyond_safe(value) {
            return Ok(WireValue::Data(value.clone()));
        }
        Ok(match value {
            Json::Array(items) => WireValue::List(
                items
                    .iter()
                    .map(|item| self.encode_data(item))
                    .collect::<Result<_, _>>()?,
            ),
            Json::Object(fields) => WireValue::Record(
                fields
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.encode_data(value)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            number => {
                self.sendable("bigint", || {
                    format!("the integer {number}, beyond ±(2^53-1) and so a bigint")
                })?;
                WireValue::BigInt(number.to_string())
            }
        })
    }
    fn encode_local(
        &self,
        object: &Arc<Object>,
        grants: &mut Vec<u64>,
    ) -> Result<WireValue, Error> {
        let identity = Arc::as_ptr(object) as usize;
        let mut exports = self.0.exports.lock().unwrap();
        let id = match exports.identities.get(&identity) {
            Some(id) => *id,
            None => {
                let id = allocate(&mut exports, object.clone())?;
                exports.identities.insert(identity, id);
                id
            }
        };
        let mut wire = grant(&mut exports, id, grants)?;
        // Runtimes before #225 accept only their own ids in a compat-format
        // origin, as for relays; the object keeps its whole chain here.
        if let (Format::Compat, WireValue::Reference { origin, .. }) = (&self.0.format, &mut wire) {
            origin.retain(|entry| !entry.contains('/'));
        }
        Ok(wire)
    }
    /// Decode every one of `values`, even past a value error (so each
    /// reference in them is imported); the first value error, if any. A
    /// transport error ends the session and stops at once.
    fn decode_each(
        &self,
        values: impl IntoIterator<Item = WireValue>,
    ) -> Result<Vec<Value>, Error> {
        let mut decoded = Vec::new();
        let mut refused = None;
        for value in values {
            match self.decode(value) {
                Ok(value) => decoded.push(value),
                Err(error @ Error::Value(_)) => {
                    refused.get_or_insert(error);
                }
                Err(error) => return Err(error),
            }
        }
        match refused {
            Some(error) => Err(error),
            None => Ok(decoded),
        }
    }
    /// Decode a value from the far end. A transport error ends the session;
    /// a value error (a tagged value this side did not declare, or one not
    /// in its format) fails only the call it came in.
    ///
    /// A value error does not stop the decode: every reference in the value
    /// is still imported, so dropping what was decoded releases each one
    /// on the far end, as for a value taken.
    fn decode(&self, value: WireValue) -> Reply {
        let refused = value.tag().and_then(|tag| {
            let capability = format!("value-{tag}");
            (!self.declares(&capability)).then(|| {
                Error::Value(format!(
                    "received a {tag} value, which this side does not take \
                     (it did not declare the capability {capability})"
                ))
            })
        });
        let value = match (refused, value) {
            // Their entries are still read, for the references in them.
            (Some(error), WireValue::Map(entries)) => {
                self.decode_each(entries.into_iter().flat_map(|(key, value)| [key, value]))?;
                return Err(error);
            }
            (Some(error), WireValue::Set(elements)) => {
                self.decode_each(elements)?;
                return Err(error);
            }
            (Some(error), _) => return Err(error),
            (None, value) => value,
        };
        let value = match decode_text(value) {
            Ok(decoded) => return decoded,
            Err(value) => value,
        };
        Ok(match value {
            WireValue::Instant(_)
            | WireValue::DateTime(_)
            | WireValue::Date(_)
            | WireValue::Time(_)
            | WireValue::Bytes(_)
            | WireValue::BigInt(_)
            | WireValue::Decimal(_)
            | WireValue::Uuid(_) => unreachable!("decode_text decodes the tagged texts"),
            WireValue::Map(entries) => {
                let mut decoded = self
                    .decode_each(entries.into_iter().flat_map(|(key, value)| [key, value]))?
                    .into_iter();
                let mut entries = Vec::new();
                while let (Some(key), Some(value)) = (decoded.next(), decoded.next()) {
                    // Undefined is null in data (design V10).
                    let value = match value {
                        Value::Undefined => Value::Data(Json::Null),
                        value => value,
                    };
                    entries.push((key, value));
                }
                check_keys(entries.iter().map(|(key, _)| key), "map key")?;
                Value::Map(entries)
            }
            WireValue::Set(elements) => {
                let elements = self.decode_each(elements)?;
                check_keys(&elements, "set element")?;
                Value::Set(elements)
            }
            WireValue::Undefined => Value::Undefined,
            WireValue::Data(value) => Value::Data(value),
            WireValue::List(values) => Value::List(self.decode_each(values)?),
            // A Rust callee is cancelled by its future being dropped, once
            // the caller gives the call up and releases its result; the
            // signal itself only marks the call as cancellable.
            WireValue::Signal => Value::Signal,
            WireValue::Record(fields) => {
                let (keys, values): (Vec<_>, Vec<_>) = fields.into_iter().unzip();
                Value::Record(keys.into_iter().zip(self.decode_each(values)?).collect())
            }
            WireValue::Reference {
                id,
                home: true,
                kind,
                origin: _,
            } => {
                let object = self.export(id)?;
                if object.kind() != kind {
                    return Err(transport("reference kind mismatch"));
                }
                // A relay coming home is the reference it forwards to, so
                // Rust code compares it equal to the import it sent.
                match &object.body {
                    Body::Relay(import) => {
                        Value::Reference(Reference(ReferenceInner::Remote(import.clone())))
                    }
                    _ => Value::Reference(Reference(ReferenceInner::Local(object))),
                }
            }
            WireValue::Reference {
                id,
                home: false,
                kind,
                origin,
            } => {
                if id == 0 || id > 9_007_199_254_740_991 {
                    return Err(transport("invalid reference identity"));
                }
                let mut imports = self.0.imports.lock().unwrap();
                let import = match imports.get(&id).and_then(Weak::upgrade) {
                    Some(import) => {
                        if import.kind != kind || import.origin != origin {
                            return Err(transport("reference kind mismatch"));
                        }
                        let mut grants = import.grants.lock().unwrap();
                        *grants = grants
                            .checked_add(1)
                            .ok_or_else(|| transport("import grant overflow"))?;
                        drop(grants);
                        import
                    }
                    None => {
                        let import = Arc::new(Import {
                            peer: Arc::downgrade(&self.0),
                            id,
                            kind,
                            grants: Mutex::new(1),
                            origin,
                        });
                        imports.insert(id, Arc::downgrade(&import));
                        import
                    }
                };
                Value::Reference(Reference(ReferenceInner::Remote(import)))
            }
        })
    }
    /// The waiter for a reply, `None` for a late reply to a cancelled call.
    /// The far end's call id prefix; empty (matching nothing a valid far
    /// end sends) before it greeted.
    fn remote_prefix(&self) -> &str {
        self.0.remote_prefix.get().map_or("\0", String::as_str)
    }
    /// Check the far end's handshake against this session's format.
    fn greet(
        &self,
        version: u32,
        endpoint: Option<String>,
        implementation: Option<Implementation>,
        capabilities: Option<Vec<String>>,
    ) -> Result<(), Error> {
        let incompatible = |reason: String| Error::Handshake(Handshake::Incompatible(reason));
        let Format::Endpoint(local) = &self.0.format else {
            if version != VERSION || endpoint.is_some() {
                return Err(incompatible(version_mismatch(
                    version,
                    VERSION,
                    implementation.as_ref(),
                )));
            }
            let capabilities: Vec<String> = capabilities
                .unwrap_or_default()
                .into_iter()
                .filter(|c| COMPAT_DECLARED.contains(&c.as_str()))
                .collect();
            if !capabilities.iter().any(|c| c == EVENTS_CAPABILITY) {
                return Err(incompatible(without_events(
                    "a runtime older than rutis 0.13",
                )));
            }
            let _ = self.0.compat_capabilities.set(capabilities);
            self.record_profile();
            return Ok(());
        };
        if version != crate::session::ENDPOINT_PROTOCOL {
            return Err(incompatible(version_mismatch(
                version,
                crate::session::ENDPOINT_PROTOCOL,
                implementation.as_ref(),
            )));
        }
        let endpoint = endpoint
            .ok_or_else(|| incompatible("the far end named no endpoint".into()))
            .and_then(|endpoint| {
                PeerId::new(endpoint).map_err(|error| incompatible(error.to_string()))
            })?;
        for (whose, expected) in [
            ("verified", &self.0.verified),
            ("expected", &local.expected),
        ] {
            if let Some(expected) = expected {
                if expected != &endpoint {
                    return Err(Error::Handshake(Handshake::IdentityMismatch(format!(
                        "the far end greeted as {endpoint}, but {expected} is the {whose} endpoint"
                    ))));
                }
            }
        }
        if endpoint == local.local {
            return Err(Error::Handshake(Handshake::IdentityMismatch(format!(
                "the far end greeted as this endpoint ({endpoint})"
            ))));
        }
        let capabilities = capabilities.unwrap_or_default();
        if !capabilities.iter().any(|c| c == EVENTS_CAPABILITY) {
            let named = match &implementation {
                Some(Implementation { name, version }) => format!("{name} {version}"),
                None => endpoint.to_string(),
            };
            return Err(incompatible(without_events(&named)));
        }
        let _ = self.0.remote_prefix.set(format!("{endpoint}:"));
        let _ = self.0.greeting.set(Greeting {
            endpoint,
            implementation,
            capabilities,
        });
        self.record_profile();
        Ok(())
    }
    fn reply_target(&self, id: &str) -> Result<Option<Waiting>, Error> {
        // On the reader thread, before any later frame of this session is
        // read: the order the wait-cycle check relies on.
        waits::returned(&self.0.tag, id);
        let mut calls = self.0.calls.lock().unwrap();
        if let Some(waiting) = calls.waiting.remove(id) {
            return Ok(Some(waiting));
        }
        if calls.cancelled.remove(id) {
            calls.orphans += 1;
            return Ok(None);
        }
        Err(transport("response for unknown call"))
    }
    fn export(&self, id: u64) -> Result<Arc<Object>, Error> {
        self.0
            .exports
            .lock()
            .unwrap()
            .entries
            .get(&id)
            .map(|e| e.object.clone())
            .ok_or_else(|| transport("unknown or released reference"))
    }
    /// A frame nested deeper than [`MAX_FRAME_DEPTH`], from a far end that
    /// does not keep values within [`MAX_DEPTH`]: its call fails, a call to
    /// this side with a throw and a reply by failing the call waiting for it;
    /// the session goes on. References it carries are not read, so they stay
    /// granted until the session ends. A frame that names no call ends the
    /// session.
    fn too_deep(&self, text: &[u8]) -> Result<(), Error> {
        #[derive(serde::Deserialize)]
        struct Head {
            op: String,
            #[serde(default)]
            id: Option<String>,
        }
        let message = format!(
            "frame nested deeper than {MAX_FRAME_DEPTH} levels: a value crossing a session \
             may nest at most {MAX_DEPTH} levels"
        );
        // serde_json skips the fields not named here without recursing.
        let head = serde_json::from_slice::<Head>(text).ok();
        match head {
            Some(Head { op, id: Some(id) }) if op == "return" || op == "throw" => {
                if !matches!(*self.0.ready.borrow(), Some(Ok(()))) {
                    return Err(transport("request before protocol handshake"));
                }
                if let Some(waiting) = self.reply_target(&id)? {
                    finish(waiting, Err(Error::Value(message)));
                }
                Ok(())
            }
            Some(Head { op, id: Some(id) })
                if matches!(op.as_str(), "invoke" | "call" | "get" | "await")
                    && matches!(*self.0.ready.borrow(), Some(Ok(())))
                    && id.starts_with(self.remote_prefix()) =>
            {
                let _ = self.write(Frame::Throw {
                    id,
                    error: Error::Value(message).into(),
                });
                Ok(())
            }
            _ => Err(Error::Transport(message)),
        }
    }
    /// The arguments of the far end's call `id`; `None` when they hold a
    /// value this side does not take (one it did not declare, or not in
    /// its format): that call fails with a throw saying why, and the
    /// session goes on.
    fn decode_args(&self, id: &str, args: WireValue) -> Result<Option<Value>, Error> {
        match self.decode(args) {
            Ok(args) => Ok(Some(args)),
            Err(error @ Error::Value(_)) => {
                if !id.starts_with(self.remote_prefix()) {
                    return Err(transport("invalid invocation identity/path"));
                }
                // Refused, it is still a call: its id is taken as a
                // completed call's is, so sending it again does not run it.
                {
                    let mut calls = self.0.calls.lock().unwrap();
                    if let Some(error) = &calls.closed {
                        return Err(error.clone());
                    }
                    calls.admit(id.strip_prefix(self.remote_prefix()))?;
                }
                let _ = self.write(Frame::Throw {
                    id: id.to_owned(),
                    error: error.into(),
                });
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    fn receive(&self, frame: Frame) -> Result<(), Error> {
        if let Some(error) = &self.0.calls.lock().unwrap().closed {
            return Err(error.clone());
        }
        if let Frame::Hello {
            version,
            endpoint,
            implementation,
            capabilities,
        } = frame
        {
            if self.0.ready.borrow().is_some() {
                return Err(transport("duplicate protocol handshake"));
            }
            self.greet(version, endpoint, implementation, capabilities)?;
            self.0.ready.send_replace(Some(Ok(())));
            return Ok(());
        }
        if !matches!(*self.0.ready.borrow(), Some(Ok(()))) {
            return Err(transport("request before protocol handshake"));
        }
        let (id, mut path, call, sync) = match frame {
            Frame::Return { id, value } => {
                // Decode first: a discarded late reply still releases the
                // references it granted when its values drop. A value this
                // side cannot take fails the call waiting for it.
                let value = match self.decode(value) {
                    Err(error @ Error::Value(_)) => Err(error),
                    value => Ok(value?),
                };
                if let Some(waiting) = self.reply_target(&id)? {
                    finish(waiting, value);
                }
                return Ok(());
            }
            Frame::Throw { id, error } => {
                if let Some(waiting) = self.reply_target(&id)? {
                    finish(waiting, Err(error.into()));
                }
                return Ok(());
            }
            Frame::Cancel { id } => {
                // Stop awaiting on the caller's behalf; queued work is dropped.
                // A synchronous call already running completes normally.
                let (awaiting, incoming) = {
                    let mut calls = self.0.calls.lock().unwrap();
                    (calls.awaiting.remove(&id), calls.incoming.remove(&id))
                };
                // The caller no longer waits for it.
                waits::answered(&self.0.tag, &id);
                drop((awaiting, incoming));
                return Ok(());
            }
            Frame::Release { reference, count } => {
                let removed = {
                    let mut exports = self.0.exports.lock().unwrap();
                    let entry = exports
                        .entries
                        .get_mut(&reference)
                        .ok_or_else(|| transport("release of unknown reference"))?;
                    if count == 0 || count > entry.grants {
                        return Err(transport("invalid reference release count"));
                    }
                    entry.grants -= count;
                    if entry.grants == 0 {
                        exports.remove(reference)
                    } else {
                        None
                    }
                };
                // Outside the lock: a dropped relay may release its import,
                // which writes to the other session.
                drop(removed);
                return Ok(());
            }
            Frame::Invoke {
                id,
                path,
                target,
                method,
                args,
                sync,
            } => {
                let Some(args) = self.decode_args(&id, args)? else {
                    return Ok(());
                };
                (id, path, Accepted::Invoke(target, method, args), sync)
            }
            // Rust exports objects only as relays of other sessions' objects.
            Frame::Call {
                id,
                path,
                reference,
                method: Some(method),
                args,
                sync,
            } => {
                let object = self.export_object(reference)?;
                let Some(args) = self.decode_args(&id, args)? else {
                    return Ok(());
                };
                (id, path, Accepted::Method(object, method, args), sync)
            }
            Frame::Get {
                id,
                path,
                reference,
                property,
                sync,
            } => (
                id,
                path,
                Accepted::Get(self.export_object(reference)?, property),
                sync,
            ),
            Frame::Call {
                id,
                path,
                reference,
                method: None,
                args,
                sync,
            } => {
                let object = self.export(reference)?;
                let Some(args) = self.decode_args(&id, args)? else {
                    return Ok(());
                };
                (id, path, Accepted::Call(object, args), sync)
            }
            Frame::Await {
                id,
                path,
                reference,
                sync,
            } => (id, path, Accepted::Await(self.export(reference)?), sync),
            Frame::Hello { .. } => unreachable!(),
        };
        if !id.starts_with(self.remote_prefix()) || path.contains(&id) {
            return Err(transport("invalid invocation identity/path"));
        }
        path.push(id.clone());
        let business = match &call {
            Accepted::Invoke(target, _, _) => !target.is_empty(),
            Accepted::Call(object, _)
            | Accepted::Await(object)
            | Accepted::Method(object, _, _)
            | Accepted::Get(object, _) => object.business,
        };
        let flight = business.then(|| {
            *self.0.activity.count.lock().unwrap() += 1;
            Flight(self.0.activity.clone())
        });
        let incoming = Incoming {
            id,
            path,
            call,
            _flight: flight,
        };
        let handle = match &incoming.call {
            Accepted::Call(object, _)
            | Accepted::Await(object)
            | Accepted::Method(object, _, _)
            | Accepted::Get(object, _) => object.executor.handle.clone(),
            Accepted::Invoke(..) => self.0.executor.clone(),
        };
        // Event operations run in frame order (events protocol §4.1, §5.5):
        // table changes and emits each in its turn, the rest started in it.
        // Never here: a reader runs no plugin code (a listener, an
        // observer), which could wait for a frame only this reader reads.
        let event_op = matches!(
            &incoming.call,
            Accepted::Invoke(target, method, _) if target.is_empty() && method.starts_with("events.")
        );
        let inline = matches!(
            &incoming.call,
            Accepted::Invoke(target, method, args) if target.is_empty() && inline_event_op(method, args)
        );
        // Its caller waits for it synchronously: what it forwards to another
        // session is checked for wait cycles. Marked before anything can run
        // or answer it, and outside this session's locks (the table is the
        // whole process's); a call refused below closes the session, which
        // drops its marks.
        if sync {
            waits::marked(&self.0.tag, &incoming.id);
        }
        let delivery = {
            let mut calls = self.0.calls.lock().unwrap();
            if let Some(error) = &calls.closed {
                return Err(error.clone());
            }
            calls.admit(incoming.id.strip_prefix(self.remote_prefix()))?;
            let waiter = incoming
                .path
                .iter()
                .rev()
                .find_map(|id| match calls.waiting.get(id) {
                    Some(Waiting::Sync { sender, .. }) => Some(sender.clone()),
                    _ => None,
                })
                .or_else(|| {
                    calls.waiting.values().find_map(|waiting| match waiting {
                        Waiting::Sync { sender, origins }
                            if origins.iter().any(|id| incoming.path.contains(id)) =>
                        {
                            Some(sender.clone())
                        }
                        _ => None,
                    })
                });
            if let Some(waiter) = waiter {
                Ok((waiter, incoming))
            } else {
                let id = incoming.id.clone();
                calls.incoming.insert(id.clone(), incoming);
                Err(id)
            }
        };
        if let (Err(id), true) = (&delivery, event_op) {
            self.run_ordered(id.clone(), inline);
        } else if let Err(id) = delivery {
            let peer = self.clone();
            handle.spawn(async move {
                let incoming = peer.0.calls.lock().unwrap().incoming.remove(&id);
                if let Some(incoming) = incoming {
                    peer.execute(incoming, false);
                }
            });
        } else if let Ok((waiter, incoming)) = delivery {
            // A dropped receiver may release imported arguments. Do not run
            // their destructors (and protocol writes) under the calls lock.
            waiter.send(Message::Invoke(incoming)).map_err(transport)?;
        }
        Ok(())
    }
    /// Queue the admitted event operation `id` behind the session's others;
    /// the session's ordered task takes it in its turn.
    fn run_ordered(&self, id: String, inline: bool) {
        {
            let mut ordered = self.0.ordered.lock().unwrap();
            ordered.queue.push_back((id, inline));
            if ordered.busy {
                return;
            }
            ordered.busy = true;
        }
        let peer = self.clone();
        self.0.executor.spawn(async move { peer.drain_ordered() });
    }
    /// The session's ordered task: each queued event operation in arrival
    /// order, run (`inline`) or started as a task, until none is left.
    fn drain_ordered(&self) {
        loop {
            let (id, inline) = {
                let mut ordered = self.0.ordered.lock().unwrap();
                match ordered.queue.pop_front() {
                    Some(next) => next,
                    None => {
                        ordered.busy = false;
                        return;
                    }
                }
            };
            if inline {
                let incoming = self.0.calls.lock().unwrap().incoming.remove(&id);
                if let Some(incoming) = incoming {
                    self.execute(incoming, false);
                }
            } else {
                let peer = self.clone();
                self.0.executor.spawn(async move {
                    let incoming = peer.0.calls.lock().unwrap().incoming.remove(&id);
                    if let Some(incoming) = incoming {
                        peer.execute(incoming, false);
                    }
                });
            }
        }
    }
    fn execute(&self, incoming: Incoming, sync: bool) {
        if self.0.calls.lock().unwrap().closed.is_some() {
            return;
        }
        let Incoming {
            id,
            path,
            call,
            _flight,
        } = incoming;
        let _path = PathGuard::enter(path.clone());
        let call = match call {
            Accepted::Call(object, args) if matches!(object.body, Body::Relay(_)) => {
                return self.relay(id, path, object, Relayed::Call(None, args), sync, _flight);
            }
            Accepted::Await(object) if matches!(object.body, Body::Relay(_)) => {
                return self.relay(id, path, object, Relayed::Await, sync, _flight);
            }
            Accepted::Method(object, method, args) => {
                return self.relay(
                    id,
                    path,
                    object,
                    Relayed::Call(Some(method), args),
                    sync,
                    _flight,
                );
            }
            Accepted::Get(object, property) => {
                return self.relay(id, path, object, Relayed::Get(property), sync, _flight);
            }
            call => call,
        };
        match call {
            Accepted::Method(..) | Accepted::Get(..) => unreachable!("objects are relays"),
            Accepted::Await(object) => {
                // Check on the pumping thread before handing the async waiter
                // to the executor that might itself be blocked by this stack.
                if sync && object.executor.blocked_here() {
                    self.respond(id, object.ready_on_blocked_executor());
                    drop(_flight);
                    return;
                }
                let (cancel, cancelled) = oneshot::channel();
                {
                    let mut calls = self.0.calls.lock().unwrap();
                    if calls.closed.is_some() {
                        return;
                    }
                    calls.awaiting.insert(
                        id.clone(),
                        Awaiting {
                            object: object.clone(),
                            path: path.clone(),
                            _cancel: cancel,
                        },
                    );
                }
                let peer = self.clone();
                object
                    .executor
                    .handle
                    .clone()
                    .spawn(ASYNC_PATH.scope(path.clone(), async move {
                        tokio::select! {
                            result = object.wait(false, path) => peer.finish_await(id, result),
                            _ = cancelled => {},
                        }
                        drop(_flight);
                    }));
            }
            Accepted::Call(object, args) => {
                // A function this side exported, called by this session: code
                // in it that calls another session forwards from this one.
                // It names no calling row: only a call to a host service a
                // row leased does (`caller_row`).
                let previous = CALLER.with(|caller| caller.replace(Some(self.clone())));
                let result = crate::session::with_caller_row(None, || object.call(args));
                CALLER.with(|caller| caller.replace(previous));
                self.respond(id, result);
                drop(_flight);
            }
            Accepted::Invoke(target, method, args) => {
                let previous = CALLER.with(|caller| caller.replace(Some(self.clone())));
                let sequence = id
                    .strip_prefix(self.remote_prefix())
                    .and_then(|n| n.parse::<u64>().ok());
                let invoking = sequence.map(|n| (self.0.tag.clone(), n));
                let outer = INVOKING.with(|cell| cell.replace(invoking));
                let result = crate::session::with_caller_row(None, || {
                    protected(|| self.0.dispatch.invoke(self, &target, &method, args))
                });
                INVOKING.with(|cell| cell.replace(outer));
                CALLER.with(|caller| caller.replace(previous));
                self.respond(id, result);
                drop(_flight);
            }
        }
    }
    fn finish_await(&self, id: String, result: Reply) {
        let awaiting = self.0.calls.lock().unwrap().awaiting.remove(&id);
        if awaiting.is_some() {
            self.respond(id, result);
        }
    }
    fn respond(&self, id: String, result: Reply) {
        waits::answered(&self.0.tag, &id);
        let result = {
            let mut writer = self.0.writer.lock().unwrap();
            let frame = match &result {
                Ok(value) => match self.encode(value, MAX_DEPTH) {
                    Ok(value) => Frame::Return { id, value },
                    Err(error) => Frame::Throw {
                        id,
                        error: error.into(),
                    },
                },
                Err(error) => Frame::Throw {
                    id,
                    error: error.clone().into(),
                },
            };
            self.write_locked(&mut writer, frame)
        };
        if let Err(error) = result {
            self.close(error);
        }
    }
}
fn allocate(exports: &mut Exports, object: Arc<Object>) -> Result<u64, Error> {
    exports.next = exports
        .next
        .checked_add(1)
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or_else(|| transport("reference identifiers exhausted"))?;
    let id = exports.next;
    exports.entries.insert(id, Export { object, grants: 0 });
    Ok(id)
}
fn grant(exports: &mut Exports, id: u64, grants: &mut Vec<u64>) -> Result<WireValue, Error> {
    let export = exports.entries.get_mut(&id).unwrap();
    export.grants = export
        .grants
        .checked_add(1)
        .ok_or_else(|| transport("reference grant overflow"))?;
    grants.push(id);
    Ok(WireValue::Reference {
        id,
        kind: export.object.kind(),
        home: false,
        origin: export.object.origin.clone(),
    })
}
fn finish(waiting: Waiting, result: Reply) {
    match waiting {
        Waiting::Sync { sender, .. } => {
            let _ = sender.send(Message::Reply(result));
        }
        Waiting::Async(sender) => {
            let _ = sender.send(result);
        }
    }
}
/// How deep a value crossing a session may nest: an array or object (of
/// data, or a list or record holding references) is one level, so a value
/// of 64 nested arrays crosses and one of 65 does not. A call's argument
/// list is not a level: each argument may nest this deep. A value nested
/// deeper fails its own call on the side that sends it (#279).
pub const MAX_DEPTH: usize = 64;

/// How deep a frame this side reads may nest, counting every JSON array
/// and object. Each level of a value takes at most three on the wire (the
/// `{"type":"map","value":[[k, v]]}` wrapper, its array and an entry's
/// pair; a list or record takes two), the frame and argument list three
/// more, and a reference at the bottom three: a value within
/// [`MAX_DEPTH`] stays within `3 * MAX_DEPTH + 6`; the rest is margin. A
/// deeper frame fails its call, or, if no call can be told from it, ends
/// the session.
pub(crate) const MAX_FRAME_DEPTH: usize = 3 * MAX_DEPTH + 16;

/// The reader thread's stack: room for parsing, decoding and dropping a
/// frame [`MAX_FRAME_DEPTH`] deep, whatever `RUST_MIN_STACK` says.
pub(crate) const READER_STACK: usize = 2 << 20;

fn too_deep() -> Error {
    Error::Value(format!(
        "value nested deeper than {MAX_DEPTH} levels, the most a value crossing a session may nest"
    ))
}

/// Whether `value` nests at most `levels` arrays and objects deep.
fn fits(value: &Json, levels: usize) -> bool {
    match value {
        Json::Array(items) => levels > 0 && items.iter().all(|item| fits(item, levels - 1)),
        Json::Object(fields) => levels > 0 && fields.values().all(|item| fits(item, levels - 1)),
        _ => true,
    }
}

/// Whether a JSON text nests arrays and objects deeper than `limit`,
/// found without parsing it (so without recursing).
/// A tagged value carried as text (instant, datetime, date, time, bytes,
/// bigint, decimal, uuid) decoded from it: `Ok` with what decoding gave,
/// or the value back when it is of another kind. The fuzz target
/// `session_frame` decodes them with this too (`fuzzing.rs`).
pub(crate) fn decode_text(value: WireValue) -> Result<Reply, WireValue> {
    Ok(match value {
        WireValue::Instant(text) => text.parse().map(Value::Instant),
        WireValue::DateTime(text) => text.parse().map(Value::DateTime),
        WireValue::Date(text) => text.parse().map(Value::Date),
        WireValue::Time(text) => text.parse().map(Value::Time),
        WireValue::Bytes(text) => base64_decode(&text).map(Value::Bytes),
        WireValue::BigInt(text) => text.parse().map(Value::BigInt),
        WireValue::Decimal(text) => text.parse().map(Value::Decimal),
        WireValue::Uuid(text) => text.parse().map(Value::Uuid),
        other => return Err(other),
    })
}

pub(crate) fn deeper_than(text: &[u8], limit: usize) -> bool {
    let (mut depth, mut string, mut escaped) = (0usize, false, false);
    for &byte in text {
        if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
            continue;
        }
        match byte {
            b'"' => string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// Parse a frame, nested at most [`MAX_FRAME_DEPTH`] deep (checked before),
/// past serde_json's own limit of 128.
pub(crate) fn parse_frame(text: &[u8]) -> Result<Frame, Error> {
    use serde::Deserialize;
    let mut deserializer = serde_json::Deserializer::from_slice(text);
    deserializer.disable_recursion_limit();
    let frame = Frame::deserialize(&mut deserializer).map_err(transport)?;
    deserializer.end().map_err(transport)?;
    Ok(frame)
}

fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}
/// The error a channel failure ends a session with: `"<label>: <reason>"`,
/// or the bare reason for an unlabelled channel.
fn ended(label: &str, error: ChannelError) -> Error {
    let reason = match label {
        "" => error.to_string(),
        label => format!("{label}: {error}"),
    };
    match error {
        ChannelError::Closed { .. } => Error::Transport(reason),
        ChannelError::Rejected { .. } => Error::Handshake(Handshake::IdentityMismatch(reason)),
    }
}

mod relay;
mod waits;
use relay::Relayed;

// The tests script the far end over a memory channel.
#[cfg(test)]
mod tests;
