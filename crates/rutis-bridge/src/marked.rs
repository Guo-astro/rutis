//! Serving `#[rutis::service]` types by name: the [`Exposer`] that rutis
//! calls when such a type is provided, and the [`Wire`] its generated
//! dispatch reaches the session's values through.
//!
//! Provided at `TypeKey::of::<T>()`, a marked type is served at
//! `host_key(name)`; provided in an instance (`TypeKey::instance::<T>(id)`),
//! at `host_key_in(name, id)`, and the instance *claims* the name: rows in
//! that instance resolve the name to it (rutis-loader reads the claims).
//! Both end with the native service's disposer.

use std::any::Any;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use rutis::host::{BoxFuture, Effect, InstanceId, TypeKey};
use rutis::host::{CtxHost, ErrorKind};
use rutis::internal::service::{
    erased_serde, exported_name, Answer, CallError, Encoded, Entry, Exposer, Exposure, Function,
    Provision, Wire,
};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Disposer, Error as RutisError};

use crate::session::{
    arg, caller_row, host_key, host_key_in, settle, Error, HostDispatch, Reference, Reply, Value,
};

struct BridgeExposer;
static EXPOSER: BridgeExposer = BridgeExposer;

/// Install the exposer. Done when the program (or library) holding the
/// bridge loads; calling it again does nothing.
pub fn install() {
    rutis::internal::service::install(&EXPOSER);
}

rutis::__on_load!(install);

/// A marked service, served by name: the methods of all its blocks. Each
/// call reads the native service through the providing context's strict
/// read path ([`Entry::resolve`]), so its read interceptors and `check()`
/// apply as they do to Rust readers.
struct Marked {
    ctx: Ctx,
    native: TypeKey,
    provision: Arc<Provision>,
    entries: Vec<&'static Entry>,
    type_name: &'static str,
}

impl Marked {
    /// The name it is served by, for messages.
    fn name(&self) -> &'static str {
        exported_name(&self.entries).unwrap_or_else(|_| short(self.type_name))
    }
}

impl HostDispatch for Marked {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.methods().contains(&method))
        else {
            let methods: Vec<&str> = self
                .entries
                .iter()
                .flat_map(|entry| entry.methods().iter().copied())
                .collect();
            return Err(Error::no_method(self.name(), method, methods));
        };
        // Read interceptors apply with the calling row as the reader, as
        // for a Rust reader in its place; with no row (a Rust or peer
        // caller), with the provider.
        let caller = caller_row();
        let actor = caller.as_ref().map(|row| row.ctx());
        let service = entry
            .resolve(&self.ctx, &self.native, &self.provision, actor)
            .map_err(|why| {
                let service = self.name();
                Error::binding(
                    crate::session::code::UNAVAILABLE,
                    // The kernel's reason names its own ids and paths: it
                    // goes in `data` only.
                    format!("service {service} is not available"),
                    Some(serde_json::json!({ "service": service, "reason": why.to_string() })),
                )
            })?;
        let name = self.name();
        match entry.invoke(&service, method, Encoded(Box::new(args)), &WIRE) {
            Ok(Answer::Ready(value)) => Ok(value_of(value)),
            Ok(Answer::Later(future)) => {
                let method = method.to_owned();
                Ok(Value::future(async move {
                    future
                        .await
                        .map(value_of)
                        .map_err(|error| error_in(error, name, &method))
                }))
            }
            Err(error) => Err(error_in(error, name, method)),
        }
    }

    fn methods(&self) -> Option<serde_json::Value> {
        let methods: Vec<&str> = self
            .entries
            .iter()
            .flat_map(|entry| entry.methods().iter().copied())
            .collect();
        Some(shapes(&methods))
    }

    fn provider(&self) -> Option<String> {
        Some(self.type_name.to_owned())
    }
}

/// The method shapes of a marked service: every method is `async` (a
/// Promise / awaitable in the caller's language).
fn shapes(methods: &[&str]) -> serde_json::Value {
    serde_json::Value::Object(
        methods
            .iter()
            .map(|method| ((*method).to_owned(), serde_json::json!("async")))
            .collect(),
    )
}

