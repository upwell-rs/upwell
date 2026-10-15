//! Runtime resolver mapping of structural Add, RebindLive, and Remove actions.

use super::*;

/// Active serves the live dependency through provider A; the candidate retargets the
/// identical live dependency to provider B, so the structural plan removes A, adds B,
/// and rebinds the consumer live.
fn live_retarget_fixture() -> (EffectiveGraph, CandidateGraph) {
    let (active, candidate) = graphs(
        &registry(
            [live_provider_a(), live_consumer()],
            [live_service_provider::<LiveProviderA>()],
        ),
        &registry(
            [live_provider_b(), live_consumer()],
            [live_service_provider::<LiveProviderB>()],
        ),
    );

    (active, candidate)
}

#[test]
fn runtime_resolver_maps_structural_add_to_reconstruct() {
    let (active, candidate) = graphs(
        &registry([replaceable(ACTIVE_FACTORIES)], []),
        &registry(
            [replaceable(CANDIDATE_FACTORIES), unrelated_singleton()],
            [],
        ),
    );

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("a structural add resolves without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 2);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "unrelated",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "replaceable",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
}

#[test]
fn runtime_resolver_maps_rebind_live_to_reconstruct() {
    let (active, candidate) = live_retarget_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("a live retarget resolves without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 2);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-provider-b",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
}

#[test]
fn runtime_resolver_omits_removed_components_by_absence() {
    let (active, candidate) = live_retarget_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("a removal retires by absence without a restart");

    assert!(matches!(
        resolved
            .structural()
            .nodes
            .iter()
            .find(|node| node.component == "live-provider-a"),
        Some(PlannedNode {
            action: NodeAction::Remove,
            ..
        })
    ));
    assert!(
        !resolved
            .decisions()
            .iter()
            .any(|decision| decision.component == "live-provider-a"),
        "a removed component carries no decision and requires no candidate seed"
    );
}
