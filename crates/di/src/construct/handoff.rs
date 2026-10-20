//! Moving an owned, `Send`-only value out of a factory through the
//! [`ComponentFactory`](crate::descriptors::ComponentFactory) output slot.

use std::sync::Mutex;

use upwell_core::TypeDescriptor;

use crate::descriptors::BoxedComponent;

/// A `Send` value boxed into a [`BoxedComponent`], whose slot requires `Sync`.
///
/// Transition-only recipes (transition handlers, hot updates) share the ordinary factory
/// fn-pointer shape, so their outputs travel boxed and are taken back by the framework
/// exactly once. The mutex is only ever moved, never locked while shared, so it cannot
/// be poisoned; a poisoned value would still be taken rather than discarded.
pub(crate) struct Handoff<T>(Mutex<T>);

impl<T: Send + 'static> Handoff<T> {
    /// Boxes `value` for the framework to [`take`](Self::take).
    pub(crate) fn boxed(value: T) -> BoxedComponent {
        BoxedComponent {
            ty: TypeDescriptor::of::<Self>(std::any::type_name::<T>()),
            value: Box::new(Self(Mutex::new(value))),
        }
    }

    /// Takes the value back, or `None` when `boxed` holds something else.
    pub(crate) fn take(boxed: BoxedComponent) -> Option<T> {
        let handoff = boxed.value.downcast::<Self>().ok()?;

        Some(
            handoff
                .0
                .into_inner()
                .unwrap_or_else(|poison| poison.into_inner()),
        )
    }
}
