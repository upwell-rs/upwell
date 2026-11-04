//! [`CapturedContext`]: the active invocation context, captured to re-enter elsewhere.

use std::future::Future;
use std::thread;

use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::context::{ContextError, InvocationContext};

/// The invocation context and `tracing` span active where it was captured, to carry into
/// spawned work. See the [module documentation](super).
#[must_use = "a captured context does nothing until work runs in it"]
#[derive(Clone, Debug)]
pub struct CapturedContext {
    context: InvocationContext,
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
            context: InvocationContext::capture()?,
            span: tracing::Span::current(),
        })
    }

    /// Runs `future` inside the captured context and span, on whichever task polls it: for a
    /// `JoinSet`, a `LocalSet` or another executor.
    pub fn enter<F: Future>(self, future: F) -> impl Future<Output = F::Output> {
        self.context.enter(future).instrument(self.span)
    }

    /// Runs `body` inside the captured context and span on the calling thread, then restores
    /// what the thread had before.
    pub fn run<R>(self, body: impl FnOnce() -> R) -> R {
        let Self { context, span } = self;

        span.in_scope(|| context.enter_sync(body))
    }

    /// `body`, wrapped to run inside the captured context wherever it is called: for
    /// `std::thread::scope`, a rayon job or another thread pool.
    pub fn bind<R>(self, body: impl FnOnce() -> R) -> impl FnOnce() -> R {
        move || self.run(body)
    }

    /// `body`, wrapped to enter the captured context on every call: for a parallel iterator's
    /// `map` or `for_each`. Each call clones the capture, a few reference-count bumps.
    pub fn bind_fn<A, R>(self, body: impl Fn(A) -> R) -> impl Fn(A) -> R {
        move |argument| self.clone().run(|| body(argument))
    }

    /// Spawns `future` on the tokio runtime inside the captured context.
    pub fn spawn<F>(self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(self.enter(future))
    }

    /// Spawns the `!Send` `future` on the current `LocalSet` inside the captured context.
    ///
    /// # Panics
    ///
    /// As `tokio::task::spawn_local`, outside a `LocalSet`.
    pub fn spawn_local<F>(self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        tokio::task::spawn_local(self.enter(future))
    }

    /// Runs `body` on tokio's blocking pool inside the captured context.
    pub fn spawn_blocking<R>(self, body: impl FnOnce() -> R + Send + 'static) -> JoinHandle<R>
    where
        R: Send + 'static,
    {
        tokio::task::spawn_blocking(self.bind(body))
    }

    /// Runs `body` on a new OS thread inside the captured context.
    pub fn spawn_thread<R>(self, body: impl FnOnce() -> R + Send + 'static) -> thread::JoinHandle<R>
    where
        R: Send + 'static,
    {
        thread::spawn(self.bind(body))
    }
}
