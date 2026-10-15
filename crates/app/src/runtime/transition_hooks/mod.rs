//! Pre-commit transition hooks: [`TransitionProposal`] and [`TransitionDecision`].
//!
//! Both kinds let components and operators inspect a pending runtime transition and reject
//! it before anything is published. They run inside the reload transaction, so the
//! reentrancy check of [`reload_config`](crate::AppRuntime::reload_config) applies.
//!
//! The user guide `docs/transition-hooks.md` covers the whole model: every phase, routing,
//! redaction, reentrancy, trigger sources, and the lifecycle hooks.
//!
//! # Phases
//!
//! A non-empty reload runs `stage → evaluate → plan → proposal → construct → hooks →
//! decision → commit`, then, once its leases are released,
//! [`ComponentRetired`](crate::ComponentRetired) for each displaced component and
//! [`TransitionCommitted`] or [`TransitionAborted`]:
//!
//! - **Proposal** runs after planning and before any candidate construction, against the
//!   **active** generation's hooks, because no candidate exists yet. A rejection here
//!   constructs nothing.
//! - **Decision** runs after candidate construction and the [`ConfigReload`] hooks, against
//!   the **candidate** generation's hooks, immediately before commit. Its
//!   [`PendingTransition`] carries the accepted config-reload outcomes, so those hooks are
//!   never re-run. A rejection drops the constructed candidate.
//!
//! Either rejection leaves config, components, and generations unchanged, under the
//! reload's rollback boundary.
//!
//! # Routing
//!
//! Participants are targeted at the components the transition plan affects, so not every
//! hook runs on every reload. A proposal participant runs only on an affected component
//! present in the active generation (replaced, rebound, or removed); a decision participant
//! only on an affected component present in the candidate (added, replaced, or rebound).
//! `#[hook(Kind, always)]` declares a hook that runs on every transition reaching its phase,
//! for operator policies on components the plan does not touch.
//!
//! Participants run sequentially in registration order and the first
//! [`TransitionVerdict::Reject`], error, or panic ends the attempt. Observers
//! (`#[hook(Kind, observe)]`) run afterwards, once every participant accepted; their
//! failures are recorded in the report and never reject.
//!
//! [`ConfigReload`]: upwell_config::ConfigReload

use std::sync::Arc;

use upwell_config::{
    CfgNext, ChangedBinding, ComponentHookReport, ConfigProperties, ConfigReload, ReloadProposal,
};
use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, RuntimeGenerationId,
    TypeDescriptor,
};
use upwell_hooks::{HookKind, HookParam};

use super::{ComponentTransitionReport, TransitionAttemptId, TransitionPhase};

mod fire;
mod outcome;
mod retirement;

pub(crate) use fire::{AffectedComponents, fire_gate};
pub(crate) use outcome::observe_outcome;
pub use outcome::{TransitionAborted, TransitionCommitted, TransitionOutcomeContext};
pub use retirement::{ComponentRetired, ComponentRetirement, RetirementContext, RetirementReason};

/// Fired after planning and before candidate construction, against the active generation.
/// A participant may reject the pending transition.
pub struct TransitionProposal;

impl HookKind for TransitionProposal {
    type Output = TransitionVerdict;
    type Cx = TransitionHookContext;

    const NAME: &'static str = "transition_proposal";

    fn ends_event(outcome: &upwell_hooks::Result<TransitionVerdict>) -> bool {
        !matches!(outcome, Ok(TransitionVerdict::Accept))
    }
}

/// Fired after candidate construction and the config-reload hooks, against the candidate
/// generation, immediately before commit. A participant may reject the pending transition.
pub struct TransitionDecision;

impl HookKind for TransitionDecision {
    type Output = TransitionVerdict;
    type Cx = TransitionHookContext;

    const NAME: &'static str = "transition_decision";

    fn ends_event(outcome: &upwell_hooks::Result<TransitionVerdict>) -> bool {
        !matches!(outcome, Ok(TransitionVerdict::Accept))
    }
}

/// A transition participant's verdict on the pending transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionVerdict {
    /// Let the transition continue.
    Accept,
    /// Reject the transition. The static reason code is recorded in the redacted report,
    /// so it must not carry config values or secrets.
    Reject(&'static str),
}

/// The context a [`TransitionProposal`] or [`TransitionDecision`] hook's parameters are
/// extracted from: the pending transition and the staged typed config.
pub struct TransitionHookContext {
    pending: PendingTransition,
    proposal: ReloadProposal,
}

impl TransitionHookContext {
    pub(crate) fn new(pending: PendingTransition, proposal: ReloadProposal) -> Self {
        Self { pending, proposal }
    }
}

