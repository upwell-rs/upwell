use std::collections::{BTreeMap, BTreeSet, HashMap};

use upwell_core::{
    AvailabilityTarget, ConditionDescriptor, ConditionPredicate, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, StaticScope,
};

use super::{ConditionCatalog, ConditionError, ConditionFactSnapshot};
use crate::{ComponentDescriptor, ComponentRegistry, Singleton};

impl ConditionCatalog {
    pub fn new(
        registry: &ComponentRegistry,
        config_facts: impl IntoIterator<Item = ConfigFactDescriptor>,
    ) -> Result<Self, ConditionError> {
        validate_manual_overrides(&registry.components)?;

        let resolved = registry
            .resolved_components()
            .map_err(ConditionError::Registry)?;
        let registry_order = resolved
            .iter()
            .map(|component| component.id)
            .collect::<Vec<_>>();
        let mut components = BTreeMap::new();
        let mut by_type = HashMap::new();

        for component in resolved {
            if components.insert(component.id, component).is_some() {
                return Err(ConditionError::DuplicateComponentId(component.id));
            }

            by_type.insert(component.ty.type_id, component);
        }

        let mut facts = BTreeMap::new();

        for fact in config_facts {
            if facts.insert(fact.id, fact).is_some() {
                return Err(ConditionError::DuplicateFactDescriptor(fact.id));
            }
        }

        let mut providers = Vec::with_capacity(registry.providers.len());
        let mut provider_ids = BTreeSet::new();

        for provider in &registry.providers {
            let component = by_type.get(&provider.concrete_ty.type_id).ok_or(
                ConditionError::MissingProviderComponent {
                    trait_type: (provider.trait_ty.type_name)(),
                    qualifier: provider.qualifier,
                },
            )?;
            let id = provider.mapping_id(component);

            if !provider_ids.insert(id) {
                return Err(ConditionError::DuplicateProviderMapping(id));
            }

            providers.push((id, *provider));
        }

        providers.sort_by_key(|(id, _)| *id);

        let provider_order = registry
            .provider_order(&components.values().copied().collect::<Vec<_>>())
            .map_err(ConditionError::Registry)?;
        let identity = super::CatalogIdentity::new(registry, &facts)?;
        let catalog = Self {
            identity,
            registry_order,
            component_order: components.keys().copied().collect(),
            components,
            providers,
            facts,
            provider_order,
        };

        catalog.validate_conditions()?;
        catalog.validate_acyclic()?;

        Ok(catalog)
    }

    pub(super) fn validate_snapshot(
        &self,
        snapshot: &ConditionFactSnapshot,
    ) -> Result<(), ConditionError> {
        for (id, descriptor) in &self.facts {
            let value = snapshot
                .facts
                .get(id)
                .ok_or(ConditionError::MissingFactValue(*id))?;

            if value.kind() != descriptor.kind {
                return Err(ConditionError::FactKindMismatch {
                    fact: *id,
                    expected: descriptor.kind,
                    actual: value.kind(),
                });
            }
        }

        for id in snapshot.facts.keys() {
            if !self.facts.contains_key(id) {
                return Err(ConditionError::UnknownFactValue(*id));
            }
        }

        Ok(())
    }

    fn validate_conditions(&self) -> Result<(), ConditionError> {
        for (owner, component) in &self.components {
            let Some(root) = component.condition else {
                continue;
            };

            if component.scope.id() != Singleton::ID {
                return Err(ConditionError::UnsupportedScope {
                    component_id: owner,
                    scope: component.scope.id(),
                });
            }

            if component
                .effective_factory()
                .map_err(ConditionError::Registry)?
                .is_none()
            {
                return Err(ConditionError::ConditionalManualComponent(owner));
            }

            let mut condition_ids = BTreeSet::new();

            self.validate_node(owner, root, &mut condition_ids)?;
        }

        Ok(())
    }

