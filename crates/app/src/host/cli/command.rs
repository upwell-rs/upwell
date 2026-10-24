use std::future::Future;

use crate::{
    App, AppHost, BootstrapContext, Built, CliError, PhaseError, PreBuild, PreparedApp, Setup,
    build_host_context, prepare_host_context, setup_host_context,
};

mod private {
    use std::future::Future;

    use crate::{AppHost, BootstrapContext, PhaseError};

    pub trait Sealed: Send + Sync + 'static {
        type State<H: AppHost>;

        fn prepare<H>(
            bootstrap: BootstrapContext,
        ) -> impl Future<Output = Result<(BootstrapContext, Self::State<H>), PhaseError>> + Send
        where
            H: AppHost,
            H::Protocol: Send,
            Self: Sized;
    }
}

/// The statically declared application lifecycle phase required by a CLI leaf command.
///
/// This trait is sealed. Commands select one of [`Setup`], [`PreBuild`], or [`Built`] as their
/// associated phase rather than implementing additional phases.
pub trait CliPhase: private::Sealed {}

impl private::Sealed for Setup {
    type State<H: AppHost> = ();

    #[allow(clippy::manual_async_fn)]
    fn prepare<H>(
        bootstrap: BootstrapContext,
    ) -> impl Future<Output = Result<(BootstrapContext, Self::State<H>), PhaseError>> + Send
    where
        H: AppHost,
        H::Protocol: Send,
    {
        async move {
            let bootstrap = setup_host_context::<H>(bootstrap).await?;

            Ok((bootstrap, ()))
        }
    }
}

impl CliPhase for Setup {}

impl private::Sealed for PreBuild {
    type State<H: AppHost> = PreparedApp<H::Protocol>;

    #[allow(clippy::manual_async_fn)]
    fn prepare<H>(
        bootstrap: BootstrapContext,
    ) -> impl Future<Output = Result<(BootstrapContext, Self::State<H>), PhaseError>> + Send
    where
        H: AppHost,
        H::Protocol: Send,
    {
        async move {
            let (bootstrap, app) = prepare_host_context::<H>(bootstrap).await?;

            Ok((bootstrap, app))
        }
    }
}

impl CliPhase for PreBuild {}

impl private::Sealed for Built {
    type State<H: AppHost> = App<H::Protocol>;

    #[allow(clippy::manual_async_fn)]
    fn prepare<H>(
        bootstrap: BootstrapContext,
    ) -> impl Future<Output = Result<(BootstrapContext, Self::State<H>), PhaseError>> + Send
    where
        H: AppHost,
        H::Protocol: Send,
    {
        async move {
            let (bootstrap, app) = build_host_context::<H>(bootstrap).await?;

            Ok((bootstrap, app))
        }
    }
}

impl CliPhase for Built {}

