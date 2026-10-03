use std::fmt;

use thiserror::Error;

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

/// Errors from the DI engine: graph validation and component construction.
#[derive(Debug, Error)]
pub enum Error {
    #[error("duplicate component id: {0}")]
    DuplicateComponentId(String),

    #[error("multiple explicit constructors registered for component type: {0}")]
    DuplicateComponentType(String),

    #[error(
        "ambiguous factory for component '{0}': more than one explicit factory (e.g. an #[init] and a factory = ..)"
    )]
    AmbiguousFactory(String),

    #[error("factory for component '{0}' has an empty stable identity")]
    EmptyFactoryId(String),

    #[error("component '{component}' registers duplicate factory identity '{factory}'")]
    DuplicateFactoryId { component: String, factory: String },

    #[error("missing dependency for component '{component}': {dependency}")]
    MissingDependency {
        /// Human-readable component name.
        component: String,
        /// Stable component identity.
        component_id: String,
        /// Human-readable dependency description, including qualifier when present.
        dependency: String,
        /// Stable underlying requested Rust type.
        type_name: String,
    },

    #[error(
        "ambiguous provider for '{type_name}': multiple components provide it; mark one `#[primary]`, \
         or inject `Vec`/`HashMap<String, _>` to receive all of them"
    )]
    AmbiguousProvider {
        /// Stable consumer component identity, when selection has a consumer.
        component_id: Option<String>,
        /// Stable underlying requested Rust type.
        type_name: String,
    },

    #[error("dependency cycle: no construction order for components: {0}")]
    DependencyCycle(String),

    /// A provider descriptor references a concrete type outside the effective component set.
    #[error("{0}")]
    ProviderComponentMissing(Box<ProviderComponentMissing>),

    #[error("provider ordering target '{target}' for component '{component}' is not registered")]
    MissingProviderOrderTarget {
        component: String,
        component_id: String,
        target: String,
        target_type: String,
    },

    #[error("provider ordering component '{component}' cannot target itself")]
    SelfProviderOrder {
        component: String,
        component_id: String,
        component_type: String,
    },

    #[error("{0}")]
    ProviderOrderSourceTraitMismatch(Box<ProviderOrderSourceTraitMismatch>),

    #[error("{0}")]
    ProviderOrderTargetTraitMismatch(Box<ProviderOrderTargetTraitMismatch>),

    #[error("{0}")]
    ProviderOrderCycle(Box<ProviderOrderCycle>),

    #[error("missing component: {0}")]
    MissingComponent(&'static str),

    #[error("candidate component '{component}' is not part of the candidate registry")]
    CandidateComponentMismatch { component: &'static str },

    #[error(
        "candidate factory for component '{component}' returned type '{actual}' instead of '{expected}'"
    )]
    CandidateFactoryTypeMismatch {
        component: &'static str,
        expected: &'static str,
        actual: &'static str,
    },

    #[error("candidate factory for component '{component}' panicked")]
    CandidateFactoryPanicked { component: &'static str },

    #[error("candidate provider projection for component '{component}' panicked")]
    CandidateProviderPanicked { component: &'static str },

    #[error("candidate root finalization panicked")]
    CandidateFinalizationPanicked,

    #[error("candidate metadata evaluation panicked")]
    CandidateMetadataPanicked,

    #[error("candidate external resolvers contain an active component source")]
    CandidateActiveResolver,

    #[error("candidate root cannot retain generation-bound component '{component}'")]
    CandidateRuntimeBoundComponent { component: &'static str },

    #[error("component '{component}' does not support generation snapshots")]
    SnapshotUnavailable { component: &'static str },

    #[error("generation snapshot for component '{component}' panicked")]
    SnapshotPanicked { component: &'static str },

    #[error(
        "generation snapshot for component '{component}' found active storage of an incompatible type"
    )]
    SnapshotStorageMismatch { component: &'static str },

    #[error(
        "generation snapshot for component '{component}' produced storage of an incompatible type"
    )]
    SnapshotOutputMismatch { component: &'static str },

    #[error(
        "the root resolver is unavailable: the root container was never attached or has been dropped"
    )]
    RootUnavailable,

    #[error("the captured scope is unavailable: it was not attached or has been dropped")]
    ScopeUnavailable,

    #[error("fresh construction is unsupported for factory-less component '{component}'")]
    UnsupportedFreshFactory {
        /// Human-readable component name.
        component: String,
        /// Stable component identity, when a descriptor is available.
        component_id: Option<String>,
        /// Stable underlying concrete Rust type.
        type_name: String,
    },

    #[error("{0}")]
    InvalidFreshDependency(Box<InvalidFreshDependency>),

    #[error("{0}")]
    DeferredTransientDependency(Box<DeferredTransientDependency>),

    #[error(
        "duplicate provider qualifier '{qualifier}' for trait '{trait_name}' in scope '{scope}': qualifier selection is first-registered, so same-scope duplicates resolve nondeterministically"
    )]
    DuplicateProviderQualifier {
        trait_name: String,
        trait_type: String,
        qualifier: String,
        scope: ScopeId,
    },

    #[error("{0}")]
    ScopeViolation(Box<ScopeViolation>),

    #[error("{0}")]
    ScopeUnreachableDependency(Box<ScopeUnreachableDependency>),

    /// An application-defined error surfaced through the DI engine — typically from a
    /// component's `#[init]` constructor or a custom factory. The `#[from]` lets app
    /// authors use `?` with any `Error + Send + Sync` source.
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T, E = Error> = core::result::Result<T, E>;
