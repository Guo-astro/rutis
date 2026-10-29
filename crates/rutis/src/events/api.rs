//! guarantee: sdk/rust-compile-errors
//! The plugin-facing event API: listeners are closures, and the five modes
//! are methods on [`Ctx`] named as in every other language.

use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use super::wire::{Payload, Value};
use super::{typed_arc, ErasedAsync, ErasedSync, Event, EventOptions, ListenerInfo, Mode};
use crate::bus::run::{AsyncNext, ErasedTerminal, FilterSpec, Inv, Keep, NoTerminal};
use crate::bus::table::Body;
use crate::bus::Opts;
use crate::ctx::Ctx;
use crate::effect::Disposer;
use crate::error::{panic_error, Error, ErrorKind, NextMisuse};
use crate::internal::CtxInternal;
use crate::key::EventKey;
use crate::BoxFuture;

// ── listeners ────────────────────────────────────────────────────

/// An async listener: any closure `|event, call| async move { … }`.
/// Authors never implement it; the compiler infers the closure's types.
///
/// It returns `Ok(None)` for "no result" (`serial` goes on), `Ok(Some(v))`
/// for a result.
///
/// An `async |event, call| …` closure is accepted too, but the compiler
/// cannot infer its argument types through this trait and stops with
/// "type annotations needed" (E0282) at the first use of `event`. Write
/// them, or use the `|event, call| async move { … }` form:
///
/// ```
/// use std::sync::Arc;
/// use rutis::events::Call;
/// # #[derive(rutis::serde::Serialize, rutis::serde::Deserialize)]
/// # #[serde(crate = "rutis::serde")]
/// # struct Ping(u32);
/// # impl rutis::Event for Ping { const NAME: &'static str = "doc::Ping"; type Value = u32; }
/// # fn listen(ctx: &rutis::Ctx) -> rutis::Result {
/// ctx.on::<Ping>(async |event: Arc<Ping>, _: Call<Ping>| Ok(Some(event.0)))?;
/// ctx.on::<Ping>(|event, _| async move { Ok(Some(event.0)) })?;
/// # Ok(())
/// # }
/// ```
#[diagnostic::on_unimplemented(
    message = "this is not a listener of `{E}`: a listener is a closure `|event, call| async move {{ ... }}`",
    label = "not `|event, call| async move {{ ... }}`",
    note = "it takes the event (`Arc<{E}>`) and the call (`Call<{E}>`), and its future returns `Ok(None)` or `Ok(Some(value))`; for a synchronous closure use `on_sync`"
)]
pub trait Listener<E: Event>:
    Fn(Arc<E>, Call<E>) -> <Self as Listener<E>>::Fut + Send + Sync + 'static
{
    type Fut: Future<Output = Result<Option<E::Value>, Error>> + Send + 'static;
}

impl<E, F, Fut> Listener<E> for F
where
    E: Event,
    F: Fn(Arc<E>, Call<E>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<E::Value>, Error>> + Send + 'static,
{
    type Fut = Fut;
}

/// One call of an async listener: which mode, under which name, and the
/// rest of a waterfall.
pub struct Call<E: Event> {
    mode: Mode,
    name: Arc<str>,
    ctx: Ctx,
    next: Option<AsyncNext>,
    _event: PhantomData<fn() -> E>,
}

impl<E: Event> Call<E> {
    /// This dispatch's mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The dispatched name (for a prefix listener, the full name).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The context that registered the listener.
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// The rest of the chain (`waterfall` only; otherwise
    /// `Err(WaterfallNext)`). Its value is the next listener's, as is:
    /// `None` when it had none.
    pub async fn next(self) -> Result<Option<E::Value>, Error> {
        let Some(next) = self.next else {
            return Err(ErrorKind::WaterfallNext {
                name: self.name.to_string(),
                misuse: NextMisuse::NotWaterfall(self.mode),
            }
            .into());
        };
        // Called once the listener's call was over (from a task it left
        // behind): its waterfall has its result already.
        if next.ended() {
            return Err(ErrorKind::WaterfallNext {
                name: self.name.to_string(),
                misuse: NextMisuse::Ended,
            }
            .into());
        }
        next.invoke_typed::<E::Value>().await
    }
}

/// One call of a synchronous listener. It lives for the call only.
pub struct SyncCall<'a, E: Event> {
    inv: &'a Inv<'a>,
    ctx: &'a Ctx,
    _event: PhantomData<fn() -> E>,
}

