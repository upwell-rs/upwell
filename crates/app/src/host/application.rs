use std::future::Future;

use super::{BootstrapContext, ExecutionMode, HostError, LifecyclePhase, PhaseError};
use crate::{
    App, AppBuilder, ApplicationPluginRegistrar, EarlyPluginCatalog, PreparedApp,
    ProtocolDefinition, ProtocolPluginRegistrar,
};

use crate::app::HostLifecycleCapabilities;

/// Static lifecycle definition implemented by every generated named application.
///
/// The trait contains only application-specific work. Framework orchestration lives in the
/// `*_host` functions below, so generated applications, custom runtimes, and CLI dispatch all use
/// the same ordering and error tagging.
pub trait AppHost {
    /// The single protocol definition selected by this host.
    type Protocol: ProtocolDefinition + Send;

    /// Whether framework bootstrap supplies the builder's config manager.
    ///
    /// When true, preparation moves the config manager from [`BootstrapContext`] into the
    /// [`AppBuilder`] before [`configure`](Self::configure) runs.
    const BOOTSTRAP_OWNS_CONFIG: bool = false;

    /// Whether framework bootstrap supplies the builder's directories manager.
    ///
    /// When true, preparation copies the resolved platform-native directories manager from
    /// [`BootstrapContext`] into the [`AppBuilder`] before [`configure`](Self::configure) runs.
    const BOOTSTRAP_OWNS_DIRECTORIES: bool = false;

    /// Generated host lifecycle callback capabilities retained for tooling projection.
    #[doc(hidden)]
    const LIFECYCLE_CAPABILITIES: HostLifecycleCapabilities =
        HostLifecycleCapabilities::new(false, false, false, false, false);

    /// Creates the declaration-configured protocol-specific application builder.
    ///
    /// Generated implementations apply the app name, protocol, discovered services and
    /// components, explicit config bindings, manager overrides, middleware, guards, and error
    /// handler declared in `app!`.
    fn builder() -> Result<AppBuilder<Self::Protocol>, upwell_config::ConfigError>;

    /// Declares parser-visible application plugins and protocol-default slot directives.
    ///
    /// This static declaration seam is available before CLI parser construction. Dynamic
    /// [`configure`](Self::configure) and [`before_build`](Self::before_build) hooks may only add
    /// monotonic late plugins through [`AppBuilder::register_plugin`].
    fn declare_plugins(_plugins: &mut ApplicationPluginRegistrar) {}

    /// Runs the application-specific setup hook.
    ///
    /// Setup receives the execution mode and any state already resolved by CLI bootstrap. It may
    /// add typed global values or tracing layers to the context. The framework finalizes tracing
    /// only after this method succeeds.
    fn setup(
        context: BootstrapContext,
    ) -> impl Future<Output = Result<BootstrapContext, PhaseError>> + Send {
        async { Ok(context) }
    }

    /// Applies application-specific configuration to the assembled builder.
    ///
    /// Framework-owned directories and config managers have already been applied when this method
    /// runs. It may register additional components, plugins, bindings, middleware, guards, or
    /// other builder state. No registry validation or component construction has happened yet.
    fn configure(
        _context: &mut BootstrapContext,
        builder: AppBuilder<Self::Protocol>,
    ) -> impl Future<Output = Result<AppBuilder<Self::Protocol>, PhaseError>> + Send {
        async { Ok(builder) }
    }

    /// Applies final application-specific builder changes immediately before validation.
    ///
    /// This is the final opportunity to modify the builder. After it returns, the framework calls
    /// [`AppBuilder::prepare`], which registers protocol and framework components, resolves
    /// platform directories and configuration, validates the registry and scopes, builds config
    /// slots, and computes component construction plans without constructing ordinary components.
    fn before_build(
        _context: &mut BootstrapContext,
        builder: AppBuilder<Self::Protocol>,
    ) -> impl Future<Output = Result<AppBuilder<Self::Protocol>, PhaseError>> + Send {
        async { Ok(builder) }
    }

    /// Applies application-specific changes after construction.
    ///
    /// The root DI container, hook manager, root resolver, application runtime, and protocol are
    /// fully constructed before this method runs. Returning the app completes the [`Built`] stage;
    /// serving and startup hooks have not begun.
    fn after_build(
        _context: &mut BootstrapContext,
        app: App<Self::Protocol>,
    ) -> impl Future<Output = Result<App<Self::Protocol>, PhaseError>> + Send {
        async { Ok(app) }
    }

    /// Runs the application-specific serving phase.
    ///
    /// The built app and lifecycle context are consumed. Generated inline serve phases resolve
    /// their typed `Injectable` parameters from the built root container before entering the user
    /// body. The user body chooses and starts the concrete transport or runtime. Hosts without a
    /// declared serve phase return [`HostError::ServeUnavailable`].
    fn serve(
        _context: BootstrapContext,
        _app: App<Self::Protocol>,
    ) -> impl Future<Output = Result<(), PhaseError>> + Send {
        async {
            Err(PhaseError::new(
                LifecyclePhase::Serve,
                HostError::ServeUnavailable,
            ))
        }
    }
}

