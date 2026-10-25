use std::collections::{BTreeMap, BTreeSet, HashMap};

use upwell_tooling_schema::{
    DocumentIdentity, Provenance, RelationshipKind, Resource, ResourceKind, ToolingDocument,
};

use super::{ToolingProjectionError, installation_provenance, type_id, type_location, with_source};
use crate::{InstallationProvenance, PreparedApp, ProtocolDefinition};

impl<D: ProtocolDefinition> PreparedApp<D> {
    /// Projects the exact immutable prepared plan without constructing runtime state.
    pub fn tooling_document(&self) -> Result<ToolingDocument, ToolingProjectionError> {
        let mut projection = Projection::new(self);

        projection.project()?;

        Ok(projection.document)
    }
}

pub(super) struct Projection<'a, D: ProtocolDefinition> {
    pub(super) app: &'a PreparedApp<D>,
    pub(super) document: ToolingDocument,
    pub(super) type_resources: BTreeSet<String>,
    pub(super) relationship_indexes: HashMap<(RelationshipKind, String, String), usize>,
}

impl<'a, D: ProtocolDefinition> Projection<'a, D> {
    pub(super) fn new(app: &'a PreparedApp<D>) -> Self {
        let identity = DocumentIdentity {
            application: app.name().to_string(),
            ..DocumentIdentity::default()
        };

        Self {
            app,
            document: ToolingDocument::new(
                env!("CARGO_PKG_VERSION"),
                identity,
                app.protocol_id().as_str(),
            ),
            type_resources: BTreeSet::new(),
            relationship_indexes: HashMap::new(),
        }
    }

    pub(super) fn project(&mut self) -> Result<(), ToolingProjectionError> {
        self.project_application();
        self.project_scopes();
        self.project_components();
        self.project_providers();
        self.project_config_bindings();
        self.project_hooks();
        self.project_entries();
        self.project_conditions();
        self.project_plugins();
        self.project_cli();
        self.project_tooling_contributions()?;

        Ok(())
    }

    pub(super) fn inactive_plugin_resource(
        &mut self,
        id: &str,
        name: &str,
        provenance: InstallationProvenance,
        decision: &str,
    ) {
        if self
            .document
            .resources
            .iter()
            .any(|resource| resource.id == id)
        {
            return;
        }

        self.resource(
            id,
            ResourceKind::Plugin,
            name,
            Some(installation_provenance(provenance)),
            BTreeMap::from([
                (String::from("effective"), String::from("false")),
                (String::from("decision"), decision.to_string()),
            ]),
        );
    }

    pub(super) fn resource(
        &mut self,
        id: &str,
        kind: ResourceKind,
        name: &str,
        provenance: Option<Provenance>,
        labels: BTreeMap<String, String>,
    ) {
        self.document.resources.push(Resource {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            display: None,
            provenance,
            labels,
            facets: BTreeMap::new(),
        });
    }

    /// The type's graph endpoint, declared on first use. A later descriptor that recorded the
    /// type's declaration backfills the source of an endpoint first seen only by reference.
    pub(super) fn type_resource(&mut self, ty: &upwell_core::TypeDescriptor) -> String {
        let rust_type = (ty.type_name)();
        let id = type_id(rust_type);
        let source = type_location(ty);

        if self.type_resources.insert(id.clone()) {
            self.resource(
                &id,
                ResourceKind::Type,
                ty.name,
                with_source(None, source),
                BTreeMap::from([(String::from("rust-type"), rust_type.to_string())]),
            );
        } else if source.is_some() {
            let resource = self
                .document
                .resources
                .iter_mut()
                .find(|resource| resource.id == id)
                .expect("declared type resource exists");

            if resource.provenance.is_none() {
                resource.provenance = with_source(None, source);
            }
        }

        id
    }
}
