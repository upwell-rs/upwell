use std::sync::Arc;

use tracing::debug;
use upwell_config::{ConfigManager, ConfigReloader, ConfigStore, ReloadableConfig};
use upwell_core::{ResolverSet, RuntimeGenerationId};
use upwell_di::{
    ComponentDescriptor, ComponentRegistry, EffectiveGraph, RootResolver, ScopeRegistry,
};
use upwell_dirs::DirectoriesManager;

use super::{AppBuilder, PreparedApp};
use crate::builtins::RuntimeReloader;
use crate::entry::EntryTable;
use crate::error::Error;
use crate::host::ExecutionMode;
use crate::lifecycle::ShutdownSignal;
use crate::protocol::{PreBuildContext, ProtocolDefinition, ValidationContext};
use crate::registry::{AppConditionEvaluation, AppRegistry};
use crate::runtime::AppConditionState;
use crate::scope::{PreparedScopeTopology, ScopePlan};

pub(crate) mod seed;
#[cfg(feature = "tooling")]
mod tooling;

#[cfg(feature = "tooling")]
pub(crate) use tooling::PreparedTooling;

impl<D: ProtocolDefinition> AppBuilder<D> {
    /// Registers and validates the application without constructing ordinary components.
    pub fn prepare(self) -> Result<PreparedApp<D>, D::Error> {
        debug!(target: "upwell::app", app = %self.name, "building app");

        let AppBuilder {
            name,
            mut registry,
            mut instances,
            config_source,
            auto_discovery_enabled,
            dirs,
            plugins,
            mut protocol,
            mode,
        } = self;
        let plugin_plan = plugins
            .freeze::<D>(auto_discovery_enabled)?
            .lower(&mut registry);
        #[cfg(feature = "tooling")]
        let mut plugin_plan = plugin_plan;
        let shutdown = ShutdownSignal::new();
        let root_resolver = RootResolver::new();
        let runtime_reloader = RuntimeReloader::new();

        protocol.register(&mut registry);

        let dirs = match dirs {
            Some(dirs) => dirs,
            None => DirectoriesManager::try_for_app(&name).map_err(Error::Directories)?,
        };

        seed::seed_framework(
            &dirs,
            &shutdown,
            &root_resolver,
            &runtime_reloader,
            &mut registry,
            &mut instances,
        );
        protocol.pre_build(&mut PreBuildContext::new(&mut registry, &mut instances))?;

        let tree = finalize_config(config_source, &dirs, auto_discovery_enabled, &mut registry)?;
        let scope_topology = Arc::new(D::SCOPE_TOPOLOGY.prepare().map_err(Error::from)?);
        let entries =
            Arc::new(EntryTable::new(&registry.entries, &scope_topology).map_err(Error::from)?);

        select_catalog(&mut registry)?;

        let (config_store, reload_slots) = ConfigStore::build(&tree).map_err(Error::from)?;
        let config_store = Arc::new(config_store);
        let evaluation = evaluate_startup_conditions(&registry, &tree, &reload_slots, mode)?;
        let condition = Arc::new(AppConditionState::new(
            Arc::new(registry.clone()),
            evaluation,
        ));
        let (eligible, resolved) = narrow_to_eligible(&mut registry, &condition)?;

        #[cfg(feature = "tooling")]
        plugin_plan.reconcile(&registry);

        seed::seed_hook_manager(&resolved, &mut instances);

        let (effective_graph, scopes) =
            plan_effective_graph(&registry, &eligible, &resolved, &scope_topology)
                .map_err(|error| undecided_graph_error(&condition, error))?;
        let provider_selection = Arc::clone(effective_graph.provider_selection());
        let root_order = scopes.root_order(&effective_graph);
        let protocol = protocol.prepare(&ValidationContext::new(
            &name,
            &registry,
            config_store.as_ref(),
            &plugin_plan,
        ))?;
        let mut resolvers = ResolverSet::new();

        resolvers.insert(config_store);

        let reload_triggers = tree.triggers();
        let reloader = ConfigReloader::new(tree, reload_slots);
        #[cfg(feature = "tooling")]
        let tooling = PreparedTooling::capture::<D>(
            mode,
            &protocol,
            &reloader,
            &registry,
            &condition,
            &provider_selection,
            crate::tooling::ProjectionSnapshot::capture(
                &provider_selection,
                &scope_topology,
                &registry.components,
                &root_order,
                &scopes.orders,
                &instances,
                &scopes.seed_destinations,
            )
            .and_then(|snapshot| {
                snapshot.with_entries(
                    &provider_selection,
                    &scope_topology,
                    &entries,
                    &scopes.layouts,
                )
            })
            .map_err(Error::from)?,
        )?;
        let scope_registry = Arc::new(
            ScopeRegistry::from_selection_model(
                scopes.transient,
                resolved
                    .iter()
                    .map(|component| (component.ty.type_id, *component))
                    .collect(),
                provider_selection,
            )
            .map_err(Error::from)?,
        );

        Ok(PreparedApp {
            name,
            registry,
            instances,
            protocol,
            plugin_plan,
            shutdown,
            root_resolver,
            reloader,
            runtime_reloader,
            reload_triggers,
            resolved: Arc::from(resolved),
            root_order: Arc::from(root_order),
            effective_graph,
            condition,
            #[cfg(feature = "tooling")]
            tooling,
            scope_registry,
            scope_topology,
            scope_orders: Arc::new(scopes.orders),
            scope_layouts: Arc::new(scopes.layouts),
            seed_destinations: Arc::new(scopes.seed_destinations),
            entries,
            resolvers,
        })
    }
}