/// Resolves and retains every parser-visible protocol and application plugin exactly once.
#[doc(hidden)]
pub fn resolve_host_plugin_catalog<H: AppHost>() -> crate::Result<EarlyPluginCatalog> {
    let mut protocol = ProtocolPluginRegistrar::new(H::Protocol::ID);
    let mut application = ApplicationPluginRegistrar::new();

    H::Protocol::register_plugins(&mut protocol);
    H::declare_plugins(&mut application);

    EarlyPluginCatalog::resolve(protocol, application)
}

/// Transfers a parser-visible catalog into bootstrap lifecycle state.
#[doc(hidden)]
pub fn retain_host_plugin_catalog(context: &mut BootstrapContext, catalog: EarlyPluginCatalog) {
    context.set_plugin_catalog(catalog);
}

/// Maps a generated application's compile-time stage to the state it owns.
///
/// The generated application stores `Stage::State` directly. A stage marker therefore changes
/// both the available methods and the data physically held by the value; lifecycle validity is not
/// represented by a runtime enum or checked dynamically.
pub trait AppStage<D: ProtocolDefinition>: Send + Sync + 'static {
    /// Data available while the generated application is in this stage.
    type State;
}

/// Initial stage before application lifecycle work begins.
///
/// Stores only [`ExecutionMode`]. Direct runtime construction has not resolved CLI options,
/// directories, config, logging, or profiles. Generated CLI dispatch resolves those first and then
/// enters the same context-based lifecycle runners.
pub struct Initial;

/// Stage after application setup and framework bootstrap finalization.
///
/// Stores [`BootstrapContext`]. The setup hook has completed and any CLI-provided tracing state has
/// been installed. No `AppBuilder` has been prepared, no registry has been validated, and no
/// components or protocol have been constructed.
pub struct Setup;

/// Stage after registration, configuration resolution, and validation.
///
/// Stores `(BootstrapContext, PreparedApp<P>)`. Framework and protocol descriptors are registered;
/// directories and config bindings are resolved; the registry, provider graph, scopes, and config
/// values are validated; and component construction orders are computed. Ordinary components, the
/// root container, runtime, and protocol are not constructed yet. Tooling mode may stop here.
pub struct PreBuild;

/// Stage after component and protocol construction.
///
/// Stores `(BootstrapContext, App<P>)`. Singleton components and the root container are built,
/// hooks and the root resolver are attached, `AppRuntime` is created, and the protocol definition is
/// finalized. The app can resolve DI dependencies or be handed to a runtime, but serving and
/// startup hooks have not started until the serve implementation does so.
pub struct Built;

impl<D: ProtocolDefinition> AppStage<D> for Initial {
    type State = ExecutionMode;
}

impl<D: ProtocolDefinition> AppStage<D> for Setup {
    type State = BootstrapContext;
}

impl<D: ProtocolDefinition> AppStage<D> for PreBuild {
    type State = (BootstrapContext, PreparedApp<D>);
}

impl<D: ProtocolDefinition> AppStage<D> for Built {
    type State = (BootstrapContext, App<D>);
}

/// Creates an empty context for `mode`, runs [`AppHost::setup`], and finalizes bootstrap tracing.
///
/// This direct runtime entry does not parse CLI options or pre-load CLI bootstrap state. Generated
/// CLI dispatch uses [`setup_host_context`] with an already resolved context instead.
pub async fn setup_host<H: AppHost>(mode: ExecutionMode) -> Result<BootstrapContext, PhaseError> {
    setup_host_context::<H>(BootstrapContext::new(mode)).await
}

/// Runs [`AppHost::setup`] with an existing context, then finalizes bootstrap tracing.
pub async fn setup_host_context<H: AppHost>(
    context: BootstrapContext,
) -> Result<BootstrapContext, PhaseError> {
    let context = H::setup(context).await?;

    #[cfg(feature = "cli")]
    let context = {
        let mut context = context;

        super::finalize_bootstrap(&mut context)
            .map_err(|source| PhaseError::new(LifecyclePhase::Setup, source))?;

        context
    };

    Ok(context)
}

/// Runs setup and preparation from an empty context for `mode`.
///
/// This is an explicit fast-forward operation equivalent to [`setup_host`] followed by
/// [`prepare_setup_host_context`]. It returns the preserved context and validated [`PreparedApp`]
/// without constructing ordinary components or the protocol.
pub async fn prepare_host<H: AppHost>(
    mode: ExecutionMode,
) -> Result<(BootstrapContext, PreparedApp<H::Protocol>), PhaseError> {
    prepare_host_context::<H>(BootstrapContext::new(mode)).await
}

/// Runs setup, preparation, and construction from an empty context for `mode`.
///
/// This explicit fast-forward operation executes every intermediate hook and validation step. It
/// rejects [`ExecutionMode::Tooling`] before constructing components.
pub async fn build_host<H: AppHost>(
    mode: ExecutionMode,
) -> Result<(BootstrapContext, App<H::Protocol>), PhaseError> {
    build_host_context::<H>(BootstrapContext::new(mode)).await
}

