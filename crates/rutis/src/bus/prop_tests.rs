//! guarantee: events/emit
//! guarantee: events/parallel
//! guarantee: events/serial-bail
//! guarantee: events/waterfall
//! guarantee: events/order
//! Property test of the five dispatch modes (#631): a random listener list
//! (synchronous, asynchronous, remote; results including "no result";
//! errors and panics; `once`, `prepend`, `global`; a custom filter) is
//! dispatched in a random mode, and the result, the errors reported and the
//! calls each listener got are compared with a reference interpreter.
//!
//! The interpreter is the protocol's algorithm written once, on the
//! listener specs alone (spec §5): the table order (prepends newest first,
//! then appends oldest first), the filter over the non-global entries in
//! that order, then the mode:
//!
//! - `emit` starts every call in order and reports failures; its result is
//!   `Ok(None)`;
//! - `parallel` calls every entry and collects failures in registration
//!   order;
//! - `serial` and `bail` stop at the first result that is not "no result",
//!   or at the first failure; `bail` refuses an asynchronous entry before
//!   calling it;
//! - `waterfall` hands each listener the rest of the chain; a listener that
//!   takes it may pass its value on or return its own, and the rest runs
//!   only then. A synchronous listener in an asynchronous waterfall gets no
//!   `next`; a synchronous waterfall can call only synchronous local
//!   listeners (a remote one cannot receive a synchronous `next`).
//!
//! `once` entries leave the table when a call of them starts, even if that
//! call then fails. A `PROPTEST_CASES` count (256 by default); a failure
//! prints the seed and the shrunk case, rerun with `RUTIS_PROPTEST_SEED`.
//!
//! The property covers what a dispatch does; the table itself is covered by
//! `tests/listener_table_model.rs`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};

use super::run::{self, AsyncNext, ErasedSyncTerminal, ErasedTerminal, Inv, SyncNext};
use super::{Body, EventBus, Opts};
use crate::ctx::Ctx;
use crate::error::{panic_error, Error, ErrorKind};
use crate::events::{
    CatchUnwind, ErasedAsync, ErasedSync, Event, EventOptions, ListenerInfo, ListenerKind, Payload,
    RemoteCall, RemoteEntry, RemoteListener, RemoteSyncCall, Value,
};
use crate::host::CtxHost;
use crate::internal::CtxInternal;
use crate::Disposer;

const NAME: &str = "prop::dispatch";

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Ev;

impl Event for Ev {
    const NAME: &'static str = NAME;
    type Value = bool;
}

/// What a listener returns: no result (`None` or `Some(false)`), a result
/// (`Some(true)`), or a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    None,
    NoResult,
    Result,
    Error,
    Panic,
}

/// What a listener does with the rest of a waterfall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Return its own outcome without calling `next`.
    Own,
    /// Call `next` and return its outcome.
    Next,
    /// Call `next`, then return its own outcome, ignoring the rest's.
    NextThenOwn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyKind {
    Sync,
    Async,
    RemoteSync,
    RemoteAsync,
}

impl BodyKind {
    fn kind(self) -> ListenerKind {
        match self {
            Self::Sync | Self::RemoteSync => ListenerKind::Sync,
            Self::Async | Self::RemoteAsync => ListenerKind::Async,
        }
    }

    fn remote(self) -> bool {
        matches!(self, Self::RemoteSync | Self::RemoteAsync)
    }

    /// An asynchronous entry: `bail` refuses it without calling it.
    fn asynchronous(self) -> bool {
        matches!(self, Self::Async | Self::RemoteAsync)
    }
}