fn short(type_name: &str) -> &str {
    let base = type_name.split('<').next().unwrap_or(type_name);
    base.rsplit("::").next().unwrap_or(base)
}

/// Where a marked service is served by name: its name, the key, and the
/// context to provide it on, in the isolation scope of the native service
/// (R3: two isolated scopes each serve their own).
struct Place {
    name: &'static str,
    type_name: &'static str,
    key: TypeKey,
    serving: Ctx,
}

impl Place {
    fn of(exposure: &Exposure<'_>) -> Result<Self, RutisError> {
        let name = exported_name(&exposure.entries)?;
        let type_name = exposure
            .entries
            .first()
            .map_or("", |entry| entry.type_name());
        let key = match exposure.native.instance_id() {
            Some(instance) => host_key_in(name, instance),
            None => host_key(name),
        };
        let serving = match exposure.scope {
            Some(scope) => exposure.ctx.isolate(key.clone(), scope),
            None => exposure.ctx.clone(),
        };
        Ok(Self {
            name,
            type_name,
            key: key.erase(),
            serving,
        })
    }

    /// The refusal for a name another service has here (R6).
    fn taken(&self) -> RutisError {
        let by = self
            .serving
            .get_key::<dyn HostDispatch>(self.key.clone())
            .and_then(|other| other.provider())
            .map(|other| format!(" by {other}"))
            .unwrap_or_default();
        rutis::Error::from(ErrorKind::ServiceExists(format!(
            "`{}` ({}): the name is already provided here{by}",
            self.name, self.type_name
        )))
    }
}

impl Exposer for BridgeExposer {
    fn admit(&self, exposure: &Exposure<'_>) -> Result<(), RutisError> {
        let place = Place::of(exposure)?;
        if rutis::internal::service::vacant(&place.serving, &place.key) {
            Ok(())
        } else {
            Err(place.taken())
        }
    }

    fn expose(&self, exposure: Exposure<'_>) -> Result<Disposer, RutisError> {
        let place = Place::of(&exposure)?;
        let Exposure {
            ctx,
            native,
            entries,
            check,
            provision,
            ..
        } = exposure;
        let instance = native.instance_id();
        let Place {
            name,
            type_name,
            key,
            serving,
        } = &place;
        let (name, type_name) = (*name, *type_name);
        let marked = Arc::new(Marked {
            ctx: ctx.clone(),
            native: native.clone(),
            provision,
            entries,
            type_name,
        });
        // The native binding's `check()` gates the name as well.
        let served = match check {
            Some(check) => {
                serving.provide_checked::<dyn HostDispatch>(key.clone(), marked, move || check())
            }
            None => serving.provide_key::<dyn HostDispatch>(key.clone(), marked),
        };
        let served = served.map_err(|error| match &*error {
            ErrorKind::ServiceExists(_) => place.taken(),
            _ => error,
        })?;
        let Some(instance) = instance else {
            return Ok(served);
        };
        let name = name.to_owned();
        let claimed = ctx.effect_named(format!("rutis-service claim: {name}"), move || {
            claim(instance, &name);
            Effect::Disposer(Box::new(move || {
                unclaim(instance, &name);
                Ok(())
            }))
        });
        match claimed {
            Ok(claimed) => Ok(rutis::internal::service::chain(claimed, served)),
            Err(error) => {
                // Not claimed: not served either.
                ctx.handle().spawn(served.dispose());
                Err(error)
            }
        }
    }
}

// ── Claims ──────────────────────────────────────────────────────

/// The names marked services are served as inside each instance (each
/// with how many providers: a name provided again before its old provider
/// is gone stays claimed).
static CLAIMS: Mutex<Option<HashMap<InstanceId, HashMap<String, usize>>>> = Mutex::new(None);

/// What to tell of a new claim: each loader rebuilds the copies whose keys
/// it changes.
static LISTENERS: Mutex<Vec<Weak<dyn Fn() + Send + Sync>>> = Mutex::new(Vec::new());

