//! rayon's entry points, carrying the active invocation context and span into pool jobs.
//!
//! rayon has no per-job hook, so each function binds the closures it hands to the pool, and
//! [`ParallelIteratorExt::current`] binds every fold and reduction of a parallel iterator.
//! Outside every context they carry only the span and run the work with any context of the
//! worker masked, and otherwise behave as the rayon function they wrap.
//!
//! # Shared pools
//!
//! A bound job that blocks in a nested `join`, `scope` or parallel iterator may run other
//! jobs of the pool on its thread while it waits, and an unbound job run there sees the
//! waiting job's context. When a context selects identity, tenant data or credentials, bind
//! every job of a pool shared with context-carrying work, or give that work its own pool and
//! [`install`] into it.

mod iter;
mod scope;

pub use iter::{ParallelIteratorExt, WithContext};
pub use scope::{Scope, scope};

use super::CapturedContext;

/// [`rayon::join`](::rayon::join), running both closures inside the active invocation
/// context and span, whichever thread steals them.
///
/// # Panics
///
/// When either closure panics, after both ended, as `rayon::join`; and when rayon's global
/// pool cannot start.
pub fn join<A, B, RA, RB>(left: A, right: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    let left_context = CapturedContext::active();
    let right_context = left_context.clone();

    ::rayon::join(left_context.bind(left), right_context.bind(right))
}

/// [`rayon::spawn`](::rayon::spawn), running `body` on the global pool inside the active
/// invocation context and span. The job keeps the invocation's scope chain alive until it
/// ends.
///
/// # Panics
///
/// When rayon's global pool cannot start, as `rayon::spawn`.
pub fn spawn<F>(body: F)
where
    F: FnOnce() + Send + 'static,
{
    ::rayon::spawn(CapturedContext::active().bind(body));
}

/// [`ThreadPool::install`](::rayon::ThreadPool::install), running `body` in `pool` inside the
/// active invocation context and span.
///
/// # Panics
///
/// When `body` panics, as `ThreadPool::install`.
pub fn install<F, R>(pool: &::rayon::ThreadPool, body: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    pool.install(CapturedContext::active().bind(body))
}
