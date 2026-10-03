use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write;

use upwell_config::{CONFIG_BINDINGS, ConditionFactSource, ConfigBinding};
use upwell_core::{ConfigFactDescriptor, DependencyDescriptor};
use upwell_di::{
    COMPONENTS, Component, ComponentDescriptor, ComponentRegistry, ConditionCatalog,
    ConditionEvaluation, ConditionFactSnapshot, PROVIDERS, ProviderDescriptor,
    ProviderSelectionModel,
};

use crate::error::Error;
use crate::scope::PreparedScopeTopology;

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

/// Holds the *agnostic* component, provider, and config-binding descriptors of an app —
/// declarations only. Runtime instances live in the
/// [`ScopeContainer`](upwell_di::ScopeContainer).
///
/// Wraps the DI engine's [`ComponentRegistry`] (component/provider graph) with the config
/// bindings, and runs the cross-cutting validation the component graph alone cannot
/// (config edges). Protocol-specific declarations (services/routes) live in the protocol
/// plugin, not here, so this stays usable by any protocol.
#[derive(Default, Debug)]
pub struct AppRegistry {
    pub components: Vec<ComponentDescriptor>,
    pub providers: Vec<ProviderDescriptor>,
    /// Config bindings (a config type bound to a property path). Populated from the
    /// auto-discovered config bindings slice and from explicit builder bindings.
    pub config_bindings: Vec<ConfigBinding>,
    /// Registered condition-fact sources (a config type's condition facts at one binding
    /// path). Populated from explicit builder registrations.
    pub condition_facts: Vec<ConditionFactSource>,
}

impl AppRegistry {
    /// Collects every link-time-registered agnostic descriptor (components, providers,
    /// config bindings) into an `AppRegistry`. Protocol variant slices (e.g. RPC services)
    /// are folded in by the protocol definition, not here.
    pub fn collect() -> Self {
        let mut components: Vec<_> = COMPONENTS.iter().copied().collect();
        let mut providers: Vec<_> = PROVIDERS.iter().copied().collect();
        let mut config_bindings: Vec<_> = CONFIG_BINDINGS.iter().map(|d| d.to_binding()).collect();

        // Distributed-slice order differs between linkers. Sort auto-discovered
        // descriptors only; explicit builder registrations retain caller order.
        components.sort_by_key(|component| component.id);
        providers.sort_by(|left, right| {
            (left.trait_ty.type_name)()
                .cmp((right.trait_ty.type_name)())
                .then_with(|| (left.concrete_ty.type_name)().cmp((right.concrete_ty.type_name)()))
                .then_with(|| left.qualifier.cmp(right.qualifier))
        });
        config_bindings.sort_by(|left, right| {
            (left.ty.type_name)()
                .cmp((right.ty.type_name)())
                .then_with(|| left.path.cmp(&right.path))
        });

        Self {
            components,
            providers,
            config_bindings,
            condition_facts: Vec::new(),
        }
    }

    /// The DI engine's view of this registry — the component/provider graph.
    pub(crate) fn component_registry(&self) -> ComponentRegistry {
        ComponentRegistry {
            components: self.components.clone(),
            providers: self.providers.clone(),
        }
    }

    /// Collapses the registered descriptors to one per type (delegated to the DI engine).
    pub fn resolved_components(&self) -> crate::Result<Vec<ComponentDescriptor>> {
        Ok(self.component_registry().resolved_components()?)
    }

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

