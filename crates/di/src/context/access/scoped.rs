//! `Scoped<S, T>`: the `T` scope `S` resolves in the active invocation.

use std::fmt;
use std::marker::PhantomData;

use upwell_core::{DependencyDescriptor, Scope};

use super::{ScopedError, current_dependency, extract, innermost, instance_of};
use crate::construct::FromContainer;
use crate::descriptors::ComponentConstructionContext;

/// A declared dependency on `T` as the instance of scope `S` in the active invocation resolves
/// it.
///
/// [`get`](Self::get) walks the active invocation's chain to the instance of `S` and resolves
/// `T` there, so a closer instance of `T` does not shadow it: during a WebSocket message,
/// `Scoped<Connection, Arc<Identity>>` is the connection's identity even when the message
/// scope holds its own. Like [`Current`](super::Current) it holds and caches nothing.
pub struct Scoped<S, T> {
    _target: PhantomData<fn() -> (S, T)>,
}

impl<S, T> Scoped<S, T> {
    /// An accessor. It captures nothing, so it is valid anywhere.
    pub const fn new() -> Self {
        Self {
            _target: PhantomData,
        }
    }
}

impl<S: Scope, T: FromContainer> Scoped<S, T> {
    /// Resolves `T` from the active invocation's instance of `S`.
    ///
    /// # Errors
    ///
    /// [`ScopedError::Context`] outside every invocation context or in one without a scope
    /// instance, [`ScopedError::ScopeNotActive`] when the chain has no instance of `S`,
    /// [`ScopedError::Unavailable`] when `S` cannot reach a `T`, and
    /// [`ScopedError::Resolution`] when building it fails.
    pub async fn get(&self) -> Result<T, ScopedError> {
        let scope =
            instance_of(innermost()?, S::ID).ok_or_else(ScopedError::scope_not_active::<S>)?;

        extract::<T>(&scope).await
    }
}

impl<S, T> Clone for Scoped<S, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, T> Copy for Scoped<S, T> {}

impl<S, T> Default for Scoped<S, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, T> fmt::Debug for Scoped<S, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Scoped<{}, {}>",
            std::any::type_name::<S>(),
            std::any::type_name::<T>()
        )
    }
}

impl<S: Scope, T: FromContainer> FromContainer for Scoped<S, T> {
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
