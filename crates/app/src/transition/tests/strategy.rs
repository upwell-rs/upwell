//! Component transition decisions and the resolved plan they produce.

use super::*;

#[test]
fn retained_live_rebinding_requires_restart_until_bindings_can_be_applied() {
    let candidate = CandidateGraph::prepare(
        RuntimeGenerationId::INITIAL,
        &AppRegistry::default(),
        &topology(),
    )
    .expect("empty candidate validates");
    let node = PlannedNode {
        component: "live-consumer",
        role: EffectiveNodeRole::Singleton,
        action: NodeAction::RebindLive,
        reasons: Box::new([]),
    };

    let error = validate_decision(&candidate, &node, Some(ComponentTransitionStrategy::Retain))
        .expect_err("live binding transitions are not applied yet");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            component: "live-consumer",
            required: Some(NodeAction::RebindLive),
            reason: RestartReason::LiveRebindUnsupported,
        })
    ));
}

fn strategy_fixture() -> (EffectiveGraph, CandidateGraph) {
    graphs(
        &registry([replaceable(ACTIVE_FACTORIES)], []),
        &registry([replaceable(CANDIDATE_FACTORIES)], []),
    )
}

#[test]
fn replacement_requires_an_explicit_component_decision() {
    let (active, candidate) = strategy_fixture();

    let error = candidate
        .resolve_transition(&active, [])
        .expect_err("missing component decision requires restart");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            reason: RestartReason::MissingDecision,
            ..
        })
    ));
}

#[test]
fn replacement_cannot_retain_a_stale_instance() {
    let (active, candidate) = strategy_fixture();

    let error = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Retain,
            }],
        )
        .expect_err("structural replacement cannot retain");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            reason: RestartReason::StaleRetention,
            ..
        })
    ));
}

#[test]
fn explicit_reconstruction_uses_candidate_graph_order() {
    let (active, candidate) = strategy_fixture();

    let resolved = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("ordinary reconstruction is supported");

    assert_eq!(resolved.construction_order(), ["replaceable"]);
    assert_eq!(resolved.decisions().len(), 1);
}

#[test]
fn duplicate_component_decisions_are_rejected() {
    let (active, candidate) = strategy_fixture();
    let decision = ComponentTransitionDecision {
        component: "replaceable",
        strategy: ComponentTransitionStrategy::Reconstruct,
    };

    let error = candidate
        .resolve_transition(&active, [decision, decision])
        .expect_err("duplicate component decisions are invalid");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            reason: RestartReason::DuplicateDecision,
            ..
        })
    ));
}

#[test]
fn unknown_component_decisions_are_rejected() {
    let (active, candidate) = strategy_fixture();

    let error = candidate
        .resolve_transition(
            &active,
            [
                ComponentTransitionDecision {
                    component: "replaceable",
                    strategy: ComponentTransitionStrategy::Reconstruct,
                },
                ComponentTransitionDecision {
                    component: "unknown",
                    strategy: ComponentTransitionStrategy::Retain,
                },
            ],
        )
        .expect_err("unknown component decision is invalid");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            reason: RestartReason::UnknownDecision,
            ..
        })
    ));
}

#[tokio::test]
async fn resolved_plan_builds_only_the_candidate_factory() {
    let (active, candidate) = strategy_fixture();
    let resolved = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("ordinary reconstruction is supported");

    let (root, scopes) = resolved
        .build_candidate_root(&candidate, Vec::new(), ResolverSet::new())
        .await
        .expect("resolved plan constructs candidate root");

    assert!(root.belongs_to_registry(&scopes));
    assert!(root.resolve::<std::sync::Arc<Replaceable>>().await.is_ok());
}

#[tokio::test]
async fn resolved_plan_cannot_build_another_candidate() {
    let (active, candidate) = strategy_fixture();
    let (_, other_candidate) = strategy_fixture();
    let resolved = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("ordinary reconstruction is supported");

    let result = resolved
        .build_candidate_root(&other_candidate, Vec::new(), ResolverSet::new())
        .await;
    let Err(error) = result else {
        panic!("resolved plan is bound to its candidate");
    };

    assert!(matches!(error, Error::StaleGraphCandidate(_)));
}

#[tokio::test]
async fn retained_values_must_match_complete_plan_dispositions() {
    let retained = ComponentDescriptor::manual(
        "retained",
        "Retained",
        TypeDescriptor::of::<u8>("Retained"),
        &Singleton,
    );
    let (active_graph, candidate) = graphs(
        &registry([replaceable(ACTIVE_FACTORIES), retained], []),
        &registry([replaceable(CANDIDATE_FACTORIES), retained], []),
    );
    let resolved = candidate
        .resolve_transition(
            &active_graph,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("unchanged component receives retain disposition");

    let result = resolved
        .build_candidate_root(&candidate, Vec::new(), ResolverSet::new())
        .await;
    let Err(error) = result else {
        panic!("missing retained value is rejected");
    };

    assert!(
        matches!(
            error,
            Error::RestartRequired(RestartRequired {
                reason: RestartReason::MissingDecision,
                ..
            })
        ),
        "the low-level seam must still enforce complete plan dispositions"
    );
}

unit_component!(
    RootBoundConsumer,
    "root-bound",
    root_bound_consumer,
    deps [Arc<RootResolver>]
);

#[test]
fn unchanged_root_resolver_consumers_require_reconstruction() {
    let (active_graph, candidate) = graphs(
        &registry(
            [
                replaceable(ACTIVE_FACTORIES),
                upwell_di::root_resolver_descriptor(),
                root_bound_consumer(),
            ],
            [],
        ),
        &registry(
            [
                replaceable(CANDIDATE_FACTORIES),
                upwell_di::root_resolver_descriptor(),
                root_bound_consumer(),
            ],
            [],
        ),
    );

    let error = candidate
        .resolve_transition(
            &active_graph,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect_err("root-bound consumer cannot be retained");

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            reason: RestartReason::GenerationBoundDependency,
            ..
        })
    ));
}
