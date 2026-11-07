//! Why the active invocation context cannot answer a lookup.

/// Why the active invocation context cannot answer a lookup.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContextError {
    /// No framework entry point established a context for the running task.
    #[error("no invocation context is active: the code runs outside every framework entry point")]
    OutsideContext,
    /// The context has no open scope instance, such as in a job run or a lifecycle hook.
    #[error("the active invocation context has no open scope instance")]
    OutsideInvocationScope,
    /// The context's runtime no longer has a root, because the application shut down.
    #[error("the active invocation context's runtime has no root")]
    RuntimeUnavailable,
}
