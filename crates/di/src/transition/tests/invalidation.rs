//! Runtime planning over invalidated roots and live closure propagation.

use super::*;

#[test]
fn invalidated_root_replaces_root_and_fixed_dependents_on_identical_graphs() {
    let active = graph(false);
    let candidate = graph(false);
    let roots = BTreeSet::from(["default-authenticator", "ghost"]);

    let plan = active
        .plan_runtime_transition(&candidate, &roots)
        .expect("candidate uses active base generation");

    assert_eq!(
        action(&plan, "default-authenticator"),
        Some(NodeAction::Replace)
    );
    assert_eq!(action(&plan, "fixed-consumer"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "fixed-dependent"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "unrelated"), None);
    assert_eq!(action(&plan, "ghost"), None);

    let root = plan
        .nodes
        .iter()
        .find(|node| node.component == "default-authenticator")
        .expect("invalidated root is planned");

    assert!(
        root.reasons
            .iter()
            .any(|reason| reason.kind == ReasonKind::RuntimeInvalidated && reason.path.is_empty())
    );
    assert!(plan.construction_order.contains(&"default-authenticator"));

    let dependent = plan
        .retirement_order
        .iter()
        .position(|component| *component == "fixed-dependent")
        .expect("fixed dependent retires");
    let consumer = plan
        .retirement_order
        .iter()
        .position(|component| *component == "fixed-consumer")
        .expect("fixed consumer retires");
    let root = plan
        .retirement_order
        .iter()
        .position(|component| *component == "default-authenticator")
        .expect("invalidated root retires");

    assert!(dependent < consumer && consumer < root);
}

#[test]
fn invalidated_root_rebinds_live_dependents_through_existing_propagation() {
    let active = graph(false);
    let candidate = graph(false);
    let roots = BTreeSet::from(["default-authenticator"]);

    let plan = active
        .plan_runtime_transition(&candidate, &roots)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert!(
        plan.bindings
            .iter()
            .any(|binding| binding.dependency.consumer == "live-consumer")
    );
}

#[test]
fn invalidated_root_replaces_the_fixed_dependent_of_a_reconstructed_live_consumer() {
    let active = graph(false);
    let candidate = graph(false);
    let roots = BTreeSet::from(["default-authenticator"]);

    let plan = active
        .plan_runtime_transition(&candidate, &roots)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert_eq!(
        action(&plan, "live-consumer-dependent"),
        Some(NodeAction::Replace)
    );

    let dependent = plan
        .nodes
        .iter()
        .find(|node| node.component == "live-consumer-dependent")
        .expect("the fixed dependent of the reconstructed live consumer is planned");

    assert!(
        dependent
            .reasons
            .iter()
            .any(|reason| reason.kind == ReasonKind::FixedDependent),
        "the fixed dependent must be explained by propagation, not a structural change"
    );
    assert!(plan.construction_order.contains(&"live-consumer-dependent"));
}

#[test]
fn invalidated_root_propagates_through_repeated_live_and_fixed_links() {
    let active = graph(false);
    let candidate = graph(false);
    let roots = BTreeSet::from(["default-authenticator"]);

    let plan = active
        .plan_runtime_transition(&candidate, &roots)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert_eq!(
        action(&plan, "live-consumer-dependent"),
        Some(NodeAction::Replace)
    );
    assert_eq!(
        action(&plan, "live-chain-consumer"),
        Some(NodeAction::RebindLive)
    );
    assert_eq!(
        action(&plan, "live-chain-dependent"),
        Some(NodeAction::Replace)
    );
    assert!(
        plan.bindings
            .iter()
            .any(|binding| binding.dependency.consumer == "live-consumer")
    );
    assert!(
        plan.bindings
            .iter()
            .any(|binding| binding.dependency.consumer == "live-chain-consumer")
    );

    let root = plan
        .construction_order
        .iter()
        .position(|component| *component == "default-authenticator")
        .expect("the invalidated root reconstructs");
    let fixed_dependent = plan
        .construction_order
        .iter()
        .position(|component| *component == "live-consumer-dependent")
        .expect("the first fixed dependent reconstructs");
    let chain_dependent = plan
        .construction_order
        .iter()
        .position(|component| *component == "live-chain-dependent")
        .expect("the closure tail reconstructs");

    assert!(root < fixed_dependent && fixed_dependent < chain_dependent);
}