fn claim(instance: InstanceId, name: &str) {
    let new = {
        let mut claims = CLAIMS.lock().unwrap();
        let count = claims
            .get_or_insert_with(HashMap::new)
            .entry(instance)
            .or_default()
            .entry(name.to_owned())
            .or_default();
        *count += 1;
        *count == 1
    };
    if new {
        let listeners: Vec<_> = {
            let mut listeners = LISTENERS.lock().unwrap();
            listeners.retain(|listener| listener.strong_count() > 0);
            listeners.iter().filter_map(Weak::upgrade).collect()
        };
        for listener in listeners {
            listener();
        }
    }
}

fn unclaim(instance: InstanceId, name: &str) {
    let mut claims = CLAIMS.lock().unwrap();
    let Some(names) = claims.as_mut().and_then(|claims| claims.get_mut(&instance)) else {
        return;
    };
    if let Some(count) = names.get_mut(name) {
        *count -= 1;
        if *count == 0 {
            names.remove(name);
        }
    }
    if names.is_empty() {
        claims.as_mut().unwrap().remove(&instance);
    }
}

/// Whether a marked service is served as `name` inside `instance`.
pub fn claimed_in(instance: InstanceId, name: &str) -> bool {
    CLAIMS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|claims| claims.get(&instance))
        .is_some_and(|names| names.contains_key(name))
}

/// The names marked services are served as inside `instance`.
pub fn claims_in(instance: InstanceId) -> BTreeSet<String> {
    CLAIMS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|claims| claims.get(&instance))
        .map(|names| names.keys().cloned().collect())
        .unwrap_or_default()
}

/// Call `listener` whenever an instance claims a name it did not have; it
/// is called until it is dropped.
pub fn on_claim(listener: &Arc<dyn Fn() + Send + Sync>) {
    LISTENERS.lock().unwrap().push(Arc::downgrade(listener));
}

// ── Diagnostics ─────────────────────────────────────────────────

/// Why `name` may be missing: a Rust service visible from `ctx` whose type
/// would be exported as `name` but is not marked. For a row that waits for
/// `name`.
pub fn unexported(ctx: &Ctx, name: &str) -> Option<String> {
    let diagnostics = ctx.diagnostics();
    diagnostics.bindings.iter().find_map(|binding| {
        let key = &binding.key;
        if key.name().is_some() {
            return None;
        }
        let type_name = key.type_name();
        if type_name.starts_with("dyn ") || type_name.contains('<') {
            return None;
        }
        let short = short(type_name);
        if default_name(short) != name {
            return None;
        }
        if rutis::internal::service::is_marked(key.type_id()) {
            if let Some(exported) = rutis::internal::service::name_of(key.type_id())
                .filter(|exported| *exported != name)
            {
                return Some(format!(
                    "`{name}` is the Rust service {type_name}, which is exported as `{exported}`"
                ));
            }
            // Visible here when `ctx` reads the key under the binding's
            // isolation label and, for an instance key, inside that instance.
            let visible = ctx.in_instance_key(key)
                && ctx.isolate_label(key).as_deref() == binding.scope.as_deref();
            if visible {
                return Some(format!(
                    "`{name}` is the Rust service {type_name}, provided here but not served"
                ));
            }
            let place = match (key.instance_id(), binding.scope.as_deref()) {
                (Some(instance), _) => format!("instance {instance}"),
                (None, Some(scope)) => format!("scope `{scope}`"),
                (None, None) => "the default scope".to_owned(),
            };
            return Some(format!(
                "`{name}` is the Rust service {type_name}, provided in {place}, not visible here"
            ));
        }
        Some(format!(
            "`{name}` is the Rust service {type_name}, which is not exported: add #[rutis::service] to its impl"
        ))
    })
}

/// As `#[rutis::service]` names a type by default.
fn default_name(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let upper = chars.iter().take_while(|c| c.is_uppercase()).count();
    let lower = if upper > 1 && upper < chars.len() && chars[upper].is_lowercase() {
        upper - 1
    } else {
        upper.max(1).min(chars.len())
    };
    chars
        .iter()
        .enumerate()
        .flat_map(|(index, c)| {
            if index < lower {
                c.to_lowercase().collect::<Vec<_>>()
            } else {
                vec![*c]
            }
        })
        .collect()
}

