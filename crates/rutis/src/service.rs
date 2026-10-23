//! Rust services the plugins of other languages call by name
//! ([`#[rutis::service]`](crate::service)): the registry each marked `impl`
//! block adds itself to, the hook [`Ctx::provide`](crate::Ctx::provide)
//! calls, and what the code the macro generates calls.
//!
//! The kernel knows no language. What a call carries (arguments, results,
//! functions passed by reference, thrown errors) is the business of the
//! [`Wire`] the bridge installs; the generated code reaches it only through
//! the object-safe traits here, with values crossing as erased serde. So a
//! marked type in a crate that depends on `rutis` alone (a dylib plugin, a
//! dependency crate) is served by whichever bridge the host links.
//!
//! A public method that cannot be bound is a compile error at that method
//! (R5):
//!
//! ```compile_fail
//! pub struct Counter(std::sync::atomic::AtomicU32);
//!
//! #[rutis::service]
//! impl Counter {
//!     pub fn bump(&mut self) {} // a shared service takes `&self`
//! }
//! ```
//!
//! Left out with `#[service(skip)]`, the rest is served:
//!
//! ```
//! pub struct Counter(std::sync::atomic::AtomicU32);
//!
//! #[rutis::service]
//! impl Counter {
//!     #[service(skip)]
//!     pub fn bump(&mut self) {}
//!     pub fn get(&self) -> u32 {
//!         self.0.load(std::sync::atomic::Ordering::Relaxed)
//!     }
//! }
//! ```

use std::any::{Any, TypeId};
use std::cell::Cell;
use std::convert::Infallible;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::{BoxFuture, CordisError, Ctx, Disposer, TypeKey};

pub use erased_serde;

// ── The registry ────────────────────────────────────────────────

/// One marked `impl` block: a type's exported name and some of its methods.
/// A type with several marked blocks has one entry per block.
pub struct Entry {
    type_id: fn() -> TypeId,
    type_name: fn() -> &'static str,
    name: &'static str,
    /// Whether `name` was given (`#[service(name = "...")]`), not derived.
    named: bool,
    methods: fn() -> &'static [&'static str],
    resolve: Resolve,
    invoke: Invoke,
}

/// Read the marked service `provision` provided at `key` through `ctx`, by
/// the strict read path ([`Ctx::require_provided_as`]): `check()` applies,
/// read interceptors apply with `actor` (the calling row) as the reader,
/// and a read that finds another binding than the one provided (the slot
/// provided again, by another build after a swap, say) is refused.
pub type Resolve = fn(
    ctx: &Ctx,
    key: &TypeKey,
    provision: &Provision,
    actor: Option<&Ctx>,
) -> Result<Arc<dyn Any + Send + Sync>, String>;

/// The native binding a marked service was provided as: the entry by name
/// serves that binding only. Filled once the binding is registered; empty
/// before (the entry is served first, R6) and never another one.
pub struct Provision(OnceLock<Weak<crate::registry::Binding>>);

impl Provision {
    pub(crate) fn new() -> Self {
        Self(OnceLock::new())
    }
    pub(crate) fn fill(&self, binding: &Arc<crate::registry::Binding>) {
        let _ = self.0.set(Arc::downgrade(binding));
    }
    /// Whether `binding` is the one provided.
    pub(crate) fn is(&self, binding: &Arc<crate::registry::Binding>) -> bool {
        self.0
            .get()
            .is_some_and(|provided| std::ptr::eq(provided.as_ptr(), Arc::as_ptr(binding)))
    }
}

/// A marked block's dispatch: call `method` of `service` with `args`.
pub type Invoke = fn(
    service: &Arc<dyn Any + Send + Sync>,
    method: &str,
    args: Encoded,
    wire: &'static dyn Wire,
) -> Result<Answer, CallError>;

impl Entry {
    /// What the macro builds. `methods` lists the block's methods the
    /// build includes (those whose `#[cfg]` holds).
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        type_id: fn() -> TypeId,
        type_name: fn() -> &'static str,
        name: &'static str,
        named: bool,
        methods: fn() -> &'static [&'static str],
        resolve: Resolve,
        invoke: Invoke,
    ) -> Self {
        Self {
            type_id,
            type_name,
            name,
            named,
            methods,
            resolve,
            invoke,
        }
    }

    pub fn type_name(&self) -> &'static str {
        (self.type_name)()
    }
    pub fn name(&self) -> &'static str {
        self.name
    }
    /// Whether the name was given (`#[service(name = "...")]`), not derived.
    pub fn named(&self) -> bool {
        self.named
    }
    pub fn methods(&self) -> &'static [&'static str] {
        (self.methods)()
    }
    /// The service provided at `key`, read through `ctx` ([`Resolve`]).
    pub fn resolve(
        &self,
        ctx: &Ctx,
        key: &TypeKey,
        provision: &Provision,
        actor: Option<&Ctx>,
    ) -> Result<Arc<dyn Any + Send + Sync>, String> {
        (self.resolve)(ctx, key, provision, actor)
    }
    /// Call `method`, one of [`Entry::methods`].
    pub fn invoke(
        &self,
        service: &Arc<dyn Any + Send + Sync>,
        method: &str,
        args: Encoded,
        wire: &'static dyn Wire,
    ) -> Result<Answer, CallError> {
        (self.invoke)(service, method, args, wire)
    }
}

/// An entry and the generation of the image that registered it.
struct Registered {
    entry: &'static Entry,
    generation: u64,
}

static ENTRIES: Mutex<Vec<Registered>> = Mutex::new(Vec::new());

/// The generation of a loaded library ([`loading`]): which image the
/// marked blocks it registered, and the code of the plugins it builds, come
/// from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Generation(u64);

/// The generation the next [`loading`] call gives the image it loads; the
/// program's own entries are generation 0.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The generation of the image whose initializers run on this thread.
    static LOADING: Cell<u64> = const { Cell::new(0) };
}

