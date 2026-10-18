//! Erased events and values: how an event crosses between listeners of
//! different Rust types, and out of the process.
//!
//! Every [`Event`](crate::Event) is serde, so the kernel can always turn it
//! into JSON and back. A dispatch carries its event as a [`Payload`]; a
//! listener whose type differs from the emitter's gets it through the bus's
//! [`Transcoder`]. Results travel as [`Value`]s the same way.

use std::any::Any;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as Json;

use crate::CordisError;

/// How one Rust type crosses as JSON. Built for a concrete type by
/// [`Codec::of`]; carried by every [`Payload`] and [`Value`].
#[derive(Clone, Copy)]
pub struct Codec {
    type_name: &'static str,
    type_id: fn() -> std::any::TypeId,
    to_json: fn(&dyn Any) -> Result<Json, CordisError>,
    from_json: fn(Json) -> Result<Box<dyn Any + Send>, CordisError>,
    to_args: fn(&dyn Any) -> Result<Vec<Json>, CordisError>,
    from_args: fn(Vec<Json>) -> Result<Box<dyn Any + Send>, CordisError>,
}

impl std::fmt::Debug for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Codec")
            .field("type", &self.type_name)
            .finish()
    }
}

fn wrong_type(expected: &'static str) -> CordisError {
    CordisError::EventValue {
        type_name: expected,
        message: "the value given to its codec has another type".into(),
    }
}

fn json_error<T>(error: serde_json::Error) -> CordisError {
    CordisError::EventValue {
        type_name: std::any::type_name::<T>(),
        message: error.to_string(),
    }
}

impl Codec {
    /// The codec of `T`.
    pub fn of<T: Serialize + DeserializeOwned + Send + 'static>() -> Self {
        Self {
            type_name: std::any::type_name::<T>(),
            type_id: std::any::TypeId::of::<T>,
            to_json: |value| {
                let value = value
                    .downcast_ref::<T>()
                    .ok_or_else(|| wrong_type(std::any::type_name::<T>()))?;
                serde_json::to_value(value).map_err(json_error::<T>)
            },
            from_json: |json| {
                serde_json::from_value::<T>(json)
                    .map(|value| Box::new(value) as Box<dyn Any + Send>)
                    .map_err(json_error::<T>)
            },
            to_args: |value| {
                let value = value
                    .downcast_ref::<T>()
                    .ok_or_else(|| wrong_type(std::any::type_name::<T>()))?;
                value
                    .serialize(super::args::ArgsSerializer)
                    .map_err(json_error::<T>)
            },
            from_args: |args| {
                let json = match args.len() {
                    0 => Json::Null,
                    1 => args.into_iter().next().expect("one argument"),
                    _ => Json::Array(args),
                };
                serde_json::from_value::<T>(json)
                    .map(|value| Box::new(value) as Box<dyn Any + Send>)
                    .map_err(json_error::<T>)
            },
        }
    }

    /// `std::any::type_name` of the type.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// The `TypeId` of the type.
    pub fn type_id(&self) -> std::any::TypeId {
        (self.type_id)()
    }

    /// `value` (which must be of this codec's type) as JSON.
    pub fn to_json(&self, value: &dyn Any) -> Result<Json, CordisError> {
        (self.to_json)(value)
    }

    /// A value of this codec's type from JSON.
    pub fn from_json(&self, json: Json) -> Result<Box<dyn Any + Send>, CordisError> {
        (self.from_json)(json)
    }

    /// `value` as event arguments (protocol spec §12.1): a tuple or tuple
    /// struct of two or more fields is one argument per field, a unit or
    /// unit struct none, anything else one argument.
    pub fn to_args(&self, value: &dyn Any) -> Result<Vec<Json>, CordisError> {
        (self.to_args)(value)
    }

    /// A value of this codec's type from event arguments: none decode from
    /// unit, one from itself, two or more as a tuple.
    pub fn from_args(&self, args: Vec<Json>) -> Result<Box<dyn Any + Send>, CordisError> {
        (self.from_args)(args)
    }
}

/// An event of a dispatch, owned and shared by its calls.
#[derive(Clone)]
pub struct Payload {
    value: Arc<dyn Any + Send + Sync>,
    codec: Codec,
}

impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Payload")
            .field("type", &self.codec.type_name)
            .finish()
    }
}

impl Payload {
    /// The payload of a typed event.
    pub fn new<E: crate::Event>(event: Arc<E>) -> Self {
        Self {
            value: event,
            codec: Codec::of::<E>(),
        }
    }

    /// A payload whose type is known only by its codec. `value` must have
    /// the codec's type.
    pub fn from_boxed(
        value: Box<dyn Any + Send + Sync>,
        codec: Codec,
    ) -> Result<Self, CordisError> {
        if (*value).type_id() != codec.type_id() {
            return Err(wrong_type(codec.type_name));
        }
        Ok(Self {
            value: Arc::from(value),
            codec,
        })
    }

