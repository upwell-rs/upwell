//! The provider switch's transition report names the removed and added providers and
//! the fixed consumer that reconstructs, each with its action and reason path.

use upwell_di::{
    Component as _, DependencyDemandId, EffectiveNodeRole, NodeAction, ReasonKind, TransitionReason,
};

use super::fixture::{
    FallbackAuthenticator, PrimaryAuthenticator, build_auth_app, config_dir_of, write_auth_config,
};
use super::gate::AuthGate;
use crate::{ComponentTransitionReport, RuntimeTransitionReport, TransitionOutcome};

/// Builds a fresh auth app on the primary and switches it to the fallback.
async fn switch_to_fallback() -> RuntimeTransitionReport {
    let (dir, app) = build_auth_app("primary").await;

    write_auth_config(&config_dir_of(&dir), "fallback");

    app.runtime()
        .reload_config()
        .await
        .expect("the provider switch commits")
}

fn affected(report: &RuntimeTransitionReport) -> Vec<(&'static str, NodeAction)> {
    report
        .components()
        .iter()
        .map(|component| (component.component, component.action))
        .collect()
}

#[tokio::test]
async fn provider_switch_reports_replaced_providers_and_the_affected_consumer() {
    let gate_demand = DependencyDemandId {
        consumer: AuthGate::ID,
        ordinal: 0,
    };

    let report = switch_to_fallback().await;
    let gate = report
        .component(AuthGate::ID)
        .expect("the fixed consumer of the switched provider is affected");
    let removed = report
        .component(PrimaryAuthenticator::ID)
        .expect("the removed provider is reported");
    let added = report
        .component(FallbackAuthenticator::ID)
        .expect("the added provider is reported");

    assert!(matches!(
        report.outcome(),
        TransitionOutcome::Committed { .. }
    ));
    assert_eq!(
        affected(&report),
        [
            (FallbackAuthenticator::ID, NodeAction::Add),
            (AuthGate::ID, NodeAction::Replace),
            (PrimaryAuthenticator::ID, NodeAction::Remove),
        ],
        "affected components follow the planner's stable component order"
    );
    assert_eq!(
        removed
            .reasons
            .iter()
            .map(|reason| reason.kind)
            .collect::<Vec<_>>(),
        [ReasonKind::Removed]
    );
    assert_eq!(
        added
            .reasons
            .iter()
            .map(|reason| reason.kind)
            .collect::<Vec<_>>(),
        [ReasonKind::Added]
    );
    assert_eq!(gate.factory, Some("static"));
    assert_eq!(gate.role, EffectiveNodeRole::Singleton);
    assert_eq!(
        gate.reasons.as_ref(),
        [
            TransitionReason {
                kind: ReasonKind::DependencyTargetChanged,
                path: Box::new([gate_demand]),
            },
            TransitionReason {
                kind: ReasonKind::FixedDependent,
                path: Box::new([gate_demand]),
            },
        ],
        "the consumer's reason path runs through its Authenticator demand"
    );
    assert_eq!(report.changed().len(), 1);
    assert_eq!(report.changed()[0].path, "auth");
}

#[tokio::test]
async fn identical_switches_produce_identical_reports() {
    let first = switch_to_fallback().await;
    let second = switch_to_fallback().await;

    let first_components: &[ComponentTransitionReport] = first.components();

    assert_eq!(first_components, second.components());
    assert_eq!(first.outcome(), second.outcome());
    assert_eq!(first.base_generation(), second.base_generation());
    assert_eq!(first.attempt(), second.attempt());
    assert_eq!(
        first
            .changed()
            .iter()
            .map(|binding| (&binding.path, binding.type_name))
            .collect::<Vec<_>>(),
        second
            .changed()
            .iter()
            .map(|binding| (&binding.path, binding.type_name))
            .collect::<Vec<_>>()
    );
}
