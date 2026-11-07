//! What a protocol or subsystem says an invocation context is for.

use std::any::Any;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// What kind of entry point established an invocation context.
///
/// The DI engine knows no concrete target: each protocol or subsystem defines its own typed
/// targets in its own crate and recognises them again with [`downcast_ref`]. Request data
/// (headers, the matched route, the method path) stays with the protocol's dispatch; a target
/// carries only what is known when the entry point is registered or accepted.
///
/// [`downcast_ref`]: dyn InvocationTarget::downcast_ref
pub trait InvocationTarget: Any + fmt::Debug + Send + Sync {}

impl dyn InvocationTarget {
    /// The target as a `T`, or `None` when it is another type.
    pub fn downcast_ref<T: InvocationTarget>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }

    /// Whether the target is a `T`.
    pub fn is<T: InvocationTarget>(&self) -> bool {
        (self as &dyn Any).is::<T>()
    }
}

/// A cheaply cloned handle to an [`InvocationTarget`].
///
/// A target without data is a `&'static` value (constant promotion makes `&Unit` one), so
/// establishing a context allocates nothing. A target with data is built once, at registration
/// or connection time, and shared: cloning the handle bumps a reference count.
#[derive(Clone)]
pub enum TargetRef {
    /// A target that lives for the whole program.
    Static(&'static dyn InvocationTarget),
    /// A target shared by every context built from it.
    Shared(Arc<dyn InvocationTarget>),
}

impl TargetRef {
    /// A handle to the program-lifetime `target`.
    pub fn from_static<T: InvocationTarget>(target: &'static T) -> Self {
        Self::Static(target)
    }

    /// A handle to the shared `target`.
    pub fn shared<T: InvocationTarget>(target: Arc<T>) -> Self {
        Self::Shared(target)
    }
}

impl Deref for TargetRef {
    type Target = dyn InvocationTarget;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Static(target) => *target,
            Self::Shared(target) => target.as_ref(),
        }
    }
}

impl fmt::Debug for TargetRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}