    fn validate_node(
        &self,
        owner: &'static str,
        node: &'static ConditionDescriptor,
        condition_ids: &mut BTreeSet<&'static str>,
    ) -> Result<(), ConditionError> {
        if !condition_ids.insert(node.id) {
            return Err(ConditionError::DuplicateConditionId {
                component_id: owner,
                condition_id: node.id,
            });
        }

        match node.predicate {
            ConditionPredicate::ConfigBool(fact) => {
                self.require_fact(fact, ConditionScalarKind::Bool, owner, node.id)?;
            }
            ConditionPredicate::ConfigEquals { fact, expected } => {
                self.require_fact(fact, expected.kind(), owner, node.id)?;
            }
            ConditionPredicate::ComponentEligible(component) => {
                self.require_component(component, owner, node.id)?;
            }
            ConditionPredicate::ProviderMappingEligible(provider) => {
                self.require_provider(provider, owner, node.id)?;
            }
            ConditionPredicate::ConfigCallback(callback) => {
                self.require_callback_kind(callback.kind, owner, node.id)?;

                for fact in callback.inputs {
                    self.facts
                        .get(fact)
                        .ok_or(ConditionError::MissingFactReference {
                            component_id: owner,
                            condition_id: node.id,
                            fact: *fact,
                        })?;
                }
            }
            ConditionPredicate::AvailabilityCallback(callback) => {
                self.require_callback_kind(callback.kind, owner, node.id)?;

                for target in callback.inputs {
                    match target {
                        AvailabilityTarget::Component(component) => {
                            self.require_component(component, owner, node.id)?;
                        }
                        AvailabilityTarget::ProviderMapping(provider) => {
                            self.require_provider(*provider, owner, node.id)?;
                        }
                    }
                }
            }
            ConditionPredicate::All(children) | ConditionPredicate::Any(children) => {
                let mut children = children.iter().collect::<Vec<_>>();
                children.sort_by_key(|child| child.id);

                for child in children {
                    self.validate_node(owner, child, condition_ids)?;
                }
            }
            ConditionPredicate::Not(child) => {
                self.validate_node(owner, child, condition_ids)?;
            }
        }

        Ok(())
    }

    fn require_callback_kind(
        &self,
        kind: &'static str,
        component_id: &'static str,
        condition_id: &'static str,
    ) -> Result<(), ConditionError> {
        if kind.is_empty() {
            return Err(ConditionError::EmptyCallbackKind {
                component_id,
                condition_id,
            });
        }

        Ok(())
    }

    fn require_component(
        &self,
        referenced: &'static str,
        component_id: &'static str,
        condition_id: &'static str,
    ) -> Result<(), ConditionError> {
        if !self.components.contains_key(referenced) {
            return Err(ConditionError::MissingComponentReference {
                component_id,
                condition_id,
                referenced,
            });
        }

        Ok(())
    }

    fn require_provider(
        &self,
        provider: upwell_core::ProviderMappingId,
        component_id: &'static str,
        condition_id: &'static str,
    ) -> Result<(), ConditionError> {
        if !self.providers.iter().any(|(id, _)| *id == provider) {
            return Err(ConditionError::MissingProviderReference {
                component_id,
                condition_id,
                provider,
            });
        }

        Ok(())
    }

    fn require_fact(
        &self,
        fact: ConfigFactId,
        expected: ConditionScalarKind,
        component_id: &'static str,
        condition_id: &'static str,
    ) -> Result<(), ConditionError> {
        let descriptor = self
            .facts
            .get(&fact)
            .ok_or(ConditionError::MissingFactReference {
                component_id,
                condition_id,
                fact,
            })?;

        if descriptor.kind != expected {
            return Err(ConditionError::FactKindMismatch {
                fact,
                expected,
                actual: descriptor.kind,
            });
        }

        Ok(())
    }
}

fn validate_manual_overrides(components: &[ComponentDescriptor]) -> Result<(), ConditionError> {
    let mut by_type = HashMap::<_, Vec<_>>::new();

    for component in components {
        by_type
            .entry(component.ty.type_id)
            .or_default()
            .push(component);
    }

    for candidates in by_type.values() {
        let conditional = candidates
            .iter()
            .find(|component| component.condition.is_some());
        let manual = candidates.iter().find(|component| {
            component
                .effective_factory()
                .map(|factory| factory.is_none())
                .unwrap_or(false)
        });

        if candidates.len() > 1
            && let (Some(conditional), Some(_)) = (conditional, manual)
        {
            return Err(ConditionError::ConditionalManualOverride(conditional.id));
        }
    }

    Ok(())
}
