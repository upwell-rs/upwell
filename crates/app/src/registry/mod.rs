use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write;

use upwell_config::{CONFIG_BINDINGS, ConditionFactSource, ConfigBinding};
use upwell_core::DependencyDescriptor;
use upwell_di::{
    COMPONENTS, Component, ComponentDescriptor, ComponentRegistry, PROVIDERS, ProviderDescriptor,
    ProviderSelectionModel,
};

use crate::error::Error;
use crate::scope::PreparedScopeTopology;

mod condition;

pub use condition::AppConditionEvaluation;

/// Holds the *agnostic* component, provider, and config-binding descriptors of an app —
/// declarations only. Runtime instances live in the
/// [`ScopeContainer`](upwell_di::ScopeContainer).
///
/// Wraps the DI engine's [`ComponentRegistry`] (component/provider graph) with the config
/// bindings, and runs the cross-cutting validation the component graph alone cannot
/// (config edges). Protocol-specific declarations (services/routes) live in the protocol
/// plugin, not here, so this stays usable by any protocol.
#[derive(Clone, Default, Debug)]
pub struct AppRegistry {
    pub components: Vec<ComponentDescriptor>,
    pub providers: Vec<ProviderDescriptor>,
    /// Config bindings (a config type bound to a property path). Populated from the
    /// auto-discovered config bindings slice and from explicit builder bindings.
    pub config_bindings: Vec<ConfigBinding>,
    /// Registered condition-fact sources (a config type's condition facts at one binding
    /// path). Populated from explicit builder registrations.
    pub condition_facts: Vec<ConditionFactSource>,
}

impl AppRegistry {
    /// Collects every link-time-registered agnostic descriptor (components, providers,
    /// config bindings) into an `AppRegistry`. Protocol variant slices (e.g. RPC services)
    /// are folded in by the protocol definition, not here.
    pub fn collect() -> Self {
        let mut components: Vec<_> = COMPONENTS.iter().copied().collect();
        let mut providers: Vec<_> = PROVIDERS.iter().copied().collect();
        let mut config_bindings: Vec<_> = CONFIG_BINDINGS.iter().map(|d| d.to_binding()).collect();

        // Distributed-slice order differs between linkers. Sort auto-discovered
        // descriptors only; explicit builder registrations retain caller order.
        components.sort_by_key(|component| component.id);
        providers.sort_by(|left, right| {
            (left.trait_ty.type_name)()
                .cmp((right.trait_ty.type_name)())
                .then_with(|| (left.concrete_ty.type_name)().cmp((right.concrete_ty.type_name)()))
                .then_with(|| left.qualifier.cmp(right.qualifier))
        });
        config_bindings.sort_by(|left, right| {
            (left.ty.type_name)()
                .cmp((right.ty.type_name)())
                .then_with(|| left.path.cmp(&right.path))
        });

        Self {
            components,
            providers,
            config_bindings,
            condition_facts: Vec::new(),
        }
    }

    /// Registers a provider mapping unless the same mapping (trait, concrete type, and
    /// qualifier) is already registered, so an explicit registration and discovery of the
    /// same component do not duplicate it.
    pub fn add_provider(&mut self, provider: ProviderDescriptor) {
        let registered = self.providers.iter().any(|existing| {
            existing.trait_ty.type_id == provider.trait_ty.type_id
                && existing.concrete_ty.type_id == provider.concrete_ty.type_id
                && existing.qualifier == provider.qualifier
        });

        if !registered {
            self.providers.push(provider);
        }
    }

    /// The DI engine's view of this registry — the component/provider graph.
    pub(crate) fn component_registry(&self) -> ComponentRegistry {
        ComponentRegistry {
            components: self.components.clone(),
            providers: self.providers.clone(),
        }
    }

    /// Collapses the registered descriptors to one per type (delegated to the DI engine).
    pub fn resolved_components(&self) -> crate::Result<Vec<ComponentDescriptor>> {
        Ok(self.component_registry().resolved_components()?)
    }

