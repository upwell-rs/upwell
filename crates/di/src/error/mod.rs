use thiserror::Error;

use upwell_core::ScopeId;

use crate::ComponentFactoryRole;

mod details;

pub use details::{
    DeferredTransientDependency, InvalidFreshDependency, ProviderComponentMissing,
    ProviderOrderCycle, ProviderOrderSourceTraitMismatch, ProviderOrderTargetTraitMismatch,
    ScopeUnreachableDependency, ScopeUnreachableProvider, ScopeViolation,
};

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

    #[error("component '{component}' declares more than one {role} recipe")]
    AmbiguousTransitionFactory {
        component: String,
        role: ComponentFactoryRole,
    },

    #[error("component '{component}' declares a {role} recipe but no ordinary factory")]
    TransitionRecipeWithoutFactory {
        component: String,
        role: ComponentFactoryRole,
    },

    #[error(
        "component '{component}' declares a {role} recipe but has scope '{scope}'; runtime transitions are singleton-only"
    )]
    TransitionRecipeOutOfScope {
        component: String,
        role: ComponentFactoryRole,
        scope: &'static str,
    },

    #[error(
        "{role} recipe for component '{component}' resolves '{dependency}' in a way its ordinary factory does not declare; declare it on the ordinary factory so the transition plan schedules it"
    )]
    UndeclaredTransitionDependency {
        component: String,
        role: ComponentFactoryRole,
        dependency: String,
    },

    #[error("component '{component}' has no planned change for its transition handler")]
    ChangeUnavailable { component: &'static str },

    #[error("component '{component}' declares no hot update")]
    HotUpdateUnavailable { component: &'static str },

    #[error("hot update for component '{component}' panicked")]
    HotUpdatePanicked { component: &'static str },

    #[error("component '{component}' has no previous instance for its replacement factory")]
    PreviousUnavailable { component: &'static str },

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
        "ambiguous provider for '{type_name}'{}: {} tie for it; mark exactly one `#[primary]`, \
         give them distinct priorities, or inject `Vec`/`HashMap<String, _>` to receive all of them",
        component_id.as_ref().map(|id| format!(" required by '{id}'")).unwrap_or_default(),
        candidates.join(", ")
    )]
    AmbiguousProvider {
        /// Stable consumer component identity, when selection has a consumer.
        component_id: Option<String>,
        /// Stable underlying requested Rust type.
        type_name: String,
        /// Stable component IDs of every eligible provider the consumer can see.
        candidates: Vec<String>,
    },

    #[error("dependency cycle: no construction order for components: {0}")]
    DependencyCycle(String),

    #[error(
        "dependency '{dependency}' of component '{component_id}' resolves to '{target}' in scope '{scope}', which is not open on every chain of the consumer"
    )]
    UnplacedDependency {
        /// Stable consumer component identity.
        component_id: String,
        /// Human-readable dependency description.
        dependency: String,
        /// Stable identity of the selected component.
        target: String,
        /// The selected component's scope.
        scope: &'static str,
    },

    /// A factory built another component than the slot it runs for holds.
    #[error("the factory for component '{component}' built '{built}' instead")]
    FactoryOutputMismatch {
        component: &'static str,
        built: &'static str,
    },

    #[error("component '{component}' has no slot in the layout of scope '{scope}'")]
    UnplannedComponent {
        component: &'static str,
        scope: &'static str,
    },

    /// A scope was opened with a seed for a component one of its factories builds.
    #[error(
        "scope '{scope}' was opened with a seed for component '{component}', which its factory builds"
    )]
    SeedForBuiltComponent {
        component: &'static str,
        scope: &'static str,
    },

    /// A scope was opened with two seeds for the same slot.
    #[error("scope '{scope}' was opened with more than one seed for component '{component}'")]
    DuplicateSeed {
        component: &'static str,
        scope: &'static str,
    },

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

    #[error("component '{component}' handle does not support generation-local snapshots")]
    SnapshotHandleUnsupported { component: &'static str },

    #[error("component '{component}' stores no live cell for a runtime rebind")]
    LiveCellUnavailable { component: &'static str },

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
