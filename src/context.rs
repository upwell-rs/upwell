//! The invocation context each framework entry point runs in.
//!
//! A singleton reads a value of the active invocation through the declared
//! [`Current<T>`](Current) and [`Scoped<S, T>`](Scoped) dependencies. Work spawned from an
//! invocation sees its context only when it is carried: through [`tokio`], [`thread`] and,
//! with the `rayon` feature, `rayon`, which mirror the executors they wrap, or through an
//! explicit [`CapturedContext`] for any other executor.

#[cfg(feature = "rayon")]
pub use upwell_di::context::rayon;
pub use upwell_di::context::{
    CapturedContext, ContextError, Current, Scoped, ScopedError, thread, tokio,
};

/// The reads behind `#[scope]`'s generated `S::is_active()` and `S::current()`.
#[doc(hidden)]
pub use upwell_di::context::{scope_is_active, scope_value};
