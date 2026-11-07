//! `std::thread`'s spawning functions, carrying the active invocation context and span.
//!
//! A detached thread keeps the invocation's scope chain alive until it ends; prefer
//! [`scope`], which joins every thread it spawns before it returns.

use std::thread::{self, JoinHandle, ScopedJoinHandle};

use super::CapturedContext;

/// [`std::thread::spawn`], running `body` on a new OS thread inside the active invocation
/// context and span.
///
/// # Panics
///
/// When the OS fails to create the thread, as `std::thread::spawn`.
pub fn spawn<F, T>(body: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::spawn(CapturedContext::active().bind(body))
}

/// [`std::thread::scope`], whose [`Scope::spawn`] runs every thread inside the invocation
/// context and span active where `body` starts.
///
/// # Panics
///
/// When a spawned thread panicked and was not joined, after every thread ended, as
/// `std::thread::scope`.
pub fn scope<'env, F, T>(body: F) -> T
where
    F: for<'scope> FnOnce(&Scope<'scope, 'env>) -> T,
{
    let context = CapturedContext::active();

    thread::scope(|inner| body(&Scope { inner, context }))
}

/// A [`std::thread::Scope`] whose threads carry a captured invocation context.
#[derive(Debug)]
pub struct Scope<'scope, 'env: 'scope> {
    inner: &'scope thread::Scope<'scope, 'env>,
    context: CapturedContext,
}

impl<'scope> Scope<'scope, '_> {
    /// [`std::thread::Scope::spawn`], running `body` inside the captured context and span.
    ///
    /// # Panics
    ///
    /// When the OS fails to create the thread, as `std::thread::Scope::spawn`.
    pub fn spawn<F, T>(&self, body: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        self.inner.spawn(self.context.clone().bind(body))
    }
}
