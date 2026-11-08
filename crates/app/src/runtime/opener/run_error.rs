//! How a [`ScopeBuilder::try_run`](super::ScopeBuilder::try_run) fails.

use std::fmt;

/// Why a [`try_run`](super::ScopeBuilder::try_run) failed: the scope, or the work itself.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunError<E> {
    /// The instance could not open or a target of the work could not resolve.
    Scope(crate::Error),
    /// The work ran and returned its own error.
    Work(E),
}

impl<E: fmt::Display> fmt::Display for RunError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => write!(formatter, "the scope failed: {error}"),
            Self::Work(error) => write!(formatter, "the work failed: {error}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for RunError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::Work(error) => Some(error),
        }
    }
}

impl From<RunError<std::convert::Infallible>> for crate::Error {
    fn from(error: RunError<std::convert::Infallible>) -> Self {
        match error {
            RunError::Scope(error) => error,
            RunError::Work(never) => match never {},
        }
    }
}
