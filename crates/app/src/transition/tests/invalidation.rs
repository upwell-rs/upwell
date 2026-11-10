//! Runtime invalidation defaults for the transactional config reload.

use super::*;
use upwell_core::Scope;

struct ManualRoot;

/// Identical active and candidate graphs, so only runtime invalidations can force
/// transition work.
fn invalidation_fixture() -> (EffectiveGraph, CandidateGraph) {
    unchanged_graphs([invalidated_root(), fixed_dependent(), unrelated_singleton()])
}

#[test]
fn runtime_invalidations_reconstruct_invalidated_root_and_fixed_dependent() {
    let (active, candidate) = invalidation_fixture();
    let invalidated = BTreeSet::from(["invalidated-root"]);

    let resolved = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect("runtime invalidations resolve without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 3);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "invalidated-root",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "fixed-dependent",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "unrelated",
        strategy: ComponentTransitionStrategy::Retain,
    }));
    assert_eq!(
        resolved.construction_order(),
        ["invalidated-root", "fixed-dependent"]
    );
}

#[test]
fn runtime_invalidated_factoryless_singleton_requires_restart() {
    let (active, candidate) = unchanged_graphs([ComponentDescriptor::manual(
        "manual-root",
        "ManualRoot",
        TypeDescriptor::of::<ManualRoot>("ManualRoot"),
        Singleton::KEY,
    )]);
    let invalidated = BTreeSet::from(["manual-root"]);

    let error = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect_err("a factoryless invalidated singleton cannot reconstruct");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            component: "manual-root",
            required: Some(NodeAction::Replace),
            reason: RestartReason::FactoryUnavailable,
        })
    ));
}

struct CountedTransient;

static TRANSIENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

test_component!(CountedTransient, "counted-transient");

async fn construct_counted_transient() -> CountedTransient {
    TRANSIENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

    CountedTransient
}

fn counted_transient() -> ComponentDescriptor {
    ComponentDescriptor::of::<CountedTransient>()
        .with_scope(Transient::KEY)
        .with_factories(upwell_di::factories!["counted-transient" => construct_counted_transient])
        .without_retention()
}

#[test]
fn runtime_invalidated_non_singleton_is_replanned_without_construction() {
    let (active, candidate) = unchanged_graphs([counted_transient()]);
    let invalidated = BTreeSet::from(["counted-transient"]);

    let resolved = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect("an invalidated non-singleton is replanned, not restarted");

    assert!(
        resolved
            .decisions()
            .iter()
            .all(|decision| decision.component != "counted-transient"),
        "the non-singleton carries no decision: it is replanned for future resolutions"
    );
    assert!(
        resolved
            .structural()
            .nodes
            .iter()
            .any(|node| node.component == "counted-transient" && node.action == NodeAction::Replace),
        "the planner still reports the non-singleton as affected"
    );
    assert_eq!(
        TRANSIENT_CONSTRUCTIONS.load(Ordering::SeqCst),
        0,
        "resolution never runs a factory callback"
    );
}
