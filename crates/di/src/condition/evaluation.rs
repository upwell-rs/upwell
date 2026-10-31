use std::collections::{BTreeMap, BTreeSet};

use upwell_core::{
    ConditionDescriptor, ConditionInputId, ConditionPredicate, ConditionPurity, Scope,
};

use super::{
    ConditionCatalog, ConditionDecision, ConditionDependency, ConditionError, ConditionEvaluation,
    ConditionFactSnapshot, ConditionOutcome, ValidatedConditionEvaluation,
};
use crate::{ComponentRegistry, ProviderSelectionModel, Singleton};

impl ConditionCatalog {
    /// The declared binding and raw-path inputs of every condition in this catalog.
    pub fn config_inputs(&self) -> &BTreeSet<ConditionInputId> {
        &self.config_inputs
    }

    pub fn evaluate(
        &self,
        snapshot: &ConditionFactSnapshot,
    ) -> Result<ConditionEvaluation, ConditionError> {
        self.validate_snapshot(snapshot)?;

        self.evaluate_from(
            snapshot,
            ConditionPurity::Trusted,
            BTreeMap::new(),
            Vec::new(),
        )
    }

    /// Evaluates every condition tooling may decide without running trusted code.
    ///
    /// Structured predicates and [`ConditionPurity::Pure`] callbacks are evaluated over
    /// `snapshot`; a trusted callback is never called, and it and every condition whose
    /// outcome depends on it are [`ConditionOutcome::NotEvaluated`]. Only components whose
    /// condition holds are eligible, so the result is the graph tooling can prove.
    pub fn evaluate_for_tooling(
        &self,
        snapshot: &ConditionFactSnapshot,
    ) -> Result<ConditionEvaluation, ConditionError> {
        self.validate_snapshot(snapshot)?;

        self.evaluate_from(snapshot, ConditionPurity::Pure, BTreeMap::new(), Vec::new())
    }

    /// Re-evaluates only conditions reachable from changed config inputs: scalar facts,
    /// typed bindings (by shared value identity), and raw config paths (by rendered value).
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

        if !previous.is_complete() {
            return Err(ConditionError::IncompleteEvaluation);
        }

        let mut changed = self
            .facts
            .keys()
            .filter(|id| previous.facts.facts.get(id) != snapshot.facts.get(id))
            .map(|id| ConditionDependency::Config(*id))
            .collect::<BTreeSet<_>>();

        changed.extend(
            snapshot
                .changed_config(&previous.facts, &self.config_inputs)
                .into_iter()
                .map(ConditionDependency::from),
        );

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

        self.evaluate_from(snapshot, ConditionPurity::Trusted, states, decisions)
    }

    /// Evaluates every component not already in `states`, running callbacks up to
    /// `runnable`: [`ConditionPurity::Trusted`] runs every callback, while
    /// [`ConditionPurity::Pure`] leaves trusted ones not evaluated.
    fn evaluate_from(
        &self,
        snapshot: &ConditionFactSnapshot,
        runnable: ConditionPurity,
        mut states: BTreeMap<&'static str, ConditionOutcome>,
        mut decisions: Vec<ConditionDecision>,
    ) -> Result<ConditionEvaluation, ConditionError> {
        let mut evaluation = Evaluation {
            snapshot,
            runnable,
            states: &mut states,
            decisions: &mut decisions,
        };

        for component in &self.component_order {
            self.evaluate_component(component, &mut evaluation)?;
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
                .filter(|id| states[**id] == ConditionOutcome::Holds)
                .map(|id| self.components[id])
                .collect(),
            providers: self
                .providers
                .iter()
                .filter(|(id, _)| states[id.component] == ConditionOutcome::Holds)
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
        changed: &BTreeSet<ConditionDependency>,
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
                dependencies
                    .iter()
                    .any(|dependency| changed.contains(dependency))
            })
            .map(|(component, _)| *component)
            .collect::<BTreeSet<_>>();

        loop {
            let before = invalidated.len();

            for (component, dependencies) in &dependencies {
                if dependencies.iter().any(|dependency| match dependency {
                    ConditionDependency::Component(target) => invalidated.contains(target),
                    ConditionDependency::Provider(target) => invalidated.contains(target.component),
                    ConditionDependency::Config(_)
                    | ConditionDependency::Binding(_)
                    | ConditionDependency::Raw(_) => false,
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

    pub(super) fn evaluate_component(
        &self,
        component_id: &'static str,
        evaluation: &mut Evaluation<'_>,
    ) -> Result<ConditionOutcome, ConditionError> {
        if let Some(outcome) = evaluation.states.get(component_id) {
            return Ok(*outcome);
        }

        let component = self.components[component_id];
        let outcome = match component.condition {
            Some(root) => self.evaluate_node(component_id, root, evaluation)?,
            None => ConditionOutcome::Holds,
        };

        evaluation.states.insert(component_id, outcome);

        Ok(outcome)
    }

    fn evaluate_node(
        &self,
        owner: &'static str,
        node: &'static ConditionDescriptor,
        evaluation: &mut Evaluation<'_>,
    ) -> Result<ConditionOutcome, ConditionError> {
        let outcome = match node.predicate {
            ConditionPredicate::ConfigBool(fact) => ConditionOutcome::from_bool(
                evaluation.snapshot.facts[&fact]
                    .as_bool()
                    .expect("validated boolean fact"),
            ),
            ConditionPredicate::ConfigEquals { fact, expected } => {
                ConditionOutcome::from_bool(expected.matches(&evaluation.snapshot.facts[&fact]))
            }
            ConditionPredicate::ComponentEligible(component) => {
                self.evaluate_component(component, evaluation)?
            }
            ConditionPredicate::ProviderMappingEligible(provider) => {
                self.evaluate_component(provider.component, evaluation)?
            }
            ConditionPredicate::Callback(callback) => {
                self.evaluate_callback(owner, node.id, callback, evaluation)?
            }
            ConditionPredicate::All(children) => self
                .evaluate_children(owner, children, evaluation)?
                .into_iter()
                .fold(ConditionOutcome::Holds, ConditionOutcome::and),
            ConditionPredicate::Any(children) => self
                .evaluate_children(owner, children, evaluation)?
                .into_iter()
                .fold(ConditionOutcome::Fails, ConditionOutcome::or),
            ConditionPredicate::Not(child) => self.evaluate_node(owner, child, evaluation)?.not(),
        };

        evaluation.decisions.push(ConditionDecision {
            component_id: owner,
            condition_id: node.id,
            predicate: node.predicate.kind(),
            callback_kind: match node.predicate {
                ConditionPredicate::Callback(callback) => Some(callback.kind),
                _ => None,
            },
            source: node.source,
            outcome,
        });

        Ok(outcome)
    }

    /// Evaluates every child in id order, without short-circuiting, so each records its
    /// decision.
    fn evaluate_children(
        &self,
        owner: &'static str,
        children: &'static [ConditionDescriptor],
        evaluation: &mut Evaluation<'_>,
    ) -> Result<Vec<ConditionOutcome>, ConditionError> {
        let mut children = children.iter().collect::<Vec<_>>();

        children.sort_by_key(|child| child.id);

        children
            .into_iter()
            .map(|child| self.evaluate_node(owner, child, evaluation))
            .collect()
    }
}

/// The mutable state of one evaluation pass.
pub(super) struct Evaluation<'a> {
    pub(super) snapshot: &'a ConditionFactSnapshot,
    pub(super) runnable: ConditionPurity,
    pub(super) states: &'a mut BTreeMap<&'static str, ConditionOutcome>,
    pub(super) decisions: &'a mut Vec<ConditionDecision>,
}
