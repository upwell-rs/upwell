//! The sequence, map, struct, and enum accessors a [`Tracer`] hands to visitors.

use serde::de::value::BorrowedStrDeserializer;
use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};

use super::tracer::{TraceStop, Tracer};
use super::{TraceState, TracedField, ValueSchema};

/// Traces one value nested under the path `segment`, discarding the produced value. A failure
/// not already attributed to a scalar inside rejects the whole position, so the next run skips
/// it with what was recorded.
fn nested<'de, S: DeserializeSeed<'de>>(
    seed: S,
    out: &mut ValueSchema,
    depth: usize,
    state: &TraceState,
    segment: &str,
) -> Result<S::Value, TraceStop> {
    state.enter(segment);

    let traced = seed.deserialize(Tracer::new(out, depth + 1, state));

    if traced.is_err() {
        state.reject(out.clone(), 1);
    }

    state.leave();

    traced
}

/// A sequence of exactly one traced element.
pub(super) struct OneElement<'a, 's> {
    out: Option<&'a mut ValueSchema>,
    depth: usize,
    state: &'s TraceState,
}

impl<'a, 's> OneElement<'a, 's> {
    pub(super) fn new(out: &'a mut ValueSchema, depth: usize, state: &'s TraceState) -> Self {
        Self {
            out: Some(out),
            depth,
            state,
        }
    }
}

impl<'de> SeqAccess<'de> for OneElement<'_, '_> {
    type Error = TraceStop;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, TraceStop> {
        match self.out.take() {
            Some(out) => nested(seed, out, self.depth, self.state, "[]").map(Some),
            None => Ok(None),
        }
    }
}

/// A fixed-length sequence whose elements are traced in order.
pub(super) struct Elements<'a, 's> {
    items: std::iter::Enumerate<std::slice::IterMut<'a, ValueSchema>>,
    depth: usize,
    state: &'s TraceState,
}

impl<'a, 's> Elements<'a, 's> {
    pub(super) fn new(items: &'a mut [ValueSchema], depth: usize, state: &'s TraceState) -> Self {
        Self {
            items: items.iter_mut().enumerate(),
            depth,
            state,
        }
    }
}

impl<'de> SeqAccess<'de> for Elements<'_, '_> {
    type Error = TraceStop;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, TraceStop> {
        match self.items.next() {
            Some((index, out)) => {
                nested(seed, out, self.depth, self.state, &format!("[{index}]")).map(Some)
            }
            None => Ok(None),
        }
    }
}

/// A map of exactly one entry: an untraced key and a traced value.
pub(super) struct OneEntry<'a, 's> {
    value: Option<&'a mut ValueSchema>,
    depth: usize,
    state: &'s TraceState,
}

impl<'a, 's> OneEntry<'a, 's> {
    pub(super) fn new(value: &'a mut ValueSchema, depth: usize, state: &'s TraceState) -> Self {
        Self {
            value: Some(value),
            depth,
            state,
        }
    }
}

impl<'de> MapAccess<'de> for OneEntry<'_, '_> {
    type Error = TraceStop;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TraceStop> {
        if self.value.is_none() {
            return Ok(None);
        }

        let mut key = ValueSchema::Any;

        nested(seed, &mut key, self.depth, self.state, "{key}").map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, TraceStop> {
        let out = self
            .value
            .take()
            .ok_or_else(|| TraceStop::new("map value without key"))?;

        nested(seed, out, self.depth, self.state, "{}")
    }
}

/// A struct's fields, each offered by name and traced in turn. A field skipped in an earlier run
/// keeps the shape it was recorded with and is not offered, so the fields after it still trace.
pub(super) struct Fields<'a, 's> {
    name: &'static str,
    fields: std::slice::IterMut<'a, TracedField>,
    current: Option<(&'a mut ValueSchema, String)>,
    depth: usize,
    state: &'s TraceState,
}

impl<'a, 's> Fields<'a, 's> {
    pub(super) fn new(
        name: &'static str,
        fields: &'a mut [TracedField],
        depth: usize,
        state: &'s TraceState,
    ) -> Self {
        Self {
            name,
            fields: fields.iter_mut(),
            current: None,
            depth,
            state,
        }
    }
}

impl<'de> MapAccess<'de> for Fields<'_, '_> {
    type Error = TraceStop;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TraceStop> {
        for field in self.fields.by_ref() {
            let segment = format!("{}.{}", self.name, field.name);
            let path = format!("{}/{segment}", self.state.current_path());

            if let Some(skipped) = self.state.skipped(path.trim_start_matches('/')) {
                field.value = skipped;

                continue;
            }

            let key = seed.deserialize(BorrowedStrDeserializer::<TraceStop>::new(field.name))?;

            self.current = Some((&mut field.value, segment));

            return Ok(Some(key));
        }

        Ok(None)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, TraceStop> {
        let (out, segment) = self
            .current
            .take()
            .ok_or_else(|| TraceStop::new("field value without key"))?;

        nested(seed, out, self.depth, self.state, &segment)
    }
}

/// Selects one enum variant so tracing can continue past it.
pub(super) struct FirstVariant<'s> {
    variant: &'static str,
    depth: usize,
    state: &'s TraceState,
}

impl<'s> FirstVariant<'s> {
    pub(super) fn new(variant: &'static str, depth: usize, state: &'s TraceState) -> Self {
        Self {
            variant,
            depth,
            state,
        }
    }
}

impl FirstVariant<'_> {
    /// Traces the selected variant's tuple or struct payload under the variant's name, so
    /// positions of different variants of one enum keep separate candidate choices.
    fn trace_payload<T>(
        self,
        trace: impl FnOnce(Tracer<'_, '_>) -> Result<T, TraceStop>,
    ) -> Result<T, TraceStop> {
        let mut payload = ValueSchema::Any;

        self.state.enter(self.variant);

        let traced = trace(Tracer::new(&mut payload, self.depth + 1, self.state));

        self.state.leave();

        traced
    }
}

impl<'de> EnumAccess<'de> for FirstVariant<'_> {
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

impl<'de> VariantAccess<'de> for FirstVariant<'_> {
    type Error = TraceStop;

    fn unit_variant(self) -> Result<(), TraceStop> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, TraceStop> {
        let mut payload = ValueSchema::Any;

        nested(seed, &mut payload, self.depth, self.state, self.variant)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, TraceStop> {
        self.trace_payload(|tracer| de::Deserializer::deserialize_tuple(tracer, len, visitor))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceStop> {
        let variant = self.variant;

        self.trace_payload(|tracer| {
            de::Deserializer::deserialize_struct(tracer, variant, fields, visitor)
        })
    }
}