#[derive(Debug, Clone)]
struct ListenerSpec {
    body: BodyKind,
    outcome: Outcome,
    action: Action,
    once: bool,
    prepend: bool,
    global: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchMode {
    Emit,
    Parallel,
    Serial,
    Waterfall,
    WaterfallSync,
    Bail,
}

#[derive(Debug, Clone)]
struct DispatchSpec {
    mode: DispatchMode,
    terminal: Outcome,
}

#[derive(Debug, Clone)]
struct Case {
    listeners: Vec<ListenerSpec>,
    dispatches: Vec<DispatchSpec>,
    /// The answers of the custom filter, when there is one, in call order.
    filter: Option<Vec<bool>>,
}

fn outcome() -> impl Strategy<Value = Outcome> {
    prop_oneof![
        Just(Outcome::None),
        Just(Outcome::NoResult),
        Just(Outcome::Result),
        Just(Outcome::Error),
        Just(Outcome::Panic),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        Just(Action::Own),
        Just(Action::Next),
        Just(Action::NextThenOwn)
    ]
}

fn body_kind() -> impl Strategy<Value = BodyKind> {
    prop_oneof![
        Just(BodyKind::Sync),
        Just(BodyKind::Async),
        Just(BodyKind::RemoteSync),
        Just(BodyKind::RemoteAsync),
    ]
}

fn listener_spec() -> impl Strategy<Value = ListenerSpec> {
    (
        body_kind(),
        outcome(),
        action(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(body, outcome, action, once, prepend, global)| ListenerSpec {
                body,
                outcome,
                action,
                once,
                prepend,
                global,
            },
        )
}

fn dispatch_spec() -> impl Strategy<Value = DispatchSpec> {
    let mode = prop_oneof![
        Just(DispatchMode::Emit),
        Just(DispatchMode::Parallel),
        Just(DispatchMode::Serial),
        Just(DispatchMode::Waterfall),
        Just(DispatchMode::WaterfallSync),
        Just(DispatchMode::Bail),
    ];
    (mode, outcome()).prop_map(|(mode, terminal)| DispatchSpec { mode, terminal })
}

fn case() -> impl Strategy<Value = Case> {
    (
        prop::collection::vec(listener_spec(), 0..7),
        prop::collection::vec(dispatch_spec(), 1..5),
        any::<bool>(),
        prop::collection::vec(any::<bool>(), 0..64),
    )
        .prop_map(|(listeners, dispatches, custom_filter, answers)| Case {
            listeners,
            dispatches,
            filter: custom_filter.then_some(answers),
        })
}

// ── the reference interpreter ───────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpectedErr {
    /// One listener (or the terminal) failed, by its tag.
    Listener(String),
    /// A synchronous mode met an asynchronous listener.
    Async,
    /// `parallel`: every failed listener, in registration order.
    Multiple(Vec<String>),
}

#[derive(Debug)]
struct Model {
    listeners: Vec<ListenerSpec>,
    /// The table id of each listener, filled in when it is registered.
    ids: Vec<u64>,
    live: Vec<bool>,
    /// Calls started (`Entry::calls`), even the ones that fail before the
    /// listener runs (`bail` meeting an asynchronous entry).
    starts: Vec<u64>,
    /// Listener invocations.
    calls: Vec<u64>,
    terminal_calls: u64,
    filter: Option<Vec<bool>>,
    filter_cursor: usize,
    sink: Vec<String>,
}

impl Model {
    fn new(case: &Case) -> Self {
        let count = case.listeners.len();
        Self {
            listeners: case.listeners.clone(),
            ids: vec![0; count],
            live: vec![true; count],
            starts: vec![0; count],
            calls: vec![0; count],
            terminal_calls: 0,
            filter: case.filter.clone(),
            filter_cursor: 0,
            sink: Vec::new(),
        }
    }

    fn tag(&self, index: usize) -> String {
        match self.listeners[index].outcome {
            Outcome::Error => format!("error L{index}"),
            Outcome::Panic => format!("panic L{index}"),
            _ => format!("L{index}"),
        }
    }

    /// The live entries in call order: prepends newest first, then appends
    /// oldest first.
    fn live_order(&self) -> Vec<usize> {
        let live = |index: &usize| self.live[*index];
        (0..self.listeners.len())
            .rev()
            .filter(|index| self.listeners[*index].prepend && live(index))
            .chain(
                (0..self.listeners.len())
                    .filter(|index| !self.listeners[*index].prepend && live(index)),
            )
            .collect()
    }

    /// The filter's next answer; a script shorter than the calls keeps.
    fn filter_answer(&mut self) -> bool {
        let Some(answers) = &self.filter else {
            return true;
        };
        let answer = answers.get(self.filter_cursor).copied().unwrap_or(true);
        self.filter_cursor += 1;
        answer
    }

    /// The entries a dispatch calls, in order: the live ones the filter
    /// keeps (`global` entries are never filtered).
    fn select(&mut self) -> Vec<usize> {
        let mut selected = Vec::new();
        for index in self.live_order() {
            let keep = self.listeners[index].global || self.filter_answer();
            if keep {
                selected.push(index);
            }
        }
        selected
    }

