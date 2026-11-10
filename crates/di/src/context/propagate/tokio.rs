//! Tokio's spawning functions, carrying the active invocation context and span.
//!
//! Each behaves as the tokio function it wraps; outside every context it carries only the
//! span and masks any context the work runs in. Dropping the returned [`JoinHandle`] detaches the task rather than cancelling it,
//! and [`JoinHandle::abort`] does not stop a blocking task that already runs, so the task
//! keeps the invocation's scope chain alive until it ends. Await request-bound work or manage
//! its handle explicitly.

use std::future::Future;

use ::tokio::task::JoinHandle;

use super::CapturedContext;

/// [`tokio::spawn`](::tokio::spawn), carrying the active invocation context and span.
///
/// # Panics
///
/// Outside a Tokio runtime, as `tokio::spawn`.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    ::tokio::spawn(CapturedContext::active().enter(future))
}

/// [`tokio::task::spawn_local`](::tokio::task::spawn_local), carrying the active invocation
/// context and span onto the current `LocalSet`.
///
/// # Panics
///
/// Outside a `LocalSet` or a local runtime, as `tokio::task::spawn_local`.
pub fn spawn_local<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    ::tokio::task::spawn_local(CapturedContext::active().enter(future))
}

/// [`tokio::task::spawn_blocking`](::tokio::task::spawn_blocking), running `body` on the
/// blocking pool inside the active invocation context and span.
///
/// # Panics
///
/// Outside a Tokio runtime, as `tokio::task::spawn_blocking`.
pub fn spawn_blocking<F, R>(body: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    ::tokio::task::spawn_blocking(CapturedContext::active().bind(body))
}
