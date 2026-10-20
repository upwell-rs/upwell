//! Shape recovery by tracing a type's serde `Deserialize` impl.
//!
//! A [`Tracer`] answers every `deserialize_*` request with the simplest valid value while
//! recording what was requested, so a type's own impl reveals its fields, scalars, and
//! collections — including types that never passed through `#[config]`.

use serde::de::value::BorrowedStrDeserializer;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess,
    Visitor,
};

use super::{TracedField, ValueSchema};

/// Bounds nesting so a recursive type (`Option<Box<Self>>`) terminates.
const MAX_DEPTH: usize = 16;

/// The traced shape of `T`.
///
/// Best effort: a self-describing request (`deserialize_any`, as untagged and internally tagged
/// enums issue) or a value the type's own validation rejects ends tracing of that position as
/// [`ValueSchema::Any`], keeping everything recorded before it.
pub fn trace<T: DeserializeOwned>() -> ValueSchema {
    let mut schema = ValueSchema::Any;

    if T::deserialize(Tracer::new(&mut schema, 0)).is_err() {
        tracing::trace!(
            target: "upwell::config",
            ty = std::any::type_name::<T>(),
            "config schema trace ended early"
        );
    }

    schema
}

/// Why tracing stopped at a position. Never surfaced: the recorded shape is the result.
#[derive(Debug)]
struct TraceStop(String);

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
struct Tracer<'a> {
    out: &'a mut ValueSchema,
    depth: usize,
}

impl<'a> Tracer<'a> {
    fn new(out: &'a mut ValueSchema, depth: usize) -> Self {
        Self { out, depth }
    }

    fn record(self, schema: ValueSchema) -> Result<Self, TraceStop> {
        if self.depth > MAX_DEPTH {
            return Err(TraceStop(String::from("maximum trace depth")));
        }

        *self.out = schema;

        Ok(self)
    }

    /// Traces one nested value into `out`, discarding the produced value.
    fn nested<'de, S: DeserializeSeed<'de>>(
        seed: S,
        out: &mut ValueSchema,
        depth: usize,
    ) -> Result<S::Value, TraceStop> {
        seed.deserialize(Tracer::new(out, depth + 1))
    }
}

macro_rules! scalar {
    ($($method:ident => $schema:expr, $visit:ident($($value:expr)?);)+) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
                self.record($schema)?;

                visitor.$visit($($value)?)
            }
        )+
    };
}

impl<'de> de::Deserializer<'de> for Tracer<'_> {
    type Error = TraceStop;

    scalar! {
        deserialize_bool => ValueSchema::Bool, visit_bool(false);
        deserialize_i8 => ValueSchema::Integer { signed: true }, visit_i8(0);
        deserialize_i16 => ValueSchema::Integer { signed: true }, visit_i16(0);
        deserialize_i32 => ValueSchema::Integer { signed: true }, visit_i32(0);
        deserialize_i64 => ValueSchema::Integer { signed: true }, visit_i64(0);
        deserialize_i128 => ValueSchema::Integer { signed: true }, visit_i128(0);
        deserialize_u8 => ValueSchema::Integer { signed: false }, visit_u8(0);
        deserialize_u16 => ValueSchema::Integer { signed: false }, visit_u16(0);
        deserialize_u32 => ValueSchema::Integer { signed: false }, visit_u32(0);
        deserialize_u64 => ValueSchema::Integer { signed: false }, visit_u64(0);
        deserialize_u128 => ValueSchema::Integer { signed: false }, visit_u128(0);
        deserialize_f32 => ValueSchema::Float, visit_f32(0.0);
        deserialize_f64 => ValueSchema::Float, visit_f64(0.0);
        deserialize_char => ValueSchema::Char, visit_char('a');
        deserialize_str => ValueSchema::String, visit_borrowed_str("");
        deserialize_string => ValueSchema::String, visit_borrowed_str("");
        deserialize_bytes => ValueSchema::Bytes, visit_borrowed_bytes(&[]);
        deserialize_byte_buf => ValueSchema::Bytes, visit_borrowed_bytes(&[]);
        deserialize_unit => ValueSchema::Unit, visit_unit();
        deserialize_identifier => ValueSchema::String, visit_borrowed_str("");
        deserialize_ignored_any => ValueSchema::Any, visit_unit();
    }

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, TraceStop> {
        self.record(ValueSchema::Any)?;

        Err(TraceStop(String::from("self-describing value")))
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceStop> {
        let tracer = self.record(ValueSchema::Optional(Box::new(ValueSchema::Any)))?;
        let ValueSchema::Optional(inner) = tracer.out else {
            unreachable!("recorded as optional above");
        };

        visitor.visit_some(Tracer::new(inner, tracer.depth + 1))
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

        visitor.visit_seq(OneElement {
            out: Some(item),
            depth: tracer.depth,
        })
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

        visitor.visit_seq(Elements {
            items: items.iter_mut(),
            depth: tracer.depth,
        })
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

        visitor.visit_map(OneEntry {
            value: Some(value),
            depth: tracer.depth,
        })
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

        visitor.visit_map(Fields {
            fields: fields.iter_mut(),
            current: None,
            depth: tracer.depth,
        })
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
        let first = variants
            .first()
            .ok_or_else(|| TraceStop(String::from("enum without variants")))?;

        visitor.visit_enum(FirstVariant {
            variant: first,
            depth: tracer.depth,
        })
    }
}

