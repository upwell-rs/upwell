//! Descriptions of config shapes for tooling: the fields, types, docs, defaults, and
//! declarations a config type deserializes from.
//!
//! A `#[config]` type declares its shape statically as [`ConfigProperties::SCHEMA`], with each
//! field's value described through [`DescribedValue`] (nested `#[config]` types and the std
//! scalars/collections), falling back to tracing the field's serde `Deserialize` impl. A type
//! that implements [`ConfigProperties`] by hand is traced as a whole, so every binding — whether
//! registered by `#[config(path = ..)]` or bound at runtime — exposes its fields.
//!
//! [`ConfigProperties`]: crate::ConfigProperties
//! [`ConfigProperties::SCHEMA`]: crate::ConfigProperties::SCHEMA

use std::fmt;

use upwell_core::{DescriptorSource, ItemSource};

use crate::EnumTag;

mod describe;
mod trace;

pub use describe::DescribedValue;
pub use trace::trace;

#[doc(hidden)]
pub mod __probe {
    pub use super::describe::{Probe, ViaDescribed, ViaOpaque, ViaSerde};
}

/// The static shape of one config type, emitted by `#[config]`.
#[derive(Clone, Copy, Debug)]
pub struct ConfigSchema {
    /// The type's `///` documentation, one line per doc line.
    pub docs: &'static str,
    /// The type's declaration.
    pub source: Option<ItemSource>,
    /// The type's fields or variants.
    pub shape: ConfigShape,
}

impl ConfigSchema {
    /// The schema of a type that declares no shape (a hand-written `ConfigProperties`).
    pub const UNDECLARED: Self = Self {
        docs: "",
        source: None,
        shape: ConfigShape::Undeclared,
    };

    /// Whether the type declared its shape statically.
    pub const fn is_declared(&self) -> bool {
        !matches!(self.shape, ConfigShape::Undeclared)
    }
}

/// The declared layout of a config type.
#[derive(Clone, Copy, Debug)]
pub enum ConfigShape {
    /// No static shape; tooling traces the type's `Deserialize` impl instead.
    Undeclared,
    /// A struct and its named fields, in declaration order.
    Struct(&'static [FieldSchema]),
    /// A struct deserialized as another value: a `#[serde(transparent)]` or newtype struct (its
    /// single field), or a tuple struct (its fields as a tuple).
    Value(fn() -> ValueSchema),
    /// An enum, its serde tagging, and its variants, in declaration order.
    Enum {
        /// How serde represents the selected variant.
        tagging: EnumTag,
        /// The variants serde accepts.
        variants: &'static [VariantSchema],
    },
}

/// One named field of a declared config struct or struct variant.
#[derive(Clone, Copy)]
pub struct FieldSchema {
    /// The key serde deserializes the field from (after `rename`/`rename_all`).
    pub name: &'static str,
    /// The Rust field identifier.
    pub ident: &'static str,
    /// The field's `///` documentation.
    pub docs: &'static str,
    /// The field type as written in the source.
    pub declared_type: &'static str,
    /// The fully qualified field type, as `type_name` reports it.
    pub rust_type: fn() -> &'static str,
    /// The `#[default = ".."]` template, when declared.
    pub default: Option<&'static str>,
    /// Whether the config must supply the field: not optional, not defaulted.
    pub required: bool,
    /// Whether the field is `#[serde(flatten)]`ed into its parent.
    pub flatten: bool,
    /// The field identifier's position.
    pub source: DescriptorSource,
    /// The shape of the field's value.
    pub value: fn() -> ValueSchema,
}

impl fmt::Debug for FieldSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldSchema")
            .field("name", &self.name)
            .field("declared_type", &self.declared_type)
            .field("default", &self.default)
            .field("required", &self.required)
            .field("flatten", &self.flatten)
            .finish_non_exhaustive()
    }
}

/// One variant of a declared config enum.
#[derive(Clone, Copy, Debug)]
pub struct VariantSchema {
    /// The tag serde deserializes the variant from.
    pub name: &'static str,
    /// The Rust variant identifier.
    pub ident: &'static str,
    /// The variant's `///` documentation.
    pub docs: &'static str,
    /// Whether the variant is selected when the config names none.
    pub default: bool,
    /// The variant identifier's position.
    pub source: DescriptorSource,
    /// The variant's payload.
    pub payload: VariantPayload,
}

/// The payload a config enum variant carries.
#[derive(Clone, Copy, Debug)]
pub enum VariantPayload {
    /// No payload.
    Unit,
    /// Named fields.
    Struct(&'static [FieldSchema]),
    /// Positional fields; a single one is a newtype variant.
    Tuple(&'static [fn() -> ValueSchema]),
}

/// The shape of one config value, resolved at tooling time.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ValueSchema {
    /// `true` or `false`.
    Bool,
    /// An integer; unsigned integers reject negative values.
    Integer {
        /// Whether negative values are accepted.
        signed: bool,
    },
    /// A floating-point number.
    Float,
    /// A single character string.
    Char,
    /// A string.
    String,
    /// Raw bytes.
    Bytes,
    /// The unit value.
    Unit,
    /// A value that may be absent or null.
    Optional(Box<ValueSchema>),
    /// A sequence of values.
    Array(Box<ValueSchema>),
    /// A table of values under arbitrary keys.
    Map(Box<ValueSchema>),
    /// A fixed-length sequence of values.
    Tuple(Vec<ValueSchema>),
    /// A declared `#[config]` type.
    Config {
        /// The fully qualified type name.
        rust_type: &'static str,
        /// The type's declared shape.
        schema: ConfigSchema,
    },
    /// A struct shape recovered by tracing its `Deserialize` impl.
    Struct {
        /// The serde container name.
        name: &'static str,
        /// The fields serde reads, with their traced values.
        fields: Vec<TracedField>,
    },
    /// An enum shape recovered by tracing its `Deserialize` impl.
    Enum {
        /// The serde container name.
        name: &'static str,
        /// The variant tags serde accepts.
        variants: Vec<&'static str>,
    },
    /// Any value: the shape is self-describing (`deserialize_any`) or could not be traced.
    Any,
}

/// One field of a traced struct shape.
#[derive(Clone, Debug)]
pub struct TracedField {
    /// The key serde reads the field from.
    pub name: &'static str,
    /// The field's traced value.
    pub value: ValueSchema,
}

/// The value shape of config type `T`: its declared schema, or its traced `Deserialize` shape
/// when it declares none.
pub fn config_value_schema<T: crate::ConfigProperties>() -> ValueSchema {
    if T::SCHEMA.is_declared() {
        return ValueSchema::Config {
            rust_type: std::any::type_name::<T>(),
            schema: T::SCHEMA,
        };
    }

    trace::<T>()
}

#[cfg(test)]
mod tests;
