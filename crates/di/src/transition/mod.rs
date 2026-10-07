//! Immutable effective dependency graphs and pure transition planning.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use upwell_core::{
    Cardinality, DependencyObservation, ProviderMappingId, ResolutionMode, RuntimeGenerationId,
    ScopeId, Singleton, StaticScope, Transient,
};

use crate::{
    ComponentDescriptor, ComponentRegistry, DependencyTarget, ProviderSelectionModel,
    topological_sort,
};

mod plan;

pub use plan::{
    BindingTransition, GraphDiff, NodeAction, NodeChange, NodeChangeKind, PlannedNode, ReasonKind,
    StaleGraphCandidate, TransitionPlan, TransitionReason,
};

/// Runtime role represented by an effective component node.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EffectiveNodeRole {
    /// A stored root instance that can participate in a runtime transition.
    Singleton,
    /// A construction recipe used by future openings of one declared scope.
    FutureScope(ScopeId),
    /// A recipe executed for each resolution rather than a stored instance.
    Transient,
}

/// Stable identity of one dependency demand in a component's selected factory.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
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
    fn live_compatible_with(&self, candidate: &Self) -> bool {
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
        config_type: &'static str,
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

/// A complete immutable effective dependency graph derived without constructing components.
pub struct EffectiveGraph {
    identity: Arc<()>,
    generation: RuntimeGenerationId,
    nodes: BTreeMap<&'static str, EffectiveNode>,
    reverse: BTreeMap<&'static str, Box<[DependencyDemandId]>>,
    construction_order: Box<[&'static str]>,
    selection: Arc<ProviderSelectionModel>,
}

impl EffectiveGraph {
    /// Validates and freezes an effective registry using caller-owned scope reachability.
    pub fn build(
        generation: RuntimeGenerationId,
        registry: &ComponentRegistry,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool + Copy,
    ) -> crate::Result<Self> {
        let components = registry.resolved_components()?;
        let selection = Arc::new(registry.provider_selection_model(&components)?);

        registry.validate_with_scope_reachability_using(&components, &selection, can_reach)?;

        Self::from_validated(generation, &components, selection, can_reach)
    }

    fn from_validated(
        generation: RuntimeGenerationId,
        components: &[ComponentDescriptor],
        selection: Arc<ProviderSelectionModel>,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool + Copy,
    ) -> crate::Result<Self> {
        let by_type = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect::<BTreeMap<_, _>>();
        let mut nodes = BTreeMap::new();

        for component in components {
            let factory = component.effective_factory()?;
            let dependencies = factory
                .map(|factory| (factory.dependencies)())
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(ordinal, dependency)| {
                    capture_demand(
                        *component, ordinal, dependency, &selection, &by_type, can_reach,
                    )
                })
                .collect::<crate::Result<Vec<_>>>()?
                .into_boxed_slice();
            let scope = component.scope.id();
            let role = if scope == Singleton::ID {
                EffectiveNodeRole::Singleton
            } else if scope == Transient::ID {
                EffectiveNodeRole::Transient
            } else {
                EffectiveNodeRole::FutureScope(scope)
            };

            nodes.insert(
                component.id,
                EffectiveNode {
                    id: component.id,
                    name: component.name,
                    concrete_type: (component.ty.type_name)(),
                    scope,
                    role,
                    factory: factory.map(|factory| FactoryIdentity { id: factory.id }),
                    dependencies,
                },
            );
        }

        let reverse = reverse_index(&nodes);
        let singletons = components
            .iter()
            .filter(|component| component.scope.id() == Singleton::ID)
            .copied()
            .collect::<Vec<_>>();
        let mut prebuilt = HashSet::new();
        let mut constructed = HashSet::new();

        for component in &singletons {
            if component.effective_factory()?.is_some() {
                constructed.insert(component.id);
            } else {
                prebuilt.insert(component.ty.type_id);
            }
        }

        let construction_order = topological_sort(&singletons, &prebuilt, &selection, can_reach)?
            .into_iter()
            .filter(|component| constructed.contains(component.id))
            .map(|component| component.id)
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Ok(Self {
            identity: Arc::new(()),
            generation,
            nodes,
            reverse,
            construction_order,
            selection,
        })
    }

    pub const fn generation(&self) -> RuntimeGenerationId {
        self.generation
    }

    /// Process-local identity used to bind prepared transition work to the exact active
    /// graph instance rather than only its numeric generation.
    #[doc(hidden)]
    pub fn identity(&self) -> &Arc<()> {
        &self.identity
    }

    /// Assigns the semantic generation allocated when this prepared graph is committed.
    #[doc(hidden)]
    pub fn into_committed_generation(mut self, generation: RuntimeGenerationId) -> Self {
        self.generation = generation;

        self
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &EffectiveNode> {
        self.nodes.values()
    }

    pub fn node(&self, id: &str) -> Option<&EffectiveNode> {
        self.nodes.get(id)
    }

    pub fn provider_selection(&self) -> &Arc<ProviderSelectionModel> {
        &self.selection
    }

    pub fn construction_order(&self) -> &[&'static str] {
        &self.construction_order
    }

    pub fn plan_transition(&self, candidate: &Self) -> Result<TransitionPlan, StaleGraphCandidate> {
        plan::plan_with_invalidations(self, candidate, &BTreeSet::new())
    }

    /// Plans a transition while forcing runtime-invalidated component roots to replace
    /// their active instances and propagate to dependents through the same reverse
    /// propagation as structural changes.
    ///
    /// Roots missing from either graph create no node, and structural Add and Remove
    /// actions keep their precedence over forced replaces. The graph diff stays purely
    /// structural.
    #[doc(hidden)]
    pub fn plan_transition_with_invalidations(
        &self,
        candidate: &Self,
        invalidated_roots: &BTreeSet<&'static str>,
    ) -> Result<TransitionPlan, StaleGraphCandidate> {
        plan::plan_with_invalidations(self, candidate, invalidated_roots)
    }
}

fn capture_demand(
    consumer: ComponentDescriptor,
    ordinal: usize,
    dependency: upwell_core::DependencyDescriptor,
    selection: &ProviderSelectionModel,
    by_type: &BTreeMap<TypeId, ComponentDescriptor>,
    can_reach: impl Fn(ScopeId, ScopeId) -> bool + Copy,
) -> crate::Result<DependencyDemand> {
    let targets = if dependency.config {
        vec![EffectiveTarget::Config {
            config_type: (dependency.ty.type_name)(),
            binding_path: dependency.qualifier,
        }]
    } else if dependency.dynamic {
        vec![EffectiveTarget::Dynamic {
            requested_type: (dependency.ty.type_name)(),
            qualifier: dependency.qualifier,
        }]
    } else {
        selection
            .selected_dependencies_with_scope_reachability(&consumer, &dependency, can_reach)
            .into_iter()
            .map(|selected| match selected.target {
                DependencyTarget::Component(component) => Ok(EffectiveTarget::Component {
                    component: component.id,
                    scope: component.scope.id(),
                }),
                DependencyTarget::Provider(provider) => {
                    let component = by_type
                        .get(&provider.concrete_ty.type_id)
                        .copied()
                        .ok_or_else(|| {
                            crate::Error::ProviderComponentMissing(Box::new(
                                crate::error::ProviderComponentMissing {
                                    trait_name: (provider.trait_ty.type_name)().to_string(),
                                    trait_type: (provider.trait_ty.type_name)().to_string(),
                                    component: (provider.concrete_ty.type_name)().to_string(),
                                    component_type: (provider.concrete_ty.type_name)().to_string(),
                                    qualifier: provider.qualifier.to_string(),
                                },
                            ))
                        })?;
                    Ok(EffectiveTarget::Provider {
                        mapping: provider.mapping_id(&component),
                        scope: component.scope.id(),
                    })
                }
            })
            .collect::<crate::Result<Vec<_>>>()?
    };

    Ok(DependencyDemand {
        id: DependencyDemandId {
            consumer: consumer.id,
            ordinal,
        },
        name: dependency.name,
        requested_type: (dependency.ty.type_name)(),
        cardinality: dependency.cardinality,
        optional: dependency.optional,
        dynamic: dependency.dynamic,
        qualifier: dependency.qualifier,
        config: dependency.config,
        resolution: dependency.resolution,
        observation: dependency.observation,
        targets: targets.into_boxed_slice(),
    })
}

fn reverse_index(
    nodes: &BTreeMap<&'static str, EffectiveNode>,
) -> BTreeMap<&'static str, Box<[DependencyDemandId]>> {
    let mut reverse = BTreeMap::<_, BTreeSet<_>>::new();

    for node in nodes.values() {
        for dependency in &node.dependencies {
            for target in &dependency.targets {
                if let Some(component) = target.component_id() {
                    reverse.entry(component).or_default().insert(dependency.id);
                }
            }
        }
    }

    reverse
        .into_iter()
        .map(|(component, consumers)| {
            (
                component,
                consumers.into_iter().collect::<Vec<_>>().into_boxed_slice(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
