//! Seeds produced before the scope they fill opens.

use std::any::Any;
use std::fmt;

use upwell_core::TypeDescriptor;
use upwell_di::BoxedComponent;

/// Values for the factory-less slots of a scope instance that is not opened yet.
///
/// Code that produces a scope's seeds ahead of its opening, such as a protocol's
/// authentication step, collects them here and hands them to
/// [`ScopeBuilder::seeds`](super::ScopeBuilder::seeds). Each seed is checked against its slot
/// when the scope opens, as [`ScopeBuilder::seed`](super::ScopeBuilder::seed) checks it.
#[derive(Default)]
pub struct Seeds {
    values: Vec<BoxedComponent>,
}

impl Seeds {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `value` for the scope's factory-less slot of `T`.
    #[must_use]
    pub fn with<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.insert(value);

        self
    }

    /// Adds `value` for the scope's factory-less slot of `T`.
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) {
        self.values.push(boxed_seed(value));
    }

    /// Whether the set holds no seed.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The number of seeds in the set.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Adds every seed of `other` after this set's own.
    pub fn extend(&mut self, other: Self) {
        self.values.extend(other.values);
    }

    pub(super) fn into_values(self) -> Vec<BoxedComponent> {
        self.values
    }
}

impl fmt::Debug for Seeds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.values.iter().map(|seed| (seed.ty.type_name)()))
            .finish()
    }
}

/// `value` boxed for the slot of its own type.
pub(super) fn boxed_seed<T: Any + Send + Sync>(value: T) -> BoxedComponent {
    BoxedComponent::stored(TypeDescriptor::of::<T>(std::any::type_name::<T>()), value)
}
