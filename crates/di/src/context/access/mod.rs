//! Declared access to the active invocation's scopes: [`Current<T>`] and [`Scoped<S, T>`].
//!
//! Both are stateless dependencies. Injecting one captures no container, imposes no build
//! order and is exempt from the captive-scope rule; every [`get`](Current::get) resolves anew
//! from the invocation context active where it is called, so a singleton may hold one and see
//! each invocation's own value. Their edges carry
//! [`ResolutionMode::Current`](upwell_core::ResolutionMode::Current), and validation still
//! requires their target to be provided in some scope.

mod current;
mod error;
mod scope_value;
mod scoped;

pub use current::Current;
pub use error::ScopedError;
pub use scope_value::{scope_is_active, scope_value};
pub use scoped::Scoped;

use std::sync::Arc;

use upwell_core::{DependencyDescriptor, ResolutionMode, ScopeId, ScopeKey};

use super::InvocationContext;
use crate::construct::FromContainer;
use crate::container::ScopeContainer;

/// `H`'s own edge, resolved from the invocation context instead of at construction.
fn current_dependency<H: FromContainer>() -> DependencyDescriptor {
    let mut dependency = H::dependency();

    dependency.resolution = ResolutionMode::Current;
    dependency.observation = upwell_core::DependencyObservation::Snapshot;

    dependency
}

/// The active invocation's innermost open scope instance, and the scope it still waits for.
struct Active {
    innermost: Arc<ScopeContainer>,
    unopened: Option<ScopeKey>,
}

/// The active invocation's scopes. Without an open instance, a scope the invocation waits for
/// but that was never opened is reported as not active.
fn active() -> Result<Active, ScopedError> {
    InvocationContext::with_current(|context| {
        let unopened = context.unopened_scope();

        match context.scope() {
            Ok(innermost) => Ok(Active {
                innermost,
                unopened,
            }),
            Err(error) => {
                Err(unopened.map_or(ScopedError::Context(error), ScopedError::not_opened))
            }
        }
    })?
}

/// The innermost open scope instance of the active invocation.
fn innermost() -> Result<Arc<ScopeContainer>, ScopedError> {
    active().map(|active| active.innermost)
}

/// The instance of scope `scope` on the chain from `innermost` up to the root.
fn instance_of(innermost: Arc<ScopeContainer>, scope: ScopeId) -> Option<Arc<ScopeContainer>> {
    let mut current = innermost;

    loop {
        if current.scope().id() == scope {
            return Some(current);
        }

        current = Arc::clone(current.parent()?);
    }
}

/// Extracts `H` from `container`, mapping the DI error to a [`ScopedError`].
async fn extract<H: FromContainer>(container: &Arc<ScopeContainer>) -> Result<H, ScopedError> {
    let dependency = H::dependency();

    container.extract::<H>().await.map_err(|error| {
        let provided = container.reaches_provider(dependency.ty.type_id, dependency.qualifier);

        ScopedError::resolution::<H>(error, provided)
    })
}

#[cfg(test)]
mod tests;