/// Run `load`, which loads a library (`dlopen`, `LoadLibraryExW`): the
/// marked blocks its initializers register belong to the returned
/// generation. Two builds of one plugin crate keep their types' `TypeId`s
/// (they depend on the crate's name and metadata, not its source), so the
/// registry tells their blocks apart by generation, and a plugin's fiber
/// marked with its library's generation ([`set_generation`]) serves what it
/// provides with that library's blocks. Loaders of plugin libraries call
/// this around the call that maps one.
pub fn loading<R>(load: impl FnOnce() -> R) -> (R, Generation) {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let outer = LOADING.with(|loading| loading.replace(generation));
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            LOADING.with(|loading| loading.set(self.0));
        }
    }
    let _restore = Restore(outer);
    (load(), Generation(generation))
}

/// The generation whose initializers run on this thread now, inside
/// [`loading`]; `None` outside.
#[doc(hidden)]
pub fn current_loading() -> Option<Generation> {
    match LOADING.with(Cell::get) {
        0 => None,
        generation => Some(Generation(generation)),
    }
}

impl Generation {
    /// A number for this generation, for diagnostics and tests.
    pub fn id(self) -> u64 {
        self.0
    }
}

/// The code running in `ctx`'s fiber, and in the fibers it starts, comes
/// from the library loaded as `generation`: a marked value it provides is
/// served with that library's blocks. Loaders call this when a plugin built
/// by a library's factory is applied.
pub fn set_generation(ctx: &Ctx, generation: Generation) {
    ctx.set_code_generation(generation.0);
}

/// Add `entry`; the macro's generated code calls this when the binary or
/// library holding it is loaded.
pub fn register(entry: &'static Entry) {
    let generation = LOADING.with(Cell::get);
    ENTRIES
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(Registered { entry, generation });
}

fn entries() -> Vec<&'static Entry> {
    ENTRIES
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .map(|known| known.entry)
        .collect()
}

/// The marked blocks of the type `type_id` registered by the image of
/// `generation`, in the order they were added. When that image registered
/// none for the type (its build left the marker out), or for code of no
/// loaded library (`None`), those of the program itself (generation 0, the
/// host and the SDK), or none: never another library's, whose code would
/// not match the value's layout.
pub fn entries_of(type_id: TypeId, generation: Option<Generation>) -> Vec<&'static Entry> {
    let entries = ENTRIES.lock().unwrap_or_else(|poison| poison.into_inner());
    let of = |generation: u64| -> Vec<&'static Entry> {
        entries
            .iter()
            .filter(|known| known.generation == generation && (known.entry.type_id)() == type_id)
            .map(|known| known.entry)
            .collect()
    };
    match generation {
        Some(Generation(generation)) if generation != 0 => {
            let own = of(generation);
            if own.is_empty() {
                of(0)
            } else {
                own
            }
        }
        _ => of(0),
    }
}

/// Generations a newer load of the same plugin replaced
/// ([`supersede`]): their blocks still serve the values they provided,
/// but their names are no longer what the program exports.
static SUPERSEDED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// `older` are earlier loads of the plugin just loaded as a newer
/// generation: name lookups ([`exported_as`], [`is_exported`]) consider the
/// newest only, so a name a newer build renamed or dropped is no longer
/// exported. Loaders call this when they load a new build of a plugin.
pub fn supersede(older: impl IntoIterator<Item = Generation>) {
    let mut superseded = SUPERSEDED
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    for Generation(generation) in older {
        if generation != 0 && !superseded.contains(&generation) {
            superseded.push(generation);
        }
    }
}

/// Whether some marked type is exported as `name`.
pub fn is_exported(name: &str) -> bool {
    exported_as(name).is_some()
}

/// The type marked to be exported as `name` (its type name), if any. A
/// type is served only once provided: this says the program can serve it.
/// Entries of one type in one generation are one block set: the name is
/// the one [`exported_name`] gives it, not each entry's own name.
pub fn exported_as(name: &str) -> Option<&'static str> {
    type_groups().into_iter().rev().find_map(|group| {
        let exported = exported_name(&group).ok()?;
        (exported == name).then(|| group.first().map(|entry| entry.type_name()))?
    })
}

/// The registered blocks grouped by (type, generation), in registration
/// order, leaving out generations a newer build superseded ([`supersede`]).
fn type_groups() -> Vec<Vec<&'static Entry>> {
    let superseded = SUPERSEDED
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let entries = ENTRIES.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut groups: Vec<(TypeId, u64, Vec<&'static Entry>)> = Vec::new();
    for known in entries.iter() {
        if superseded.contains(&known.generation) {
            continue;
        }
        let type_id = (known.entry.type_id)();
        match groups
            .iter_mut()
            .find(|(id, generation, _)| *id == type_id && *generation == known.generation)
        {
            Some((_, _, group)) => group.push(known.entry),
            None => groups.push((type_id, known.generation, vec![known.entry])),
        }
    }
    groups.into_iter().map(|(_, _, group)| group).collect()
}

/// Whether the type `type_id` is marked.
pub fn is_marked(type_id: TypeId) -> bool {
    entries().iter().any(|entry| (entry.type_id)() == type_id)
}

/// The effective exported name of the type `type_id`, if it is marked: the
/// name a block gave it, else the derived one.
pub fn name_of(type_id: TypeId) -> Option<&'static str> {
    let entries = entries();
    let mut fallback = None;
    let mut named = None;
    for entry in entries.iter().filter(|entry| (entry.type_id)() == type_id) {
        fallback.get_or_insert(entry.name());
        if entry.named() {
            named = Some(entry.name());
        }
    }
    named.or(fallback)
}

