//! The invocation context: what the current task runs inside of.
//!
//! Every framework entry point (a protocol's dispatch, a job run, a lifecycle hook) runs its
//! whole future inside one [`InvocationContext`], kept in a task-local. It holds:
//!
//! - an [`InvocationTarget`] saying which entry point this is. The engine defines no concrete
//!   target: each protocol or subsystem defines its own typed targets and recognises them with
//!   [`downcast_ref`](dyn InvocationTarget::downcast_ref);
//! - the application runtime, opaque here ([`ContextRuntime`]), which supplies the root of the
//!   current generation while no scope is open;
//! - the innermost open scope instance. A `ScopeContainer` links to its parents, so the
//!   innermost instance is the whole chain up to the root, and it pins the runtime generation
//!   it was opened on.
//!
//! Reading the context is framework-internal: the lookups are `pub(crate)`, and other
//! framework crates only establish a context ([`InvocationContext::enter`]), capture it to
//! re-enter elsewhere ([`InvocationContext::capture`]), or bind a lazily opened scope
//! ([`bind_opened_scope`]). Users reach scoped values through declared dependencies only.
//! Outside every entry point a lookup fails with [`ContextError::OutsideContext`], never a
//! panic.
//!
//! # Scopes that open after the context
//!
//! A protocol may open an invocation's scope only when something first resolves from it, so a
//! request, call or message scope whose entry builds nothing is opened on its first
//! resolution. The context is established
//! before that, around the whole dispatch, with a *pending* scope: the scope it expects and,
//! when the scope opens under one, the parent instance. Until the scope opens, the parent is
//! the innermost scope; the opening then binds its container into the pending slot of the
//! running context through [`bind_opened_scope`].
//!
//! The slot is a once-cell stored inline in the context, which the task-local holds by value,
//! so establishing a context allocates nothing. Only code running in the dispatching task
//! sees the bind. A context captured before the scope opened keeps the pending state as it
//! was at the capture, so work it is re-entered in sees the scope only if it opens the scope
//! itself; capture after the scope opens to carry it.
//!
//! # Capture and re-entry
//!
//! [`InvocationContext::capture`] clones the active context: a few reference-count bumps and
//! no allocation. The clone holds its scope instance strongly, so the instance lives until the
//! clone is dropped. [`InvocationContext::enter`] runs a future inside it, on any task, and
//! [`InvocationContext::enter_sync`] runs a closure inside it, on any thread.
//!
//! # Public access and propagation
//!
//! Users never resolve from the context directly. They declare [`Current<T>`] or
//! [`Scoped<S, T>`] dependencies, which resolve from the active invocation on each call, and
//! carry the context into spawned work with the [`tokio`], [`thread`] and (with the `rayon`
//! feature) `rayon` modules, or with an explicit [`CapturedContext`] for any other executor.
//! The one direct read is a `#[scope]` type's own value, through its generated
//! `S::current()` and `S::is_active()` ([`scope_value`], [`scope_is_active`]).

mod access;
mod error;
mod invocation_context;
mod propagate;
mod runtime;
mod target;

pub use access::{Current, Scoped, ScopedError, scope_is_active, scope_value};
pub use error::ContextError;
pub use invocation_context::{Entered, InvocationContext, bind_opened_scope};
#[cfg(feature = "rayon")]
pub use propagate::rayon;
pub use propagate::{CapturedContext, thread, tokio};
pub use runtime::ContextRuntime;
pub use target::{InvocationTarget, TargetRef};

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
