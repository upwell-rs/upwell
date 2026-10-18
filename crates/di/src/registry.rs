use std::any::TypeId;
use std::collections::{HashMap, HashSet};

pub(crate) mod order;
pub(crate) mod selection;

pub use selection::{
    DependencySelectionReason, DependencySelectionStage, DependencyTarget, ProviderSelectionModel,
    SelectedDependency,
};

use upwell_core::{Cardinality, ResolutionMode, Scope, ScopeId, Singleton};

use crate::descriptors::{COMPONENTS, ComponentDescriptor, PROVIDERS, ProviderDescriptor};
use crate::error::Error;

/// Holds the component and provider *descriptors* of an application — declarations
/// only. Runtime instances live in the [`ScopeContainer`](crate::container::ScopeContainer).
///
/// This is the DI engine's own registry: it validates the component/provider graph
/// (ids, dependencies, scopes) and resolves the per-type descriptor set the container
/// builds from. Higher layers (services/RPC, config bindings) wrap it with their own
/// validation. Config edges (`#[config]`) are *skipped* here — they resolve against an
/// external resolver, validated by the config layer.
#[derive(Default, Debug, Clone)]
pub struct ComponentRegistry {
    pub components: Vec<ComponentDescriptor>,
    pub providers: Vec<ProviderDescriptor>,
}