/// The name the marked blocks `entries` of one type export it as: the one
/// a block names, else the derived one. Two blocks naming it differently
/// is an error.
pub fn exported_name(entries: &[&'static Entry]) -> Result<&'static str, CordisError> {
    let mut named: Option<&'static str> = None;
    for entry in entries.iter().filter(|entry| entry.named) {
        match named {
            Some(name) if name != entry.name => {
                return Err(CordisError::Validation {
                    issues: vec![format!(
                        "two #[rutis::service] blocks of {} name it `{name}` and `{}`",
                        entry.type_name(),
                        entry.name
                    )],
                })
            }
            _ => named = Some(entry.name),
        }
    }
    Ok(named.unwrap_or_else(|| entries.first().map_or("", |entry| entry.name)))
}

// ── Exposure at provide time ────────────────────────────────────

/// A marked service being provided, for the [`Exposer`] to serve by name.
#[non_exhaustive]
pub struct Exposure<'a> {
    /// The context providing it: serve the name there, and read the service
    /// through it on every call ([`Entry::resolve`]).
    pub ctx: &'a Ctx,
    /// Where the native service is provided.
    pub native: &'a TypeKey,
    /// The isolation scope ([`Ctx::isolate`]) the native service is
    /// provided in, if any: the name is served in the same one.
    pub scope: Option<&'a str>,
    /// The marked blocks of its type.
    pub entries: Vec<&'static Entry>,
    /// The `check()` predicate the native binding is gated by
    /// ([`Ctx::provide_as_with_check`]): it gates the name too.
    pub check: Option<Check>,
    /// The native binding being provided, for [`Entry::resolve`].
    pub provision: Arc<Provision>,
}

/// What serves marked services to other languages: the bridge installs it.
///
/// A provide asks [`admit`](Exposer::admit) before the native service is
/// registered and [`expose`](Exposer::expose) after it: a name that cannot
/// be served refuses the provide with nothing served, and the name is
/// never served before the native service it calls exists. Exposed after
/// it, the name is withdrawn before it, by the provide's disposer and by
/// the provider's unload alike (cleanups run last in, first out).
pub trait Exposer: Send + Sync + 'static {
    /// Whether the marked service `exposure` describes can be served by
    /// name: its blocks agree on the name and the name is free where it
    /// would be served ([`vacant`]). Nothing is registered.
    fn admit(&self, exposure: &Exposure<'_>) -> Result<(), CordisError>;
    /// Serve the marked service `exposure` describes by name, in the same
    /// scope as the native one, which is provided by now. The returned
    /// disposer stops serving it.
    fn expose(&self, exposure: Exposure<'_>) -> Result<Disposer, CordisError>;
}

/// Whether a provide of `key` on `ctx` finds its slot free (in the
/// isolation scope `ctx` gives `key`): for an [`Exposer`] checking the name
/// before the native service is registered.
pub fn vacant(ctx: &Ctx, key: &TypeKey) -> bool {
    !ctx.slot_taken(key)
}

/// The `check()` predicate a service was provided with
/// ([`Ctx::provide_as_with_check`]): the entry by name is gated by it too.
pub type Check = Arc<dyn Fn() -> bool + Send + Sync>;

static EXPOSER: OnceLock<&'static dyn Exposer> = OnceLock::new();

/// Install the exposer; the first one stays.
pub fn install(exposer: &'static dyn Exposer) {
    let _ = EXPOSER.set(exposer);
}

/// The marked blocks of `T`: `None` when `T` is not marked or nothing
/// serves marked services.
pub(crate) fn marked<T: ?Sized + 'static>(ctx: &Ctx) -> Option<Vec<&'static Entry>> {
    EXPOSER.get()?;
    // The blocks of the library whose code provides it: a value of one
    // build is never served with another build's code.
    let generation = ctx.code_generation().map(Generation);
    let entries = entries_of(TypeId::of::<T>(), generation);
    (!entries.is_empty()).then_some(entries)
}

/// Whether a marked service can be served by name ([`Exposer::admit`]).
pub(crate) fn admit(exposure: &Exposure<'_>) -> Result<(), CordisError> {
    match EXPOSER.get() {
        Some(exposer) => exposer.admit(exposure),
        None => Ok(()),
    }
}

/// Serve a marked service by name ([`marked`]).
pub(crate) fn expose(exposure: Exposure<'_>) -> Option<Result<Disposer, CordisError>> {
    let exposer = EXPOSER.get()?;
    Some(exposer.expose(exposure))
}

/// One disposer for both: `first` is disposed, then `then`.
pub fn chain(first: Disposer, then: Disposer) -> Disposer {
    Disposer::new(Box::new(move || {
        Box::pin(async move {
            let first = first.dispose().await;
            let then = then.dispose().await;
            first.and(then)
        })
    }))
}

// ── What a call carries ─────────────────────────────────────────

/// A value as the wire holds it (for the bridge, its call value).
pub struct Encoded(pub Box<dyn Any + Send>);

/// What a marked method returned: now, or once its future settles.
pub enum Answer {
    Ready(Encoded),
    Later(BoxFuture<'static, Result<Encoded, CallError>>),
}

/// A failed call into or out of a marked service: what a function passed
/// by another language fails with, and what a method's `Err` is thrown as.
/// The error type of a function parameter must convert from it
/// (`CordisError`, `anyhow::Error` and boxed errors do).
pub struct CallError {
    message: String,
    raw: Option<Box<dyn Any + Send + Sync>>,
}

impl CallError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            raw: None,
        }
    }

    /// An error carrying the wire's own error, read back with
    /// [`CallError::into_raw`].
    pub fn with_raw(message: impl Into<String>, raw: Box<dyn Any + Send + Sync>) -> Self {
        Self {
            message: message.into(),
            raw: Some(raw),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn into_raw(self) -> (String, Option<Box<dyn Any + Send + Sync>>) {
        (self.message, self.raw)
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::fmt::Debug for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CallError").field(&self.message).finish()
    }
}

impl std::error::Error for CallError {}

impl From<CallError> for CordisError {
    fn from(error: CallError) -> Self {
        CordisError::PluginFailed(Box::new(error))
    }
}

/// A function another language passed, to call back.
pub trait Function: Send + Sync + 'static {
    fn call(&self, args: Vec<Encoded>) -> BoxFuture<'static, Result<Encoded, CallError>>;
}

/// Where a marked service's calls come from: the bridge.
pub trait Wire: Send + Sync + 'static {
    /// The `count` arguments of a call, those not passed undefined.
    fn arguments(&self, args: Encoded, count: usize) -> Result<Vec<Encoded>, CallError>;
    /// Decode `value` as data, handing `into` the deserializer.
    fn decode(
        &self,
        value: Encoded,
        into: &mut dyn FnMut(
            &mut dyn erased_serde::Deserializer<'static>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), CallError>;
    fn encode(&self, value: &dyn erased_serde::Serialize) -> Result<Encoded, CallError>;
    fn undefined(&self) -> Encoded;
    /// The function passed as `value`.
    fn function(&self, value: Encoded) -> Result<Arc<dyn Function>, CallError>;
    /// The context of the row calling, read while the call starts.
    fn caller(&self) -> Option<Ctx>;
    /// A call that is wrong in itself (arity, a value of the wrong shape).
    fn error(&self, message: String) -> CallError;
    /// A method's `Err` as thrown: `name`, `code`, `message`, `data`.
    fn failure(
        &self,
        name: &str,
        code: &str,
        message: String,
        data: Option<&dyn erased_serde::Serialize>,
    ) -> CallError;
    /// `error`, when it is the wire's own error type: thrown as it is.
    fn forwarded(&self, error: &dyn Any) -> Option<CallError>;
}

// ── What generated code calls ───────────────────────────────────

/// The marked `T` a block serves.
pub fn downcast<T: Any + Send + Sync>(service: &Arc<dyn Any + Send + Sync>) -> Arc<T> {
    service
        .clone()
        .downcast::<T>()
        .expect("a marked service is served as its own type")
}

/// Decode a parameter (any `Deserialize` type).
pub fn decode<T: DeserializeOwned>(
    wire: &'static dyn Wire,
    value: Encoded,
) -> Result<T, CallError> {
    let mut decoded = None;
    wire.decode(value, &mut |deserializer| {
        decoded = Some(erased_serde::deserialize::<T>(deserializer)?);
        Ok(())
    })?;
    decoded.ok_or_else(|| wire.error("the value was not decoded".into()))
}

/// The calling row's context, for a method taking `&Ctx`.
pub fn caller(wire: &'static dyn Wire, method: &str) -> Result<Ctx, CallError> {
    wire.caller().ok_or_else(|| {
        wire.error(format!(
            "{method} needs the calling row's context: only a row of a language runtime can call it"
        ))
    })
}

/// The function passed for a parameter taking one.
pub fn function(wire: &'static dyn Wire, value: Encoded) -> Result<Arc<dyn Function>, CallError> {
    wire.function(value)
}

/// Encode an argument of a call back.
pub fn encode_arg<T: Serialize + ?Sized>(
    wire: &'static dyn Wire,
    value: &T,
) -> Result<Encoded, CallError> {
    wire.encode(&value as &dyn erased_serde::Serialize)
}

/// Call `function` back with `args` and decode what it returned to `X`
/// (ignored for `()`); its failure is `E`'s.
pub fn call_back<X, E>(
    function: &Arc<dyn Function>,
    wire: &'static dyn Wire,
    args: Vec<Result<Encoded, CallError>>,
) -> BoxFuture<'static, Result<X, E>>
where
    X: DeserializeOwned + 'static,
    E: From<CallError> + 'static,
{
    let function = function.clone();
    Box::pin(async move {
        let args = args.into_iter().collect::<Result<Vec<_>, _>>()?;
        let returned = function.call(args).await?;
        // What a function returns where Rust expects nothing is ignored.
        if TypeId::of::<X>() == TypeId::of::<()>() {
            return Ok(decode::<X>(wire, wire.undefined())?);
        }
        Ok(decode::<X>(wire, returned)?)
    })
}

/// A parameter type: `Param::<P>::NEW` picks, at the call site, whether
/// it takes a function ([`FromFunction`]) or data.
pub struct Param<P>(PhantomData<fn() -> P>);

impl<P> Param<P> {
    pub const NEW: Self = Param(PhantomData);
}

/// A parameter that takes a function as an `Arc<dyn Fn ...>` or `Box<dyn Fn
/// ...>` returning `BoxFuture<'static, Result<X, E>>`.
pub trait FromFunction: Sized {
    fn from_function(function: Arc<dyn Function>, wire: &'static dyn Wire) -> Self;
}

pub trait ViaFunction<P> {
    fn __rutis_param(&self, wire: &'static dyn Wire, value: Encoded) -> Result<P, CallError>;
}

impl<P: FromFunction> ViaFunction<P> for Param<P> {
    fn __rutis_param(&self, wire: &'static dyn Wire, value: Encoded) -> Result<P, CallError> {
        Ok(P::from_function(wire.function(value)?, wire))
    }
}

pub trait ViaData<P> {
    fn __rutis_param(&self, wire: &'static dyn Wire, value: Encoded) -> Result<P, CallError>;
}

impl<P: DeserializeOwned> ViaData<P> for &Param<P> {
    fn __rutis_param(&self, wire: &'static dyn Wire, value: Encoded) -> Result<P, CallError> {
        decode(wire, value)
    }
}

macro_rules! from_function {
    ($($arg:ident),*) => {
        #[allow(non_snake_case)]
        impl<$($arg,)* X, E> FromFunction
            for Arc<dyn Fn($($arg),*) -> BoxFuture<'static, Result<X, E>> + Send + Sync>
        where
            $($arg: Serialize + 'static,)*
            X: DeserializeOwned + 'static,
            E: From<CallError> + 'static,
        {
            fn from_function(function: Arc<dyn Function>, wire: &'static dyn Wire) -> Self {
                Arc::new(move |$($arg: $arg),*| call_back(&function, wire, vec![$(encode_arg(wire, &$arg)),*]))
            }
        }

        #[allow(non_snake_case)]
        impl<$($arg,)* X, E> FromFunction
            for Box<dyn Fn($($arg),*) -> BoxFuture<'static, Result<X, E>> + Send + Sync>
        where
            $($arg: Serialize + 'static,)*
            X: DeserializeOwned + 'static,
            E: From<CallError> + 'static,
        {
            fn from_function(function: Arc<dyn Function>, wire: &'static dyn Wire) -> Self {
                Box::new(move |$($arg: $arg),*| call_back(&function, wire, vec![$(encode_arg(wire, &$arg)),*]))
            }
        }

        #[allow(non_snake_case)]
        impl<$($arg,)* X, E> FromFunction
            for Box<dyn Fn($($arg),*) -> BoxFuture<'static, Result<X, E>> + Send>
        where
            $($arg: Serialize + 'static,)*
            X: DeserializeOwned + 'static,
            E: From<CallError> + 'static,
        {
            fn from_function(function: Arc<dyn Function>, wire: &'static dyn Wire) -> Self {
                Box::new(move |$($arg: $arg),*| call_back(&function, wire, vec![$(encode_arg(wire, &$arg)),*]))
            }
        }
    };
}

from_function!();
from_function!(A0);
from_function!(A0, A1);
from_function!(A0, A1, A2);
from_function!(A0, A1, A2, A3);

/// What a method returned, split at the call site into its `Ok` and `Err`
/// (a `Result`, whatever alias names it) or taken whole.
pub struct Returned<R>(Cell<Option<R>>);

impl<R> Returned<R> {
    pub fn new(returned: R) -> Self {
        Self(Cell::new(Some(returned)))
    }
    fn take(&self) -> R {
        self.0.take().expect("a returned value is split once")
    }
}

pub trait SplitResult {
    type Ok;
    type Err;
    fn __rutis_split(&self) -> Result<Self::Ok, Self::Err>;
}

impl<T, E> SplitResult for Returned<Result<T, E>> {
    type Ok = T;
    type Err = E;
    fn __rutis_split(&self) -> Result<T, E> {
        self.take()
    }
}

pub trait SplitValue {
    type Ok;
    fn __rutis_split(&self) -> Result<Self::Ok, Infallible>;
}

impl<R> SplitValue for &Returned<R> {
    type Ok = R;
    fn __rutis_split(&self) -> Result<R, Infallible> {
        Ok(self.take())
    }
}

/// A type another language can get back from a marked method.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be returned to other languages",
    label = "not `Serialize`",
    note = "a marked method returns a `Serialize` value, a `Result` of one, or, written as `async fn` or `BoxFuture<'_, ...>`, a future of one"
)]
pub trait Returnable: Serialize + 'static {}
impl<T: Serialize + 'static> Returnable for T {}

/// Encode a method's result (`()` is undefined).
pub fn encode_ok<T: Returnable>(wire: &'static dyn Wire, value: &T) -> Result<Encoded, CallError> {
    if TypeId::of::<T>() == TypeId::of::<()>() {
        return Ok(wire.undefined());
    }
    wire.encode(value as &dyn erased_serde::Serialize)
}

// ── Errors ──────────────────────────────────────────────────────

/// `Err(e)` of a marked method: first the wire's own error, thrown as it
/// is; otherwise by [`Failure`].
pub fn forwarded<E: Any>(wire: &'static dyn Wire, error: &E) -> Option<CallError> {
    wire.forwarded(error)
}

/// An error, classified at the call site by what its type implements: the
/// most specific of the `__rutis_fail` impls below applies.
pub struct Failure<'a, E>(pub &'a E);

/// The short name of `E`: its path's last segment, without generics.
fn short_name<E: ?Sized>() -> &'static str {
    let full = std::any::type_name::<E>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}

/// The identifier `{error:?}` starts with: the variant of a derived enum
/// `Debug`, the type of a derived struct one.
fn debug_ident(debug: &str) -> String {
    debug
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Whether `a` and `b` are one identifier in two serde casings
/// (`too_big`, `TooBig`, `TOO-BIG`).
fn same_ident(a: &str, b: &str) -> bool {
    let letters = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    !a.is_empty() && letters(a) == letters(b)
}

/// Types whose `Debug` names no variant: their code is their name.
fn opaque_debug<E: ?Sized>() -> bool {
    let full = std::any::type_name::<E>();
    full.starts_with("anyhow::") || full.starts_with("alloc::boxed::Box<")
}

pub trait FailString {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError;
}
impl FailString for &&&&&Failure<'_, String> {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError {
        wire.failure("Error", "Error", self.0.clone(), None)
    }
}

pub trait FailCordis {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError;
}
impl FailCordis for &&&&Failure<'_, CordisError> {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError {
        let code = debug_ident(&format!("{:?}", self.0));
        wire.failure("CordisError", &code, self.0.to_string(), None)
    }
}

pub trait FailIo {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError;
}
impl FailIo for &&&Failure<'_, std::io::Error> {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError {
        wire.failure("Error", "Error", self.0.to_string(), None)
    }
}

