//! Spawning functions that carry the active invocation context when there is one.
//!
//! Each captures with [`CapturedContext::capture`] and, outside every context, spawns plainly:
//! there is nothing to carry, so the error is dropped. Capture explicitly to require a context.

use std::future::Future;
use std::thread;

use tokio::task::JoinHandle;
use tracing::Instrument;

use super::CapturedContext;

/// `tokio::spawn`, carrying the active invocation context and span.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match CapturedContext::capture() {
        Ok(captured) => captured.spawn(future),
        Err(_) => tokio::spawn(future.in_current_span()),
    }
}

/// `tokio::task::spawn_local`, carrying the active invocation context and span.
///
/// # Panics
///
/// As `tokio::task::spawn_local`, outside a `LocalSet`.
pub fn spawn_local<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    match CapturedContext::capture() {
        Ok(captured) => captured.spawn_local(future),
        Err(_) => tokio::task::spawn_local(future.in_current_span()),
    }
}

/// `tokio::task::spawn_blocking`, carrying the active invocation context and span.
pub fn spawn_blocking<R>(body: impl FnOnce() -> R + Send + 'static) -> JoinHandle<R>
where
    R: Send + 'static,
{
    match CapturedContext::capture() {
        Ok(captured) => captured.spawn_blocking(body),
        Err(_) => {
            let span = tracing::Span::current();

            tokio::task::spawn_blocking(move || span.in_scope(body))
        }
    }
}

/// `std::thread::spawn`, carrying the active invocation context and span.
pub fn spawn_thread<R>(body: impl FnOnce() -> R + Send + 'static) -> thread::JoinHandle<R>
where
    R: Send + 'static,
{
    match CapturedContext::capture() {
        Ok(captured) => captured.spawn_thread(body),
        Err(_) => {
            let span = tracing::Span::current();

            thread::spawn(move || span.in_scope(body))
        }
    }
}
