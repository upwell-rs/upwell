//! Carrying the invocation context into spawned work.
//!
//! Propagation is explicit: a plain `tokio::spawn`, `spawn_blocking` or `std::thread::spawn`
//! carries nothing. The modules here mirror the executors they wrap and carry the active
//! context, with its [`InvocationTarget`](super::InvocationTarget) and scope chain intact,
//! plus the current `tracing` span:
//!
//! - [`tokio`]: [`spawn`](tokio::spawn), [`spawn_local`](tokio::spawn_local) and
//!   [`spawn_blocking`](tokio::spawn_blocking);
//! - [`thread`]: [`spawn`](thread::spawn) and [`scope`](thread::scope);
//! - `rayon`, with the `rayon` feature: `join`, `scope`, `spawn`, `install` and the
//!   `.current()` parallel-iterator adapter.
//!
//! Each function captures the context where it is called. Outside every context it carries
//! only the span, runs the work with any context of the thread or task it lands on masked, and
//! otherwise behaves as the function it wraps. To require a context, or to
//! carry one into another executor (a `JoinSet`, a custom pool), capture it explicitly with
//! [`CapturedContext::capture`] and use [`enter`](CapturedContext::enter),
//! [`run`](CapturedContext::run), [`bind`](CapturedContext::bind) or
//! [`bind_fn`](CapturedContext::bind_fn).
//!
//! # Lifetime and cancellation
//!
//! A capture holds its scope instance, and so the whole chain and the generation it pinned,
//! strongly until the captured work ends. Cancellation is the executor's own and nothing is
//! tied to the invocation's completion: dropping a tokio `JoinHandle` detaches the task
//! rather than cancelling it, and aborting a `spawn_blocking` task that already runs does not
//! stop it. A detached task therefore keeps a request's components alive past the response.
//! Await request-bound work (`JoinSet`, `join!`, a scoped thread) or manage its handle
//! explicitly.
//!
//! # Pending scopes
//!
//! A capture taken before the invocation's scope opens keeps that pending state, so the
//! captured work sees the scope's parent. Capture after the handler resolved anything from its
//! scope, which opens it.
//!
//! # Synchronous code
//!
//! While a context is entered synchronously ([`CapturedContext::run`], a bound closure) it
//! lives in the same thread-local an entered future reads, so synchronous and asynchronous
//! code on that thread see the same context, and `Handle::block_on` inside resolves from it.
//! Each entry restores what the thread had before, so bound jobs sharing a pool never see each
//! other's contexts.

mod captured;
#[cfg(feature = "rayon")]
pub mod rayon;
pub mod thread;
pub mod tokio;

pub use captured::CapturedContext;

#[cfg(test)]
mod tests;