impl<E: Event> SyncCall<'_, E> {
    /// This dispatch's mode.
    pub fn mode(&self) -> Mode {
        self.inv.mode
    }

    /// The dispatched name (for a prefix listener, the full name).
    pub fn name(&self) -> &str {
        self.inv.name
    }

    /// The context that registered the listener.
    pub fn ctx(&self) -> &Ctx {
        self.ctx
    }

    /// A synchronous listener has no rest of the chain to run: in a
    /// `waterfall` and in every other mode, `Err(WaterfallNext)`.
    pub fn next(self) -> Result<Option<E::Value>, Error> {
        let inv = self.inv;
        let misuse = match inv.mode {
            Mode::Waterfall => NextMisuse::SyncInAsync,
            mode => NextMisuse::NotWaterfall(mode),
        };
        Err(ErrorKind::WaterfallNext {
            name: inv.name.to_string(),
            misuse,
        }
        .into())
    }
}

/// Runs an owned-`Call` listener: the call owns its `next`, which runs
/// the rest of the waterfall when the listener asks for it.
pub(crate) struct OwnedAsync<L, E>(pub L, pub PhantomData<fn(E) -> E>);

impl<E: Event, L: Listener<E>> ErasedAsync for OwnedAsync<L, E> {
    fn call<'a>(
        &'a self,
        ctx: &'a Ctx,
        inv: &'a Inv<'a>,
        next: Option<AsyncNext>,
    ) -> BoxFuture<'a, Result<Option<Value>, Error>> {
        Box::pin(async move {
            let event = typed_arc::<E>(inv)?;
            // When this call ends (returns, fails, panics or is dropped),
            // its `next` is `Ended`.
            let _ended = next.as_ref().map(AsyncNext::end_on_drop);
            let call = Call {
                mode: inv.mode,
                name: inv.name.clone(),
                ctx: ctx.clone(),
                next,
                _event: PhantomData,
            };
            Ok((self.0)(event, call).await?.map(Value::new))
        })
    }
}

pub(crate) struct OwnedSync<F, E>(pub F, pub PhantomData<fn(E) -> E>);

impl<E, F> ErasedSync for OwnedSync<F, E>
where
    E: Event,
    F: Fn(Arc<E>, SyncCall<'_, E>) -> Result<Option<E::Value>, Error> + Send + Sync + 'static,
{
    fn call(&self, ctx: &Ctx, inv: &Inv<'_>) -> Result<Option<Value>, Error> {
        let event = typed_arc::<E>(inv)?;
        let call = SyncCall {
            inv,
            ctx,
            _event: PhantomData,
        };
        Ok((self.0)(event, call)?.map(Value::new))
    }
}

/// Registration with options: `ctx.listen::<E>()`.
///
/// ```no_run
/// # use rutis::serde::{Deserialize, Serialize};
/// # #[derive(Serialize, Deserialize)] struct Created(String);
/// # impl rutis::Event for Created { const NAME: &'static str = "session/created"; type Value = (); }
/// # fn demo(ctx: &rutis::Ctx) -> rutis::Result {
/// ctx.listen::<Created>().prefix("session/").once().on(|_, call| async move {
///     println!("first {}", call.name());
///     Ok(None)
/// })?;
/// # Ok(()) }
/// ```
pub struct Listen<'c, E: Event> {
    ctx: &'c Ctx,
    name: Option<Arc<str>>,
    prefix: bool,
    opts: EventOptions,
    _event: PhantomData<fn() -> E>,
}

impl<'c, E: Event> Listen<'c, E> {
    pub(crate) fn new(ctx: &'c Ctx) -> Self {
        Self {
            ctx,
            name: None,
            prefix: false,
            opts: EventOptions::default(),
            _event: PhantomData,
        }
    }