pub trait FailSerde {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError;
}
impl<E: Serialize + std::fmt::Debug + std::fmt::Display> FailSerde for &&Failure<'_, E> {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError {
        let name = short_name::<E>();
        let debug = debug_ident(&format!("{:?}", self.0));
        let code = serde_code(name, &debug, capture::code(self.0));
        wire.failure(
            name,
            &code,
            self.0.to_string(),
            Some(&self.0 as &dyn erased_serde::Serialize),
        )
    }
}

/// The code of a serde error value, matching the `build.rs` generator.
fn serde_code(name: &str, debug: &str, captured: capture::Code) -> String {
    match captured {
        // An externally tagged variant.
        capture::Code::Variant(code) => code,
        // A struct (or a transparent value): its serialized text was the
        // code in the generator.
        capture::Code::Text(code) if debug == name => code,
        // An untagged newtype variant: its Rust name, as `variant_failure`
        // gives it, not the message it serialized to.
        capture::Code::Text(_) if !debug.is_empty() => debug.to_owned(),
        // An internally or adjacently tagged variant: its tag, which is
        // the variant's name as serde renames it.
        capture::Code::Tag(tag) if same_ident(&tag, debug) => tag,
        // Untagged: the variant's Rust name.
        _ if !debug.is_empty() => debug.to_owned(),
        _ => name.to_owned(),
    }
}

