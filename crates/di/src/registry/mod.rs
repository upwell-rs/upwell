use std::any::TypeId;
use std::collections::{HashMap, HashSet};

mod dependencies;
pub(crate) mod order;
mod resolution;
mod scopes;
pub(crate) mod selection;

use scopes::{scope_allows_by_rank, scope_allows_with};

pub use selection::{
    DependencySelectionReason, DependencySelectionStage, DependencyTarget, ProviderSelectionModel,
    SelectedDependency,
};

use upwell_core::{ScopeId, ScopeKey};

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
                        && existing.is_retainable()
                        && !component.is_retainable()
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
        can_access: impl Fn(ScopeKey, ScopeKey) -> bool,
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

    fn validate_component_ids(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let mut seen = HashSet::new();
        for c in components {
            if !seen.insert(c.id) {
                return Err(Error::DuplicateComponentId(c.id.to_string()));
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
