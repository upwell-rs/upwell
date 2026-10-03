//! Leaf vocabulary for the Upwell framework.
//!
//! This crate is the bottom of the dependency graph: it depends on nothing internal and
//! everything else depends on it. It defines the shared *language* the layers above
//! speak — type descriptors, the dependency-edge model, component scopes, the by-type
//! [`Descriptor`] seam — and the [`resolver`] abstraction through which all dependency
//! resolution flows.
//!
//! It contains no runtime, no container, no config, and no protocol code. Those live in
//! `upwell-di`, `upwell-config`, `upwell-hooks`, and `upwell-daemon`.

pub mod condition;
pub mod dependency;
pub mod descriptor;
pub mod generation;
pub mod id;
pub mod resolver;
pub mod scope;
pub mod types;

pub use condition::{
    AvailabilityConditionCallback, AvailabilityConditionContext, AvailabilityTarget,
    ConditionDescriptor, ConditionPredicate, ConditionPredicateKind, ConditionScalar,
    ConditionScalarKind, ConditionScalarLiteral, ConfigConditionCallback, ConfigConditionContext,
    ConfigFactDescriptor, ConfigFactId, DescriptorSource, ProviderMappingId,
};
pub use dependency::{Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode};
pub use descriptor::{Descriptor, DescriptorFor, RegistryFor, RuntimeDescriptor, UpwellDescriptor};
pub use generation::RuntimeGenerationId;
pub use id::{FRAMEWORK_NAMESPACE, IdErrorKind, InvalidNamespacedId, NamespacedIdType};
pub use resolver::{Resolver, ResolverCtx, ResolverCtxExt, ResolverSet};
pub use scope::{
    InvalidScopeId, InvalidScopeIdReason, Scope, ScopeId, Singleton, StaticScope, Transient,
};
pub use types::{TypeDescriptor, type_id_of};