    /// Returns the effective descriptor registered for component type `T`.
    ///
    /// Duplicate registrations are resolved by the same rules used during registry validation.
    pub fn resolved_component<T: Component>(&self) -> crate::Result<Option<ComponentDescriptor>> {
        let type_id = TypeId::of::<T>();
        let component = self
            .resolved_components()?
            .into_iter()
            .find(|component| component.ty.type_id == type_id);

        Ok(component)
    }

    /// Validates structural consistency: the component graph (via the DI engine), then the
    /// config-binding rules.
    pub fn validate(&self) -> crate::Result<()> {
        self.component_registry().validate()?;

        let components = self.resolved_components()?;

        self.validate_configs(&components)?;

        Ok(())
    }

    /// Validates the component graph against a prepared protocol-owned scope topology,
    /// then applies the application configuration-binding rules.
    pub fn validate_with_scope_topology(
        &self,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<()> {
        self.component_registry()
            .validate_with_scope_reachability(|consumer, dependency| {
                topology.is_reachable(&consumer, &dependency)
            })?;

        let components = self.resolved_components()?;

        self.validate_configs(&components)?;

        Ok(())
    }

    pub(crate) fn validate_effective_with_scope_topology(
        &self,
        components: &[ComponentDescriptor],
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<()> {
        self.component_registry()
            .validate_with_scope_reachability_using(
                components,
                selection,
                |consumer, dependency| topology.is_reachable(&consumer, &dependency),
            )?;

        self.validate_configs(components)?;

        Ok(())
    }

    /// Validates config edges against the registered bindings: a `#[config("path")]` edge
    /// must have a binding of its type at that path, and a `#[config]` shorthand edge must
    /// have exactly one binding of its type.
    pub(crate) fn validate_configs(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let mut bound: HashMap<TypeId, Vec<&str>> = HashMap::new();

        for binding in &self.config_bindings {
            bound
                .entry(binding.ty.type_id)
                .or_default()
                .push(&binding.path);
        }

        for c in components {
            for dep in c.dependencies().iter().filter(|dep| dep.config) {
                let dep_id = dep.ty.type_id;
                let paths = bound.get(&dep_id);

                match dep.qualifier {
                    Some(path) => {
                        let found = paths.is_some_and(|ps| ps.contains(&path));

                        if !found {
                            return Err(Error::MissingConfig {
                                component: c.name.to_string(),
                                type_name: (dep.ty.type_name)().to_string(),
                                path: path.to_string(),
                            });
                        }
                    }

                    None => {
                        let bound_paths = paths.cloned().unwrap_or_default();

                        match bound_paths.as_slice() {
                            [_] => {}

                            [] => {
                                return Err(Error::MissingConfig {
                                    component: c.name.to_string(),
                                    type_name: (dep.ty.type_name)().to_string(),
                                    path: "<unqualified>".to_string(),
                                });
                            }

                            _ => {
                                return Err(Error::AmbiguousConfig {
                                    component: c.name.to_string(),
                                    type_name: (dep.ty.type_name)().to_string(),
                                    count: bound_paths.len(),
                                    paths: bound_paths.join(", "),
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn write_components(&self, f: &mut impl Write) -> fmt::Result {
        let components = self
            .resolved_components()
            .unwrap_or_else(|_| self.components.clone());

        writeln!(f, "Components:")?;
        for c in &components {
            writeln!(f, "  {}", c.name)?;

            let deps = c.dependencies();

            if !deps.is_empty() {
                Self::write_dependency(f, deps.iter())?
            }
        }

        Ok(())
    }

    fn write_dependency<'a>(
        f: &mut impl Write,
        deps: impl Iterator<Item = &'a DependencyDescriptor>,
    ) -> fmt::Result {
        write!(f, "    depends on:")?;
        for (i, dep) in deps.enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }

            write!(f, " {}", dep.name)?;
        }

        writeln!(f)?;

        Ok(())
    }
}

impl fmt::Display for AppRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_components(f)
    }
}

#[cfg(test)]
mod tests;
