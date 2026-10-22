//! Existing Rust services used by name from the plugins of other languages,
//! without a hand-written [`HostDispatch`].
//!
//! The application's `build.rs` lists the types to export
//! ([`build::Exports`](crate::build::Exports), feature
//! `cordis` of the build dependency); the generator reads their source,
//! including from dependency crates, and generates a dispatcher for each
//! and TypeScript declarations. [`include_exports!`](crate::include_exports)
//! declares the generated module, whose `all()` lists one [`Export`] per
//! type:
//!
//! ```ignore
//! rutis_bridge::include_exports!();
//!
//! let mut catalog = rutis_loader::ServiceCatalog::new();
//! catalog.export_all(exports::all());
//! ```
//!
//! An export is a plugin ([`Export::plugin`]) that waits for the native
//! service (`TypeKey::of::<T>()`, or that key in an instance) and provides
//! its dispatcher at `host_key(name)` (`host_key_in(name, instance)`), so
//! rows of every language gate on it, call its methods, and stop when it
//! goes. The rest of this module is what the generated code calls.

use std::sync::Arc;

use rutis::{BoxFuture, CordisError, Ctx, Effect, InstanceId, Plugin, TypeKey};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

use crate::session::{host_key, Error, HostDispatch, Reference, Reply, Value};

#[doc(hidden)]
pub use crate::marked::{claimed_in, claims_in, install, on_claim, unexported};

type Wrap = dyn Fn(&Ctx, &TypeKey) -> Result<Arc<dyn HostDispatch>, CordisError> + Send + Sync;

/// A Rust service exported by name: the native service of type `T`, and the
/// generated dispatcher that serves it to other languages.
#[derive(Clone)]
pub struct Export {
    name: String,
    group: Option<String>,
    native: TypeKey,
    wrap: Arc<Wrap>,
}

impl std::fmt::Debug for Export {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Export")
            .field("name", &self.name)
            .field("group", &self.group)
            .field("native", &self.native.describe())
            .finish_non_exhaustive()
    }
}

impl Export {
    /// Export the service `T`, provided at `TypeKey::of::<T>()`, as `name`,
    /// served by `dispatch` (the generated dispatcher).
    pub fn new<T: Send + Sync + 'static>(
        name: impl Into<String>,
        dispatch: fn(Arc<T>) -> Arc<dyn HostDispatch>,
    ) -> Self {
        Self {
            name: name.into(),
            group: None,
            native: TypeKey::of::<T>(),
            wrap: Arc::new(move |ctx: &Ctx, key: &TypeKey| {
                Ok(dispatch(ctx.require_as::<T>(key.clone())?))
            }),
        }
    }

    /// Export it inside each instance of the instanced group `group`
    /// instead: the native service is at `TypeKey::of::<T>()` with the
    /// instance's id, the dispatcher at `host_key_in(name, instance)`, and
    /// a row resolves the name to its nearest enclosing `group` instance, as
    /// a name registered with `register_shared_instance` does.
    pub fn in_instances(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The instanced group the export is inside, if any.
    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    /// The plugin serving the export: outside instances (`None`), or inside
    /// the instance `instance` of its group. It waits for the native
    /// service and provides the dispatcher while that is there.
    /// rutis-loader mounts these itself for the exports in its catalog.
    pub fn plugin(&self, instance: Option<InstanceId>) -> ExportPlugin {
        let (native, provided) = match instance {
            Some(instance) => (
                self.native.clone().with_instance(instance),
                host_key(&self.name).with_instance(instance),
            ),
            None => (self.native.clone(), host_key(&self.name)),
        };
        ExportPlugin {
            label: format!("rutis-export:{}", self.name),
            injects: vec![native],
            provided,
            wrap: self.wrap.clone(),
        }
    }
}

/// See [`Export::plugin`].
pub struct ExportPlugin {
    label: String,
    injects: Vec<TypeKey>,
    provided: TypeKey,
    wrap: Arc<Wrap>,
}

impl Plugin for ExportPlugin {
    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let dispatch = (self.wrap)(ctx, &self.injects[0])?;
            ctx.provide_as::<dyn HostDispatch>(self.provided.clone(), dispatch)?;
            Ok(Effect::Done)
        })
    }
}

// ── What the generated dispatchers call ─────────────────────────

#[doc(hidden)]
pub use serde_json::Value as Json;

