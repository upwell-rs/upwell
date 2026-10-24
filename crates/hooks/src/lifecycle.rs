//! Built-in process-lifecycle hook kinds: [`Startup`] and [`Shutdown`].
//!
//! Both run on singleton components (their `&self` resolves on the root container) and are
//! fired by the daemon's `serve`/`run`. They fill a gap no other extension point covers —
//! running code *after the daemon is built but before it serves*, and *on graceful stop* —
//! unlike per-component setup (`#[init]`) or request cross-cutting (middleware/guards). Take
//! their dependencies through `&self`; neither carries inputs.

use super::{HookKind, HookParticipation};

/// Fired once after the daemon is built and before it begins serving.
///
/// Participants (`#[hook(Startup)]`) are startup gates: they run sequentially, and an
/// `Err` from any of them aborts startup, so later gates never run and the daemon does not
/// begin accepting work. Observers (`#[hook(Startup, observe)]`) run once every gate has
/// passed, before serving begins; their errors are logged and never abort startup.
pub struct Startup;

impl HookKind for Startup {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "startup";
}

/// Fired once when the daemon begins a graceful shutdown (the serve/run loop has stopped).
///
/// Shutdown has no outcome a hook could affect, so it is observer-only: every
/// `#[hook(Shutdown)]` is an observer, all of them run, and their errors are logged while
/// shutdown proceeds regardless.
pub struct Shutdown;

impl HookKind for Shutdown {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "shutdown";
    const PARTICIPATION: HookParticipation = HookParticipation::ObserversOnly;
}
