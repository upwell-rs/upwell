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
pub mod entry;
pub mod generation;
pub mod id;
pub mod resolver;
pub mod scope;
pub mod source;
pub mod types;

pub use condition::{
    ConditionCallback, ConditionContext, ConditionDescriptor, ConditionExpression, ConditionInput,
    ConditionInputId, ConditionInputRead, ConditionInputValue, ConditionPredicate,
    ConditionPredicateKind, ConditionPurity, ConditionScalar, ConditionScalarKind,
    ConditionScalarLiteral, ConfigBindingId, ConfigFactDescriptor, ConfigFactId, Eligibility,
    ProviderMappingId,
};
pub use dependency::{Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode};
pub use descriptor::{Descriptor, DescriptorFor, RegistryFor, RuntimeDescriptor, UpwellDescriptor};
pub use entry::EntryKey;
pub use generation::RuntimeGenerationId;
pub use id::{FRAMEWORK_NAMESPACE, IdErrorKind, InvalidNamespacedId, NamespacedIdType};
pub use resolver::{Resolver, ResolverCtx, ResolverCtxExt, ResolverSet};
pub use scope::{
    InvalidScopeId, InvalidScopeIdReason, Scope, ScopeId, ScopeKey, Singleton, Transient,
};
pub use source::{DescriptorSource, ItemSource};
pub use types::{TypeDescriptor, type_id_of};