/// `e`'s serde value, for an error's `data`.
#[doc(hidden)]
pub fn data<E: Serialize + ?Sized>(error: &E) -> Option<Json> {
    serde_json::to_value(error).ok()
}

/// The method shapes of a generated dispatcher: every method is `async`
/// (a Promise / awaitable in the caller's language).
#[doc(hidden)]
pub fn shapes(methods: &[&str]) -> Json {
    Json::Object(
        methods
            .iter()
            .map(|method| ((*method).to_owned(), json!("async")))
            .collect(),
    )
}

/// The context of the row calling `method` ([`crate::session::caller_row`]),
/// for a method taking `&Ctx`: what it registers there goes when that row
/// unloads. A call from outside any row (Rust, a peer node) is refused.
#[doc(hidden)]
pub fn caller_ctx(method: &str) -> Result<Ctx, Error> {
    crate::session::caller_row()
        .map(|row| row.ctx().clone())
        .ok_or_else(|| {
            Error::Value(format!(
                "{method} needs the calling row's context: only a row of a language runtime can call it"
            ))
        })
}

/// The `count` arguments of a call, those not passed being undefined.
#[doc(hidden)]
pub fn arguments(args: Value, count: usize) -> Result<std::vec::IntoIter<Value>, Error> {
    let mut args = match args {
        Value::Undefined => Vec::new(),
        args => args.list()?,
    };
    if args.len() > count {
        return Err(Error::Value(format!(
            "expected at most {count} arguments, received {}",
            args.len()
        )));
    }
    args.resize(count, Value::Undefined);
    Ok(args.into_iter())
}

/// Decode a parameter (any `Deserialize` type).
#[doc(hidden)]
pub fn decode<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    crate::session::decode_data(value)
}

/// Encode a result (any `Serialize` type). An integer beyond ±(2^53−1)
/// crosses as a bigint.
#[doc(hidden)]
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Reply {
    crate::session::arg(value)
}

/// A function the caller passed, to call back.
#[doc(hidden)]
#[derive(Clone)]
pub struct Callback(Reference);

/// The function passed as a parameter.
#[doc(hidden)]
pub fn callback(value: Value) -> Result<Callback, Error> {
    match value {
        Value::Reference(reference) if reference.is_function() => Ok(Callback(reference)),
        _ => Err(Error::Value("expected a function".into())),
    }
}

impl Callback {
    /// Call it with `args` and wait for what it returns to settle (a
    /// Promise is awaited); its failure is `E`'s, from the call's error.
    pub fn invoke<X, E>(&self, args: Vec<Reply>) -> BoxFuture<'static, Result<X, E>>
    where
        X: DeserializeOwned + 'static,
        E: From<Error> + 'static,
    {
        let reference = self.0.clone();
        Box::pin(async move {
            let args = args.into_iter().collect::<Result<Vec<_>, _>>()?;
            let returned = reference.call_async(Value::List(args)).await?;
            let settled = crate::session::settle(returned).await?;
            // What a callback returns where Rust expects nothing is ignored.
            if std::any::TypeId::of::<X>() == std::any::TypeId::of::<()>() {
                return Ok(decode(Value::Undefined)?);
            }
            Ok(decode(settled)?)
        })
    }
}

/// A parameter that takes a function as an `Arc<dyn Fn ...>` or `Box<dyn Fn
/// ...>` returning `BoxFuture<'static, Result<X, E>>`: the dispatcher builds
/// one that calls the caller's function, whose returned Promise settles
/// before the future does, and whose failure is the future's `Err`.
#[doc(hidden)]
pub trait FromCallback: Sized {
    fn from_callback(callback: Callback) -> Self;
}

