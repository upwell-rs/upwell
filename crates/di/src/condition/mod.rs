use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use upwell_core::{
    ConditionScalar, ConfigFactDescriptor, ConfigFactId, ProviderMappingId, ScopeId,
};

use crate::{ComponentDescriptor, ComponentRegistry, ProviderDescriptor, ProviderSelectionModel};

mod error;
mod evaluation;
mod graph;
mod validation;

pub use error::ConditionError;
pub use graph::{AvailabilityEdge, ConditionDependency};

/// Typed config facts supplied to one catalog evaluation.
#[derive(Clone, Default)]
pub struct ConditionFactSnapshot {
    pub(super) facts: BTreeMap<ConfigFactId, ConditionScalar>,
}

impl ConditionFactSnapshot {
    pub fn new(
        facts: impl IntoIterator<Item = (ConfigFactId, ConditionScalar)>,
    ) -> Result<Self, ConditionError> {
        let mut snapshot = Self::default();

        for (id, value) in facts {
            if snapshot.facts.insert(id, value).is_some() {
                return Err(ConditionError::DuplicateFactValue(id));
            }
        }

        Ok(snapshot)
    }
}

impl fmt::Debug for ConditionFactSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let facts = self
            .facts
            .iter()
            .map(|(id, value)| (*id, value.kind()))
            .collect::<Vec<_>>();

        formatter
            .debug_struct("ConditionFactSnapshot")
            .field("facts", &facts)
            .finish()
    }
}

/// One redacted condition-node outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConditionDecision {
    pub component_id: &'static str,
    pub condition_id: &'static str,
    pub predicate: upwell_core::ConditionPredicateKind,
    pub callback_kind: Option<&'static str>,
    pub source: upwell_core::DescriptorSource,
    pub outcome: bool,
}

/// Deterministic eligibility result for one supplied fact snapshot.
#[derive(Clone, Debug)]
pub struct ConditionEvaluation {
    pub(super) catalog: CatalogIdentity,
    pub(super) facts: ConditionFactSnapshot,
    pub(super) eligible: ComponentRegistry,
    pub(super) components: BTreeMap<&'static str, bool>,
    pub(super) providers: BTreeMap<ProviderMappingId, bool>,
    pub(super) decisions: Vec<ConditionDecision>,
}

impl ConditionEvaluation {
    pub fn eligible_registry(&self) -> &ComponentRegistry {
        &self.eligible
    }

    pub fn component_eligible(&self, id: &str) -> Option<bool> {
        self.components.get(id).copied()
    }

    pub fn provider_eligible(&self, id: ProviderMappingId) -> Option<bool> {
        self.providers.get(&id).copied()
    }

    pub fn decisions(&self) -> &[ConditionDecision] {
        &self.decisions
    }

    /// Returns whether this evaluation was produced from `registry`'s static DI catalog.
    pub fn belongs_to(&self, registry: &ComponentRegistry) -> Result<bool, ConditionError> {
        Ok(self.catalog.registry == RegistryIdentity::new(registry)?)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(super) struct CatalogIdentity {
    registry: RegistryIdentity,
    facts: Box<[ConfigFactDescriptor]>,
}

#[derive(Clone, Eq, PartialEq)]
struct RegistryIdentity {
    components: Box<[ComponentIdentity]>,
    providers: Box<[ProviderIdentity]>,
}

#[derive(Clone, Eq, PartialEq)]
struct ComponentIdentity {
    id: &'static str,
    concrete_type: TypeIdentity,
    scope: ScopeId,
    scope_name: &'static str,
    scope_rank: u8,
    scope_transient: bool,
    condition: Option<usize>,
    factory: Option<&'static str>,
    factories: usize,
    hooks: usize,
    construct: Option<usize>,
    dependencies: Option<usize>,
}

#[derive(Clone, Eq, PartialEq)]
struct ProviderIdentity {
    mapping: ProviderMappingId,
    trait_type: TypeIdentity,
    concrete_type: TypeIdentity,
    primary: bool,
    priority: i64,
    ordering: Box<[ProviderOrderIdentity]>,
    erase: usize,
}

#[derive(Clone, Eq, PartialEq)]
struct ProviderOrderIdentity {
    target_type: TypeIdentity,
    traits: Box<[TypeIdentity]>,
    before: bool,
}

#[derive(Clone, Eq, PartialEq)]
struct TypeIdentity {
    id: TypeId,
    name: &'static str,
}

impl TypeIdentity {
    fn new(descriptor: upwell_core::TypeDescriptor) -> Self {
        Self {
            id: descriptor.type_id,
            name: (descriptor.type_name)(),
        }
    }
}

impl CatalogIdentity {
    fn new(
        registry: &ComponentRegistry,
        facts: &BTreeMap<ConfigFactId, ConfigFactDescriptor>,
    ) -> Result<Self, ConditionError> {
        Ok(Self {
            registry: RegistryIdentity::new(registry)?,
            facts: facts.values().copied().collect(),
        })
    }
}

impl fmt::Debug for CatalogIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CatalogIdentity(<redacted>)")
    }
}

