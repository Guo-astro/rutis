//! Decoding a config into a Rust type, its errors written as config
//! problems (config schema design §3.3): a `Deserializer` over
//! `serde_json::Value` that records the path and the method called, so
//! serde's own text (and its internal type names) never shows.

use std::fmt;

use serde::de::{self, DeserializeOwned, DeserializeSeed, IntoDeserializer, Visitor};
use serde_json::{Map, Value};

use super::{closest, value_text, ConfigError, ConfigIssue, PathSegment};

/// Decode `value` into `T`. With `schema` (the config was validated against
/// the plugin's schema first), any error is `<path> is not a value the
/// plugin accepts`; without one, an error is written by its kind (wrong
/// type, missing field, unknown variant or field). No config (`null`) that
/// `T` does not take is decoded as `{}`.
pub fn decode<T: DeserializeOwned>(value: &Value, schema: bool) -> Result<T, ConfigError> {
    let first = T::deserialize(De::new(value.clone(), Vec::new()));
    let result = match (first, value) {
        (Ok(config), _) => return Ok(config),
        // No config where `T` is an object: as `{}`, so that a missing
        // field is named. Where `T` is not an object, the first error.
        (Err(error), Value::Null) => {
            match T::deserialize(De::new(Value::Object(Map::new()), Vec::new())) {
                Ok(config) => return Ok(config),
                Err(retry) if retry.at_root_type() => Err(error),
                Err(retry) => Err(retry),
            }
        }
        (Err(error), _) => Err(error),
    };
    result.map_err(|error| ConfigError::new(vec![error.issue(schema)]))
}

/// Why decoding failed.
#[derive(Debug)]
enum Kind {
    /// A value of the wrong type for the method called (`a string`).
    Type { expected: String, got: String },
    /// A field of a struct not written.
    Missing(&'static str),
    /// A variant name not among `expected`.
    Variant {
        got: String,
        expected: &'static [&'static str],
    },
    /// A key not among a struct's fields (`deny_unknown_fields`).
    Field {
        got: String,
        expected: &'static [&'static str],
    },
    /// Anything else.
    Other,
}

/// A decoding error: its kind, and where (set once, by the innermost
/// deserializer it passes).
#[derive(Debug)]
pub(crate) struct DecodeError {
    kind: Kind,
    path: Option<Vec<PathSegment>>,
}

impl DecodeError {
    fn new(kind: Kind) -> Self {
        DecodeError { kind, path: None }
    }

    /// Placed at `path`, unless it was placed already.
    fn at(mut self, path: &[PathSegment]) -> Self {
        if self.path.is_none() {
            self.path = Some(path.to_vec());
        }
        self
    }

    /// A wrong type at the root: `{}` is not what the type takes.
    fn at_root_type(&self) -> bool {
        matches!(self.kind, Kind::Type { .. }) && self.path.as_deref().is_none_or(<[_]>::is_empty)
    }

    /// The problem as a config issue.
    fn issue(self, schema: bool) -> ConfigIssue {
        let mut path = self.path.unwrap_or_default();
        if schema {
            if let Kind::Missing(field) = self.kind {
                path.push(PathSegment::Key(field.to_owned()));
            }
            return ConfigIssue::at(path, "is not a value the plugin accepts");
        }
        match self.kind {
            Kind::Type { expected, got } => {
                ConfigIssue::at(path, &format!("should be {expected}, got {got}"))
            }
            Kind::Missing(field) => {
                path.push(PathSegment::Key(field.to_owned()));
                ConfigIssue::at(path, "is required")
            }
            Kind::Variant { got, expected } => ConfigIssue::at(
                path,
                &format!(
                    "should be {}, got {}",
                    alternatives(expected),
                    value_text(&Value::String(got))
                ),
            ),
            Kind::Field { got, expected } => {
                let mut known: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
                known.sort();
                let reason = match closest(&got, &known) {
                    Some(like) => format!("is not a config key (did you mean {like}?)"),
                    None => "is not a config key".to_owned(),
                };
                ConfigIssue::at(path, &reason)
            }
            Kind::Other => ConfigIssue::at(path, "is not a value the plugin accepts"),
        }
    }
}

/// `"a"`, `"a" or "b"`, `one of "a", "b", "c"`.
fn alternatives(names: &[&str]) -> String {
    let texts: Vec<String> = names
        .iter()
        .map(|name| value_text(&Value::String((*name).to_owned())))
        .collect();
    match texts.as_slice() {
        [one] => one.clone(),
        [a, b] => format!("{a} or {b}"),
        _ => format!("one of {}", texts.join(", ")),
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.kind)
    }
}

impl std::error::Error for DecodeError {}

