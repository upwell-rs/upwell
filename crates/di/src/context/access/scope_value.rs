//! The active instance of a scope type, behind `#[scope]`'s generated `S::is_active()` and
//! `S::current()`.
//!
//! Reading the active scope's own value is the one public read of the context: it builds
//! nothing, is typed and closed, and each instance carries its own value.

use std::sync::Arc;

use upwell_core::Scope;

use super::{ScopedError, innermost, instance_of};
use crate::container::ScopeContainer;

/// The active invocation's instance of `S`.
fn active<S: Scope>() -> Result<Arc<ScopeContainer>, ScopedError> {
    instance_of(innermost()?, S::ID).ok_or(ScopedError::ScopeNotActive { scope: S::NAME })
}

/// Whether the active invocation runs in an instance of `S`.
#[doc(hidden)]
pub fn scope_is_active<S: Scope>() -> bool {
    active::<S>().is_ok()
}

/// The value of the active invocation's instance of `S`, its params.
///
/// # Errors
///
/// [`ScopedError::Context`] outside an invocation scope, [`ScopedError::ScopeNotActive`] when
/// the chain has no instance of `S`, and [`ScopedError::Unavailable`] when the instance holds
/// no value, as for a zero-sized scope.
#[doc(hidden)]
pub fn scope_value<S: Scope>() -> Result<Arc<S>, ScopedError> {
    active::<S>()?
        .built_component::<Arc<S>>()
        .ok_or(ScopedError::Unavailable {
            type_name: std::any::type_name::<S>(),
        })
}