macro_rules! from_callback {
    ($($arg:ident),*) => {
        #[allow(non_snake_case)]
        impl<$($arg,)* X, E> FromCallback
            for Arc<dyn Fn($($arg),*) -> BoxFuture<'static, Result<X, E>> + Send + Sync>
        where
            $($arg: Serialize + 'static,)*
            X: DeserializeOwned + 'static,
            E: From<Error> + 'static,
        {
            fn from_callback(callback: Callback) -> Self {
                Arc::new(move |$($arg: $arg),*| callback.invoke(vec![$(encode(&$arg)),*]))
            }
        }

        #[allow(non_snake_case)]
        impl<$($arg,)* X, E> FromCallback
            for Box<dyn Fn($($arg),*) -> BoxFuture<'static, Result<X, E>> + Send + Sync>
        where
            $($arg: Serialize + 'static,)*
            X: DeserializeOwned + 'static,
            E: From<Error> + 'static,
        {
            fn from_callback(callback: Callback) -> Self {
                Box::new(move |$($arg: $arg),*| callback.invoke(vec![$(encode(&$arg)),*]))
            }
        }

        #[allow(non_snake_case)]
        impl<$($arg,)* X, E> FromCallback
            for Box<dyn Fn($($arg),*) -> BoxFuture<'static, Result<X, E>> + Send>
        where
            $($arg: Serialize + 'static,)*
            X: DeserializeOwned + 'static,
            E: From<Error> + 'static,
        {
            fn from_callback(callback: Callback) -> Self {
                Box::new(move |$($arg: $arg),*| callback.invoke(vec![$(encode(&$arg)),*]))
            }
        }
    };
}

from_callback!();
from_callback!(A0);
from_callback!(A0, A1);
from_callback!(A0, A1, A2);
from_callback!(A0, A1, A2, A3);

/// Declare the module [`build::Exports`](crate::build::Exports)
/// generated, `exports` by default (`include_exports!(pub(crate) mod
/// services)` names it): `exports::all()` lists the [`Export`]s, one
/// function per export returns its own, and `exports::TYPESCRIPT` holds the
/// TypeScript declarations.
#[macro_export]
macro_rules! include_exports {
    () => {
        $crate::include_exports!(pub mod exports);
    };
    ($vis:vis mod $name:ident) => {
        #[allow(clippy::all, dead_code, unused_imports, unused_mut, unused_variables, non_snake_case)]
        $vis mod $name {
            include!(concat!(env!("OUT_DIR"), "/rutis_exports.rs"));
        }
    };
}

// ── Errors ──────────────────────────────────────────────────────

/// The error a method's `Err(e)` becomes: thrown in the caller's language
/// (a JavaScript `Error`, a Python `RemoteError`, a Go `*RemoteError`),
/// named `name`, with `code`, the message `e`'s `Display` and `data`, `e`'s
/// serde value when it has one. Read back in Rust with [`Error::code`] and
/// [`Error::data`].
pub fn failure(name: &str, code: &str, message: String, data: Option<Json>) -> Error {
    let mut nodes = Vec::new();
    nodes.push(Json::Null);
    let mut root = serde_json::Map::new();
    root.insert("type".into(), json!("error"));
    root.insert("name".into(), json!(name));
    root.insert("message".into(), json!(message));
    root.insert("code".into(), json!(code));
    if let Some(data) = data {
        let data = graph_value(data, &mut nodes);
        root.insert("data".into(), data);
    }
    nodes[0] = Json::Object(root);
    Error::Remote {
        name: name.to_owned(),
        message,
        graph: Some(json!({ "root": { "type": "reference", "value": 0 }, "nodes": nodes })),
    }
}

/// `data` as an error graph value: JSON as it is, unless it holds an
/// integer beyond ±(2^53−1), which crosses as a bigint (design V6), its
/// arrays and objects then as nodes.
fn graph_value(data: Json, nodes: &mut Vec<Json>) -> Json {
    if !crate::session::values::holds_beyond_safe(&data) {
        return json!({ "type": "data", "value": data });
    }
    match data {
        Json::Number(number) => json!({ "type": "bigint", "value": number.to_string() }),
        Json::Array(items) => {
            let id = nodes.len();
            nodes.push(Json::Null);
            let values: Vec<Json> = items
                .into_iter()
                .map(|item| graph_value(item, nodes))
                .collect();
            nodes[id] = json!({ "type": "array", "values": values });
            json!({ "type": "reference", "value": id })
        }
        Json::Object(fields) => {
            let id = nodes.len();
            nodes.push(Json::Null);
            let values: Vec<Json> = fields
                .into_iter()
                .map(|(key, value)| json!([key, graph_value(value, nodes)]))
                .collect();
            nodes[id] = json!({ "type": "object", "values": values });
            json!({ "type": "reference", "value": id })
        }
        other => json!({ "type": "data", "value": other }),
    }
}

/// `Err(e)` of a type with serde: `data` is its serde value; `code` the
/// string it serializes to, or its variant's name as serde writes it (the
/// one key of an externally tagged variant, the `tag` field of an
/// internally or adjacently tagged one), else `name`.
#[doc(hidden)]
pub fn serde_failure<E: Serialize + std::fmt::Display + ?Sized>(
    name: &str,
    tag: Option<&str>,
    variant: bool,
    error: &E,
) -> Error {
    let data = serde_json::to_value(error).ok();
    let code = match (&data, tag, variant) {
        (Some(Json::String(code)), _, _) => Some(code.clone()),
        (Some(Json::Object(fields)), Some(tag), true) => {
            fields.get(tag).and_then(Json::as_str).map(str::to_owned)
        }
        (Some(Json::Object(fields)), None, true) if fields.len() == 1 => {
            fields.keys().next().cloned()
        }
        _ => None,
    };
    failure(
        name,
        code.as_deref().unwrap_or(name),
        error.to_string(),
        data,
    )
}

/// `Err(e)` of an enum whose variant serde does not name (no serde, or
/// untagged): `code` is the variant's Rust name, read from its derived
/// `Debug`.
#[doc(hidden)]
pub fn variant_failure<E: std::fmt::Debug + std::fmt::Display + ?Sized>(
    name: &str,
    error: &E,
    data: Option<Json>,
) -> Error {
    let debug = format!("{error:?}");
    let variant: String = debug
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let code = if variant.is_empty() { name } else { &variant };
    failure(name, code, error.to_string(), data)
}

/// `Err(e)` of a type without serde: `code` is the type's name.
#[doc(hidden)]
pub fn display_failure<E: std::fmt::Display + ?Sized>(name: &str, error: &E) -> Error {
    failure(name, name, error.to_string(), None)
}

/// `Err(message)` of a `String`: named and coded `Error`, the string its
/// message, no data.
#[doc(hidden)]
pub fn string_failure(error: &str) -> Error {
    failure("Error", "Error", error.to_owned(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize)]
    enum Problem {
        Missing,
        Invalid { field: String },
        Big(u64),
    }

    impl std::fmt::Display for Problem {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "problem {self:?}")
        }
    }

    #[derive(Debug, Serialize)]
    struct Single {
        message: String,
    }

    impl std::fmt::Display for Single {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.message)
        }
    }

    #[test]
    fn errors_carry_their_code_message_and_data() {
        let error = serde_failure("Problem", None, true, &Problem::Missing);
        assert_eq!(error.code(), Some("Missing"));
        assert_eq!(error.data(), Some(json!("Missing")));
        assert_eq!(error.to_string(), "Problem: problem Missing");

        let error = serde_failure(
            "Problem",
            None,
            true,
            &Problem::Invalid {
                field: "title".into(),
            },
        );
        assert_eq!(error.code(), Some("Invalid"));
        assert_eq!(
            error.data(),
            Some(json!({ "Invalid": { "field": "title" } }))
        );

        // Beyond 2^53 it crosses as a bigint, and reads back exactly.
        let error = serde_failure("Problem", None, true, &Problem::Big(u64::MAX));
        assert_eq!(error.data(), Some(json!({ "Big": u64::MAX })));
        let Error::Remote { graph, .. } = &error else {
            panic!()
        };
        assert!(graph.as_ref().unwrap().to_string().contains("\"bigint\""));

        // One field is still a struct, not an externally tagged variant.
        let error = serde_failure(
            "Single",
            None,
            false,
            &Single {
                message: "one".into(),
            },
        );
        assert_eq!(error.code(), Some("Single"));
        assert_eq!(error.data(), Some(json!({ "message": "one" })));

        let error = variant_failure("Problem", &Problem::Invalid { field: "x".into() }, None);
        assert_eq!((error.code(), error.data()), (Some("Invalid"), None));
        let error = string_failure("no");
        assert_eq!((error.code(), error.data()), (Some("Error"), None));
        assert_eq!(error.to_string(), "Error: no");
    }

    #[test]
    fn arguments_not_passed_are_undefined() {
        let args = arguments(Value::List(vec![json!(1).into()]), 2).unwrap();
        assert_eq!(
            args.collect::<Vec<_>>(),
            [json!(1).into(), Value::Undefined]
        );
        assert!(arguments(Value::List(vec![Value::Undefined; 3]), 2).is_err());
        let value: Option<u64> = decode(Value::Undefined).unwrap();
        assert_eq!(value, None);
    }
}
