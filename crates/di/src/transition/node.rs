//! The nodes of an effective graph: roles, dependency demands and their selected targets.

use std::any::TypeId;

use upwell_core::{
    Cardinality, DependencyObservation, ProviderMappingId, ResolutionMode, Scope, ScopeId,
    Singleton,
};

use super::plan::{self};

/// Runtime role represented by an effective component node.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum EffectiveNodeRole {
    /// A stored root instance that can participate in a runtime transition.
    Singleton,
    /// A construction recipe used by future openings of one declared scope.
    FutureScope(
        #[cfg_attr(feature = "serde", serde(serialize_with = "serialize_scope_id"))] ScopeId,
    ),
    /// A recipe executed for each resolution rather than a stored instance.
    Transient,
}

/// Serializes a scope identity as its stable namespaced string.
#[cfg(feature = "serde")]
fn serialize_scope_id<S: serde::Serializer>(
    scope: &ScopeId,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(scope.as_str())
}

/// Stable identity of one dependency demand in a component's selected factory.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DependencyDemandId {
    pub consumer: &'static str,
    pub ordinal: usize,
}

/// Comparison-relevant shape of one dependency demand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyDemand {
    pub id: DependencyDemandId,
    pub name: &'static str,
    pub requested_type: &'static str,
    pub cardinality: Cardinality,
    pub optional: bool,
    pub dynamic: bool,
    pub qualifier: Option<&'static str>,
    pub config: bool,
    pub resolution: ResolutionMode,
    pub observation: DependencyObservation,
    pub targets: Box<[EffectiveTarget]>,
}

impl DependencyDemand {
    /// Whether a consumer's handle for this demand is the exact live cell its root
    /// stores, so a runtime transition can republish it in place: a single, eagerly
    /// resolved, non-config live dependency on one singleton component or provider.
    pub fn rebindable(&self) -> bool {
        plan::is_live(self)
            && !self.config
            && self.resolution == ResolutionMode::Eager
            && self.targets.iter().all(|target| match target {
                EffectiveTarget::Component { scope, .. }
                | EffectiveTarget::Provider { scope, .. } => *scope == Singleton::ID,
                EffectiveTarget::Config { .. } | EffectiveTarget::Dynamic { .. } => false,
            })
    }

    pub(super) fn live_compatible_with(&self, candidate: &Self) -> bool {
        self.cardinality == Cardinality::One
            && candidate.cardinality == Cardinality::One
            && self.observation == DependencyObservation::Live
            && candidate.observation == DependencyObservation::Live
            && !self.optional
            && !candidate.optional
            && !self.dynamic
            && !candidate.dynamic
            && self.requested_type == candidate.requested_type
            && self.qualifier == candidate.qualifier
            && self.config == candidate.config
            && self.resolution == candidate.resolution
            && self.targets.len() == 1
            && candidate.targets.len() == 1
    }
}

/// Producer-authoritative target selected for one effective dependency demand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveTarget {
    Component {
        component: &'static str,
        scope: ScopeId,
    },
    Provider {
        mapping: ProviderMappingId,
        scope: ScopeId,
    },
    Config {
        /// The dependency type's full `std::any::type_name` path, for diagnostics.
        config_type: &'static str,
        /// The dependency type's exact identity, for type matching.
        config_type_id: TypeId,
        binding_path: Option<&'static str>,
    },
    Dynamic {
        requested_type: &'static str,
        qualifier: Option<&'static str>,
    },
}

impl EffectiveTarget {
    pub fn component_id(self) -> Option<&'static str> {
        match self {
            Self::Component { component, .. } => Some(component),
            Self::Provider { mapping, .. } => Some(mapping.component),
            Self::Config { .. } | Self::Dynamic { .. } => None,
        }
    }
}

/// One immutable node in a complete validated effective graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveNode {
    pub id: &'static str,
    pub name: &'static str,
    pub concrete_type: &'static str,
    pub scope: ScopeId,
    pub role: EffectiveNodeRole,
    pub factory: Option<FactoryIdentity>,
    pub dependencies: Box<[DependencyDemand]>,
}

/// One selected construction recipe identity.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct FactoryIdentity {
    pub id: &'static str,
}

impl std::fmt::Debug for FactoryIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("FactoryIdentity")
            .field(&self.id)
            .finish()
    }
}