/// The redacted view of a pending runtime transition a pre-commit hook inspects: why each
/// affected component transitions and which bindings changed, never config values.
///
/// Cheap to clone. Typed staged values are reached through `CfgNext<T>` parameters.
#[derive(Clone)]
pub struct PendingTransition {
    inner: Arc<PendingInner>,
}

struct PendingInner {
    attempt: TransitionAttemptId,
    base_generation: RuntimeGenerationId,
    phase: TransitionPhase,
    changed: Arc<[ChangedBinding]>,
    components: Arc<[ComponentTransitionReport]>,
    config_hooks: Box<[ComponentHookReport]>,
}

impl PendingTransition {
    /// The pending transition at the proposal phase.
    pub(crate) fn proposed(
        attempt: TransitionAttemptId,
        base_generation: RuntimeGenerationId,
        changed: &[ChangedBinding],
        components: &[ComponentTransitionReport],
    ) -> Self {
        Self {
            inner: Arc::new(PendingInner {
                attempt,
                base_generation,
                phase: TransitionPhase::Proposal,
                changed: changed.into(),
                components: components.into(),
                config_hooks: Box::default(),
            }),
        }
    }

    /// The same transition at the decision phase, carrying the accepted config-reload hook
    /// outcomes.
    pub(crate) fn decided(&self, config_hooks: &[ComponentHookReport]) -> Self {
        Self {
            inner: Arc::new(PendingInner {
                attempt: self.inner.attempt,
                base_generation: self.inner.base_generation,
                phase: TransitionPhase::Decision,
                changed: Arc::clone(&self.inner.changed),
                components: Arc::clone(&self.inner.components),
                config_hooks: config_hooks.into(),
            }),
        }
    }

    /// The attempt this transition belongs to.
    pub fn attempt(&self) -> TransitionAttemptId {
        self.inner.attempt
    }

    /// The committed generation the transition was planned against.
    pub fn base_generation(&self) -> RuntimeGenerationId {
        self.inner.base_generation
    }

    /// The pre-commit phase the hook runs in: [`Proposal`](TransitionPhase::Proposal) or
    /// [`Decision`](TransitionPhase::Decision).
    pub fn phase(&self) -> TransitionPhase {
        self.inner.phase
    }

    /// The bindings whose source changed: paths and type names, never values.
    pub fn changed(&self) -> &[ChangedBinding] {
        &self.inner.changed
    }

    /// Every component the transition plan affects, with its action and reason paths, in
    /// stable component order.
    pub fn components(&self) -> &[ComponentTransitionReport] {
        &self.inner.components
    }

    /// The affected component with the given stable ID, or `None` when the plan does not
    /// affect it.
    pub fn component(&self, id: &str) -> Option<&ComponentTransitionReport> {
        self.inner
            .components
            .iter()
            .find(|component| component.component == id)
    }

    /// The accepted config-reload hook outcomes. Empty during the proposal phase.
    pub fn config_hooks(&self) -> &[ComponentHookReport] {
        &self.inner.config_hooks
    }
}

/// The dependency edge a [`PendingTransition`] parameter reports: supplied by the firing
/// transition at runtime, so it is `dynamic` and exempt from static graph validation.
fn pending_dependency() -> DependencyDescriptor {
    DependencyDescriptor {
        name: "PendingTransition",
        ty: TypeDescriptor::of::<PendingTransition>("PendingTransition"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: true,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }
}

impl<K: HookKind<Cx = TransitionHookContext>> HookParam<K> for PendingTransition {
    fn dependency(_path: Option<&'static str>) -> DependencyDescriptor {
        pending_dependency()
    }

    fn extract(
        cx: &TransitionHookContext,
        _path: Option<&'static str>,
    ) -> upwell_hooks::Result<Self> {
        Ok(cx.pending.clone())
    }
}

impl<T: ConfigProperties> HookParam<TransitionProposal> for CfgNext<T> {
    fn dependency(path: Option<&'static str>) -> DependencyDescriptor {
        <Self as HookParam<ConfigReload>>::dependency(path)
    }

    fn extract(
        cx: &TransitionHookContext,
        path: Option<&'static str>,
    ) -> upwell_hooks::Result<Self> {
        staged_next(cx, path)
    }
}

impl<T: ConfigProperties> HookParam<TransitionDecision> for CfgNext<T> {
    fn dependency(path: Option<&'static str>) -> DependencyDescriptor {
        <Self as HookParam<ConfigReload>>::dependency(path)
    }

    fn extract(
        cx: &TransitionHookContext,
        path: Option<&'static str>,
    ) -> upwell_hooks::Result<Self> {
        staged_next(cx, path)
    }
}

fn staged_next<T: ConfigProperties>(
    cx: &TransitionHookContext,
    path: Option<&'static str>,
) -> upwell_hooks::Result<CfgNext<T>> {
    cx.proposal
        .next::<T>(path)
        .ok_or(upwell_hooks::Error::MissingParam(T::NAME))
}
