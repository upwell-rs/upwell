//! Runtime invalidation of live-observing consumers and their closures.

use super::*;

unit_component!(
    LiveInvalidatedConsumer,
    "live-invalidated-consumer",
    live_invalidated_consumer,
    deps [upwell_di::Dep<InvalidatedRoot>]
);
unit_component!(
    LiveConsumerDependent,
    "live-consumer-dependent",
    live_consumer_dependent,
    deps [Arc<LiveInvalidatedConsumer>]
);

struct ClosureTransient;

static CLOSURE_TRANSIENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

test_component!(ClosureTransient, "closure-transient");

async fn construct_closure_transient(_: Arc<LiveInvalidatedConsumer>) -> ClosureTransient {
    CLOSURE_TRANSIENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

    ClosureTransient
}

fn closure_transient() -> ComponentDescriptor {
    ComponentDescriptor::of::<ClosureTransient>()
        .with_scope(&Transient)
        .with_factories(upwell_di::factories!["closure-transient" => construct_closure_transient])
        .without_retention()
}

/// Identical active and candidate graphs where the invalidated root feeds a live
/// consumer whose own snapshot dependents must join the reconstruction closure.
fn live_invalidation_fixture() -> (EffectiveGraph, CandidateGraph) {
    unchanged_graphs([
        invalidated_root(),
        live_invalidated_consumer(),
        live_consumer_dependent(),
        unrelated_singleton(),
    ])
}

#[test]
fn runtime_resolver_maps_the_invalidated_live_closure_to_reconstruct() {
    let (active, candidate) = live_invalidation_fixture();
    let invalidated = BTreeSet::from(["invalidated-root"]);

    let resolved = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect("the invalidated live closure resolves without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 4);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "invalidated-root",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-invalidated-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-consumer-dependent",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "unrelated",
        strategy: ComponentTransitionStrategy::Retain,
    }));
    assert_eq!(
        resolved.construction_order(),
        [
            "invalidated-root",
            "live-invalidated-consumer",
            "live-consumer-dependent"
        ]
    );
}

#[test]
fn runtime_invalidated_non_reconstructible_dependent_requires_restart_before_construction() {
    let (active, candidate) = unchanged_graphs([
        invalidated_root(),
        live_invalidated_consumer(),
        closure_transient(),
    ]);
    let invalidated = BTreeSet::from(["invalidated-root"]);

    let error = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect_err("a non-singleton closure dependent cannot join a runtime transition");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            component: "closure-transient",
            required: Some(NodeAction::Replace),
            reason: RestartReason::NonSingleton,
        })
    ));
    assert_eq!(
        CLOSURE_TRANSIENT_CONSTRUCTIONS.load(Ordering::SeqCst),
        0,
        "resolution must reject before any factory callback runs"
    );
}