/// A sequence of exactly one traced element.
struct OneElement<'a> {
    out: Option<&'a mut ValueSchema>,
    depth: usize,
}

impl<'de> SeqAccess<'de> for OneElement<'_> {
    type Error = TraceStop;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, TraceStop> {
        match self.out.take() {
            Some(out) => Tracer::nested(seed, out, self.depth).map(Some),
            None => Ok(None),
        }
    }
}

/// A fixed-length sequence whose elements are traced in order.
struct Elements<'a> {
    items: std::slice::IterMut<'a, ValueSchema>,
    depth: usize,
}

impl<'de> SeqAccess<'de> for Elements<'_> {
    type Error = TraceStop;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, TraceStop> {
        match self.items.next() {
            Some(out) => Tracer::nested(seed, out, self.depth).map(Some),
            None => Ok(None),
        }
    }
}

/// A map of exactly one entry: an untraced key and a traced value.
struct OneEntry<'a> {
    value: Option<&'a mut ValueSchema>,
    depth: usize,
}

impl<'de> MapAccess<'de> for OneEntry<'_> {
    type Error = TraceStop;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TraceStop> {
        if self.value.is_none() {
            return Ok(None);
        }

        let mut key = ValueSchema::Any;

        Tracer::nested(seed, &mut key, self.depth).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, TraceStop> {
        let out = self
            .value
            .take()
            .ok_or_else(|| TraceStop(String::from("map value without key")))?;

        Tracer::nested(seed, out, self.depth)
    }
}

/// A struct's fields, each offered by name and traced in turn.
struct Fields<'a> {
    fields: std::slice::IterMut<'a, TracedField>,
    current: Option<&'a mut ValueSchema>,
    depth: usize,
}

impl<'de> MapAccess<'de> for Fields<'_> {
    type Error = TraceStop;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TraceStop> {
        let Some(field) = self.fields.next() else {
            return Ok(None);
        };
        let key = seed.deserialize(BorrowedStrDeserializer::<TraceStop>::new(field.name))?;

        self.current = Some(&mut field.value);

        Ok(Some(key))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, TraceStop> {
        let out = self
            .current
            .take()
            .ok_or_else(|| TraceStop(String::from("field value without key")))?;

        Tracer::nested(seed, out, self.depth)
    }
}

/// Selects an enum's first variant so tracing can continue past it.
struct FirstVariant {
    variant: &'static str,
    depth: usize,
}

impl<'de> EnumAccess<'de> for FirstVariant {
    type Error = TraceStop;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), TraceStop> {
        let tag = seed.deserialize(BorrowedStrDeserializer::<TraceStop>::new(self.variant))?;

        Ok((tag, self))
    }
}

impl<'de> VariantAccess<'de> for FirstVariant {
    type Error = TraceStop;

    fn unit_variant(self) -> Result<(), TraceStop> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, TraceStop> {
        let mut payload = ValueSchema::Any;

        Tracer::nested(seed, &mut payload, self.depth)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, TraceStop> {
        let mut payload = ValueSchema::Any;

        de::Deserializer::deserialize_tuple(Tracer::new(&mut payload, self.depth + 1), len, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        let mut payload = ValueSchema::Any;

        de::Deserializer::deserialize_struct(
            Tracer::new(&mut payload, self.depth + 1),
            self.variant,
            fields,
            visitor,
        )
    }
}
