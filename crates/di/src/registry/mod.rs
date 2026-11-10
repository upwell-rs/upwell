use std::any::TypeId;
use std::collections::{HashMap, HashSet};

mod dependencies;
pub(crate) mod order;
pub(crate) mod placements;
mod resolution;
pub(crate) mod scopes;
pub(crate) mod selection;

use scopes::{never_encloses, scope_allows_by_rank, scope_allows_with};

pub use selection::{
    DependencyConsumer, DependencyFault, DependencySelectionReason, DependencySelectionStage,
    DependencyTarget, ProviderSelectionModel, SelectedDependency,
};

use upwell_core::{Scope, ScopeId, ScopeKey, Singleton};

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

    /// Collapses the registered descriptors to one per type and scope. A component that
    /// builds in several scopes (explicit factories with distinct scopes, or descriptors of
    /// one type declared in distinct scopes) keeps one placement per scope. Within one scope,
    /// a manually-provided instance (an empty-factory descriptor) **overrides** an
    /// auto-constructed one. When factoryless descriptors share the same identity, a raw
    /// non-snapshot-capable descriptor overrides a typed catalog descriptor so active
    /// seed provenance cannot be upgraded by later candidate metadata. The per-scope
    /// factory ambiguity check runs here via [`ComponentDescriptor::effective_factory`].
    pub fn resolved_components(&self) -> crate::Result<Vec<ComponentDescriptor>> {
        let mut chosen = Vec::new();
        let mut positions: HashMap<(TypeId, ScopeId), usize> = HashMap::new();

        for registered in &self.components {
            registered.validate_scope_lists()?;

            for component in registered.expand_placements() {
                component.validate_factories()?;
                Self::place(&mut chosen, &mut positions, component)?;
            }
        }

        placements::validate_placement_set(&chosen)?;

        Ok(chosen)
    }

    /// Places `component` by type and scope. A manual instance replaces an auto-constructed
    /// descriptor at its original position, so lifecycle hook ordering never depends on
    /// `HashMap` iteration. A raw pre-built registration replaces a snapshot-capable factoryless
    /// catalog descriptor with the same id, because it has stronger provenance; this keeps the
    /// active-seed policy for later transitions.
    fn place(
        chosen: &mut Vec<ComponentDescriptor>,
        positions: &mut HashMap<(TypeId, ScopeId), usize>,
        component: ComponentDescriptor,
    ) -> crate::Result<()> {
        let key = (component.ty.type_id, component.scope.id());
        let new_manual = component.effective_factory()?.is_none();

        let Some(position) = positions.get(&key).copied() else {
            positions.insert(key, chosen.len());
            chosen.push(component);

            return Ok(());
        };

        let existing = &chosen[position];
        let existing_manual = existing.effective_factory()?.is_none();

        let replaces_factory_built = new_manual && !existing_manual;
        let replaces_catalog_seed = new_manual
            && existing_manual
            && existing.id == component.id
            && existing.is_retainable()
            && !component.is_retainable();

        if replaces_factory_built || replaces_catalog_seed {
            chosen[position] = component;
        } else if new_manual == existing_manual && existing.id != component.id {
            return Err(Error::DuplicateComponentType(
                (component.ty.type_name)().to_string(),
            ));
        }

        Ok(())
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

        self.validate_with_scope_access(
            &components,
            &selection,
            scope_allows_by_rank,
            never_encloses,
        )
    }

    /// Validates the component graph against caller-defined scope reachability.
    ///
    /// `can_reach` receives the consumer and dependency stable scope IDs and should return
    /// `true` only when the dependency's boundary is the consumer's boundary or an ancestor on
    /// every chain the consumer opens in. `may_enclose(ancestor, descendant)` returns whether
    /// `ancestor` is open around `descendant` on at least one chain: an optional edge may name a
    /// target there, which is absent on the other chains. Pass `|_, _| false` for a topology
    /// whose every ancestor is on every chain. Transient dependencies remain universally
    /// constructible and invoke neither predicate.
    pub fn validate_with_scope_reachability(
        &self,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
        may_enclose: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<()> {
        let components = self.resolved_components()?;
        let selection = self.provider_selection_model(&components)?;

        self.validate_with_scope_reachability_using(&components, &selection, can_reach, may_enclose)
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
        may_enclose: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<()> {
        self.validate_with_scope_access(
            components,
            selection,
            |consumer, dependency| scope_allows_with(consumer, dependency, &can_reach),
            may_enclose,
        )
    }

    pub(crate) fn validate_with_scope_access(
        &self,
        components: &[ComponentDescriptor],
        selection: &selection::ProviderSelectionModel,
        can_access: impl Fn(ScopeKey, ScopeKey) -> bool,
        may_enclose: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<()> {
        self.validate_component_ids(components)?;
        Self::validate_construction(components)?;
        selection::validate_provider_components(components, &self.providers)?;
        self.validate_dependencies_with(components, selection, &can_access)?;
        self.validate_provider_qualifiers(components)?;
        self.validate_deferred_dependencies_with(components, selection, &can_access)?;
        self.validate_fresh_dependencies_with(components, selection, &can_access)?;
        self.validate_scopes_with(components, selection, &can_access, &may_enclose)?;

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

    /// Rejects an eager component of the root or the transient scope: neither opens scope
    /// instances, so there is no opening to build it on. Rejects a context value of the root
    /// or the transient scope, or one with a factory, for the same reason: it is seeded into
    /// scope instances and never built.
    fn validate_construction(components: &[ComponentDescriptor]) -> crate::Result<()> {
        for component in components.iter().filter(|c| c.construction.is_context()) {
            let outside = component.scope.is_transient() || component.scope.id() == Singleton::ID;

            if outside || component.effective_factory()?.is_some() {
                return Err(Error::InvalidContext {
                    component_id: component.id.to_string(),
                    scope: component.scope.name(),
                    scope_id: component.scope.id(),
                });
            }
        }

        for component in components.iter().filter(|c| c.construction.is_eager()) {
            if component.scope.is_transient() || component.scope.id() == Singleton::ID {
                return Err(Error::EagerOutsideScope {
                    component_id: component.id.to_string(),
                    scope: component.scope.name(),
                });
            }

            if component.effective_factory()?.is_none() {
                return Err(Error::EagerWithoutFactory {
                    component_id: component.id.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Validates and topologically orders all providers independently per trait.
    pub fn provider_order(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<HashMap<TypeId, HashMap<TypeId, usize>>> {
        order::build(components, &self.providers)
    }
}

#[cfg(test)]
mod tests;