// ── The wire ────────────────────────────────────────────────────

/// The `count` arguments of a call, those not passed being undefined.
fn arguments(args: Value, count: usize) -> Result<std::vec::IntoIter<Value>, CallError> {
    let mut args = match args {
        Value::Undefined => Vec::new(),
        args => args.list().map_err(call_error)?,
    };
    if args.len() > count {
        return Err(BadArgument::TooMany {
            expected: count,
            received: args.len(),
        }
        .into());
    }
    args.resize(count, Value::Undefined);
    Ok(args.into_iter())
}

/// An argument of a call a marked service's method cannot take. The
/// call (`service.method`) is named where the error leaves the service
/// ([`error_in`]).
#[derive(Debug, Clone, PartialEq)]
enum BadArgument {
    TooMany {
        expected: usize,
        received: usize,
    },
    /// Argument `argument` (from 1) not passed.
    Missing {
        argument: usize,
        expected: String,
    },
    /// Argument `argument` (from 1) of the wrong type or value.
    Wrong {
        argument: usize,
        expected: String,
        actual: String,
    },
    /// Argument `argument` (from 1) not valid for another reason, said
    /// in `why`.
    Invalid {
        argument: usize,
        why: String,
    },
}

impl BadArgument {
    /// The text after `service.method` (`: argument 1 should be …`, or
    /// ` takes 1 argument, got 2`).
    fn text(&self) -> String {
        match self {
            Self::TooMany { expected, received } => {
                let plural = if *expected == 1 { "" } else { "s" };
                format!(" takes {expected} argument{plural}, got {received}")
            }
            Self::Missing { argument, .. } => format!(": argument {argument} is missing"),
            Self::Wrong {
                argument,
                expected,
                actual,
            } => format!(": argument {argument} should be {expected}, got {actual}"),
            Self::Invalid { argument, why } => format!(": argument {argument} is not valid: {why}"),
        }
    }

    /// The error of a call to `service.method` (code `bad-argument`).
    fn error(&self, service: &str, method: &str) -> Error {
        let mut data = serde_json::json!({ "service": service, "method": method });
        match self {
            Self::TooMany { expected, received } => {
                data["expected"] = (*expected).into();
                data["received"] = (*received).into();
            }
            Self::Missing { argument, expected }
            | Self::Wrong {
                argument, expected, ..
            } => {
                data["argument"] = (*argument).into();
                data["expected"] = expected.clone().into();
            }
            Self::Invalid { argument, .. } => data["argument"] = (*argument).into(),
        }
        Error::binding(
            crate::session::code::BAD_ARGUMENT,
            format!("{service}.{method}{}", self.text()),
            Some(data),
        )
    }

    /// What decoding argument `argument` (`value`) said (`why`), in the
    /// caller's words: `Missing` for an argument not passed, `Wrong` for a
    /// value of another type, saying what it should be and what it is.
    fn of_decoding(argument: usize, value: &Value, why: &str) -> Self {
        let expected = why
            .rsplit_once(", expected ")
            .map(|(_, expected)| expected_in_words(expected));
        if let (Value::Undefined, Some(expected)) = (value, expected.clone()) {
            return Self::Missing { argument, expected };
        }
        let unexpected = why
            .strip_prefix("invalid type: ")
            .or_else(|| why.strip_prefix("invalid value: "))
            .and_then(|rest| rest.rsplit_once(", expected "))
            .map(|(actual, _)| actual);
        match (expected, unexpected) {
            (Some(expected), Some(actual)) => Self::Wrong {
                argument,
                expected,
                actual: tagged_in_words(value).unwrap_or_else(|| actual_in_words(actual)),
            },
            _ => Self::Invalid {
                argument,
                why: why.to_owned(),
            },
        }
    }
}

impl From<BadArgument> for CallError {
    fn from(bad: BadArgument) -> Self {
        CallError::with_raw(
            bad.text().trim_start_matches(": ").trim().to_owned(),
            Box::new(bad),
        )
    }
}