#[test]
fn ordinary_planning_leaves_the_fixed_closure_of_live_rebinds_untouched() {
    let active = graph(false);
    let candidate = graph(true);

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert_eq!(action(&plan, "live-consumer-dependent"), None);
    assert_eq!(action(&plan, "live-chain-consumer"), None);
    assert_eq!(action(&plan, "live-chain-dependent"), None);
}

#[test]
fn runtime_planning_with_an_empty_invalidation_set_propagates_live_closures() {
    let active = graph(false);
    let identical = graph(false);
    let switched = graph(true);

    let identical_plan = active
        .plan_runtime_transition(&identical, &BTreeSet::new())
        .expect("candidate uses active base generation");

    assert!(identical_plan.is_noop());

    let plan = active
        .plan_runtime_transition(&switched, &BTreeSet::new())
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "custom-authenticator"), Some(NodeAction::Add));
    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert_eq!(
        action(&plan, "live-consumer-dependent"),
        Some(NodeAction::Replace)
    );
    assert_eq!(
        action(&plan, "live-chain-consumer"),
        Some(NodeAction::RebindLive)
    );
    assert_eq!(
        action(&plan, "live-chain-dependent"),
        Some(NodeAction::Replace)
    );
    assert_eq!(action(&plan, "fixed-consumer"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "fixed-dependent"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "unrelated"), None);
    assert_eq!(
        plan.bindings.len(),
        2,
        "both live consumers in the propagated closure rebind"
    );
}

#[test]
fn repeated_invalidated_planning_is_deterministic() {
    let active = graph(false);
    let candidate = graph(false);
    let roots = BTreeSet::from(["default-authenticator"]);

    assert_eq!(
        active.plan_runtime_transition(&candidate, &roots),
        active.plan_runtime_transition(&candidate, &roots)
    );
}

#[test]
fn invalidations_leave_the_structural_diff_unchanged() {
    let active = graph(false);
    let candidate = graph(true);
    let roots = BTreeSet::from(["default-authenticator"]);

    let ordinary = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");
    let invalidated = active
        .plan_runtime_transition(&candidate, &roots)
        .expect("candidate uses active base generation");

    assert_eq!(invalidated.diff, ordinary.diff);
    assert!(
        ordinary
            .diff
            .nodes
            .iter()
            .any(|change| change.component == "custom-authenticator")
    );
}

#[test]
fn one_sided_invalidations_preserve_structural_add_and_remove() {
    let active = graph(false);
    let mut candidate_registry = registry(true);
    candidate_registry
        .components
        .retain(|component| component.id != "unrelated");
    let candidate =
        EffectiveGraph::build(RuntimeGenerationId::INITIAL, &candidate_registry, |_, _| {
            true
        })
        .expect("candidate validates");
    let roots = BTreeSet::from(["unrelated", "custom-authenticator"]);

    let plan = active
        .plan_runtime_transition(&candidate, &roots)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "unrelated"), Some(NodeAction::Remove));
    assert_eq!(action(&plan, "custom-authenticator"), Some(NodeAction::Add));

    let removed = plan
        .nodes
        .iter()
        .find(|node| node.component == "unrelated")
        .expect("removed component is planned");
    let added = plan
        .nodes
        .iter()
        .find(|node| node.component == "custom-authenticator")
        .expect("added component is planned");

    assert!(
        removed
            .reasons
            .iter()
            .any(|reason| reason.kind == ReasonKind::Removed && reason.path.is_empty())
    );
    assert!(
        added
            .reasons
            .iter()
            .any(|reason| reason.kind == ReasonKind::Added && reason.path.is_empty())
    );
    assert!(
        removed
            .reasons
            .iter()
            .all(|reason| reason.kind != ReasonKind::RuntimeInvalidated)
    );
    assert!(
        added
            .reasons
            .iter()
            .all(|reason| reason.kind != ReasonKind::RuntimeInvalidated)
    );
}