impl de::Error for DecodeError {
    fn custom<T: fmt::Display>(_: T) -> Self {
        DecodeError::new(Kind::Other)
    }

    fn missing_field(field: &'static str) -> Self {
        DecodeError::new(Kind::Missing(field))
    }

    fn unknown_variant(variant: &str, expected: &'static [&'static str]) -> Self {
        DecodeError::new(Kind::Variant {
            got: variant.to_owned(),
            expected,
        })
    }

    fn unknown_field(field: &str, expected: &'static [&'static str]) -> Self {
        DecodeError::new(Kind::Field {
            got: field.to_owned(),
            expected,
        })
    }
}

/// A value at a path.
struct De {
    value: Value,
    path: Vec<PathSegment>,
    /// An object's key: numbers and booleans are read from its text, as
    /// serde_json reads map keys (`HashMap<u32, _>` from `{"1": …}`).
    key: bool,
}

impl De {
    fn new(value: Value, path: Vec<PathSegment>) -> Self {
        De {
            value,
            path,
            key: false,
        }
    }

    fn key(key: String, path: Vec<PathSegment>) -> Self {
        De {
            value: Value::String(key),
            path,
            key: true,
        }
    }

    /// A key's text as the number or boolean it spells, if it does.
    fn parsed(&self) -> Option<Value> {
        let Value::String(text) = &self.value else {
            return None;
        };
        if !self.key {
            return None;
        }
        match text.as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => serde_json::from_str::<serde_json::Number>(text)
                .ok()
                .map(Value::Number),
        }
    }

    /// `value` is not what the method called (`expected`) takes.
    fn wrong(&self, expected: &str) -> DecodeError {
        DecodeError::new(Kind::Type {
            expected: expected.to_owned(),
            got: value_text(&self.value),
        })
        .at(&self.path)
    }

    fn integer<'de, V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        if let Some(Value::Number(n)) = self.parsed() {
            return De::new(Value::Number(n), self.path).integer(visitor);
        }
        let path = self.path.clone();
        let result = match &self.value {
            Value::Number(n) if n.is_u64() => visitor.visit_u64(n.as_u64().unwrap()),
            Value::Number(n) if n.is_i64() => visitor.visit_i64(n.as_i64().unwrap()),
            Value::Number(n) => match n.as_f64() {
                Some(f) if f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0 => {
                    visitor.visit_i64(f as i64)
                }
                _ => return Err(self.wrong("an integer")),
            },
            _ => return Err(self.wrong("an integer")),
        };
        result.map_err(|e: DecodeError| e.at(&path))
    }
}

macro_rules! integers {
    ($($method:ident)*) => {
        $(fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
            self.integer(visitor)
        })*
    };
}