    /// One call started; a `once` entry leaves the table even when the call
    /// then fails before the listener runs.
    fn start(&mut self, index: usize, called: bool) {
        self.starts[index] += 1;
        if called {
            self.calls[index] += 1;
        }
        if self.listeners[index].once {
            self.live[index] = false;
        }
    }

    fn own_result(&self, index: usize) -> Result<Option<bool>, ExpectedErr> {
        outcome_result(self.listeners[index].outcome, &self.tag(index))
    }

    fn dispatch(&mut self, spec: &DispatchSpec) -> Result<Option<bool>, ExpectedErr> {
        match spec.mode {
            DispatchMode::Emit => {
                for index in self.select() {
                    self.start(index, true);
                    if matches!(
                        self.listeners[index].outcome,
                        Outcome::Error | Outcome::Panic
                    ) {
                        self.sink.push(self.tag(index));
                    }
                }
                Ok(None)
            }
            DispatchMode::Parallel => {
                let mut errors = Vec::new();
                for index in self.select() {
                    self.start(index, true);
                    if matches!(
                        self.listeners[index].outcome,
                        Outcome::Error | Outcome::Panic
                    ) {
                        errors.push(self.tag(index));
                    }
                }
                match errors.is_empty() {
                    true => Ok(None),
                    false => Err(ExpectedErr::Multiple(errors)),
                }
            }
            DispatchMode::Serial => {
                for index in self.select() {
                    self.start(index, true);
                    match self.listeners[index].outcome {
                        Outcome::Error | Outcome::Panic => {
                            return Err(ExpectedErr::Listener(self.tag(index)))
                        }
                        Outcome::Result => return Ok(Some(true)),
                        Outcome::None | Outcome::NoResult => {}
                    }
                }
                Ok(None)
            }
            DispatchMode::Bail => {
                for index in self.select() {
                    let body = self.listeners[index].body;
                    self.start(index, !body.asynchronous());
                    if body.asynchronous() {
                        return Err(ExpectedErr::Async);
                    }
                    match self.listeners[index].outcome {
                        Outcome::Error => {
                            return Err(ExpectedErr::Listener(self.tag(index)));
                        }
                        Outcome::Panic => {
                            // `user_call` reports the panic, then fails.
                            self.sink.push(self.tag(index));
                            return Err(ExpectedErr::Listener(self.tag(index)));
                        }
                        Outcome::Result => return Ok(Some(true)),
                        Outcome::None | Outcome::NoResult => {}
                    }
                }
                Ok(None)
            }
            DispatchMode::Waterfall => {
                let chain = self.select();
                self.waterfall_async(&chain, 0, spec.terminal)
            }
            DispatchMode::WaterfallSync => {
                let chain = self.select();
                self.waterfall_sync(&chain, 0, spec.terminal)
            }
        }
    }

    fn waterfall_async(
        &mut self,
        chain: &[usize],
        at: usize,
        terminal: Outcome,
    ) -> Result<Option<bool>, ExpectedErr> {
        let Some(&index) = chain.get(at) else {
            self.terminal_calls += 1;
            return outcome_result(terminal, terminal_tag(terminal));
        };
        // A synchronous listener in an asynchronous waterfall is called with
        // no `next` (`call_async`).
        let action = match self.listeners[index].body {
            BodyKind::Sync => Action::Own,
            _ => self.listeners[index].action,
        };
        let own = self.own_result(index);
        self.start(index, true);
        match action {
            Action::Own => own,
            Action::Next => self.waterfall_async(chain, at + 1, terminal),
            Action::NextThenOwn => {
                let _ = self.waterfall_async(chain, at + 1, terminal);
                own
            }
        }
    }