    /// Extracts the condition-fact scalars of every registered fact source from a
    /// staged reload's values, producing a validated fact snapshot.
    ///
    /// Each staged value is its binding's `(TypeId, path, erased value)`. A source whose
    /// binding has no staged value contributes nothing — the transactional reload stages
    /// every binding, changed or not, so registered sources always resolve.
    pub fn condition_snapshot<'a>(
        &self,
        staged: impl IntoIterator<Item = (TypeId, &'a str, &'a dyn std::any::Any)>,
    ) -> Result<ConditionFactSnapshot, upwell_di::ConditionError> {
        let staged = staged.into_iter().collect::<Vec<_>>();
        let mut scalars = Vec::new();

        for source in &self.condition_facts {
            for (type_id, path, value) in &staged {
                if *type_id == source.ty.type_id && *path == source.path {
                    scalars.extend((source.facts.scalars)(*value));
                }
            }
        }

        ConditionFactSnapshot::new(scalars)
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

    /// Returns the effective descriptor registered for component type `T`.
    ///
    /// Duplicate registrations are resolved by the same rules used during registry validation.
    pub fn resolved_component<T: Component>(&self) -> crate::Result<Option<ComponentDescriptor>> {
        let type_id = TypeId::of::<T>();
        let component = self
            .resolved_components()?
            .into_iter()
            .find(|component| component.ty.type_id == type_id);

        Ok(component)
    }

    /// Validates structural consistency: the component graph (via the DI engine), then the
    /// config-binding rules.
    pub fn validate(&self) -> crate::Result<()> {
        self.component_registry().validate()?;

        let components = self.resolved_components()?;

        self.validate_configs(&components)?;

        Ok(())
    }

    /// Validates the component graph against a prepared protocol-owned scope topology,
    /// then applies the application configuration-binding rules.
    pub fn validate_with_scope_topology(
        &self,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<()> {
        self.component_registry()
            .validate_with_scope_reachability(|consumer, dependency| {
                topology.is_reachable(&consumer, &dependency)
            })?;

        let components = self.resolved_components()?;

        self.validate_configs(&components)?;

        Ok(())
    }

    pub(crate) fn validate_effective_with_scope_topology(
        &self,
        components: &[ComponentDescriptor],
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<()> {
        self.component_registry()
            .validate_with_scope_reachability_using(
                components,
                selection,
                |consumer, dependency| topology.is_reachable(&consumer, &dependency),
            )?;

        self.validate_configs(components)?;

        Ok(())
    }

    /// Validates config edges against the registered bindings: a `#[config("path")]` edge
    /// must have a binding of its type at that path, and a `#[config]` shorthand edge must
    /// have exactly one binding of its type.
    pub(crate) fn validate_configs(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let mut bound: HashMap<TypeId, Vec<&str>> = HashMap::new();

        for binding in &self.config_bindings {
            bound
                .entry(binding.ty.type_id)
                .or_default()
                .push(&binding.path);
        }

        for c in components {
            for dep in c.dependencies().iter().filter(|dep| dep.config) {
                let dep_id = dep.ty.type_id;
                let paths = bound.get(&dep_id);

                match dep.qualifier {
                    Some(path) => {
                        let found = paths.is_some_and(|ps| ps.contains(&path));

                        if !found {
                            return Err(Error::MissingConfig {
                                component: c.name.to_string(),
                                type_name: (dep.ty.type_name)().to_string(),
                                path: path.to_string(),
                            });
                        }
                    }

                    None => {
                        let bound_paths = paths.cloned().unwrap_or_default();

                        match bound_paths.as_slice() {
                            [_] => {}

                            [] => {
                                return Err(Error::MissingConfig {
                                    component: c.name.to_string(),
                                    type_name: (dep.ty.type_name)().to_string(),
                                    path: "<unqualified>".to_string(),
                                });
                            }

                            _ => {
                                return Err(Error::AmbiguousConfig {
                                    component: c.name.to_string(),
                                    type_name: (dep.ty.type_name)().to_string(),
                                    count: bound_paths.len(),
                                    paths: bound_paths.join(", "),
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn condition_identity(&self) -> HashMap<(TypeId, String), usize> {
        let mut identity = HashMap::new();

        for binding in &self.config_bindings {
            *identity
                .entry((binding.ty.type_id, binding.path.clone()))
                .or_default() += 1;
        }

        identity
    }

    fn write_components(&self, f: &mut impl Write) -> fmt::Result {
        let components = self
            .resolved_components()
            .unwrap_or_else(|_| self.components.clone());

        writeln!(f, "Components:")?;
        for c in &components {
            writeln!(f, "  {}", c.name)?;

            let deps = c.dependencies();

            if !deps.is_empty() {
                Self::write_dependency(f, deps.iter())?
            }
        }

        Ok(())
    }

    fn write_dependency<'a>(
        f: &mut impl Write,
        deps: impl Iterator<Item = &'a DependencyDescriptor>,
    ) -> fmt::Result {
        write!(f, "    depends on:")?;
        for (i, dep) in deps.enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }

            write!(f, " {}", dep.name)?;
        }

        writeln!(f)?;

        Ok(())
    }
}

impl fmt::Display for AppRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_components(f)
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::future::Future;
    use std::pin::Pin;

    use upwell_config::{ConditionFactSource, ConditionFacts, ConfigBinding, ConfigProperties};
    use upwell_core::{
        Cardinality, ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId,
        DependencyDescriptor, TypeDescriptor,
    };
    use upwell_di::{
        BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
        ComponentFactoryDescriptor, Singleton,
    };

    use super::AppRegistry;
    use crate::Error;

    #[derive(serde::Deserialize)]
    struct TestConfig;

    impl ConfigProperties for TestConfig {
        const NAME: &'static str = "TestConfig";
    }

    struct RegisteredComponent;

    impl Component for RegisteredComponent {
        type Handle = std::sync::Arc<Self>;

        const ID: &'static str = "registered";
        const NAME: &'static str = "RegisteredComponent";

        fn into_handle(self) -> Self::Handle {
            std::sync::Arc::new(self)
        }
    }

    fn fake_factory<'a>(
        _: &'a mut ComponentConstructionContext,
    ) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + 'a>> {
        Box::pin(async { unreachable!("registry validation does not construct components") })
    }

    fn config_deps() -> Vec<DependencyDescriptor> {
        vec![DependencyDescriptor {
            name: "cfg",
            ty: TypeDescriptor::of::<TestConfig>("TestConfig"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: true,
            resolution: upwell_core::ResolutionMode::Eager,
            observation: upwell_core::DependencyObservation::Live,
        }]
    }

    static CONFIG_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "static",
        construct: fake_factory,
        dependencies: config_deps,
        default: false,
    }];

    fn config_factories() -> &'static [ComponentFactoryDescriptor] {
        &CONFIG_FACTORY
    }

    fn component() -> ComponentDescriptor {
        ComponentDescriptor {
            id: "needs_config",
            name: "NeedsConfig",
            ty: TypeDescriptor::of::<()>("NeedsConfig"),
            scope: &Singleton,
            condition: None,
            factories: config_factories,
            hooks: upwell_hooks::no_hooks,
            generation_snapshot: None,
        }
    }

    #[test]
    fn resolves_component_descriptor_by_concrete_type() {
        let registry = AppRegistry {
            components: vec![ComponentDescriptor::of::<RegisteredComponent>()],
            providers: Vec::new(),
            config_bindings: Vec::new(),
            condition_facts: Vec::new(),
        };

        let descriptor = registry
            .resolved_component::<RegisteredComponent>()
            .expect("component resolution succeeds")
            .expect("typed component is registered");

        assert_eq!(descriptor.id, RegisteredComponent::ID);
        assert_eq!(descriptor.ty.type_id, TypeId::of::<RegisteredComponent>());
    }

    #[test]
    fn missing_unqualified_config_binding_is_missing_not_ambiguous() {
        let registry = AppRegistry {
            components: vec![component()],
            providers: Vec::new(),
            config_bindings: Vec::new(),
            condition_facts: Vec::new(),
        };

        let err = registry.validate().expect_err("config binding is missing");

        assert!(matches!(
            err,
            Error::MissingConfig {
                component,
                type_name,
                path,
            } if component == "NeedsConfig"
                && type_name.ends_with("TestConfig")
                && path == "<unqualified>"
        ));
    }

    #[test]
    fn multiple_unqualified_config_bindings_are_ambiguous() {
        let registry = AppRegistry {
            components: vec![component()],
            providers: Vec::new(),
            config_bindings: vec![
                ConfigBinding::of::<TestConfig>("one"),
                ConfigBinding::of::<TestConfig>("two"),
            ],
            condition_facts: Vec::new(),
        };

        let err = registry
            .validate()
            .expect_err("config binding is ambiguous");

        assert!(matches!(
            err,
            Error::AmbiguousConfig {
                component,
                type_name,
                count: 2,
                paths,
            } if component == "NeedsConfig"
                && type_name.ends_with("TestConfig")
                && paths == "one, two"
        ));
    }

    #[test]
    fn condition_snapshot_extracts_scalars_from_staged_values() {
        #[derive(serde::Deserialize)]
        struct FlagConfig {
            enabled: bool,
        }

        impl ConfigProperties for FlagConfig {
            const NAME: &'static str = "FlagConfig";
        }

        impl ConditionFacts for FlagConfig {
            fn condition_facts() -> Vec<ConfigFactDescriptor> {
                vec![ConfigFactDescriptor {
                    id: ConfigFactId::new("FlagConfig", "flags", "enabled"),
                    kind: ConditionScalarKind::Bool,
                    source: upwell_core::descriptor_source!(),
                }]
            }

            fn condition_scalars(&self) -> Vec<(ConfigFactId, ConditionScalar)> {
                vec![(
                    ConfigFactId::new("FlagConfig", "flags", "enabled"),
                    ConditionScalar::Bool(self.enabled),
                )]
            }
        }

        let mut registry = AppRegistry::default();
        registry
            .condition_facts
            .push(ConditionFactSource::of::<FlagConfig>("flags"));

        let enabled = FlagConfig { enabled: true };
        let disabled = FlagConfig { enabled: false };
        let unrelated = TestConfig;

        let snapshot = registry
            .condition_snapshot([
                (
                    TypeId::of::<FlagConfig>(),
                    "flags",
                    &std::sync::Arc::new(enabled) as &dyn std::any::Any,
                ),
                (
                    TypeId::of::<FlagConfig>(),
                    "other",
                    &disabled as &dyn std::any::Any,
                ),
                (
                    TypeId::of::<TestConfig>(),
                    "flags",
                    &unrelated as &dyn std::any::Any,
                ),
            ])
            .expect("snapshot validates");

        registry
            .evaluate_conditions(<FlagConfig as ConditionFacts>::condition_facts(), &snapshot)
            .expect("the matching staged binding supplies the declared fact");
    }
}