/// What a Rust type expects, as a developer of any language reads it.
fn expected_in_words(expected: &str) -> String {
    let expected = expected.trim();
    match expected {
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" => "an integer".into(),
        "f32" | "f64" => "a number".into(),
        "a borrowed string" | "a string" | "a char" => "a string".into(),
        "a boolean" => "a boolean".into(),
        "a sequence" => "a list".into(),
        "a map" => "an object".into(),
        "unit" => "nothing".into(),
        _ if expected.starts_with("struct ") || expected.starts_with("a map") => "an object".into(),
        _ if expected.starts_with("a tuple") || expected.starts_with("tuple") => "a list".into(),
        _ if expected.starts_with("enum ") => "one of its values".into(),
        other => other.trim_matches('`').to_owned(),
    }
}

/// A value as serde names it (`string "x"`, `integer `5``, `map`), shown
/// short: `"x"`, `5`, `an object`.
fn actual_in_words(actual: &str) -> String {
    let actual = actual.trim();
    if let Some(text) = actual.strip_prefix("string ") {
        return text.to_owned();
    }
    if let (Some(start), Some(end)) = (actual.find('`'), actual.rfind('`')) {
        if start < end {
            return actual[start + 1..end].to_owned();
        }
    }
    match actual {
        "map" => "an object".into(),
        "sequence" => "a list".into(),
        "null" | "unit value" | "unit" | "Option value" => "null".into(),
        other => other.to_owned(),
    }
}

/// A value of a kind not every language has, named (`a bigint`, `bytes`).
fn tagged_in_words(value: &Value) -> Option<String> {
    Some(
        match value {
            Value::BigInt(_) => "a bigint",
            Value::Bytes(_) => "bytes",
            Value::Instant(_) => "a point in time",
            Value::DateTime(_) => "a date and time",
            Value::Date(_) => "a date",
            Value::Time(_) => "a time of day",
            Value::Decimal(_) => "a decimal",
            Value::Uuid(_) => "a UUID",
            Value::Map(_) => "a map",
            Value::Set(_) => "a set",
            Value::Reference(reference) if reference.is_function() => "a function",
            Value::Reference(_) => "an object reference",
            _ => return None,
        }
        .to_owned(),
    )
}

/// An argument of a call, as [`BridgeWire::arguments`] hands it out: its
/// position (from 1) kept for messages.
struct Argument {
    position: usize,
    value: Value,
}

/// The plugin definition passed as a parameter (`rutis::Definition`): the
/// caller is read now, while the call starts; its declarations are read
/// from its runtime when the future runs, before the method is called.
fn definition(value: Value) -> BoxFuture<'static, Result<rutis::Definition, Error>> {
    let (row, process) = (caller_row(), crate::runtime::calling_process());
    Box::pin(async move {
        let process = process.ok_or_else(|| {
            Error::refused(
                "a plugin definition can only come from a language runtime's call".into(),
            )
        })?;
        let Value::Reference(reference) = value else {
            return Err(Error::Value(
                "expected a plugin definition, received data".into(),
            ));
        };
        crate::runtime::read_definition(process, reference, row).await
    })
}

/// How the text for a plugin that does not declare `name` ends.
fn undeclared_tail(name: &str) -> String {
    format!(": add '{name}' to its inject")
}

struct BridgeWire;
static WIRE: BridgeWire = BridgeWire;

fn value_of(encoded: Encoded) -> Value {
    argument_of(encoded).1
}

/// A value and, for an argument of the call, its position (from 1).
fn argument_of(encoded: Encoded) -> (Option<usize>, Value) {
    match encoded.0.downcast::<Value>() {
        Ok(value) => (None, *value),
        Err(other) => match other.downcast::<Argument>() {
            Ok(argument) => (Some(argument.position), argument.value),
            Err(_) => (None, Value::Undefined),
        },
    }
}

fn error_of(error: CallError) -> Error {
    let (message, raw) = error.into_raw();
    match raw.map(|raw| raw.downcast::<Error>()) {
        Some(Ok(error)) => *error,
        Some(Err(raw)) => match raw.downcast::<BadArgument>() {
            // Outside a call to a named service (a function passed back).
            Ok(bad) => Error::binding(
                crate::session::code::BAD_ARGUMENT,
                bad.text().trim_start_matches(": ").trim().to_owned(),
                None,
            ),
            Err(_) => Error::Value(message),
        },
        None => Error::Value(message),
    }
}

