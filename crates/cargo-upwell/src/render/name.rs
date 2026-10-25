use upwell_tooling_schema::{
    CliOwner, CliProviderKind, DiagnosticSeverity, RelationshipKind, ResourceKind,
};

use crate::cli::InspectCliProviderKind;

pub(crate) fn resource_kind_name(kind: &ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Unknown => "unknown",
        ResourceKind::Application => "application",
        ResourceKind::Protocol => "protocol",
        ResourceKind::Plugin => "plugin",
        ResourceKind::Component => "component",
        ResourceKind::Provider => "provider",
        ResourceKind::ConfigBinding => "config-binding",
        ResourceKind::Hook => "hook",
        ResourceKind::Lifecycle => "lifecycle",
        ResourceKind::Scope => "scope",
        ResourceKind::Type => "type",
        ResourceKind::Contribution => "contribution",
        ResourceKind::Contributor => "contributor",
        ResourceKind::PluginSlot => "plugin-slot",
        ResourceKind::Condition => "condition",
        ResourceKind::Entry => "entry",
        _ => "resource",
    }
}

pub(crate) fn relationship_kind_name(kind: &RelationshipKind) -> &'static str {
    match kind {
        RelationshipKind::DependsOn => "depends-on",
        RelationshipKind::Provides => "provides",
        RelationshipKind::Binds => "binds",
        RelationshipKind::Hooks => "hooks",
        RelationshipKind::OpensScope => "opens-scope",
        RelationshipKind::Contains => "contains",
        RelationshipKind::Contributes => "contributes",
        RelationshipKind::Replaces => "replaces",
        RelationshipKind::Suppresses => "suppresses",
        RelationshipKind::OrdersBefore => "orders-before",
        RelationshipKind::OrdersAfter => "orders-after",
        RelationshipKind::Validates => "validates",
        RelationshipKind::Conflicts => "conflicts",
        RelationshipKind::Gates => "gates",
        RelationshipKind::Enters => "enters",
        _ => "relates-to",
    }
}

pub(crate) fn cli_provider_kind_matches(
    kind: CliProviderKind,
    filter: InspectCliProviderKind,
) -> bool {
    matches!(
        (kind, filter),
        (CliProviderKind::Args, InspectCliProviderKind::Args)
            | (CliProviderKind::Command, InspectCliProviderKind::Command)
            | (
                CliProviderKind::CommandSet,
                InspectCliProviderKind::CommandSet
            )
    )
}

pub(crate) fn cli_provider_kind_name(kind: CliProviderKind) -> &'static str {
    match kind {
        CliProviderKind::Args => "args",
        CliProviderKind::Command => "command",
        CliProviderKind::CommandSet => "command-set",
        _ => "provider",
    }
}

pub(crate) fn cli_owner_name(owner: &CliOwner) -> &str {
    match owner {
        CliOwner::Framework => "framework",
        CliOwner::Application => "application",
        CliOwner::Plugin { provider } => provider,
    }
}

pub(crate) fn diagnostic_severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Error => "error",
        _ => "diagnostic",
    }
}
