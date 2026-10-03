use std::collections::BTreeMap;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, Singleton,
    StaticScope, Transient,
};
use upwell_di::{DependencyTarget, ProviderDescriptor};
use upwell_tooling_schema::{RelationshipKind, ResourceKind};

use super::super::snapshot::{ComponentSnapshot, ConstructionPlanEntry, DependencySnapshot};
use super::super::{
    Projection, component_id, config_binding_id, contribution_provenance, provider_id, scope_id,
};
use crate::ProtocolDefinition;

impl<D: ProtocolDefinition> Projection<'_, D> {
    pub(crate) fn project_components(&mut self) {
        let components = self.app.tooling_snapshot().components().to_vec();

        for component in &components {
            self.project_component(component);
        }

        self.project_construction_plan(
            Singleton::ID.as_str(),
            self.app.tooling_snapshot().root_plan().to_vec(),
        );

        for boundary in self.app.scope_topology().boundaries() {
            let Some(order) = self.app.tooling_snapshot().scope_plan(&boundary.id()) else {
                continue;
            };

            self.project_construction_plan(boundary.id().as_str(), order.to_vec());
        }
    }

    pub(crate) fn project_providers(&mut self) {
        let providers = self.app.registry().providers.clone();

        for provider in &providers {
            self.project_provider(provider);
        }

        let mut traits = providers
            .iter()
            .map(|provider| provider.trait_ty.type_id)
            .collect::<Vec<_>>();

        traits.sort();
        traits.dedup();

        for trait_ty in traits {
            let mut ordered = providers
                .iter()
                .filter(|provider| provider.trait_ty.type_id == trait_ty)
                .filter_map(|provider| {
                    self.app
                        .provider_order(trait_ty, provider.concrete_ty.type_id)
                        .map(|ordinal| (ordinal, provider))
                })
                .collect::<Vec<_>>();

            ordered.sort_by_key(|(ordinal, _)| *ordinal);

            for pair in ordered.windows(2) {
                let [(_, before), (_, after)] = pair else {
                    continue;
                };
                let trait_resource =
                    self.type_resource((before.trait_ty.type_name)(), before.trait_ty.name);

                self.relationship(
                    RelationshipKind::OrdersBefore,
                    &provider_id(before),
                    &provider_id(after),
                    [
                        (String::from("trait-target"), trait_resource),
                        (
                            String::from("rust-trait"),
                            (before.trait_ty.type_name)().to_string(),
                        ),
                    ],
                );
            }
        }
    }

    fn project_construction_plan(&mut self, scope: &str, order: Vec<ConstructionPlanEntry>) {
        let mut previous: Option<String> = None;

        for (ordinal, entry) in order.into_iter().enumerate() {
            let id = component_id(entry.descriptor.id);
            let resource = self
                .document
                .resources
                .iter_mut()
                .find(|resource| resource.id == id)
                .expect("retained construction component is projected");

            resource
                .labels
                .insert(String::from("construction-plan"), scope.to_string());
            resource
                .labels
                .insert(String::from("plan-ordinal"), ordinal.to_string());
            resource.labels.insert(
                String::from("plan-entry-kind"),
                if entry.has_factory {
                    String::from("factory-construction")
                } else {
                    String::from("seeded-verification")
                },
            );

            if let Some(previous) = previous {
                self.relationship(
                    RelationshipKind::OrdersBefore,
                    &previous,
                    &id,
                    [(String::from("scope"), scope.to_string())],
                );
            }

            previous = Some(id);
        }
    }

    fn project_component(&mut self, component: &ComponentSnapshot) {
        let descriptor = component.descriptor;
        let id = component_id(descriptor.id);
        let ty = self.type_resource((descriptor.ty.type_name)(), descriptor.ty.name);
        let scope = scope_id(descriptor.scope.id().as_str());
        let construction = if descriptor.scope.id() == Transient::ID {
            if component.has_factory {
                "transient-on-demand"
            } else {
                "manual-transient"
            }
        } else if component.seed_destination.is_some() {
            "scope-seed"
        } else if component.has_factory {
            "planned-factory"
        } else if component.seeded {
            "seeded"
        } else {
            "manual-unseeded"
        };
        let labels = BTreeMap::from([
            (String::from("component-id"), descriptor.id.to_string()),
            (
                String::from("rust-type"),
                (descriptor.ty.type_name)().to_string(),
            ),
            (
                String::from("scope-id"),
                descriptor.scope.id().as_str().to_string(),
            ),
            (String::from("construction"), construction.to_string()),
            (
                String::from("factory-selection"),
                component.factory_selection.name().to_string(),
            ),
            (
                String::from("factory-candidate-count"),
                component.factory_candidate_count.to_string(),
            ),
            (
                String::from("factory-explicit-count"),
                component.factory_explicit_count.to_string(),
            ),
        ]);
        let provenance = self
            .app
            .plugin_plan()
            .selected_contribution(&id)
            .map(contribution_provenance);

        self.resource(
            &id,
            ResourceKind::Component,
            descriptor.name,
            provenance,
            labels,
        );
        self.relationship(RelationshipKind::Provides, &id, &ty, []);
        self.relationship(
            RelationshipKind::DependsOn,
            &id,
            &scope,
            [(String::from("role"), String::from("scope"))],
        );

        for dependency in &component.dependencies {
            self.project_dependency(&id, dependency);
        }
    }

    pub(super) fn project_dependency(&mut self, owner: &str, dependency: &DependencySnapshot) {
        let descriptor = &dependency.descriptor;
        let target = self.type_resource((descriptor.ty.type_name)(), descriptor.ty.name);
        let mut labels = dependency_labels(descriptor);

        if descriptor.config {
            let bindings = self
                .app
                .registry()
                .config_bindings
                .iter()
                .filter(|binding| binding.ty.type_id == descriptor.ty.type_id)
                .filter(|binding| descriptor.qualifier.is_none_or(|path| binding.path == path))
                .collect::<Vec<_>>();

            if let [binding] = bindings.as_slice() {
                self.relationship_map(
                    RelationshipKind::DependsOn,
                    owner,
                    &config_binding_id((binding.ty.type_name)(), &binding.path),
                    labels,
                );

                return;
            }

            labels.insert(
                String::from("binding-resolution"),
                if bindings.is_empty() {
                    String::from("missing")
                } else {
                    String::from("ambiguous")
                },
            );
            labels.insert(
                String::from("binding-cardinality"),
                bindings.len().to_string(),
            );
        }

        self.relationship_map(RelationshipKind::DependsOn, owner, &target, labels);

        for selected in &dependency.selected {
            let (target, role) = match selected.target {
                DependencyTarget::Component(component) => {
                    (component_id(component.id), "resolved-component")
                }
                DependencyTarget::Provider(provider) => {
                    (provider_id(&provider), "resolved-provider")
                }
            };
            let mut labels = dependency_labels(descriptor);

            labels.insert(String::from("role"), role.to_string());
            labels.insert(
                String::from("requested-type"),
                (descriptor.ty.type_name)().to_string(),
            );
            labels.insert(
                String::from("selection-reason"),
                selected.reason.as_str().to_string(),
            );

            if let Some(scope) = selected.scope {
                labels.insert(String::from("selected-scope"), scope.to_string());
            }

            if let Some(stage) = selected.stage {
                labels.insert(String::from("selection-stage"), stage.as_str().to_string());
            }

            self.relationship_map(RelationshipKind::DependsOn, owner, &target, labels);
        }
    }

    fn project_provider(&mut self, provider: &ProviderDescriptor) {
        let id = provider_id(provider);
        let trait_resource =
            self.type_resource((provider.trait_ty.type_name)(), provider.trait_ty.name);
        let component = self.app.component_resource_id(provider.concrete_ty.type_id);
        let order = self
            .app
            .provider_order(provider.trait_ty.type_id, provider.concrete_ty.type_id);
        let mut labels = BTreeMap::from([
            (String::from("primary"), provider.primary.to_string()),
            (String::from("priority"), provider.priority.to_string()),
            (String::from("qualifier"), provider.qualifier.to_string()),
            (
                String::from("rust-trait"),
                (provider.trait_ty.type_name)().to_string(),
            ),
            (String::from("trait-target"), trait_resource.clone()),
        ]);

        if let Some(order) = order {
            labels.insert(String::from("processing-ordinal"), order.to_string());
        }

        let provenance = self
            .app
            .plugin_plan()
            .selected_contribution(&id)
            .map(contribution_provenance);

        self.resource(
            &id,
            ResourceKind::Provider,
            provider.trait_ty.name,
            provenance,
            labels,
        );
        self.relationship(RelationshipKind::Provides, &id, &trait_resource, []);

        if let Some(component) = &component {
            self.relationship(RelationshipKind::Provides, component, &id, []);
        }
    }
}

fn dependency_labels(dependency: &DependencyDescriptor) -> BTreeMap<String, String> {
    let cardinality = match dependency.cardinality {
        Cardinality::One => "one",
        Cardinality::Collection => "collection",
        Cardinality::Keyed => "keyed",
    };
    let resolution = match dependency.resolution {
        ResolutionMode::Eager => "eager",
        ResolutionMode::Deferred => "deferred",
        ResolutionMode::Lazy => "lazy",
        ResolutionMode::Fresh => "fresh",
    };
    let observation = match dependency.observation {
        DependencyObservation::Snapshot => "snapshot",
        DependencyObservation::Live => "live",
    };
    let mut labels = BTreeMap::from([
        (String::from("cardinality"), cardinality.to_string()),
        (String::from("dynamic"), dependency.dynamic.to_string()),
        (String::from("name"), dependency.name.to_string()),
        (String::from("optional"), dependency.optional.to_string()),
        (String::from("observation"), observation.to_string()),
        (String::from("resolution"), resolution.to_string()),
    ]);

    if let Some(qualifier) = dependency.qualifier {
        labels.insert(String::from("qualifier"), qualifier.to_string());
    }

    labels
}
