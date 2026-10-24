use std::future::Future;

use super::ErasedPluginCliCommand;
use crate::{
    App, AppHost, AppRegistry, BootstrapContext, Built, CliPhase, CommandContext,
    CommandContextError, EffectivePluginPlan, PreBuild, ProtocolDefinition, Setup,
};

/// A typed plugin command that participates in generated lifecycle-aware dispatch.
pub trait PluginCliCommand: Sync {
    /// The statically required application lifecycle phase.
    type Phase: PluginCliPhase;

    /// The typed failure returned by this command.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Executes this parsed command against protocol-neutral state for its declared phase.
    fn run(
        &self,
        context: PluginCommandContext<Self::Phase>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// A sealed CLI phase that defines the protocol-neutral state exposed to plugin commands.
pub trait PluginCliPhase: CliPhase {
    /// Protocol-neutral state carried by a plugin command at this phase.
    #[doc(hidden)]
    type PluginState: Send;

    /// Converts the corresponding application command context into plugin-owned state.
    #[doc(hidden)]
    fn into_plugin<H>(context: CommandContext<H, Self>) -> PluginCommandContext<Self>
    where
        H: AppHost,
        H::Protocol: Send,
        Self: Sized;

    /// Erases one command while preserving this sealed phase in the private runner variant.
    #[doc(hidden)]
    fn erase<T>(command: T) -> ErasedPluginCliCommand
    where
        T: PluginCliCommand<Phase = Self> + Send + Sync + 'static,
        Self: Sized;
}

/// Protocol-neutral lifecycle state supplied to a selected plugin command.
pub struct PluginCommandContext<P: PluginCliPhase> {
    bootstrap: BootstrapContext,
    state: P::PluginState,
}

/// Configured application metadata exposed to a plugin command.
#[doc(hidden)]
pub struct PluginPreBuildState {
    name: String,
    registry: AppRegistry,
    plugin_plan: EffectivePluginPlan,
}

/// Built application metadata and ownership exposed to a plugin command.
#[doc(hidden)]
pub struct PluginBuiltState {
    name: String,
    container: std::sync::Arc<upwell_di::ScopeContainer>,
    plugin_plan: EffectivePluginPlan,
    _owner: Box<dyn Send>,
}

impl<P: PluginCliPhase> PluginCommandContext<P> {
    /// Global bootstrap state and typed application/plugin argument groups.
    pub const fn bootstrap(&self) -> &BootstrapContext {
        &self.bootstrap
    }

    /// Mutable global bootstrap state and typed application/plugin argument groups.
    pub fn bootstrap_mut(&mut self) -> &mut BootstrapContext {
        &mut self.bootstrap
    }

    /// Borrows a required typed bootstrap value.
    pub fn require<T: Send + Sync + 'static>(&self) -> Result<&T, CommandContextError> {
        self.bootstrap
            .get::<T>()
            .ok_or(CommandContextError::MissingValue {
                type_name: std::any::type_name::<T>(),
            })
    }

    pub(crate) fn from_application<H>(context: CommandContext<H, P>) -> Self
    where
        H: AppHost,
        H::Protocol: Send,
    {
        P::into_plugin(context)
    }
}

impl PluginCommandContext<PreBuild> {
    /// The configured application name guaranteed by this context's phase.
    pub fn application_name(&self) -> &str {
        &self.state.name
    }

    /// The validated application registry guaranteed by this context's phase.
    pub const fn registry(&self) -> &AppRegistry {
        &self.state.registry
    }

    /// The immutable effective plugin plan guaranteed by this context's phase.
    pub const fn plugin_plan(&self) -> &EffectivePluginPlan {
        &self.state.plugin_plan
    }
}

impl PluginCommandContext<Built> {
    /// The built application name guaranteed by this context's phase.
    pub fn application_name(&self) -> &str {
        &self.state.name
    }

    /// The immutable effective plugin plan guaranteed by this context's phase.
    pub const fn plugin_plan(&self) -> &EffectivePluginPlan {
        &self.state.plugin_plan
    }

    /// Resolves an `Injectable` from the built plugin command's root DI container.
    pub fn resolve<T>(&self) -> impl Future<Output = Result<T, upwell_di::Error>> + Send + use<T>
    where
        T: upwell_di::Injectable,
    {
        let container = std::sync::Arc::clone(&self.state.container);

        async move {
            container
                .resolve::<T>()
                .await?
                .ok_or_else(|| upwell_di::Error::MissingDependency {
                    component: String::from("plugin CLI command"),
                    component_id: String::from("plugin-cli-command"),
                    dependency: std::any::type_name::<T>().to_string(),
                    type_name: std::any::type_name::<T>().to_string(),
                })
        }
    }
}

impl PluginCliPhase for Setup {
    type PluginState = ();

    fn into_plugin<H>(context: CommandContext<H, Self>) -> PluginCommandContext<Self>
    where
        H: AppHost,
        H::Protocol: Send,
    {
        PluginCommandContext {
            bootstrap: context.into_bootstrap(),
            state: (),
        }
    }

    fn erase<T>(command: T) -> ErasedPluginCliCommand
    where
        T: PluginCliCommand<Phase = Self> + Send + Sync + 'static,
    {
        ErasedPluginCliCommand::setup(command)
    }
}

impl PluginCliPhase for PreBuild {
    type PluginState = PluginPreBuildState;

    fn into_plugin<H>(context: CommandContext<H, Self>) -> PluginCommandContext<Self>
    where
        H: AppHost,
        H::Protocol: Send,
    {
        let (bootstrap, app) = context.into_parts();
        let (name, registry, plugin_plan) = app.into_cli_parts();

        PluginCommandContext {
            bootstrap,
            state: PluginPreBuildState {
                name,
                registry,
                plugin_plan,
            },
        }
    }

    fn erase<T>(command: T) -> ErasedPluginCliCommand
    where
        T: PluginCliCommand<Phase = Self> + Send + Sync + 'static,
    {
        ErasedPluginCliCommand::pre_build(command)
    }
}

impl PluginCliPhase for Built {
    type PluginState = PluginBuiltState;

    fn into_plugin<H>(context: CommandContext<H, Self>) -> PluginCommandContext<Self>
    where
        H: AppHost,
        H::Protocol: Send,
    {
        let (bootstrap, app) = context.into_parts();

        built_plugin_context(bootstrap, app)
    }

    fn erase<T>(command: T) -> ErasedPluginCliCommand
    where
        T: PluginCliCommand<Phase = Self> + Send + Sync + 'static,
    {
        ErasedPluginCliCommand::built(command)
    }
}

fn built_plugin_context<D: ProtocolDefinition + Send>(
    bootstrap: BootstrapContext,
    app: App<D>,
) -> PluginCommandContext<Built> {
    let name = app.name().to_owned();
    let container = app.container();
    let plugin_plan = app.plugin_plan().clone();

    PluginCommandContext {
        bootstrap,
        state: PluginBuiltState {
            name,
            container,
            plugin_plan,
            _owner: Box::new(app),
        },
    }
}
