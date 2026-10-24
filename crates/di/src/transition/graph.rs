//! The effective graph: the selected, validated component graph of one runtime generation.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use upwell_core::{RuntimeGenerationId, ScopeId, Singleton, StaticScope, Transient};

use super::plan::{self, LiveClosure};
use super::{
    DependencyDemand, DependencyDemandId, EffectiveNode, EffectiveNodeRole, EffectiveTarget,
    FactoryIdentity, StaleGraphCandidate, TransitionPlan,
};
use crate::{
    ComponentDescriptor, ComponentRegistry, DependencyTarget, ProviderSelectionModel,
    topological_sort,
};

/// A complete immutable effective dependency graph derived without constructing components.
///
/// The derived graph data is shared, so [`same_graph`](Self::same_graph) can hand an
/// unchanged graph to a new candidate without rebuilding it; the identity and generation
/// stay per instance.
pub struct EffectiveGraph {
    pub(super) identity: Arc<()>,
    pub(super) generation: RuntimeGenerationId,
    pub(super) nodes: Arc<BTreeMap<&'static str, EffectiveNode>>,
    pub(super) reverse: Arc<BTreeMap<&'static str, Box<[DependencyDemandId]>>>,
    pub(super) singleton_order: Arc<[&'static str]>,
    pub(super) construction_order: Arc<[&'static str]>,
    pub(super) selection: Arc<ProviderSelectionModel>,
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

        let singleton_order = topological_sort(&singletons, &prebuilt, &selection, can_reach)?
            .into_iter()
            .map(|component| component.id)
            .collect::<Arc<[_]>>();
        let construction_order = singleton_order
            .iter()
            .filter(|id| constructed.contains(*id))
            .copied()
            .collect::<Arc<[_]>>();

        Ok(Self {
            identity: Arc::new(()),
            generation,
            nodes: Arc::new(nodes),
            reverse: Arc::new(reverse),
            singleton_order,
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

    /// A new graph instance over this graph's unchanged data, for a candidate whose
    /// effective registry is identical. It has its own process-local identity, so prepared
    /// transition work stays bound to exactly one instance, and keeps this generation until
    /// it is committed.
    #[doc(hidden)]
    pub fn same_graph(&self) -> Self {
        Self {
            identity: Arc::new(()),
            generation: self.generation,
            nodes: Arc::clone(&self.nodes),
            reverse: Arc::clone(&self.reverse),
            singleton_order: Arc::clone(&self.singleton_order),
            construction_order: Arc::clone(&self.construction_order),
            selection: Arc::clone(&self.selection),
        }
    }

    /// Whether this graph and `other` share their derived data, as a graph produced by
    /// [`same_graph`](Self::same_graph) does with its source.
    #[doc(hidden)]
    pub fn shares_data_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.nodes, &other.nodes)
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

    /// Every singleton, factory-less ones included, in construction order. Building a
    /// root walks this order, so seeded instances are checked where they would have been
    /// constructed.
    pub fn singleton_order(&self) -> &[&'static str] {
        &self.singleton_order
    }

    pub fn construction_order(&self) -> &[&'static str] {
        &self.construction_order
    }

    /// Plans an ordinary DI transition between two validated graphs.
    ///
    /// Live-compatible retargets and live dependents plan as local `RebindLive`
    /// actions whose closure stays local to the rebind itself: the rebind's own
    /// dependents are never propagated, because a live rebind keeps the instance
    /// alive. Use [`plan_runtime_transition`](Self::plan_runtime_transition) for the
    /// runtime, which reconstructs every live consumer it cannot rebind in place.
    pub fn plan_transition(&self, candidate: &Self) -> Result<TransitionPlan, StaleGraphCandidate> {
        plan::plan_with_invalidations(self, candidate, &BTreeSet::new(), LiveClosure::Local)
    }

    /// Plans a transition for the runtime: runtime-invalidated roots replace their
    /// active instances, and every live consumer the runtime will reconstruct
    /// propagates to its own dependents through the same reverse reachability to a
    /// fixed point.
    ///
    /// Roots missing from either graph create no node, and structural Add and Remove
    /// actions keep their precedence over forced replaces. The graph diff stays purely
    /// structural.
    ///
    /// `rebinds_in_place` decides, for one live binding's active and candidate demands,
    /// whether the runtime can keep the consumer and republish its held live cell: the
    /// runtime must be able to carry the consumer across generations and reach the live
    /// cell of every target on both sides. A live binding whose demand is
    /// [`rebindable`](DependencyDemand::rebindable) on both sides and that the predicate
    /// accepts plans with [`BindingTransition::in_place`] set; the consumer keeps its
    /// instance, so it does not propagate. Every other live consumer is reconstructed:
    /// it propagates through the changed-instance queue and its fixed (`Snapshot`)
    /// dependents plan as `Replace` rather than retaining instances snapshotted from the
    /// retired active root. This holds even when `invalidated_roots` is empty, because a
    /// pure provider/factory change can start the closure on its own.
    #[doc(hidden)]
    pub fn plan_runtime_transition(
        &self,
        candidate: &Self,
        invalidated_roots: &BTreeSet<&'static str>,
        rebinds_in_place: impl Fn(&DependencyDemand, &DependencyDemand) -> bool,
    ) -> Result<TransitionPlan, StaleGraphCandidate> {
        plan::plan_with_invalidations(
            self,
            candidate,
            invalidated_roots,
            LiveClosure::Runtime(&rebinds_in_place),
        )
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
            config_type_id: dependency.ty.type_id,
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
