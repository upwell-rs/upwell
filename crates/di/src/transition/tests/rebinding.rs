//! Runtime planning of live consumers the runtime rebinds in place.

use super::*;

fn binding(plan: &TransitionPlan, consumer: &str) -> Option<BindingTransition> {
    plan.bindings
        .iter()
        .find(|binding| binding.dependency.consumer == consumer)
        .copied()
}

#[test]
fn accepted_live_consumers_rebind_in_place_without_propagating() {
    let active = graph(false);
    let switched = graph(true);

    let plan = active
        .plan_runtime_transition(&switched, &BTreeSet::new(), |_| true)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert!(plan.rebinds_in_place("live-consumer"));
    assert_eq!(
        action(&plan, "live-consumer-dependent"),
        None,
        "a consumer rebound in place keeps its instance, so its dependents retain"
    );
    assert_eq!(action(&plan, "live-chain-consumer"), None);
    assert_eq!(action(&plan, "fixed-consumer"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "fixed-dependent"), Some(NodeAction::Replace));
    assert_eq!(plan.bindings.len(), 1);
}

#[test]
fn rejected_live_consumers_reconstruct_and_propagate() {
    let active = graph(false);
    let switched = graph(true);

    let plan = active
        .plan_runtime_transition(&switched, &BTreeSet::new(), |consumer| {
            consumer != "live-consumer"
        })
        .expect("candidate uses active base generation");

    assert_eq!(
        binding(&plan, "live-consumer").map(|binding| binding.in_place),
        Some(false)
    );
    assert!(!plan.rebinds_in_place("live-consumer"));
    assert_eq!(
        action(&plan, "live-consumer-dependent"),
        Some(NodeAction::Replace)
    );
    assert_eq!(
        action(&plan, "live-chain-consumer"),
        Some(NodeAction::RebindLive),
        "the replaced dependent's own live consumer is reached through propagation"
    );
    assert!(
        plan.rebinds_in_place("live-chain-consumer"),
        "an accepted live consumer further down the closure still rebinds in place"
    );
    assert_eq!(action(&plan, "live-chain-dependent"), None);
}

#[test]
fn live_dependents_of_an_invalidated_root_rebind_in_place() {
    let active = graph(false);
    let candidate = graph(false);
    let roots = BTreeSet::from(["default-authenticator"]);

    let plan = active
        .plan_runtime_transition(&candidate, &roots, |_| true)
        .expect("candidate uses active base generation");

    assert_eq!(
        action(&plan, "default-authenticator"),
        Some(NodeAction::Replace)
    );
    assert!(plan.rebinds_in_place("live-consumer"));
    assert_eq!(action(&plan, "live-consumer-dependent"), None);
    assert_eq!(action(&plan, "fixed-consumer"), Some(NodeAction::Replace));
}

#[test]
fn ordinary_planning_marks_rebindable_demands_in_place() {
    let active = graph(false);
    let switched = graph(true);

    let plan = active
        .plan_transition(&switched)
        .expect("candidate uses active base generation");

    assert!(plan.rebinds_in_place("live-consumer"));
    assert!(
        !plan.rebinds_in_place("fixed-consumer"),
        "a component without live bindings has nothing to rebind"
    );
}
