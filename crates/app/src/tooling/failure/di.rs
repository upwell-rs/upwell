use super::FailureDetails;

pub(in crate::tooling) fn di_failure(
    error: &upwell_di::Error,
    phase: Option<String>,
) -> FailureDetails {
    match error {
        upwell_di::Error::MissingDependency {
            component_id,
            type_name,
            ..
        } => (
            "upwell/tooling-dependency-missing",
            "A component dependency has no registered provider.",
            phase,
            vec![component_resource(component_id), type_resource(type_name)],
            Vec::new(),
            Some("Register one provider for the missing dependency type."),
        ),
        upwell_di::Error::DependencyCycle(_) => (
            "upwell/tooling-dependency-cycle",
            "Component dependencies contain a construction cycle.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Break the component dependency cycle."),
        ),
        upwell_di::Error::AmbiguousProvider {
            component_id,
            type_name,
            ..
        } => (
            "upwell/tooling-provider-ambiguous",
            "A dependency has more than one eligible provider.",
            phase,
            optional_component_and_type(component_id.as_deref(), type_name),
            Vec::new(),
            Some(
                "Mark exactly one provider primary, give the providers distinct priorities, or request a provider collection.",
            ),
        ),
        upwell_di::Error::ProviderComponentMissing(error) => {
            let upwell_di::ProviderComponentMissing {
                trait_type,
                component_type,
                qualifier,
                ..
            } = error.as_ref();

            (
                "upwell/tooling-provider-component-missing",
                "A provider descriptor references a concrete component that is not in the effective component set.",
                phase,
                vec![
                    provider_resource(trait_type, component_type, qualifier),
                    component_resource(component_type),
                    type_resource(trait_type),
                    type_resource(component_type),
                ],
                Vec::new(),
                Some(
                    "Register the provider's concrete component or remove the orphan provider descriptor.",
                ),
            )
        }
        upwell_di::Error::ScopeViolation(error) => {
            let upwell_di::ScopeViolation {
                component_id,
                dependency_type,
                component_scope_id,
                dependency_scope_id,
                ..
            } = error.as_ref();

            (
                "upwell/tooling-scope-violation",
                "A component dependency crosses an inaccessible or shorter-lived scope boundary.",
                phase,
                vec![
                    component_resource(component_id),
                    type_resource(dependency_type),
                    scope_resource(*component_scope_id),
                    scope_resource(*dependency_scope_id),
                ],
                Vec::new(),
                Some("Move the dependency to a reachable scope or shorten the consumer lifetime."),
            )
        }
        upwell_di::Error::ScopeUnreachableDependency(error) => {
            let upwell_di::ScopeUnreachableDependency {
                component_id,
                dependency_type,
                component_scope_id,
                providers,
                ..
            } = error.as_ref();
            let mut resources = vec![
                component_resource(component_id),
                type_resource(dependency_type),
                scope_resource(*component_scope_id),
            ];

            for provider in providers {
                resources.extend([
                    provider_resource(
                        dependency_type,
                        &provider.component_type,
                        &provider.qualifier,
                    ),
                    component_resource(&provider.component_id),
                    type_resource(&provider.component_type),
                    scope_resource(provider.scope_id),
                ]);
            }

            (
                "upwell/tooling-scope-unreachable",
                "Registered providers exist, but none is reachable from the consumer scope.",
                phase,
                resources,
                Vec::new(),
                Some(
                    "Move a provider into the consumer's scope chain or move the consumer beneath a provider scope.",
                ),
            )
        }
        upwell_di::Error::InvalidFreshDependency(error) => {
            let upwell_di::InvalidFreshDependency {
                component_id,
                dependency_type,
                component_scope,
                dependency_scope,
                ..
            } = error.as_ref();

            (
                "upwell/tooling-fresh-dependency-invalid",
                "A fresh dependency cannot be constructed from the consumer scope.",
                phase,
                vec![
                    component_resource(component_id),
                    type_resource(dependency_type),
                    scope_resource(*component_scope),
                    scope_resource(*dependency_scope),
                ],
                Vec::new(),
                Some(
                    "Use a reachable factory-backed target or change the dependency resolution mode.",
                ),
            )
        }
        upwell_di::Error::DeferredTransientDependency(error) => {
            let upwell_di::DeferredTransientDependency {
                component_id,
                dependency_type,
                component_scope,
                dependency_scope,
                ..
            } = error.as_ref();

            (
                "upwell/tooling-deferred-transient",
                "A deferred dependency selected a transient target that cannot be hydrated.",
                phase,
                vec![
                    component_resource(component_id),
                    type_resource(dependency_type),
                    scope_resource(*component_scope),
                    scope_resource(*dependency_scope),
                ],
                Vec::new(),
                Some("Store the target in a scope or use eager, lazy, or fresh resolution."),
            )
        }
        upwell_di::Error::UnsupportedFreshFactory {
            component_id,
            type_name,
            ..
        } => (
            "upwell/tooling-fresh-factory-unsupported",
            "Fresh construction selected a component without a usable factory.",
            phase,
            optional_component_and_type(component_id.as_deref(), type_name),
            Vec::new(),
            Some("Register a component factory or use stored resolution."),
        ),
        upwell_di::Error::DuplicateProviderQualifier {
            trait_type, scope, ..
        } => (
            "upwell/tooling-provider-qualifier-duplicate",
            "Provider qualifier selection is ambiguous within one scope.",
            phase,
            vec![type_resource(trait_type), scope_resource(*scope)],
            Vec::new(),
            Some("Give same-scope providers unique qualifiers."),
        ),
        upwell_di::Error::MissingProviderOrderTarget {
            component_id,
            target_type,
            ..
        } => provider_order_failure(
            phase,
            vec![component_resource(component_id), type_resource(target_type)],
        ),
        upwell_di::Error::SelfProviderOrder {
            component_id,
            component_type,
            ..
        } => provider_order_failure(
            phase,
            vec![
                component_resource(component_id),
                type_resource(component_type),
            ],
        ),
        upwell_di::Error::ProviderOrderSourceTraitMismatch(error) => {
            let upwell_di::ProviderOrderSourceTraitMismatch {
                component_id,
                component_type,
                trait_type,
                ..
            } = error.as_ref();

            provider_order_failure(
                phase,
                vec![
                    component_resource(component_id),
                    type_resource(component_type),
                    type_resource(trait_type),
                ],
            )
        }
        upwell_di::Error::ProviderOrderTargetTraitMismatch(error) => {
            let upwell_di::ProviderOrderTargetTraitMismatch {
                component_id,
                component_type,
                target_id,
                target_type,
                trait_type,
                ..
            } = error.as_ref();

            provider_order_failure(
                phase,
                vec![
                    component_resource(component_id),
                    type_resource(component_type),
                    component_resource(target_id),
                    type_resource(target_type),
                    type_resource(trait_type),
                ],
            )
        }
        upwell_di::Error::ProviderOrderCycle(error) => {
            let upwell_di::ProviderOrderCycle {
                trait_type,
                component_ids,
                component_types,
                ..
            } = error.as_ref();
            let mut resources = vec![type_resource(trait_type)];

            resources.extend(component_ids.iter().map(|id| component_resource(id)));
            resources.extend(component_types.iter().map(|ty| type_resource(ty)));

            provider_order_failure(phase, resources)
        }
        _ => (
            "upwell/tooling-dependency-graph",
            "The dependency graph is structurally invalid.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Run the selected target normally to investigate the dependency failure."),
        ),
    }
}

fn component_resource(component: &str) -> String {
    format!("component:{component}")
}

fn type_resource(type_name: &str) -> String {
    format!("type:{type_name}")
}

fn scope_resource(scope: upwell_core::ScopeId) -> String {
    format!("scope:{scope}")
}

fn provider_resource(trait_type: &str, component_type: &str, qualifier: &str) -> String {
    format!("provider:{trait_type}:{component_type}:{qualifier}")
}

fn optional_component_and_type(component: Option<&str>, type_name: &str) -> Vec<String> {
    let mut resources = Vec::new();

    if let Some(component) = component {
        resources.push(component_resource(component));
    }

    resources.push(type_resource(type_name));

    resources
}

fn provider_order_failure(phase: Option<String>, resources: Vec<String>) -> FailureDetails {
    (
        "upwell/tooling-provider-order",
        "Provider precedence declarations are structurally invalid.",
        phase,
        resources,
        Vec::new(),
        Some("Correct the provider before/after targets, trait restrictions, or ordering cycle."),
    )
}
