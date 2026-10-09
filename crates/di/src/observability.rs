use std::collections::BTreeMap;

use tracing::{debug, error, trace};
use upwell_core::ConditionPredicateKind;

use crate::condition::{AvailabilityEdge, ConditionEvaluation};
use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};
use crate::registry::{DependencySelectionReason, DependencySelectionStage};

pub(crate) const CONDITION_TARGET: &str = "upwell::di::condition";
pub(crate) const GRAPH_TARGET: &str = "upwell::di::graph";
pub(crate) const SELECTION_TARGET: &str = "upwell::di::selection";

pub(crate) fn condition_evaluation(
    evaluation: &ConditionEvaluation,
    components: &BTreeMap<&'static str, ComponentDescriptor>,
) {
    if !tracing::enabled!(target: CONDITION_TARGET, tracing::Level::TRACE) {
        return;
    }

    for decision in evaluation.decisions() {
        let root_condition_id = components[decision.component_id]
            .condition
            .map_or("", |condition| condition.id);

        trace!(
            target: CONDITION_TARGET,
            event_name = "condition-node",
            schema_version = 1_u64,
            phase = "candidate",
            component_id = decision.component_id,
            condition_id = decision.condition_id,
            root_condition_id,
            is_root = decision.condition_id == root_condition_id,
            predicate_kind = predicate_kind(decision.predicate),
            callback_kind = decision.callback_kind.unwrap_or(""),
            node_outcome = decision.outcome,
            "condition node evaluated"
        );
    }

    for (component_id, component) in components {
        trace!(
            target: CONDITION_TARGET,
            event_name = "eligibility",
            schema_version = 1_u64,
            phase = "candidate",
            subject_kind = "component",
            subject_id = *component_id,
            eligible = evaluation.component_eligible(component_id).unwrap_or(false),
            reason = if component.condition.is_some() {
                "condition-root"
            } else {
                "unconditional"
            },
            "component eligibility computed"
        );
    }

    for (mapping, eligible) in &evaluation.providers {
        trace!(
            target: CONDITION_TARGET,
            event_name = "eligibility",
            schema_version = 1_u64,
            phase = "candidate",
            subject_kind = "provider-mapping",
            subject_id = %mapping,
            eligible,
            reason = "inherits-component",
            "provider mapping eligibility computed"
        );
    }
}

pub(crate) fn condition_validation(evaluation: &ConditionEvaluation) {
    debug!(
        target: CONDITION_TARGET,
        event_name = "condition-validation",
        schema_version = 1_u64,
        phase = "validation",
        result = "accepted",
        component_count = evaluation.eligible_registry().components.len(),
        provider_count = evaluation.eligible_registry().providers.len(),
        decision_count = evaluation.decisions().len(),
        "conditional DI candidate validated"
    );
}

pub(crate) fn condition_validation_rejected(evaluation: &ConditionEvaluation) {
    debug!(
        target: CONDITION_TARGET,
        event_name = "condition-validation",
        schema_version = 1_u64,
        phase = "validation",
        result = "rejected",
        rejection_code = "registry-validation",
        component_count = evaluation.eligible_registry().components.len(),
        provider_count = evaluation.eligible_registry().providers.len(),
        decision_count = evaluation.decisions().len(),
        "conditional DI candidate rejected"
    );
}

pub(crate) fn availability_edge(edge: &AvailabilityEdge) {
    trace!(
        target: GRAPH_TARGET,
        event_name = "graph-edge",
        schema_version = 1_u64,
        graph_kind = "condition-availability",
        from_id = edge.from,
        to_id = edge.to,
        accepted = true,
        reason = "declared-availability-input",
        via_kind = predicate_kind(edge.predicate),
        via_id = edge.condition_id,
        negated = edge.negated,
        "condition availability edge retained"
    );
}

pub(crate) fn availability_cycle(cycle: &[AvailabilityEdge]) {
    let cycle_id = cycle
        .iter()
        .flat_map(|edge| [edge.from, edge.to])
        .min()
        .unwrap_or("");

    for edge in cycle {
        error!(
            target: GRAPH_TARGET,
            event_name = "cycle-edge",
            schema_version = 1_u64,
            graph_kind = "condition-availability",
            cycle_id,
            from_id = edge.from,
            to_id = edge.to,
            via_kind = predicate_kind(edge.predicate),
            via_id = edge.condition_id,
            negated = edge.negated,
            "condition availability cycle edge"
        );
    }
}

pub(crate) fn provider_selection(
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
    provider: ProviderDescriptor,
    component: ComponentDescriptor,
    reason: DependencySelectionReason,
    stage: DependencySelectionStage,
) {
    trace!(
        target: SELECTION_TARGET,
        event_name = "selection",
        schema_version = 1_u64,
        phase = "validation",
        consumer_id = consumer.id,
        dependency_name = dependency.name,
        dependency_type = (dependency.ty.type_name)(),
        target_kind = "provider-mapping",
        target_id = %provider.mapping_id(&component),
        target_scope_id = %component.scope.id(),
        reason = reason.as_str(),
        stage = stage.as_str(),
        "dependency provider selected"
    );
}

