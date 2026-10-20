//! Component retirement: [`ComponentRetired`] tells each singleton a committed generation
//! displaced that it no longer serves new resolutions.
//!
//! Retirement is logical displacement, not destruction. It fires immediately after the
//! attempt that published the new generation released its leases, while the attempt still
//! pins the displaced generation's root. Each displaced instance is the receiver of its own
//! retirement hooks, resolved through that old root rather than the replacement. Pinned
//! [`RuntimeView`]s, open scopes, and other `Arc` owners may keep using the instance after
//! retirement and until they release it, so a retirement hook must leave the instance
//! usable: stop accepting new work or schedule cleanup, but do not invalidate state a
//! pinned caller still reads. Retained, rebound, and newly added components never retire.
//!
//! # Firing
//!
//! [`ComponentRetired`] fires once per displaced singleton, in the report's
//! [`displaced`](crate::RuntimeTransitionReport::displaced) order: every consumer retires
//! before the components it holds a fixed dependency on, while components linked only
//! through live dependencies may retire in either order. A targeted hook runs only for its own component's
//! retirement; an `always` hook observes every component's retirement and reads which one
//! from its [`ComponentRetirement`]. Retirement runs before the attempt's
//! [`TransitionCommitted`](crate::TransitionCommitted) observers, outside the reentrancy
//! scope, so a retirement hook may request a follow-up reload.
//!
//! # Failures
//!
//! A failing or panicking retirement hook is traced as `component retirement hook failed`
//! on `upwell::transition`, with the attempt, retired component, hook owner, and failure
//! category but never its error message or panic payload. It never affects the published
//! generation, other retirements, or later reloads.
//!
//! # Final release
//!
//! Final destruction is observed separately. When the last owner of a displaced
//! generation's root lets go, the root's instances drop and one `displaced runtime
//! generation released` event is emitted on `upwell::transition`. The event carries the
//! generation, the attempt that displaced it, and the microseconds since displacement.
//! [`AppRuntime::displaced_generations`](crate::AppRuntime::displaced_generations) lists
//! the displaced generations whose roots are still alive. The current generation is never
//! reported. There is no release hook: its receivers would have to resolve through the root
//! that was just dropped.

use std::any::TypeId;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, RuntimeGenerationId,
    TypeDescriptor,
};
use upwell_di::NodeAction;
use upwell_hooks::{HookKind, HookParam, HookParticipation};

use super::super::RuntimeView;
use super::HookOwners;
use crate::RuntimeTransitionReport;
use crate::runtime::report::{TRANSITION_TARGET, TransitionAttemptId, TransitionOutcome};

/// Fired once on each singleton a committed generation displaced, with the displaced
/// instance as the receiver.
pub struct ComponentRetired;

impl HookKind for ComponentRetired {
    type Output = ();
    type Cx = RetirementContext;

    const NAME: &'static str = "component_retired";
    const PARTICIPATION: HookParticipation = HookParticipation::ObserversOnly;
}

/// Why a committed generation displaced a component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RetirementReason {
    /// The new generation constructed a replacement instance.
    Replaced,
    /// The new generation no longer contains the component.
    Removed,
}

impl RetirementReason {
    /// The reason's stable snake-case name, as used in tracing.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Replaced => "replaced",
            Self::Removed => "removed",
        }
    }
}

/// One component's retirement: which component, why, and the generations it moved
/// between. A [`ComponentRetired`] hook parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ComponentRetirement {
    /// The retired component's stable ID.
    pub component: &'static str,
    /// Why it was displaced.
    pub reason: RetirementReason,
    /// The attempt that displaced it.
    pub attempt: TransitionAttemptId,
    /// The generation the retired instance belongs to.
    pub retired_generation: RuntimeGenerationId,
    /// The generation that displaced it.
    pub successor_generation: RuntimeGenerationId,
}

/// The context a [`ComponentRetired`] hook's parameters are extracted from: one
/// component's retirement.
pub struct RetirementContext {
    retirement: ComponentRetirement,
}

impl HookParam<ComponentRetired> for ComponentRetirement {
    fn dependency(_path: Option<&'static str>) -> DependencyDescriptor {
        DependencyDescriptor {
            name: "ComponentRetirement",
            ty: TypeDescriptor::of::<ComponentRetirement>("ComponentRetirement"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: true,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Eager,
            observation: DependencyObservation::Snapshot,
        }
    }

    fn extract(cx: &RetirementContext, _path: Option<&'static str>) -> upwell_hooks::Result<Self> {
        Ok(cx.retirement)
    }
}

/// Fires [`ComponentRetired`] on the displaced generation's hooks for every component the
/// committed `report` displaced, in retirement order.
pub(super) async fn retire_displaced(report: &RuntimeTransitionReport, base: &RuntimeView) {
    let TransitionOutcome::Committed { generation } = *report.outcome() else {
        return;
    };

    if !base.hooks().has::<ComponentRetired>() {
        return;
    }

    let owners = HookOwners::new(base.resolved_components());

    for &component in report.displaced() {
        let Some(retirement) = retirement_of(report, component, generation) else {
            continue;
        };

        retire(base, &owners, retirement).await;
    }
}

fn retirement_of(
    report: &RuntimeTransitionReport,
    component: &'static str,
    successor_generation: RuntimeGenerationId,
) -> Option<ComponentRetirement> {
    let reason = match report.component(component)?.action {
        NodeAction::Replace => RetirementReason::Replaced,
        NodeAction::Remove => RetirementReason::Removed,
        NodeAction::Retain | NodeAction::RebindLive | NodeAction::Add => return None,
    };

    Some(ComponentRetirement {
        component,
        reason,
        attempt: report.attempt(),
        retired_generation: report.base_generation(),
        successor_generation,
    })
}

/// Fires one component's retirement. A displaced ID missing from the base generation's
/// descriptors would leave its targeted hooks unselected, so that case is traced rather
/// than silently skipped; `always` hooks still run.
async fn retire(base: &RuntimeView, owners: &HookOwners, retirement: ComponentRetirement) {
    let ty = component_type(base, retirement.component);
    let cx = RetirementContext { retirement };

    if ty.is_none() {
        debug_assert!(
            false,
            "a displaced component is missing from its generation"
        );
        tracing::warn!(
            target: TRANSITION_TARGET,
            attempt = retirement.attempt.get(),
            component = retirement.component,
            "displaced component has no descriptor; its targeted retirement hooks are skipped"
        );
    }

    let fired = base
        .hooks()
        .fire::<ComponentRetired>(&cx, |hook| Some(hook.component_ty.type_id) == ty)
        .await;

    for (owner, outcome) in fired.observers {
        let Err(error) = outcome else {
            continue;
        };
        let category = match error {
            upwell_hooks::Error::Panicked { .. } => "panicked",
            _ => "failed",
        };

        tracing::warn!(
            target: TRANSITION_TARGET,
            attempt = retirement.attempt.get(),
            component = retirement.component,
            reason = retirement.reason.as_str(),
            hook = ComponentRetired::NAME,
            owner = owners.id(&owner),
            outcome = category,
            "component retirement hook failed"
        );
    }
}

fn component_type(base: &RuntimeView, component: &str) -> Option<TypeId> {
    base.resolved_components()
        .iter()
        .find(|descriptor| descriptor.id == component)
        .map(|descriptor| descriptor.ty.type_id)
}
