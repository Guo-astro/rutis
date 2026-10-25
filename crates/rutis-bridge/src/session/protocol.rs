use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::Error;

pub(crate) const VERSION: u32 = crate::session::PROTOCOL;

/// Who speaks at the far end, for diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Function,
    Future,
    /// An object with methods and live properties, addressed by reference.
    Object,
    /// A plugin definition: not called, only loaded by the runtime that
    /// defined it (`docs/design-plugin-values-2026-10-25.md` §5).
    Plugin,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum WireValue {
    Undefined,
    Data(Value),
    List(Vec<WireValue>),
    /// A plain object whose fields contain references.
    Record(std::collections::BTreeMap<String, WireValue>),
    /// An AbortSignal for the receiving call: it aborts when the caller
    /// cancels the call.
    Signal,
    Reference {
        id: u64,
        home: bool,
        kind: Kind,
        origin: Vec<String>,
    },
    // The tagged values (#308; `docs/design-protocol-plugins-2026-09-25.md`
    // §6.1), sent only to a far end that declared `value-<tag>`. Their text
    // is checked when decoded (`session::values`).
    /// RFC 3339 in UTC.
    Instant(String),
    #[serde(rename = "datetime")]
    DateTime(String),
    Date(String),
    Time(String),
    /// Base64, padded.
    Bytes(String),
    /// Key-value pairs in the sender's order.
    Map(Vec<(WireValue, WireValue)>),
    Set(Vec<WireValue>),
    #[serde(rename = "bigint")]
    BigInt(String),
    Decimal(String),
    Uuid(String),
}

impl WireValue {
    /// The tag of a tagged value, whose capability is `value-<tag>`.
    pub(crate) fn tag(&self) -> Option<&'static str> {
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
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Failure {
    pub name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<Value>,
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        match error {
            Error::Remote {
                name,
                message,
                graph,
            } => Self {
                name,
                message,
                graph,
            },
            Error::SyncWaitCycle(message) => Self {
                name: "SyncWaitCycle".into(),
                message,
                graph: None,
            },
            error => Self {
                name: "BindingError".into(),
                message: error.to_string(),
                graph: None,
            },
        }
    }
}
impl From<Failure> for Error {
    fn from(error: Failure) -> Self {
        if error.name == "SyncWaitCycle" && error.graph.is_none() {
            Self::SyncWaitCycle(error.message)
        } else {
            Self::Remote {
                name: error.name,
                message: error.message,
                graph: error.graph,
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Frame {
    /// Opens a session. Version 2 (compat) carries only the version; the
    /// endpoint format (3) also names the endpoint, its implementation and
    /// its capabilities.
    Hello {
        version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        implementation: Option<Implementation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Vec<String>>,
    },
    Invoke {
        id: String,
        path: Vec<String>,
        target: String,
        method: String,
        args: WireValue,
        /// The caller waits for the reply synchronously (`sync-wait`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
    },
    /// Call a function reference, or a method of an object reference.
    Call {
        id: String,
        path: Vec<String>,
        reference: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        method: Option<String>,
        args: WireValue,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
    },
    /// Read a property of an object reference (a live read).
    Get {
        id: String,
        path: Vec<String>,
        reference: u64,
        property: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
    },
    Await {
        id: String,
        path: Vec<String>,
        reference: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
    },
    Return {
        id: String,
        value: WireValue,
    },
    Throw {
        id: String,
        error: Failure,
    },
    Release {
        reference: u64,
        count: u64,
    },
    /// The caller gave up on call `id`: abort its signal / stop awaiting.
    /// A reply may still arrive and is then discarded.
    Cancel {
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A float in a frame reads back as the same double (#290). Written in
    /// their shortest form, these doubles read one or two units in the last
    /// place off under serde_json's default float parsing; its
    /// `float_roundtrip` feature reads them exactly.
    #[test]
    fn floats_in_a_frame_read_back_bit_exact() {
        for x in [
            1.0934904699147477e61_f64,
            5.763334961677081e-103,
            2.3852144308262914e-48,
            1.3677769034820771e302,
            3.278540359048182e-167,
            2.917874861685172e-272,
        ] {
            let frame = format!(
                r#"{{"op":"return","id":"1","value":{{"type":"data","value":[{}]}}}}"#,
                serde_json::to_string(&x).unwrap()
            );
            let Ok(Frame::Return {
                value: WireValue::Data(value),
                ..
            }) = serde_json::from_str::<Frame>(&frame)
            else {
                panic!("{frame} is not a return frame");
            };
            let got = value[0].as_f64().unwrap();
            assert_eq!(got.to_bits(), x.to_bits(), "{x:e} read back as {got:e}");
        }
    }
}