pub(crate) fn direct_selection(
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
    component: ComponentDescriptor,
) {
    trace!(
        target: SELECTION_TARGET,
        event_name = "selection",
        schema_version = 1_u64,
        phase = "validation",
        consumer_id = consumer.id,
        dependency_name = dependency.name,
        dependency_type = (dependency.ty.type_name)(),
        target_kind = "component",
        target_id = component.id,
        target_scope_id = %component.scope.id(),
        reason = DependencySelectionReason::DirectConcrete.as_str(),
        stage = DependencySelectionStage::DirectConcrete.as_str(),
        "direct component dependency selected"
    );
}

pub(crate) fn selection_absent(
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
) {
    trace!(
        target: SELECTION_TARGET,
        event_name = "selection",
        schema_version = 1_u64,
        phase = "validation",
        consumer_id = consumer.id,
        dependency_name = dependency.name,
        dependency_type = (dependency.ty.type_name)(),
        selected = false,
        reason = if dependency.optional {
            "optional-absent"
        } else {
            "no-eligible-candidate"
        },
        "dependency has no selected target"
    );
}

pub(crate) fn selection_external(
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
    reason: &'static str,
) {
    trace!(
        target: SELECTION_TARGET,
        event_name = "selection",
        schema_version = 1_u64,
        phase = "validation",
        consumer_id = consumer.id,
        dependency_name = dependency.name,
        dependency_type = (dependency.ty.type_name)(),
        selected = false,
        reason,
        "dependency resolves outside static provider selection"
    );
}

pub(crate) fn construction_edge(
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
    accepted: bool,
    reason: &'static str,
) {
    trace!(
        target: GRAPH_TARGET,
        event_name = "graph-edge-declaration",
        schema_version = 1_u64,
        graph_kind = "construction",
        from_id = consumer.id,
        dependency_name = dependency.name,
        dependency_type = (dependency.ty.type_name)(),
        accepted,
        reason,
        resolution = ?dependency.resolution,
        observation = ?dependency.observation,
        "construction edge decision"
    );
}

pub(crate) fn construction_wait(consumer: &ComponentDescriptor, dependency: &ComponentDescriptor) {
    trace!(
        target: GRAPH_TARGET,
        event_name = "graph-edge",
        schema_version = 1_u64,
        graph_kind = "construction",
        from_id = consumer.id,
        to_id = dependency.id,
        accepted = true,
        reason = "required-eager",
        "construction wait edge retained"
    );
}

pub(crate) fn build_position(component: &ComponentDescriptor, position: usize) {
    trace!(
        target: GRAPH_TARGET,
        event_name = "build-position",
        schema_version = 1_u64,
        graph_kind = "construction",
        scope_id = %component.scope.id(),
        component_id = component.id,
        position,
        "component construction position resolved"
    );
}

pub(crate) fn construction_cycle(
    cycle_id: &str,
    members: &[&str],
    edges: &[(&str, &str)],
    blocked: &[&str],
) {
    error!(
        target: GRAPH_TARGET,
        event_name = "cycle-summary",
        schema_version = 1_u64,
        graph_kind = "construction",
        cycle_id,
        cycle_count = members.len(),
        blocked_count = blocked.len(),
        "construction dependency cycle detected"
    );

    for member in members {
        error!(
            target: GRAPH_TARGET,
            event_name = "cycle-member",
            schema_version = 1_u64,
            graph_kind = "construction",
            cycle_id,
            component_id = *member,
            "construction dependency cycle member"
        );
    }

    for (from, to) in edges {
        error!(
            target: GRAPH_TARGET,
            event_name = "cycle-edge",
            schema_version = 1_u64,
            graph_kind = "construction",
            cycle_id,
            from_id = *from,
            to_id = *to,
            "construction dependency cycle edge"
        );
    }

    for component_id in blocked {
        error!(
            target: GRAPH_TARGET,
            event_name = "cycle-blocked",
            schema_version = 1_u64,
            graph_kind = "construction",
            cycle_id,
            component_id = *component_id,
            "component blocked behind construction cycle"
        );
    }
}

fn predicate_kind(kind: ConditionPredicateKind) -> &'static str {
    match kind {
        ConditionPredicateKind::ConfigBool => "config-bool",
        ConditionPredicateKind::ConfigEquals => "config-equals",
        ConditionPredicateKind::ComponentEligible => "component-eligible",
        ConditionPredicateKind::ProviderMappingEligible => "provider-mapping-eligible",
        ConditionPredicateKind::ConfigCallback => "config-callback",
        ConditionPredicateKind::AvailabilityCallback => "availability-callback",
        ConditionPredicateKind::All => "all",
        ConditionPredicateKind::Any => "any",
        ConditionPredicateKind::Not => "not",
    }
}
