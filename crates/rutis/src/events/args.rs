//! A value as event arguments (protocol spec §12.1): the top-level shape
//! decides how many arguments it is. A tuple or tuple struct of two or more
//! fields is one argument per field, a unit or unit struct none, anything
//! else one argument. Everything below the top level is plain JSON.

use serde::ser::{self, Serialize};
use serde_json::value::Serializer as JsonSerializer;
use serde_json::{Error, Value as Json};

pub(crate) struct ArgsSerializer;

fn one(json: Result<Json, Error>) -> Result<Vec<Json>, Error> {
    json.map(|json| vec![json])
}

pub(crate) struct Fields {
    fields: Vec<Json>,
}

impl ser::SerializeTuple for Fields {
    type Ok = Vec<Json>;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.fields.push(serde_json::to_value(value)?);
        Ok(())
    }
    fn end(self) -> Result<Vec<Json>, Error> {
        Ok(self.fields)
    }
}

impl ser::SerializeTupleStruct for Fields {
    type Ok = Vec<Json>;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeTuple::serialize_element(self, value)
    }
    fn end(self) -> Result<Vec<Json>, Error> {
        Ok(self.fields)
    }
}

/// Wraps one of serde_json's compound serializers: the result is one argument.
pub(crate) struct One<S>(S);

macro_rules! one_compound {
    ($trait:ident, $method:ident $(, $key:ident)?) => {
        impl<S: ser::$trait<Ok = Json, Error = Error>> ser::$trait for One<S> {
            type Ok = Vec<Json>;
            type Error = Error;
            fn $method<T: Serialize + ?Sized>(&mut self, $($key: &'static str,)? value: &T) -> Result<(), Error> {
                self.0.$method($($key,)? value)
            }
            fn end(self) -> Result<Vec<Json>, Error> {
                one(self.0.end())
            }
        }
    };
}

one_compound!(SerializeSeq, serialize_element);
one_compound!(SerializeTupleVariant, serialize_field);
one_compound!(SerializeStruct, serialize_field, key);
one_compound!(SerializeStructVariant, serialize_field, key);

impl<S: ser::SerializeMap<Ok = Json, Error = Error>> ser::SerializeMap for One<S> {
    type Ok = Vec<Json>;
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        self.0.serialize_key(key)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.0.serialize_value(value)
    }
    fn end(self) -> Result<Vec<Json>, Error> {
        one(self.0.end())
    }
}

/// A tuple of fewer than two fields is one argument (an array).
pub(crate) enum Tuple {
    Fields(Fields),
    One(Vec<Json>),
}

impl ser::SerializeTuple for Tuple {
    type Ok = Vec<Json>;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        match self {
            Tuple::Fields(fields) => ser::SerializeTuple::serialize_element(fields, value),
            Tuple::One(items) => {
                items.push(serde_json::to_value(value)?);
                Ok(())
            }
        }
    }
    fn end(self) -> Result<Vec<Json>, Error> {
        match self {
            Tuple::Fields(fields) => Ok(fields.fields),
            Tuple::One(items) => Ok(vec![Json::Array(items)]),
        }
    }
}

impl ser::SerializeTupleStruct for Tuple {
    type Ok = Vec<Json>;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeTuple::serialize_element(self, value)
    }
    fn end(self) -> Result<Vec<Json>, Error> {
        ser::SerializeTuple::end(self)
    }
}

fn tuple(len: usize) -> Tuple {
    if len >= 2 {
        Tuple::Fields(Fields {
            fields: Vec::with_capacity(len),
        })
    } else {
        Tuple::One(Vec::new())
    }
}

macro_rules! scalar {
    ($($method:ident($ty:ty)),* $(,)?) => {
        $(fn $method(self, v: $ty) -> Result<Vec<Json>, Error> {
            one(JsonSerializer.$method(v))
        })*
    };
}

impl ser::Serializer for ArgsSerializer {
    type Ok = Vec<Json>;
    type Error = Error;
    type SerializeSeq = One<<JsonSerializer as ser::Serializer>::SerializeSeq>;
    type SerializeTuple = Tuple;
    type SerializeTupleStruct = Tuple;
    type SerializeTupleVariant = One<<JsonSerializer as ser::Serializer>::SerializeTupleVariant>;
    type SerializeMap = One<<JsonSerializer as ser::Serializer>::SerializeMap>;
    type SerializeStruct = One<<JsonSerializer as ser::Serializer>::SerializeStruct>;
    type SerializeStructVariant = One<<JsonSerializer as ser::Serializer>::SerializeStructVariant>;

    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
    );

    fn serialize_none(self) -> Result<Vec<Json>, Error> {
        one(JsonSerializer.serialize_none())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Vec<Json>, Error> {
        one(serde_json::to_value(value))
    }
    fn serialize_unit(self) -> Result<Vec<Json>, Error> {
        Ok(Vec::new())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Vec<Json>, Error> {
        Ok(Vec::new())
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<Vec<Json>, Error> {
        one(JsonSerializer.serialize_unit_variant(name, index, variant))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Vec<Json>, Error> {
        one(JsonSerializer.serialize_newtype_struct(name, value))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Vec<Json>, Error> {
        one(JsonSerializer.serialize_newtype_variant(name, index, variant, value))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        JsonSerializer.serialize_seq(len).map(One)
    }
    fn serialize_tuple(self, len: usize) -> Result<Tuple, Error> {
        Ok(tuple(len))
    }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Tuple, Error> {
        Ok(tuple(len))
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        JsonSerializer
            .serialize_tuple_variant(name, index, variant, len)
            .map(One)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        JsonSerializer.serialize_map(len).map(One)
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        JsonSerializer.serialize_struct(name, len).map(One)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        JsonSerializer
            .serialize_struct_variant(name, index, variant, len)
            .map(One)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Pair(String, i64);
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct City {
        city: String,
    }
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Unit;
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Single(i64);
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct OneTuple((i64,));

    fn args<T: Serialize>(value: &T) -> Vec<Json> {
        value.serialize(ArgsSerializer).unwrap()
    }

    /// guarantee: events/12.1-args
    #[test]
    fn top_level_shape_decides_the_arguments() {
        assert_eq!(args(&Pair("x".into(), 2)), vec![json!("x"), json!(2)]);
        assert_eq!(
            args(&("x", 2, true)),
            vec![json!("x"), json!(2), json!(true)]
        );
        assert_eq!(
            args(&City {
                city: "Oslo".into()
            }),
            vec![json!({"city": "Oslo"})]
        );
        assert_eq!(args(&Unit), Vec::<Json>::new());
        assert_eq!(args(&()), Vec::<Json>::new());
        assert_eq!(args(&Single(3)), vec![json!(3)]);
        assert_eq!(args(&vec![1, 2]), vec![json!([1, 2])]);
        assert_eq!(args(&(5,)), vec![json!([5])]);
        assert_eq!(args(&Some(1)), vec![json!(1)]);
        let codec = crate::events::Codec::of::<Pair>();
        let back = codec.from_args(vec![json!("x"), json!(2)]).unwrap();
        assert_eq!(*back.downcast::<Pair>().unwrap(), Pair("x".into(), 2));
        let unit = crate::events::Codec::of::<Unit>()
            .from_args(vec![])
            .unwrap();
        assert_eq!(*unit.downcast::<Unit>().unwrap(), Unit);
        let city = crate::events::Codec::of::<City>()
            .from_args(vec![json!({"city": "Oslo"})])
            .unwrap();
        assert_eq!(city.downcast::<City>().unwrap().city, "Oslo");
    }
}
