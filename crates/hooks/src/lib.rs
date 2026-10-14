//! A general lifecycle/event **hook** system.
//!
//! A hook is an `async` method on a component or service, marked `#[hook(Kind)]`, that the
//! framework calls when an event of `Kind` occurs. `Kind` is a [`HookKind`] type that owns
//! the hook's *output* (what the method returns) and its *context* (the typed inputs an
//! invocation carries). Built-in kinds are [`Startup`] and [`Shutdown`]; config reload is a
//! kind defined in `upwell-config`. The system is deliberately general so user-defined
//! event kinds can be added the same way.
//!
//! # Participants and observers
//!
//! A hook's [`HookRole`] is its authority over the event it was fired for. A
//! **participant** returns the kind's `Output`, the channel through which it affects the
//! event's outcome (accept, reject, abort, …), and an error or panic is fail-closed: the
//! kind's [`ends_event`](HookKind::ends_event) decides whether it ends the event. An
//! **observer** (`#[hook(Kind, observe)]`) has no say: it returns `()` or `Result<(), E>`,
//! and its failure is reported but never interpreted. Both may run arbitrary code; the
//! split is about who decides the outcome, not about who may fail.
//!
//! Observing is owned by the runtime, never by the firing site: [`HookManager::fire`]
//! runs participants sequentially in registration order, stops at the first outcome that
//! ends the event, and only then runs every observer concurrently. An observer therefore
//! never sees an event a participant can still change, nor one a participant ended. A kind
//! with no outcome to affect declares
//! [`ObserversOnly`](HookParticipation::ObserversOnly), making each of its hooks an
//! observer (e.g. [`Shutdown`]).
//!
//! Routing is independent of role: a firing site selects the hooks its event targets
//! (e.g. hooks on affected components), and `#[hook(Kind, always)]` opts a hook out of
//! that selection so it runs on every event of its kind.
//!
//! Hooks do **not** receive component dependencies as parameters — those are reached through
//! `&self` (a hook may also be self-less). A hook's only parameters are the kind's inputs,
//! each a [`HookParam`] extracted from the kind's context. Hooks are collected per type
//! (the `{Type}Hooks` distributed slice, exposed via [`ComponentHooks`]) and registered into
//! a [`HookManager`]; a type with no hooks contributes nothing at runtime.
//!
//! This crate is generic over the [`ResolverCtx`](upwell_core::ResolverCtx): a hook's
//! erased [`HookCall`] resolves its receiver through the resolver context, so the hook
//! layer never names the DI container (which sits above it).

mod descriptor;
mod error;
mod kind;
mod lifecycle;
mod manager;

pub use descriptor::{ComponentHooks, HookCall, HookDescriptor, no_hooks};
pub use error::{Error, Result};
pub use kind::{HookKind, HookParam, HookParticipation, HookRole, HookRouting};
pub use lifecycle::{Shutdown, Startup};
pub use manager::{FiredHooks, HookManager};

/// The stable component id of the seeded [`HookManager`] singleton.
pub const HOOK_MANAGER_ID: &str = "upwell:hook-manager";

/// The display name of the seeded [`HookManager`] singleton.
pub const HOOK_MANAGER_NAME: &str = "HookManager";

#[cfg(test)]
mod tests;
