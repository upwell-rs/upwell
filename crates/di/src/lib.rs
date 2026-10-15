//! The Upwell dependency-injection engine.
//!
//! This crate owns the runtime DI machinery: the parent-linked [`ScopeContainer`], the
//! construction-time [`Factory`]/[`FromContainer`] extractors, the component and provider
//! descriptors, and the [`ComponentRegistry`] that validates the graph. It builds on the
//! leaf vocabulary in `upwell-core` (type descriptors, the dependency model, the
//! resolver abstraction) and on `upwell-hooks` for the per-component hook slice each
//! [`ComponentDescriptor`] carries.
//!
//! Config is deliberately *not* here: it is an external resolver (`upwell-config`)
//! reached through the [`ResolverCtx`](upwell_core::ResolverCtx), so the container
//! stays unaware of it.

pub mod condition;
pub mod construct;
pub mod container;
pub mod descriptors;
pub mod error;
mod observability;
mod primitives;
pub mod registry;
pub mod root;
mod seeded;
#[cfg(test)]
mod test_support;
pub mod transition;

pub use condition::{
    AvailabilityEdge, ConditionCatalog, ConditionDecision, ConditionDependency, ConditionError,
    ConditionEvaluation, ConditionFactSnapshot, ValidatedConditionEvaluation,
};
pub use construct::{
    Factory, FactoryOutput, FromContainer, dependency_of, dependency_of_observed, dispatch_factory,
    factory_dependencies, short_name,
};
pub use container::{
    ComponentContainer, ComponentSource, ScopeContainer, ScopeRegistry, topological_sort,
};
pub use descriptors::component::from_boxed;
pub use descriptors::{
    BoxedComponent, COMPONENTS, Cardinality, Component, ComponentConstructionContext,
    ComponentDescriptor, ComponentFactories, ComponentFactory, ComponentFactoryDescriptor,
    ConditionDescriptor, Dep, DependencyDescriptor, DependencyObservation, DescriptorFor, Dynamic,
    Injectable, Live, LiveRef, PROVIDERS, Provide, ProviderDescriptor, ProviderMappingId,
    ProviderOf, ProviderOrder, ProviderOrderDirection, Registration, RegistryFor, ResolutionMode,
    Scope, ScopeId, ServiceComponent, Singleton, StaticScope, Transient, TypeDescriptor,
    UpwellDescriptor, Wired, Wiring,
};
pub use error::{
    DeferredTransientDependency, Error, InvalidFreshDependency, ProviderComponentMissing,
    ProviderOrderCycle, ProviderOrderSourceTraitMismatch, ProviderOrderTargetTraitMismatch, Result,
    ScopeUnreachableDependency, ScopeUnreachableProvider, ScopeViolation,
};
pub use primitives::{Deferred, Fresh, FreshFromContainer, Lazy};
pub use registry::{
    ComponentRegistry, DependencySelectionReason, DependencySelectionStage, DependencyTarget,
    ProviderSelectionModel, SelectedDependency,
};
pub use root::{ROOT_RESOLVER_ID, ROOT_RESOLVER_NAME, RootResolver, root_resolver_descriptor};
pub use transition::{
    BindingTransition, DependencyDemand, DependencyDemandId, EffectiveGraph, EffectiveNode,
    EffectiveNodeRole, EffectiveTarget, FactoryIdentity, GraphDiff, NodeAction, NodeChange,
    NodeChangeKind, PlannedNode, ReasonKind, StaleGraphCandidate, TransitionPlan, TransitionReason,
};

/// Re-exported so macro-generated code can reach the `#[distributed_slice]` attribute
/// through a stable path.
#[doc(hidden)]
pub use linkme;

/// Re-exported so macro-generated code can reach `inventory::collect!`/`submit!`/`iter`
/// through a stable path — the `inventory` registration backend, selected on macOS or the
/// `upwell_hybrid` cfg.
#[doc(hidden)]
pub use inventory;