    pub fn as_ref(&self) -> PayloadRef<'_> {
        PayloadRef {
            value: &*self.value,
            codec: self.codec,
        }
    }

    pub fn downcast<E: 'static>(&self) -> Option<&E> {
        self.value.downcast_ref()
    }

    pub fn codec(&self) -> &Codec {
        &self.codec
    }

    pub fn to_json(&self) -> Result<Json, CordisError> {
        self.as_ref().to_json()
    }

    pub fn to_args(&self) -> Result<Vec<Json>, CordisError> {
        self.as_ref().to_args()
    }

    pub(crate) fn arc(&self) -> &Arc<dyn Any + Send + Sync> {
        &self.value
    }
}

/// A borrowed event: what a listener of another type, a remote listener and
/// a transcoder see.
#[derive(Clone, Copy)]
pub struct PayloadRef<'a> {
    value: &'a (dyn Any + Send + Sync),
    codec: Codec,
}

impl<'a> PayloadRef<'a> {
    pub(crate) fn borrowed<E: crate::Event>(event: &'a E) -> Self {
        Self {
            value: event,
            codec: Codec::of::<E>(),
        }
    }

    pub fn downcast<E: 'static>(&self) -> Option<&'a E> {
        self.value.downcast_ref()
    }

    pub fn as_any(&self) -> &'a (dyn Any + Send + Sync) {
        self.value
    }

    pub fn codec(&self) -> &Codec {
        &self.codec
    }

    pub fn to_json(&self) -> Result<Json, CordisError> {
        self.codec.to_json(self.value)
    }

    pub fn to_args(&self) -> Result<Vec<Json>, CordisError> {
        self.codec.to_args(self.value)
    }
}

/// A listener's result, or a waterfall's value, of any type.
pub struct Value {
    value: Box<dyn Any + Send>,
    codec: Codec,
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Value")
            .field("type", &self.codec.type_name)
            .finish()
    }
}

impl Value {
    pub fn new<T: Serialize + DeserializeOwned + Send + 'static>(value: T) -> Self {
        Self {
            value: Box::new(value),
            codec: Codec::of::<T>(),
        }
    }

    /// A value whose type is known only by its codec. `value` must have the
    /// codec's type.
    pub fn from_boxed(value: Box<dyn Any + Send>, codec: Codec) -> Result<Self, CordisError> {
        if (*value).type_id() != codec.type_id() {
            return Err(wrong_type(codec.type_name));
        }
        Ok(Self { value, codec })
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.value.downcast_ref()
    }

    pub fn as_any(&self) -> &(dyn Any + Send) {
        &*self.value
    }

    pub fn codec(&self) -> &Codec {
        &self.codec
    }

    pub fn to_json(&self) -> Result<Json, CordisError> {
        self.codec.to_json(&*self.value)
    }

    /// Take the value as `T`: as it is when it has that type, otherwise
    /// through `transcoder`.
    pub fn into_typed<T: Serialize + DeserializeOwned + Send + 'static>(
        self,
        transcoder: &dyn Transcoder,
    ) -> Result<T, CordisError> {
        if self.value.is::<T>() {
            return Ok(*self.value.downcast::<T>().expect("checked type"));
        }
        let codec = Codec::of::<T>();
        let boxed = transcoder.value(&self, &codec)?;
        boxed
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| wrong_type(codec.type_name))
    }

    /// "No result" (protocol spec §1): `false`, or JSON `null` / `false`;
    /// a value of another type, but `()`, when it serializes to one of them
    /// (a session value holding `null`, say). A `None` never becomes a `Value`.
    pub fn is_no_result(&self) -> bool {
        if let Some(value) = self.value.downcast_ref::<bool>() {
            return !*value;
        }
        if let Some(json) = self.value.downcast_ref::<Json>() {
            return matches!(json, Json::Null | Json::Bool(false));
        }
        // `Some(())` is a result (only `None` is no result in Rust), though
        // it serializes to `null`.
        if self.value.is::<()>() {
            return false;
        }
        // A value that does not serialize (a reference, say) is a result.
        self.to_json()
            .is_ok_and(|json| matches!(json, Json::Null | Json::Bool(false)))
    }
}

/// Turns an event or a value into another Rust type. The bus uses it when a
/// listener's type differs from the dispatch's, and for results of another
/// type. The default goes through JSON: events by their arguments
/// ([`Codec::to_args`] then [`Codec::from_args`]), values by
/// [`Codec::to_json`] then [`Codec::from_json`]. Install another with
/// [`EventBus::set_transcoder`](crate::EventBus::set_transcoder).
pub trait Transcoder: Send + Sync + 'static {
    fn event(
        &self,
        from: PayloadRef<'_>,
        into: &Codec,
    ) -> Result<Box<dyn Any + Send>, CordisError> {
        into.from_args(from.to_args()?)
    }

    fn value(&self, from: &Value, into: &Codec) -> Result<Box<dyn Any + Send>, CordisError> {
        into.from_json(from.to_json()?)
    }
}

/// The default [`Transcoder`]: JSON.
pub struct JsonTranscoder;

impl Transcoder for JsonTranscoder {}

/// Decode `event` as `E`: itself when it is one, otherwise through `t`.
pub(crate) fn decode_event<E: crate::Event>(
    t: &dyn Transcoder,
    event: PayloadRef<'_>,
) -> Result<Box<E>, CordisError> {
    let codec = Codec::of::<E>();
    t.event(event, &codec)?
        .downcast::<E>()
        .map_err(|_| wrong_type(codec.type_name))
}