/// Runs setup and preparation from an existing bootstrap context.
///
/// CLI dispatch uses this when a command requires validated configuration and registry metadata
/// but must not construct components or the protocol.
pub async fn prepare_host_context<H: AppHost>(
    context: BootstrapContext,
) -> Result<(BootstrapContext, PreparedApp<H::Protocol>), PhaseError> {
    let context = setup_host_context::<H>(context).await?;

    prepare_setup_host_context::<H>(context).await
}

/// Prepares a host whose setup stage has already completed.
///
/// This creates the declaration-configured builder, applies framework-owned directories and config
/// state, runs [`AppHost::configure`] and [`AppHost::before_build`], then calls
/// [`AppBuilder::prepare`]. It does not construct ordinary components or the protocol.
pub async fn prepare_setup_host_context<H: AppHost>(
    mut context: BootstrapContext,
) -> Result<(BootstrapContext, PreparedApp<H::Protocol>), PhaseError> {
    let plugins = match context.take_plugin_catalog() {
        Some(plugins) => plugins,
        None => resolve_host_plugin_catalog::<H>()
            .map_err(|source| PhaseError::new(LifecyclePhase::Prepare, source))?,
    };
    let builder =
        H::builder().map_err(|source| PhaseError::new(LifecyclePhase::Configure, source))?;
    let builder = builder.with_early_plugin_catalog(plugins);

    #[cfg(feature = "cli")]
    let builder = {
        let mut builder = builder;

        if H::BOOTSTRAP_OWNS_DIRECTORIES {
            builder = super::configure_bootstrap_directories(&mut context, builder);
        }

        if H::BOOTSTRAP_OWNS_CONFIG {
            builder = super::configure_bootstrap_config(&mut context, builder);
        }

        builder
    };

    let builder = H::configure(&mut context, builder).await?;
    let builder = H::before_build(&mut context, builder).await?;
    let prepared = builder
        .prepare()
        .map_err(|source| PhaseError::new(LifecyclePhase::Prepare, source))?;

    #[cfg(feature = "tooling")]
    let prepared = {
        let mut prepared = prepared;

        prepared.retain_host_lifecycle(H::LIFECYCLE_CAPABILITIES);

        prepared
    };

    Ok((context, prepared))
}

/// Runs setup, preparation, and construction from an existing bootstrap context.
///
/// CLI dispatch uses this for built commands and the generated serve command. Tooling mode is
/// rejected before any ordinary component or protocol construction begins.
pub async fn build_host_context<H: AppHost>(
    context: BootstrapContext,
) -> Result<(BootstrapContext, App<H::Protocol>), PhaseError> {
    let (context, prepared) = prepare_host_context::<H>(context).await?;

    build_prepared_host::<H>(context, prepared).await
}

/// Builds a host whose registration and validation have completed.
///
/// This constructs singleton components and the root DI container, attaches hooks and the root
/// resolver, creates the runtime, constructs the protocol runtime, and runs
/// [`AppHost::after_build`]. It does not invoke [`AppHost::serve`].
pub async fn build_prepared_host<H: AppHost>(
    mut context: BootstrapContext,
    prepared: PreparedApp<H::Protocol>,
) -> Result<(BootstrapContext, App<H::Protocol>), PhaseError> {
    if context.mode().is_tooling() {
        return Err(tooling_construction_error());
    }

    let app = prepared
        .build()
        .await
        .map_err(|source| PhaseError::new(LifecyclePhase::Build, source))?;
    let app = H::after_build(&mut context, app).await?;

    Ok((context, app))
}

/// Consumes a built host and invokes [`AppHost::serve`].
///
/// All setup, validation, construction, and `after_build` work is complete before this function is
/// called. Transport startup, startup hooks, shutdown waiting, and protocol serving are controlled
/// by the host's serve implementation.
pub async fn serve_host<H: AppHost>(
    context: BootstrapContext,
    app: App<H::Protocol>,
) -> Result<(), PhaseError> {
    H::serve(context, app).await
}

/// Resolves one typed dependency from a built application's root container.
///
/// Resolution supports both already-built handles and transient construction through the normal DI
/// engine. The root container handle is shared into the returned `Send` future; the built app and
/// protocol are not cloned or borrowed across the await.
pub fn resolve_host_dependency<P, H>(
    app: &App<P>,
    consumer: &str,
) -> impl Future<Output = Result<H, upwell_di::Error>> + Send
where
    P: ProtocolDefinition,
    H: upwell_di::Injectable,
{
    let container = app.container();
    let consumer = consumer.to_string();

    async move {
        container
            .resolve::<H>()
            .await?
            .ok_or_else(|| upwell_di::Error::MissingDependency {
                component: consumer.clone(),
                component_id: consumer,
                dependency: std::any::type_name::<H>().to_string(),
                type_name: std::any::type_name::<H>().to_string(),
            })
    }
}

fn tooling_construction_error() -> PhaseError {
    PhaseError::new(LifecyclePhase::Build, HostError::ToolingConstruction)
}
