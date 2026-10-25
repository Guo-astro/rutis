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

/// What the marker keys of one encode or decode start with.
const PREFIX: &str = "\u{0}rutis:";

/// The keys of the markers one encode or decode uses: a single-field
/// object `{<reference>: n}` stands for the n-th reference, `{<value>: n}`
/// for the n-th tagged value. Each operation picks keys the data it
/// carries does not use (see [`arg`], [`decode_value`]), so user JSON is
/// never read as a marker, whatever keys it has.
#[derive(Clone)]
struct Keys {
    reference: String,
    value: String,
}

impl Keys {
    fn new(nonce: u64) -> Self {
        Self {
            reference: format!("{PREFIX}reference:{nonce}"),
            value: format!("{PREFIX}value:{nonce}"),
        }
    }
    /// Keys whose names are not in `taken`.
    fn avoiding(taken: &std::collections::HashSet<String>) -> Self {
        loop {
            let keys = Self::new(NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
            if !taken.contains(&keys.reference) && !taken.contains(&keys.value) {
                return keys;
            }
        }
    }
    fn reference_marker(&self, index: usize) -> Json {
        Json::Object(Map::from_iter([(self.reference.clone(), index.into())]))
    }
    fn value_marker(&self, index: usize) -> Json {
        Json::Object(Map::from_iter([(self.value.clone(), index.into())]))
    }
}

static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A tagged value inside serde data: a leaf, or a map or set whose entries
/// are JSON holding markers of the same tables.
#[derive(Clone)]
enum Tagged {
    Leaf(Value),
    Map(Vec<(Json, Json)>),
    Set(Vec<Json>),
}

thread_local! {
    /// The marker keys of the encode or decode in progress on this thread.
    static KEYS: RefCell<Option<Keys>> = const { RefCell::new(None) };
    static DECODING: RefCell<Vec<Reference>> = const { RefCell::new(Vec::new()) };
    static ENCODING: RefCell<Option<Vec<Reference>>> = const { RefCell::new(None) };
    static DECODING_TAGGED: RefCell<Vec<Tagged>> = const { RefCell::new(Vec::new()) };
    static ENCODING_TAGGED: RefCell<Option<Vec<Tagged>>> = const { RefCell::new(None) };
}

fn current_keys() -> Option<Keys> {
    KEYS.with(|keys| keys.borrow().clone())
}

/// The index a single-field `{<key>: n}` object stands for, if `json` is
/// one and `key` is a marker key of the operation in progress.
fn marker_index(json: &Json, key: impl Fn(&Keys) -> &str) -> Option<usize> {
    let keys = current_keys()?;
    json.as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.get(key(&keys)))
        .and_then(Json::as_u64)
        .map(|index| index as usize)
}

/// Every object key in `json` that starts with [`PREFIX`].
fn prefixed_keys(json: &Json, found: &mut std::collections::HashSet<String>) {
    match json {
        Json::Object(fields) => {
            for (key, value) in fields {
                if key.starts_with(PREFIX) {
                    found.insert(key.clone());
                }
                prefixed_keys(value, found);
            }
        }
        Json::Array(items) => items.iter().for_each(|item| prefixed_keys(item, found)),
        _ => {}
    }
}

/// Serialize a tagged value: while an argument is encoded, as a marker of
/// `tagged()`; otherwise as `plain` does.
fn serialize_tagged<S: Serializer>(
    serializer: S,
    tagged: impl FnOnce() -> Result<Tagged, String>,
    plain: impl FnOnce(S) -> Result<S::Ok, S::Error>,
) -> Result<S::Ok, S::Error> {
    let keys = match current_keys() {
        Some(keys) if ENCODING_TAGGED.with(|current| current.borrow().is_some()) => keys,
        _ => return plain(serializer),
    };
    // Built before it is recorded: a map's entries record their own.
    let tagged = tagged().map_err(ser::Error::custom)?;
    let index = ENCODING_TAGGED.with(|current| {
        let mut current = current.borrow_mut();
        let list = current.as_mut().expect("still encoding");
        list.push(tagged);
        list.len() - 1
    });
    keys.value_marker(index).serialize(serializer)
}

