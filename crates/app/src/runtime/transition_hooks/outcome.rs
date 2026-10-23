//! Transition outcome observers: [`TransitionCommitted`] and [`TransitionAborted`].
//!
//! Both kinds are observer-only: every `#[hook(TransitionCommitted)]` or
//! `#[hook(TransitionAborted)]` is an observer of an outcome that is already final, so it
//! cannot roll back, reject, or delay a later reload. They receive the attempt's final
//! redacted [`RuntimeTransitionReport`] and fire for every reload cause.
//!
//! # Timing
//!
//! Observers fire after the attempt has released both serialization leases and left the
//! reentrancy scope, and before [`reload_config`](crate::AppRuntime::reload_config) or the
//! trigger that requested the attempt returns. An observer may therefore request a
//! follow-up reload: it runs as a new attempt instead of failing with
//! [`ReentrantReload`](crate::Error::ReentrantReload). Another attempt may already be
//! running or committed by the time an observer runs, so observers rely on the report's
//! generation IDs rather than the runtime's current generation.
//!
//! - [`TransitionCommitted`] fires on the **published** generation's hooks, once per
//!   committed attempt, after every displaced component received
//!   [`ComponentRetired`](crate::ComponentRetired).
//! - [`TransitionAborted`] fires on the **base** generation's hooks, which stayed active,
//!   once per rejected attempt.
//!
//! A no-op attempt fires neither. Neither fires for an attempt that was cancelled (its
//! future dropped) or for a reload rejected as reentrant before it acquired the leases;
//! those produce no report.
//!
//! # Failures
//!
//! A failing or panicking observer is traced as `transition outcome observer failed` on
//! `upwell::transition`, with the attempt, hook kind, component, and failure category but
//! never its error message or panic payload. It never affects the published generation,
//! other observers, or later reloads.
//!
//! # Lifecycle ordering
//!
//! Reload triggers start after the `Startup` event and stop before the `Shutdown` event, so
//! triggered outcome observers fire strictly between them. A direct
//! [`reload_config`](crate::AppRuntime::reload_config) call fires them whenever it is made.

use upwell_core::DependencyDescriptor;
use upwell_hooks::{HookKind, HookParam, HookParticipation};

use super::super::RuntimeView;
use super::retirement::retire_displaced;
use super::{HookOwners, event_dependency, failed_observers};
use crate::RuntimeTransitionReport;
use crate::runtime::report::TRANSITION_TARGET;

/// Fired once after an attempt published a new generation, on that generation's hooks.
pub struct TransitionCommitted;

impl HookKind for TransitionCommitted {
    type Output = ();
    type Cx = TransitionOutcomeContext;

    const NAME: &'static str = "transition_committed";
    const PARTICIPATION: HookParticipation = HookParticipation::ObserversOnly;
}

/// Fired once after an attempt was rejected, on the hooks of the generation that stayed
/// active.
pub struct TransitionAborted;

impl HookKind for TransitionAborted {
    type Output = ();
    type Cx = TransitionOutcomeContext;

    const NAME: &'static str = "transition_aborted";
    const PARTICIPATION: HookParticipation = HookParticipation::ObserversOnly;
}

/// The context a [`TransitionCommitted`] or [`TransitionAborted`] observer's parameters
/// are extracted from: the attempt's final redacted report.
pub struct TransitionOutcomeContext {
    report: RuntimeTransitionReport,
}

impl<K: HookKind<Cx = TransitionOutcomeContext>> HookParam<K> for RuntimeTransitionReport {
    fn dependency(_path: Option<&'static str>) -> DependencyDescriptor {
        event_dependency::<Self>("RuntimeTransitionReport")
    }

    fn extract(
        cx: &TransitionOutcomeContext,
        _path: Option<&'static str>,
    ) -> upwell_hooks::Result<Self> {
        Ok(cx.report.clone())
    }
}

/// Fires the outcome observers of one finished attempt: for a committed attempt, the
/// displaced components' retirement on the base generation and then committed observers on
/// the published generation; for a rejected one, aborted observers on the base generation.
pub(crate) async fn observe_outcome(
    outcome: &crate::Result<RuntimeTransitionReport>,
    base: &RuntimeView,
    published: Option<&RuntimeView>,
) {
    match (outcome, published) {
        (Ok(report), Some(published)) if report.is_committed() => {
            retire_displaced(report, base).await;
            fire::<TransitionCommitted>(published, report).await;
        }
        (Err(crate::Error::TransitionRejected(rejection)), _) => {
            fire::<TransitionAborted>(base, rejection.report()).await;
        }
        _ => {}
    }
}

async fn fire<K>(view: &RuntimeView, report: &RuntimeTransitionReport)
where
    K: HookKind<Cx = TransitionOutcomeContext>,
{
    let hooks = view.hooks();

    if !hooks.has::<K>() {
        return;
    }

    let cx = TransitionOutcomeContext {
        report: report.clone(),
    };
    let fired = hooks.fire_all::<K>(&cx).await;
    let owners = HookOwners::new(view.resolved_components());

    for (component, outcome) in failed_observers(fired.observers, &owners) {
        tracing::warn!(
            target: TRANSITION_TARGET,
            attempt = report.attempt().get(),
            hook = K::NAME,
            component,
            outcome = outcome.as_str(),
            "transition outcome observer failed"
        );
    }
}