    fn waterfall_sync(
        &mut self,
        chain: &[usize],
        at: usize,
        terminal: Outcome,
    ) -> Result<Option<bool>, ExpectedErr> {
        let Some(&index) = chain.get(at) else {
            self.terminal_calls += 1;
            // `SyncNext` calls the terminal through `user_call`.
            if terminal == Outcome::Panic {
                self.sink.push("panic terminal".to_owned());
            }
            return outcome_result(terminal, terminal_tag(terminal));
        };
        let body = self.listeners[index].body;
        if body != BodyKind::Sync {
            // An asynchronous listener, and a remote one (which has no
            // synchronous `next`), fail before being called.
            self.start(index, false);
            return Err(ExpectedErr::Async);
        }
        let own = self.own_result(index);
        let tag = self.tag(index);
        self.start(index, true);
        let own_panicked = self.listeners[index].outcome == Outcome::Panic;
        match self.listeners[index].action {
            Action::Own => {
                if own_panicked {
                    self.sink.push(tag);
                }
                own
            }
            Action::Next => self.waterfall_sync(chain, at + 1, terminal),
            Action::NextThenOwn => {
                let _ = self.waterfall_sync(chain, at + 1, terminal);
                if own_panicked {
                    self.sink.push(tag);
                }
                own
            }
        }
    }
}

fn outcome_result(outcome: Outcome, tag: &str) -> Result<Option<bool>, ExpectedErr> {
    match outcome {
        Outcome::None => Ok(None),
        Outcome::NoResult => Ok(Some(false)),
        Outcome::Result => Ok(Some(true)),
        Outcome::Error | Outcome::Panic => Err(ExpectedErr::Listener(tag.to_owned())),
    }
}

fn terminal_tag(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Error => "error terminal",
        Outcome::Panic => "panic terminal",
        _ => "terminal",
    }
}

/// The failure an actual error names: the innermost listener's message, the
/// asynchronous-listener refusal, or `parallel`'s ordered list.
fn expected_err(error: &Error) -> ExpectedErr {
    match error.kind() {
        Some(ErrorKind::ListenersFailed { errors, .. }) => {
            ExpectedErr::Multiple(errors.iter().map(|error| sink_tag(error)).collect())
        }
        Some(ErrorKind::AsyncListener { .. }) => ExpectedErr::Async,
        Some(kind) => kind_tag(kind),
        // A plugin's own error (`Error::msg`): its text is the tag.
        None => ExpectedErr::Listener(error.to_string()),
    }
}

fn kind_tag(kind: &ErrorKind) -> ExpectedErr {
    match kind {
        ErrorKind::PluginFailed(error) => ExpectedErr::Listener(error.to_string()),
        ErrorKind::ListenerFailed { error, .. } => kind_tag(error),
        ErrorKind::SyncEventPanicked(inner) => kind_tag(inner),
        ErrorKind::AsyncListener { .. } => ExpectedErr::Async,
        other => ExpectedErr::Listener(format!("{other}")),
    }
}

/// How the error sink names an error.
fn sink_tag(kind: &ErrorKind) -> String {
    match kind {
        ErrorKind::PluginFailed(error) => error.to_string(),
        ErrorKind::ListenerFailed { error, .. } => sink_tag(error),
        ErrorKind::SyncEventPanicked(inner) => sink_tag(inner),
        ErrorKind::AsyncListener { .. } => "async listener".to_owned(),
        other => format!("{other}"),
    }
}

// ── the runtime side ────────────────────────────────────────────

fn outcome_value(outcome: Outcome, tag: &str) -> Result<Option<Value>, Error> {
    match outcome {
        Outcome::None => Ok(None),
        Outcome::NoResult => Ok(Some(Value::new(false))),
        Outcome::Result => Ok(Some(Value::new(true))),
        Outcome::Error => Err(Error::msg(format!("error {tag}"))),
        Outcome::Panic => panic!("panic {tag}"),
    }
}

struct Probe {
    tag: String,
    outcome: Outcome,
    action: Action,
    calls: Arc<AtomicU64>,
}

impl ErasedSync for Probe {
    fn call(
        &self,
        _ctx: &Ctx,
        _inv: &Inv<'_>,
        next: Option<SyncNext<'_>>,
    ) -> Result<Option<Value>, Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        match (self.action, next) {
            (Action::Next, Some(next)) => next.invoke(),
            (Action::NextThenOwn, Some(next)) => {
                let _ = next.invoke();
                outcome_value(self.outcome, &self.tag)
            }
            // Every other mode hands a synchronous listener no `next`.
            _ => outcome_value(self.outcome, &self.tag),
        }
    }
}

impl ErasedAsync for Probe {
    fn call<'a>(
        &'a self,
        _ctx: &'a Ctx,
        _inv: &'a Inv<'a>,
        next: Option<AsyncNext>,
    ) -> crate::BoxFuture<'a, Result<Option<Value>, Error>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let outcome = self.outcome;
        let action = self.action;
        let tag = self.tag.clone();
        Box::pin(async move {
            match (action, next) {
                (Action::Next, Some(next)) => next.invoke().await,
                (Action::NextThenOwn, Some(next)) => {
                    let _ = next.invoke().await;
                    outcome_value(outcome, &tag)
                }
                _ => outcome_value(outcome, &tag),
            }
        })
    }
}