pub trait FailDebug {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError;
}
impl<E: std::fmt::Debug + std::fmt::Display> FailDebug for &Failure<'_, E> {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError {
        let name = short_name::<E>();
        let debug = debug_ident(&format!("{:?}", self.0));
        let code = if opaque_debug::<E>() || debug.is_empty() {
            name.to_owned()
        } else {
            debug
        };
        wire.failure(name, &code, self.0.to_string(), None)
    }
}

/// An error another language can get thrown from a marked method.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be thrown to other languages",
    label = "not `Display`",
    note = "the error of a marked method's `Result` implements `Display`; its message is that"
)]
pub trait Throwable: std::fmt::Display {}
impl<E: std::fmt::Display + ?Sized> Throwable for E {}

pub trait FailDisplay {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError;
}
impl<E: Throwable> FailDisplay for Failure<'_, E> {
    fn __rutis_fail(&self, wire: &'static dyn Wire) -> CallError {
        let name = short_name::<E>();
        wire.failure(name, name, self.0.to_string(), None)
    }
}

/// The code serde gives an error value: the variant name it passes for an
/// externally tagged variant, the text a value serializes to, or the text
/// first field of a struct (an internally or adjacently tagged variant's
/// tag).
mod capture {
    use serde::ser::{self, Serialize, Serializer};