/// [`error_of`] for a call to `service.method`: an argument it cannot
/// take named as one of that call.
fn error_in(error: CallError, service: &str, method: &str) -> Error {
    match error
        .raw()
        .and_then(|raw| raw.downcast_ref::<BadArgument>())
    {
        Some(bad) => bad.error(service, method),
        None => error_of(error),
    }
}

fn call_error(error: Error) -> CallError {
    CallError::with_raw(error.to_string(), Box::new(error))
}

impl From<CallError> for Error {
    fn from(error: CallError) -> Self {
        error_of(error)
    }
}

/// A function the caller passed.
struct Passed(Reference);

impl Function for Passed {
    fn call(&self, args: Vec<Encoded>) -> BoxFuture<'static, Result<Encoded, CallError>> {
        let reference = self.0.clone();
        Box::pin(async move {
            let args = args.into_iter().map(value_of).collect();
            let returned = reference
                .call_async(Value::List(args))
                .await
                .map_err(call_error)?;
            let settled = settle(returned).await.map_err(call_error)?;
            Ok(Encoded(Box::new(settled)))
        })
    }
}

impl Wire for BridgeWire {
    fn arguments(&self, args: Encoded, count: usize) -> Result<Vec<Encoded>, CallError> {
        let args = arguments(value_of(args), count)?;
        Ok(args
            .enumerate()
            .map(|(index, value)| {
                Encoded(Box::new(Argument {
                    position: index + 1,
                    value,
                }))
            })
            .collect())
    }

    fn decode(
        &self,
        value: Encoded,
        into: &mut dyn FnMut(
            &mut dyn erased_serde::Deserializer<'static>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), CallError> {
        let (position, value) = argument_of(value);
        let Some(position) = position else {
            return crate::session::decode_data_with(value, |json| {
                into(&mut <dyn erased_serde::Deserializer>::erase(json))
                    .map_err(|error| error.to_string())
            })
            .map_err(call_error);
        };
        let shown = value.clone();
        crate::session::decode_data_with(value, |json| {
            into(&mut <dyn erased_serde::Deserializer>::erase(json))
                .map_err(|error| error.to_string())
        })
        .map_err(|error| match error {
            Error::Value(why) => BadArgument::of_decoding(position, &shown, &why).into(),
            error => call_error(error),
        })
    }

    fn encode(&self, value: &dyn erased_serde::Serialize) -> Result<Encoded, CallError> {
        let value = arg(value).map_err(call_error)?;
        Ok(Encoded(Box::new(value)))
    }

    fn undefined(&self) -> Encoded {
        Encoded(Box::new(Value::Undefined))
    }

    fn function(&self, value: Encoded) -> Result<Arc<dyn Function>, CallError> {
        match value_of(value) {
            Value::Reference(reference) if reference.is_function() => {
                Ok(Arc::new(Passed(reference)))
            }
            _ => Err(call_error(Error::Value("expected a function".into()))),
        }
    }

    fn definition(
        &self,
        value: Encoded,
    ) -> BoxFuture<'static, Result<rutis::Definition, CallError>> {
        // Who calls is read now, while the call starts.
        let reading = definition(value_of(value));
        Box::pin(async move { reading.await.map_err(call_error) })
    }

    fn caller(&self) -> Option<Ctx> {
        caller_row().map(|row| row.ctx().clone())
    }

    fn no_caller(&self, method: &str) -> String {
        match crate::session::unleased() {
            // A plugin using the service it does not declare (#676).
            Some(name) => format!(
                "{method} needs the calling row's context, which a call carries only when the \
                 calling plugin declares {name} in its inject{}",
                undeclared_tail(&name)
            ),
            None => format!(
                "{method} needs the calling row's context: only a row of a language runtime can call it"
            ),
        }
    }