/// Finalizes the config manager: the builder's source or the app's config directory, with
/// `#[config(path)]` auto-discovery when enabled, then the explicit and condition-fact
/// bindings. The registry keeps the final binding list.
fn finalize_config(
    source: Option<ConfigManager>,
    dirs: &DirectoriesManager,
    auto_discovery_enabled: bool,
    registry: &mut AppRegistry,
) -> crate::Result<ConfigManager> {
    let explicit_bindings = std::mem::take(&mut registry.config_bindings);
    let mut tree = match source {
        Some(config) => config,
        None => ConfigManager::load_in(&dirs.config_path(), &[])?,
    }
    .with_directories(dirs);

    if auto_discovery_enabled {
        tree = tree.auto_discover();
    }

    for binding in explicit_bindings {
        tree.register_binding(binding);
    }

    for binding in registry.condition_bindings() {
        tree.register_binding(binding);
    }

    registry.config_bindings = tree.bindings().to_vec();

    Ok(tree)
}

/// Collapses the registry to its full component catalog, where explicit factories override
/// defaults, and validates provider selection over it. The catalog keeps conditionally
/// disabled descriptors so a later reload can re-include them.
fn select_catalog(registry: &mut AppRegistry) -> crate::Result<()> {
    let catalog = registry.resolved_components()?;

    registry
        .component_registry()
        .provider_selection_model(&catalog)?;
    registry.components = catalog;

    Ok(())
}

/// Narrows the registry to the eligible components and providers of the startup
/// evaluation, returning the eligible component registry and its resolved components.
fn narrow_to_eligible(
    registry: &mut AppRegistry,
    condition: &AppConditionState,
) -> crate::Result<(ComponentRegistry, Vec<ComponentDescriptor>)> {
    let eligible = condition
        .evaluation()
        .evaluation()
        .eligible_registry()
        .clone();
    let resolved = eligible.resolved_components()?;

    registry.components = eligible.components.clone();
    registry.providers = eligible.providers.clone();

    Ok((eligible, resolved))
}

/// Builds and validates the initial effective graph of the eligible registry, and partitions
/// its scope plans.
fn plan_effective_graph(
    registry: &AppRegistry,
    eligible: &ComponentRegistry,
    resolved: &[ComponentDescriptor],
    topology: &PreparedScopeTopology,
) -> crate::Result<(EffectiveGraph, ScopePlan)> {
    let graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        eligible,
        |consumer, dependency| topology.is_reachable(&consumer, &dependency),
    )
    .map_err(Error::from)?;
    let selection = graph.provider_selection();

    registry.validate_effective_with_scope_topology(resolved, selection, topology)?;
    crate::entry::validate_targets(registry, selection, topology)?;

    let scopes = ScopePlan::partition(resolved, selection, topology, &registry.entries)?;

    Ok((graph, scopes))
}

/// Names the undecided conditions when a tooling preparation's graph fails validation, since
/// a component left out because its condition was not evaluated may be what the graph lacks.
fn undecided_graph_error(condition: &AppConditionState, error: Error) -> Error {
    let not_evaluated = condition
        .evaluation()
        .evaluation()
        .not_evaluated()
        .collect::<Vec<_>>();

    if not_evaluated.is_empty() {
        return error;
    }

    Error::UndecidedConditionGraph {
        not_evaluated,
        source: Box::new(error),
    }
}

/// Evaluates the startup conditions from the staged config values, with the same
/// evaluator the transactional reload uses, or in tooling mode the tooling evaluator,
/// which never runs trusted callbacks. An app with no condition facts gets an
/// empty-facts evaluation, which leaves every unconditional component eligible.
///
/// Staging, fact extraction, descriptor callbacks, and evaluation are application
/// extension points, so — like the reload's condition stage — the whole stage runs
/// inside a panic boundary: a panic returns the redacted preparation panic error,
/// while ordinary errors pass through unchanged.
pub(crate) fn evaluate_startup_conditions(
    registry: &AppRegistry,
    tree: &ConfigManager,
    reload_slots: &[Box<dyn ReloadableConfig>],
    mode: ExecutionMode,
) -> crate::Result<AppConditionEvaluation> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let staged: Vec<_> = reload_slots
            .iter()
            .map(|slot| slot.stage_current())
            .collect();
        let snapshot = registry.condition_snapshot(
            staged
                .iter()
                .map(|entry| (entry.type_id(), entry.path(), entry.shared_value())),
            |path| tree.resolve_raw(path),
        )?;
        let facts = registry
            .condition_facts
            .iter()
            .flat_map(|source| (source.facts.descriptors)(source.path));

        match mode {
            ExecutionMode::Tooling => registry.evaluate_tooling_conditions(facts, &snapshot),
            _ => registry.evaluate_conditions(facts, &snapshot),
        }
        .map_err(Error::from)
    }))
    .map_err(|_| Error::CandidatePreparationPanicked)?
}
