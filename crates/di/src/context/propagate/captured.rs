//! [`CapturedContext`]: the active invocation context, captured to re-enter elsewhere.

use std::future::Future;

use futures::future::Either;
use tracing::Instrument;

use crate::context::{ContextError, InvocationContext};

/// The invocation context and `tracing` span active where it was captured, to carry into
/// work another executor runs. See the [module documentation](super).
#[must_use = "a captured context does nothing until work runs in it"]
#[derive(Clone, Debug)]
pub struct CapturedContext {
    context: Option<InvocationContext>,
    span: tracing::Span,
}

impl CapturedContext {
    /// Captures the active invocation context and the current span.
    ///
    /// # Errors
    ///
    /// [`ContextError::OutsideContext`] when no invocation context is active.
    pub fn capture() -> Result<Self, ContextError> {
        Ok(Self {
            context: Some(InvocationContext::capture()?),
            span: tracing::Span::current(),
        })
    }

    /// The active invocation context when there is one, and the current span: what the
    /// spawning functions carry.
    pub(super) fn active() -> Self {
        Self {
            context: InvocationContext::capture().ok(),
            span: tracing::Span::current(),
        }
    }

    /// Runs `future` inside the captured context and span, on whichever task polls it: for a
    /// `JoinSet`, a `LocalSet` or another executor.
    pub fn enter<F: Future>(self, future: F) -> impl Future<Output = F::Output> {
        let Self { context, span } = self;
        let future = match context {
            Some(context) => Either::Left(context.enter(future)),
            None => Either::Right(future),
        };

        future.instrument(span)
    }

    /// Runs `body` inside the captured context and span on the calling thread, then restores
    /// what the thread had before.
    pub fn run<R>(self, body: impl FnOnce() -> R) -> R {
        let Self { context, span } = self;

        span.in_scope(|| match context {
            Some(context) => context.enter_sync(body),
            None => body(),
        })
    }

    /// Runs `body` inside a copy of the captured context, keeping the capture for more work.
    /// The copy costs a few reference-count bumps.
    pub(super) fn run_in<R>(&self, body: impl FnOnce() -> R) -> R {
        self.clone().run(body)
    }

    /// `body`, wrapped to run inside the captured context wherever it is called: for a thread
    /// pool or another executor's job.
    pub fn bind<R>(self, body: impl FnOnce() -> R) -> impl FnOnce() -> R {
        move || self.run(body)
    }

    /// `body`, wrapped to enter the captured context on every call: for a parallel iterator's
    /// `map` or `for_each`. Each call copies the capture, a few reference-count bumps.
    pub fn bind_fn<A, R>(self, body: impl Fn(A) -> R) -> impl Fn(A) -> R {
        move |argument| self.run_in(|| body(argument))
    }
}
