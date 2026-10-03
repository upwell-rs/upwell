use std::collections::{BTreeMap, BTreeSet};

use upwell_core::{
    AvailabilityConditionContext, AvailabilityTarget, ConditionDescriptor, ConditionPredicate,
    ConfigConditionContext, StaticScope,
};

use super::{
    ConditionCatalog, ConditionDecision, ConditionError, ConditionEvaluation,
    ConditionFactSnapshot, ValidatedConditionEvaluation,
};
use crate::{ComponentRegistry, ProviderSelectionModel, Singleton};

impl ConditionCatalog {
    pub fn evaluate(
        &self,
        snapshot: &ConditionFactSnapshot,
    ) -> Result<ConditionEvaluation, ConditionError> {
        self.validate_snapshot(snapshot)?;

        self.evaluate_from(snapshot, BTreeMap::new(), Vec::new())
    }

    /// Re-evaluates only conditions reachable from changed config facts.
    ///
    /// The returned evaluation is still complete and is suitable for ordinary graph
    /// validation. `previous` must have been produced by this catalog. Trusted callbacks
    /// must obey their descriptor contract: declared inputs are exhaustive and evaluation
    /// is deterministic, non-blocking, panic-free, and side-effect free.
    pub fn evaluate_changed(
        &self,
        previous: &ConditionEvaluation,
        snapshot: &ConditionFactSnapshot,
    ) -> Result<ConditionEvaluation, ConditionError> {
        self.validate_snapshot(snapshot)?;

        if previous.catalog != self.identity {
            return Err(ConditionError::EvaluationCatalogMismatch);
        }

        let changed = self
            .facts
            .keys()
            .filter(|id| previous.facts.facts.get(id) != snapshot.facts.get(id))
            .copied()
            .collect::<BTreeSet<_>>();

        if changed.is_empty() {
            return Ok(previous.clone());
        }

        let invalidated = self.invalidated_components(&changed)?;
        let states = previous
            .components
            .iter()
            .filter(|(component, _)| !invalidated.contains(**component))
            .map(|(component, state)| (*component, *state))
            .collect();
        let decisions = previous
            .decisions
            .iter()
            .filter(|decision| !invalidated.contains(decision.component_id))
            .cloned()
            .collect();

        self.evaluate_from(snapshot, states, decisions)
    }

    fn evaluate_from(
        &self,
        snapshot: &ConditionFactSnapshot,
        mut states: BTreeMap<&'static str, bool>,
        mut decisions: Vec<ConditionDecision>,
    ) -> Result<ConditionEvaluation, ConditionError> {
        for component in &self.component_order {
            self.evaluate_component(component, snapshot, &mut states, &mut decisions)?;
        }

        let providers = self
            .providers
            .iter()
            .map(|(id, _)| (*id, states[id.component]))
            .collect::<BTreeMap<_, _>>();
        let eligible = ComponentRegistry {
            components: self
                .registry_order
                .iter()
                .filter(|id| states[**id])
                .map(|id| self.components[id])
                .collect(),
            providers: self
                .providers
                .iter()
                .filter(|(id, _)| states[id.component])
                .map(|(_, provider)| *provider)
                .collect(),
        };
        decisions.sort_by_key(|decision| (decision.component_id, decision.condition_id));

        let evaluation = ConditionEvaluation {
            catalog: self.identity.clone(),
            facts: snapshot.clone(),
            eligible,
            components: states,
            providers,
            decisions,
        };

        crate::observability::condition_evaluation(&evaluation, &self.components);

        Ok(evaluation)
    }

