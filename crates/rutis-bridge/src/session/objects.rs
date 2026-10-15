//! Live Cordis objects and tagged values inside generated data types.
//!
//! Generated types keep using serde. While a value is decoded, every object
//! reference in it is replaced by a marker that `ObjectRef` deserializes back
//! into the reference; encoding an argument does the reverse. Outside these
//! two operations an `ObjectRef` cannot be serialized.
//!
//! Tagged values (an [`Instant`], [`Bytes`], a [`ValueMap`], ...) go the
//! same way, so they cross under their own tag and not as JSON text or
//! arrays. Outside the two operations they serialize as their text (an
//! instant as RFC 3339, bytes as base64) or as arrays (a map as pairs), and
//! deserialize from that too.

use std::cell::RefCell;

use serde::de::DeserializeOwned;
use serde::{de, ser, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as Json};

use crate::session::rpc::{self, Reference, Value};
use crate::session::values::{
    base64_decode, base64_encode, describe, BigInt, Date, DateTime, Decimal, Instant, TimeOfDay,
    Uuid,
};
use crate::session::Error;

const MARK: &str = "\u{0}rutis:reference";
const TAGGED: &str = "\u{0}rutis:value";

/// A tagged value inside serde data: a leaf, or a map or set whose entries
/// are JSON holding markers of the same tables.
#[derive(Clone)]
enum Tagged {
    Leaf(Value),
    Map(Vec<(Json, Json)>),
    Set(Vec<Json>),
}

thread_local! {
    static DECODING: RefCell<Vec<Reference>> = const { RefCell::new(Vec::new()) };
    static ENCODING: RefCell<Option<Vec<Reference>>> = const { RefCell::new(None) };
    static DECODING_TAGGED: RefCell<Vec<Tagged>> = const { RefCell::new(Vec::new()) };
    static ENCODING_TAGGED: RefCell<Option<Vec<Tagged>>> = const { RefCell::new(None) };
}

fn tag_marker(index: usize) -> Json {
    let mut marker = Map::new();
    marker.insert(TAGGED.into(), index.into());
    Json::Object(marker)
}

/// Serialize a tagged value: while an argument is encoded, as a marker of
/// `tagged()`; otherwise as `plain` does.
fn serialize_tagged<S: Serializer>(
    serializer: S,
    tagged: impl FnOnce() -> Result<Tagged, String>,
    plain: impl FnOnce(S) -> Result<S::Ok, S::Error>,
) -> Result<S::Ok, S::Error> {
    if ENCODING_TAGGED.with(|current| current.borrow().is_none()) {
        return plain(serializer);
    }
    // Built before it is recorded: a map's entries record their own.
    let tagged = tagged().map_err(ser::Error::custom)?;
    let index = ENCODING_TAGGED.with(|current| {
        let mut current = current.borrow_mut();
        let list = current.as_mut().expect("still encoding");
        list.push(tagged);
        list.len() - 1
    });
    tag_marker(index).serialize(serializer)
}

/// Deserialize what may be a tagged value: the one a marker stands for, or
/// else the plain JSON.
fn deserialize_tagged<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Result<Tagged, Json>, D::Error> {
    let json = Json::deserialize(deserializer)?;
    let index = json
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.get(TAGGED))
        .and_then(Json::as_u64);
    let Some(index) = index else {
        return Ok(Err(json));
    };
    // Not consumed, as for references.
    DECODING_TAGGED
        .with(|tagged| tagged.borrow().get(index as usize).cloned())
        .map(Ok)
        .ok_or_else(|| de::Error::custom("tagged value outside of a decode"))
}

/// Serde for the types of tagged values kept as text: a marker inside
/// `arg` / `decode_value`, the text elsewhere.
macro_rules! tagged_text {
    ($($type:ident => $variant:ident, $what:literal;)*) => {$(
        impl Serialize for $type {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serialize_tagged(
                    serializer,
                    || Ok(Tagged::Leaf(Value::$variant(self.clone()))),
                    |serializer| serializer.collect_str(self),
                )
            }
        }
        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                match deserialize_tagged(deserializer)? {
                    Ok(Tagged::Leaf(Value::$variant(value))) => Ok(value),
                    Err(Json::String(text)) => text.parse().map_err(de::Error::custom),
                    Ok(Tagged::Leaf(other)) => Err(de::Error::custom(format!(
                        concat!("expected ", $what, ", received {}"),
                        describe(&other)
                    ))),
                    Ok(_) => Err(de::Error::custom(concat!(
                        "expected ", $what, ", received a map or set"
                    ))),
                    Err(other) => Err(de::Error::custom(format!(
                        concat!("expected ", $what, ", received {}"),
                        other
                    ))),
                }
            }
        }
    )*};
}
tagged_text! {
    Instant => Instant, "an instant";
    DateTime => DateTime, "a datetime";
    Date => Date, "a date";
    TimeOfDay => Time, "a time";
    BigInt => BigInt, "a bigint";
    Decimal => Decimal, "a decimal";
    Uuid => Uuid, "a uuid";
}