/// A fully parsed CLI leaf command dispatched by a generated application host.
pub trait CliCommand<H>: Sync
where
    H: AppHost,
    H::Protocol: Send,
{
    /// The statically required application lifecycle phase.
    type Phase: CliPhase;

    /// The typed failure returned by this command.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Executes this parsed command against its statically selected application state.
    fn run(
        &self,
        context: CommandContext<H, Self::Phase>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Lifecycle-aware application state supplied to a parsed CLI leaf command.
///
/// The phase parameter controls which state accessors exist. A setup command cannot compile if it
/// attempts to access configured or built application state:
///
/// ```compile_fail
/// # use upwell_app::{AppHost, CommandContext, Setup};
/// # fn invalid<H: AppHost>(context: CommandContext<H, Setup>) {
/// let _ = context.prepared();
/// # }
/// ```
///
/// Likewise, dependency resolution is available only to commands declaring [`Built`]:
///
/// ```compile_fail
/// # use upwell_app::{AppHost, CommandContext, PreBuild};
/// # fn invalid<H: AppHost>(context: CommandContext<H, PreBuild>) {
/// let _ = context.resolve::<String>();
/// # }
/// ```
pub struct CommandContext<H: AppHost, P: CliPhase> {
    bootstrap: BootstrapContext,
    state: <P as private::Sealed>::State<H>,
}

impl<H: AppHost, P: CliPhase> CommandContext<H, P> {
    fn new(bootstrap: BootstrapContext, state: <P as private::Sealed>::State<H>) -> Self {
        Self { bootstrap, state }
    }

    /// Global bootstrap state and typed global argument groups.
    pub const fn bootstrap(&self) -> &BootstrapContext {
        &self.bootstrap
    }

    /// Mutable global bootstrap state and typed global argument groups.
    pub fn bootstrap_mut(&mut self) -> &mut BootstrapContext {
        &mut self.bootstrap
    }

    /// Borrows a required typed bootstrap value.
    ///
    /// # Errors
    ///
    /// Returns [`CommandContextError::MissingValue`] when no value of type `T` is present.
    pub fn require<T: Send + Sync + 'static>(&self) -> Result<&T, CommandContextError> {
        self.bootstrap
            .get::<T>()
            .ok_or(CommandContextError::MissingValue {
                type_name: std::any::type_name::<T>(),
            })
    }
}

/// Prepares a context at one of the framework's sealed CLI phases.
#[doc(hidden)]
pub async fn prepare_cli_context<H, P>(
    bootstrap: BootstrapContext,
) -> Result<CommandContext<H, P>, PhaseError>
where
    H: AppHost,
    H::Protocol: Send,
    P: CliPhase,
{
    let (bootstrap, state) = P::prepare::<H>(bootstrap).await?;

    Ok(CommandContext::new(bootstrap, state))
}

impl<H: AppHost> CommandContext<H, Setup> {
    pub(crate) fn into_bootstrap(self) -> BootstrapContext {
        self.bootstrap
    }
}

impl<H: AppHost> CommandContext<H, PreBuild> {
    /// The validated prepared application guaranteed by this context's phase.
    pub const fn prepared(&self) -> &PreparedApp<H::Protocol> {
        &self.state
    }

    pub(crate) fn into_parts(self) -> (BootstrapContext, PreparedApp<H::Protocol>) {
        (self.bootstrap, self.state)
    }
}

impl<H: AppHost> CommandContext<H, Built> {
    /// The constructed application guaranteed by this context's phase.
    pub const fn app(&self) -> &App<H::Protocol> {
        &self.state
    }

    /// Resolves an `Injectable` from the built application's root DI container.
    pub fn resolve<T>(&self) -> impl Future<Output = Result<T, upwell_di::Error>> + Send + use<H, T>
    where
        T: upwell_di::Injectable,
    {
        let container = self.state.container().clone();

        async move {
            container
                .resolve::<T>()
                .await?
                .ok_or_else(|| upwell_di::Error::MissingDependency {
                    component: std::any::type_name::<H>().to_string(),
                    component_id: std::any::type_name::<H>().to_string(),
                    dependency: std::any::type_name::<T>().to_string(),
                    type_name: std::any::type_name::<T>().to_string(),
                })
        }
    }

    /// Consumes a built context for generated framework serve dispatch.
    #[doc(hidden)]
    pub fn into_parts(self) -> (BootstrapContext, App<H::Protocol>) {
        (self.bootstrap, self.state)
    }
}

/// Prepares and dispatches one statically typed application CLI leaf.
#[doc(hidden)]
pub async fn dispatch_cli_command<H, C>(
    command: &C,
    bootstrap: BootstrapContext,
    command_path: &'static str,
) -> Result<(), CliError>
where
    H: AppHost,
    H::Protocol: Send,
    C: CliCommand<H>,
{
    let context = prepare_cli_context::<H, C::Phase>(bootstrap).await?;

    command
        .run(context)
        .await
        .map_err(|source| CommandError::new(command_path, source))?;

    Ok(())
}

/// A command required a typed bootstrap value that was not parsed or inserted.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommandContextError {
    /// A required typed bootstrap value was not available.
    #[error("command context is missing required bootstrap value '{type_name}'")]
    MissingValue {
        /// The missing concrete Rust type name.
        type_name: &'static str,
    },
}

/// A typed leaf-command failure annotated with its complete CLI path.
#[derive(Debug, thiserror::Error)]
#[error("command `{command}` failed: {source}")]
pub struct CommandError {
    command: String,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl CommandError {
    /// Wraps a typed command failure with the command path users invoked.
    pub fn new(
        command: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            command: command.into(),
            source: Box::new(source),
        }
    }

    pub(crate) fn boxed(
        command: impl Into<String>,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            command: command.into(),
            source,
        }
    }

    /// The complete space-separated CLI command path.
    pub fn command(&self) -> &str {
        &self.command
    }
}

#[cfg(test)]
mod tests;
