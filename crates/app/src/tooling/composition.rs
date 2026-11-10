use std::collections::BTreeMap;

use upwell_tooling_schema::{RelationshipKind, ResourceKind};

use super::{
    Projection, ToolingContributionError, ToolingProjectionError, contribution_id,
    contribution_provenance, contributor_id, installation_provenance, plugin_id,
};
use crate::{
    CompositionPhase, PluginContributionKind, ProtocolDefinition, RelationKind, RelationTarget,
    SlotPolicy,
};

impl<D: ProtocolDefinition> Projection<'_, D> {
    pub(super) fn project_plugins(&mut self) {
        let plan = self.app.plugin_plan();

        for replacement in plan.resolution().replacements() {
            self.inactive_plugin_resource(
                &plugin_id(replacement.replaced().as_str()),
                replacement.replaced().as_str(),
                replacement.replaced_provenance(),
                "replaced",
            );
        }

        for suppression in plan.resolution().suppressions() {
            let decision = format!("suppression:{}", suppression.slot().as_str());

            self.plugin_slot_resource(suppression.slot().as_str());
            self.inactive_plugin_resource(
                &plugin_id(suppression.suppressed().as_str()),
                suppression.suppressed().as_str(),
                suppression.suppressed_provenance(),
                "suppressed",
            );
            self.resource(
                &decision,
                ResourceKind::Contribution,
                suppression.slot().as_str(),
                Some(installation_provenance(suppression.provenance())),
                BTreeMap::from([(
                    String::from("slot"),
                    suppression.slot().as_str().to_string(),
                )]),
            );
            self.relationship(
                RelationshipKind::DependsOn,
                &decision,
                &plugin_slot_id(suppression.slot().as_str()),
                [(String::from("role"), String::from("suppressed-slot"))],
            );
        }

        for (order, plugin) in plan.resolution().plugins().iter().enumerate() {
            let id = plugin_id(plugin.id().as_str());
            let mut labels = BTreeMap::from([
                (
                    String::from("phase"),
                    phase_name(plugin.phase()).to_string(),
                ),
                (String::from("plugin-id"), plugin.id().as_str().to_string()),
                (String::from("resolution-order"), order.to_string()),
                (String::from("effective"), String::from("true")),
            ]);

            if let Some((slot, policy)) = plugin.slot() {
                self.plugin_slot_resource(slot.as_str());
                labels.insert(String::from("slot"), slot.as_str().to_string());
                labels.insert(
                    String::from("slot-policy"),
                    slot_policy_name(policy).to_string(),
                );
            }

            self.resource(
                &id,
                ResourceKind::Plugin,
                plugin.id().as_str(),
                Some(installation_provenance(plugin.provenance())),
                labels,
            );

            if let Some((slot, _)) = plugin.slot() {
                self.relationship(
                    RelationshipKind::Provides,
                    &id,
                    &plugin_slot_id(slot.as_str()),
                    [],
                );
            }
        }

        for plugin in plan.resolution().plugins() {
            for relation in plugin.relations() {
                match relation.target() {
                    RelationTarget::Plugin(plugin) => {
                        let id = plugin_id(plugin.as_str());

                        if !self
                            .document
                            .resources
                            .iter()
                            .any(|resource| resource.id == id)
                        {
                            self.resource(
                                &id,
                                ResourceKind::Plugin,
                                plugin.as_str(),
                                None,
                                BTreeMap::from([
                                    (String::from("effective"), String::from("false")),
                                    (String::from("decision"), String::from("relation-endpoint")),
                                ]),
                            );
                        }
                    }
                    RelationTarget::Slot(slot) => self.plugin_slot_resource(slot.as_str()),
                }
            }
        }

        for plugin in plan.resolution().plugins() {
            let id = plugin_id(plugin.id().as_str());

            for relation in plugin.relations() {
                let target = match relation.target() {
                    RelationTarget::Plugin(plugin) => plugin_id(plugin.as_str()),
                    RelationTarget::Slot(slot) => plugin_slot_id(slot.as_str()),
                };
                let kind = match relation.kind() {
                    RelationKind::Requires => RelationshipKind::DependsOn,
                    RelationKind::Before => RelationshipKind::OrdersBefore,
                    RelationKind::After => RelationshipKind::OrdersAfter,
                    RelationKind::Conflicts => RelationshipKind::Conflicts,
                };

                self.relationship(kind, &id, &target, []);
            }
        }

        for replacement in plan.resolution().replacements() {
            let replaced = plugin_id(replacement.replaced().as_str());

            self.relationship(
                RelationshipKind::Replaces,
                &plugin_id(replacement.replacement().as_str()),
                &replaced,
                [(
                    String::from("slot"),
                    replacement.slot().as_str().to_string(),
                )],
            );
        }

        for suppression in plan.resolution().suppressions() {
            let decision = format!("suppression:{}", suppression.slot().as_str());
            let suppressed = plugin_id(suppression.suppressed().as_str());

            self.relationship(RelationshipKind::Suppresses, &decision, &suppressed, []);
        }

        for (index, (contribution, reconciliation)) in plan.reconciled_contributions().enumerate() {
            let provenance = contribution.provenance();
            let id = contribution_id(provenance);
            let contributor = contributor_id(provenance.contributor());
            let decision = reconciliation.decision.name();
            let mut labels = BTreeMap::from([
                (
                    String::from("contribution-kind"),
                    contribution_kind_name(contribution.kind()).to_string(),
                ),
                (String::from("decision"), decision.to_string()),
                (String::from("emission-index"), index.to_string()),
                (
                    String::from("requested-target"),
                    reconciliation.requested.clone(),
                ),
            ]);

            if let Some(target) = &reconciliation.applied {
                let label = if decision == "applied" {
                    "applied-target"
                } else {
                    "effective-target"
                };

                labels.insert(String::from(label), target.to_string());
            }

            self.resource(
                &id,
                ResourceKind::Contribution,
                provenance.contribution().as_str(),
                Some(contribution_provenance(provenance)),
                labels,
            );
            self.relationship(RelationshipKind::Contributes, &contributor, &id, []);

            if decision == "applied"
                && let Some(applied) = &reconciliation.applied
            {
                self.relationship(
                    RelationshipKind::Contributes,
                    &id,
                    applied,
                    [(String::from("decision"), decision.to_string())],
                );
            }
        }
    }

    #[cfg(feature = "cli")]
    pub(super) fn project_cli(&mut self) {
        self.document.cli = self.app.plugin_plan().cli_parser_metadata().cloned();

        let providers = self
            .document
            .cli
            .as_ref()
            .map(|cli| cli.providers.clone())
            .unwrap_or_default();

        for provider in providers {
            let id = upwell_tooling_schema::contribution_id(
                &provider.contributor,
                &provider.contribution,
            );

            if !self
                .document
                .resources
                .iter()
                .any(|resource| resource.id == id)
            {
                self.resource(
                    &id,
                    ResourceKind::Contribution,
                    &provider.contribution,
                    Some(upwell_tooling_schema::Provenance {
                        owner: Some(provider.contributor.clone()),
                        origin: Some(String::from("cli-provider")),
                        ..upwell_tooling_schema::Provenance::default()
                    }),
                    BTreeMap::from([
                        (String::from("provider-id"), provider.id),
                        (
                            String::from("provider-kind"),
                            cli_provider_kind_name(provider.kind).to_string(),
                        ),
                    ]),
                );
            }

            self.relationship(
                RelationshipKind::Contributes,
                &provider.contributor,
                &id,
                [],
            );
        }
    }

    #[cfg(not(feature = "cli"))]
    pub(super) fn project_cli(&mut self) {}

    pub(super) fn project_tooling_contributions(&mut self) -> Result<(), ToolingProjectionError> {
        self.merge_tooling_contributions(self.app.protocol_tooling().clone())?;

        for contributions in self.app.plugin_plan().tooling_contributions() {
            self.merge_tooling_contributions(contributions.clone())?;
        }

        Ok(())
    }

    fn merge_tooling_contributions(
        &mut self,
        contributions: crate::tooling::ToolingContributionSet,
    ) -> Result<(), ToolingProjectionError> {
        let owner = self
            .document
            .resources
            .iter_mut()
            .find(|resource| resource.id == contributions.owner)
            .ok_or_else(|| ToolingContributionError::UnknownOwner {
                id: contributions.owner.clone(),
            })?;

        owner.facets.extend(contributions.owner_facets);

        if contributions.owner.starts_with("protocol:") && contributions.owner_display.is_none() {
            return Err(ToolingContributionError::MissingProtocolDisplay {
                id: contributions.owner,
            }
            .into());
        }

        owner.display = contributions.owner_display;
        self.document.resources.extend(contributions.resources);

        for relationship in contributions.relationships {
            self.relationship_map(
                relationship.kind,
                &relationship.from,
                &relationship.to,
                relationship.labels,
            );
        }

        Ok(())
    }

    fn plugin_slot_resource(&mut self, slot: &str) {
        let id = plugin_slot_id(slot);

        if self
            .document
            .resources
            .iter()
            .any(|resource| resource.id == id)
        {
            return;
        }

        self.resource(
            &id,
            ResourceKind::PluginSlot,
            slot,
            None,
            BTreeMap::from([(String::from("slot-id"), slot.to_string())]),
        );
    }
}

fn plugin_slot_id(id: &str) -> String {
    format!("plugin-slot:{id}")
}

fn phase_name(phase: CompositionPhase) -> &'static str {
    match phase {
        CompositionPhase::Early => "early",
        CompositionPhase::Late => "late",
    }
}

fn slot_policy_name(policy: SlotPolicy) -> &'static str {
    match policy {
        SlotPolicy::Fixed => "fixed",
        SlotPolicy::Replaceable => "replaceable",
        SlotPolicy::Optional => "optional",
    }
}

fn contribution_kind_name(kind: PluginContributionKind) -> &'static str {
    match kind {
        PluginContributionKind::Component => "component",
        PluginContributionKind::Provider => "provider",
        PluginContributionKind::ConfigBinding => "config-binding",
        PluginContributionKind::Entry => "entry",
    }
}

#[cfg(feature = "cli")]
fn cli_provider_kind_name(kind: upwell_tooling_schema::CliProviderKind) -> &'static str {
    match kind {
        upwell_tooling_schema::CliProviderKind::Args => "args",
        upwell_tooling_schema::CliProviderKind::Command => "command",
        upwell_tooling_schema::CliProviderKind::CommandSet => "command-set",
        _ => "unknown",
    }
}