struct RemoteProbe {
    kind: ListenerKind,
    tag: String,
    outcome: Outcome,
    action: Action,
    calls: Arc<AtomicU64>,
}

impl RemoteListener for RemoteProbe {
    fn kind(&self) -> ListenerKind {
        self.kind
    }

    fn call<'a>(
        &'a self,
        call: RemoteCall<'a>,
    ) -> crate::BoxFuture<'a, Result<Option<Value>, Error>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let outcome = self.outcome;
        let action = self.action;
        let tag = self.tag.clone();
        let next = call.next();
        Box::pin(async move {
            match (action, next) {
                (Action::Next, Some(next)) => next.call().await,
                (Action::NextThenOwn, Some(next)) => {
                    let _ = next.call().await;
                    outcome_value(outcome, &tag)
                }
                _ => outcome_value(outcome, &tag),
            }
        })
    }

    fn call_sync(&self, _call: RemoteSyncCall<'_>) -> Result<Option<Value>, Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        outcome_value(self.outcome, &self.tag)
    }
}

struct AsyncTerminal {
    outcome: Outcome,
    tag: String,
    calls: Arc<AtomicU64>,
}

impl ErasedTerminal for AsyncTerminal {
    fn call<'a>(&'a self, _inv: &'a Inv<'a>) -> crate::BoxFuture<'a, Result<Option<Value>, Error>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let outcome = self.outcome;
        let tag = self.tag.clone();
        Box::pin(async move {
            // As `FnTerminal`: a panic in the terminal is the dispatch's
            // error, not a panic of the caller.
            match CatchUnwind::new(async move { outcome_value(outcome, &tag) }).await {
                Ok(result) => result,
                Err(panic) => Err(panic_error(panic)),
            }
        })
    }
}

struct SyncTerminal {
    outcome: Outcome,
    tag: String,
    calls: Arc<AtomicU64>,
}

impl ErasedSyncTerminal for SyncTerminal {
    fn call(&self, _inv: &Inv<'_>) -> Result<Option<Value>, Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        outcome_value(self.outcome, &self.tag)
    }
}

struct FilterScript {
    answers: Vec<bool>,
    cursor: Mutex<usize>,
}

impl FilterScript {
    fn keep(&self) -> bool {
        let mut cursor = self.cursor.lock().unwrap();
        let answer = self.answers.get(*cursor).copied().unwrap_or(true);
        *cursor += 1;
        answer
    }

    fn cursor(&self) -> usize {
        *self.cursor.lock().unwrap()
    }
}

struct Harness {
    root: Ctx,
    bus: EventBus,
    /// The errors the root reported while the case ran.
    sink: Arc<Mutex<Vec<String>>>,
    /// The listener invocations, by registration index.
    calls: Vec<Arc<AtomicU64>>,
    /// The listener table's ids, by registration index.
    ids: Vec<u64>,
    filter: Option<Arc<FilterScript>>,
    terminal_calls: Arc<AtomicU64>,
    disposers: Vec<Disposer>,
}

impl Harness {
    fn new(root: Ctx, sink: Arc<Mutex<Vec<String>>>, case: &Case) -> Result<Self, TestCaseError> {
        let bus = root.events().clone();
        let mut harness = Self {
            root,
            bus,
            sink,
            calls: Vec::new(),
            ids: Vec::new(),
            filter: case.filter.clone().map(|answers| {
                Arc::new(FilterScript {
                    answers,
                    cursor: Mutex::new(0),
                })
            }),
            terminal_calls: Arc::new(AtomicU64::new(0)),
            disposers: Vec::new(),
        };
        for (index, listener) in case.listeners.iter().enumerate() {
            harness.register(index, listener)?;
        }
        Ok(harness)
    }

