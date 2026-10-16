//! Evaluation of trusted condition callbacks over their declared inputs.

use std::collections::BTreeMap;

use upwell_core::condition::scope;
use upwell_core::{ConditionCallback, ConditionContext, ConditionInputId, ConditionInputValue};

use super::{ConditionCatalog, ConditionDecision, ConditionError, ConditionFactSnapshot};

impl ConditionCatalog {
    /// Evaluates the eligibility inputs first, then runs the callback with exactly its declared
    /// inputs and its raw paths installed as the thread's `resolve` scope.
    ///
    /// A rejected raw read fails the evaluation even when the callback still returned a value,
    /// so an undeclared or invalid input can never silently select a branch.
    pub(super) fn evaluate_callback(
        &self,
        owner: &'static str,
        condition_id: &'static str,
        callback: &'static ConditionCallback,
        snapshot: &ConditionFactSnapshot,
        states: &mut BTreeMap<&'static str, bool>,
        decisions: &mut Vec<ConditionDecision>,
    ) -> Result<bool, ConditionError> {
        let mut ids = callback.inputs.to_vec();
        let mut eligibility = BTreeMap::new();

        ids.sort_unstable();
        ids.dedup();

        for id in &ids {
            let target = match id {
                ConditionInputId::Component(component) => *component,
                ConditionInputId::ProviderMapping(provider) => provider.component,
                ConditionInputId::Fact(_)
                | ConditionInputId::Binding(_)
                | ConditionInputId::Raw(_) => {
                    continue;
                }
            };

            eligibility.insert(
                *id,
                self.evaluate_component(target, snapshot, states, decisions)?,
            );
        }

        let values = ids
            .iter()
            .map(|id| (*id, input_value(*id, snapshot, &eligibility)))
            .collect::<Vec<_>>();
        let raw = ids
            .iter()
            .filter_map(|id| match id {
                ConditionInputId::Raw(path) => Some((*path, snapshot.config[id].raw())),
                _ => None,
            })
            .collect();

        let (outcome, rejection) =
            scope::with_raw_inputs(raw, || (callback.evaluate)(ConditionContext::new(&values)));

        match rejection {
            Some(rejection) => Err(ConditionError::CallbackInputRejected {
                component_id: owner,
                condition_id,
                rejection,
            }),

            None => Ok(outcome),
        }
    }
}

/// The value of one validated declared input.
fn input_value<'a>(
    id: ConditionInputId,
    snapshot: &'a ConditionFactSnapshot,
    eligibility: &BTreeMap<ConditionInputId, bool>,
) -> ConditionInputValue<'a> {
    match id {
        ConditionInputId::Fact(fact) => ConditionInputValue::Fact(&snapshot.facts[&fact]),
        ConditionInputId::Binding(_) | ConditionInputId::Raw(_) => {
            ConditionInputValue::Config(snapshot.config[&id].value())
        }
        ConditionInputId::Component(_) | ConditionInputId::ProviderMapping(_) => {
            ConditionInputValue::Eligible(eligibility[&id])
        }
    }
}
