//! Structural transition planning: retention, replacement, retirement order and identity.

use super::*;

#[test]
fn provider_switch_retains_live_consumer_and_replaces_fixed_closure() {
    let active = graph(false);
    let candidate = graph(true);

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "custom-authenticator"), Some(NodeAction::Add));
    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert_eq!(action(&plan, "fixed-consumer"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "fixed-dependent"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "unrelated"), None);
    assert_eq!(plan.bindings.len(), 1);
}

#[test]
fn repeated_planning_is_deterministic() {
    let active = graph(false);
    let candidate = graph(true);

    assert_eq!(
        active.plan_transition(&candidate),
        active.plan_transition(&candidate)
    );
}

#[test]
fn identical_graphs_produce_a_no_op_plan() {
    let active = graph(false);
    let candidate = graph(false);

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert!(plan.is_noop());
    assert!(
        plan.diff
            .nodes
            .iter()
            .all(|change| change.kinds.as_ref() == [NodeChangeKind::Unchanged])
    );
}

#[test]
fn replacement_suppresses_redundant_live_binding_work() {
    let active = graph(false);
    let mut candidate_registry = registry(true);
    let live = candidate_registry
        .components
        .iter_mut()
        .find(|component| component.id == "live-consumer")
        .expect("live consumer exists");
    live.factories = fixed_factory;
    let candidate =
        EffectiveGraph::build(RuntimeGenerationId::INITIAL, &candidate_registry, |_, _| {
            true
        })
        .expect("candidate validates");

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::Replace));
    assert!(
        plan.bindings
            .iter()
            .all(|binding| binding.dependency.consumer != "live-consumer")
    );
}

#[test]
fn retirement_orders_fixed_consumers_before_dependencies() {
    let active = graph(true);
    let candidate = graph(false);

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");
    let fixed = plan
        .retirement_order
        .iter()
        .position(|component| *component == "fixed-consumer")
        .expect("fixed consumer retires");
    let dependent = plan
        .retirement_order
        .iter()
        .position(|component| *component == "fixed-dependent")
        .expect("fixed dependent retires");

    assert!(dependent < fixed);
}

#[test]
fn stale_candidate_generation_is_rejected() {
    let active = graph(false);
    let candidate =
        EffectiveGraph::build(RuntimeGenerationId::new(1), &registry(true), |_, _| true)
            .expect("candidate validates");

    assert_eq!(
        active.plan_transition(&candidate),
        Err(StaleGraphCandidate {
            active: RuntimeGenerationId::INITIAL,
            candidate: RuntimeGenerationId::new(1),
        })
    );
}

#[test]
fn singleton_to_scoped_change_still_retires_the_active_singleton() {
    let active = graph(false);
    let mut candidate_registry = registry(false);
    let unrelated = candidate_registry
        .components
        .iter_mut()
        .find(|component| component.id == "unrelated")
        .expect("unrelated component exists");
    unrelated.scope = &RequestScope;
    let candidate = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &candidate_registry,
        |consumer, dependency| consumer == dependency,
    )
    .expect("candidate validates");

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert!(plan.retirement_order.contains(&"unrelated"));
    assert!(!plan.construction_order.contains(&"unrelated"));
}

#[test]
fn changed_recipe_identity_replaces_the_component() {
    let active = graph(false);
    let mut candidate_registry = registry(false);
    let unrelated = candidate_registry
        .components
        .iter_mut()
        .find(|component| component.id == "unrelated")
        .expect("unrelated component exists");
    unrelated.factories = changed_empty_factory;
    let candidate =
        EffectiveGraph::build(RuntimeGenerationId::INITIAL, &candidate_registry, |_, _| {
            true
        })
        .expect("candidate validates");

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "unrelated"), Some(NodeAction::Replace));
    assert!(plan.diff.nodes.iter().any(|change| {
        change.component == "unrelated" && change.kinds.contains(&NodeChangeKind::FactoryChanged)
    }));
}

#[test]
fn shared_graph_plans_match_independently_built_identical_graphs() {
    let active = graph(false);
    let shared = active.same_graph();
    let rebuilt = graph(false);
    let roots = BTreeSet::from(["default-authenticator"]);

    assert!(shared.shares_data_with(&active));
    assert!(!rebuilt.shares_data_with(&active));
    assert_eq!(
        active.plan_transition(&shared),
        active.plan_transition(&rebuilt)
    );
    assert_eq!(
        active.plan_runtime_transition(&shared, &roots, |_, _| false),
        active.plan_runtime_transition(&rebuilt, &roots, |_, _| false)
    );
}

/// The original pass-based retirement ordering, kept as the oracle for the layered one.
fn reference_retirement_order(
    graph: &EffectiveGraph,
    components: &BTreeSet<&'static str>,
) -> Vec<&'static str> {
    let mut remaining = components.clone();
    let mut order = Vec::new();

    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|component| {
                !remaining.iter().any(|consumer| {
                    consumer != *component && graph.fixed_targets(consumer).contains(*component)
                })
            })
            .copied()
            .collect::<Vec<_>>();

        if ready.is_empty() {
            order.extend(remaining.iter().copied());

            break;
        }

        for component in ready {
            remaining.remove(component);
            order.push(component);
        }
    }

    order
}

#[test]
fn layered_retirement_order_matches_the_pass_based_order_for_every_subset() {
    for graph in [graph(false), graph(true)] {
        let ids = graph.nodes().map(|node| node.id).collect::<Vec<_>>();

        for mask in 0_u32..(1 << ids.len()) {
            let subset = ids
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, id)| *id)
                .collect::<BTreeSet<_>>();

            assert_eq!(
                graph.retirement_order(&subset),
                reference_retirement_order(&graph, &subset),
                "retirement order differs for {subset:?}"
            );
        }
    }
}
