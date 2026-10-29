use crate::{DependencyStatus, FiberState, InstanceId, PluginId, TypeKey};
use std::panic::Location;
use std::sync::Arc;

/// Why a strict service read was rejected. Optional `get` reads do not use this check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServiceReadFailure {
    Undeclared,
    Unavailable(DependencyStatus),
    OutOfScope,
    Inactive,
    TypeMismatch,
}

impl std::fmt::Display for ServiceReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undeclared => f.write_str("dependency not declared"),
            Self::Unavailable(status) => write!(f, "declared dependency unavailable ({status})"),
            Self::OutOfScope => f.write_str("instance outside caller's fiber ancestry"),
            Self::Inactive => f.write_str("caller context inactive"),
            Self::TypeMismatch => f.write_str("service key has a different value type"),
        }
    }
}

/// A strict read error with the key, caller identity, and source location.
#[derive(Debug, thiserror::Error)]
#[error("strict service read {key:?} by fiber {plugin_id:?} instance {instance} at {location}: {reason}")]
#[non_exhaustive]
pub struct ServiceReadError {
    pub key: TypeKey,
    pub plugin_id: PluginId,
    pub instance: InstanceId,
    pub location: &'static Location<'static>,
    pub reason: ServiceReadFailure,
}

/// Why a provider-owned mutable service update was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServiceWriteFailure {
    WrongOwner,
    Stale,
}

impl std::fmt::Display for ServiceWriteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongOwner => f.write_str("writer called outside its provider fiber"),
            Self::Stale => {
                f.write_str("service binding or provider generation is no longer active")
            }
        }
    }
}

/// A mutable service update error with the original binding identity.
#[derive(Debug, thiserror::Error)]
#[error(
    "service write {key:?} by fiber {provider:?} generation {generation} at {location}: {reason}"
)]
#[non_exhaustive]
pub struct ServiceWriteError {
    pub key: TypeKey,
    pub provider: PluginId,
    pub generation: u64,
    pub location: &'static Location<'static>,
    pub reason: ServiceWriteFailure,
}