impl<'de> de::Deserializer<'de> for De {
    type Error = DecodeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        let path = self.path.clone();
        let result = match self.value {
            Value::Null => visitor.visit_unit(),
            Value::Bool(b) => visitor.visit_bool(b),
            Value::Number(n) => match (n.as_u64(), n.as_i64(), n.as_f64()) {
                (Some(u), _, _) => visitor.visit_u64(u),
                (_, Some(i), _) => visitor.visit_i64(i),
                (_, _, Some(f)) => visitor.visit_f64(f),
                _ => Err(DecodeError::new(Kind::Other)),
            },
            Value::String(s) => visitor.visit_string(s),
            Value::Array(items) => visitor.visit_seq(Seq::new(items, path.clone())),
            Value::Object(map) => visitor.visit_map(Fields::new(map, path.clone())),
        };
        result.map_err(|e: DecodeError| e.at(&path))
    }

    integers!(deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128);

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        if let Some(Value::Bool(b)) = self.parsed() {
            return visitor
                .visit_bool(b)
                .map_err(|e: DecodeError| e.at(&self.path));
        }
        match self.value {
            Value::Bool(b) => visitor
                .visit_bool(b)
                .map_err(|e: DecodeError| e.at(&self.path)),
            _ => Err(self.wrong("a boolean")),
        }
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        if let Some(Value::Number(n)) = self.parsed() {
            return De::new(Value::Number(n), self.path).deserialize_f64(visitor);
        }
        match self.value.as_f64() {
            Some(f) => visitor
                .visit_f64(f)
                .map_err(|e: DecodeError| e.at(&self.path)),
            None => Err(self.wrong("a number")),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            Value::String(s) => visitor
                .visit_string(s)
                .map_err(|e: DecodeError| e.at(&self.path)),
            _ => Err(self.wrong("a string")),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            Value::String(_) => self.deserialize_string(visitor),
            _ => self.deserialize_seq(visitor),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        let path = self.path.clone();
        match self.value {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
        .map_err(|e: DecodeError| e.at(&path))
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            Value::Null => visitor
                .visit_unit()
                .map_err(|e: DecodeError| e.at(&self.path)),
            _ => Err(self.wrong("null")),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        let path = self.path.clone();
        visitor
            .visit_newtype_struct(self)
            .map_err(|e: DecodeError| e.at(&path))
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            Value::Array(items) => visitor
                .visit_seq(Seq::new(items, self.path.clone()))
                .map_err(|e: DecodeError| e.at(&self.path)),
            _ => Err(self.wrong("an array")),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            Value::Object(map) => visitor
                .visit_map(Fields::new(map, self.path.clone()))
                .map_err(|e: DecodeError| e.at(&self.path)),
            _ => Err(self.wrong("an object")),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        // A struct from an array of its fields, as serde_json takes it.
        match self.value {
            Value::Array(_) => self.deserialize_seq(visitor),
            _ => self.deserialize_map(visitor),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        let path = self.path.clone();
        let (variant, value) = match self.value {
            Value::String(variant) => (variant, None),
            Value::Object(map) if map.len() == 1 => {
                let (variant, value) = map.into_iter().next().expect("one entry");
                (variant, Some(value))
            }
            _ => return Err(self.wrong(&alternatives(variants))),
        };
        visitor
            .visit_enum(Variant {
                variant,
                value,
                path: path.clone(),
            })
            .map_err(|e: DecodeError| e.at(&path))
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        visitor.visit_unit()
    }
}

/// An array's items, each at its index.
struct Seq {
    items: std::vec::IntoIter<Value>,
    index: usize,
    path: Vec<PathSegment>,
}

impl Seq {
    fn new(items: Vec<Value>, path: Vec<PathSegment>) -> Self {
        Seq {
            items: items.into_iter(),
            index: 0,
            path,
        }
    }
}

impl<'de> de::SeqAccess<'de> for Seq {
    type Error = DecodeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, DecodeError> {
        let Some(item) = self.items.next() else {
            return Ok(None);
        };
        let mut path = self.path.clone();
        path.push(PathSegment::Index(self.index));
        self.index += 1;
        seed.deserialize(De::new(item, path)).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len())
    }
}

/// An object's entries, each value at its key.
struct Fields {
    entries: serde_json::map::IntoIter,
    value: Option<(String, Value)>,
    path: Vec<PathSegment>,
}

impl Fields {
    fn new(map: Map<String, Value>, path: Vec<PathSegment>) -> Self {
        Fields {
            entries: map.into_iter(),
            value: None,
            path,
        }
    }
}

impl<'de> de::MapAccess<'de> for Fields {
    type Error = DecodeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DecodeError> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        let mut path = self.path.clone();
        path.push(PathSegment::Key(key.clone()));
        let decoded = seed.deserialize(De::key(key.clone(), path))?;
        self.value = Some((key, value));
        Ok(Some(decoded))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, DecodeError> {
        let (key, value) = self.value.take().expect("a key first");
        let mut path = self.path.clone();
        path.push(PathSegment::Key(key));
        seed.deserialize(De::new(value, path))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

/// An enum's variant (a name, or an object of one key) and its value.
struct Variant {
    variant: String,
    value: Option<Value>,
    path: Vec<PathSegment>,
}

impl<'de> de::EnumAccess<'de> for Variant {
    type Error = DecodeError;
    type Variant = Content;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Content), DecodeError> {
        let name = De::new(Value::String(self.variant.clone()), self.path.clone());
        let decoded = seed.deserialize(name)?;
        let mut path = self.path;
        if self.value.is_some() {
            path.push(PathSegment::Key(self.variant));
        }
        Ok((
            decoded,
            Content {
                value: self.value,
                path,
            },
        ))
    }
}

/// The value of an enum's variant.
struct Content {
    value: Option<Value>,
    path: Vec<PathSegment>,
}

impl Content {
    fn de(self) -> De {
        De::new(self.value.unwrap_or(Value::Null), self.path)
    }
}

impl<'de> de::VariantAccess<'de> for Content {
    type Error = DecodeError;

