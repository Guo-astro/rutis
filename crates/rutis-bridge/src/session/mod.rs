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
pub use services::{call_service, caller_row, host_key, host_key_in, CallerRow, HostDispatch};
pub(crate) use services::{unleased, with_caller_row, with_unleased};
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
    /// A call this side refused, other than for a value: with a stable
    /// [`code`] when it is one a developer meets (a method that does not
    /// exist, wrong arguments, a service that is not there; the
    /// diagnostics design §4.3), without one for a far end that broke the
    /// protocol. The far end gets a `BindingError` carrying the code and
    /// `data`.
    #[error("{message}")]
    Binding {
        code: Option<&'static str>,
        message: String,
        data: Option<serde_json::Value>,
    },
}

/// The codes of the `BindingError`s rutis raises (the diagnostics design
/// §4.3). A program decides by these, never by the text.
pub mod code {
    /// A service used without being declared in `inject`.
    pub const NOT_DECLARED: &str = "not-declared";
    /// A service with no provider now.
    pub const UNAVAILABLE: &str = "unavailable";
    /// A method the service does not have.
    pub const NO_METHOD: &str = "no-method";
    /// Arguments of the wrong number or type.
    pub const BAD_ARGUMENT: &str = "bad-argument";
    /// A value that cannot cross, or a message over the limit.
    pub const BAD_VALUE: &str = "bad-value";
    /// The far end's runtime ended or went away during the call.
    pub const PEER_GONE: &str = "peer-gone";
}

impl Error {
    /// A call refused with `code` (one of [`code`]), saying `message`, with
    /// the details in `data`.
    pub fn binding(code: &'static str, message: String, data: Option<serde_json::Value>) -> Self {
        Self::Binding {
            code: Some(code),
            message,
            data,
        }
    }

    /// A call refused because the far end broke the protocol: no code.
    pub fn refused(message: String) -> Self {
        Self::Binding {
            code: None,
            message,
            data: None,
        }
    }

    /// The `code` an error carries: one a Rust service method returned as
    /// `Err(e)` ([`crate::exports`]), any error whose graph names one (a
    /// JavaScript error with a string `code`), and rutis's own
    /// ([`code`]: a value error is `bad-value`, a session that ended
    /// during the call `peer-gone`). `None` for others.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Remote { graph, .. } => graph_root(graph.as_ref()?)?.get("code")?.as_str(),
            Self::Binding { code, .. } => *code,
            Self::Value(_) => Some(code::BAD_VALUE),
            Self::Transport(_) => Some(code::PEER_GONE),
            Self::SyncWaitCycle(_) | Self::Handshake(_) => None,
        }
    }

    /// The `data` an error carries, as [`Error::code`]: the serde value of
    /// the `E` a Rust service method returned.
    pub fn data(&self) -> Option<serde_json::Value> {
        if let Self::Binding { data, .. } = self {
            return data.clone();
        }
        let Self::Remote { graph, .. } = self else {
            return None;
        };
        let graph = graph.as_ref()?;
        let data = graph_root(graph)?.get("data")?;
        graph_value(graph, data, 0)
    }
}

impl Error {
    /// Where the error was thrown, as the language that threw it wrote it
    /// (`file:line` frames). `None` when it carries none.
    pub fn stack(&self) -> Option<&str> {
        let Self::Remote { graph, .. } = self else {
            return None;
        };
        graph_root(graph.as_ref()?)?.get("stack")?.as_str()
    }

    /// Where callers threw this error on again, after receiving it: the
    /// frames of each rethrow, the first caller first (the diagnostics
    /// design §4.4). Empty when none recorded it.
    pub fn rethrown(&self) -> Vec<String> {
        let Self::Remote {
            graph: Some(graph), ..
        } = self
        else {
            return Vec::new();
        };
        graph_root(graph)
            .and_then(|root| root.get("rethrown"))
            .and_then(|items| items.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// This error with `site` on its root node, unless it has one already
    /// (the first hop that recorded it wins). Errors without an error graph
    /// are returned as they are.
    pub fn with_site(self, site: &Site) -> Self {
        let Self::Remote {
            name,
            message,
            graph: Some(mut graph),
        } = self
        else {
            return self;
        };
        let root = graph
            .get("root")
            .filter(|root| root.get("type").and_then(|t| t.as_str()) == Some("reference"))
            .and_then(|root| root.get("value"))
            .and_then(|index| index.as_u64());
        if let Some(node) = root
            .and_then(|index| graph.get_mut("nodes")?.get_mut(index as usize))
            .and_then(|node| node.as_object_mut())
            .filter(|node| node.get("type").and_then(|t| t.as_str()) == Some("error"))
        {
            if !node.contains_key("site") {
                if let Ok(site) = serde_json::to_value(site) {
                    node.insert("site".into(), site);
                }
            }
        }
        Self::Remote {
            name,
            message,
            graph: Some(graph),
        }
    }

    /// Which row the error came from and what it was doing, as rutis
    /// recorded it where the error crossed (`docs/design-diagnostics-2026-10-26.md`
    /// §4.1). For whoever reads the output and for the host; the SDKs do not
    /// hand it to the calling plugin's code.
    pub fn site(&self) -> Option<Site> {
        let Self::Remote { graph, .. } = self else {
            return None;
        };
        let site = graph_root(graph.as_ref()?)?.get("site")?;
        serde_json::from_value(site.clone()).ok()
    }
}

/// Which row an error came from and what it was doing (§4.1 of the
/// diagnostics design): one optional field of an error node.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Site {
    /// The row's name (`weather`, or `weather in phone-1` inside an instance).
    pub row: String,
    /// `call`, `apply`, `cleanup` or `listener`.
    pub doing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
}

impl Site {
    /// The row `row` doing `doing` (`apply`, `cleanup`).
    pub fn new(row: impl Into<String>, doing: impl Into<String>) -> Self {
        Self {
            row: row.into(),
            doing: doing.into(),
            service: None,
            method: None,
            event: None,
        }
    }

    /// The row `row` serving a call to `service.method`.
    pub fn call(
        row: impl Into<String>,
        service: impl Into<String>,
        method: impl Into<String>,
    ) -> Self {
        Self {
            service: Some(service.into()),
            method: Some(method.into()),
            ..Self::new(row, "call")
        }
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
