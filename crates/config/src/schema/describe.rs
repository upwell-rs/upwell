//! Static value descriptions and the probe `#[config]` uses to pick the richest one per field.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::marker::PhantomData;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use serde::de::DeserializeOwned;

use super::{ValueSchema, trace};

/// A config value type whose shape is known statically.
///
/// Implemented for the std scalars and collections and, by `#[config]`, for every config type,
/// so a nested `#[config]` field carries its own documented schema. Other field types fall back
/// to tracing their `Deserialize` impl.
pub trait DescribedValue {
    /// The value's shape.
    fn describe() -> ValueSchema;
}

macro_rules! described {
    ($schema:expr => $($ty:ty),+ $(,)?) => {
        $(
            impl DescribedValue for $ty {
                fn describe() -> ValueSchema {
                    $schema
                }
            }
        )+
    };
}

described!(ValueSchema::Bool => bool);
described!(ValueSchema::Integer { signed: true } => i8, i16, i32, i64, i128, isize);
described!(ValueSchema::Integer { signed: false } => u8, u16, u32, u64, u128, usize);
described!(ValueSchema::Float => f32, f64);
described!(ValueSchema::Char => char);
described!(
    ValueSchema::String => String, PathBuf, IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr,
);

macro_rules! described_wrapper {
    ($wrap:expr => $($ty:ident),+ $(,)?) => {
        $(
            impl<T: DescribedValue> DescribedValue for $ty<T> {
                fn describe() -> ValueSchema {
                    $wrap(Box::new(T::describe()))
                }
            }
        )+
    };
}

described_wrapper!(ValueSchema::Optional => Option);
described_wrapper!(ValueSchema::Array => Vec, BTreeSet, HashSet);

impl<T: DescribedValue> DescribedValue for Box<T> {
    fn describe() -> ValueSchema {
        T::describe()
    }
}

impl<K, V: DescribedValue> DescribedValue for BTreeMap<K, V> {
    fn describe() -> ValueSchema {
        ValueSchema::Map(Box::new(V::describe()))
    }
}

impl<K, V: DescribedValue, S> DescribedValue for HashMap<K, V, S> {
    fn describe() -> ValueSchema {
        ValueSchema::Map(Box::new(V::describe()))
    }
}

/// A zero-sized stand-in for a field type, probed by method resolution.
///
/// `(&&&Probe::<T>::NEW).value_schema()` resolves to the most specific applicable level:
/// [`ViaDescribed`] (`T: DescribedValue`), then [`ViaSerde`] (`T: DeserializeOwned`, traced),
/// then [`ViaOpaque`] (any `T`). Autoref ranking happens at the macro's concrete field type, so
/// no specialization is needed and an undescribable field type still compiles.
pub struct Probe<T>(PhantomData<fn() -> T>);

impl<T> Probe<T> {
    /// The probe value.
    pub const NEW: Self = Self(PhantomData);
}

/// The [`DescribedValue`] probe level.
pub trait ViaDescribed {
    /// The probed type's value shape.
    fn value_schema(&self) -> ValueSchema;
}

impl<T: DescribedValue> ViaDescribed for &&Probe<T> {
    fn value_schema(&self) -> ValueSchema {
        T::describe()
    }
}

/// The traced-`Deserialize` probe level.
pub trait ViaSerde {
    /// The probed type's value shape.
    fn value_schema(&self) -> ValueSchema;
}

impl<T: DeserializeOwned> ViaSerde for &Probe<T> {
    fn value_schema(&self) -> ValueSchema {
        trace::<T>()
    }
}

/// The opaque probe level, for a field type with no known shape.
pub trait ViaOpaque {
    /// The probed type's value shape.
    fn value_schema(&self) -> ValueSchema;
}

impl<T> ViaOpaque for Probe<T> {
    fn value_schema(&self) -> ValueSchema {
        ValueSchema::Any
    }
}