    fn invalidated_components(
        &self,
        changed: &BTreeSet<upwell_core::ConfigFactId>,
    ) -> Result<BTreeSet<&'static str>, ConditionError> {
        let dependencies = self
            .component_order
            .iter()
            .map(|component| {
                self.dependencies(component)
                    .map(|dependencies| (*component, dependencies))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let mut invalidated = dependencies
            .iter()
            .filter(|(_, dependencies)| {
                dependencies.iter().any(|dependency| {
                    matches!(dependency, super::ConditionDependency::Config(fact) if changed.contains(fact))
                })
            })
            .map(|(component, _)| *component)
            .collect::<BTreeSet<_>>();

        loop {
            let before = invalidated.len();

            for (component, dependencies) in &dependencies {
                if dependencies.iter().any(|dependency| match dependency {
                    super::ConditionDependency::Component(target) => invalidated.contains(target),
                    super::ConditionDependency::Provider(target) => {
                        invalidated.contains(target.component)
                    }
                    super::ConditionDependency::Config(_) => false,
                }) {
                    invalidated.insert(*component);
                }
            }

            if invalidated.len() == before {
                return Ok(invalidated);
            }
        }
    }

    pub fn evaluate_validated(
        &self,
        snapshot: &ConditionFactSnapshot,
    ) -> Result<ValidatedConditionEvaluation, ConditionError> {
        let evaluation = self.evaluate(snapshot)?;
        let selection = ProviderSelectionModel::new(
            &evaluation.eligible.components,
            evaluation.eligible.providers.clone(),
            self.provider_order.clone(),
        )
        .map_err(ConditionError::Registry)?;

        if let Err(error) = evaluation.eligible_registry().validate_with_scope_access(
            &evaluation.eligible.components,
            &selection,
            |consumer, dependency| {
                if dependency.is_transient() {
                    return true;
                }

                if consumer.is_transient() {
                    return dependency.id() == Singleton::ID;
                }

                dependency.rank() >= consumer.rank()
            },
        ) {
            crate::observability::condition_validation_rejected(&evaluation);

            return Err(ConditionError::Registry(error));
        }

        crate::observability::condition_validation(&evaluation);

        Ok(ValidatedConditionEvaluation {
            evaluation,
            selection,
        })
    }

    fn evaluate_component(
        &self,
        component_id: &'static str,
        snapshot: &ConditionFactSnapshot,
        states: &mut BTreeMap<&'static str, bool>,
        decisions: &mut Vec<ConditionDecision>,
    ) -> Result<bool, ConditionError> {
        if let Some(value) = states.get(component_id) {
            return Ok(*value);
        }

        let component = self.components[component_id];
        let outcome = match component.condition {
            Some(root) => self.evaluate_node(component_id, root, snapshot, states, decisions)?,
            None => true,
        };

        states.insert(component_id, outcome);

        Ok(outcome)
    }

    fn evaluate_node(
        &self,
        owner: &'static str,
        node: &'static ConditionDescriptor,
        snapshot: &ConditionFactSnapshot,
        states: &mut BTreeMap<&'static str, bool>,
        decisions: &mut Vec<ConditionDecision>,
    ) -> Result<bool, ConditionError> {
        let outcome = match node.predicate {
            ConditionPredicate::ConfigBool(fact) => snapshot.facts[&fact]
                .as_bool()
                .expect("validated boolean fact"),
            ConditionPredicate::ConfigEquals { fact, expected } => {
                expected.matches(&snapshot.facts[&fact])
            }
            ConditionPredicate::ComponentEligible(component) => {
                self.evaluate_component(component, snapshot, states, decisions)?
            }
            ConditionPredicate::ProviderMappingEligible(provider) => {
                self.evaluate_component(provider.component, snapshot, states, decisions)?
            }
            ConditionPredicate::ConfigCallback(callback) => {
                let mut input_ids = callback.inputs.to_vec();
                input_ids.sort_unstable();
                input_ids.dedup();
                let inputs = input_ids
                    .iter()
                    .map(|id| (*id, &snapshot.facts[id]))
                    .collect::<Vec<_>>();

                (callback.evaluate)(ConfigConditionContext::new(&inputs))
            }
            ConditionPredicate::AvailabilityCallback(callback) => {
                let mut targets = callback.inputs.to_vec();
                targets.sort_unstable();
                targets.dedup();
                let mut inputs = Vec::with_capacity(targets.len());

                for target in targets {
                    let eligible = match target {
                        AvailabilityTarget::Component(component) => {
                            self.evaluate_component(component, snapshot, states, decisions)?
                        }
                        AvailabilityTarget::ProviderMapping(provider) => self.evaluate_component(
                            provider.component,
                            snapshot,
                            states,
                            decisions,
                        )?,
                    };

                    inputs.push((target, eligible));
                }

                (callback.evaluate)(AvailabilityConditionContext::new(&inputs))
            }
            ConditionPredicate::All(children) => {
                let mut outcomes = Vec::with_capacity(children.len());
                let mut children = children.iter().collect::<Vec<_>>();
                children.sort_by_key(|child| child.id);

                for child in children {
                    outcomes.push(self.evaluate_node(owner, child, snapshot, states, decisions)?);
                }

                outcomes.into_iter().all(|outcome| outcome)
            }
            ConditionPredicate::Any(children) => {
                let mut outcomes = Vec::with_capacity(children.len());
                let mut children = children.iter().collect::<Vec<_>>();
                children.sort_by_key(|child| child.id);

                for child in children {
                    outcomes.push(self.evaluate_node(owner, child, snapshot, states, decisions)?);
                }

                outcomes.into_iter().any(|outcome| outcome)
            }
            ConditionPredicate::Not(child) => {
                !self.evaluate_node(owner, child, snapshot, states, decisions)?
            }
        };

        decisions.push(ConditionDecision {
            component_id: owner,
            condition_id: node.id,
            predicate: node.predicate.kind(),
            callback_kind: match node.predicate {
                ConditionPredicate::ConfigCallback(callback) => Some(callback.kind),
                ConditionPredicate::AvailabilityCallback(callback) => Some(callback.kind),
                _ => None,
            },
            source: node.source,
            outcome,
        });

        Ok(outcome)
    }
}