    /// Listen under `name` instead of `E::NAME` (an event type without a
    /// name of its own needs one).
    pub fn name(mut self, name: impl Into<Arc<str>>) -> Self {
        self.name = Some(name.into());
        self.prefix = false;
        self
    }

    /// Listen to every name starting with `prefix`; `call.name()` is the
    /// dispatched one.
    pub fn prefix(mut self, prefix: impl Into<Arc<str>>) -> Self {
        self.name = Some(prefix.into());
        self.prefix = true;
        self
    }

    /// Called at most once: the first dispatch that selects it claims it.
    pub fn once(mut self) -> Self {
        self.opts.once = true;
        self
    }

    /// Take a position before every existing listener.
    pub fn prepend(mut self) -> Self {
        self.opts.prepend = true;
        self
    }

    /// Custom filters skip it.
    pub fn global(mut self) -> Self {
        self.opts.global = true;
        self
    }

    /// Register an async listener.
    pub fn on(self, listener: impl Listener<E>) -> Result<Disposer, Error> {
        let label = if self.prefix {
            "event pattern"
        } else {
            "event listener"
        };
        let body = Body::Async(Arc::new(OwnedAsync(listener, PhantomData)));
        self.register(body, label)
    }

    /// Register a synchronous listener: every mode can call it.
    pub fn on_sync(
        self,
        listener: impl Fn(Arc<E>, SyncCall<'_, E>) -> Result<Option<E::Value>, Error>
            + Send
            + Sync
            + 'static,
    ) -> Result<Disposer, Error> {
        let label = if self.prefix {
            "sync event pattern"
        } else {
            "sync event listener"
        };
        let body = Body::Sync(Arc::new(OwnedSync(listener, PhantomData)));
        self.register(body, label)
    }

    fn register(self, body: Body, label: &str) -> Result<Disposer, Error> {
        let name = match self.name {
            Some(name) => name,
            None => EventKey::<E>::of().checked_name()?,
        };
        self.ctx
            .events()
            .register(self.ctx, name, self.prefix, body, self.opts, None, label)
    }
}

// ── dispatch ─────────────────────────────────────────────────────

fn name_of<E: Event>(event: &E, named: Option<&Arc<str>>) -> Result<Arc<str>, Error> {
    if let Some(name) = named {
        return Ok(name.clone());
    }
    let name = event.name();
    if name.is_empty() {
        return EventKey::<E>::of().checked_name();
    }
    Ok(Arc::from(name))
}

fn typed_value<V: serde::Serialize + serde::de::DeserializeOwned + Send + 'static>(
    ctx: &Ctx,
    value: Option<Value>,
) -> Result<Option<V>, Error> {
    let bus = ctx.events();
    value
        .map(|value| value.into_typed::<V>(&*bus.transcoder()))
        .transpose()
}

/// Where a dispatch goes: the context it is emitted from, and its filter.
#[derive(Clone)]
struct Target<'c> {
    ctx: &'c Ctx,
    keep: Option<Arc<Keep>>,
    /// Dispatch under this name instead of the event's (`CtxInternal::named`).
    named: Option<Arc<str>>,
}

impl<'c> Target<'c> {
    fn opts(&self) -> Opts {
        Opts {
            filter: self.keep.clone().map(FilterSpec::Each),
            ..Opts::default()
        }
    }

    fn emit<E: Event>(&self, event: Arc<E>) -> Result<(), Error> {
        let name = name_of(&*event, self.named.as_ref())?;
        self.ctx
            .events()
            .emit_erased(self.ctx, name, Payload::new(event), self.opts())
            .map(drop)
    }

    fn parallel<E: Event>(&self, event: Arc<E>) -> BoxFuture<'c, Result<(), Error>> {
        match name_of(&*event, self.named.as_ref()) {
            Ok(name) => {
                self.ctx
                    .events()
                    .parallel_erased(self.ctx, name, Payload::new(event), self.opts())
            }
            Err(error) => Box::pin(std::future::ready(Err(error))),
        }
    }

    fn serial<E: Event>(&self, event: Arc<E>) -> BoxFuture<'c, Result<Option<E::Value>, Error>> {
        let ctx = self.ctx;
        let name = match name_of(&*event, self.named.as_ref()) {
            Ok(name) => name,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let run = ctx
            .events()
            .serial_erased(ctx, name, Payload::new(event), self.opts());
        Box::pin(async move { typed_value(ctx, run.await?) })
    }

    fn bail<E: Event>(&self, event: Arc<E>) -> Result<Option<E::Value>, Error> {
        let name = name_of(&*event, self.named.as_ref())?;
        let payload = Payload::new(event);
        let out = self
            .ctx
            .events()
            .bail_erased(self.ctx, name, payload.as_ref(), self.opts())?;
        typed_value(self.ctx, out)
    }
}