impl ComponentRegistry {
    /// Builds the immutable provider selection model for an effective component set.
    pub fn provider_selection_model(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<selection::ProviderSelectionModel> {
        let order = self.provider_order(components)?;

        selection::ProviderSelectionModel::new(components, self.providers.clone(), order)
    }

    /// Collects every link-time-registered component and provider descriptor.
    pub fn collect() -> Self {
        let mut components: Vec<_> = COMPONENTS.iter().copied().collect();
        let mut providers: Vec<_> = PROVIDERS.iter().copied().collect();

        // linkme does not promise cross-platform iteration order. Stable discovery
        // keeps construction and lifecycle hook ordering equal on every target.
        components.sort_by_key(|component| component.id);
        providers.sort_by(|left, right| {
            (left.trait_ty.type_name)()
                .cmp((right.trait_ty.type_name)())
                .then_with(|| (left.concrete_ty.type_name)().cmp((right.concrete_ty.type_name)()))
                .then_with(|| left.qualifier.cmp(right.qualifier))
        });

        Self {
            components,
            providers,
        }
    }

    /// Collapses the registered descriptors to one per type. A manually-provided
    /// instance (an empty-factory descriptor) **overrides** an auto-constructed one
    /// for the same type. When factoryless descriptors share the same identity, a raw
    /// non-snapshot-capable descriptor overrides a typed catalog descriptor so active
    /// seed provenance cannot be upgraded by later candidate metadata. The per-type
    /// factory ambiguity check runs here via [`ComponentDescriptor::effective_factory`].
    pub fn resolved_components(&self) -> crate::Result<Vec<ComponentDescriptor>> {
        let mut chosen = Vec::new();
        let mut positions: HashMap<TypeId, usize> = HashMap::new();

        for component in &self.components {
            component.validate_factories()?;

            let type_id = component.ty.type_id;
            let new_manual = component.effective_factory()?.is_none();

            match positions.get(&type_id).copied() {
                None => {
                    positions.insert(type_id, chosen.len());
                    chosen.push(*component);
                }

                Some(position) => {
                    let existing = &chosen[position];
                    let existing_manual = existing.effective_factory()?.is_none();

                    if new_manual && !existing_manual {
                        // An override replaces the descriptor at its original position.
                        // Lifecycle hook ordering must not depend on HashMap iteration.
                        chosen[position] = *component;
                    } else if new_manual
                        && existing_manual
                        && existing.id == component.id
                        && existing.generation_snapshot.is_some()
                        && component.generation_snapshot.is_none()
                    {
                        // A raw pre-built registration has stronger provenance than a
                        // snapshot-capable factoryless catalog descriptor for the same
                        // component. Preserve that active-seed policy for later transitions.
                        chosen[position] = *component;
                    } else if new_manual == existing_manual && existing.id != component.id {
                        return Err(Error::DuplicateComponentType(
                            (component.ty.type_name)().to_string(),
                        ));
                    }
                }
            }
        }

        Ok(chosen)
    }

    /// Validates the component graph using the legacy rank-based scope model.
    ///
    /// This entry point remains suitable for direct DI users without an explicit
    /// scope topology. Equal-or-higher-ranked scopes are treated as reachable, so
    /// callers with branching scope paths should use
    /// [`validate_with_scope_reachability`](Self::validate_with_scope_reachability).
    pub fn validate(&self) -> crate::Result<()> {
        let components = self.resolved_components()?;
        let selection = self.provider_selection_model(&components)?;

        self.validate_with_scope_access(&components, &selection, scope_allows_by_rank)
    }

    /// Validates the component graph against caller-defined scope reachability.
    ///
    /// The predicate receives the consumer and dependency stable scope IDs and
    /// should return `true` only when the dependency's boundary is the consumer's
    /// boundary or an ancestor reachable from it. Transient dependencies remain
    /// universally constructible and do not invoke the predicate.
    pub fn validate_with_scope_reachability(
        &self,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<()> {
        let components = self.resolved_components()?;
        let selection = self.provider_selection_model(&components)?;

        self.validate_with_scope_reachability_using(&components, &selection, can_reach)
    }

    /// Validates an effective component set with its retained provider selection model.
    ///
    /// This entry point lets application preparation reuse the exact immutable model
    /// for validation, construction planning, runtime resolution, and tooling.
    #[doc(hidden)]
    pub fn validate_with_scope_reachability_using(
        &self,
        components: &[ComponentDescriptor],
        selection: &selection::ProviderSelectionModel,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<()> {
        self.validate_with_scope_access(components, selection, |consumer, dependency| {
            scope_allows_with(consumer, dependency, &can_reach)
        })
    }

    pub(crate) fn validate_with_scope_access(
        &self,
        components: &[ComponentDescriptor],
        selection: &selection::ProviderSelectionModel,
        can_access: impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<()> {
        self.validate_component_ids(components)?;
        selection::validate_provider_components(components, &self.providers)?;
        self.validate_dependencies_with(components, selection, &can_access)?;
        self.validate_provider_qualifiers(components)?;
        self.validate_deferred_dependencies_with(components, selection, &can_access)?;
        self.validate_fresh_dependencies_with(components, selection, &can_access)?;
        self.validate_scopes_with(components, selection, &can_access)?;

        Ok(())
    }

    /// Enforces the captive-dependency rule: a non-transient component may depend
    /// only on equal-or-longer-lived non-transient components. Checked against
    /// [`Scope::rank`], not by matching each label.
    pub fn validate_scopes(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_scopes_with(components, &selection, &scope_allows_by_rank)
    }

    fn validate_scopes_with(
        &self,
        components: &[ComponentDescriptor],
        selection: &selection::ProviderSelectionModel,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<()> {
        let scope_of: HashMap<TypeId, &'static dyn Scope> =
            components.iter().map(|c| (c.ty.type_id, c.scope)).collect();

        for c in components {
            for dep in c.dependencies() {
                // Config edges resolve against external bindings; dynamic edges are
                // runtime-provided. Neither participates in the scope rule.
                if dep.dynamic || dep.config {
                    continue;
                }

                let dep_id = dep.ty.type_id;

                let dep_scopes = match scope_of.get(&dep_id) {
                    Some(scope) => vec![(*scope, (dep.ty.type_name)())],
                    None => self.selected_dependency_scopes(selection, c, &dep, can_access)?,
                };

                for (dep_scope, dep_name) in dep_scopes {
                    if dep.resolution != ResolutionMode::Fresh && !can_access(c.scope, dep_scope) {
                        return Err(Error::ScopeViolation(Box::new(
                            crate::error::ScopeViolation {
                                component: c.name.to_string(),
                                component_id: c.id.to_string(),
                                dependency: dep_name.to_string(),
                                dependency_type: (dep.ty.type_name)().to_string(),
                                component_scope: c.scope.name(),
                                component_scope_id: c.scope.id(),
                                dependency_scope: dep_scope.name(),
                                dependency_scope_id: dep_scope.id(),
                            },
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    fn validate_component_ids(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let mut seen = HashSet::new();
        for c in components {
            if !seen.insert(c.id) {
                return Err(Error::DuplicateComponentId(c.id.to_string()));
            }
        }

        Ok(())
    }

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

    fn validate_dependencies_with(
        &self,
        components: &[ComponentDescriptor],
        model: &selection::ProviderSelectionModel,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<()> {
        let available: HashSet<TypeId> = components.iter().map(|c| c.ty.type_id).collect();

        for c in components {
            for dep in c.dependencies() {
                // Config edges are validated against bindings by the config layer.
                if dep.config {
                    continue;
                }

                let dep_id = dep.ty.type_id;
                let matching = model.has_matching_provider(dep_id, dep.qualifier);
                let visible =
                    model.has_visible_provider(dep_id, dep.qualifier, c.scope, can_access);
                let must_exist =
                    dep.cardinality.requires_provider() && !dep.optional && !dep.dynamic;

                if must_exist && matching && !visible && !available.contains(&dep_id) {
                    return Err(Self::scope_unreachable_dependency(model, c, &dep));
                }

                if let Some(qualifier) = dep.qualifier {
                    let found = dep.dynamic || visible;

                    if !found {
                        return Err(Error::MissingDependency {
                            component: c.name.to_string(),
                            component_id: c.id.to_string(),
                            dependency: format!(
                                "{} (qualifier `{qualifier}`)",
                                (dep.ty.type_name)()
                            ),
                            type_name: (dep.ty.type_name)().to_string(),
                        });
                    }

                    continue;
                }

                if dep.cardinality == Cardinality::One && !dep.dynamic && matching && visible {
                    let selected = model.select_runtime_one(
                        dep_id,
                        dep.qualifier,
                        dep.resolution,
                        c.scope,
                        can_access,
                    );

                    if let Some(selected) = selected {
                        if let Some(component) =
                            model.component(selected.provider.concrete_ty.type_id)
                        {
                            crate::observability::provider_selection(
                                c,
                                &dep,
                                selected.provider,
                                component,
                                selected.reason,
                                selected.stage,
                            );
                        }
                    } else {
                        crate::observability::selection_absent(c, &dep);

                        return Err(Error::AmbiguousProvider {
                            component_id: Some(c.id.to_string()),
                            type_name: (dep.ty.type_name)().to_string(),
                        });
                    }
                }

                if must_exist && !available.contains(&dep_id) && (!matching || !visible) {
                    return Err(Error::MissingDependency {
                        component: c.name.to_string(),
                        component_id: c.id.to_string(),
                        dependency: (dep.ty.type_name)().to_string(),
                        type_name: (dep.ty.type_name)().to_string(),
                    });
                }
            }
        }

        Ok(())
    }

    fn scope_unreachable_dependency(
        model: &selection::ProviderSelectionModel,
        consumer: &ComponentDescriptor,
        dependency: &upwell_core::DependencyDescriptor,
    ) -> Error {
        let providers = model
            .matching_providers(dependency.ty.type_id, dependency.qualifier)
            .into_iter()
            .filter_map(|provider| {
                let component = model.component(provider.concrete_ty.type_id)?;

                Some(crate::error::ScopeUnreachableProvider {
                    component: component.name.to_string(),
                    component_id: component.id.to_string(),
                    component_type: (component.ty.type_name)().to_string(),
                    scope: component.scope.name().to_string(),
                    scope_id: component.scope.id(),
                    qualifier: provider.qualifier.to_string(),
                })
            })
            .collect();
        let dependency_name = dependency.qualifier.map_or_else(
            || dependency.name.to_string(),
            |qualifier| format!("{} (qualifier `{qualifier}`)", dependency.name),
        );

        Error::ScopeUnreachableDependency(Box::new(crate::error::ScopeUnreachableDependency {
            component: consumer.name.to_string(),
            component_id: consumer.id.to_string(),
            dependency: dependency_name,
            dependency_type: (dependency.ty.type_name)().to_string(),
            component_scope: consumer.scope.name().to_string(),
            component_scope_id: consumer.scope.id(),
            providers,
        }))
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
        let scope_of: HashMap<TypeId, ScopeId> = components
            .iter()
            .map(|component| (component.ty.type_id, component.scope.id()))
            .collect();
        let mut seen: HashSet<(TypeId, &str, ScopeId)> = HashSet::new();

        for provider in &self.providers {
            let Some(scope) = scope_of.get(&provider.concrete_ty.type_id).copied() else {
                continue;
            };

            if !seen.insert((provider.trait_ty.type_id, provider.qualifier, scope)) {
                return Err(Error::DuplicateProviderQualifier {
                    trait_name: (provider.trait_ty.type_name)().to_string(),
                    trait_type: (provider.trait_ty.type_name)().to_string(),
                    qualifier: provider.qualifier.to_string(),
                    scope,
                });
            }
        }

        Ok(())
    }

    /// Rejects deferred dependencies whose selected target is transient. Deferred
    /// handles retain only a weak reference, so their targets must be stored by a
    /// concrete scope after construction.
    pub fn validate_deferred_dependencies(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_deferred_dependencies_with(components, &selection, &scope_allows_by_rank)
    }

    fn validate_deferred_dependencies_with(
        &self,
        components: &[ComponentDescriptor],
        model: &selection::ProviderSelectionModel,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<()> {
        let by_type: HashMap<TypeId, ComponentDescriptor> = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect();

        for consumer in components {
            for dependency in consumer
                .dependencies()
                .into_iter()
                .filter(|dependency| dependency.resolution == ResolutionMode::Deferred)
            {
                let target = match by_type.get(&dependency.ty.type_id).copied() {
                    Some(target) => Some(target),
                    None => {
                        let selected = model.select_runtime_one(
                            dependency.ty.type_id,
                            dependency.qualifier,
                            ResolutionMode::Deferred,
                            consumer.scope,
                            can_access,
                        );

                        match selected {
                            Some(selected) => {
                                model.component(selected.provider.concrete_ty.type_id)
                            }
                            None if !model.has_visible_provider(
                                dependency.ty.type_id,
                                dependency.qualifier,
                                consumer.scope,
                                can_access,
                            ) =>
                            {
                                None
                            }
                            None => {
                                return Err(Error::AmbiguousProvider {
                                    component_id: Some(consumer.id.to_string()),
                                    type_name: (dependency.ty.type_name)().to_string(),
                                });
                            }
                        }
                    }
                };

                if target.is_some_and(|target| target.scope.is_transient()) {
                    return Err(Error::DeferredTransientDependency(Box::new(
                        crate::error::DeferredTransientDependency {
                            component: consumer.name.to_string(),
                            component_id: consumer.id.to_string(),
                            dependency: (dependency.ty.type_name)().to_string(),
                            dependency_type: (dependency.ty.type_name)().to_string(),
                            component_scope: consumer.scope.id(),
                            dependency_scope: target
                                .expect("transient target was selected")
                                .scope
                                .id(),
                        },
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validates that forced-fresh targets have factories and can resolve their eager
    /// dependencies from the target's construction scope.
    pub fn validate_fresh_dependencies(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_fresh_dependencies_with(components, &selection, &scope_allows_by_rank)
    }

    fn validate_fresh_dependencies_with(
        &self,
        components: &[ComponentDescriptor],
        model: &selection::ProviderSelectionModel,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<()> {
        let by_type: HashMap<TypeId, ComponentDescriptor> = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect();
        let scope_of: HashMap<TypeId, &'static dyn Scope> = components
            .iter()
            .map(|component| (component.ty.type_id, component.scope))
            .collect();

        for consumer in components {
            for dependency in consumer
                .dependencies()
                .into_iter()
                .filter(|dependency| dependency.resolution == ResolutionMode::Fresh)
            {
                let targets =
                    self.fresh_targets(model, consumer, &dependency, &by_type, can_access)?;

                for target in targets {
                    // The consumer retains a fresh instance permanently, so the
                    // target's own scope must be visible from the consumer's scope —
                    // the same lifetime rule an eager edge follows. Collection shapes
                    // skip inaccessible providers at runtime, so only validate the
                    // accessible subset.
                    if !can_access(consumer.scope, target.scope) {
                        if dependency.cardinality == Cardinality::One {
                            return Err(Error::InvalidFreshDependency(Box::new(
                                crate::error::InvalidFreshDependency {
                                    component: consumer.name.to_string(),
                                    component_id: consumer.id.to_string(),
                                    dependency: target.name.to_string(),
                                    dependency_type: (target.ty.type_name)().to_string(),
                                    component_scope: consumer.scope.id(),
                                    dependency_scope: target.scope.id(),
                                },
                            )));
                        }

                        continue;
                    }

                    if target.effective_factory()?.is_none() {
                        return Err(Error::UnsupportedFreshFactory {
                            component: target.name.to_string(),
                            component_id: Some(target.id.to_string()),
                            type_name: (target.ty.type_name)().to_string(),
                        });
                    }

                    for target_dependency in target.dependencies().into_iter().filter(|edge| {
                        edge.resolution == ResolutionMode::Eager && !edge.dynamic && !edge.config
                    }) {
                        let dependency_scopes = match scope_of.get(&target_dependency.ty.type_id) {
                            Some(scope) => vec![*scope],
                            None => self
                                .selected_dependency_scopes(
                                    model,
                                    &target,
                                    &target_dependency,
                                    can_access,
                                )?
                                .into_iter()
                                .map(|(scope, _)| scope)
                                .collect(),
                        };

                        if dependency_scopes
                            .iter()
                            .any(|scope| !can_access(consumer.scope, *scope))
                        {
                            return Err(Error::InvalidFreshDependency(Box::new(
                                crate::error::InvalidFreshDependency {
                                    component: consumer.name.to_string(),
                                    component_id: consumer.id.to_string(),
                                    dependency: format!(
                                        "{} -> {}",
                                        target.name,
                                        (target_dependency.ty.type_name)()
                                    ),
                                    dependency_type: (target_dependency.ty.type_name)().to_string(),
                                    component_scope: consumer.scope.id(),
                                    dependency_scope: dependency_scopes
                                        .iter()
                                        .find(|scope| !can_access(consumer.scope, **scope))
                                        .expect("an inaccessible scope was found")
                                        .id(),
                                },
                            )));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn fresh_targets(
        &self,
        model: &selection::ProviderSelectionModel,
        consumer: &ComponentDescriptor,
        dependency: &upwell_core::DependencyDescriptor,
        by_type: &HashMap<TypeId, ComponentDescriptor>,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<Vec<ComponentDescriptor>> {
        if let Some(component) = by_type.get(&dependency.ty.type_id) {
            return Ok(vec![*component]);
        }

        let selected = match dependency.cardinality {
            Cardinality::One => model
                .select_runtime_one(
                    dependency.ty.type_id,
                    dependency.qualifier,
                    ResolutionMode::Fresh,
                    consumer.scope,
                    can_access,
                )
                .into_iter()
                .collect(),
            Cardinality::Collection | Cardinality::Keyed => model.select_runtime_collection(
                dependency.ty.type_id,
                ResolutionMode::Fresh,
                consumer.scope,
                can_access,
            ),
        };

        Ok(selected
            .into_iter()
            .filter_map(|selection| model.component(selection.provider.concrete_ty.type_id))
            .collect())
    }

    fn selected_dependency_scopes(
        &self,
        model: &selection::ProviderSelectionModel,
        consumer: &ComponentDescriptor,
        dependency: &upwell_core::DependencyDescriptor,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<Vec<(&'static dyn Scope, &'static str)>> {
        let selected = match dependency.cardinality {
            Cardinality::One => model
                .select_runtime_one(
                    dependency.ty.type_id,
                    dependency.qualifier,
                    dependency.resolution,
                    consumer.scope,
                    can_access,
                )
                .into_iter()
                .collect(),
            Cardinality::Collection => model.select_runtime_collection(
                dependency.ty.type_id,
                dependency.resolution,
                consumer.scope,
                can_access,
            ),
            Cardinality::Keyed => model.select_runtime_keyed(
                dependency.ty.type_id,
                dependency.resolution,
                consumer.scope,
                can_access,
            ),
        };

        Ok(selected
            .into_iter()
            .filter_map(|selection| {
                let component = model.component(selection.provider.concrete_ty.type_id)?;

                Some((
                    component.scope,
                    (selection.provider.concrete_ty.type_name)(),
                ))
            })
            .collect())
    }

    /// Validates and topologically orders all providers independently per trait.
    pub fn provider_order(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<HashMap<TypeId, HashMap<TypeId, usize>>> {
        order::build(components, &self.providers)
    }
}

/// Whether a `consumer`-scoped component may hold a `dependency`-scoped one under
/// the topology-neutral legacy rank model.
fn scope_allows_by_rank(consumer: &dyn Scope, dependency: &dyn Scope) -> bool {
    if dependency.is_transient() {
        return true;
    }

    if consumer.is_transient() {
        return dependency.id() == Singleton.id();
    }

    dependency.rank() >= consumer.rank()
}

fn scope_allows_with(
    consumer: &dyn Scope,
    dependency: &dyn Scope,
    can_reach: &impl Fn(ScopeId, ScopeId) -> bool,
) -> bool {
    if dependency.is_transient() {
        return true;
    }

    if consumer.is_transient() {
        return dependency.id() == Singleton.id();
    }

    can_reach(consumer.id(), dependency.id())
}

#[cfg(test)]
mod tests;