impl RegistryIdentity {
    fn new(registry: &ComponentRegistry) -> Result<Self, ConditionError> {
        let components = registry
            .resolved_components()
            .map_err(ConditionError::Registry)?;
        let by_type = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect::<HashMap<_, _>>();
        let components = components
            .iter()
            .map(|component| {
                let factory = component
                    .effective_factory()
                    .map_err(ConditionError::Registry)?;

                Ok(ComponentIdentity {
                    id: component.id,
                    concrete_type: TypeIdentity::new(component.ty),
                    scope: component.scope.id(),
                    scope_name: component.scope.name(),
                    scope_rank: component.scope.rank(),
                    scope_transient: component.scope.is_transient(),
                    condition: component
                        .condition
                        .map(|condition| std::ptr::from_ref(condition).addr()),
                    factory: factory.map(|factory| factory.id),
                    factories: component.factories as usize,
                    hooks: component.hooks as usize,
                    construct: factory.map(|factory| factory.construct as usize),
                    dependencies: factory.map(|factory| factory.dependencies as usize),
                })
            })
            .collect::<Result<Vec<_>, ConditionError>>()?
            .into_boxed_slice();
        let mut providers = registry
            .providers
            .iter()
            .map(|provider| {
                let component = by_type.get(&provider.concrete_ty.type_id).ok_or(
                    ConditionError::MissingProviderComponent {
                        trait_type: (provider.trait_ty.type_name)(),
                        qualifier: provider.qualifier,
                    },
                )?;
                let ordering = provider
                    .ordering
                    .iter()
                    .map(|order| ProviderOrderIdentity {
                        target_type: TypeIdentity::new(order.target),
                        traits: order
                            .traits
                            .iter()
                            .copied()
                            .map(TypeIdentity::new)
                            .collect(),
                        before: matches!(order.direction, crate::ProviderOrderDirection::Before),
                    })
                    .collect();

                Ok(ProviderIdentity {
                    mapping: provider.mapping_id(component),
                    trait_type: TypeIdentity::new(provider.trait_ty),
                    concrete_type: TypeIdentity::new(provider.concrete_ty),
                    primary: provider.primary,
                    priority: provider.priority,
                    ordering,
                    erase: provider.erase as usize,
                })
            })
            .collect::<Result<Vec<_>, ConditionError>>()?;

        providers.sort_by_key(|provider| provider.mapping);

        Ok(Self {
            components,
            providers: providers.into_boxed_slice(),
        })
    }
}

/// Eligibility decisions whose ordinary DI graph has also passed validation.
pub struct ValidatedConditionEvaluation {
    pub(super) evaluation: ConditionEvaluation,
    pub(super) selection: ProviderSelectionModel,
}

impl ValidatedConditionEvaluation {
    pub fn evaluation(&self) -> &ConditionEvaluation {
        &self.evaluation
    }

    pub fn registry(&self) -> &ComponentRegistry {
        self.evaluation.eligible_registry()
    }

    pub fn provider_selection(&self) -> &ProviderSelectionModel {
        &self.selection
    }
}

/// Validated static inputs for deterministic condition evaluation.
pub struct ConditionCatalog {
    pub(super) identity: CatalogIdentity,
    pub(super) components: BTreeMap<&'static str, ComponentDescriptor>,
    pub(super) registry_order: Vec<&'static str>,
    pub(super) component_order: Vec<&'static str>,
    pub(super) providers: Vec<(ProviderMappingId, ProviderDescriptor)>,
    pub(super) facts: BTreeMap<ConfigFactId, ConfigFactDescriptor>,
    pub(super) provider_order: HashMap<TypeId, HashMap<TypeId, usize>>,
}

#[cfg(test)]
mod tests;
