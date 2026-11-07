//! `Current<T>`: the closest `T` in the active invocation.

use std::fmt;
use std::marker::PhantomData;

use upwell_core::DependencyDescriptor;

use super::{Active, ScopedError, active, current_dependency, extract};
use crate::construct::FromContainer;
use crate::descriptors::ComponentConstructionContext;

/// A declared dependency on the closest `T` in the active invocation.
///
/// [`get`](Self::get) resolves `T` from the innermost scope instance of the invocation context
/// active where it is called, through that instance's parent chain, so the closest instance
/// wins: during a WebSocket message a value the message scope holds shadows the connection's.
/// It holds nothing and caches nothing, so a singleton may keep one and still see each
/// invocation's own value; the value is gone once the invocation ends.
///
/// Prefer ordinary injection where the consumer lives no longer than `T`: it is planned and
/// resolved once. `Current<T>` is for a longer-lived component that reads a shorter-lived value
/// per call, such as a singleton reading the request's identity.
pub struct Current<T> {
    _target: PhantomData<fn() -> T>,
}

impl<T> Current<T> {
    /// An accessor. It captures nothing, so it is valid anywhere.
    pub const fn new() -> Self {
        Self {
            _target: PhantomData,
        }
    }
}

impl<T: FromContainer> Current<T> {
    /// Resolves `T` from the active invocation's innermost scope instance.
    ///
    /// # Errors
    ///
    /// [`ScopedError::Context`] outside every invocation context or in one without a scope
    /// instance, [`ScopedError::ScopeNotActive`] when the scope the invocation runs in was never
    /// opened and nothing open provides `T`, [`ScopedError::Unavailable`] when nothing on the
    /// chain provides `T`, and
    /// [`ScopedError::Resolution`] when building it fails.
    pub async fn get(&self) -> Result<T, ScopedError> {
        let Active {
            innermost,
            unopened,
        } = active()?;

        match (extract::<T>(&innermost).await, unopened) {
            (Err(ScopedError::Unavailable { .. }), Some(scope)) => {
                Err(ScopedError::not_opened(scope))
            }
            (value, _) => value,
        }
    }
}

impl<T> Clone for Current<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Current<T> {}

impl<T> Default for Current<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for Current<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Current<{}>", std::any::type_name::<T>())
    }
}

impl<T: FromContainer> FromContainer for Current<T> {
    fn dependency() -> DependencyDescriptor {
        current_dependency::<T>()
    }

    async fn from_container(_cx: &ComponentConstructionContext) -> crate::Result<Self> {
        Ok(Self::new())
    }

    fn ready_at(_cx: &ComponentConstructionContext, _index: usize) -> Option<crate::Result<Self>> {
        Some(Ok(Self::new()))
    }
}
