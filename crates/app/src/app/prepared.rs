use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;
use upwell_config::{ConfigReloader, ReloadTriggers};
use upwell_core::ResolverSet;
use upwell_di::{
    BoxedComponent, ComponentDescriptor, EffectiveGraph, RootResolver, ScopeContainer,
    ScopeRegistry,
};

use super::App;
#[cfg(feature = "tooling")]
use super::prepare::PreparedTooling;
use crate::builtins::RuntimeReloader;
use crate::entry::EntryTable;
use crate::error::Error;
use crate::lifecycle::ShutdownSignal;
use crate::plugin::EffectivePluginPlan;
use crate::protocol::{PreparedProtocol, ProtocolDefinition};
use crate::registry::AppRegistry;
use crate::runtime::{AppConditionState, AppRuntime, PreparedRuntimeGeneration, RuntimeScopePlan};
use crate::scope::{PreparedScopeTopology, SeedDestination};

/// A validated application assembly awaiting runtime component construction.
///
/// Preparation resolves registrations, configuration, protocol validation, and scope plans
/// without invoking factory-backed application components or constructing the served protocol.
pub struct PreparedApp<D: ProtocolDefinition> {
    pub(super) name: String,
    pub(super) registry: AppRegistry,
    pub(super) instances: Vec<BoxedComponent>,
    pub(super) protocol: D::Prepared,
    pub(super) plugin_plan: EffectivePluginPlan,
    pub(super) shutdown: ShutdownSignal,
    pub(super) root_resolver: RootResolver,
    pub(super) reloader: ConfigReloader,
    pub(super) runtime_reloader: RuntimeReloader,
    pub(super) reload_triggers: ReloadTriggers,
    pub(super) resolved: Arc<[ComponentDescriptor]>,
    pub(super) root_order: Arc<[ComponentDescriptor]>,
    pub(super) effective_graph: EffectiveGraph,
    pub(super) condition: Arc<AppConditionState>,
    #[cfg(feature = "tooling")]
    pub(super) tooling: PreparedTooling,
    pub(super) scope_registry: Arc<ScopeRegistry>,
    pub(super) scope_topology: Arc<PreparedScopeTopology>,
    pub(super) scope_orders: Arc<HashMap<upwell_core::ScopeId, Vec<ComponentDescriptor>>>,
    pub(super) seed_destinations: Arc<HashMap<std::any::TypeId, SeedDestination>>,
    pub(super) entries: Arc<EntryTable>,
    pub(super) resolvers: ResolverSet,
}

/// Generated host callback capabilities retained only for prepared-state tooling projection.
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct HostLifecycleCapabilities {
    #[cfg(feature = "tooling")]
    pub(crate) setup: bool,
    #[cfg(feature = "tooling")]
    pub(crate) configure: bool,
    #[cfg(feature = "tooling")]
    pub(crate) before_build: bool,
    #[cfg(feature = "tooling")]
    pub(crate) after_build: bool,
    #[cfg(feature = "tooling")]
    pub(crate) serve: bool,
}

impl HostLifecycleCapabilities {
    /// Creates one generated host lifecycle capability value.
    #[doc(hidden)]
    pub const fn new(
        setup: bool,
        configure: bool,
        before_build: bool,
        after_build: bool,
        serve: bool,
    ) -> Self {
        #[cfg(not(feature = "tooling"))]
        let _ = (setup, configure, before_build, after_build, serve);

        Self {
            #[cfg(feature = "tooling")]
            setup,
            #[cfg(feature = "tooling")]
            configure,
            #[cfg(feature = "tooling")]
            before_build,
            #[cfg(feature = "tooling")]
            after_build,
            #[cfg(feature = "tooling")]
            serve,
        }
    }
}

impl<D: ProtocolDefinition> PreparedApp<D> {
    #[cfg(feature = "cli")]
    pub(crate) fn into_cli_parts(self) -> (String, AppRegistry, EffectivePluginPlan) {
        (self.name, self.registry, self.plugin_plan)
    }

    /// The configured application name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The validated effective application registry.
    pub fn registry(&self) -> &AppRegistry {
        &self.registry
    }

    /// The validated protocol state awaiting runtime construction.
    pub fn protocol(&self) -> &D::Prepared {
        &self.protocol
    }

    /// The immutable effective plugin plan lowered during preparation.
    pub fn plugin_plan(&self) -> &EffectivePluginPlan {
        &self.plugin_plan
    }

    /// The stable identity of the selected protocol definition.
    pub const fn protocol_id(&self) -> crate::ProtocolId {
        D::ID
    }

    /// The validated protocol-owned scope topology used for planning and runtime opening.
    pub fn scope_topology(&self) -> &PreparedScopeTopology {
        &self.scope_topology
    }

    /// The validated entry points, indexed by dense entry identity.
    pub fn entries(&self) -> &EntryTable {
        &self.entries
    }

    /// Constructs ordinary components and finalizes the application protocol.
    pub async fn build(self) -> Result<App<D>, D::Error> {
        let PreparedApp {
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
            resolved,
            root_order,
            effective_graph,
            condition,
            #[cfg(feature = "tooling")]
                tooling: _,
            scope_registry,
            scope_topology,
            scope_orders,
            seed_destinations,
            entries: _,
            resolvers,
        } = self;

        if !condition.evaluation().evaluation().is_complete() {
            return Err(Error::UndecidedConditionsBuild.into());
        }

        super::prepare::seed::ensure_seeded(&root_order, &instances)?;

        let root = ScopeContainer::build_root(
            &root_order,
            instances,
            resolvers,
            Arc::clone(&scope_registry),
        )
        .await
        .map_err(Error::from)?;

        root_resolver.attach(&root);

        info!(target: "upwell::app",
            app = %name,
            components = registry.components.len(),
            "app built"
        );

        let generation = PreparedRuntimeGeneration::new(
            root,
            scope_registry,
            RuntimeScopePlan::new(scope_topology, scope_orders, seed_destinations),
            resolved,
            effective_graph,
            condition,
        );
        let runtime = AppRuntime::new(Arc::from(name.as_str()), generation, reloader);

        runtime_reloader.attach(&runtime);

        let protocol = protocol.build(&runtime)?;

        Ok(App {
            name,
            registry,
            runtime,
            protocol,
            plugin_plan,
            shutdown,
            reloader: runtime_reloader,
            reload_triggers,
        })
    }
}
