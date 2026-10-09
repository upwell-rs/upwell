use thiserror::Error;

use upwell_core::ScopeId;

/// Errors from the protocol-agnostic application core: registry validation, the DI
/// engine, config, and hooks.
///
/// A protocol's own error type wraps this (typically via `#[from]`), so a protocol's
/// `build`/`serve` can absorb assembly failures while adding its own variants.
#[derive(Debug, Error)]
pub enum Error {
    /// A component descriptor uses a non-universal scope absent from the protocol topology.
    #[error(
        "component '{component}' declares scope '{scope}', which the active protocol does not open"
    )]
    UndeclaredScope {
        /// The component's stable Rust type name.
        component: String,
        /// The undeclared stable scope identity.
        scope: ScopeId,
    },

    /// A protocol attempted to open a boundary absent from its prepared topology.
    #[error("scope boundary '{scope}' is not declared by the active protocol")]
    UndeclaredScopeOpen {
        /// The requested stable scope identity.
        scope: ScopeId,
    },

    /// A parent container belongs to another prepared application runtime.
    #[error("cannot open scope '{child}' over foreign-runtime parent '{parent}'")]
    ForeignScopeParent {
        /// The requested child boundary.
        child: ScopeId,
        /// The supplied parent's stable scope identity.
        parent: ScopeId,
    },

    /// A nested boundary was opened over a parent without a runtime-generation lease.
    #[error("cannot open scope '{child}' over unpinned parent '{parent}'")]
    UnpinnedScopeParent {
        /// The requested child boundary.
        child: ScopeId,
        /// The supplied parent's stable scope identity.
        parent: ScopeId,
    },

    /// A child was opened over a parent other than its declared parent.
    #[error("scope '{child}' requires parent '{expected}', but parent '{actual}' was supplied")]
    InvalidScopeParent {
        /// The requested child boundary.
        child: ScopeId,
        /// The topology-declared parent identity.
        expected: ScopeId,
        /// The supplied parent container's identity.
        actual: ScopeId,
    },

    /// A dynamic seed is registered for another boundary.
    #[error("seed type '{type_name}' belongs to scope '{expected}', not opened scope '{actual}'")]
    InvalidSeedDestination {
        /// The seeded Rust type name.
        type_name: &'static str,
        /// The descriptor's declared destination.
        expected: ScopeId,
        /// The boundary being opened.
        actual: ScopeId,
    },

    /// A dynamic seed has no factory-less descriptor registered for this application.
    #[error("seed type '{type_name}' is not registered for opened scope '{scope}'")]
    UnregisteredSeed {
        /// The boundary being opened.
        scope: ScopeId,
        /// The seeded Rust type name.
        type_name: &'static str,
    },

    /// The same concrete seed type was supplied more than once.
    #[error("seed type '{type_name}' is supplied more than once for scope '{scope}'")]
    DuplicateSeedType {
        /// The boundary being opened.
        scope: ScopeId,
        /// The duplicated Rust type name.
        type_name: &'static str,
    },

    #[error(
        "missing config for component '{component}': no binding of type '{type_name}' \
         at path '{path}'"
    )]
    MissingConfig {
        component: String,
        type_name: String,
        path: String,
    },

    #[error(
        "ambiguous config for component '{component}': type '{type_name}' is bound at \
         {count} paths ({paths}); name one with `#[config(\"..\")]`"
    )]
    AmbiguousConfig {
        component: String,
        type_name: String,
        count: usize,
        paths: String,
    },

    /// No safe platform directory layout was available for the application.
    #[error("failed to resolve safe application directories: {0}")]
    Directories(#[source] std::io::Error),

    /// A component-graph failure from the DI engine (cycle, missing dependency, ambiguous
    /// provider, scope violation, duplicate/ambiguous factory, …).
    #[error(transparent)]
    Di(#[from] upwell_di::Error),

    /// A conditional component catalog or evaluation is invalid.
    #[error(transparent)]
    Condition(#[from] upwell_di::ConditionError),

    /// A condition evaluation was produced from another application's config bindings.
    #[error("condition evaluation belongs to another application config-binding catalog")]
    ConditionEvaluationApplicationMismatch,

    /// The requested graph transition cannot be applied safely without restarting.
    #[error(transparent)]
    RestartRequired(#[from] crate::transition::RestartRequired),

    /// The candidate graph was derived from another committed generation.
    #[error(transparent)]
    StaleGraphCandidate(#[from] upwell_di::StaleGraphCandidate),

    /// The same type was supplied as a candidate generation override more than once.
    #[error("generation override type '{type_name}' is supplied more than once")]
    DuplicateGenerationOverride {
        /// The duplicated Rust type name.
        type_name: &'static str,
    },

    /// A candidate generation override does not replace a singleton the plan retains.
    #[error("generation override type '{type_name}' does not replace a retained singleton")]
    InvalidGenerationOverride {
        /// The rejected Rust type name.
        type_name: &'static str,
    },

    /// Candidate preparation crossed a panicking extension boundary.
    #[error("candidate runtime preparation panicked")]
    CandidatePreparationPanicked,

    /// A runtime reloader was used before its application was built or after its runtime
    /// was dropped.
    #[error("the application runtime is not attached or has been dropped")]
    RuntimeUnavailable,

    /// A configuration loading, binding, or substitution failure.
    #[error(transparent)]
    Config(#[from] upwell_config::ConfigError),

    /// A configuration reload failed (load, bind, hook rejection, or panic) before
    /// anything was published. Boxed to keep the app error within the
    /// `result_large_err` budget every `crate::Result` in the crate shares.
    #[error(transparent)]
    ConfigReload(Box<upwell_config::ConfigReloadError>),

    /// A hook failure (e.g. an unresolvable receiver or parameter).
    #[error(transparent)]
    Hook(#[from] upwell_hooks::Error),

    /// A protocol-owned scope topology declaration is structurally invalid.
    #[error(transparent)]
    ScopeTopology(#[from] crate::scope::ScopeTopologyError),

    /// Plugin declarations could not be resolved into one deterministic effective plan.
    #[error(transparent)]
    Composition(#[from] crate::CompositionDiagnostics),

    /// Retained plugin contributions could not be frozen or lowered safely.
    #[error(transparent)]
    PluginPlan(#[from] crate::PluginPlanError),

    /// A protocol supplied structurally invalid owner-scoped tooling metadata.
    #[cfg(feature = "tooling")]
    #[error(transparent)]
    ToolingContribution(#[from] crate::ToolingContributionError),

    /// An application-defined error surfaced through the framework.
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl From<upwell_config::ConfigReloadError> for Error {
    fn from(error: upwell_config::ConfigReloadError) -> Self {
        Self::ConfigReload(Box::new(error))
    }
}

/// The app-layer result type.
pub type Result<T, E = Error> = core::result::Result<T, E>;
