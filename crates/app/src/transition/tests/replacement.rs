//! The `Replace` strategy: explicit decisions need a declared replacement factory and an
//! active instance, and the candidate root builds the replacement from the supplied
//! previous instance.

use upwell_di::Previous;

use super::*;

async fn carry_replaceable(previous: Previous<Replaceable>) -> Replaceable {
    Replaceable {
        label: previous.label,
    }
}

const REPLACEABLE_FACTORIES: fn() -> &'static [ComponentFactoryDescriptor] = upwell_di::factories![
    "candidate" => candidate_replaceable,
    replace "carry" => carry_replaceable,
];

const REPLACE: ComponentTransitionDecision = ComponentTransitionDecision {
    component: "replaceable",
    strategy: ComponentTransitionStrategy::Replace,
};

fn replacement_fixture() -> (EffectiveGraph, CandidateGraph) {
    graphs(
        &registry([replaceable(ACTIVE_FACTORIES)], []),
        &registry([replaceable(REPLACEABLE_FACTORIES)], []),
    )
}

fn previous_replaceable(label: &'static str) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<Replaceable>(Replaceable::NAME),
        value: Box::new(Injectable::into_stored(Arc::new(Replaceable { label }))),
    }
}

fn assert_restart(error: Error, expected: RestartReason) {
    assert!(
        matches!(
            error,
            Error::RestartRequired(RestartRequired { reason, .. }) if reason == expected
        ),
        "expected {expected:?}, got {error:?}"
    );
}

#[test]
fn replacement_requires_a_declared_replacement_factory() {
    let (active, candidate) = graphs(
        &registry([replaceable(ACTIVE_FACTORIES)], []),
        &registry([replaceable(CANDIDATE_FACTORIES)], []),
    );

    let error = candidate
        .resolve_transition(&active, [REPLACE])
        .expect_err("the candidate declares no replacement factory");

    assert_restart(error, RestartReason::ReplacementUnavailable);
}

#[test]
fn added_components_have_no_previous_instance_to_replace() {
    let (active, candidate) = graphs(
        &AppRegistry::default(),
        &registry([replaceable(REPLACEABLE_FACTORIES)], []),
    );

    let error = candidate
        .resolve_transition(&active, [REPLACE])
        .expect_err("an added component has no active instance");

    assert_restart(error, RestartReason::PreviousUnavailable);
}

#[tokio::test]
async fn replacement_builds_from_the_supplied_previous_instance() {
    let (active, candidate) = replacement_fixture();
    let resolved = candidate
        .resolve_transition(&active, [REPLACE])
        .expect("a declared replacement is supported");

    let (root, _) = resolved
        .build_candidate_root(
            &candidate,
            Vec::new(),
            vec![previous_replaceable("active")],
            ResolverSet::new(),
        )
        .await
        .expect("the replacement constructs the candidate root");
    let replacement = root
        .get::<Replaceable>()
        .expect("the replacement is in the candidate root");

    assert_eq!(resolved.construction_order(), ["replaceable"]);
    assert_eq!(
        replacement.label, "active",
        "the replacement factory, not the ordinary one, built the instance"
    );
}

#[tokio::test]
async fn replace_decisions_require_their_previous_instance() {
    let (active, candidate) = replacement_fixture();
    let resolved = candidate
        .resolve_transition(&active, [REPLACE])
        .expect("a declared replacement is supported");

    let result = resolved
        .build_candidate_root(&candidate, Vec::new(), Vec::new(), ResolverSet::new())
        .await;
    let Err(error) = result else {
        panic!("a replacement without its previous instance is rejected");
    };

    assert_restart(error, RestartReason::MissingDecision);
}

#[tokio::test]
async fn replacing_an_instance_without_a_snapshot_adapter_requires_restart() {
    let active_hooks = HookManager::new(Vec::new());
    let active_registry = registry([replaceable(ACTIVE_FACTORIES)], []);
    let candidate = candidate_graph(&registry([replaceable(REPLACEABLE_FACTORIES)], []));
    let view = active_view(&active_registry, vec![seed_hook_manager(&active_hooks)]).await;
    let resolved = candidate
        .resolve_transition(view.effective_graph(), [REPLACE])
        .expect("a declared replacement is supported");

    let result = resolved
        .build_candidate_root_from_active(&candidate, &view, ResolverSet::new())
        .await;
    let Err(error) = result else {
        panic!("a non-retainable previous instance cannot be snapshotted");
    };

    assert_restart(error, RestartReason::ManualInstanceUnsupported);
}
