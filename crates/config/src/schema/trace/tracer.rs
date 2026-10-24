//! The recording deserializer.

use serde::de::{self, Visitor};

use super::access::{Elements, Fields, FirstVariant, OneElement, OneEntry};
use super::{TraceState, TracedField, ValueSchema};

/// Bounds nesting so a recursive type (`Option<Box<Self>>`) terminates.
const MAX_DEPTH: usize = 16;

/// String values offered in turn until the type's own parsing accepts one: empty first, then
/// common shapes of parsed strings (addresses, durations, identifiers, URLs, paths, ids, times).
const STRINGS: &[&str] = &[
    "",
    "0.0.0.0",
    "127.0.0.1:0",
    "1s",
    "a",
    "0",
    "true",
    "http://localhost/",
    "/",
    "00000000-0000-0000-0000-000000000000",
    "1970-01-01T00:00:00Z",
];

/// Why tracing stopped at a position. Never surfaced: the recorded shape is the result.
#[derive(Debug)]
pub(super) struct TraceStop(String);

impl TraceStop {
    pub(super) fn new(reason: &str) -> Self {
        Self(reason.to_string())
    }
}

impl std::fmt::Display for TraceStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TraceStop {}

impl de::Error for TraceStop {
    fn custom<T: std::fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

/// A deserializer recording the shape requested of it into `out`.
pub(super) struct Tracer<'a, 's> {
    out: &'a mut ValueSchema,
    depth: usize,
    state: &'s TraceState,
}

impl<'a, 's> Tracer<'a, 's> {
    pub(super) fn new(out: &'a mut ValueSchema, depth: usize, state: &'s TraceState) -> Self {
        Self { out, depth, state }
    }

    fn record(self, schema: ValueSchema) -> Result<Self, TraceStop> {
        if self.depth > MAX_DEPTH {
            return Err(TraceStop::new("maximum trace depth"));
        }

        *self.out = schema;

        Ok(self)
    }

    /// Records `schema`, offers the current candidate of `candidates` through `visit`, and reports
    /// a rejection so the next run offers the next candidate.
    fn scalar<'de, V, T>(
        self,
        schema: ValueSchema,
        candidates: &[T],
        visitor: V,
        visit: impl FnOnce(V, T) -> Result<V::Value, TraceStop>,
    ) -> Result<V::Value, TraceStop>
    where
        V: Visitor<'de>,
        T: Copy,
    {
        let state = self.state;
        let value = candidates[state.choice().min(candidates.len() - 1)];

        self.record(schema.clone())?;

        visit(visitor, value).inspect_err(|_| state.reject(schema, candidates.len()))
    }
}

macro_rules! scalar {
    ($($method:ident => $schema:expr, $candidates:expr, $visit:ident;)+) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
                self.scalar($schema, $candidates, visitor, |visitor, value| visitor.$visit(value))
            }
        )+
    };
}

impl<'de> de::Deserializer<'de> for Tracer<'_, '_> {
    type Error = TraceStop;

    scalar! {
        deserialize_bool => ValueSchema::Bool, &[false, true], visit_bool;
        deserialize_i8 => ValueSchema::Integer { signed: true }, &[0, 1], visit_i8;
        deserialize_i16 => ValueSchema::Integer { signed: true }, &[0, 1], visit_i16;
        deserialize_i32 => ValueSchema::Integer { signed: true }, &[0, 1], visit_i32;
        deserialize_i64 => ValueSchema::Integer { signed: true }, &[0, 1], visit_i64;
        deserialize_i128 => ValueSchema::Integer { signed: true }, &[0, 1], visit_i128;
        deserialize_u8 => ValueSchema::Integer { signed: false }, &[0, 1], visit_u8;
        deserialize_u16 => ValueSchema::Integer { signed: false }, &[0, 1], visit_u16;
        deserialize_u32 => ValueSchema::Integer { signed: false }, &[0, 1], visit_u32;
        deserialize_u64 => ValueSchema::Integer { signed: false }, &[0, 1], visit_u64;
        deserialize_u128 => ValueSchema::Integer { signed: false }, &[0, 1], visit_u128;
        deserialize_f32 => ValueSchema::Float, &[0.0, 1.0], visit_f32;
        deserialize_f64 => ValueSchema::Float, &[0.0, 1.0], visit_f64;
        deserialize_char => ValueSchema::Char, &['a', '0'], visit_char;
        deserialize_str => ValueSchema::String, STRINGS, visit_borrowed_str;
        deserialize_string => ValueSchema::String, STRINGS, visit_borrowed_str;
        deserialize_identifier => ValueSchema::String, STRINGS, visit_borrowed_str;
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        self.record(ValueSchema::Bytes)?;

        visitor.visit_borrowed_bytes(&[])
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        self.record(ValueSchema::Unit)?;

        visitor.visit_unit()
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        visitor.visit_unit()
    }

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, TraceStop> {
        self.record(ValueSchema::Any)?;

        Err(TraceStop::new("self-describing value"))
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        let tracer = self.record(ValueSchema::Optional(Box::new(ValueSchema::Any)))?;
        let ValueSchema::Optional(inner) = tracer.out else {
            unreachable!("recorded as optional above");
        };

        visitor.visit_some(Tracer::new(inner, tracer.depth + 1, tracer.state))
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        let tracer = self.record(ValueSchema::Array(Box::new(ValueSchema::Any)))?;
        let ValueSchema::Array(item) = tracer.out else {
            unreachable!("recorded as an array above");
        };

        visitor.visit_seq(OneElement::new(item, tracer.depth, tracer.state))
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        let tracer = self.record(ValueSchema::Tuple(vec![ValueSchema::Any; len]))?;
        let ValueSchema::Tuple(items) = tracer.out else {
            unreachable!("recorded as a tuple above");
        };

        visitor.visit_seq(Elements::new(items, tracer.depth, tracer.state))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        let tracer = self.record(ValueSchema::Map(Box::new(ValueSchema::Any)))?;
        let ValueSchema::Map(value) = tracer.out else {
            unreachable!("recorded as a map above");
        };

        visitor.visit_map(OneEntry::new(value, tracer.depth, tracer.state))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        let fields = fields
            .iter()
            .map(|name| TracedField {
                name,
                value: ValueSchema::Any,
            })
            .collect();
        let tracer = self.record(ValueSchema::Struct { name, fields })?;
        let ValueSchema::Struct { fields, .. } = tracer.out else {
            unreachable!("recorded as a struct above");
        };

        visitor.visit_map(Fields::new(name, fields, tracer.depth, tracer.state))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        let tracer = self.record(ValueSchema::Enum {
            name,
            variants: variants.to_vec(),
        })?;
        let variant = variants
            .get(tracer.state.choice())
            .or(variants.first())
            .ok_or_else(|| TraceStop::new("enum without variants"))?;
        let state = tracer.state;

        visitor
            .visit_enum(FirstVariant::new(variant, tracer.depth, state))
            .inspect_err(|_| {
                state.reject(
                    ValueSchema::Enum {
                        name,
                        variants: variants.to_vec(),
                    },
                    variants.len(),
                )
            })
    }
}