/// What went wrong in rutis: the variants a host matches on, through
/// [`Error::kind`] or `match &*error` (an [`Error`] dereferences to its
/// kind). An error from plugin code is `PluginFailed`.
///
/// It is an `Error + Send + Sync + 'static`; errors are shared as
/// `Arc<ErrorKind>` (the error sink, a fiber's result):
///
/// ```
/// // guarantee: core/D11
/// fn shared<T: std::error::Error + Send + Sync + 'static>() {}
/// shared::<rutis::host::ErrorKind>();
/// let first = std::sync::Arc::new(rutis::host::ErrorKind::Closed);
/// let second = std::sync::Arc::clone(&first);
/// assert!(std::sync::Arc::ptr_eq(&first, &second));
/// ```
///
/// Neither it nor [`Error`] implements `Clone`; copies would lose the
/// identity that joiners compare (D25):
///
/// ```compile_fail,E0277
/// // guarantee: core/D11
/// fn cloneable<T: Clone>() {}
/// cloneable::<rutis::host::ErrorKind>();
/// ```
///
/// ```compile_fail,E0277
/// // guarantee: core/D11
/// fn cloneable<T: Clone>() {}
/// cloneable::<rutis::Error>();
/// ```
///
/// `PluginFailed` wraps only errors that are not rutis errors; a rutis
/// error returned from `apply` or a cleanup keeps its own kind and is not
/// wrapped again.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ErrorKind {
    #[error(transparent)]
    ServiceRead(#[from] ServiceReadError),
    #[error("service {0:?} not found in scope")]
    ServiceNotFound(String),
    /// Plugin code's own error, shown as it is: what the plugin said, with
    /// no prefix of rutis's (#677).
    #[error("{0}")]
    PluginFailed(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("multiple errors: {errors:?}")]
    Aggregate {
        /// The member errors, not flattened. Members are `Arc`s because
        /// errors are not `Clone` and a member may be shared with other
        /// holders of the same error.
        errors: Vec<Arc<ErrorKind>>,
    },
    #[error("fiber disposed")]
    InactiveEffect,
    #[error("stale plugin context: load generation {expected}, current generation {current}")]
    StaleGeneration { expected: u64, current: u64 },
    #[error("plugin context generation {generation} is inactive in state {state:?}")]
    InactiveGeneration { generation: u64, state: FiberState },
    #[error("root has been shut down")]
    Closed,
    #[error("config validation failed: {issues:?}")]
    Validation { issues: Vec<String> },
    /// A dependency is gone. Returned from `apply` after a strict read
    /// (`require` / `require_as`) of a declared dependency found it
    /// unavailable in the same load, it rolls the load back to Pending
    /// instead of Failed; otherwise it is an ordinary load failure. That
    /// read's own `ServiceRead` error, returned as is (`require(..)?`),
    /// does the same.
    #[error("dependency unsatisfied: {0:?}")]
    InjectUnsatisfied(Vec<String>),
    /// A service was registered twice for the same key in the same scope.
    #[error("service {0:?} already registered in scope")]
    ServiceExists(String),
    #[error("instance {instance} is outside the caller's fiber subtree")]
    InstanceOutOfScope { instance: InstanceId },
    /// `parallel` had failures: one per failed listener, in registration
    /// order (`AggregateError`, also for a single failure).
    #[error("{}", listeners_failed(errors.len(), name))]
    ListenersFailed {
        name: String,
        errors: Vec<Arc<ErrorKind>>,
    },
    /// `bail`, or a synchronous `waterfall`, met an asynchronous listener.
    #[error(
        "listener of {name} in {listener} is asynchronous and cannot run in a synchronous {mode}"
    )]
    AsyncListener {
        name: String,
        listener: String,
        mode: crate::Mode,
    },
    /// A waterfall's `next` was misused.
    #[error("{}", misuse.message(name))]
    WaterfallNext { name: String, misuse: NextMisuse },
    /// A plugin emitted a `rutis/*` event.
    #[error("rutis/* events are emitted by rutis only: {name}")]
    ReservedEvent { name: String },
    /// An `emit` listener failed; reported to the error sink, never to the
    /// emitter (protocol spec §5.6).
    #[error("listener of {name} in {listener} failed: {error}")]
    ListenerFailed {
        name: String,
        listener: String,
        error: Arc<ErrorKind>,
    },
    /// An `emit` filter failed; reported to the error sink.
    #[error("filter of {name} from {emitter} failed: {error}")]
    FilterFailed {
        name: String,
        emitter: String,
        error: Arc<ErrorKind>,
    },
    /// The entry is no longer in the listener table.
    #[error("listener {id} of {name} is withdrawn")]
    ListenerWithdrawn { id: u64, name: String },
    /// An event or result did not convert to or from a Rust type.
    #[error("event value of type {type_name}: {message}")]
    EventValue {
        type_name: &'static str,
        message: String,
    },
    #[error(transparent)]
    SyncEventPanicked(Arc<ErrorKind>),
    /// An event type with no name of its own (an empty `NAME`,
    /// `rutis_bridge::events::Args`) was listened to or sent without one:
    /// `ctx.listen::<E>().name(..)`, or a value whose `Event::name` gives it,
    /// was needed (protocol spec §12.1). The message is the protocol's and
    /// stays as it is in every language.
    #[error("{event} events need a name: EventKey::dynamic(name)")]
    EventNeedsName { event: String },
    /// An unload waited `seconds` (`RUTIS_SHUTDOWN_TIMEOUT`) for the event
    /// calls of `listeners` (`row <id>` or `plugin <name>`) still running,
    /// and went on without them (protocol spec §7.1); reported to the error
    /// sink.
    #[error("listeners of {listeners} still running after {seconds}s; unloading anyway")]
    ListenersStillRunning { listeners: String, seconds: f64 },
    /// The plugin `plugin` has waited `seconds` for the services `missing`
    /// (its `Deps`, its `injects`), which nothing provides: reported once to
    /// the error sink each time it starts waiting, after
    /// [`CtxHost::report_waiting_after`](crate::host::CtxHost::report_waiting_after)
    /// (10 s by default). It goes on waiting: the report is all that
    /// happens. Plugins of rutis itself (a link waiting for its peer, say)
    /// are not reported: their state says what they wait for.
    #[error(
        "{plugin}: still waiting after {seconds}s for {}: it starts once that is provided",
        missing.join(", ")
    )]
    StillWaiting {
        plugin: String,
        seconds: f64,
        missing: Vec<String>,
    },
    /// The plugin, or loader row, `plugin` failed to start: reported once
    /// to the error sink each time it fails, besides whoever awaits it, so
    /// an embedding that reads no result still sees it (on stderr, by
    /// default). Plugins of rutis itself report their own failures.
    #[error("{plugin}: failed: {error}")]
    StartFailed {
        plugin: String,
        error: Arc<ErrorKind>,
    },
    /// The language runtime `runtime` went down: its process ended, or did
    /// not start (`reason` says how). Reported to the error sink; its rows
    /// wait for it.
    #[error("runtime {runtime}: down ({reason})")]
    RuntimeDown { runtime: String, reason: String },
    /// A type marked `#[rutis::service]` was provided, but nothing serves
    /// marked services by name in this program (rutis-bridge is not
    /// linked): reported once per service, the first time it is provided.
    #[error(
        "service {service} is provided by type but not registered by name: depend on \
         rutis-bridge to use it by name and to check clashes"
    )]
    NotServedByName { service: String },
}

