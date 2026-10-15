use upwell_core::{ConditionScalarKind, ConfigFactId, ProviderMappingId};

use super::AvailabilityEdge;

#[derive(Debug, thiserror::Error)]
pub enum ConditionError {
    #[error(transparent)]
    Registry(crate::Error),
    #[error("duplicate component id: {0}")]
    DuplicateComponentId(&'static str),
    #[error("unknown component id: {0}")]
    UnknownComponentId(String),
    #[error("duplicate provider mapping id: {0}")]
    DuplicateProviderMapping(ProviderMappingId),
    #[error(
        "provider mapping for trait '{trait_type}' and qualifier '{qualifier}' has no component"
    )]
    MissingProviderComponent {
        trait_type: &'static str,
        qualifier: &'static str,
    },
    #[error("duplicate config fact descriptor: {0:?}")]
    DuplicateFactDescriptor(ConfigFactId),
    #[error("duplicate supplied config fact: {0:?}")]
    DuplicateFactValue(ConfigFactId),
    #[error("missing supplied config fact: {0:?}")]
    MissingFactValue(ConfigFactId),
    #[error("unknown supplied config fact: {0:?}")]
    UnknownFactValue(ConfigFactId),
    #[error("the previous condition evaluation belongs to another catalog")]
    EvaluationCatalogMismatch,
    #[error("config fact kind mismatch for {fact:?}: expected {expected:?}, found {actual:?}")]
    FactKindMismatch {
        fact: ConfigFactId,
        expected: ConditionScalarKind,
        actual: ConditionScalarKind,
    },
    #[error("duplicate condition id '{condition_id}' on component '{component_id}'")]
    DuplicateConditionId {
        component_id: &'static str,
        condition_id: &'static str,
    },
    #[error(
        "condition '{condition_id}' on '{component_id}' references missing config fact {fact:?}"
    )]
    MissingFactReference {
        component_id: &'static str,
        condition_id: &'static str,
        fact: ConfigFactId,
    },
    #[error(
        "condition '{condition_id}' on '{component_id}' references missing component '{referenced}'"
    )]
    MissingComponentReference {
        component_id: &'static str,
        condition_id: &'static str,
        referenced: &'static str,
    },
    #[error(
        "condition '{condition_id}' on '{component_id}' references missing provider mapping {provider}"
    )]
    MissingProviderReference {
        component_id: &'static str,
        condition_id: &'static str,
        provider: ProviderMappingId,
    },
    #[error("condition callback on '{component_id}' at '{condition_id}' has an empty kind")]
    EmptyCallbackKind {
        component_id: &'static str,
        condition_id: &'static str,
    },
    #[error("conditional component '{component_id}' uses unsupported scope '{scope}'")]
    UnsupportedScope {
        component_id: &'static str,
        scope: upwell_core::ScopeId,
    },
    #[error("factory-less component '{0}' cannot be conditional")]
    ConditionalManualComponent(&'static str),
    #[error("manual registration cannot override conditional component '{0}'")]
    ConditionalManualOverride(&'static str),
    #[error("condition availability cycle: {0:?}")]
    AvailabilityCycle(Vec<AvailabilityEdge>),
}