    fn error(&self, message: String) -> CallError {
        // The call of a plugin that does not declare the service it uses
        // (`no_caller`): `not-declared`.
        if let Some(name) = crate::session::unleased() {
            if message.ends_with(&undeclared_tail(&name)) {
                return call_error(Error::binding(
                    crate::session::code::NOT_DECLARED,
                    message,
                    Some(serde_json::json!({ "service": name })),
                ));
            }
        }
        call_error(Error::Value(message))
    }

    fn failure(
        &self,
        name: &str,
        code: &str,
        message: String,
        data: Option<&dyn erased_serde::Serialize>,
    ) -> CallError {
        let data = data.and_then(|data| serde_json::to_value(data).ok());
        call_error(crate::exports::failure(name, code, message, data))
    }

    fn located(&self, error: CallError, method: &str, file: &str, line: u32) -> CallError {
        let (message, raw) = error.into_raw();
        match raw.map(|raw| raw.downcast::<Error>()) {
            Some(Ok(thrown)) => call_error(crate::exports::located(*thrown, method, file, line)),
            Some(Err(raw)) => CallError::with_raw(message, raw),
            None => CallError::new(message),
        }
    }

    fn rethrown(&self, error: CallError, method: &str, file: &str, line: u32) -> CallError {
        let (message, raw) = error.into_raw();
        match raw.map(|raw| raw.downcast::<Error>()) {
            Some(Ok(thrown)) => call_error(crate::exports::rethrown(*thrown, method, file, line)),
            Some(Err(raw)) => CallError::with_raw(message, raw),
            None => CallError::new(message),
        }
    }

