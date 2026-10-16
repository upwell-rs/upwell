//! Condition evaluation over an application's registered components and config.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use upwell_config::ConfigError;
use upwell_config::condition::ResolvedConfig;
use upwell_core::{ConditionInputId, ConfigFactDescriptor};
use upwell_di::{ConditionCatalog, ConditionEvaluation, ConditionFactSnapshot};

use super::AppRegistry;
use crate::error::Error;

/// A condition evaluation bound to one application's DI and config catalogs.
#[derive(Clone, Debug)]
pub struct AppConditionEvaluation {
    evaluation: ConditionEvaluation,
    bindings: HashMap<(TypeId, String), usize>,
}

impl AppConditionEvaluation {
    pub fn evaluation(&self) -> &ConditionEvaluation {
        &self.evaluation
    }

    pub(crate) fn belongs_to(&self, registry: &AppRegistry) -> bool {
        self.bindings == registry.condition_identity()
    }
}

/// One staged config binding: its type, path, and shared staged value.
pub type StagedBinding<'a> = (TypeId, &'a str, &'a Arc<dyn Any + Send + Sync>);

impl AppRegistry {
    /// Evaluates static eligibility from an already validated typed fact snapshot.
    ///
    /// This does not load configuration or construct ordinary components. The returned registry
    /// contains eligible declarations and still requires ordinary scope-aware graph validation.
    pub fn evaluate_conditions(
        &self,
        facts: impl IntoIterator<Item = ConfigFactDescriptor>,
        snapshot: &ConditionFactSnapshot,
    ) -> Result<AppConditionEvaluation, upwell_di::ConditionError> {
        let evaluation =
            ConditionCatalog::new(&self.component_registry(), facts)?.evaluate(snapshot)?;

        Ok(AppConditionEvaluation {
            evaluation,
            bindings: self.condition_identity(),
        })
    }

    /// Builds the condition snapshot of a staged config: the fact scalars of every
    /// registered fact source, the staged value of every binding a condition callback
    /// reads, and the resolved subtree of every raw path a callback reads.
    ///
    /// Each staged binding is `(TypeId, path, shared value)`. The transactional reload
    /// stages every binding, changed or not, so registered sources always resolve; a binding
    /// a callback declares but no config binds stays absent and fails evaluation with a
    /// missing-input error. `resolve_raw` renders one raw path of the same candidate config.
    pub fn condition_snapshot<'a>(
        &self,
        staged: impl IntoIterator<Item = StagedBinding<'a>>,
        mut resolve_raw: impl FnMut(&'static str) -> Result<Option<ResolvedConfig>, ConfigError>,
    ) -> crate::Result<ConditionFactSnapshot> {
        let staged = staged.into_iter().collect::<Vec<_>>();
        let mut scalars = Vec::new();

        for source in &self.condition_facts {
            for (type_id, path, value) in &staged {
                if *type_id == source.ty.type_id && *path == source.path {
                    scalars.extend((source.facts.scalars)(source.path, value.as_ref()));
                }
            }
        }

        let mut snapshot = ConditionFactSnapshot::new(scalars)?;

        for input in self.component_registry().condition_config_inputs() {
            match input {
                ConditionInputId::Binding(binding) => {
                    let value = staged.iter().find(|(type_id, path, _)| {
                        *type_id == binding.type_id && *path == binding.path
                    });

                    if let Some((_, _, value)) = value {
                        snapshot.insert_binding(binding, Arc::clone(value))?;
                    }
                }

                ConditionInputId::Raw(path) => snapshot.insert_raw(path, resolve_raw(path)?)?,

                ConditionInputId::Fact(_)
                | ConditionInputId::Component(_)
                | ConditionInputId::ProviderMapping(_) => {}
            }
        }

        Ok(snapshot)
    }

    /// Whether any condition callback reads a raw config path. Such a condition can change
    /// with the config tree even when no typed binding does.
    pub fn reads_raw_config(&self) -> bool {
        self.component_registry()
            .condition_config_inputs()
            .iter()
            .any(|input| matches!(input, ConditionInputId::Raw(_)))
    }

    /// Incrementally re-evaluates conditions while preserving this application's catalog identity.
    pub fn evaluate_changed_conditions(
        &self,
        facts: impl IntoIterator<Item = ConfigFactDescriptor>,
        previous: &AppConditionEvaluation,
        snapshot: &ConditionFactSnapshot,
    ) -> crate::Result<AppConditionEvaluation> {
        let bindings = self.condition_identity();

        if previous.bindings != bindings {
            return Err(Error::ConditionEvaluationApplicationMismatch);
        }

        let evaluation = ConditionCatalog::new(&self.component_registry(), facts)?
            .evaluate_changed(&previous.evaluation, snapshot)?;

        Ok(AppConditionEvaluation {
            evaluation,
            bindings,
        })
    }

    pub(super) fn condition_identity(&self) -> HashMap<(TypeId, String), usize> {
        let mut identity = HashMap::new();

        for binding in &self.config_bindings {
            *identity
                .entry((binding.ty.type_id, binding.path.clone()))
                .or_default() += 1;
        }

        identity
    }
}
