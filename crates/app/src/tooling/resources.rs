use std::collections::BTreeMap;

use upwell_core::{Singleton, StaticScope, Transient};
use upwell_tooling_schema::{Provenance, RelationshipKind, ResourceKind};

use super::{
    Projection, component_id, config_binding_id, contribution_provenance, item_location,
    lifecycle_resource, scope_id, type_location, with_source,
};
use crate::ProtocolDefinition;

mod dependency;

impl<D: ProtocolDefinition> Projection<'_, D> {
    pub(super) fn project_application(&mut self) {
        self.resource(
            "framework",
            ResourceKind::Contributor,
            "upwell",
            Some(Provenance {
                owner: Some(String::from("framework")),
                origin: Some(String::from("framework")),
                ..Provenance::default()
            }),
            BTreeMap::new(),
        );
        self.resource(
            "application",
            ResourceKind::Application,
            self.app.name(),
            None,
            BTreeMap::new(),
        );
        let protocol = format!("protocol:{}", self.app.protocol_id().as_str());

        self.resource(
            &protocol,
            ResourceKind::Protocol,
            self.app.protocol_id().as_str(),
            Some(Provenance {
                owner: Some(protocol.clone()),
                origin: Some(String::from("protocol")),
                ..Provenance::default()
            }),
            BTreeMap::from([(
                String::from("protocol-id"),
                self.app.protocol_id().as_str().to_string(),
            )]),
        );
        self.relationship(RelationshipKind::Validates, &protocol, "application", []);

        let host = self.app.host_lifecycle();

        if let Some(host) = host {
            for (name, callback) in [
                ("setup", host.setup),
                ("configure", host.configure),
                ("before_build", host.before_build),
                ("after_build", host.after_build),
                ("serve", host.serve),
            ] {
                self.lifecycle(name, "host-phase", Some(callback), false);
            }
        }

        for name in ["prepare", "build"] {
            self.lifecycle(name, "construction-phase", None, false);
        }

        for (name, repeatable) in [
            ("startup", false),
            ("shutdown", false),
            ("config_reload", true),
        ] {
            self.lifecycle(name, "runtime-event", None, repeatable);
        }

        if host.is_some() {
            self.lifecycle_order("setup", "configure");
            self.lifecycle_order("configure", "before_build");
            self.lifecycle_order("before_build", "prepare");
        }

        self.lifecycle_order("prepare", "build");

        if host.is_some() {
            self.lifecycle_order("build", "after_build");
        }

        if host.is_some() {
            self.lifecycle_order("after_build", "serve");
        }
    }

    pub(super) fn project_scopes(&mut self) {
        self.framework_scope(
            Singleton::ID.as_str(),
            Singleton::NAME,
            Singleton::RANK,
            false,
        );
        self.framework_scope(
            Transient::ID.as_str(),
            Transient::NAME,
            Transient::RANK,
            true,
        );

        for boundary in self.app.scope_topology().boundaries() {
            let id = scope_id(boundary.id().as_str());
            let parent = scope_id(boundary.parent().id().as_str());

            self.resource(
                &id,
                ResourceKind::Scope,
                boundary.name(),
                Some(Provenance {
                    owner: Some(format!("protocol:{}", self.app.protocol_id().as_str())),
                    origin: Some(String::from("protocol")),
                    ..Provenance::default()
                }),
                BTreeMap::from([
                    (String::from("rank"), boundary.rank().to_string()),
                    (String::from("scope-id"), boundary.id().as_str().to_string()),
                ]),
            );
            self.relationship(RelationshipKind::OpensScope, &parent, &id, []);
        }
    }

    pub(super) fn project_config_bindings(&mut self) {
        let bindings = self.app.registry().config_bindings.clone();

        for binding in &bindings {
            let id = config_binding_id((binding.ty.type_name)(), &binding.path);
            let ty = self.type_resource(&binding.ty);
            let provenance = self
                .app
                .plugin_plan()
                .selected_contribution(&id)
                .map(contribution_provenance);

            self.resource(
                &id,
                ResourceKind::ConfigBinding,
                binding.ty.name,
                with_source(provenance, type_location(&binding.ty)),
                BTreeMap::from([
                    (String::from("path"), binding.path.clone()),
                    (String::from("redacted"), String::from("true")),
                    (String::from("value-exported"), String::from("false")),
                ]),
            );
            self.relationship(RelationshipKind::Binds, &id, &ty, []);
        }

        self.project_config_sources();
    }

    pub(super) fn project_hooks(&mut self) {
        let components = self.app.tooling_snapshot().components().to_vec();

        for component in &components {
            let component_resource = component_id(component.descriptor.id);

            for hook in &component.hooks {
                let id = format!(
                    "hook:{}:{}:{}",
                    component.descriptor.id, hook.kind, hook.ordinal
                );

                self.resource(
                    &id,
                    ResourceKind::Hook,
                    hook.kind,
                    with_source(None, hook.source.as_ref().and_then(item_location)),
                    BTreeMap::from([(String::from("ordinal"), hook.ordinal.to_string())]),
                );
                self.relationship(RelationshipKind::Hooks, &component_resource, &id, []);

                if let Some(lifecycle) = lifecycle_resource(hook.kind) {
                    self.relationship(RelationshipKind::Hooks, &id, lifecycle, []);
                }

                for dependency in &hook.dependencies {
                    self.project_dependency(&id, dependency);
                }
            }
        }
    }

    fn framework_scope(&mut self, id: &str, name: &str, rank: u8, transient: bool) {
        self.resource(
            &scope_id(id),
            ResourceKind::Scope,
            name,
            Some(Provenance {
                owner: Some(String::from("framework")),
                origin: Some(String::from("framework")),
                ..Provenance::default()
            }),
            BTreeMap::from([
                (String::from("rank"), rank.to_string()),
                (String::from("scope-id"), id.to_string()),
                (String::from("transient"), transient.to_string()),
            ]),
        );
    }

    fn lifecycle(&mut self, name: &str, category: &str, callback: Option<bool>, repeatable: bool) {
        let mut labels = BTreeMap::from([
            (String::from("category"), category.to_string()),
            (String::from("repeatable"), repeatable.to_string()),
        ]);

        if let Some(callback) = callback {
            labels.insert(String::from("callback"), callback.to_string());
        }

        self.resource(
            &format!("lifecycle:{name}"),
            ResourceKind::Lifecycle,
            name,
            None,
            labels,
        );
    }

    fn lifecycle_order(&mut self, before: &str, after: &str) {
        self.relationship(
            RelationshipKind::OrdersBefore,
            &format!("lifecycle:{before}"),
            &format!("lifecycle:{after}"),
            [],
        );
    }

    fn project_config_sources(&mut self) {
        let mut previous: Option<String> = None;

        for (ordinal, path) in self.app.config_sources().iter().enumerate() {
            let display = path.display().to_string();

            if display.is_empty() || display == "<in-memory>" {
                continue;
            }

            let id = format!("config-source:{ordinal}");

            self.resource(
                &id,
                ResourceKind::Contribution,
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("config source"),
                Some(Provenance {
                    owner: Some(String::from("framework")),
                    origin: Some(String::from("config-source")),
                    ordinal: Some(ordinal as u32),
                    ..Provenance::default()
                }),
                BTreeMap::from([
                    (String::from("path"), display),
                    (String::from("merge-ordinal"), ordinal.to_string()),
                    (String::from("values-exported"), String::from("false")),
                ]),
            );

            if let Some(previous) = previous {
                self.relationship(
                    RelationshipKind::OrdersBefore,
                    &previous,
                    &id,
                    [(String::from("order"), String::from("config-merge"))],
                );
            }

            previous = Some(id);
        }
    }
}