/// How a waterfall's `next` was misused (protocol spec §11 `WaterfallNext`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NextMisuse {
    /// A second call.
    AlreadyCalled,
    /// After its waterfall ended or was cancelled.
    Ended,
    /// Outside a waterfall.
    NotWaterfall(crate::Mode),
    /// By a synchronous listener in an asynchronous waterfall.
    SyncInAsync,
    /// A remote synchronous `next` called off the dispatching thread.
    OtherThread,
}

impl NextMisuse {
    fn message(&self, name: &str) -> String {
        match self {
            NextMisuse::AlreadyCalled => format!("next of {name} was already called"),
            NextMisuse::Ended => format!("next of {name} was called after its waterfall ended"),
            NextMisuse::NotWaterfall(mode) => {
                format!("next is available only in a waterfall (this is {mode})")
            }
            NextMisuse::SyncInAsync => "a synchronous listener cannot call next in an \
                 asynchronous waterfall; register it with on"
                .to_owned(),
            NextMisuse::OtherThread => {
                format!("next of {name} was called off the thread of its waterfall")
            }
        }
    }
}

impl ErrorKind {
    /// The cross-language error name of an event error (protocol spec §11):
    /// `AggregateError`, `AsyncListener`, `WaterfallNext`, `ReservedEvent`;
    /// `None` for every other error.
    pub fn wire_name(&self) -> Option<&'static str> {
        match self {
            ErrorKind::ListenersFailed { .. } => Some("AggregateError"),
            ErrorKind::AsyncListener { .. } => Some("AsyncListener"),
            ErrorKind::WaterfallNext { .. } => Some("WaterfallNext"),
            ErrorKind::ReservedEvent { .. } => Some("ReservedEvent"),
            ErrorKind::EventNeedsName { .. } => Some("BindingError"),
            _ => None,
        }
    }
}