    fn register(&mut self, index: usize, listener: &ListenerSpec) -> Result<(), TestCaseError> {
        let options = EventOptions {
            once: listener.once,
            prepend: listener.prepend,
            global: listener.global,
        };
        let calls = Arc::new(AtomicU64::new(0));
        self.calls.push(calls.clone());
        let tag = format!("L{index}");
        let disposer = match listener.body {
            BodyKind::Sync => {
                let probe = Probe {
                    tag,
                    outcome: listener.outcome,
                    action: listener.action,
                    calls,
                };
                self.bus.register(
                    &self.root,
                    Arc::from(NAME),
                    false,
                    Body::Sync(Arc::new(probe)),
                    options,
                    None,
                    "probe sync listener",
                )
            }
            BodyKind::Async => {
                let probe = Probe {
                    tag,
                    outcome: listener.outcome,
                    action: listener.action,
                    calls,
                };
                self.bus.register(
                    &self.root,
                    Arc::from(NAME),
                    false,
                    Body::Async(Arc::new(probe)),
                    options,
                    None,
                    "probe async listener",
                )
            }
            BodyKind::RemoteSync | BodyKind::RemoteAsync => {
                let probe = RemoteProbe {
                    kind: listener.body.kind(),
                    tag,
                    outcome: listener.outcome,
                    action: listener.action,
                    calls,
                };
                self.bus.on_remote(
                    &self.root,
                    RemoteEntry {
                        name: NAME.to_owned(),
                        prefix: false,
                        info: ListenerInfo {
                            name: NAME.to_owned(),
                            prefix: false,
                            plugin: "probe".to_owned(),
                            row: None,
                            runtime: None,
                            node: None,
                            scope: Vec::new(),
                            global: listener.global,
                        },
                        options,
                        origin: index as u64,
                    },
                    Arc::new(probe),
                )
            }
        }
        .map_err(|error| {
            TestCaseError::fail(format!(
                "registering listener {index} ({listener:?}): {error}"
            ))
        })?;
        self.disposers.push(disposer);
        // This root registers nothing else, so the largest id is the new
        // entry's.
        let id = self
            .bus
            .subscriptions()
            .iter()
            .map(|entry| entry.id)
            .max()
            .expect("the entry just registered");
        self.ids.push(id);
        Ok(())
    }

    fn opts(&self) -> Opts {
        match &self.filter {
            None => Opts::default(),
            Some(script) => {
                let script = script.clone();
                let keep: Arc<run::Keep> = Arc::new(move |_info: &ListenerInfo| Ok(script.keep()));
                Opts {
                    filter: Some(run::FilterSpec::Each(keep)),
                    ..Opts::default()
                }
            }
        }
    }

    async fn dispatch(&self, spec: &DispatchSpec) -> Result<Option<Value>, Error> {
        let payload = Payload::new(Arc::new(Ev));
        let opts = self.opts();
        match spec.mode {
            DispatchMode::Emit => {
                let started = self
                    .bus
                    .emit_erased(&self.root, Arc::from(NAME), payload, opts)?;
                let _ = started.await;
                // The emit starts its calls in a task of its own; its
                // backlog entry goes when every call of it started.
                rutis_test_support::until(|| self.bus.backlogs().is_empty(), "emit calls started")
                    .await;
                Ok(None)
            }
            DispatchMode::Parallel => self
                .bus
                .parallel_erased(&self.root, Arc::from(NAME), payload, opts)
                .await
                .map(|()| None),
            DispatchMode::Serial => {
                self.bus
                    .serial_erased(&self.root, Arc::from(NAME), payload, opts)
                    .await
            }
            DispatchMode::Waterfall => {
                let terminal = Box::new(AsyncTerminal {
                    outcome: spec.terminal,
                    tag: "terminal".to_owned(),
                    calls: self.terminal_calls.clone(),
                });
                self.bus
                    .waterfall_erased(&self.root, Arc::from(NAME), payload, terminal, opts)
                    .await
            }
            DispatchMode::WaterfallSync => {
                let terminal = SyncTerminal {
                    outcome: spec.terminal,
                    tag: "terminal".to_owned(),
                    calls: self.terminal_calls.clone(),
                };
                self.bus.waterfall_sync_erased(
                    &self.root,
                    Arc::from(NAME),
                    payload.as_ref(),
                    &terminal,
                    opts,
                )
            }
            DispatchMode::Bail => {
                self.bus
                    .bail_erased(&self.root, Arc::from(NAME), payload.as_ref(), opts)
            }
        }
    }