/// The five ways to send, with a filter: `ctx.filter(keep)`. `keep` sees
/// each non-global candidate's descriptor, in this process, before any
/// call.
pub struct Filtered<'c> {
    target: Target<'c>,
}

impl<'c> Filtered<'c> {
    pub(crate) fn new(ctx: &'c Ctx, keep: Arc<Keep>) -> Self {
        Self {
            target: Target {
                ctx,
                keep: Some(keep),
                named: None,
            },
        }
    }

    /// The five ways to send under `name` instead of the event's own.
    pub(crate) fn named(ctx: &'c Ctx, name: Arc<str>) -> Self {
        Self {
            target: Target {
                ctx,
                keep: None,
                named: Some(name),
            },
        }
    }

    /// `emit`: returns at once; a listener's failure goes to the error sink.
    pub fn emit<E: Event>(&self, event: impl crate::events::IntoEvent<E>) -> Result<(), Error> {
        self.target.emit(event.into_event())
    }

    /// `parallel`: waits for every call; failures are `ListenersFailed`.
    pub fn parallel<E: Event>(
        &self,
        event: impl crate::events::IntoEvent<E>,
    ) -> BoxFuture<'c, Result<(), Error>> {
        self.target.parallel(event.into_event())
    }

    /// `serial`: the first result that is not "no result".
    pub fn serial<E: Event>(
        &self,
        event: impl crate::events::IntoEvent<E>,
    ) -> BoxFuture<'c, Result<Option<E::Value>, Error>> {
        self.target.serial(event.into_event())
    }

    /// `waterfall`: each listener gets `next`; `.terminal(…)` ends the chain.
    pub fn waterfall<E: Event>(&self, event: impl crate::events::IntoEvent<E>) -> Waterfall<'c, E> {
        Waterfall::new(self.target.clone(), event.into_event())
    }

    /// `bail`, synchronously: as `serial`, inline; an async listener is an
    /// `AsyncListener` error.
    pub fn bail<E: Event>(
        &self,
        event: impl crate::events::IntoEvent<E>,
    ) -> Result<Option<E::Value>, Error> {
        self.target.bail(event.into_event())
    }
}

/// A `waterfall` to run: `.terminal(…)` optionally, then `.await`.
pub struct Waterfall<'c, E: Event> {
    target: Target<'c>,
    event: Arc<E>,
    terminal: Option<Box<dyn ErasedTerminal + Send + 'static>>,
}

impl<'c, E: Event> Waterfall<'c, E> {
    fn new(target: Target<'c>, event: Arc<E>) -> Self {
        Self {
            target,
            event,
            terminal: None,
        }
    }

    /// What the last listener's `next` returns (without one, `None`).
    pub fn terminal<F, Fut>(mut self, terminal: F) -> Self
    where
        F: FnOnce(Arc<E>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<E::Value>, Error>> + Send + 'static,
    {
        self.terminal = Some(Box::new(FnTerminal::<E, F>(
            Mutex::new(Some(terminal)),
            PhantomData,
        )));
        self
    }
}

impl<'c, E: Event> IntoFuture for Waterfall<'c, E> {
    type Output = Result<Option<E::Value>, Error>;
    type IntoFuture = BoxFuture<'c, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        let Waterfall {
            target,
            event,
            terminal,
        } = self;
        Box::pin(async move {
            let ctx = target.ctx;
            let name = name_of(&*event, target.named.as_ref())?;
            let payload = Payload::new(event);
            let terminal: Box<dyn ErasedTerminal + Send> = match terminal {
                Some(terminal) => terminal,
                None => Box::new(NoTerminal),
            };
            let out = ctx
                .events()
                .waterfall_erased(ctx, name, payload, terminal, target.opts())
                .await?;
            typed_value(ctx, out)
        })
    }
}

