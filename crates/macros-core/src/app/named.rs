use proc_macro2::TokenStream;
use quote::quote;

use super::NamedApp;
use super::builder;
#[cfg(feature = "cli")]
use super::cli::{self, CliInput};
use super::phase;
use crate::paths::Paths;

mod serve;
#[cfg(feature = "tooling")]
mod tooling;

/// Expands a reusable typed-state application host and its lifecycle hooks.
pub(super) fn expand(input: NamedApp) -> TokenStream {
    let NamedApp {
        attributes,
        visibility,
        ident,
        mut assembly,
    } = input;
    let phases = std::mem::take(&mut assembly.phases);
    let cli_policy = std::mem::take(&mut assembly.cli_policy);
    #[cfg(feature = "cli")]
    let cli_declarations = std::mem::take(&mut assembly.cli);
    let has_config_manager = assembly.config_manager.is_some();
    let has_directories_manager = assembly.directories_manager.is_some();
    let has_setup_callback = phases.setup.is_some();
    let has_configure_callback = phases.configure.is_some();
    let has_before_build_callback = phases.before_build.is_some();
    let has_after_build_callback = phases.after_build.is_some();
    let has_serve_callback = phases.serve.is_some();
    let paths = Paths::upwell().resolve(assembly.upwell.take(), None);
    let protocol = &assembly.protocol.value;
    let protocol_associated_type = syn::Ident::new(
        "Protocol",
        proc_macro2::Span::call_site().located_at(assembly.protocol.key.span()),
    );
    let setup_method = phases
        .setup
        .as_ref()
        .map(|phase| phase.key.clone())
        .unwrap_or_else(|| quote::format_ident!("setup", span = ident.span()));
    let configure_method = phases
        .configure
        .as_ref()
        .map(|phase| phase.key.clone())
        .unwrap_or_else(|| quote::format_ident!("configure", span = ident.span()));
    let before_build_method = phases
        .before_build
        .as_ref()
        .map(|phase| phase.key.clone())
        .unwrap_or_else(|| quote::format_ident!("before_build", span = ident.span()));
    let after_build_method = phases
        .after_build
        .as_ref()
        .map(|phase| phase.key.clone())
        .unwrap_or_else(|| quote::format_ident!("after_build", span = ident.span()));
    let plugin_declarations = assembly
        .plugins
        .iter()
        .map(|directive| match directive {
            super::model::PluginDirective::Install(plugin) => {
                quote!(plugins.register::<#plugin>();)
            }
            super::model::PluginDirective::Replace { slot, plugin } => {
                quote!(plugins.replace_with::<#plugin>(#slot);)
            }
            super::model::PluginDirective::Suppress(slot) => quote!(plugins.suppress(#slot);),
        })
        .collect::<Vec<_>>();
    let application_plugin_registrar = paths.core("ApplicationPluginRegistrar");
    let app = paths.core("App");
    let app_builder = paths.core("AppBuilder");
    let app_host = paths.core("AppHost");
    let host_lifecycle_capabilities = paths.core("HostLifecycleCapabilities");
    let app_stage = paths.core("AppStage");
    let bootstrap_context = paths.core("BootstrapContext");
    let build_host = paths.core("build_host");
    let build_prepared_host = paths.core("build_prepared_host");
    let built = paths.core("Built");
    let config_error = paths.core("ConfigError");
    let execution_mode = paths.core("ExecutionMode");
    let initial = paths.core("Initial");
    let lifecycle_phase = paths.core("LifecyclePhase");
    let phase_error = paths.core("PhaseError");
    let pre_build = paths.core("PreBuild");
    let prepared_app = paths.core("PreparedApp");
    let prepare_host = paths.core("prepare_host");
    let prepare_setup_host_context = paths.core("prepare_setup_host_context");
    let resolve_host_dependency = paths.core("resolve_host_dependency");
    let di_error = paths.core("DiError");
    let injectable = paths.core("Injectable");
    let serve_host = paths.core("serve_host");
    let setup = paths.core("Setup");
    let setup_host = paths.core("setup_host");
    let setup_call = phase::result(
        phases.setup.as_ref(),
        quote!(context),
        &[quote!(context)],
        quote!(#lifecycle_phase::Setup),
        &phase_error,
    );
    #[cfg_attr(not(feature = "cli"), allow(unused_variables))]
    let policy = match super::policy::expand(
        &cli_policy,
        phases.serve.as_ref(),
        &visibility,
        &ident,
        &paths,
    ) {
        Ok(policy) => policy,
        Err(error) => return error.into_compile_error(),
    };
    let configure_call = phase::call(
        phases.configure.as_ref(),
        quote!(builder),
        &[quote!(context), quote!(builder)],
        quote!(#lifecycle_phase::Configure),
        &phase_error,
    );
    let before_build_call = phase::call(
        phases.before_build.as_ref(),
        quote!(builder),
        &[quote!(context), quote!(builder)],
        quote!(#lifecycle_phase::BeforeBuild),
        &phase_error,
    );
    let after_build_call = phase::call(
        phases.after_build.as_ref(),
        quote!(app),
        &[quote!(context), quote!(app)],
        quote!(#lifecycle_phase::AfterBuild),
        &phase_error,
    );
    let serve_impl = serve::expand(
        phases.serve.as_ref(),
        &ident,
        protocol,
        &app,
        &bootstrap_context,
        &lifecycle_phase,
        &phase_error,
        &resolve_host_dependency,
    );
    let initial_serve = phases.serve.as_ref().map(|_| {
        quote! {
            /// Executes the complete lifecycle from the initial execution mode through serving.
            ///
            /// This creates a `BootstrapContext`, runs the declared `setup` hook, and finalizes
            /// tracing contributed during setup. It then creates the declaration-configured
            /// builder, applies framework-owned directories and config managers, runs `configure`
            /// and `before_build`, and prepares the application. Builder creation immediately
            /// collects link-time components, providers, config bindings, and protocol variants;
            /// preparation resolves those discovered defaults against explicit component/config
            /// declarations, registers framework and protocol descriptors, resolves platform
            /// directories and configuration, validates the DI graph, providers, scopes, protocol,
            /// and config values, and computes construction plans.
            ///
            /// After preparation, this constructs singleton components and the root DI container,
            /// attaches hooks and the root resolver, creates `AppRuntime`, finalizes the protocol,
            /// and runs `after_build`. Finally it resolves any typed inline serve parameters from
            /// the built root container and consumes the context and app in the declared serve
            /// body. No lifecycle stage is skipped despite this single-call fast-forward.
            ///
            /// # Errors
            ///
            /// Returns `PhaseError` tagged with the boundary that failed: `Setup` for setup or
            /// tracing finalization; `Configure` or `BeforeBuild` for builder hooks; `Prepare` for
            /// directory/config loading, registry/provider/scope/config/protocol validation, or
            /// dependency planning; `Build` for tooling rejection, DI construction, runtime, or
            /// protocol finalization; `AfterBuild` for that hook; and `Serve` for injected serve
            /// dependencies or the declared serve body.
            pub async fn serve(self) -> ::core::result::Result<(), #phase_error> {
                self.build().await?.serve().await
            }
        }
    });
    let setup_serve = phases.serve.as_ref().map(|_| {
        quote! {
            /// Executes preparation, construction, and serving from an already completed setup.
            ///
            /// The existing `BootstrapContext` is preserved. This creates the declaration-configured
            /// builder, applies framework-owned directories and config managers, runs `configure`
            /// and `before_build`. Builder creation immediately collects link-time components,
            /// providers, config bindings, and protocol variants; preparation resolves discovered
            /// defaults against explicit declarations, registers framework and protocol
            /// descriptors, resolves config, validates DI providers, scopes, graph, protocol, and
            /// config values, and computes construction plans.
            ///
            /// It then constructs singleton components and the root container, attaches hooks and
            /// the root resolver, creates `AppRuntime`, finalizes the protocol, runs `after_build`,
            /// resolves typed inline serve parameters through DI, and consumes the built app in the
            /// declared serve body. The setup hook and tracing finalization are not repeated.
            ///
            /// # Errors
            ///
            /// Returns `Configure`, `BeforeBuild`, `Prepare`, `Build`, `AfterBuild`, or `Serve`
            /// `PhaseError` values for the corresponding boundary. `Setup` cannot fail here because
            /// setup and tracing finalization already completed and are not repeated.
            pub async fn serve(self) -> ::core::result::Result<(), #phase_error> {
                self.build().await?.serve().await
            }
        }
    });
    let pre_build_serve = phases.serve.as_ref().map(|_| {
        quote! {
            /// Executes construction and serving from an already prepared and validated app.
            ///
            /// This consumes the stored `PreparedApp`, constructs singleton components and the
            /// root DI container in the previously validated order, attaches component hooks and
            /// the root resolver, creates `AppRuntime`, constructs the protocol runtime, and runs
            /// `after_build`. It then resolves typed inline serve parameters from the built root
            /// container and consumes the lifecycle context and app in the declared serve body.
            /// Setup, builder configuration, auto-discovery, config resolution, and graph
            /// validation are not repeated.
            ///
            /// # Errors
            ///
            /// Returns `Build` for tooling rejection, DI/root-container construction, runtime
            /// creation, or protocol finalization; `AfterBuild` for that hook; and `Serve` for
            /// injected dependency resolution or the serve body. Preparation errors cannot occur
            /// because this stage already owns a validated `PreparedApp`.
            pub async fn serve(self) -> ::core::result::Result<(), #phase_error> {
                self.build().await?.serve().await
            }
        }
    });
    let built_serve = phases.serve.as_ref().map(|_| {
        quote! {
            /// Executes only the application-defined serve phase for an already built app.
            ///
            /// This consumes the stored `BootstrapContext` and built `App`. For an inline serve
            /// declaration, each additional typed parameter is resolved through the root DI
            /// container before the body starts; resolution may construct transient dependencies.
            /// The serve body is responsible for selecting and starting its transport or runtime,
            /// running the protocol's serve envelope, and waiting for shutdown. Setup, preparation,
            /// DI graph validation, component construction, and `after_build` are not repeated.
            ///
            /// # Errors
            ///
            /// Returns `PhaseError` tagged `Serve` when a typed inline dependency cannot be
            /// resolved or when the declared serve body returns an error.
            pub async fn serve(self) -> ::core::result::Result<(), #phase_error> {
                let (context, app) = self.state;

                #serve_host::<#ident<#initial>>(context, app).await
            }
        }
    });

    #[cfg(feature = "cli")]
    let cli = match cli::expand(CliInput {
        visibility: &visibility,
        ident: &ident,
        attributes: &attributes,
        application_name: &assembly.name.value,
        paths: &paths,
        declarations: &cli_declarations,
        policy: &policy,
    }) {
        Ok(cli) => cli,
        Err(error) => return error.into_compile_error(),
    };

    #[cfg(not(feature = "cli"))]
    let cli = TokenStream::new();

    #[cfg(feature = "tooling")]
    let tooling = match tooling::expand(&ident, &assembly.name.value, &paths) {
        Ok(tooling) => tooling,
        Err(error) => return error.into_compile_error(),
    };

    #[cfg(not(feature = "tooling"))]
    let tooling = TokenStream::new();

    let builder = builder::expand_with_paths(&assembly, &paths);

    quote! {
        #(#attributes)*
        #visibility struct #ident<Stage: #app_stage<#protocol> = #initial> {
            state: Stage::State,
        }

        impl<Stage: #app_stage<#protocol>> #ident<Stage> {
            /// Wraps framework or custom-runtime state in this compile-time application stage.
            ///
            /// `Stage::State` determines the exact required representation: `ExecutionMode` for
            /// `Initial`, `BootstrapContext` for `Setup`, `(BootstrapContext, PreparedApp<P>)` for
            /// `PreBuild`, and `(BootstrapContext, App<P>)` for `Built`. This function performs no
            /// lifecycle work, validation, registration, discovery, construction, or serving; the
            /// type system only permits state matching the selected stage.
            ///
            /// # Type parameters
            ///
            /// - `Stage` selects both the methods available on the resulting application and the
            ///   exact associated `Stage::State` accepted by this function.
            pub fn from_state(state: Stage::State) -> Self {
                Self { state }
            }

            /// Removes the compile-time application wrapper and returns this stage's exact state.
            ///
            /// This is intended for custom runtimes that manage storage or scheduling themselves.
            /// It performs no lifecycle transition and does not rerun setup, discovery,
            /// registration, validation, construction, or serving. Re-wrap the returned value with
            /// `from_state` only under the same `Stage` type.
            ///
            /// # Type parameters
            ///
            /// - `Stage` determines the associated state type returned by this method.
            pub fn into_state(self) -> Stage::State {
                self.state
            }
        }

        impl #ident<#initial> {
            /// Creates an initial application containing only the selected execution mode.
            ///
            /// This performs no CLI parsing or bootstrap. It does not resolve platform directories,
            /// load configuration, install tracing, create an `AppBuilder`, auto-discover or
            /// register descriptors, validate the DI graph, construct components, finalize the
            /// protocol, or start serving. Generated CLI dispatch performs its own option/config
            /// bootstrap before entering the same context-based lifecycle runners.
            pub fn new(mode: #execution_mode) -> Self {
                Self { state: mode }
            }

            /// Creates a fresh `AppBuilder` from this application's `app!` declaration.
            ///
            /// The builder receives the declared name and protocol and enables link-time
            /// auto-discovery. Calling `auto_discover` immediately collects all link-time component,
            /// provider, and config-binding descriptors into the builder registry and asks the
            /// protocol definition to collect its link-time variants, such as RPC service descriptors.
            /// Every expression in `components` is then registered as an explicit pre-built
            /// instance; every `Type => "path"` entry in `configs` becomes an explicit config
            /// binding; explicit config or directories managers, middleware, guards, and the error
            /// handler are applied. Explicit registrations retain declaration order and are
            /// resolved against discovered defaults during preparation.
            ///
            /// The `services` list does not register runtime services. Under `di-check`, it emits
            /// compile-time `Wired` assertions for the listed service dependency graphs. Runtime
            /// protocol services come from the protocol definition's link-time auto-discovery.
            ///
            /// This method does not run setup/configure hooks, load default platform config,
            /// collect auto-discovered config bindings, register protocol/framework descriptors,
            /// validate the DI graph or config, construct components, or finalize the protocol.
            ///
            /// # Errors
            ///
            /// Returns `ConfigError` when a declaration-configured manager must load a config
            /// source and that source cannot be read or parsed. Pure in-memory builder assembly and
            /// descriptor collection do not perform runtime graph validation here.
            pub fn builder() -> ::core::result::Result<#app_builder<#protocol>, #config_error> {
                ::core::result::Result::Ok(
                    Self::__builder()?.with_plugin_declarations(|plugins| {
                        #(#plugin_declarations)*
                    })
                )
            }

            fn __builder() -> ::core::result::Result<#app_builder<#protocol>, #config_error> {
                ::core::result::Result::Ok(#builder)
            }

            /// Executes the setup stage and returns `Application<Setup>`.
            ///
            /// This creates an empty `BootstrapContext` containing the execution mode, invokes the
            /// declared `setup` hook, preserves typed values and tracing layers inserted by that
            /// hook, and finalizes tracing after the hook succeeds. Direct runtime use does not
            /// parse CLI options, resolve platform directories, or load CLI-selected config first;
            /// generated CLI dispatch supplies that state before calling the same setup runner.
            ///
            /// No `AppBuilder` is created. Auto-discovery, explicit component/config registration,
            /// framework/protocol registration, config resolution, DI validation, component
            /// construction, protocol finalization, `after_build`, and serving have not run.
            ///
            /// # Errors
            ///
            /// Returns `PhaseError` tagged `Setup` when the declared setup hook fails or generated
            /// bootstrap cannot finalize tracing. Direct `new(mode)` construction has no CLI
            /// directory/config bootstrap failure because it begins with an empty context.
            pub async fn setup(self) -> ::core::result::Result<#ident<#setup>, #phase_error> {
                let context = #setup_host::<Self>(self.state).await?;

                Ok(#ident::from_state(context))
            }

            /// Executes setup and the complete preparation stage, returning `Application<PreBuild>`.
            ///
            /// This first performs the same setup and tracing finalization as `setup()`. It then
            /// creates the declaration-configured builder. When managers were not explicitly
            /// declared, framework bootstrap supplies platform-native directories and the selected
            /// config manager. The declared `configure` and `before_build` hooks run before
            /// `AppBuilder::prepare`.
            ///
            /// Preparation folds in link-time auto-discovered protocol services, components, and
            /// path-bearing config types, combines them with explicitly declared components and
            /// config bindings, registers protocol-owned and framework built-in descriptors,
            /// resolves directory placeholders and config values, validates descriptors,
            /// providers, scopes, config bindings, and protocol configuration, and computes the
            /// per-scope construction order. Ordinary components, the root container, runtime, and
            /// finalized protocol are not constructed. `after_build` and serve do not run.
            ///
            /// # Errors
            ///
            /// Returns `Setup` for setup or tracing finalization; `Configure` for builder creation,
            /// bootstrap manager application, or the configure hook; `BeforeBuild` for the final
            /// builder hook; and `Prepare` for directory/config loading, duplicate or invalid
            /// descriptors/providers, missing dependencies, cycles, scope violations, config
            /// binding/deserialization, protocol validation, or construction-plan generation.
            pub async fn prepare(self) -> ::core::result::Result<#ident<#pre_build>, #phase_error> {
                let state = #prepare_host::<Self>(self.state).await?;

                Ok(#ident::from_state(state))
            }

            /// Executes setup, preparation, and construction, returning `Application<Built>`.
            ///
            /// Setup and preparation perform all work documented by `setup()` and `prepare()`,
            /// including declaration registration, link-time auto-discovery, config/directory
            /// resolution, protocol/framework registration, and complete DI/config validation.
            /// Construction then builds singleton components in validated dependency order, creates
            /// the root DI container, attaches component hooks and the `RootResolver`, creates
            /// `AppRuntime`, constructs the protocol runtime, and invokes `after_build`.
            ///
            /// This method does not invoke the declared serve phase, open a transport, run startup
            /// hooks, or wait for shutdown. Tooling mode is rejected before component or protocol
            /// construction begins.
            ///
            /// # Errors
            ///
            /// Propagates all `setup()` and `prepare()` failures with their original phase. Returns
            /// `Build` for tooling-mode rejection, component/root-container construction, runtime
            /// creation, or protocol finalization, and `AfterBuild` when that hook fails.
            pub async fn build(self) -> ::core::result::Result<#ident<#built>, #phase_error> {
                let state = #build_host::<Self>(self.state).await?;

                Ok(#ident::from_state(state))
            }

            #initial_serve
        }

        impl #ident<#setup> {
            /// Borrows the bootstrap context produced by the completed setup stage.
            ///
            /// It contains the execution mode and any typed values or tracing state contributed by
            /// framework bootstrap and the setup hook. No builder, auto-discovery, registration,
            /// config/DI validation, component construction, protocol finalization, or serving has
            /// occurred merely because this accessor is available.
            pub fn context(&self) -> &#bootstrap_context {
                &self.state
            }

            /// Executes the complete preparation stage from an already completed setup.
            ///
            /// This creates the declaration-configured builder, applies framework-owned
            /// platform directories and config managers when no explicit manager was declared,
            /// runs `configure` and `before_build`, and calls `AppBuilder::prepare`. Preparation
            /// combines explicit declarations with link-time auto-discovered protocol services,
            /// components, and path-bearing config types; registers protocol/framework built-ins;
            /// resolves config and directory placeholders; validates config, descriptors,
            /// providers, scopes, and dependency cycles; and computes construction plans.
            ///
            /// The setup hook and tracing finalization are not repeated. Ordinary components, the
            /// root container, runtime, and protocol are not constructed, and serving does not run.
            ///
            /// # Errors
            ///
            /// Returns `Configure` for builder creation, bootstrap manager application, or the
            /// configure hook; `BeforeBuild` for the final builder hook; and `Prepare` for
            /// directory/config loading, invalid or duplicate descriptors/providers, missing
            /// dependencies, cycles, scope violations, config binding/deserialization, protocol
            /// validation, or construction planning. Setup is not repeated and cannot fail here.
            pub async fn prepare(self) -> ::core::result::Result<#ident<#pre_build>, #phase_error> {
                let state = #prepare_setup_host_context::<#ident<#initial>>(self.state).await?;

                Ok(#ident::from_state(state))
            }

            /// Executes preparation and construction from an already completed setup.
            ///
            /// This performs every registration, auto-discovery, config/directory resolution,
            /// validation, and construction-planning step documented by `prepare()`. It then builds
            /// singleton components, creates the root DI container, attaches hooks and the root
            /// resolver, creates `AppRuntime`, constructs the protocol runtime, and runs
            /// `after_build`. Setup is not repeated. The serve phase, transport startup, startup
            /// hooks, and shutdown wait do not run.
            ///
            /// # Errors
            ///
            /// Propagates `Configure`, `BeforeBuild`, or `Prepare` failures from preparation.
            /// Returns `Build` for tooling-mode rejection, component/root-container construction,
            /// runtime creation, or protocol finalization, and `AfterBuild` for that hook.
            pub async fn build(self) -> ::core::result::Result<#ident<#built>, #phase_error> {
                self.prepare().await?.build().await
            }

            #setup_serve
        }

        impl #ident<#pre_build> {
            /// Borrows the bootstrap context preserved by the completed preparation stage.
            ///
            /// Setup and tracing finalization, builder configuration, manager application,
            /// auto-discovery, explicit and framework/protocol registration, config resolution,
            /// graph/scope/config validation, and construction planning have completed. Ordinary
            /// components, the root container, runtime, protocol, and serve phase have not.
            pub fn context(&self) -> &#bootstrap_context {
                &self.state.0
            }

            /// Borrows the prepared application before component and protocol construction.
            ///
            /// It contains the effective validated registry, resolved config slots and reload
            /// metadata, framework seed instances, prepared protocol state, scope registry, provider
            /// ordering, and component construction plans produced by auto-discovery plus explicit
            /// registration. Ordinary components, the root container, `AppRuntime`, and the
            /// finalized protocol do not exist yet.
            pub fn app(&self) -> &#prepared_app<#protocol> {
                &self.state.1
            }

            /// Consumes `Application<PreBuild>` into its bootstrap context and `PreparedApp`.
            ///
            /// This supports custom runtimes that want to store, inspect, or construct the
            /// validated app themselves. It performs no additional discovery, registration,
            /// config resolution, validation, component construction, protocol finalization, or
            /// serving; all preparation work represented by this stage has already completed.
            ///
            /// # Returns
            ///
            /// Returns the preserved `BootstrapContext` first and the validated `PreparedApp`
            /// second. Extraction performs no fallible lifecycle work.
            pub fn into_parts(self) -> (#bootstrap_context, #prepared_app<#protocol>) {
                self.state
            }

            /// Constructs the already prepared application and returns `Application<Built>`.
            ///
            /// This builds singleton components in the validated dependency order, creates the
            /// root DI container, attaches component hooks and the root resolver, creates
            /// `AppRuntime`, constructs the protocol runtime, and invokes `after_build`. It does not
            /// repeat setup, builder configuration, auto-discovery, registration, config
            /// resolution, or graph validation. It also does not invoke serve, open a transport,
            /// run startup hooks, or wait for shutdown. Tooling mode is rejected before build.
            ///
            /// # Errors
            ///
            /// Returns `PhaseError` tagged `Build` for tooling-mode rejection, component or
            /// dependency construction, root-container creation, runtime creation, or protocol
            /// finalization. Returns `AfterBuild` when the declared post-construction hook fails.
            /// Preparation errors cannot occur because this state is already validated.
            pub async fn build(self) -> ::core::result::Result<#ident<#built>, #phase_error> {
                let (context, app) = self.state;
                let state = #build_prepared_host::<#ident<#initial>>(context, app).await?;

                Ok(#ident::from_state(state))
            }

            #pre_build_serve
        }

        impl #ident<#built> {
            /// Borrows the bootstrap context preserved after setup, preparation, and construction.
            ///
            /// All setup/configure/before-build/after-build hooks, registration, auto-discovery,
            /// config resolution, validation, component construction, root-container creation,
            /// runtime creation, and protocol finalization have completed. Serving, transport
            /// startup, startup hooks, and shutdown waiting have not necessarily begun.
            pub fn context(&self) -> &#bootstrap_context {
                &self.state.0
            }

            /// Borrows the fully constructed application without starting it.
            ///
            /// The returned app owns the validated registry, built root DI container, attached
            /// hook manager and root resolver, completed `AppRuntime`, finalized protocol,
            /// shutdown signal, and config reloader. This accessor does not resolve extra
            /// dependencies, invoke serve, open a transport, run startup hooks, or consume state.
            pub fn app(&self) -> &#app<#protocol> {
                &self.state.1
            }

            /// Resolves an `Injectable` from the built root DI container.
            ///
            /// Existing singleton/config/dependency handles are returned through normal DI handle
            /// semantics; transient targets may be constructed for this resolution. Missing,
            /// ambiguous, or failed transient dependencies return typed `DiError` values naming
            /// this generated application as the consumer. The application remains built and is
            /// not consumed; serving, startup hooks, and shutdown handling do not begin.
            ///
            /// # Type parameters
            ///
            /// - `H` is the concrete value or handle requested from DI and must implement
            ///   `Injectable`. Config handles, component handles, trait-provider handles, and
            ///   supported transient targets use the same resolution rules as component factories.
            ///
            /// # Errors
            ///
            /// Returns `DiError` when the dependency is missing or ambiguous, a scope/root is
            /// unavailable, a stored value cannot be downcast to `H`, or fresh/transient
            /// dependency resolution or construction fails.
            pub async fn resolve<H>(&self) -> ::core::result::Result<H, #di_error>
            where
                H: #injectable,
            {
                #resolve_host_dependency(&self.state.1, stringify!(#ident)).await
            }

            /// Consumes `Application<Built>` into its bootstrap context and built `App`.
            ///
            /// This is the handoff for custom runtimes that want to choose transport, startup, or
            /// shutdown behavior themselves. It performs no dependency resolution, serving,
            /// transport setup, startup hooks, or shutdown wait. All setup, registration,
            /// validation, component construction, runtime creation, protocol finalization, and
            /// `after_build` work represented by this stage has already completed.
            ///
            /// # Returns
            ///
            /// Returns the preserved `BootstrapContext` first and the fully constructed `App`
            /// second. Extraction performs no fallible work and does not run shutdown behavior.
            pub fn into_parts(self) -> (#bootstrap_context, #app<#protocol>) {
                self.state
            }

            #built_serve
        }

        impl #app_host for #ident<#initial> {
            type #protocol_associated_type = #protocol;

            const BOOTSTRAP_OWNS_CONFIG: bool = #has_config_manager == false;
            const BOOTSTRAP_OWNS_DIRECTORIES: bool = #has_directories_manager == false;
            const LIFECYCLE_CAPABILITIES: #host_lifecycle_capabilities =
                #host_lifecycle_capabilities::new(
                    #has_setup_callback,
                    #has_configure_callback,
                    #has_before_build_callback,
                    #has_after_build_callback,
                    #has_serve_callback,
                );

            fn builder() -> ::core::result::Result<#app_builder<#protocol>, #config_error> {
                Self::__builder()
            }

            fn declare_plugins(plugins: &mut #application_plugin_registrar) {
                #(#plugin_declarations)*
            }

            async fn #setup_method(context: #bootstrap_context) -> ::core::result::Result<#bootstrap_context, #phase_error> {
                #setup_call
            }

            async fn #configure_method(
                context: &mut #bootstrap_context,
                builder: #app_builder<#protocol>,
            ) -> ::core::result::Result<#app_builder<#protocol>, #phase_error> {
                let builder = #configure_call;

                Ok(builder)
            }

            async fn #before_build_method(
                context: &mut #bootstrap_context,
                builder: #app_builder<#protocol>,
            ) -> ::core::result::Result<#app_builder<#protocol>, #phase_error> {
                let builder = #before_build_call;

                Ok(builder)
            }

            async fn #after_build_method(
                context: &mut #bootstrap_context,
                app: #app<#protocol>,
            ) -> ::core::result::Result<#app<#protocol>, #phase_error> {
                let app = #after_build_call;

                Ok(app)
            }

            #serve_impl
        }

        #cli

        #tooling
    }
}