/// The error of every rutis signature.
///
/// `?` turns any `std::error::Error + Send + Sync + 'static` into it; a
/// rutis error keeps its [`ErrorKind`], any other is the plugin's own
/// error: [`Error::kind`] is `None` and the host sees it inside
/// [`ErrorKind::PluginFailed`]. It does not implement `std::error::Error`
/// itself (that is what lets `?` accept any error, as with
/// `anyhow::Error`); it dereferences to the [`ErrorKind`] the host sees,
/// which does, and converts into `Box<dyn std::error::Error + Send + Sync>`.
/// Shared errors (the error sink, a plugin's result) are `Arc<ErrorKind>`.
///
/// ```
/// fn read() -> rutis::Result<String> {
///     Ok(std::fs::read_to_string("/nonexistent/rutis")?) // io::Error through `?`
/// }
/// fn host() -> Result<(), Box<dyn std::error::Error>> {
///     let _ = read().is_err();
///     Err(rutis::Error::msg("boom"))?
/// }
/// assert!(host().is_err());
/// ```
pub struct Error {
    kind: Box<ErrorKind>,
    /// Made from a non-rutis error or a message: plugin code's own error.
    foreign: bool,
}

/// `Result` with [`Error`]: `rutis::Result` is `Result<(), Error>`.
pub type Result<T = (), E = Error> = std::result::Result<T, E>;

impl Error {
    fn new(kind: ErrorKind, foreign: bool) -> Self {
        Error {
            kind: Box::new(kind),
            foreign,
        }
    }

    /// Which rutis failure this is, to match on; `None` for an error that
    /// came from plugin code (`?` on another error type, [`Error::msg`]).
    pub fn kind(&self) -> Option<&ErrorKind> {
        (!self.foreign).then_some(&*self.kind)
    }

    /// The kind the host sees, by value: a plugin code error is a
    /// [`ErrorKind::PluginFailed`].
    pub fn into_kind(self) -> ErrorKind {
        *self.kind
    }

    /// This error as the shared form errors take when several parties
    /// hold them (the error sink, a fiber's result).
    pub(crate) fn shared(self) -> Arc<ErrorKind> {
        Arc::new(self.into_kind())
    }

    /// An error with this message (a `PluginFailed`).
    pub fn msg(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::PluginFailed(message.into().into()), true)
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(error: E) -> Self {
        // A rutis error keeps its kind (D25: no second wrapping); a strict
        // read error keeps its `ServiceRead` variant.
        let mut slot = Some(error);
        let any: &mut dyn std::any::Any = &mut slot;
        if let Some(kind) = any.downcast_mut::<Option<ErrorKind>>() {
            return Error::new(kind.take().expect("taken once"), false);
        }
        if let Some(read) = any.downcast_mut::<Option<ServiceReadError>>() {
            return Error::new(
                ErrorKind::ServiceRead(read.take().expect("taken once")),
                false,
            );
        }
        let error = slot.take().expect("taken once");
        Error::new(ErrorKind::PluginFailed(Box::new(error)), true)
    }
}

impl std::ops::Deref for Error {
    type Target = ErrorKind;
    fn deref(&self) -> &ErrorKind {
        &self.kind
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.kind, f)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&*self.kind, f)
    }
}

impl AsRef<dyn std::error::Error + Send + Sync + 'static> for Error {
    fn as_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        &*self.kind
    }
}

impl From<Error> for Box<dyn std::error::Error + Send + Sync + 'static> {
    fn from(error: Error) -> Self {
        error.kind
    }
}

impl From<Error> for Box<dyn std::error::Error + 'static> {
    fn from(error: Error) -> Self {
        error.kind
    }
}

/// Where errors nobody awaits are reported. The default writes to stderr
/// ([`default_sink`]).
///
/// It takes `Arc<ErrorKind>`: a dispose error is cached and shared by its
/// transition, and `Error` is deliberately not `Clone`.
pub type ErrorSink = Arc<dyn Fn(Arc<ErrorKind>) + Send + Sync>;

/// The sink `Ctx::root` / `Ctx::root_with` install: each error on stderr
/// as its text says, with no prefix of rutis's. A plugin's or row's failed
/// start, a long wait and a runtime that went down begin with the name of
/// the plugin, row or runtime (`weather: failed: ...`). A host installing
/// its own sink ([`Ctx::root_with_sink`](crate::host::CtxHost::root_with_sink))
/// replaces it, and can delegate to it or drop what it reports itself.
pub fn default_sink() -> ErrorSink {
    Arc::new(|e: Arc<ErrorKind>| eprintln!("{}", format_for_sink(&e)))
}

