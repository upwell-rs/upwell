//! The boxed detail payloads of the DI error variants.

use std::fmt;

use upwell_core::ScopeId;

/// Stable identities describing a provider whose concrete component is absent.
#[derive(Debug)]
pub struct ProviderComponentMissing {
    /// Human-readable provided trait name.
    pub trait_name: String,
    /// Stable provided Rust trait type.
    pub trait_type: String,
    /// Human-readable concrete component name from the provider descriptor.
    pub component: String,
    /// Stable concrete Rust component type.
    pub component_type: String,
    /// Stable provider qualifier.
    pub qualifier: String,
}

impl fmt::Display for ProviderComponentMissing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "provider '{}' for trait '{}' has no effective concrete component '{}'",
            self.qualifier, self.trait_name, self.component
        )
    }
}

/// Stable identities describing a provider-order target/trait mismatch.
#[derive(Debug)]
pub struct ProviderOrderTargetTraitMismatch {
    pub component: String,
    pub component_id: String,
    pub component_type: String,
    pub target: String,
    pub target_id: String,
    pub target_type: String,
    pub trait_name: String,
    pub trait_type: String,
}

/// Stable identities describing a provider-order source/trait mismatch.
#[derive(Debug)]
pub struct ProviderOrderSourceTraitMismatch {
    pub component: String,
    pub component_id: String,
    pub component_type: String,
    pub trait_name: String,
    pub trait_type: String,
}

impl fmt::Display for ProviderOrderSourceTraitMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "provider ordering component '{}' does not provide restricted trait '{}'",
            self.component, self.trait_name
        )
    }
}

/// Stable identities describing a provider-order cycle.
#[derive(Debug)]
pub struct ProviderOrderCycle {
    pub trait_name: String,
    pub trait_type: String,
    pub components: String,
    pub component_ids: Vec<String>,
    pub component_types: Vec<String>,
}

impl fmt::Display for ProviderOrderCycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "provider ordering cycle for trait '{}': {}",
            self.trait_name, self.components
        )
    }
}

impl fmt::Display for ProviderOrderTargetTraitMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "provider ordering target '{}' does not provide trait '{}' required by '{}'",
            self.target, self.trait_name, self.component
        )
    }
}

/// Stable identities describing an invalid fresh dependency path.
#[derive(Debug)]
pub struct InvalidFreshDependency {
    pub component: String,
    pub component_id: String,
    pub dependency: String,
    pub dependency_type: String,
    pub component_scope: ScopeId,
    pub dependency_scope: ScopeId,
}

impl fmt::Display for InvalidFreshDependency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid fresh dependency for component '{}': {}",
            self.component, self.dependency
        )
    }
}

/// Stable identities describing an unsupported deferred transient target.
#[derive(Debug)]
pub struct DeferredTransientDependency {
    pub component: String,
    pub component_id: String,
    pub dependency: String,
    pub dependency_type: String,
    pub component_scope: ScopeId,
    pub dependency_scope: ScopeId,
}

impl fmt::Display for DeferredTransientDependency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "component '{}' cannot defer transient dependency '{}': deferred targets must be stored in a scope",
            self.component, self.dependency
        )
    }
}

/// Stable identities describing an inaccessible component dependency.
#[derive(Debug)]
pub struct ScopeViolation {
    pub component: String,
    pub component_id: String,
    pub dependency: String,
    pub dependency_type: String,
    pub component_scope: &'static str,
    pub component_scope_id: ScopeId,
    pub dependency_scope: &'static str,
    pub dependency_scope_id: ScopeId,
}

impl fmt::Display for ScopeViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "scope violation: component '{}' ({}) depends on '{}' ({}), which is shorter-lived",
            self.component, self.component_scope, self.dependency, self.dependency_scope
        )
    }
}

/// One registered trait provider that is not visible from a dependency's consumer scope.
#[derive(Debug)]
pub struct ScopeUnreachableProvider {
    /// Human-readable provider component name.
    pub component: String,
    /// Stable provider component identity.
    pub component_id: String,
    /// Stable concrete Rust type provided by the component.
    pub component_type: String,
    /// Human-readable provider scope name.
    pub scope: String,
    /// Stable provider scope identity.
    pub scope_id: ScopeId,
    /// Stable provider qualifier.
    pub qualifier: String,
}

/// Stable identities describing a required trait dependency whose providers are unreachable.
#[derive(Debug)]
pub struct ScopeUnreachableDependency {
    /// Human-readable consumer component name.
    pub component: String,
    /// Stable consumer component identity.
    pub component_id: String,
    /// Human-readable requested dependency name.
    pub dependency: String,
    /// Stable requested Rust type.
    pub dependency_type: String,
    /// Human-readable consumer scope name.
    pub component_scope: String,
    /// Stable consumer scope identity.
    pub component_scope_id: ScopeId,
    /// Registered providers excluded by scope reachability.
    pub providers: Vec<ScopeUnreachableProvider>,
}

impl fmt::Display for ScopeUnreachableDependency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let providers = self
            .providers
            .iter()
            .map(|provider| format!("'{}' ({})", provider.component, provider.scope))
            .collect::<Vec<_>>()
            .join(", ");

        write!(
            formatter,
            "scope-unreachable dependency: component '{}' ({}) requires '{}', but matching providers are only available in inaccessible scopes: {}",
            self.component, self.component_scope, self.dependency, providers
        )
    }
}