struct FnTerminal<E, F>(Mutex<Option<F>>, PhantomData<fn(E) -> E>);

impl<E, F, Fut> ErasedTerminal for FnTerminal<E, F>
where
    E: Event,
    F: FnOnce(Arc<E>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Option<E::Value>, Error>> + Send + 'static,
{
    fn call<'a>(&'a self, inv: &'a Inv<'a>) -> BoxFuture<'a, Result<Option<Value>, Error>> {
        let terminal = self.0.lock().unwrap().take();
        Box::pin(async move {
            let Some(terminal) = terminal else {
                return Ok(None);
            };
            let event = typed_arc::<E>(inv)?;
            let out = super::CatchUnwind::new(terminal(event))
                .await
                .unwrap_or_else(|panic| Err(panic_error(panic)))?;
            Ok(out.map(Value::new))
        })
    }
}

// ── on Ctx ───────────────────────────────────────────────────────

impl Ctx {
    /// Listen to `E` with an async closure: `ctx.on::<E>(|event, call| async move { … })`.
    /// Withdrawn when the plugin stops (or by the returned [`Disposer`]).
    /// An `async |event, call|` closure needs its argument types written
    /// ([`Listener`]).
    pub fn on<E: Event>(&self, listener: impl Listener<E>) -> Result<Disposer, Error> {
        self.listen::<E>().on(listener)
    }

    /// Listen to `E` with a synchronous closure, which every mode can call
    /// (`bail` needs one).
    pub fn on_sync<E: Event>(
        &self,
        listener: impl Fn(Arc<E>, SyncCall<'_, E>) -> Result<Option<E::Value>, Error>
            + Send
            + Sync
            + 'static,
    ) -> Result<Disposer, Error> {
        self.listen::<E>().on_sync(listener)
    }

    /// Listen with options: a prefix, another name, `once`, `prepend`,
    /// `global`.
    pub fn listen<E: Event>(&self) -> Listen<'_, E> {
        Listen::new(self)
    }

    /// `emit`: returns at once. A listener's failure goes to the error sink,
    /// never to the emitter.
    pub fn emit<E: Event>(&self, event: impl crate::events::IntoEvent<E>) -> Result<(), Error> {
        self.target().emit(event.into_event())
    }

    /// `parallel`: starts every call and waits for all. Failures are
    /// `ListenersFailed`, in registration order.
    pub fn parallel<E: Event>(
        &self,
        event: impl crate::events::IntoEvent<E>,
    ) -> BoxFuture<'_, Result<(), Error>> {
        self.target().parallel(event.into_event())
    }

    /// `serial`: one call at a time; the first result that is not "no
    /// result" (`None`, `Some(false)`) ends it.
    pub fn serial<E: Event>(
        &self,
        event: impl crate::events::IntoEvent<E>,
    ) -> BoxFuture<'_, Result<Option<E::Value>, Error>> {
        self.target().serial(event.into_event())
    }

    /// `waterfall`: each listener gets `next`; `.terminal(…)` ends the
    /// chain, then `.await`.
    pub fn waterfall<E: Event>(&self, event: impl crate::events::IntoEvent<E>) -> Waterfall<'_, E> {
        Waterfall::new(self.target(), event.into_event())
    }

    /// `bail`, synchronously: as `serial`, inline; an async listener is an
    /// `AsyncListener` error.
    pub fn bail<E: Event>(
        &self,
        event: impl crate::events::IntoEvent<E>,
    ) -> Result<Option<E::Value>, Error> {
        self.target().bail(event.into_event())
    }

    /// The five ways to send, to the listeners `keep` accepts.
    pub fn filter(
        &self,
        keep: impl Fn(&ListenerInfo) -> bool + Send + Sync + 'static,
    ) -> Filtered<'_> {
        Filtered::new(self, Arc::new(move |info: &ListenerInfo| Ok(keep(info))))
    }

    fn target(&self) -> Target<'_> {
        Target {
            ctx: self,
            keep: None,
            named: None,
        }
    }
}