fn format_for_sink(error: &ErrorKind) -> String {
    fn write_source(source: &(dyn std::error::Error + 'static), out: &mut String, indent: usize) {
        if let Some(error) = source.downcast_ref::<ErrorKind>() {
            write_error(error, out, indent);
            return;
        }
        out.push_str(&source.to_string());
        if let Some(next) = source.source() {
            out.push('\n');
            out.push_str(&" ".repeat(indent + 2));
            out.push_str("caused by: ");
            write_source(next, out, indent + 2);
        }
    }

    fn write_error(error: &ErrorKind, out: &mut String, indent: usize) {
        match error {
            ErrorKind::Aggregate { errors } => {
                out.push_str("multiple errors:");
                for (index, member) in errors.iter().enumerate() {
                    out.push('\n');
                    out.push_str(&" ".repeat(indent));
                    out.push_str(&format!("{}. ", index + 1));
                    write_error(member, out, indent + 3);
                }
            }
            ErrorKind::PluginFailed(source) => {
                let display = error.to_string();
                let direct = source.to_string();
                match display.strip_suffix(&direct) {
                    Some(prefix) => {
                        // Keep the public Display as the source of the prefix,
                        // while formatting a nested rutis error structurally.
                        out.push_str(prefix);
                        write_source(source.as_ref(), out, indent);
                    }
                    None => {
                        out.push_str(&display);
                        // A future Display that omits the direct source still
                        // gets its deeper chain without printing it twice.
                        if let Some(next) = source.source() {
                            out.push('\n');
                            out.push_str(&" ".repeat(indent + 2));
                            out.push_str("caused by: ");
                            write_source(next, out, indent + 2);
                        }
                    }
                }
            }
            ErrorKind::StartFailed { plugin, error } => {
                out.push_str(&format!("{plugin}: failed: "));
                write_error(error, out, indent);
            }
            _ => out.push_str(&error.to_string()),
        }
    }

    let mut out = String::new();
    write_error(error, &mut out, 0);
    out
}

/// One error as it is, several as an unflattened `Aggregate` of the shared
/// `Arc`s; `None` when there are no errors.
pub(crate) fn aggregate_arcs(errors: Vec<Arc<ErrorKind>>) -> Option<Arc<ErrorKind>> {
    match errors.len() {
        0 => None,
        1 => errors.into_iter().next(),
        _ => Some(Arc::new(ErrorKind::Aggregate { errors })),
    }
}

/// Turns a panic caught at a task boundary into a `PluginFailed` carrying
/// the panic message.
pub(crate) fn panic_error(p: Box<dyn std::any::Any + Send>) -> Error {
    let msg = if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic in task".to_string()
    };
    ErrorKind::PluginFailed(msg.into()).into()
}

/// Turns a `JoinError` into an error. A panic becomes [`panic_error`]; a
/// cancellation becomes a "task cancelled" error, since `into_panic()` would
/// panic again on a cancelled task.
pub(crate) fn join_panic_error(join_err: tokio::task::JoinError) -> Error {
    if join_err.is_panic() {
        panic_error(join_err.into_panic())
    } else {
        ErrorKind::PluginFailed("task cancelled before completion".into()).into()
    }
}

