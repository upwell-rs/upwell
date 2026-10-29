use std::any::Any;
use std::fmt;
use std::ops::Deref;

use upwell_core::TypeDescriptor;

use super::super::Injectable;

/// A type-erased instantiated component.
///
/// `value` stores `Arc<T>` inside a `Box<dyn Any + Send + Sync>`, which
/// allows recovery via `value.downcast_ref::<Arc<T>>()`.
pub struct BoxedComponent {
    pub ty: TypeDescriptor,
    pub value: Box<dyn Any + Send + Sync>,
}

impl AsRef<Box<dyn Any + Send + Sync>> for BoxedComponent {
    fn as_ref(&self) -> &Box<dyn Any + Send + Sync> {
        &self.value
    }
}

impl Deref for BoxedComponent {
    type Target = Box<dyn Any + Send + Sync>;

    fn deref(&self) -> &Box<dyn Any + Send + Sync> {
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

/// Recovers handle `H` from a boxed slot: downcasts to its stored representation
/// (`H::Stored`) and derives the handle. This is the single seam through which
/// every resolution converts storage back into a handle — a snapshot for `Arc<T>`,
/// a shared clone for `Dep<T>`, a plain clone for a by-value handle. `None` if the
/// box holds a different stored type.
///
/// Public so external resolvers (e.g. the config store) can recover a handle from a
/// seed they hold without re-implementing the downcast.
pub fn from_boxed<H: Injectable>(boxed: &BoxedComponent) -> Option<H> {
    boxed.value.downcast_ref::<H::Stored>().map(H::from_stored)
}
