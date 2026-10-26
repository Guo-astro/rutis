//! The session protocol between rutis and another process: calls both
//! ways, references to objects and functions, async results and
//! cancellation, over any [`Channel`](crate::channel::Channel). Language
//! runtimes and links between nodes both run on it.
//!
//! A rutis service a session can call is a [`HostDispatch`], provided under
//! [`host_key`]`(name)`.

/// Wire protocol version of local runtime sessions. The Node runtime package
/// declares the version it speaks as `rutisProtocol` in its package.json;
/// builds check they match.
pub const PROTOCOL: u32 = 2;

/// The endpoint session format: endpoint ids in the handshake and in call
/// ids, capabilities, either side calling the other. Network sessions use
/// it (WebSocket subprotocol `rutis.3`); local runtime sessions keep
/// [`PROTOCOL`].
pub const ENDPOINT_PROTOCOL: u32 = 3;

mod objects;
pub(crate) mod protocol;
pub(crate) mod rpc;
mod services;
#[cfg(feature = "testing")]
pub mod testing;
pub mod values;

pub use objects::{
    arg, decode_value, Bytes, Dynamic, JsError, ObjectRef, RemoteFunction, ValueMap, ValueSet,
};
pub(crate) use objects::{decode_data_with, decode_with, encode_with, transcode_with, Marked};
pub use rpc::*;
pub(crate) use services::with_caller_row;
pub use services::{caller_row, host_key, host_key_in, CallerRow, HostDispatch};
pub use values::{BigInt, Date, DateTime, Decimal, Instant, TimeOfDay, Uuid};

/// What [`Error`] says about a session that ended or a call that failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Transport(String),
    #[error("{name}: {message}")]
    Remote {
        name: String,
        message: String,
        graph: Option<serde_json::Value>,
    },
    #[error("synchronous wait cycle: {0}")]
    SyncWaitCycle(String),
    /// A value that cannot cross, or a call this side refused: only that
    /// call fails. The text says which call, which argument (or the
    /// result) and why (#675).
    #[error("{0}")]
    Value(String),
    /// The session could not be established: see [`Handshake`].
    #[error("{0}")]
    Handshake(Handshake),
}

impl Error {
    /// The `code` an error carries: one a Rust service method returned as
    /// `Err(e)` ([`crate::exports`]), or any error whose graph names one
    /// (a JavaScript error with a string `code`). `None` for others.
    pub fn code(&self) -> Option<&str> {
        let Self::Remote { graph, .. } = self else {
            return None;
        };
        graph_root(graph.as_ref()?)?.get("code")?.as_str()
    }

    /// The `data` an error carries, as [`Error::code`]: the serde value of
    /// the `E` a Rust service method returned.
    pub fn data(&self) -> Option<serde_json::Value> {
        let Self::Remote { graph, .. } = self else {
            return None;
        };
        let graph = graph.as_ref()?;
        let data = graph_root(graph)?.get("data")?;
        graph_value(graph, data, 0)
    }
}

/// The node a graph's root refers to.
fn graph_root(graph: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    let root = graph.get("root")?;
    if root.get("type")?.as_str()? != "reference" {
        return None;
    }
    graph
        .get("nodes")?
        .get(root.get("value")?.as_u64()? as usize)?
        .as_object()
}

/// A value of an error graph as JSON: data, a bigint (as the number, or
/// its text beyond 64 bits), and the arrays and objects of its nodes.
fn graph_value(
    graph: &serde_json::Value,
    value: &serde_json::Value,
    depth: usize,
) -> Option<serde_json::Value> {
    use serde_json::Value as Json;
    if depth > MAX_DEPTH {
        return None;
    }
    match value.get("type")?.as_str()? {
        "undefined" => Some(Json::Null),
        "data" => value.get("value").cloned(),
        "bigint" | "number" => {
            let text = value.get("value")?.as_str()?;
            Some(
                text.parse::<serde_json::Number>()
                    .map(Json::Number)
                    .unwrap_or_else(|_| Json::String(text.to_owned())),
            )
        }
        "reference" => {
            let node = graph
                .get("nodes")?
                .get(value.get("value")?.as_u64()? as usize)?;
            match node.get("type")?.as_str()? {
                "array" => node
                    .get("values")?
                    .as_array()?
                    .iter()
                    .map(|item| graph_value(graph, item, depth + 1))
                    .collect::<Option<Vec<_>>>()
                    .map(Json::Array),
                "object" => node
                    .get("values")?
                    .as_array()?
                    .iter()
                    .map(|entry| {
                        let key = entry.get(0)?.as_str()?.to_owned();
                        Some((key, graph_value(graph, entry.get(1)?, depth + 1)?))
                    })
                    .collect::<Option<serde_json::Map<_, _>>>()
                    .map(Json::Object),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Why a session handshake failed. A link stops retrying an incompatible
/// far end, and retries slowly one whose identity does not match.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Handshake {
    /// Another protocol version or format, or a malformed handshake.
    #[error("incompatible session: {0}")]
    Incompatible(String),
    /// The far end named an endpoint other than the one verified or expected.
    #[error("endpoint mismatch: {0}")]
    IdentityMismatch(String),
}

/// An error raised on the Rust side, as the other side sees it.
pub fn native_error(error: impl std::fmt::Display) -> Error {
    Error::Remote {
        name: "RustError".into(),
        message: error.to_string(),
        graph: None,
    }
}

pub fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|error| Error::Value(error.to_string()))
}

/// Deserialize a field that TypeScript declares both optional and nullable
/// (`key?: T | null`) as `Option<Option<T>>`: a missing field is `None`, an
/// explicit `null` is `Some(None)`. Use with `#[serde(default)]`.
pub fn nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

/// Encode an optional argument: `None` is passed as JS `undefined`, not
/// `null`, so defaults and `=== undefined` checks behave as in native calls.
pub fn optional<T: serde::Serialize>(value: Option<T>) -> Result<Value, Error> {
    match value {
        Some(value) => arg(&value),
        None => Ok(Value::Undefined),
    }
}
