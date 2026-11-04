//! Why a declared accessor could not resolve from the active invocation.

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
    /// The pinned scope has no instance on the active invocation's chain.
    #[error("scope `{scope}` has no instance in the active invocation")]
    ScopeNotActive {
        /// The pinned scope's name.
        scope: &'static str,
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
    /// The error for a resolution of `H` that failed with `error`.
    pub(super) fn resolution<H: FromContainer>(error: Error) -> Self {
        let type_name = (H::dependency().ty.type_name)();

        match error {
            Error::MissingComponent(_) => Self::Unavailable { type_name },
            source => Self::Resolution {
                type_name,
                source: Box::new(source),
            },
        }
    }
}