/// Binary data in generated types (tag `bytes`; JS `Uint8Array`): a
/// `Vec<u8>` that crosses as bytes, where serde would make a `Vec<u8>` an
/// array of numbers. Outside a call it serializes as base64 text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Bytes(pub Vec<u8>);

impl std::ops::Deref for Bytes {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}
impl std::ops::DerefMut for Bytes {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}
impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}
impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}
impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_tagged(
            serializer,
            || Ok(Tagged::Leaf(Value::Bytes(self.0.clone()))),
            |serializer| serializer.serialize_str(&base64_encode(&self.0)),
        )
    }
}
impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserialize_tagged(deserializer)? {
            Ok(Tagged::Leaf(Value::Bytes(bytes))) => Ok(Self(bytes)),
            Err(Json::String(text)) => base64_decode(&text).map(Self).map_err(de::Error::custom),
            Ok(Tagged::Leaf(other)) => Err(de::Error::custom(format!(
                "expected bytes, received {}",
                describe(&other)
            ))),
            Ok(_) => Err(de::Error::custom("expected bytes, received a map or set")),
            Err(other) => Err(de::Error::custom(format!(
                "expected bytes, received {other}"
            ))),
        }
    }
}

/// A map in generated types (tag `map`; JS `Map`): its entries in order.
/// It crosses as a map, never as an object; its keys must be of the kinds
/// every language takes as keys (see [`Value::Map`]). Outside a call it
/// serializes as an array of pairs.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueMap<K, V>(pub Vec<(K, V)>);

impl<K, V> Default for ValueMap<K, V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<K, V> From<Vec<(K, V)>> for ValueMap<K, V> {
    fn from(entries: Vec<(K, V)>) -> Self {
        Self(entries)
    }
}
impl<K: Serialize, V: Serialize> Serialize for ValueMap<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_tagged(
            serializer,
            || {
                self.0
                    .iter()
                    .map(|(key, value)| Ok((plain_json(key)?, plain_json(value)?)))
                    .collect::<Result<_, String>>()
                    .map(Tagged::Map)
            },
            |serializer| self.0.serialize(serializer),
        )
    }
}
impl<'de, K: DeserializeOwned, V: DeserializeOwned> Deserialize<'de> for ValueMap<K, V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserialize_tagged(deserializer)? {
            Ok(Tagged::Map(entries)) => entries
                .into_iter()
                .map(|(key, value)| Ok((from_json(key)?, from_json(value)?)))
                .collect::<Result<_, String>>()
                .map(Self)
                .map_err(de::Error::custom),
            Ok(_) => Err(de::Error::custom("expected a map, received another value")),
            Err(json) => from_json(json).map(Self).map_err(de::Error::custom),
        }
    }
}

/// A set in generated types (tag `set`; JS `Set`): its elements in order,
/// of the kinds a map key may be. Outside a call it serializes as an array.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueSet<T>(pub Vec<T>);

impl<T> Default for ValueSet<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<T> From<Vec<T>> for ValueSet<T> {
    fn from(elements: Vec<T>) -> Self {
        Self(elements)
    }
}
impl<T: Serialize> Serialize for ValueSet<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_tagged(
            serializer,
            || {
                self.0
                    .iter()
                    .map(plain_json)
                    .collect::<Result<_, String>>()
                    .map(Tagged::Set)
            },
            |serializer| self.0.serialize(serializer),
        )
    }
}
impl<'de, T: DeserializeOwned> Deserialize<'de> for ValueSet<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserialize_tagged(deserializer)? {
            Ok(Tagged::Set(elements)) => elements
                .into_iter()
                .map(from_json)
                .collect::<Result<_, String>>()
                .map(Self)
                .map_err(de::Error::custom),
            Ok(_) => Err(de::Error::custom("expected a set, received another value")),
            Err(json) => from_json(json).map(Self).map_err(de::Error::custom),
        }
    }
}

/// An entry of a map or set as JSON; references and tagged values in it
/// become markers of the encoding in progress.
fn plain_json<T: Serialize + ?Sized>(value: &T) -> Result<Json, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

/// An entry of a map or set read from JSON holding markers of the decoding
/// in progress.
fn from_json<T: DeserializeOwned>(json: Json) -> Result<T, String> {
    serde_json::from_value(json).map_err(|error| error.to_string())
}

/// A live object owned by the Cordis side: property reads and method calls
/// go to that object, so its identity and state are never copied. Passing it
/// back as an argument hands the original object to Cordis.
/// Two `ObjectRef`s are equal when they address the same object.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectRef(Reference);