    pub enum Code {
        Variant(String),
        Text(String),
        Tag(String),
        None,
    }

    pub fn code<T: Serialize + ?Sized>(value: &T) -> Code {
        value.serialize(Top).unwrap_or(Code::None)
    }

    #[derive(Debug)]
    pub struct Stop;
    impl std::fmt::Display for Stop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("stop")
        }
    }
    impl std::error::Error for Stop {}
    impl ser::Error for Stop {
        fn custom<M: std::fmt::Display>(_: M) -> Self {
            Stop
        }
    }

    /// The top-level value.
    struct Top;

    /// The first field of a struct or map, if it is text.
    pub struct First(Option<String>);

    /// Text, and nothing else.
    struct Text;

    macro_rules! none {
        ($($method:ident($($ty:ty),*);)*) => {
            $(fn $method(self, $(_: $ty),*) -> Result<Code, Stop> { Ok(Code::None) })*
        };
    }

    impl Serializer for Top {
        type Ok = Code;
        type Error = Stop;
        type SerializeSeq = ser::Impossible<Code, Stop>;
        type SerializeTuple = ser::Impossible<Code, Stop>;
        type SerializeTupleStruct = ser::Impossible<Code, Stop>;
        type SerializeTupleVariant = Named;
        type SerializeMap = First;
        type SerializeStruct = First;
        type SerializeStructVariant = Named;

        none! {
            serialize_bool(bool); serialize_i8(i8); serialize_i16(i16); serialize_i32(i32);
            serialize_i64(i64); serialize_u8(u8); serialize_u16(u16); serialize_u32(u32);
            serialize_u64(u64); serialize_f32(f32); serialize_f64(f64); serialize_char(char);
            serialize_bytes(&[u8]); serialize_none(); serialize_unit();
            serialize_unit_struct(&'static str);
        }

        fn serialize_str(self, value: &str) -> Result<Code, Stop> {
            Ok(Code::Text(value.to_owned()))
        }
        fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Code, Stop> {
            value.serialize(self)
        }
        fn serialize_unit_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
        ) -> Result<Code, Stop> {
            Ok(Code::Variant(variant.to_owned()))
        }
        fn serialize_newtype_struct<T: Serialize + ?Sized>(
            self,
            _: &'static str,
            value: &T,
        ) -> Result<Code, Stop> {
            value.serialize(self)
        }
        fn serialize_newtype_variant<T: Serialize + ?Sized>(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
            _: &T,
        ) -> Result<Code, Stop> {
            Ok(Code::Variant(variant.to_owned()))
        }
        fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Stop> {
            Err(Stop)
        }
        fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Stop> {
            Err(Stop)
        }
        fn serialize_tuple_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleStruct, Stop> {
            Err(Stop)
        }
        fn serialize_tuple_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
            _: usize,
        ) -> Result<Named, Stop> {
            Ok(Named(variant))
        }
        fn serialize_map(self, _: Option<usize>) -> Result<First, Stop> {
            Ok(First(None))
        }
        fn serialize_struct(self, _: &'static str, _: usize) -> Result<First, Stop> {
            Ok(First(None))
        }
        fn serialize_struct_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
            _: usize,
        ) -> Result<Named, Stop> {
            Ok(Named(variant))
        }
    }

    impl First {
        fn first<T: Serialize + ?Sized>(&mut self, value: &T) {
            if self.0.is_none() {
                self.0 = Some(value.serialize(Text).unwrap_or_default());
            }
        }
        fn code(self) -> Code {
            match self.0 {
                Some(text) if !text.is_empty() => Code::Tag(text),
                _ => Code::None,
            }
        }
    }

    impl ser::SerializeStruct for First {
        type Ok = Code;
        type Error = Stop;
        fn serialize_field<T: Serialize + ?Sized>(
            &mut self,
            _: &'static str,
            value: &T,
        ) -> Result<(), Stop> {
            self.first(value);
            Ok(())
        }
        fn end(self) -> Result<Code, Stop> {
            Ok(self.code())
        }
    }

    impl ser::SerializeMap for First {
        type Ok = Code;
        type Error = Stop;
        fn serialize_key<T: Serialize + ?Sized>(&mut self, _: &T) -> Result<(), Stop> {
            Ok(())
        }
        fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Stop> {
            self.first(value);
            Ok(())
        }
        fn end(self) -> Result<Code, Stop> {
            Ok(self.code())
        }
    }

    /// An externally tagged tuple or struct variant: its fields do not
    /// matter.
    pub struct Named(&'static str);

    impl ser::SerializeTupleVariant for Named {
        type Ok = Code;
        type Error = Stop;
        fn serialize_field<T: Serialize + ?Sized>(&mut self, _: &T) -> Result<(), Stop> {
            Ok(())
        }
        fn end(self) -> Result<Code, Stop> {
            Ok(Code::Variant(self.0.to_owned()))
        }
    }

    impl ser::SerializeStructVariant for Named {
        type Ok = Code;
        type Error = Stop;
        fn serialize_field<T: Serialize + ?Sized>(
            &mut self,
            _: &'static str,
            _: &T,
        ) -> Result<(), Stop> {
            Ok(())
        }
        fn end(self) -> Result<Code, Stop> {
            Ok(Code::Variant(self.0.to_owned()))
        }
    }

    impl Serializer for Text {
        type Ok = String;
        type Error = Stop;
        type SerializeSeq = ser::Impossible<String, Stop>;
        type SerializeTuple = ser::Impossible<String, Stop>;
        type SerializeTupleStruct = ser::Impossible<String, Stop>;
        type SerializeTupleVariant = ser::Impossible<String, Stop>;
        type SerializeMap = ser::Impossible<String, Stop>;
        type SerializeStruct = ser::Impossible<String, Stop>;
        type SerializeStructVariant = ser::Impossible<String, Stop>;

        fn serialize_str(self, value: &str) -> Result<String, Stop> {
            Ok(value.to_owned())
        }
        fn serialize_bool(self, _: bool) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_i8(self, _: i8) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_i16(self, _: i16) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_i32(self, _: i32) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_i64(self, _: i64) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_u8(self, _: u8) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_u16(self, _: u16) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_u32(self, _: u32) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_u64(self, _: u64) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_f32(self, _: f32) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_f64(self, _: f64) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_char(self, _: char) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_bytes(self, _: &[u8]) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_none(self) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_unit(self) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_unit_struct(self, _: &'static str) -> Result<String, Stop> {
            Err(Stop)
        }
        // An adjacently tagged enum writes its tag as a unit variant.
        fn serialize_unit_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
        ) -> Result<String, Stop> {
            Ok(variant.to_owned())
        }
        fn serialize_newtype_struct<T: Serialize + ?Sized>(
            self,
            _: &'static str,
            _: &T,
        ) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_newtype_variant<T: Serialize + ?Sized>(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: &T,
        ) -> Result<String, Stop> {
            Err(Stop)
        }
        fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Stop> {
            Err(Stop)
        }
        fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Stop> {
            Err(Stop)
        }
        fn serialize_tuple_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleStruct, Stop> {
            Err(Stop)
        }
        fn serialize_tuple_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleVariant, Stop> {
            Err(Stop)
        }
        fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Stop> {
            Err(Stop)
        }
        fn serialize_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeStruct, Stop> {
            Err(Stop)
        }
        fn serialize_struct_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeStructVariant, Stop> {
            Err(Stop)
        }
    }
}