    fn forwarded(&self, error: &dyn Any) -> Option<CallError> {
        error.downcast_ref::<Error>().cloned().map(call_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_not_passed_are_undefined() {
        let args = arguments(Value::List(vec![serde_json::json!(1).into()]), 2).unwrap();
        assert_eq!(
            args.collect::<Vec<_>>(),
            [serde_json::json!(1).into(), Value::Undefined]
        );
        let too_many = arguments(Value::List(vec![Value::Undefined; 3]), 2).unwrap_err();
        assert_eq!(
            error_in(too_many, "adder", "add").to_string(),
            "adder.add takes 2 arguments, got 3"
        );
        let one = arguments(Value::List(vec![Value::Undefined; 2]), 1).unwrap_err();
        assert_eq!(
            error_in(one, "adder", "add").to_string(),
            "adder.add takes 1 argument, got 2"
        );
    }

    #[test]
    fn names_follow_the_macro() {
        assert_eq!(default_name("Calendar"), "calendar");
        assert_eq!(default_name("HTTPClient"), "httpClient");
        assert_eq!(default_name("IO"), "io");
    }

    pub struct Guarded;

    #[rutis::service]
    impl Guarded {
        pub fn ping(&self) -> u32 {
            1
        }
    }

    pub struct Adder;

    #[rutis::service]
    impl Adder {
        pub fn add(&self, a: u32, label: String) -> String {
            format!("{label}{a}")
        }
    }

    /// A call that is wrong says which service, method and argument, in
    /// words of any language, with code `bad-argument` / `no-method`
    /// (#677).
    #[tokio::test]
    async fn wrong_calls_name_the_service_method_and_argument() {
        let root = Ctx::root().unwrap();
        root.provide(Adder).unwrap();
        let service = root.get_at::<dyn HostDispatch>(&host_key("adder")).unwrap();
        let call = |method: &str, args: Vec<Value>| service.invoke(method, Value::List(args));
        let refused = |reply: Reply| async move {
            match reply {
                Err(error) => error,
                Ok(value) => settle(value).await.unwrap_err(),
            }
        };
        let cases: Vec<(&str, Vec<Value>, &str, &str)> = vec![
            (
                "add",
                vec![serde_json::json!("x").into(), serde_json::json!("n").into()],
                "adder.add: argument 1 should be an integer, got \"x\"",
                "bad-argument",
            ),
            (
                "add",
                vec![Value::Bytes(vec![1]), serde_json::json!("n").into()],
                "adder.add: argument 1 should be an integer, got bytes",
                "bad-argument",
            ),
            (
                "add",
                vec![serde_json::json!(1).into()],
                "adder.add: argument 2 is missing",
                "bad-argument",
            ),
            (
                "add",
                vec![serde_json::json!(1).into(), serde_json::json!(2).into()],
                "adder.add: argument 2 should be a string, got 2",
                "bad-argument",
            ),
            (
                "add",
                vec![Value::Undefined; 3],
                "adder.add takes 2 arguments, got 3",
                "bad-argument",
            ),
            (
                "ad",
                vec![],
                "adder has no method ad (methods: add)",
                "no-method",
            ),
        ];
        for (method, args, text, code) in cases {
            let error = refused(call(method, args)).await;
            assert_eq!(error.to_string(), text);
            assert_eq!(error.code(), Some(code), "{text}");
        }
        let error = refused(call("add", vec![serde_json::json!(1).into()])).await;
        assert_eq!(
            error.data(),
            Some(
                serde_json::json!({ "service": "adder", "method": "add", "argument": 2, "expected": "a string" })
            )
        );
        root.shutdown().await.unwrap();
    }

    /// A plugin that hands out its context, and runs `then` on it first.
    struct Probe<F>(std::sync::mpsc::Sender<Ctx>, F);

    impl<F: Fn(&Ctx) + Send + Sync + 'static> rutis::host::DynPlugin for Probe<F> {
        fn name(&self) -> &str {
            "probe"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
            (self.1)(ctx);
            self.0.send(ctx.clone()).unwrap();
            Box::pin(async { Ok(Effect::Done) })
        }
    }

    async fn child(parent: &Ctx, then: impl Fn(&Ctx) + Send + Sync + 'static) -> Ctx {
        let (tx, rx) = std::sync::mpsc::channel();
        (&parent.plugin(Probe(tx, then))).await.unwrap();
        rx.recv().unwrap()
    }

    fn call_as(service: &Arc<dyn HostDispatch>, caller: Option<&Ctx>) -> Result<(), Error> {
        let row = caller.map(|ctx| crate::session::CallerRow::new("row", "row", ctx.clone()));
        crate::session::with_caller_row(row, || service.invoke("ping", Value::List(Vec::new())))
            .map(|_| ())
    }

    /// Read interceptors of a call by name are those a Rust reader in the
    /// calling row's place gets: the ones of the row's ancestors, not of
    /// the provider's.
    #[tokio::test]
    async fn read_interceptors_are_the_calling_rows() {
        use rutis::internal::ServiceIntercept;
        let root = Ctx::root().unwrap();
        root.provide(Guarded).unwrap();
        let service = root
            .get_at::<dyn HostDispatch>(&host_key("guarded"))
            .unwrap();
        // A plugin below the root denies reads of `Guarded` in its subtree.
        let guard = child(&root, |ctx| {
            ctx.intercept_require_as::<Guarded>(TypeKey::of::<Guarded>(), |_| {
                ServiceIntercept::Deny
            })
            .unwrap();
        })
        .await;
        let inside = child(&guard, |_| {}).await;
        let outside = child(&root, |_| {}).await;
        assert!(
            call_as(&service, Some(&inside)).is_err(),
            "denied in the guard's subtree"
        );
        assert!(call_as(&service, Some(&outside)).is_ok());
        assert!(
            call_as(&service, None).is_ok(),
            "no row: the provider reads"
        );
        root.shutdown().await.unwrap();
    }

    /// A reference to the entry kept after its service went serves nothing,
    /// even once the slot is provided again (by another build, say): it
    /// never runs on a value it was not provided with.
    #[tokio::test]
    async fn a_kept_entry_serves_only_the_binding_it_was_provided_with() {
        let root = Ctx::root().unwrap();
        let first = root.provide(Guarded).unwrap();
        let kept = root
            .get_at::<dyn HostDispatch>(&host_key("guarded"))
            .unwrap();
        assert!(call_as(&kept, None).is_ok());
        first.dispose().await.unwrap();
        root.provide(Guarded).unwrap();
        let error = call_as(&kept, None).unwrap_err();
        assert!(error.to_string().contains("not available"), "{error}");
        let current = root
            .get_at::<dyn HostDispatch>(&host_key("guarded"))
            .unwrap();
        assert!(call_as(&current, None).is_ok());
        root.shutdown().await.unwrap();
    }
}
