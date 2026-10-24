//! `serialize_with` adapters for the foreign core and config types a report embeds.

use std::time::Duration;

use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};
use upwell_config::{ChangedBinding, ComponentHookReport, HookOutcome};
use upwell_core::RuntimeGenerationId;
use upwell_hooks::HookRole;

pub(super) fn generation<S: Serializer>(
    generation: &RuntimeGenerationId,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(generation.get())
}

pub(super) fn duration_micros<S: Serializer>(
    duration: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(duration.as_micros()).unwrap_or(u64::MAX))
}

pub(super) fn changed<S: Serializer>(
    changed: &[ChangedBinding],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut seq = serializer.serialize_seq(Some(changed.len()))?;

    for binding in changed {
        seq.serialize_element(&ChangedBindingRepr(binding))?;
    }

    seq.end()
}

pub(super) fn role<S: Serializer>(role: &HookRole, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match role {
        HookRole::Participant => "participant",
        HookRole::Observer => "observer",
    })
}

pub(super) fn hooks<S: Serializer>(
    hooks: &[ComponentHookReport],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut seq = serializer.serialize_seq(Some(hooks.len()))?;

    for hook in hooks {
        seq.serialize_element(&HookReportRepr(hook))?;
    }

    seq.end()
}

/// Serializes a changed binding as its path and type name.
struct ChangedBindingRepr<'a>(&'a ChangedBinding);

impl Serialize for ChangedBindingRepr<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut binding = serializer.serialize_struct("ChangedBinding", 2)?;

        binding.serialize_field("path", &self.0.path)?;
        binding.serialize_field("type_name", self.0.type_name)?;

        binding.end()
    }
}

/// Serializes a hook report as its component and snake-case outcome.
struct HookReportRepr<'a>(&'a ComponentHookReport);

impl Serialize for HookReportRepr<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (outcome, restart_reason) = match self.0.outcome {
            HookOutcome::Unchanged => ("unchanged", None),
            HookOutcome::Reloaded => ("reloaded", None),
            HookOutcome::RestartRequired(reason) => ("restart_required", Some(reason)),
        };
        let mut hook = serializer.serialize_struct("ComponentHookReport", 3)?;

        hook.serialize_field("component", self.0.component)?;
        hook.serialize_field("outcome", outcome)?;
        hook.serialize_field("restart_reason", &restart_reason)?;

        hook.end()
    }
}