    fn unit_variant(self) -> Result<(), DecodeError> {
        match &self.value {
            None | Some(Value::Null) => Ok(()),
            Some(_) => Err(self.de().wrong("null")),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, DecodeError> {
        seed.deserialize(self.de())
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        de::Deserializer::deserialize_seq(self.de(), visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        de::Deserializer::deserialize_map(self.de(), visitor)
    }
}

impl<'de> IntoDeserializer<'de, DecodeError> for De {
    type Deserializer = De;

    fn into_deserializer(self) -> De {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::decode;
    use serde::Deserialize;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[derive(Debug, Deserialize, PartialEq)]
    #[allow(dead_code)]
    struct Weather {
        city: String,
        #[serde(default)]
        days: u32,
        #[serde(default)]
        units: Option<Units>,
        #[serde(default)]
        home: Option<Home>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        extra: BTreeMap<String, u8>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum Units {
        Metric,
        Imperial,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Home {
        lat: f64,
    }

    fn text(value: serde_json::Value, schema: bool) -> String {
        decode::<Weather>(&value, schema).unwrap_err().to_string()
    }

    /// Without a schema, each kind of decoder error is written as a
    /// config problem (config schema design §3.3).
    #[test]
    fn decoder_errors_are_written_by_kind() {
        assert_eq!(
            text(json!({ "city": 42 }), false),
            "config: city should be a string, got 42"
        );
        assert_eq!(text(json!({}), false), "config: city is required");
        assert_eq!(
            text(json!({ "city": "a", "days": 1.5 }), false),
            "config: days should be an integer, got 1.5"
        );
        assert_eq!(
            text(json!({ "city": "a", "units": "kelvin" }), false),
            "config: units should be \"metric\" or \"imperial\", got \"kelvin\""
        );
        assert_eq!(
            text(json!({ "city": "a", "units": 3 }), false),
            "config: units should be \"metric\" or \"imperial\", got 3"
        );
        assert_eq!(
            text(
                json!({ "city": "a", "home": { "lat": 1, "lot": 2 } }),
                false
            ),
            "config: home.lot is not a config key (did you mean lat?)"
        );
        assert_eq!(
            text(json!({ "city": "a", "home": {} }), false),
            "config: home.lat is required"
        );
        assert_eq!(
            text(json!({ "city": "a", "tags": ["x", 2] }), false),
            "config: tags[1] should be a string, got 2"
        );
        assert_eq!(
            text(json!({ "city": "a", "days": -1 }), false),
            "config: days is not a value the plugin accepts"
        );
        assert_eq!(
            text(json!({ "city": "a", "extra": { "x-y": 300 } }), false),
            "config: extra[\"x-y\"] is not a value the plugin accepts"
        );
        assert_eq!(
            text(json!("Oslo"), false),
            "config: config should be an object, got \"Oslo\""
        );
    }

    /// With a schema, a decoder error means the schema and the type
    /// disagree: one sentence.
    #[test]
    fn with_a_schema_one_sentence() {
        assert_eq!(
            text(json!({ "city": 42 }), true),
            "config: city is not a value the plugin accepts"
        );
        assert_eq!(
            text(json!({}), true),
            "config: city is not a value the plugin accepts"
        );
    }

    #[test]
    fn values_decode() {
        let weather: Weather = decode(
            &json!({ "city": "Oslo", "days": 3.0, "units": "metric", "home": { "lat": 1 } }),
            false,
        )
        .unwrap();
        assert_eq!(weather.days, 3);
        assert_eq!(weather.units, Some(Units::Metric));
        assert_eq!(weather.home, Some(Home { lat: 1.0 }));
        // No config decodes as {} where that works.
        #[derive(Deserialize, Default, Debug, PartialEq)]
        struct Empty {
            #[serde(default)]
            n: u32,
        }
        assert_eq!(
            decode::<Empty>(&json!(null), false).unwrap(),
            Empty::default()
        );
        assert_eq!(
            decode::<Weather>(&json!(null), false)
                .unwrap_err()
                .to_string(),
            "config: city is required"
        );
        let raw: serde_json::Value = decode(&json!({ "a": [1, "b"] }), false).unwrap();
        assert_eq!(raw, json!({ "a": [1, "b"] }));
        // Keys read as numbers and booleans, as serde_json reads them.
        let numbered: BTreeMap<u32, String> = decode(&json!({ "1": "a" }), false).unwrap();
        assert_eq!(numbered[&1], "a");
        let flags: BTreeMap<bool, u8> = decode(&json!({ "true": 1 }), false).unwrap();
        assert_eq!(flags[&true], 1);
        assert_eq!(
            decode::<BTreeMap<u32, u8>>(&json!({ "x": 1 }), false)
                .unwrap_err()
                .to_string(),
            "config: x should be an integer, got \"x\""
        );
        // A struct from an array of its fields.
        let home: Home = decode(&json!([2.5]), false).unwrap();
        assert_eq!(home, Home { lat: 2.5 });
        // No config where the type is not an object: its own error.
        assert_eq!(
            decode::<Vec<String>>(&json!(null), false)
                .unwrap_err()
                .to_string(),
            "config: config should be an array, got null"
        );
    }
}
