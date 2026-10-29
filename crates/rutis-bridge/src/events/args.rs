//! Events whose arguments are session values, and how events and results
//! cross between Rust types and sessions (events protocol §10, §12.1).
//!
//! What a runtime emits reaches the bus as [`Args`]: its arguments as they
//! came, references and tagged values included. A Rust listener of a typed
//! event gets it through the bus's transcoder, [`SessionTranscoder`], which
//! decodes the arguments as `session::decode_value` decodes a call's; a
//! remote listener of a typed event gets its arguments encoded as
//! `session::arg` encodes a call's. Results go the same ways as
//! [`ArgValue`]s.

use std::any::Any;
use std::sync::Arc;

use rutis::events::Mode;
use rutis::host::ErrorKind;
use rutis::internal::events::{Codec, Payload, PayloadRef, Transcoder, Value as Result_};
use rutis::{Error as RutisError, Event};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::session::{decode_with, encode_with, transcode_with, Dynamic, Error, Value};

/// A dynamic event: its arguments as session values. Listen to it by name
/// with `ctx.listen::<Args>().name(name)`, and send it under a name with
/// `CtxInternal::named(&ctx, name)`, to take whatever arguments the event
/// has. It has no name of its own: without one every registration and
/// dispatch fails (`Args events need a name: EventKey::dynamic(name)`,
/// events protocol §12.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args(pub Vec<Value>);

impl Event for Args {
    /// Empty: no name of its own (see [`Args`]).
    const NAME: &'static str = "";
    type Value = ArgValue;
}

/// A result of an [`Args`] event: a session value.
#[derive(Clone, Debug, PartialEq)]
pub struct ArgValue(pub Value);

impl Serialize for Args {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let items: Vec<Dynamic> = self.0.iter().cloned().map(Dynamic).collect();
        items.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Args {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let items = Vec::<Dynamic>::deserialize(deserializer)?;
        Ok(Self(items.into_iter().map(|item| item.0).collect()))
    }
}

impl Serialize for ArgValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Dynamic(self.0.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ArgValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Dynamic::deserialize(deserializer).map(|value| Self(value.0))
    }
}

fn event_error(type_name: &'static str, message: impl Into<String>) -> RutisError {
    rutis::Error::from(ErrorKind::EventValue {
        type_name,
        message: message.into(),
    })
}

/// The bus's transcoder once rutis-bridge runs on it: conversions keep
/// references and tagged values, and [`Args`] maps to and from the
/// arguments of a typed event.
pub(crate) struct SessionTranscoder;

impl Transcoder for SessionTranscoder {
    fn event(&self, from: PayloadRef<'_>, into: &Codec) -> Result<Box<dyn Any + Send>, RutisError> {
        let into_args = into.type_id() == std::any::TypeId::of::<Args>();
        if let Some(args) = from.downcast::<Args>() {
            if into_args {
                return Ok(Box::new(args.clone()));
            }
            return decode_with(Value::List(args.0.clone()), |json| {
                let Json::Array(items) = json else {
                    return Err("the arguments are not a list".to_owned());
                };
                into.from_args(items).map_err(|error| error.to_string())
            })
            .map_err(|error| event_error(into.type_name(), error.to_string()));
        }
        if into_args {
            return to_wire(from)
                .map(|values| Box::new(Args(values)) as Box<dyn Any + Send>)
                .map_err(|error| event_error(from.codec().type_name(), error.to_string()));
        }
        transcode_with(
            || from.to_args().map(Json::Array).map_err(|e| e.to_string()),
            |json| {
                let Json::Array(items) = json else {
                    return Err("the arguments are not a list".to_owned());
                };
                into.from_args(items).map_err(|error| error.to_string())
            },
        )
        .map_err(|error| event_error(into.type_name(), error))
    }

    fn value(&self, from: &Result_, into: &Codec) -> Result<Box<dyn Any + Send>, RutisError> {
        // Only a listener of a runtime waiting for its waterfall
        // synchronously answers a promise unsettled (§5.4).
        if let Some(ArgValue(Value::Reference(value))) = from.downcast_ref::<ArgValue>() {
            if value.is_future() {
                return Err(super::errors::to_cordis(super::errors::promise_of_waiting()));
            }
        }
        transcode_with(
            || from.to_json().map_err(|e| e.to_string()),
            |json| into.from_json(json).map_err(|error| error.to_string()),
        )
        .map_err(|error| event_error(into.type_name(), error))
    }
}

/// An event's arguments, as a session sends them.
pub(crate) fn to_wire(event: PayloadRef<'_>) -> Result<Vec<Value>, Error> {
    if let Some(args) = event.downcast::<Args>() {
        return Ok(args.0.clone());
    }
    let encoded = encode_with(|| {
        event
            .to_args()
            .map(Json::Array)
            .map_err(|error| error.to_string())
    })?;
    encoded.list()
}

/// The event a session's arguments make.
pub(crate) fn from_wire(args: Vec<Value>) -> Payload {
    Payload::new(Arc::new(Args(args)))
}

/// A result as a session sends it: `undefined` for none.
pub(crate) fn result_to_wire(result: Option<Result_>) -> Result<Value, Error> {
    let Some(result) = result else {
        return Ok(Value::Undefined);
    };
    if let Some(value) = result.downcast_ref::<ArgValue>() {
        return Ok(value.0.clone());
    }
    encode_with(|| result.to_json().map_err(|error| error.to_string()))
}

/// A result a session sent, in `mode`: `undefined` and `null` are none;
/// `false` too, but in a waterfall, which returns its value as is (events
/// protocol §1, §5.4).
pub(crate) fn result_from_wire(value: Value, mode: Mode) -> Option<Result_> {
    match &value {
        Value::Undefined | Value::Data(Json::Null) => None,
        Value::Data(Json::Bool(false)) if !mode.is_waterfall() => None,
        _ => Some(Result_::new(ArgValue(value))),
    }
}
