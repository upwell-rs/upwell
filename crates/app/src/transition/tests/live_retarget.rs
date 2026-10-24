//! Runtime propagation of live retarget closures under an empty invalidation set.

use super::*;

unit_component!(
    RetargetDependent,
    "retarget-dependent",
    retarget_dependent,
    deps [Arc<LiveConsumer>]
);
unit_component!(
    RetargetChainConsumer,
    "retarget-chain-consumer",
    retarget_chain_consumer,
    deps [upwell_di::Dep<RetargetDependent>]
);
unit_component!(
    RetargetChainDependent,
    "retarget-chain-dependent",
    retarget_chain_dependent,
    deps [Arc<RetargetChainConsumer>]
);

struct RetargetClosureTransient;

static RETARGET_CLOSURE_TRANSIENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

test_component!(RetargetClosureTransient, "retarget-closure-transient");

async fn construct_retarget_closure_transient(_: Arc<LiveConsumer>) -> RetargetClosureTransient {
    RETARGET_CLOSURE_TRANSIENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

    RetargetClosureTransient
}

fn retarget_closure_transient() -> ComponentDescriptor {
    ComponentDescriptor::of::<RetargetClosureTransient>()
        .with_scope(&Transient)
        .with_factories(upwell_di::factories![
            "retarget-closure-transient" => construct_retarget_closure_transient
        ])
        .without_retention()
}

/// Active serves the live dependency through provider A; the candidate retargets the
/// identical live dependency to provider B. The live consumer's snapshot dependent and
/// the live chain behind it are structurally untouched, so only runtime propagation
/// can pull them into the reconstruction closure.
fn live_retarget_closure_fixture() -> (EffectiveGraph, CandidateGraph) {
    let (active, candidate) = graphs(
        &registry(
            [
                live_provider_a(),
                live_consumer(),
                retarget_dependent(),
                retarget_chain_consumer(),
                retarget_chain_dependent(),
            ],
            [live_service_provider::<LiveProviderA>()],
        ),
        &registry(
            [
                live_provider_b(),
                live_consumer(),
                retarget_dependent(),
                retarget_chain_consumer(),
                retarget_chain_dependent(),
            ],
            [live_service_provider::<LiveProviderB>()],
        ),
    );

    (active, candidate)
}

#[test]
fn runtime_resolver_propagates_the_live_retarget_closure_with_empty_invalidations() {
    let (active, candidate) = live_retarget_closure_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("the live retarget closure resolves without restart");
    let decisions = resolved.decisions();

    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(
        decisions.contains(&ComponentTransitionDecision {
            component: "retarget-dependent",
            strategy: ComponentTransitionStrategy::Reconstruct,
        }),
        "the snapshot dependent of the reconstructed live consumer must reconstruct, \
         not retain an instance snapshotted from the retired active root"
    );

    let structural = resolved.structural();

    assert!(matches!(
        structural
            .nodes
            .iter()
            .find(|node| node.component == "live-consumer"),
        Some(PlannedNode {
            action: NodeAction::RebindLive,
            ..
        })
    ));
    assert!(matches!(
        structural
            .nodes
            .iter()
            .find(|node| node.component == "retarget-dependent"),
        Some(PlannedNode {
            action: NodeAction::Replace,
            ..
        })
    ));
}

#[test]
fn runtime_resolver_propagates_repeated_live_chains_with_empty_invalidations() {
    let (active, candidate) = live_retarget_closure_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("the repeated live chain resolves without restart");
    let decisions = resolved.decisions();

    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "retarget-chain-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "retarget-chain-dependent",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert_eq!(
        resolved.construction_order(),
        [
            "live-provider-b",
            "live-consumer",
            "retarget-dependent",
            "retarget-chain-consumer",
            "retarget-chain-dependent",
        ],
        "the propagated closure reconstructs in dependency order"
    );
}

#[test]
fn runtime_resolver_replans_a_non_singleton_live_retarget_closure() {
    let (active, candidate) = graphs(
        &registry(
            [
                live_provider_a(),
                live_consumer(),
                retarget_closure_transient(),
            ],
            [live_service_provider::<LiveProviderA>()],
        ),
        &registry(
            [
                live_provider_b(),
                live_consumer(),
                retarget_closure_transient(),
            ],
            [live_service_provider::<LiveProviderB>()],
        ),
    );

    let invalidated = BTreeSet::new();
    let resolved = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect("a non-singleton closure dependent is replanned, not restarted");

    assert!(
        resolved
            .decisions()
            .iter()
            .all(|decision| decision.component != "retarget-closure-transient"),
        "the non-singleton carries no decision: it is replanned for future resolutions"
    );
    assert!(
        resolved
            .structural()
            .nodes
            .iter()
            .any(|node| node.component == "retarget-closure-transient"
                && node.action == NodeAction::Replace),
        "the planner still reports the non-singleton as affected"
    );
    assert_eq!(
        RETARGET_CLOSURE_TRANSIENT_CONSTRUCTIONS.load(Ordering::SeqCst),
        0,
        "resolution never runs a factory callback"
    );
}