/// `1 listener of <event> failed` / `<n> listeners of <event> failed`.
fn listeners_failed(count: usize, event: &str) -> String {
    match count {
        1 => format!("1 listener of {event} failed"),
        n => format!("{n} listeners of {event} failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("outer cause")]
    struct OuterCause {
        #[source]
        source: InnerCause,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("inner cause")]
    struct InnerCause;

    /// guarantee: core/D25
    #[test]
    fn plugin_failure_display_and_sink_preserve_each_cause_once() {
        let error = ErrorKind::PluginFailed(Box::new(OuterCause { source: InnerCause }));
        assert_eq!(error.to_string(), "outer cause");
        assert_eq!(
            format_for_sink(&error),
            "outer cause\n  caused by: inner cause"
        );
        assert_eq!(
            std::error::Error::source(&error).unwrap().to_string(),
            "outer cause"
        );
    }

    /// One failed listener is `1 listener`, more are `n listeners` (#677).
    #[test]
    fn listeners_failed_counts_in_words() {
        let failed = |count: usize| ErrorKind::ListenersFailed {
            name: "weather/asked".into(),
            errors: (0..count).map(|_| Arc::new(ErrorKind::Closed)).collect(),
        };
        assert_eq!(failed(1).to_string(), "1 listener of weather/asked failed");
        assert_eq!(failed(2).to_string(), "2 listeners of weather/asked failed");
    }

    /// The default sink's lines start with the plugin, row or runtime they
    /// are about, with no platform name before it (#678).
    /// guarantee: core/features.default-output
    #[test]
    fn default_sink_lines_start_with_the_name_they_are_about() {
        let failed = ErrorKind::StartFailed {
            plugin: "weather".into(),
            error: Arc::new(ErrorKind::Aggregate {
                errors: vec![
                    Arc::new(ErrorKind::PluginFailed("no API key".into())),
                    Arc::new(ErrorKind::PluginFailed("cleanup detail".into())),
                ],
            }),
        };
        assert_eq!(
            format_for_sink(&failed),
            "weather: failed: multiple errors:\n1. no API key\n2. cleanup detail"
        );
        let down = ErrorKind::RuntimeDown {
            runtime: "py".into(),
            reason: "runtime process exited with exit status: 1".into(),
        };
        assert_eq!(
            format_for_sink(&down),
            "runtime py: down (runtime process exited with exit status: 1)"
        );
        let waiting = ErrorKind::StillWaiting {
            plugin: "chat".into(),
            seconds: 10.0,
            missing: vec!["calendar".into()],
        };
        assert_eq!(
            format_for_sink(&waiting),
            "chat: still waiting after 10s for calendar: it starts once that is provided"
        );
    }

    #[test]
    fn sink_formats_aggregate_members_without_debug_noise() {
        let error = ErrorKind::Aggregate {
            errors: vec![
                Arc::new(ErrorKind::PluginFailed("cleanup detail".into())),
                Arc::new(ErrorKind::ServiceNotFound("missing".into())),
            ],
        };
        assert_eq!(
            format_for_sink(&error),
            "multiple errors:\n1. cleanup detail\n2. service \"missing\" not found in scope"
        );
    }

    #[test]
    fn sink_recurses_into_aggregate_used_as_plugin_failure_source() {
        let nested = ErrorKind::Aggregate {
            errors: vec![Arc::new(ErrorKind::ServiceNotFound("nested".into()))],
        };
        let error = ErrorKind::PluginFailed(Box::new(nested));
        assert_eq!(
            format_for_sink(&error),
            "multiple errors:\n1. service \"nested\" not found in scope"
        );
    }

    #[derive(Debug, thiserror::Error)]
    #[error("quota exceeded")]
    struct Quota;

    #[test]
    fn question_mark_keeps_rutis_kinds_and_wraps_foreign_errors() {
        let kept: Error = ErrorKind::Closed.into();
        assert!(matches!(kept.kind(), Some(ErrorKind::Closed)));
        let wrapped: Error = Quota.into();
        assert!(wrapped.kind().is_none(), "a plugin's own error has no kind");
        match &*wrapped {
            ErrorKind::PluginFailed(source) => assert_eq!(source.to_string(), "quota exceeded"),
            other => panic!("expected PluginFailed, got {other:?}"),
        }
        let io: Error = std::io::Error::other("disk").into();
        assert!(io.kind().is_none());
        assert!(matches!(&*io, ErrorKind::PluginFailed(_)));
        assert!(Error::msg("boom").kind().is_none());
        let boxed: Box<dyn std::error::Error + Send + Sync> = kept.into();
        assert_eq!(boxed.to_string(), "root has been shut down");
    }
}