impl ObjectRef {
    /// Call a method and wait for it to return.
    pub fn call(&self, method: &str, args: Vec<Value>) -> Result<Value, Error> {
        self.0.call_method(method, Value::List(args))
    }

    /// Call a method; a returned Promise is awaited.
    pub async fn call_async(&self, method: &str, args: Vec<Value>) -> Result<Value, Error> {
        rpc::settle(self.0.call_method_async(method, Value::List(args)).await?).await
    }

    /// Read a property. Every read reaches the object.
    pub fn get(&self, property: &str) -> Result<Value, Error> {
        self.0.get(property)
    }

    pub fn reference(&self) -> &Reference {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ObjectRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let reference = referenced(deserializer)?;
        if !reference.is_object() {
            return Err(de::Error::custom("expected a live object"));
        }
        Ok(Self(reference))
    }
}

impl Serialize for ObjectRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        mark(&self.0, serializer)
    }
}

/// A function owned by the Cordis side, for example a disposer a method
/// returned. Calling it runs the original function; equal when it is the
/// same function.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFunction(Reference);

impl RemoteFunction {
    /// Call it and wait for it to return.
    pub fn call(&self, args: Vec<Value>) -> Result<Value, Error> {
        self.0.call(Value::List(args))
    }

    /// Call it; a returned Promise is awaited.
    pub async fn call_async(&self, args: Vec<Value>) -> Result<Value, Error> {
        rpc::settle(self.0.call_async(Value::List(args)).await?).await
    }

    pub fn reference(&self) -> &Reference {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RemoteFunction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let reference = referenced(deserializer)?;
        if !reference.is_function() {
            return Err(de::Error::custom("expected a function"));
        }
        Ok(Self(reference))
    }
}

impl Serialize for RemoteFunction {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        mark(&self.0, serializer)
    }
}

/// A JS `Error` passed as a value (for example to a callback): its name,
/// message and stack cross as data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsError {
    pub name: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for JsError {}

fn referenced<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Reference, D::Error> {
    let marker = Json::deserialize(deserializer)?;
    let index = marker
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.get(MARK))
        .and_then(Json::as_u64)
        .ok_or_else(|| de::Error::custom("expected a reference, received data"))?;
    // Not consumed: serde may try the same input again, e.g. for the next
    // variant of an untagged enum.
    DECODING
        .with(|references| references.borrow().get(index as usize).cloned())
        .ok_or_else(|| de::Error::custom("reference outside of a decode"))
}

fn mark<S: Serializer>(reference: &Reference, serializer: S) -> Result<S::Ok, S::Error> {
    let index = ENCODING.with(|references| {
        references.borrow_mut().as_mut().map(|references| {
            references.push(reference.clone());
            references.len() - 1
        })
    });
    let index = index.ok_or_else(|| {
        ser::Error::custom("a reference can only be serialized as a call argument")
    })?;
    let mut marker = Map::new();
    marker.insert(MARK.into(), index.into());
    Json::Object(marker).serialize(serializer)
}

/// Decode a call result into a generated type; object references in it
/// become `ObjectRef`s.
///
/// Every reference must end up in an `ObjectRef` or `RemoteFunction` of the
/// result: one that lands in dynamic JSON (or is ignored) would lose the
/// object, so the decode fails instead of returning the internal marker as
/// data. Which references the result holds is read back from the decoded
/// value, so alternatives serde tried and discarded do not count, and each
/// occurrence of a reference needs its own holder.
///
/// Tagged values likewise decode into their types ([`Instant`], [`Bytes`],
/// [`ValueMap`], ...); one that would land in dynamic JSON fails the decode.
pub fn decode_value<T: de::DeserializeOwned + Serialize>(value: Value) -> Result<T, Error> {
    let mut references = Vec::new();
    let mut tagged = Vec::new();
    let json = marked(value, &mut references, &mut tagged);
    if references.is_empty() && tagged.is_empty() {
        return serde_json::from_value(json).map_err(|error| Error::Value(error.to_string()));
    }
    let count = tagged.len();
    let previous = DECODING.with(|current| current.replace(references.clone()));
    let previous_tagged = DECODING_TAGGED.with(|current| current.replace(tagged));
    let result = serde_json::from_value::<T>(json);
    DECODING.with(|current| current.replace(previous));
    DECODING_TAGGED.with(|current| current.replace(previous_tagged));
    let value = result.map_err(|error| Error::Value(error.to_string()))?;
    // Per occurrence: the same object may appear several times, and a typed
    // occurrence must not cover one that landed in JSON.
    let (mut held, typed) = held(&value)?;
    // Each tagged value in a type records itself again; one in JSON does not.
    if typed < count {
        return Err(Error::Value(
            "the value holds a tagged value (an instant, bytes, a map, ...) that its Rust type \
             cannot represent"
                .into(),
        ));
    }
    let unheld = references.iter().any(|reference| {
        match held.iter().position(|holder| holder == reference) {
            Some(index) => {
                held.swap_remove(index);
                false
            }
            None => true,
        }
    });
    if unheld {
        return Err(Error::Value(
            "the value holds a live Cordis object or function that its Rust type cannot represent"
                .into(),
        ));
    }
    Ok(value)
}

