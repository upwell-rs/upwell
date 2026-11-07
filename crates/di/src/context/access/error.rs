//! Why a declared accessor could not resolve from the active invocation.

use upwell_core::{Scope, ScopeId, ScopeKey};

use crate::construct::FromContainer;
use crate::context::ContextError;
use crate::error::Error;

/// Why a [`Current`](super::Current) or [`Scoped`](super::Scoped) accessor could not resolve
/// its target from the active invocation.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ScopedError {
    /// No invocation context is active, or it has no open scope instance.
    #[error(transparent)]
    Context(#[from] ContextError),
    /// The pinned scope has no instance on the active invocation's chain, or the scope the
    /// invocation runs in was never opened.
    #[error("scope `{name}` (`{scope}`) has no open instance in the active invocation")]
    ScopeNotActive {
        /// The pinned scope's identity.
        scope: ScopeId,
        /// The pinned scope's display name.
        name: &'static str,
    },
    /// Nothing on the active invocation's chain provides the target.
    #[error("`{type_name}` is not available in the active invocation's scopes")]
    Unavailable {
        /// The target's type name.
        type_name: &'static str,
    },
    /// Building the target failed.
    #[error("resolving `{type_name}` in the active invocation failed")]
    Resolution {
        /// The target's type name.
        type_name: &'static str,
        /// The DI error the resolution failed with.
        #[source]
        source: Box<Error>,
    },
}

impl ScopedError {
    /// The error for scope `S` having no instance on the active chain.
    pub(super) const fn scope_not_active<S: Scope>() -> Self {
        Self::not_opened(S::KEY)
    }

    /// The error for `scope` having no open instance in the active invocation.
    pub(super) const fn not_opened(scope: ScopeKey) -> Self {
        Self::ScopeNotActive {
            scope: scope.id(),
            name: scope.name(),
        }
    }

    /// The error for a resolution of `H` that failed with `error`. A missing component is
    /// `H`'s own absence only when nothing on the chain provides its target (`provided` is
    /// false); otherwise building the target failed on a missing dependency of its own, which
    /// is kept as the source.
    pub(super) fn resolution<H: FromContainer>(error: Error, provided: bool) -> Self {
        let type_name = (H::dependency().ty.type_name)();

        match error {
            Error::MissingComponent(_) if !provided => Self::Unavailable { type_name },
            source => Self::Resolution {
                type_name,
                source: Box::new(source),
            },
        }
    }
}
