use upwell_core::DependencyDescriptor;

use crate::Result;

/// A kind of hook: the event a `#[hook(Kind)]` method reacts to.
///
/// The kind owns the contract: `Output` is what each participant of this kind returns
/// (the action channel through which a participant affects the event), and `Cx` is the
/// owned, per-invocation context its parameters are extracted from (e.g. the proposed
/// config values for a reload). [`ends_event`](Self::ends_event) decides which
/// participant outcomes end the event, and [`PARTICIPATION`](Self::PARTICIPATION)
/// decides which [`HookRole`]s its hooks may take.
pub trait HookKind: 'static {
    /// What each participant of this kind returns and the runner collects.
    type Output: Send + 'static;

    /// The owned context one invocation carries, that this kind's [`HookParam`]s read.
    /// `Send + Sync` so the hook future (which borrows it) stays `Send`.
    type Cx: Send + Sync + 'static;

    /// A stable name for diagnostics and indexing.
    const NAME: &'static str;

    /// Which roles this kind's hooks may take. A `#[hook(Kind)]` without `observe` on an
    /// [`ObserversOnly`](HookParticipation::ObserversOnly) kind is an observer.
    const PARTICIPATION: HookParticipation = HookParticipation::ParticipantsAndObservers;

    /// Whether a participant's outcome ends the event: no later participant runs and no
    /// observer runs. Defaults to ending on any error, so a failed participant is
    /// fail-closed.
    fn ends_event(outcome: &Result<Self::Output>) -> bool {
        outcome.is_err()
    }
}

/// Which [`HookRole`]s the hooks of one [`HookKind`] may take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookParticipation {
    /// Hooks are participants unless declared `observe`.
    ParticipantsAndObservers,
    /// Every hook is an observer: the event has no outcome a hook could affect.
    ObserversOnly,
}

/// The authority one hook has over the event it was fired for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookRole {
    /// Returns the kind's [`Output`](HookKind::Output) and may end the event. Participants
    /// run sequentially in registration order before the event settles.
    Participant,
    /// Has no say in the event's outcome. Observers run concurrently once every
    /// participant accepted, and their errors are reported but never interpreted.
    Observer,
}

/// Whether a hook runs only when its firing site selects it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookRouting {
    /// Runs when the firing site's selection includes it, for example when the
    /// component it is declared on is affected by the event.
    Targeted,
    /// Runs on every event of its kind, regardless of the firing site's selection.
    Always,
}

/// A parameter a `#[hook(K)]` method may take: an input of kind `K`, extracted from the
/// kind's context — never a component dependency (those come through `&self`).
pub trait HookParam<K: HookKind>: Sized {
    /// The dependency edge this parameter contributes, for validation and event routing
    /// (e.g. the config path a reload hook targets). `path` is the param's `#[config("..")]`
    /// literal, or `None` for the by-type shorthand.
    fn dependency(path: Option<&'static str>) -> DependencyDescriptor;

    /// Extracts this parameter from the kind's context.
    fn extract(cx: &K::Cx, path: Option<&'static str>) -> Result<Self>;
}