/// Serialize `value` as an argument is encoded: the references it holds in
/// its `ObjectRef`s and `RemoteFunction`s, and its tagged values.
fn encoding<T: Serialize + ?Sized>(
    value: &T,
) -> (serde_json::Result<Json>, Vec<Reference>, Vec<Tagged>) {
    let previous = ENCODING.with(|current| current.replace(Some(Vec::new())));
    let previous_tagged = ENCODING_TAGGED.with(|current| current.replace(Some(Vec::new())));
    let json = serde_json::to_value(value);
    let references = ENCODING
        .with(|current| current.replace(previous))
        .unwrap_or_default();
    let tagged = ENCODING_TAGGED
        .with(|current| current.replace(previous_tagged))
        .unwrap_or_default();
    (json, references, tagged)
}

/// The references a decoded value holds in its `ObjectRef`s and
/// `RemoteFunction`s, and how many tagged values it holds in their types.
fn held<T: Serialize>(value: &T) -> Result<(Vec<Reference>, usize), Error> {
    let (result, references, tagged) = encoding(value);
    result.map_err(|error| Error::Value(error.to_string()))?;
    Ok((references, tagged.len()))
}

fn marked(value: Value, references: &mut Vec<Reference>, tagged: &mut Vec<Tagged>) -> Json {
    match value {
        Value::Undefined => Json::Null,
        Value::Data(json) => json,
        Value::List(values) => Json::Array(
            values
                .into_iter()
                .map(|value| marked(value, references, tagged))
                .collect(),
        ),
        Value::Record(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, marked(value, references, tagged)))
                .collect(),
        ),
        Value::Signal => Json::Null,
        Value::Reference(reference) => {
            references.push(reference);
            let mut marker = Map::new();
            marker.insert(MARK.into(), (references.len() - 1).into());
            Json::Object(marker)
        }
        Value::Map(entries) => {
            let entries = entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        marked(key, references, tagged),
                        marked(value, references, tagged),
                    )
                })
                .collect();
            tagged.push(Tagged::Map(entries));
            tag_marker(tagged.len() - 1)
        }
        Value::Set(elements) => {
            let elements = elements
                .into_iter()
                .map(|element| marked(element, references, tagged))
                .collect();
            tagged.push(Tagged::Set(elements));
            tag_marker(tagged.len() - 1)
        }
        leaf => {
            tagged.push(Tagged::Leaf(leaf));
            tag_marker(tagged.len() - 1)
        }
    }
}

/// Encode a call argument; `ObjectRef`s in it pass their original object,
/// and tagged values ([`Instant`], [`Bytes`], [`ValueMap`], ...) cross
/// under their tags.
pub fn arg<T: Serialize + ?Sized>(value: &T) -> Result<Value, Error> {
    let (json, references, tagged) = encoding(value);
    let json = json.map_err(|error| Error::Value(error.to_string()))?;
    Ok(if references.is_empty() && tagged.is_empty() {
        Value::Data(json)
    } else {
        unmarked(json, &references, &tagged)
    })
}

fn unmarked(json: Json, references: &[Reference], tagged: &[Tagged]) -> Value {
    match json {
        Json::Object(fields) => {
            if fields.len() == 1 {
                if let Some(reference) = fields
                    .get(MARK)
                    .and_then(Json::as_u64)
                    .and_then(|index| references.get(index as usize))
                {
                    return Value::Reference(reference.clone());
                }
                if let Some(value) = fields
                    .get(TAGGED)
                    .and_then(Json::as_u64)
                    .and_then(|index| tagged.get(index as usize))
                {
                    let unmarked = |json: &Json| unmarked(json.clone(), references, tagged);
                    return match value {
                        Tagged::Leaf(value) => value.clone(),
                        Tagged::Map(entries) => Value::Map(
                            entries
                                .iter()
                                .map(|(key, value)| (unmarked(key), unmarked(value)))
                                .collect(),
                        ),
                        Tagged::Set(elements) => {
                            Value::Set(elements.iter().map(unmarked).collect())
                        }
                    };
                }
            }
            Value::Record(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, unmarked(value, references, tagged)))
                    .collect(),
            )
        }
        Json::Array(values) => Value::List(
            values
                .into_iter()
                .map(|value| unmarked(value, references, tagged))
                .collect(),
        ),
        json => Value::Data(json),
    }
}
