//! Explicit validation and runtime defaults for transition handlers and hot updates.

use super::*;
use crate::transition::strategy::validate_decision;
use upwell_di::{
    ComponentChange, GraphDiff, HotUpdate, OneShot, Previous, ReasonKind, TransitionPlan,
    TransitionReason,
};

fn decline(_: &ComponentChange) -> Option<OneShot<Replaceable>> {
    None
}

async fn keep(_: Previous<Replaceable>) -> HotUpdate<Replaceable> {
    HotUpdate::new(|_| ())
}

const RECIPE_FACTORIES: fn() -> &'static [ComponentFactoryDescriptor] = upwell_di::factories![
    "candidate" => candidate_replaceable,
    transition "decline" => decline,
    hot_update "keep" => keep,
];

/// A candidate graph holding `Replaceable` built from `factories`, plus a one-node plan
/// for it with `action`, `role` and one reason of each `kind`.
fn recipe_case(
    factories: fn() -> &'static [ComponentFactoryDescriptor],
    action: NodeAction,
    role: EffectiveNodeRole,
    kinds: &[ReasonKind],
) -> (CandidateGraph, TransitionPlan) {
    let candidate = candidate_graph(&registry([replaceable(factories)], []));
    let reasons = kinds
        .iter()
        .map(|&kind| TransitionReason {
            kind,
            path: Box::new([]),
        })
        .collect();
    let plan = TransitionPlan {
        base_generation: RuntimeGenerationId::INITIAL,
        diff: GraphDiff {
            nodes: Box::new([]),
        },
        nodes: Box::new([PlannedNode {
            component: "replaceable",
            role,
            action,
            reasons,
        }]),
        bindings: Box::new([]),
        construction_order: Box::new([]),
        retirement_order: Box::new([]),
    };

    (candidate, plan)
}

fn rejection(
    factories: fn() -> &'static [ComponentFactoryDescriptor],
    action: NodeAction,
    role: EffectiveNodeRole,
    kinds: &[ReasonKind],
    strategy: ComponentTransitionStrategy,
) -> RestartReason {
    let (candidate, plan) = recipe_case(factories, action, role, kinds);
    let error = validate_decision(&candidate, &plan, &plan.nodes[0], Some(strategy))
        .expect_err("the decision requires restart");

    match error {
        Error::RestartRequired(RestartRequired { reason, .. }) => reason,
        other => panic!("expected a restart requirement, got {other:?}"),
    }
}

#[test]
fn recipes_the_component_did_not_declare_require_restart() {
    let singleton = EffectiveNodeRole::Singleton;
    let fixed = &[ReasonKind::FixedDependent];

    assert_eq!(
        rejection(
            CANDIDATE_FACTORIES,
            NodeAction::Replace,
            singleton,
            fixed,
            ComponentTransitionStrategy::Transition,
        ),
        RestartReason::TransitionHandlerUnavailable
    );
    assert_eq!(
        rejection(
            CANDIDATE_FACTORIES,
            NodeAction::Replace,
            singleton,
            fixed,
            ComponentTransitionStrategy::HotUpdate,
        ),
        RestartReason::HotUpdateUnavailable
    );
}

#[test]
fn hot_updates_are_rejected_outside_their_supported_transitions() {
    let singleton = EffectiveNodeRole::Singleton;
    let transient = EffectiveNodeRole::Transient;

    for kind in [
        ReasonKind::FactoryChanged,
        ReasonKind::IdentityChanged,
        ReasonKind::DependencyContractChanged,
    ] {
        assert_eq!(
            rejection(
                RECIPE_FACTORIES,
                NodeAction::Replace,
                singleton,
                &[ReasonKind::FixedDependent, kind],
                ComponentTransitionStrategy::HotUpdate,
            ),
            RestartReason::HotUpdateIncompatible,
            "{kind:?}"
        );
    }

    assert_eq!(
        rejection(
            RECIPE_FACTORIES,
            NodeAction::Add,
            singleton,
            &[ReasonKind::Added],
            ComponentTransitionStrategy::HotUpdate,
        ),
        RestartReason::PreviousUnavailable
    );
    assert_eq!(
        rejection(
            RECIPE_FACTORIES,
            NodeAction::Replace,
            transient,
            &[ReasonKind::FixedDependent],
            ComponentTransitionStrategy::HotUpdate,
        ),
        RestartReason::NonSingleton
    );
}

#[test]
fn declared_recipes_validate_for_replaced_and_rebound_singletons() {
    for action in [NodeAction::Replace, NodeAction::RebindLive] {
        for strategy in [
            ComponentTransitionStrategy::Transition,
            ComponentTransitionStrategy::HotUpdate,
        ] {
            let (candidate, plan) = recipe_case(
                RECIPE_FACTORIES,
                action,
                EffectiveNodeRole::Singleton,
                &[ReasonKind::RuntimeInvalidated],
            );

            validate_decision(&candidate, &plan, &plan.nodes[0], Some(strategy))
                .unwrap_or_else(|error| panic!("{action:?}/{strategy:?}: {error}"));
        }
    }
}

#[test]
fn runtime_defaults_prefer_hot_update_then_transition_handler() {
    let (candidate, plan) = recipe_case(
        RECIPE_FACTORIES,
        NodeAction::Replace,
        EffectiveNodeRole::Singleton,
        &[ReasonKind::FixedDependent],
    );
    let (_, changed) = recipe_case(
        RECIPE_FACTORIES,
        NodeAction::Replace,
        EffectiveNodeRole::Singleton,
        &[ReasonKind::FactoryChanged],
    );
    let (plain, _) = recipe_case(
        CANDIDATE_FACTORIES,
        NodeAction::Replace,
        EffectiveNodeRole::Singleton,
        &[ReasonKind::FixedDependent],
    );

    assert_eq!(
        candidate
            .declared_strategy(&plan.nodes[0])
            .expect("recipes resolve"),
        ComponentTransitionStrategy::HotUpdate
    );
    assert_eq!(
        candidate
            .declared_strategy(&changed.nodes[0])
            .expect("recipes resolve"),
        ComponentTransitionStrategy::Transition,
        "a changed factory cannot update in place, so the handler decides"
    );
    assert_eq!(
        plain
            .declared_strategy(&plan.nodes[0])
            .expect("recipes resolve"),
        ComponentTransitionStrategy::Reconstruct
    );
}
