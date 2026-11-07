//! Dependency validation: every eager dependency resolves to exactly the providers its cardinality allows.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::{Cardinality, ResolutionMode, ScopeId, ScopeKey};

use super::ComponentRegistry;
use super::scopes::{scope_allows_by_rank, scope_allows_with, scope_unreachable_dependency};
use super::selection::{self, SelectedDependency};
use crate::descriptors::ComponentDescriptor;
use crate::error::Error;

impl ComponentRegistry {
    /// Validates that every non-config single dependency is satisfiable by a
    /// component or trait provider.
    pub fn validate_dependencies(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_dependencies_with(components, &selection, &scope_allows_by_rank)
    }

    /// Returns the runtime targets selected for one validated dependency using
    /// caller-defined scope reachability.
    ///
    /// The predicate has the same meaning as
    /// [`validate_with_scope_reachability`](Self::validate_with_scope_reachability).
    pub fn selected_dependencies_with_scope_reachability(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &upwell_core::DependencyDescriptor,
        components: &[ComponentDescriptor],
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<Vec<SelectedDependency>> {
        selection::select(
            self,
            consumer,
            dependency,
            components,
            &|consumer, dependency| scope_allows_with(consumer, dependency, &can_reach),
        )
    }

    pub(super) fn validate_dependencies_with(
        &self,
        components: &[ComponentDescriptor],
        model: &selection::ProviderSelectionModel,
        can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
    ) -> crate::Result<()> {
        let available: HashSet<TypeId> = components.iter().map(|c| c.ty.type_id).collect();
        let is_component = |type_id: TypeId| available.contains(&type_id);

        for consumer in components {
            for dependency in consumer.dependencies() {
                check_dependency(model, &is_component, consumer, &dependency, can_access)?;
            }
        }

        Ok(())
    }

    /// Rejects duplicate `(trait, qualifier)` providers within one scope: a
    /// qualifier selects the first registered match, so two providers sharing a
    /// qualifier in the same scope would resolve by build order — descriptor-order
    /// dependent and effectively arbitrary. Distinct scopes may legitimately share
    /// a qualifier (the closer scope wins by design).
    pub fn validate_provider_qualifiers(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<()> {
        let mut scopes_of: HashMap<TypeId, Vec<ScopeId>> = HashMap::new();
        let mut seen: HashSet<(TypeId, &str, ScopeId)> = HashSet::new();

        for component in components {
            scopes_of
                .entry(component.ty.type_id)
                .or_default()
                .push(component.scope.id());
        }

        for provider in &self.providers {
            let scopes = scopes_of
                .get(&provider.concrete_ty.type_id)
                .map_or(&[][..], Vec::as_slice);

            for scope in scopes.iter().copied() {
                if !seen.insert((provider.trait_ty.type_id, provider.qualifier, scope)) {
                    return Err(Error::DuplicateProviderQualifier {
                        trait_name: (provider.trait_ty.type_name)().to_string(),
                        trait_type: (provider.trait_ty.type_name)().to_string(),
                        qualifier: provider.qualifier.to_string(),
                        scope,
                    });
                }
            }
        }

        Ok(())
    }
}

/// Requires one eager dependency edge of `consumer` to resolve to exactly the providers its
/// cardinality allows. `is_component` reports whether a type is a registered component.
pub(super) fn check_dependency(
    model: &selection::ProviderSelectionModel,
    is_component: &impl Fn(TypeId) -> bool,
    consumer: &ComponentDescriptor,
    dep: &upwell_core::DependencyDescriptor,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> crate::Result<()> {
    // Config edges are validated against bindings by the config layer.
    if dep.config {
        return Ok(());
    }

    if dep.resolution == ResolutionMode::Current {
        return check_current(model, is_component, consumer, dep);
    }

    let dep_id = dep.ty.type_id;
    let matching = model.has_matching_provider(dep_id, dep.qualifier);
    let visible = model.has_visible_provider(dep_id, dep.qualifier, consumer.scope, can_access);
    let must_exist = dep.cardinality.requires_provider() && !dep.optional && !dep.dynamic;
    let registered = is_component(dep_id);

    if must_exist && matching && !visible && !registered {
        return Err(scope_unreachable_dependency(model, consumer, dep));
    }

    if let Some(qualifier) = dep.qualifier {
        let found = dep.dynamic || visible;

        if !found {
            return Err(Error::MissingDependency {
                component: consumer.name.to_string(),
                component_id: consumer.id.to_string(),
                dependency: format!("{} (qualifier `{qualifier}`)", (dep.ty.type_name)()),
                type_name: (dep.ty.type_name)().to_string(),
            });
        }

        return Ok(());
    }

    if dep.cardinality == Cardinality::One && !dep.dynamic && matching && visible {
        let selected = model.select_runtime_one(
            dep_id,
            dep.qualifier,
            dep.resolution,
            consumer.scope,
            can_access,
        );

        match selected {
            Some(selected) => {
                if let Some(component) = model.component_from(
                    selected.provider.concrete_ty.type_id,
                    consumer.scope,
                    can_access,
                ) {
                    crate::observability::provider_selection(
                        consumer,
                        dep,
                        selected.provider,
                        component,
                        selected.reason,
                        selected.stage,
                    );
                }
            }

            None => {
                crate::observability::selection_absent(consumer, dep);

                return Err(unselectable_provider(
                    consumer,
                    dep,
                    model.visible_provider_ids(
                        dep_id,
                        dep.qualifier,
                        dep.resolution,
                        consumer.scope,
                        can_access,
                    ),
                ));
            }
        }
    }

    if must_exist && !registered && (!matching || !visible) {
        return Err(Error::MissingDependency {
            component: consumer.name.to_string(),
            component_id: consumer.id.to_string(),
            dependency: (dep.ty.type_name)().to_string(),
            type_name: (dep.ty.type_name)().to_string(),
        });
    }

    Ok(())
}

/// Requires the target of an invocation-context edge (`Current<T>`, `Scoped<S, T>`) to be
/// provided in some scope, unless the edge is optional or runtime-provided. Where it lives is
/// not checked: the edge resolves from whichever chain is active when it is read.
fn check_current(
    model: &selection::ProviderSelectionModel,
    is_component: &impl Fn(TypeId) -> bool,
    consumer: &ComponentDescriptor,
    dep: &upwell_core::DependencyDescriptor,
) -> crate::Result<()> {
    let dep_id = dep.ty.type_id;
    let must_exist = dep.cardinality.requires_provider() && !dep.optional && !dep.dynamic;
    let provided = is_component(dep_id) || model.has_matching_provider(dep_id, dep.qualifier);

    if !must_exist || provided {
        return Ok(());
    }

    Err(Error::MissingDependency {
        component: consumer.name.to_string(),
        component_id: consumer.id.to_string(),
        dependency: (dep.ty.type_name)().to_string(),
        type_name: (dep.ty.type_name)().to_string(),
    })
}

/// The error for a single-provider dependency no provider can be selected for: a tie
/// among `candidates`, or, when deferred resolution filtered every visible provider out,
/// a deferred dependency that only transient providers could satisfy.
pub(super) fn unselectable_provider(
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
    candidates: Vec<String>,
) -> Error {
    let type_name = (dependency.ty.type_name)().to_string();

    if candidates.is_empty() {
        return Error::DeferredTransientDependency(Box::new(
            crate::error::DeferredTransientDependency {
                component: consumer.name.to_string(),
                component_id: consumer.id.to_string(),
                dependency: type_name.clone(),
                dependency_type: type_name,
                component_scope: consumer.scope.id(),
                dependency_scope: <upwell_core::Transient as upwell_core::Scope>::ID,
            },
        ));
    }

    Error::AmbiguousProvider {
        component_id: Some(consumer.id.to_string()),
        type_name,
        candidates,
    }
}
