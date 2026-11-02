use std::any::Any;
use std::fmt;
use std::ops::Deref;

use upwell_core::TypeDescriptor;

use super::super::Injectable;
use super::StoredValue;

/// A type-erased instantiated component.
///
/// `value` holds what a scope stores for the instance, the [`Injectable::Stored`] of its handle,
/// recovered with [`from_boxed`] or `value.downcast_ref::<Live<T>>()`.
pub struct BoxedComponent {
    pub ty: TypeDescriptor,
    pub value: StoredValue,
}

impl BoxedComponent {
    /// A component of type `ty` storing `value`.
    pub fn stored<S: Any + Send + Sync>(ty: TypeDescriptor, value: S) -> Self {
        Self {
            ty,
            value: StoredValue::new(value),
        }
    }
}

impl AsRef<StoredValue> for BoxedComponent {
    fn as_ref(&self) -> &StoredValue {
        &self.value
    }
}

impl Deref for BoxedComponent {
    type Target = StoredValue;

    fn deref(&self) -> &StoredValue {
        &self.value
    }
}

impl fmt::Debug for BoxedComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxedComponent")
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

/// Recovers handle `H` from a stored component: downcasts to its stored representation
/// (`H::Stored`) and derives the handle. This is the single seam through which
/// every resolution converts storage back into a handle — a snapshot for `Arc<T>`,
/// a shared clone for `Dep<T>`, a plain clone for a by-value handle. `None` if the
/// component stores a different type.
///
/// Public so external resolvers (e.g. the config store) can recover a handle from a
/// seed they hold without re-implementing the downcast.
pub fn from_boxed<H: Injectable>(boxed: &BoxedComponent) -> Option<H> {
    boxed.value.downcast_ref::<H::Stored>().map(H::from_stored)
}
