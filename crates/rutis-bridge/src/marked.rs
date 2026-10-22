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

use rutis::service::{
    erased_serde, exported_name, Answer, CallError, Encoded, Entry, Exposer, Exposure, Function,
    Provision, Wire,
};
use rutis::{BoxFuture, CordisError, Ctx, Disposer, Effect, InstanceId, TypeKey};

use crate::session::{
    arg, caller_row, host_key, host_key_in, settle, Error, HostDispatch, Reference, Reply, Value,
};

struct BridgeExposer;
static EXPOSER: BridgeExposer = BridgeExposer;

/// Install the exposer. Done when the program (or library) holding the
/// bridge loads; calling it again does nothing.
pub fn install() {
    rutis::service::install(&EXPOSER);
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

impl HostDispatch for Marked {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.methods().contains(&method))
        else {
            return Err(Error::Value(format!(
                "{} has no method {method}",
                short(self.type_name)
            )));
        };
        // Read interceptors apply with the calling row as the reader, as
        // for a Rust reader in its place; with no row (a Rust or peer
        // caller), with the provider.
        let caller = caller_row();
        let actor = caller.as_ref().map(|row| row.ctx());
        let service = entry
            .resolve(&self.ctx, &self.native, &self.provision, actor)
            .map_err(|why| {
                Error::Value(format!("{} is not available: {why}", short(self.type_name)))
            })?;
        match entry.invoke(&service, method, Encoded(Box::new(args)), &WIRE) {
            Ok(Answer::Ready(value)) => Ok(value_of(value)),
            Ok(Answer::Later(future)) => Ok(Value::future(async move {
                future.await.map(value_of).map_err(error_of)
            })),
            Err(error) => Err(error_of(error)),
        }
    }

    fn methods(&self) -> Option<serde_json::Value> {
        let methods: Vec<&str> = self
            .entries
            .iter()
            .flat_map(|entry| entry.methods().iter().copied())
            .collect();
        Some(crate::exports::shapes(&methods))
    }

    fn provider(&self) -> Option<String> {
        Some(self.type_name.to_owned())
    }
}

fn short(type_name: &str) -> &str {
    let base = type_name.split('<').next().unwrap_or(type_name);
    base.rsplit("::").next().unwrap_or(base)
}

impl Exposer for BridgeExposer {
    fn expose(&self, exposure: Exposure<'_>) -> Result<Disposer, CordisError> {
        let Exposure {
            ctx,
            native,
            scope,
            entries,
            check,
            provision,
            ..
        } = exposure;
        let name = exported_name(&entries)?;
        let type_name = entries.first().map_or("", |entry| entry.type_name());
        let instance = native.instance_id();
        let key = match instance {
            Some(instance) => host_key_in(name, instance),
            None => host_key(name),
        };
        let marked = Arc::new(Marked {
            ctx: ctx.clone(),
            native: native.clone(),
            provision,
            entries,
            type_name,
        });
        // In the isolation scope of the native service (R3): two isolated
        // scopes each serve their own.
        let serving = match scope {
            Some(scope) => ctx.isolate(key.clone(), scope),
            None => ctx.clone(),
        };
        // The native binding's `check()` gates the name as well.
        let served = match check {
            Some(check) => {
                serving
                    .provide_as_with_check::<dyn HostDispatch>(key.clone(), marked, move || check())
            }
            None => serving.provide_as::<dyn HostDispatch>(key.clone(), marked),
        };
        let served = served.map_err(|error| match error {
            CordisError::ServiceExists(_) => {
                let by = serving
                    .get_as::<dyn HostDispatch>(key.clone())
                    .and_then(|other| other.provider())
                    .map(|other| format!(" by {other}"))
                    .unwrap_or_default();
                CordisError::ServiceExists(format!(
                    "`{name}` ({type_name}): the name is already provided here{by}"
                ))
            }
            error => error,
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
            Ok(claimed) => Ok(rutis::service::chain(claimed, served)),
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
        if rutis::service::is_marked(key.type_id()) {
            if let Some(exported) = rutis::service::name_of(key.type_id())
                .filter(|exported| *exported != name)
            {
                return Some(format!(
                    "`{name}` is the Rust service {type_name}, which is exported as `{exported}`"
                ));
            }
            let here = ctx.scope();
            let visible = match (key.instance_id(), binding.scope.as_deref()) {
                (Some(instance), _) => here.iter().any(|scope| scope.id == instance.to_string()),
                (None, Some(group)) => {
                    ctx.isolate_label(key).as_deref() == Some(group)
                        || here
                            .iter()
                            .any(|scope| scope.group == group || scope.id == group)
                }
                (None, None) => here.is_empty(),
            };
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

struct BridgeWire;
static WIRE: BridgeWire = BridgeWire;

fn value_of(encoded: Encoded) -> Value {
    match encoded.0.downcast::<Value>() {
        Ok(value) => *value,
        Err(_) => Value::Undefined,
    }
}

fn error_of(error: CallError) -> Error {
    let (message, raw) = error.into_raw();
    match raw.map(|raw| raw.downcast::<Error>()) {
        Some(Ok(error)) => *error,
        _ => Error::Value(message),
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
        let args = crate::exports::arguments(value_of(args), count).map_err(call_error)?;
        Ok(args.map(|arg| Encoded(Box::new(arg))).collect())
    }

    fn decode(
        &self,
        value: Encoded,
        into: &mut dyn FnMut(
            &mut dyn erased_serde::Deserializer<'static>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), CallError> {
        crate::session::decode_data_with(value_of(value), |json| {
            into(&mut <dyn erased_serde::Deserializer>::erase(json))
                .map_err(|error| error.to_string())
        })
        .map_err(call_error)
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

    fn caller(&self) -> Option<Ctx> {
        caller_row().map(|row| row.ctx().clone())
    }

    fn error(&self, message: String) -> CallError {
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

    fn forwarded(&self, error: &dyn Any) -> Option<CallError> {
        error.downcast_ref::<Error>().cloned().map(call_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A plugin that hands out its context, and runs `then` on it first.
    struct Probe<F>(std::sync::mpsc::Sender<Ctx>, F);

    impl<F: Fn(&Ctx) + Send + Sync + 'static> rutis::Plugin for Probe<F> {
        fn name(&self) -> &str {
            "probe"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
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
        use rutis::ServiceIntercept;
        let root = Ctx::root().unwrap();
        root.provide(Guarded).unwrap();
        let service = root
            .get_as::<dyn HostDispatch>(host_key("guarded"))
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
            .get_as::<dyn HostDispatch>(host_key("guarded"))
            .unwrap();
        assert!(call_as(&kept, None).is_ok());
        first.dispose().await.unwrap();
        root.provide(Guarded).unwrap();
        let error = call_as(&kept, None).unwrap_err();
        assert!(error.to_string().contains("not available"), "{error}");
        let current = root
            .get_as::<dyn HostDispatch>(host_key("guarded"))
            .unwrap();
        assert!(call_as(&current, None).is_ok());
        root.shutdown().await.unwrap();
    }
}