/// Run `register` when the binary or library holding this is loaded: what
/// `#[rutis::service]` adds its registration with, and how a bridge installs
/// its [`Exposer`].
#[doc(hidden)]
#[macro_export]
macro_rules! __on_load {
    ($register:path) => {
        const _: () = {
            #[used]
            #[cfg_attr(
                any(
                    target_os = "linux",
                    target_os = "android",
                    target_os = "freebsd",
                    target_os = "netbsd",
                    target_os = "openbsd",
                    target_os = "dragonfly",
                    target_os = "illumos",
                    target_os = "solaris"
                ),
                unsafe(link_section = ".init_array")
            )]
            #[cfg_attr(
                target_vendor = "apple",
                unsafe(link_section = "__DATA,__mod_init_func")
            )]
            #[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
            static ON_LOAD: extern "C" fn() = {
                extern "C" fn on_load() {
                    $register()
                }
                on_load
            };
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type "rebuilt" by later images, as a dylib rebuilt without a version
    /// bump keeps its `TypeId`.
    struct Rebuilt;

    fn rebuilt_id() -> TypeId {
        TypeId::of::<Rebuilt>()
    }
    fn rebuilt_name() -> &'static str {
        "Rebuilt"
    }
    fn no_methods() -> &'static [&'static str] {
        &[]
    }
    fn no_resolve(
        _: &Ctx,
        _: &TypeKey,
        _: &Provision,
        _: Option<&Ctx>,
    ) -> Result<Arc<dyn Any + Send + Sync>, String> {
        Err("unused".into())
    }
    fn no_invoke(
        _: &Arc<dyn Any + Send + Sync>,
        _: &str,
        _: Encoded,
        _: &'static dyn Wire,
    ) -> Result<Answer, CallError> {
        Err(CallError::new("unused"))
    }
    const fn block(name: &'static str) -> Entry {
        Entry::new(
            rebuilt_id,
            rebuilt_name,
            name,
            true,
            no_methods,
            no_resolve,
            no_invoke,
        )
    }
    static FIRST: Entry = block("first");
    static SECOND_A: Entry = block("second");
    static SECOND_B: Entry = block("second");
    static OLDER: Entry = block("older");

    const fn derived(name: &'static str) -> Entry {
        Entry::new(
            rebuilt_id,
            rebuilt_name,
            name,
            false,
            no_methods,
            no_resolve,
            no_invoke,
        )
    }
    static DERIVED: Entry = derived("t");
    static GIVEN: Entry = block("tt");

    fn names(generation: Option<Generation>) -> Vec<&'static str> {
        entries_of(rebuilt_id(), generation)
            .iter()
            .map(|entry| entry.name())
            .collect()
    }

    /// Two images registering blocks for one `TypeId` (two builds of one
    /// plugin crate) are both kept and told apart by generation: each
    /// image's values are served with its own blocks. An image that
    /// registered none for the type (its build dropped the marker) never
    /// borrows another image's: only the program's own (generation 0), or
    /// nothing.
    #[test]
    fn each_images_blocks_are_its_own() {
        let ((), first) = loading(|| register(&FIRST));
        let ((), second) = loading(|| {
            register(&SECOND_A);
            register(&SECOND_B);
        });
        let ((), unmarked) = loading(|| {});
        assert_eq!(names(Some(first)), ["first"]);
        assert_eq!(names(Some(second)), ["second", "second"]);
        assert!(names(Some(unmarked)).is_empty(), "no other build's blocks");
        assert!(
            names(None).is_empty(),
            "no library's blocks for the program"
        );
        // The program's own blocks (generation 0) are the fallback.
        register(&OLDER);
        assert_eq!(names(Some(unmarked)), ["older"]);
        assert_eq!(names(None), ["older"]);
        assert_eq!(names(Some(first)), ["first"]);
    }

    #[test]
    fn one_type_in_one_generation_has_one_exported_name() {
        let ((), _) = loading(|| {
            register(&DERIVED);
            register(&GIVEN);
        });
        assert!(is_exported("tt"));
        assert!(!is_exported("t"), "the derived name is not served");
        assert_eq!(exported_as("tt"), Some("Rebuilt"));
    }

    struct Renamed;
    fn renamed_id() -> TypeId {
        TypeId::of::<Renamed>()
    }
    static OLD_NAME: Entry = Entry::new(
        renamed_id,
        rebuilt_name,
        "oldName",
        true,
        no_methods,
        no_resolve,
        no_invoke,
    );
    static NEW_NAME: Entry = Entry::new(
        renamed_id,
        rebuilt_name,
        "newName",
        true,
        no_methods,
        no_resolve,
        no_invoke,
    );

    /// Once a newer build of a plugin supersedes the older, names are
    /// looked up in the newer only: a name it renamed is no longer
    /// exported, while the older build's blocks still serve its values.
    #[test]
    fn a_name_a_newer_build_renamed_is_no_longer_exported() {
        let ((), older) = loading(|| register(&OLD_NAME));
        assert!(is_exported("oldName"));
        let ((), newer) = loading(|| register(&NEW_NAME));
        supersede([older]);
        assert!(!is_exported("oldName"));
        assert!(is_exported("newName"));
        assert_eq!(entries_of(renamed_id(), Some(older)).len(), 1);
        assert_eq!(entries_of(renamed_id(), Some(newer)).len(), 1);
    }

    #[derive(Debug, Serialize)]
    enum External {
        Missing,
        Invalid { field: String },
    }

    #[derive(Debug, Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Internal {
        TooBig { size: u64 },
    }

    #[derive(Debug, Serialize)]
    #[serde(tag = "t", content = "c")]
    enum Adjacent {
        Late(u32),
    }

    #[derive(Debug, Serialize)]
    #[serde(untagged)]
    enum Untagged {
        Text { message: String },
    }

    #[derive(Debug, Serialize)]
    #[serde(untagged)]
    enum UntaggedNewtype {
        Message(String),
    }

    #[derive(Debug, Serialize)]
    struct Plain {
        message: String,
    }

    fn code<T: Serialize + ?Sized>(value: &T) -> Option<String> {
        match capture::code(value) {
            capture::Code::Variant(code) => Some(format!("variant {code}")),
            capture::Code::Text(code) => Some(format!("text {code}")),
            capture::Code::Tag(code) => Some(format!("tag {code}")),
            capture::Code::None => None,
        }
    }

    #[test]
    fn serde_gives_the_code_of_each_tagging() {
        assert_eq!(code(&External::Missing).as_deref(), Some("variant Missing"));
        assert_eq!(
            code(&External::Invalid { field: "x".into() }).as_deref(),
            Some("variant Invalid")
        );
        assert_eq!(
            code(&Internal::TooBig { size: 1 }).as_deref(),
            Some("tag too_big")
        );
        assert_eq!(code(&Adjacent::Late(1)).as_deref(), Some("tag Late"));
        assert_eq!(code(&"plain").as_deref(), Some("text plain"));
        // Untagged and plain structs look alike to serde: the caller tells
        // them apart by their Debug.
        assert_eq!(
            code(&Untagged::Text {
                message: "m".into()
            })
            .as_deref(),
            Some("tag m")
        );
        assert_eq!(
            code(&Plain {
                message: "m".into()
            })
            .as_deref(),
            Some("tag m")
        );
        assert_eq!(code(&1u32), None);
    }

    #[test]
    fn error_codes_match_the_build_generator() {
        let untagged = UntaggedNewtype::Message("boom".into());
        assert_eq!(
            serde_code(
                "UntaggedNewtype",
                &debug_ident(&format!("{untagged:?}")),
                capture::code(&untagged)
            ),
            "Message"
        );
        assert_eq!(
            serde_code(
                "Plain",
                "Plain",
                capture::code(&Plain {
                    message: "m".into()
                })
            ),
            "Plain"
        );
        assert_eq!(
            serde_code(
                "External",
                &format!("{:?}", External::Missing),
                capture::code(&External::Missing)
            ),
            "Missing"
        );
    }

    #[test]
    fn short_names_drop_paths_and_generics() {
        assert_eq!(short_name::<External>(), "External");
        assert_eq!(short_name::<std::io::Error>(), "Error");
        assert_eq!(short_name::<Box<dyn std::error::Error>>(), "Box");
        assert!(opaque_debug::<Box<dyn std::error::Error>>());
        assert_eq!(debug_ident("Invalid { field: \"x\" }"), "Invalid");
    }
}