/// Deserialize what may be a tagged value: the one a marker stands for, or
/// else the plain JSON.
fn deserialize_tagged<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Result<Tagged, Json>, D::Error> {
    let json = Json::deserialize(deserializer)?;
    let Some(index) = marker_index(&json, |keys| &keys.value) else {
        return Ok(Err(json));
    };
    // Not consumed, as for references.
    DECODING_TAGGED
        .with(|tagged| tagged.borrow().get(index).cloned())
        .map(Ok)
        .ok_or_else(|| de::Error::custom("tagged value outside of a decode"))
}

/// Serde for the types of tagged values kept as text: a marker inside
/// `arg` / `decode_value`, the text elsewhere. `integers`: whether the
/// type also takes an integer.
macro_rules! tagged_text {
    ($($type:ident => $variant:ident, $what:literal, $integers:literal;)*) => {$(
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
                    // A bigint that fits 64 bits is decoded as its number
                    // (see `marked`); only a `BigInt` takes one.
                    Err(Json::Number(number)) if $integers && !number.is_f64() => {
                        number.to_string().parse().map_err(de::Error::custom)
                    }
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
    Instant => Instant, "an instant", false;
    DateTime => DateTime, "a datetime", false;
    Date => Date, "a date", false;
    TimeOfDay => Time, "a time", false;
    BigInt => BigInt, "a bigint", true;
    Decimal => Decimal, "a decimal", false;
    Uuid => Uuid, "a uuid", false;
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
    value.serialize(Exact).map_err(|error| error.to_string())
}

/// Serializes as `serde_json::to_value` does, except that an `i128` or
/// `u128` beyond 64 bits, which JSON data cannot hold, becomes a bigint
/// (design V6); used while an argument is encoded.
struct Exact;

/// Scalars as `serde_json::to_value` makes them.
macro_rules! as_json {
    ($($method:ident($type:ty);)*) => {$(
        fn $method(self, value: $type) -> Result<Json, serde_json::Error> {
            serde_json::value::Serializer.$method(value)
        }
    )*};
}

impl Serializer for Exact {
    type Ok = Json;
    type Error = serde_json::Error;
    type SerializeSeq = Items;
    type SerializeTuple = Items;
    type SerializeTupleStruct = Items;
    type SerializeTupleVariant = Variant<Items>;
    type SerializeMap = Fields;
    type SerializeStruct = Fields;
    type SerializeStructVariant = Variant<Fields>;

    as_json! {
        serialize_bool(bool);
        serialize_i8(i8);
        serialize_i16(i16);
        serialize_i32(i32);
        serialize_i64(i64);
        serialize_u8(u8);
        serialize_u16(u16);
        serialize_u32(u32);
        serialize_u64(u64);
        serialize_f32(f32);
        serialize_f64(f64);
        serialize_char(char);
        serialize_str(&str);
        serialize_bytes(&[u8]);
        serialize_unit_struct(&'static str);
    }
    fn serialize_i128(self, value: i128) -> Result<Json, serde_json::Error> {
        match i64::try_from(value) {
            Ok(value) => Ok(value.into()),
            Err(_) => BigInt::from(value).serialize(serde_json::value::Serializer),
        }
    }
    fn serialize_u128(self, value: u128) -> Result<Json, serde_json::Error> {
        match u64::try_from(value) {
            Ok(value) => Ok(value.into()),
            Err(_) => BigInt::from(value).serialize(serde_json::value::Serializer),
        }
    }
    fn serialize_none(self) -> Result<Json, serde_json::Error> {
        Ok(Json::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Json, serde_json::Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Json, serde_json::Error> {
        Ok(Json::Null)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<Json, serde_json::Error> {
        Ok(variant.into())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<Json, serde_json::Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Json, serde_json::Error> {
        Ok(Json::Object(Map::from_iter([(
            variant.to_owned(),
            value.serialize(self)?,
        )])))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Items, serde_json::Error> {
        Ok(Items(Vec::with_capacity(len.unwrap_or(0))))
    }
    fn serialize_tuple(self, len: usize) -> Result<Items, serde_json::Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        len: usize,
    ) -> Result<Items, serde_json::Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Variant<Items>, serde_json::Error> {
        Ok(Variant(variant, self.serialize_seq(Some(len))?))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Fields, serde_json::Error> {
        Ok(Fields(Map::new(), None))
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Fields, serde_json::Error> {
        self.serialize_map(None)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Variant<Fields>, serde_json::Error> {
        Ok(Variant(variant, self.serialize_map(None)?))
    }
}

/// The items of an array being serialized by [`Exact`].
struct Items(Vec<Json>);

impl ser::SerializeSeq for Items {
    type Ok = Json;
    type Error = serde_json::Error;
    fn serialize_element<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        self.0.push(value.serialize(Exact)?);
        Ok(())
    }
    fn end(self) -> Result<Json, serde_json::Error> {
        Ok(Json::Array(self.0))
    }
}
impl ser::SerializeTuple for Items {
    type Ok = Json;
    type Error = serde_json::Error;
    fn serialize_element<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Json, serde_json::Error> {
        ser::SerializeSeq::end(self)
    }
}
impl ser::SerializeTupleStruct for Items {
    type Ok = Json;
    type Error = serde_json::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Json, serde_json::Error> {
        ser::SerializeSeq::end(self)
    }
}

/// The fields of an object being serialized by [`Exact`], and the key
/// waiting for its value.
struct Fields(Map<String, Json>, Option<String>);

impl ser::SerializeMap for Fields {
    type Ok = Json;
    type Error = serde_json::Error;
    /// A key as `serde_json` takes one: a string, or a number, boolean or
    /// character written as one.
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), serde_json::Error> {
        self.1 = Some(match key.serialize(serde_json::value::Serializer)? {
            Json::String(key) => key,
            Json::Number(key) => key.to_string(),
            Json::Bool(key) => key.to_string(),
            _ => return Err(ser::Error::custom("key must be a string")),
        });
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        let key = self.1.take().expect("a key before its value");
        self.0.insert(key, value.serialize(Exact)?);
        Ok(())
    }
    fn end(self) -> Result<Json, serde_json::Error> {
        Ok(Json::Object(self.0))
    }
}
impl ser::SerializeStruct for Fields {
    type Ok = Json;
    type Error = serde_json::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        self.0.insert(key.to_owned(), value.serialize(Exact)?);
        Ok(())
    }
    fn end(self) -> Result<Json, serde_json::Error> {
        ser::SerializeMap::end(self)
    }
}

/// An enum variant with fields being serialized by [`Exact`]:
/// `{"<variant>": <fields>}`.
struct Variant<T>(&'static str, T);

impl ser::SerializeTupleVariant for Variant<Items> {
    type Ok = Json;
    type Error = serde_json::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        ser::SerializeSeq::serialize_element(&mut self.1, value)
    }
    fn end(self) -> Result<Json, serde_json::Error> {
        let items = ser::SerializeSeq::end(self.1)?;
        Ok(Json::Object(Map::from_iter([(self.0.to_owned(), items)])))
    }
}
impl ser::SerializeStructVariant for Variant<Fields> {
    type Ok = Json;
    type Error = serde_json::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        ser::SerializeStruct::serialize_field(&mut self.1, key, value)
    }
    fn end(self) -> Result<Json, serde_json::Error> {
        let fields = ser::SerializeMap::end(self.1)?;
        Ok(Json::Object(Map::from_iter([(self.0.to_owned(), fields)])))
    }
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
    let index = marker_index(&marker, |keys| &keys.reference)
        .ok_or_else(|| de::Error::custom("expected a reference, received data"))?;
    // Not consumed: serde may try the same input again, e.g. for the next
    // variant of an untagged enum.
    DECODING
        .with(|references| references.borrow().get(index).cloned())
        .ok_or_else(|| de::Error::custom("reference outside of a decode"))
}

/// A reference serialized as itself, wherever it is (a plugin definition
/// going back to its runtime, `rutis::Definition`).
pub(crate) struct Marked(pub(crate) Reference);

impl Serialize for Marked {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        mark(&self.0, serializer)
    }
}

fn mark<S: Serializer>(reference: &Reference, serializer: S) -> Result<S::Ok, S::Error> {
    let index = ENCODING.with(|references| {
        references.borrow_mut().as_mut().map(|references| {
            references.push(reference.clone());
            references.len() - 1
        })
    });
    let (index, keys) = index.zip(current_keys()).ok_or_else(|| {
        ser::Error::custom("a reference can only be serialized as a call argument")
    })?;
    keys.reference_marker(index).serialize(serializer)
}

/// Run `operation` with `keys` as this thread's marker keys.
fn with_keys<R>(keys: &Keys, operation: impl FnOnce() -> R) -> R {
    let previous = KEYS.with(|current| current.replace(Some(keys.clone())));
    let result = operation();
    KEYS.with(|current| current.replace(previous));
    result
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
    // Marker keys no object in the data uses: the data is never read as a
    // marker.
    let mut taken = std::collections::HashSet::new();
    value_keys(&value, &mut taken);
    let keys = Keys::avoiding(&taken);
    let mut references = Vec::new();
    let mut tagged = Vec::new();
    let json = marked(value, &keys, &mut references, &mut tagged);
    if references.is_empty() && tagged.is_empty() {
        return with_keys(&keys, || serde_json::from_value(json))
            .map_err(|error| Error::Value(error.to_string()));
    }
    let count = tagged.len();
    let previous = DECODING.with(|current| current.replace(references.clone()));
    let previous_tagged = DECODING_TAGGED.with(|current| current.replace(tagged));
    let result = with_keys(&keys, || serde_json::from_value::<T>(json));
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

/// Decode an argument into a data type, which, unlike with
/// [`decode_value`], need not be `Serialize` too: `consume` deserializes
/// from the JSON it is given. How a `#[rutis::service]` dispatch decodes
/// its parameters. Tagged values decode into their types (an integer type
/// takes a bigint that fits it); a function or object reference is
/// refused, since data cannot hold one.
pub(crate) fn decode_data_with(
    value: Value,
    consume: impl FnOnce(Json) -> Result<(), String>,
) -> Result<(), Error> {
    let mut taken = std::collections::HashSet::new();
    value_keys(&value, &mut taken);
    let keys = Keys::avoiding(&taken);
    let mut references = Vec::new();
    let mut tagged = Vec::new();
    let json = marked(value, &keys, &mut references, &mut tagged);
    if !references.is_empty() {
        return Err(Error::Value(
            "a function or object reference was passed where data is expected".into(),
        ));
    }
    let previous = DECODING_TAGGED.with(|current| current.replace(tagged));
    let result = with_keys(&keys, || consume(json));
    DECODING_TAGGED.with(|current| current.replace(previous));
    result.map_err(Error::Value)
}

/// Serialize `value` as an argument is encoded: the references it holds in
/// its `ObjectRef`s and `RemoteFunction`s, and its tagged values.
fn encoding<T: Serialize + ?Sized>(
    value: &T,
    keys: &Keys,
) -> (serde_json::Result<Json>, Vec<Reference>, Vec<Tagged>) {
    let previous = ENCODING.with(|current| current.replace(Some(Vec::new())));
    let previous_tagged = ENCODING_TAGGED.with(|current| current.replace(Some(Vec::new())));
    let json = with_keys(keys, || value.serialize(Exact));
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
    // Only what is recorded counts here, so any keys do.
    let keys = Keys::new(NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let (result, references, tagged) = encoding(value, &keys);
    result.map_err(|error| Error::Value(error.to_string()))?;
    // A bigint that fits 64 bits was decoded as a number (see `marked`).
    let typed = tagged
        .iter()
        .filter(|tagged| {
            !matches!(tagged, Tagged::Leaf(Value::BigInt(value)) if value.to_json_number().is_some())
        })
        .count();
    Ok((references, typed))
}

/// Every object key starting with [`PREFIX`] in the data of `value`.
fn value_keys(value: &Value, found: &mut std::collections::HashSet<String>) {
    match value {
        Value::Data(json) => prefixed_keys(json, found),
        Value::List(values) => values.iter().for_each(|value| value_keys(value, found)),
        Value::Record(fields) => {
            for (key, value) in fields {
                if key.starts_with(PREFIX) {
                    found.insert(key.clone());
                }
                value_keys(value, found);
            }
        }
        Value::Map(entries) => {
            for (key, value) in entries {
                value_keys(key, found);
                value_keys(value, found);
            }
        }
        Value::Set(elements) => elements.iter().for_each(|value| value_keys(value, found)),
        _ => {}
    }
}

fn marked(
    value: Value,
    keys: &Keys,
    references: &mut Vec<Reference>,
    tagged: &mut Vec<Tagged>,
) -> Json {
    match value {
        Value::Undefined => Json::Null,
        Value::Data(json) => json,
        Value::List(values) => Json::Array(
            values
                .into_iter()
                .map(|value| marked(value, keys, references, tagged))
                .collect(),
        ),
        Value::Record(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, marked(value, keys, references, tagged)))
                .collect(),
        ),
        Value::Signal => Json::Null,
        Value::Reference(reference) => {
            references.push(reference);
            keys.reference_marker(references.len() - 1)
        }
        Value::Map(entries) => {
            let entries = entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        marked(key, keys, references, tagged),
                        marked(value, keys, references, tagged),
                    )
                })
                .collect();
            tagged.push(Tagged::Map(entries));
            keys.value_marker(tagged.len() - 1)
        }
        Value::Set(elements) => {
            let elements = elements
                .into_iter()
                .map(|element| marked(element, keys, references, tagged))
                .collect();
            tagged.push(Tagged::Set(elements));
            keys.value_marker(tagged.len() - 1)
        }
        // An integer type takes a bigint that fits it (design V6); a
        // `BigInt` takes the number too.
        Value::BigInt(value) if value.to_json_number().is_some() => {
            Json::Number(value.to_json_number().expect("fits"))
        }
        leaf => {
            tagged.push(Tagged::Leaf(leaf));
            keys.value_marker(tagged.len() - 1)
        }
    }
}

/// How often `json` and the entries of `tagged` use each marker key, as
/// object keys anywhere (marker or not).
fn key_uses(json: &Json, tagged: &[Tagged], keys: &Keys) -> (usize, usize) {
    fn walk(json: &Json, keys: &Keys, uses: &mut (usize, usize)) {
        match json {
            Json::Object(fields) => {
                for (key, value) in fields {
                    uses.0 += usize::from(*key == keys.reference);
                    uses.1 += usize::from(*key == keys.value);
                    walk(value, keys, uses);
                }
            }
            Json::Array(items) => items.iter().for_each(|item| walk(item, keys, uses)),
            _ => {}
        }
    }
    let mut uses = (0, 0);
    walk(json, keys, &mut uses);
    for entry in tagged {
        match entry {
            Tagged::Leaf(_) => {}
            Tagged::Map(entries) => entries.iter().for_each(|(key, value)| {
                walk(key, keys, &mut uses);
                walk(value, keys, &mut uses);
            }),
            Tagged::Set(elements) => elements.iter().for_each(|json| walk(json, keys, &mut uses)),
        }
    }
    uses
}

/// Encode a call argument; `ObjectRef`s in it pass their original object,
/// and tagged values ([`Instant`], [`Bytes`], [`ValueMap`], ...) cross
/// under their tags.
pub fn arg<T: Serialize + ?Sized>(value: &T) -> Result<Value, Error> {
    let mut keys = Keys::new(NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    // The data is only known once serialized: if any of it uses a marker
    // key, which only the markers recorded may, encode again with keys it
    // does not use. Serialized again, the same data uses the same keys.
    for _ in 0..3 {
        let (json, references, tagged) = encoding(value, &keys);
        let json = json.map_err(|error| Error::Value(error.to_string()))?;
        if references.is_empty() && tagged.is_empty() {
            return Ok(Value::Data(json));
        }
        if key_uses(&json, &tagged, &keys) == (references.len(), tagged.len()) {
            return Ok(unmarked(json, &keys, &references, &tagged));
        }
        let mut taken = std::collections::HashSet::new();
        prefixed_keys(&json, &mut taken);
        for entry in &tagged {
            match entry {
                Tagged::Leaf(_) => {}
                Tagged::Map(entries) => entries.iter().for_each(|(key, value)| {
                    prefixed_keys(key, &mut taken);
                    prefixed_keys(value, &mut taken);
                }),
                Tagged::Set(elements) => elements
                    .iter()
                    .for_each(|json| prefixed_keys(json, &mut taken)),
            }
        }
        keys = Keys::avoiding(&taken);
    }
    Err(Error::Value(
        "the value serializes differently each time, so its markers cannot be told from its data"
            .into(),
    ))
}

fn unmarked(json: Json, keys: &Keys, references: &[Reference], tagged: &[Tagged]) -> Value {
    match json {
        Json::Object(fields) => {
            if fields.len() == 1 {
                if let Some(reference) = fields
                    .get(&keys.reference)
                    .and_then(Json::as_u64)
                    .and_then(|index| references.get(index as usize))
                {
                    return Value::Reference(reference.clone());
                }
                if let Some(value) = fields
                    .get(&keys.value)
                    .and_then(Json::as_u64)
                    .and_then(|index| tagged.get(index as usize))
                {
                    // A map's entries hold only markers recorded before the
                    // map's own: this ends.
                    let unmarked = |json: &Json| unmarked(json.clone(), keys, references, tagged);
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
                    .map(|(key, value)| (key, unmarked(value, keys, references, tagged)))
                    .collect(),
            )
        }
        Json::Array(values) => Value::List(
            values
                .into_iter()
                .map(|value| unmarked(value, keys, references, tagged))
                .collect(),
        ),
        json => Value::Data(json),
    }
}

/// The session value of what `produce` serializes: run inside an encode,
/// so the [`ObjectRef`]s, [`RemoteFunction`]s, tagged values and
/// [`Dynamic`]s it serializes cross as themselves, as in [`arg`]. For
/// serializations done elsewhere (an event's type-erased codec).
pub(crate) fn encode_with(produce: impl FnOnce() -> Result<Json, String>) -> Result<Value, Error> {
    let keys = Keys::new(NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let previous = ENCODING.with(|current| current.replace(Some(Vec::new())));
    let previous_tagged = ENCODING_TAGGED.with(|current| current.replace(Some(Vec::new())));
    let json = with_keys(&keys, produce);
    let references = ENCODING
        .with(|current| current.replace(previous))
        .unwrap_or_default();
    let tagged = ENCODING_TAGGED
        .with(|current| current.replace(previous_tagged))
        .unwrap_or_default();
    let json = json.map_err(Error::Value)?;
    if references.is_empty() && tagged.is_empty() {
        return Ok(Value::Data(json));
    }
    Ok(unmarked(json, &keys, &references, &tagged))
}

/// Decode `value` with `consume`, which deserializes from the JSON it is
/// given: run inside a decode, so references and tagged values reach the
/// types that take them ([`decode_value`] without its holder check, for
/// deserializations done elsewhere).
pub(crate) fn decode_with<R>(
    value: Value,
    consume: impl FnOnce(Json) -> Result<R, String>,
) -> Result<R, Error> {
    let mut taken = std::collections::HashSet::new();
    value_keys(&value, &mut taken);
    let keys = Keys::avoiding(&taken);
    let mut references = Vec::new();
    let mut tagged = Vec::new();
    let json = marked(value, &keys, &mut references, &mut tagged);
    decoding(&keys, references, tagged, || consume(json)).map_err(Error::Value)
}

/// Run `operation` as a decode with `keys` and these tables.
fn decoding<R>(
    keys: &Keys,
    references: Vec<Reference>,
    tagged: Vec<Tagged>,
    operation: impl FnOnce() -> R,
) -> R {
    let previous = DECODING.with(|current| current.replace(references));
    let previous_tagged = DECODING_TAGGED.with(|current| current.replace(tagged));
    let result = with_keys(keys, operation);
    DECODING.with(|current| current.replace(previous));
    DECODING_TAGGED.with(|current| current.replace(previous_tagged));
    result
}

/// What `consume` makes of what `produce` serializes, as if it had crossed
/// a session: references and tagged values pass from one to the other as
/// themselves. For converting one event type into another (the logical
/// bus's transcoder).
pub(crate) fn transcode_with<R>(
    produce: impl FnOnce() -> Result<Json, String>,
    consume: impl FnOnce(Json) -> Result<R, String>,
) -> Result<R, String> {
    let keys = Keys::new(NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let previous = ENCODING.with(|current| current.replace(Some(Vec::new())));
    let previous_tagged = ENCODING_TAGGED.with(|current| current.replace(Some(Vec::new())));
    let json = with_keys(&keys, produce);
    let references = ENCODING
        .with(|current| current.replace(previous))
        .unwrap_or_default();
    let tagged = ENCODING_TAGGED
        .with(|current| current.replace(previous_tagged))
        .unwrap_or_default();
    let json = json?;
    decoding(&keys, references, tagged, || consume(json))
}

/// Any session value inside serde data, kept as it is: in an encode (or a
/// transcode) its references and tagged values become markers of it, in a
/// decode they are read back from them. Outside both only plain data
/// serializes, and anything deserializes as data. Dynamic events
/// (`rutis_bridge::events::Args`) carry their arguments as these.
#[derive(Clone, Debug, PartialEq)]
pub struct Dynamic(pub Value);

impl Serialize for Dynamic {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(keys) = current_keys().filter(|_| ENCODING.with(|e| e.borrow().is_some())) else {
            return self
                .0
                .clone()
                .json()
                .map_err(ser::Error::custom)?
                .serialize(serializer);
        };
        if matches!(self.0, Value::Signal) {
            return Err(ser::Error::custom("a signal cannot be passed as data"));
        }
        let mut references = Vec::new();
        let mut tagged = Vec::new();
        // Recorded with indices after what this encode recorded already.
        let (base, base_tagged) = (
            ENCODING.with(|e| e.borrow().as_ref().map_or(0, Vec::len)),
            ENCODING_TAGGED.with(|e| e.borrow().as_ref().map_or(0, Vec::len)),
        );
        let json = marked_from(
            self.0.clone(),
            &keys,
            &mut references,
            &mut tagged,
            (base, base_tagged),
        );
        ENCODING.with(|e| {
            if let Some(list) = e.borrow_mut().as_mut() {
                list.extend(references);
            }
        });
        ENCODING_TAGGED.with(|e| {
            if let Some(list) = e.borrow_mut().as_mut() {
                list.extend(tagged);
            }
        });
        json.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Dynamic {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = Json::deserialize(deserializer)?;
        let Some(keys) = current_keys() else {
            return Ok(Self(Value::Data(json)));
        };
        let references = DECODING.with(|current| current.borrow().clone());
        let tagged = DECODING_TAGGED.with(|current| current.borrow().clone());
        Ok(Self(unmarked(json, &keys, &references, &tagged)))
    }
}

/// [`marked`] into tables that continue others: indices start at `base`
/// (references, tagged values).
fn marked_from(
    value: Value,
    keys: &Keys,
    references: &mut Vec<Reference>,
    tagged: &mut Vec<Tagged>,
    base: (usize, usize),
) -> Json {
    let mut local_references = Vec::new();
    let mut local_tagged = Vec::new();
    let json = marked(value, keys, &mut local_references, &mut local_tagged);
    let json = shift_markers(json, keys, base);
    let local_tagged = local_tagged
        .into_iter()
        .map(|entry| match entry {
            Tagged::Leaf(value) => Tagged::Leaf(value),
            Tagged::Map(entries) => Tagged::Map(
                entries
                    .into_iter()
                    .map(|(k, v)| (shift_markers(k, keys, base), shift_markers(v, keys, base)))
                    .collect(),
            ),
            Tagged::Set(elements) => Tagged::Set(
                elements
                    .into_iter()
                    .map(|json| shift_markers(json, keys, base))
                    .collect(),
            ),
        })
        .collect::<Vec<_>>();
    references.extend(local_references);
    tagged.extend(local_tagged);
    json
}

/// `json` with each marker of `keys` moved up by `base`.
fn shift_markers(json: Json, keys: &Keys, base: (usize, usize)) -> Json {
    match json {
        Json::Object(fields) => {
            if fields.len() == 1 {
                if let Some(index) = fields.get(&keys.reference).and_then(Json::as_u64) {
                    return keys.reference_marker(index as usize + base.0);
                }
                if let Some(index) = fields.get(&keys.value).and_then(Json::as_u64) {
                    return keys.value_marker(index as usize + base.1);
                }
            }
            Json::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, shift_markers(value, keys, base)))
                    .collect(),
            )
        }
        Json::Array(items) => Json::Array(
            items
                .into_iter()
                .map(|item| shift_markers(item, keys, base))
                .collect(),
        ),
        json => json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    /// Objects shaped as markers, under the keys of the operations about
    /// to run on any thread and the unnumbered keys of earlier versions,
    /// pointing at the first entries of the tables.
    fn forgeries() -> Vec<Json> {
        let next = NONCE.load(Ordering::Relaxed);
        let mut forged = vec![
            json!({ "\u{0}rutis:value": 0 }),
            json!({ "\u{0}rutis:reference": 0 }),
        ];
        for nonce in next..next + 32 {
            let keys = Keys::new(nonce);
            for index in 0..3 {
                forged.push(keys.value_marker(index));
                forged.push(keys.reference_marker(index));
            }
        }
        forged
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Payload {
        at: Instant,
        data: Json,
        counts: ValueMap<String, Json>,
        nested: ValueMap<String, ValueMap<String, Json>>,
        tags: ValueSet<String>,
    }

    fn payload() -> Payload {
        let forged = forgeries();
        let entries: Vec<(String, Json)> = forged
            .iter()
            .enumerate()
            .map(|(index, json)| (format!("k{index}"), json.clone()))
            .collect();
        Payload {
            at: "2026-10-15T03:22:15Z".parse().unwrap(),
            data: json!({ "list": forged.clone(), "deeper": { "x": forged[2].clone() } }),
            counts: ValueMap(entries.clone()),
            nested: ValueMap(vec![("inner".into(), ValueMap(entries))]),
            tags: ValueSet(
                forged
                    .iter()
                    .map(|json| json.as_object().unwrap().keys().next().unwrap().clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            ),
        }
    }

    /// User JSON whose objects look like markers, even under the very keys
    /// the next operations pick, is carried as data, top level and inside
    /// maps, sets and nested values: never read as a marker, and never
    /// sending the decode around a cycle.
    #[test]
    fn data_shaped_like_a_marker_stays_data() {
        let sent = payload();
        let value = arg(&sent).unwrap();
        let Value::Record(fields) = &value else {
            panic!("a record expected, got {value:?}")
        };
        assert_eq!(fields["at"], Value::Instant(sent.at));
        let Value::Map(counts) = &fields["counts"] else {
            panic!("a map expected")
        };
        for ((_, got), (_, json)) in counts.iter().zip(&sent.counts.0) {
            assert_eq!(
                got,
                &Value::Record(
                    json.as_object()
                        .unwrap()
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::Data(value.clone())))
                        .collect()
                )
            );
        }
        let back: Payload = decode_value(value).unwrap();
        assert_eq!(back, sent);

        // Received: plain data and the same data inside a map.
        for forged in forgeries() {
            let got: Json = decode_value(Value::Data(forged.clone())).unwrap();
            assert_eq!(got, forged);
            let map = Value::Map(vec![(Value::Data(json!("k")), Value::Data(forged.clone()))]);
            let got: ValueMap<String, Json> = decode_value(map).unwrap();
            assert_eq!(got.0, vec![("k".to_owned(), forged.clone())]);
            let list = Value::List(vec![
                Value::Instant("2026-10-15T03:22:15Z".parse().unwrap()),
                Value::Data(forged.clone()),
            ]);
            let (_, got): (Instant, Json) = decode_value(list).unwrap();
            assert_eq!(got, forged);
        }
    }
}
