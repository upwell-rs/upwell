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
    /// for the same type. The per-type factory ambiguity check runs here via
    /// [`ComponentDescriptor::effective_factory`].
    pub fn resolved_components(&self) -> crate::Result<Vec<ComponentDescriptor>> {
        let mut chosen = Vec::new();
        let mut positions: HashMap<TypeId, usize> = HashMap::new();

        for component in &self.components {
            component.validate_factory_ids()?;

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
mod tests {
    use std::{cell::Cell, future::Future, pin::Pin};

    use super::*;
    use crate::descriptors::{
        BoxedComponent, ComponentConstructionContext, ComponentDescriptor,
        ComponentFactoryDescriptor,
    };
    use upwell_core::{Cardinality, DependencyDescriptor, StaticScope, Transient, TypeDescriptor};

    /// Local stand-in intermediate scopes (the captive rule only cares about rank
    /// ordering): `Connection` outranks `Request`, both between singleton and transient.
    /// They are defined here rather than imported so the DI engine stays unaware of any
    /// protocol's concrete scopes.
    struct Connection;
    struct Request;

    impl Scope for Connection {
        fn id(&self) -> ScopeId {
            ScopeId::new("test/connection").expect("valid test scope ID")
        }

        fn rank(&self) -> u8 {
            2
        }

        fn name(&self) -> &'static str {
            "Connection"
        }
    }

    impl Scope for Request {
        fn id(&self) -> ScopeId {
            ScopeId::new("test/request").expect("valid test scope ID")
        }

        fn rank(&self) -> u8 {
            1
        }

        fn name(&self) -> &'static str {
            "Request"
        }
    }

    /// Two distinct scopes that deliberately share a rank: `rank` is a lifetime
    /// order, not an identity, so equal-rank siblings still resolve through
    /// separate containers. Used to prove deferred selection groups by scope
    /// identity rather than merging equal ranks.
    struct SiblingA;
    struct SiblingB;

    impl StaticScope for SiblingA {
        const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/sibling-a");
        const RANK: u8 = 3;
        const NAME: &'static str = "SiblingA";
    }

    impl StaticScope for SiblingB {
        const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/sibling-b");
        const RANK: u8 = 3;
        const NAME: &'static str = "SiblingB";
    }

    fn fake_factory<'a>(
        _: &'a mut ComponentConstructionContext,
    ) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>> {
        Box::pin(async { todo!() })
    }

    fn no_dependencies() -> Vec<DependencyDescriptor> {
        Vec::new()
    }

    fn descriptor_with_factories(
        factories: fn() -> &'static [ComponentFactoryDescriptor],
    ) -> ComponentDescriptor {
        ComponentDescriptor {
            id: "factory-fixture",
            name: "FactoryFixture",
            ty: TypeDescriptor::of::<u128>("FactoryFixture"),
            scope: &Singleton,
            condition: None,
            factories,
            hooks: upwell_hooks::no_hooks,
            generation_snapshot: None,
        }
    }

    #[test]
    fn factory_ids_are_validated_during_registry_resolution() {
        static EMPTY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
            id: "",
            construct: fake_factory,
            dependencies: no_dependencies,
            default: false,
        }];
        static DUPLICATE: [ComponentFactoryDescriptor; 2] = [
            ComponentFactoryDescriptor {
                id: "duplicate",
                construct: fake_factory,
                dependencies: no_dependencies,
                default: false,
            },
            ComponentFactoryDescriptor {
                id: "duplicate",
                construct: fake_factory,
                dependencies: no_dependencies,
                default: true,
            },
        ];
        fn empty() -> &'static [ComponentFactoryDescriptor] {
            &EMPTY
        }
        fn duplicate() -> &'static [ComponentFactoryDescriptor] {
            &DUPLICATE
        }

        let empty_error = ComponentRegistry {
            components: vec![descriptor_with_factories(empty)],
            providers: Vec::new(),
        }
        .resolved_components()
        .expect_err("empty factory ID is invalid");
        let duplicate_error = ComponentRegistry {
            components: vec![descriptor_with_factories(duplicate)],
            providers: Vec::new(),
        }
        .resolved_components()
        .expect_err("duplicate factory ID is invalid");

        assert!(matches!(empty_error, Error::EmptyFactoryId(_)));
        assert!(matches!(duplicate_error, Error::DuplicateFactoryId { .. }));
    }

    #[test]
    fn explicit_factory_still_wins_over_multiple_defaults() {
        static FACTORIES: [ComponentFactoryDescriptor; 3] = [
            ComponentFactoryDescriptor {
                id: "default-a",
                construct: fake_factory,
                dependencies: no_dependencies,
                default: true,
            },
            ComponentFactoryDescriptor {
                id: "explicit",
                construct: fake_factory,
                dependencies: no_dependencies,
                default: false,
            },
            ComponentFactoryDescriptor {
                id: "default-b",
                construct: fake_factory,
                dependencies: no_dependencies,
                default: true,
            },
        ];
        fn factories() -> &'static [ComponentFactoryDescriptor] {
            &FACTORIES
        }

        let descriptor = descriptor_with_factories(factories);
        let resolved = ComponentRegistry {
            components: vec![descriptor],
            providers: Vec::new(),
        }
        .resolved_components()
        .expect("multiple defaults remain valid when one explicit factory exists");

        assert_eq!(
            resolved[0]
                .effective_factory()
                .expect("factory selection succeeds")
                .expect("explicit factory exists")
                .id,
            "explicit"
        );
    }

    macro_rules! scoped {
        ($name:expr, $scope:expr, $dep:expr, $ty:expr $(,)?) => {{
            fn deps() -> ::std::vec::Vec<DependencyDescriptor> {
                $dep.to_vec()
            }

            static FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
                id: "static",
                construct: fake_factory,
                dependencies: deps,
                default: false,
            }];

            fn factories() -> &'static [ComponentFactoryDescriptor] {
                &FACTORIES
            }

            ComponentDescriptor {
                id: $name,
                name: $name,
                ty: $ty,
                scope: $scope,
                condition: None,
                factories,
                hooks: ::upwell_hooks::no_hooks,
                generation_snapshot: None,
            }
        }};
    }

    fn pg_pool_deps() -> Vec<DependencyDescriptor> {
        Vec::new()
    }

    static PG_POOL_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "static",
        construct: fake_factory,
        dependencies: pg_pool_deps,
        default: false,
    }];

    fn pg_pool_factories() -> &'static [ComponentFactoryDescriptor] {
        &PG_POOL_FACTORIES
    }

    static PG_POOL: ComponentDescriptor = ComponentDescriptor {
        id: "pg_pool",
        name: "PgPool",
        ty: TypeDescriptor::of::<u16>("PgPool"),
        scope: &Singleton,
        condition: None,
        factories: pg_pool_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };

    fn backup_repo_deps() -> Vec<DependencyDescriptor> {
        vec![DependencyDescriptor {
            name: "PgPool",
            ty: TypeDescriptor::of::<u16>("PgPool"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Eager,
            observation: upwell_core::DependencyObservation::Snapshot,
        }]
    }

    static BACKUP_REPO_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "static",
        construct: fake_factory,
        dependencies: backup_repo_deps,
        default: false,
    }];

    fn backup_repo_factories() -> &'static [ComponentFactoryDescriptor] {
        &BACKUP_REPO_FACTORIES
    }

    static BACKUP_REPO: ComponentDescriptor = ComponentDescriptor {
        id: "backup_repo",
        name: "BackupRepository",
        ty: TypeDescriptor::of::<u8>("BackupRepository"),
        scope: &Singleton,
        condition: None,
        factories: backup_repo_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };

    #[test]
    fn validate_passes_with_fulfilled_dependencies() {
        let registry = ComponentRegistry {
            components: vec![BACKUP_REPO, PG_POOL],
            ..Default::default()
        };

        assert!(registry.validate().is_ok());
    }

    #[test]
    fn validate_detects_duplicate_component_ids() {
        let registry = ComponentRegistry {
            components: vec![BACKUP_REPO, BACKUP_REPO],
            ..Default::default()
        };

        assert!(registry.validate().is_err());
    }

    #[test]
    fn validate_detects_missing_dependency() {
        let registry = ComponentRegistry {
            components: vec![BACKUP_REPO],
            ..Default::default()
        };

        assert!(registry.validate().is_err());
    }

    #[test]
    fn validate_accepts_manual_component_descriptor() {
        let without = ComponentRegistry {
            components: vec![BACKUP_REPO],
            ..Default::default()
        };

        assert!(without.validate().is_err());

        let manual = ComponentDescriptor::manual(
            "pg_pool_manual",
            "PgPool",
            TypeDescriptor::of::<u16>("PgPool"),
            &Singleton,
        );
        let with = ComponentRegistry {
            components: vec![BACKUP_REPO, manual],
            ..Default::default()
        };

        assert!(with.validate().is_ok());
    }

    #[test]
    fn resolved_components_preserve_registration_order_through_manual_override() {
        let manual = ComponentDescriptor::manual(
            "pg_pool_manual",
            "PgPool",
            TypeDescriptor::of::<u16>("PgPool"),
            &Singleton,
        );
        let registry = ComponentRegistry {
            components: vec![PG_POOL, BACKUP_REPO, manual],
            ..Default::default()
        };

        let resolved = registry.resolved_components().expect("resolve components");
        let ids: Vec<_> = resolved.iter().map(|component| component.id).collect();

        assert_eq!(ids, ["pg_pool_manual", "backup_repo"]);
    }

    // --- Scope validation --------------------------------------------------
    //
    // Stand-in types per scope: i16 = singleton, i32 = connection, i64 = request.

    static SINGLETON_DEP_ON_REQUEST: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "ReqComp",
        ty: TypeDescriptor::of::<i64>("ReqComp"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    static REQUEST_DEP_ON_CONNECTION: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "ConnComp",
        ty: TypeDescriptor::of::<i32>("ConnComp"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    static SINGLETON_DEFERRED_TRANSIENT: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "TransientDeferred",
        ty: TypeDescriptor::of::<u32>("TransientDeferred"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Deferred,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    static SINGLETON_FRESH_REQUEST: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "RequestFresh",
        ty: TypeDescriptor::of::<u128>("RequestFresh"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Fresh,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    static SINGLETON_FRESH_SHARED_COLLECTION: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
        cardinality: Cardinality::Collection,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Fresh,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    static SINGLETON_FRESH_SHARED_KEYED: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
        cardinality: Cardinality::Keyed,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Fresh,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    static REQUEST_FRESH_CONNECTION_TARGET: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "ConnectionFreshTarget",
        ty: TypeDescriptor::of::<i128>("ConnectionFreshTarget"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Fresh,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    static CONNECTION_TARGET_DEP_ON_SHARED: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    #[test]
    fn validate_rejects_fresh_target_with_shorter_lived_scope() {
        let request = scoped!(
            "RequestFresh",
            &Request,
            &[],
            TypeDescriptor::of::<u128>("RequestFresh"),
        );
        let singleton = scoped!(
            "FreshConsumer",
            &Singleton,
            &SINGLETON_FRESH_REQUEST,
            TypeDescriptor::of::<i8>("FreshConsumer"),
        );
        let registry = ComponentRegistry {
            components: vec![singleton, request],
            ..Default::default()
        };

        assert!(matches!(
            registry.validate(),
            Err(Error::InvalidFreshDependency(_))
        ));
    }

    #[test]
    fn validate_selects_nested_fresh_dependencies_from_target_scope() {
        let provider = scoped!(
            "ConnectionProvider",
            &Connection,
            &[],
            TypeDescriptor::of::<usize>("ConnectionProvider"),
        );
        let target = scoped!(
            "ConnectionFreshTarget",
            &Connection,
            &CONNECTION_TARGET_DEP_ON_SHARED,
            TypeDescriptor::of::<i128>("ConnectionFreshTarget"),
        );
        let consumer = scoped!(
            "RequestFreshConsumer",
            &Request,
            &REQUEST_FRESH_CONNECTION_TARGET,
            TypeDescriptor::of::<isize>("RequestFreshConsumer"),
        );
        let registry = ComponentRegistry {
            components: vec![consumer, target, provider],
            providers: vec![trait_provider(provider.ty, "connection", false)],
        };
        let selection = registry
            .provider_selection_model(&registry.components)
            .expect("provider selection model validates");
        let selected_from_target_scope = Cell::new(false);

        registry
            .validate_fresh_dependencies_with(
                &registry.components,
                &selection,
                &|consumer_scope, dependency_scope| {
                    if consumer_scope.id() == Connection.id()
                        && dependency_scope.id() == Connection.id()
                    {
                        selected_from_target_scope.set(true);
                    }

                    scope_allows_by_rank(consumer_scope, dependency_scope)
                },
            )
            .expect("nested fresh dependency validates");

        assert!(
            selected_from_target_scope.get(),
            "nested dependencies must be selected from the fresh target's scope"
        );
    }

    #[test]
    fn validate_rejects_fresh_collection_with_accessible_factoryless_provider() {
        let consumer = scoped!(
            "FreshCollectionConsumer",
            &Singleton,
            &SINGLETON_FRESH_SHARED_COLLECTION,
            TypeDescriptor::of::<u64>("FreshCollectionConsumer"),
        );
        let manual = ComponentDescriptor::manual(
            "manual-provider",
            "ManualProvider",
            TypeDescriptor::of::<u8>("ManualProvider"),
            &Singleton,
        );
        let registry = ComponentRegistry {
            components: vec![consumer, manual],
            providers: vec![trait_provider(manual.ty, "manual", false)],
        };

        assert!(matches!(
            registry.validate(),
            Err(Error::UnsupportedFreshFactory {
                component_id: Some(component_id),
                ..
            }) if component_id == "manual-provider"
        ));
    }

    #[test]
    fn validate_rejects_fresh_keyed_with_accessible_factoryless_provider() {
        let consumer = scoped!(
            "FreshKeyedConsumer",
            &Singleton,
            &SINGLETON_FRESH_SHARED_KEYED,
            TypeDescriptor::of::<u64>("FreshKeyedConsumer"),
        );
        let manual = ComponentDescriptor::manual(
            "manual-provider",
            "ManualProvider",
            TypeDescriptor::of::<u8>("ManualProvider"),
            &Singleton,
        );
        let registry = ComponentRegistry {
            components: vec![consumer, manual],
            providers: vec![trait_provider(manual.ty, "manual", false)],
        };

        assert!(matches!(
            registry.validate(),
            Err(Error::UnsupportedFreshFactory {
                component_id: Some(component_id),
                ..
            }) if component_id == "manual-provider"
        ));
    }

    #[test]
    fn validate_allows_lazy_dependency_on_transient_target() {
        static SINGLETON_LAZY_TRANSIENT: [DependencyDescriptor; 1] = [DependencyDescriptor {
            name: "TransientLazy",
            ty: TypeDescriptor::of::<u32>("TransientLazy"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Lazy,
            observation: upwell_core::DependencyObservation::Snapshot,
        }];
        let transient = scoped!(
            "TransientLazy",
            &Transient,
            &[],
            TypeDescriptor::of::<u32>("TransientLazy"),
        );
        let singleton = scoped!(
            "LazyConsumer",
            &Singleton,
            &SINGLETON_LAZY_TRANSIENT,
            TypeDescriptor::of::<u8>("LazyConsumer"),
        );
        let registry = ComponentRegistry {
            components: vec![singleton, transient],
            ..Default::default()
        };

        assert!(registry.validate().is_ok());
    }

    fn unreachable_erase(_: &BoxedComponent) -> BoxedComponent {
        unreachable!("validation never erases providers")
    }

    fn trait_provider(
        concrete: TypeDescriptor,
        qualifier: &'static str,
        primary: bool,
    ) -> ProviderDescriptor {
        ProviderDescriptor {
            trait_ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
            concrete_ty: concrete,
            qualifier,
            primary,
            priority: 0,
            ordering: &[],
            erase: unreachable_erase,
        }
    }

    #[test]
    fn validate_rejects_duplicate_provider_qualifier_in_one_scope() {
        let first = scoped!(
            "FirstQ",
            &Singleton,
            &[],
            TypeDescriptor::of::<u8>("FirstQ")
        );
        let second = scoped!(
            "SecondQ",
            &Singleton,
            &[],
            TypeDescriptor::of::<u16>("SecondQ")
        );
        let registry = ComponentRegistry {
            components: vec![first, second],
            providers: vec![
                trait_provider(TypeDescriptor::of::<u8>("FirstQ"), "same", false),
                trait_provider(TypeDescriptor::of::<u16>("SecondQ"), "same", false),
            ],
        };

        assert!(matches!(
            registry.validate(),
            Err(Error::DuplicateProviderQualifier { .. })
        ));
    }

    #[test]
    fn validate_allows_duplicate_provider_qualifier_across_scopes() {
        let first = scoped!(
            "FirstQ",
            &Singleton,
            &[],
            TypeDescriptor::of::<u8>("FirstQ")
        );
        let second = scoped!(
            "SecondQ",
            &Request,
            &[],
            TypeDescriptor::of::<u16>("SecondQ")
        );
        let registry = ComponentRegistry {
            components: vec![first, second],
            providers: vec![
                trait_provider(TypeDescriptor::of::<u8>("FirstQ"), "same", false),
                trait_provider(TypeDescriptor::of::<u16>("SecondQ"), "same", false),
            ],
        };

        assert!(registry.validate().is_ok());
    }

    #[test]
    fn validate_allows_deferred_trait_with_scoped_provider_beside_transient_primary() {
        static SINGLETON_DEFERRED_SHARED: [DependencyDescriptor; 1] = [DependencyDescriptor {
            name: "SharedTrait",
            ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Deferred,
            observation: upwell_core::DependencyObservation::Snapshot,
        }];
        let consumer = scoped!(
            "DeferredTraitConsumer",
            &Singleton,
            &SINGLETON_DEFERRED_SHARED,
            TypeDescriptor::of::<u64>("DeferredTraitConsumer"),
        );
        let scoped_provider = scoped!(
            "ScopedProvider",
            &Singleton,
            &[],
            TypeDescriptor::of::<u8>("ScopedProvider"),
        );
        let transient_provider = scoped!(
            "TransientProvider",
            &Transient,
            &[],
            TypeDescriptor::of::<u16>("TransientProvider"),
        );
        let registry = ComponentRegistry {
            components: vec![consumer, scoped_provider, transient_provider],
            providers: vec![
                trait_provider(
                    TypeDescriptor::of::<u16>("TransientProvider"),
                    "transient",
                    true,
                ),
                trait_provider(TypeDescriptor::of::<u8>("ScopedProvider"), "scoped", false),
            ],
        };

        // Hydration resolves through scope stores, which never hold transients,
        // so the scoped provider — not the transient global primary — is the
        // validated target.
        assert!(registry.validate().is_ok());
    }

    static SINGLETON_DEFERRED_SHARED: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Deferred,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];

    #[test]
    fn validate_rejects_ambiguous_deferred_candidates() {
        let consumer = scoped!(
            "AmbiguousDeferredConsumer",
            &Singleton,
            &SINGLETON_DEFERRED_SHARED,
            TypeDescriptor::of::<u64>("AmbiguousDeferredConsumer"),
        );
        let first = scoped!(
            "FirstScoped",
            &Singleton,
            &[],
            TypeDescriptor::of::<u8>("FirstScoped")
        );
        let second = scoped!(
            "SecondScoped",
            &Singleton,
            &[],
            TypeDescriptor::of::<u16>("SecondScoped"),
        );
        let transient = scoped!(
            "TransientPrimary",
            &Transient,
            &[],
            TypeDescriptor::of::<u32>("TransientPrimary"),
        );
        let registry = ComponentRegistry {
            components: vec![consumer, first, second, transient],
            providers: vec![
                trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
                trait_provider(TypeDescriptor::of::<u8>("FirstScoped"), "a", false),
                trait_provider(TypeDescriptor::of::<u16>("SecondScoped"), "b", false),
            ],
        };

        // The transient primary passes the global primary-count check, but after
        // transient filtering the scoped set is ambiguous with no parent fallback
        // left — hydration could never select deterministically.
        assert!(matches!(
            registry.validate(),
            Err(Error::AmbiguousProvider { .. })
        ));
    }

    #[test]
    fn validate_selects_deferred_candidates_scope_locally() {
        static REQUEST_DEFERRED_SHARED: [DependencyDescriptor; 1] = [DependencyDescriptor {
            name: "SharedTrait",
            ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Deferred,
            observation: upwell_core::DependencyObservation::Snapshot,
        }];
        let consumer = scoped!(
            "RequestDeferredConsumer",
            &Request,
            &REQUEST_DEFERRED_SHARED,
            TypeDescriptor::of::<u64>("RequestDeferredConsumer"),
        );
        let local = scoped!(
            "LocalProvider",
            &Request,
            &[],
            TypeDescriptor::of::<u8>("LocalProvider"),
        );
        let parent = scoped!(
            "ParentProvider",
            &Singleton,
            &[],
            TypeDescriptor::of::<u16>("ParentProvider"),
        );
        let transient = scoped!(
            "TransientPrimary",
            &Transient,
            &[],
            TypeDescriptor::of::<u32>("TransientPrimary"),
        );
        let registry = ComponentRegistry {
            components: vec![consumer, local, parent, transient],
            providers: vec![
                trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
                trait_provider(TypeDescriptor::of::<u8>("LocalProvider"), "local", false),
                trait_provider(TypeDescriptor::of::<u16>("ParentProvider"), "parent", false),
            ],
        };

        // The consumer's own scope has an unambiguous local provider, so the
        // cross-scope candidate set is not ambiguous for hydration.
        assert!(registry.validate().is_ok());
    }

    #[test]
    fn validate_walks_deferred_candidates_in_scope_chain_order() {
        static REQUEST_DEFERRED_CHAIN: [DependencyDescriptor; 1] = [DependencyDescriptor {
            name: "SharedTrait",
            ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Deferred,
            observation: upwell_core::DependencyObservation::Snapshot,
        }];
        let consumer = scoped!(
            "ChainConsumer",
            &Request,
            &REQUEST_DEFERRED_CHAIN,
            TypeDescriptor::of::<u64>("ChainConsumer"),
        );
        let connection = scoped!(
            "ConnectionProvider",
            &Connection,
            &[],
            TypeDescriptor::of::<u8>("ConnectionProvider"),
        );
        let singleton = scoped!(
            "SingletonProvider",
            &Singleton,
            &[],
            TypeDescriptor::of::<u16>("SingletonProvider"),
        );
        let transient = scoped!(
            "TransientPrimary",
            &Transient,
            &[],
            TypeDescriptor::of::<u32>("TransientPrimary"),
        );
        let registry = ComponentRegistry {
            components: vec![consumer, connection, singleton, transient],
            providers: vec![
                trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
                trait_provider(
                    TypeDescriptor::of::<u8>("ConnectionProvider"),
                    "conn",
                    false,
                ),
                trait_provider(
                    TypeDescriptor::of::<u16>("SingletonProvider"),
                    "root",
                    false,
                ),
            ],
        };

        // The merged non-transient set is ambiguous, but runtime hydration walks
        // scope by scope and the connection scope selects its sole provider —
        // so this is a valid registry, not an ambiguous one.
        assert!(registry.validate().is_ok());
    }

    #[test]
    fn validate_groups_deferred_candidates_by_scope_identity_not_rank() {
        static REQUEST_DEFERRED_SIBLINGS: [DependencyDescriptor; 1] = [DependencyDescriptor {
            name: "SharedTrait",
            ty: TypeDescriptor::of::<dyn Send>("SharedTrait"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Deferred,
            observation: upwell_core::DependencyObservation::Snapshot,
        }];
        let consumer = scoped!(
            "SiblingConsumer",
            &Request,
            &REQUEST_DEFERRED_SIBLINGS,
            TypeDescriptor::of::<u64>("SiblingConsumer"),
        );
        let sibling_a = scoped!(
            "SiblingAProvider",
            &SiblingA,
            &[],
            TypeDescriptor::of::<u8>("SiblingAProvider"),
        );
        let sibling_b = scoped!(
            "SiblingBProvider",
            &SiblingB,
            &[],
            TypeDescriptor::of::<u16>("SiblingBProvider"),
        );
        let transient = scoped!(
            "TransientPrimary",
            &Transient,
            &[],
            TypeDescriptor::of::<u32>("TransientPrimary"),
        );
        let registry = ComponentRegistry {
            components: vec![consumer, sibling_a, sibling_b, transient],
            providers: vec![
                trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
                trait_provider(TypeDescriptor::of::<u8>("SiblingAProvider"), "a", false),
                trait_provider(TypeDescriptor::of::<u16>("SiblingBProvider"), "b", false),
            ],
        };

        // Both providers live in distinct scopes that share rank 3. Runtime walks
        // each container individually and the nearer scope selects its sole
        // provider, so grouping by rank alone would merge the siblings into a
        // false ambiguity. Grouping by scope identity keeps them separate.
        assert!(registry.validate().is_ok());
    }

    #[test]
    fn validate_rejects_singleton_depending_on_request() {
        let request = scoped!(
            "ReqComp",
            &Request,
            &[],
            TypeDescriptor::of::<i64>("ReqComp"),
        );
        let singleton = scoped!(
            "RootComp",
            &Singleton,
            &SINGLETON_DEP_ON_REQUEST,
            TypeDescriptor::of::<i16>("RootComp"),
        );

        let registry = ComponentRegistry {
            components: vec![singleton, request],
            ..Default::default()
        };

        assert!(matches!(registry.validate(), Err(Error::ScopeViolation(_))));
    }

    #[test]
    fn validate_allows_request_depending_on_connection() {
        let connection = scoped!(
            "ConnComp",
            &Connection,
            &[],
            TypeDescriptor::of::<i32>("ConnComp"),
        );
        let request = scoped!(
            "ReqComp",
            &Request,
            &REQUEST_DEP_ON_CONNECTION,
            TypeDescriptor::of::<i64>("ReqComp"),
        );

        let registry = ComponentRegistry {
            components: vec![connection, request],
            ..Default::default()
        };

        assert!(registry.validate_scopes(&registry.components).is_ok());
    }

    #[test]
    fn validate_rejects_transient_depending_on_connection() {
        let connection = scoped!(
            "ConnComp",
            &Connection,
            &[],
            TypeDescriptor::of::<i32>("ConnComp"),
        );
        let transient = scoped!(
            "TransComp",
            &Transient,
            &REQUEST_DEP_ON_CONNECTION,
            TypeDescriptor::of::<i64>("TransComp"),
        );

        let registry = ComponentRegistry {
            components: vec![connection, transient],
            ..Default::default()
        };

        assert!(matches!(
            registry.validate_scopes(&registry.components),
            Err(Error::ScopeViolation(_))
        ));
    }

    #[test]
    fn validate_rejects_deferred_transient_target() {
        let transient = scoped!(
            "TransientDeferred",
            &Transient,
            &[],
            TypeDescriptor::of::<u32>("TransientDeferred"),
        );
        let singleton = scoped!(
            "DeferredConsumer",
            &Singleton,
            &SINGLETON_DEFERRED_TRANSIENT,
            TypeDescriptor::of::<u64>("DeferredConsumer"),
        );
        let registry = ComponentRegistry {
            components: vec![singleton, transient],
            ..Default::default()
        };

        assert!(matches!(
            registry.validate(),
            Err(Error::DeferredTransientDependency(_))
        ));
    }
}

#[cfg(test)]
#[path = "registry/reachability_tests.rs"]
mod reachability_tests;