    fn check(&self, model: &Model, what: &str) -> Result<(), TestCaseError> {
        let actual_sink = self.sink.lock().unwrap().clone();
        if actual_sink != model.sink {
            return Err(TestCaseError::fail(format!(
                "after {what}: the error sink holds {actual_sink:?}, the model {:?}",
                model.sink
            )));
        }
        for (index, calls) in self.calls.iter().enumerate() {
            let actual = calls.load(Ordering::Relaxed);
            if actual != model.calls[index] {
                return Err(TestCaseError::fail(format!(
                    "after {what}: listener {index} ran {actual} times, the model {}",
                    model.calls[index]
                )));
            }
        }
        let terminal = self.terminal_calls.load(Ordering::Relaxed);
        if terminal != model.terminal_calls {
            return Err(TestCaseError::fail(format!(
                "after {what}: the terminal ran {terminal} times, the model {}",
                model.terminal_calls
            )));
        }
        let actual: Vec<_> = self
            .bus
            .subscriptions()
            .into_iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.calls,
                    entry.prepend,
                    entry.once,
                    entry.global,
                    entry.kind,
                    entry.remote,
                )
            })
            .collect();
        let expected: Vec<_> = model
            .live_order()
            .into_iter()
            .map(|index| {
                let listener = &model.listeners[index];
                (
                    model.ids[index],
                    model.starts[index],
                    listener.prepend,
                    listener.once,
                    listener.global,
                    listener.body.kind(),
                    listener.body.remote(),
                )
            })
            .collect();
        if actual != expected {
            return Err(TestCaseError::fail(format!(
                "after {what}: the table is {actual:?}, the model {expected:?}"
            )));
        }
        let cursor = match &self.filter {
            Some(script) => script.cursor(),
            None => 0,
        };
        if cursor != model.filter_cursor {
            return Err(TestCaseError::fail(format!(
                "after {what}: the filter ran {cursor} times, the model {}",
                model.filter_cursor
            )));
        }
        Ok(())
    }

    async fn release(mut self) {
        for disposer in std::mem::take(&mut self.disposers) {
            let _ = disposer.dispose().await;
        }
    }
}

async fn run(case: Case) -> Result<(), TestCaseError> {
    let sink: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink_for_root = sink.clone();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |error: Arc<ErrorKind>| {
            sink_for_root.lock().unwrap().push(sink_tag(&error));
        }),
    );
    let harness = Harness::new(root.clone(), sink.clone(), &case)?;
    let mut model = Model::new(&case);
    model.ids.clone_from(&harness.ids);
    let mut outcome = Ok(());
    for (step, spec) in case.dispatches.iter().enumerate() {
        let what = format!("dispatch {step} {spec:?}");
        let expected = model.dispatch(spec);
        let actual = match harness.dispatch(spec).await {
            Ok(value) => Ok(value.map(|value| {
                *value
                    .downcast_ref::<bool>()
                    .unwrap_or_else(|| panic!("a listener returned a non-bool value"))
            })),
            Err(error) => Err(expected_err(&error)),
        };
        if actual != expected {
            outcome = Err(TestCaseError::fail(format!(
                "after {what}: the dispatch returned {actual:?}, the model {expected:?}"
            )));
            break;
        }
        if let Err(error) = harness.check(&model, &what) {
            outcome = Err(error);
            break;
        }
    }
    harness.release().await;
    let _ = root.shutdown().await;
    outcome
}

/// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED` or a
/// fresh one; on failure, panic with the seed and the shrunk input.
fn check<S: Strategy>(name: &str, strategy: S, test: impl Fn(S::Value) -> Result<(), TestCaseError>)
where
    S::Value: std::fmt::Debug,
{
    let seed: u64 = match std::env::var("RUTIS_PROPTEST_SEED") {
        Ok(seed) => seed.parse().expect("RUTIS_PROPTEST_SEED is a u64"),
        Err(_) => {
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            );
            hasher.finish()
        }
    };
    let mut bytes = [0u8; 32];
    for chunk in bytes.chunks_mut(8) {
        chunk.copy_from_slice(&seed.to_le_bytes());
    }
    // `Config::default()` reads PROPTEST_CASES (256 when unset).
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let cases = config.cases;
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    let started = std::time::Instant::now();
    if let Err(error) = runner.run(&strategy, test) {
        panic!("{name}: {error}\nreplay with RUTIS_PROPTEST_SEED={seed}");
    }
    eprintln!(
        "{name}: {cases} cases in {:?} (seed {seed})",
        started.elapsed()
    );
}

#[test]
fn the_five_dispatch_modes_match_a_reference_interpreter() {
    check("random listeners and dispatches", case(), |case| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(run(case))
    });
}
