//! Carrying the invocation context into spawned work.
//!
//! Propagation is explicit: a plain `tokio::spawn`, `spawn_blocking` or `std::thread::spawn`
//! carries nothing. [`CapturedContext::capture`] clones the active context and the current
//! `tracing` span, and its methods re-enter both wherever the work runs: a tokio task, the
//! blocking pool, an OS thread, a rayon pool (with the `rayon` feature) or any future or
//! closure handed to another executor. The free functions [`spawn`], [`spawn_local`],
//! [`spawn_blocking`] and [`spawn_thread`] capture the active context when there is one.
//!
//! # Lifetime
//!
//! A capture holds its scope instance, and so the whole chain and the generation it pinned,
//! strongly until the captured work ends. A detached task therefore keeps a request's
//! components alive past the response; prefer structured concurrency (`JoinSet`, `join!`) for
//! work bound to a request. Cancellation is tokio's own: aborting or dropping a handle cancels
//! as usual, and nothing is tied to the response's completion.
//!
//! # Pending scopes
//!
//! A capture taken before the invocation's scope opens keeps that pending state, so the
//! captured work sees the scope's parent. Capture after the handler resolved anything from its
//! scope, which opens it.
//!
//! # Synchronous code
//!
//! While a context is entered synchronously ([`CapturedContext::run`], [`CapturedContext::bind`])
//! it lives in the same thread-local an entered future reads, so synchronous and asynchronous
//! code on that thread see the same context, and `Handle::block_on` inside resolves from it.
//! Each entry restores what the thread had before, so bound jobs sharing a pool never see each
//! other's contexts.

mod captured;
#[cfg(feature = "rayon")]
mod rayon;
mod spawn;

pub use captured::CapturedContext;
pub use spawn::{spawn, spawn_blocking, spawn_local, spawn_thread};

#[cfg(test)]
mod tests;
