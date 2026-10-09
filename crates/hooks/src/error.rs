use thiserror::Error;

/// Errors from running a hook: framework failures only. A hook's *domain* outcome (e.g.
/// whether a config reload is accepted) is its [`HookKind::Output`](crate::HookKind),
/// not an error.
#[derive(Debug, Error)]
pub enum Error {
    /// The resolver context's owning runtime generation is no longer available.
    #[error("hook resolver context is no longer available")]
    ResolverUnavailable,

    /// The hook's `&self` receiver could not be resolved from the resolver context.
    #[error("hook receiver not found: {0}")]
    MissingReceiver(&'static str),

    /// A hook parameter could not be extracted from the kind's context (e.g. a
    /// `CfgNext<T>` whose binding is not staged in the reload proposal).
    #[error("hook parameter not available: {0}")]
    MissingParam(&'static str),

    /// A hook panicked. The payload is intentionally omitted because panic strings
    /// can contain application secrets and hook runners are operational boundaries.
    #[error("hook '{hook}' on '{component}' panicked")]
    Panicked {
        hook: &'static str,
        component: &'static str,
    },

    /// Generated hook metadata returned a value for the wrong hook kind.
    #[error("hook '{hook}' on '{component}' returned an invalid output type")]
    InvalidOutput {
        hook: &'static str,
        component: &'static str,
    },

    /// An application-defined error raised while extracting a hook parameter or running
    /// the hook body.
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T, E = Error> = core::result::Result<T, E>;
