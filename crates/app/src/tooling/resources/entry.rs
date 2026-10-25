use std::collections::BTreeMap;

use upwell_tooling_schema::{RelationshipKind, ResourceKind};

use super::super::{
    Projection, contribution_provenance, entry_id, item_location, scope_id, with_source,
};
use crate::ProtocolDefinition;

impl<D: ProtocolDefinition> Projection<'_, D> {
    /// Projects every prepared entry with the scope it enters and its injected targets.
    pub(in crate::tooling) fn project_entries(&mut self) {
        let entries = self.app.tooling_snapshot().entries().to_vec();

        for entry in &entries {
            let descriptor = entry.descriptor;
            let id = entry_id(descriptor.key);
            let provenance = self
                .app
                .plugin_plan()
                .selected_contribution(&id)
                .map(contribution_provenance);
            let mut labels = BTreeMap::from([
                (
                    String::from("entry-id"),
                    descriptor.key.as_str().to_string(),
                ),
                (
                    String::from("scope-id"),
                    descriptor.scope.id().as_str().to_string(),
                ),
            ]);

            if let Some(owner) = descriptor.owner {
                labels.insert(
                    String::from("owner-type"),
                    (owner.ty.type_name)().to_string(),
                );
                labels.insert(String::from("operation"), owner.operation.to_string());
            }

            self.resource(
                &id,
                ResourceKind::Entry,
                descriptor.key.as_str(),
                with_source(
                    provenance,
                    descriptor.source.as_ref().and_then(item_location),
                ),
                labels,
            );
            self.relationship(
                RelationshipKind::Enters,
                &id,
                &scope_id(descriptor.scope.id().as_str()),
                [],
            );

            for target in &entry.targets {
                self.project_dependency(&id, target);
            }
        }
    }
}
