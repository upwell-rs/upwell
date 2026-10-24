//! # Upwell
//!
//! A component- and service-oriented framework. Depend on this single crate; it re-exports
//! the layered core — `upwell-core` (vocabulary + resolver), `upwell-di` (the DI
//! engine), `upwell-hooks`, `upwell-dirs`, `upwell-config`, and `upwell-app`
//! (the protocol-agnostic application core) — plus the procedural macros.
//!
//! The native RPC daemon lives in its own `upwell-rpc` crate and is re-exported here
//! behind the default-on **`daemon`** feature (disable it with `default-features = false`
//! for a core-framework-only build, or depend on `upwell-rpc` directly).

// ---------------------------------------------------------------------------
// Leaf vocabulary + resolver model.
// ---------------------------------------------------------------------------
#[cfg(not(target_family = "wasm"))]
pub use upwell_core::*;

/// Component lifetime scopes: the [`Scope`] trait a protocol's topology is built from,
/// plus the universal marker types. A protocol-scoped component selects a marker from that
/// protocol's module; a `#[component]` defaults to [`Singleton`].
///
/// The core defines only the universal anchors [`Singleton`] and [`Transient`]; `Connection` and `Request` are RPC-protocol scopes from
/// `upwell-rpc`, available with the `daemon` feature.
#[cfg(not(target_family = "wasm"))]
pub mod scope {
    pub use upwell_core::scope::{Singleton, Transient};

    #[cfg(feature = "daemon")]
    pub use upwell_rpc::scope::{Connection, Request}; // should not be here, should be in crate::daemon with the rest of the daemon related items
}

/// Component condition descriptors, callback inputs, and [`resolve`](condition::resolve),
/// the raw config read a condition callback may make.
#[cfg(not(target_family = "wasm"))]
pub mod condition {
    pub use upwell_config::condition::{ResolvedConfig, resolve};
    pub use upwell_core::condition::*;
}

// ---------------------------------------------------------------------------
// DI engine: descriptors, container, factories, registry.
// ---------------------------------------------------------------------------
#[cfg(not(target_family = "wasm"))]
pub use upwell_di::{
    BoxedComponent, COMPONENTS, Component, ComponentChange, ComponentConstructionContext,
    ComponentContainer, ComponentDescriptor, ComponentFactories, ComponentFactory,
    ComponentFactoryDescriptor, ComponentFactoryRole, ComponentRegistry, ComponentSource,
    ConditionCatalog, ConditionDecision, ConditionDependency, ConditionError, ConditionEvaluation,
    ConditionFactSnapshot, ConditionOutcome, DeclaredScope, Deferred, Dep, DescriptorFor, Dynamic,
    EffectiveGraph, Factory, FactoryIdentity, FactoryOutput, Fresh, FreshFromContainer,
    FromContainer, GenerationSnapshot, HotUpdate, HotUpdateFactory, HotUpdateOutput, Injectable,
    Lazy, Live, LiveCell, LiveRebind, LiveRef, NodeAction, OneShot, PROVIDERS, PlannedNode,
    Previous, Provide, ProviderDescriptor, ProviderOf, ProviderOrder, ProviderOrderDirection,
    ReasonKind, Registration, RegistryFor, ReplacementFactory, RootResolver, ScopeContainer,
    ScopeRegistry, ServiceComponent, StaleGraphCandidate, TransitionPlan, TransitionReason,
    UpwellDescriptor, ValidatedConditionEvaluation, Wired, Wiring, dependency_of,
    dependency_of_observed, dispatch_factory, dispatch_hot_update, dispatch_replacement,
    dispatch_transition, erase_provider, factories, factory_dependencies, from_boxed,
    hot_update_dependencies, provider_live_cell, replacement_dependencies, topological_sort,
};
/// The DI layer's own error/result, exposed under distinct names so macro-generated
/// **factory** code can name them without colliding with another layer's `Error`/`Result`.
#[cfg(not(target_family = "wasm"))]
pub use upwell_di::{Error as DiError, Result as DiResult};

// ---------------------------------------------------------------------------
// Hooks.
// ---------------------------------------------------------------------------
#[cfg(not(target_family = "wasm"))]
pub use upwell_hooks::{
    ComponentHooks, FiredHook, FiredHooks, HookCall, HookDescriptor, HookKind, HookManager,
    HookParam, HookParticipation, HookRole, HookRouting, Shutdown, Startup, no_hooks,
};
/// The hook layer's own error/result, exposed under distinct names so macro-generated hook
/// code can name them without colliding with another layer's `Error`/`Result`.
#[cfg(not(target_family = "wasm"))]
pub use upwell_hooks::{Error as HookError, Result as HookResult};

// ---------------------------------------------------------------------------
// Directories.
// ---------------------------------------------------------------------------
#[cfg(not(target_family = "wasm"))]
pub use upwell_dirs::{Dir, DirKind, DirectoriesManager};

// ---------------------------------------------------------------------------
// Config: Cfg, ConfigManager, reload, the config store, the directory resolver.
// ---------------------------------------------------------------------------
#[cfg(not(target_family = "wasm"))]
pub use upwell_config::schema;
#[cfg(not(target_family = "wasm"))]
pub use upwell_config::{
    BindingFacts, CONFIG_BINDINGS, Cfg, CfgNext, ChangedBinding, ComponentHookReport,
    ConditionFactSource, ConditionFacts, ConfigBinding, ConfigBindingDescriptor, ConfigDefaults,
    ConfigError, ConfigManager, ConfigProperties, ConfigReload, ConfigReloadError, ConfigReloader,
    ConfigStore, ContainerConfigExt, DefaultSpec, DirectoriesResolver, EnumTag, HookOutcome,
    ReloadCause, ReloadProposal, ReloadSummary, ReloadTarget, ReloadTriggers, ReloadableConfig,
    StagedConfig, StagedReload, spawn_reload_triggers, stop_reload_triggers,
};

// ---------------------------------------------------------------------------
// Protocol-agnostic application core (always available): the App/Plugin seam, the runtime
// handle, lifecycle/shutdown, and the opt-in config-property builtins.
// ---------------------------------------------------------------------------
#[cfg(not(target_family = "wasm"))]
pub use upwell_app::Error as AppError;
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub use upwell_app::HostLifecycleCapabilities;
#[cfg(not(target_family = "wasm"))]
pub use upwell_app::contribute;
#[cfg(not(target_family = "wasm"))]
pub use upwell_app::{
    App, AppBuilder, AppConditionEvaluation, AppConditionState, AppHost, AppRegistry, AppRuntime,
    AppStage, ApplicationPluginRegistrar, BootstrapContext, Built, CandidateGraph,
    ComponentRetired, ComponentRetirement, ComponentTransitionReport, CompositionDiagnostic,
    CompositionDiagnostics, CompositionDirective, CompositionEdge, CompositionPhase,
    CompositionTarget, ContributionId, ContributionProvenance, Contributor, DisplacedGeneration,
    EarlyPluginCatalog, EarlyPluginPlan, EffectivePluginPlan, ExecutionMode, HostError,
    IdErrorKind, Initial, InstallationOrigin, InstallationProvenance, InvalidCompositionId,
    LifecyclePhase, LogFormat, LoggingConfig, PendingTransition, PhaseError, PhaseTiming, Plugin,
    PluginContribution, PluginContributionKind, PluginContributions, PluginDeclaration, PluginId,
    PluginPlanError, PluginRelation, PluginResolutionPlan, PluginSlotId, PluginWithOptions,
    PreBuild, PreBuildContext, PreparedApp, PreparedProtocol, PreparedScopeTopology,
    ProtocolDefinition, ProtocolId, ProtocolPluginRegistrar, ProtocolRuntime, RelationKind,
    RelationTarget, ReplacementDecision, ResolvedPlugin, RestartReason, RestartRequired,
    RetirementContext, RetirementReason, RuntimeReloader, RuntimeTransitionReport, RuntimeView,
    ScopeBoundary, ScopeParent, ScopeTopology, ScopeTopologyError, Serve, ServerConfig, Setup,
    ShutdownHandle, ShutdownSignal, SlotPolicy, SpanEvents, SuppressionDecision, TransitionAborted,
    TransitionAttemptId, TransitionCommitted, TransitionDecision, TransitionFailure,
    TransitionHookContext, TransitionHookOutcome, TransitionHookReport, TransitionOutcome,
    TransitionOutcomeContext, TransitionPhase, TransitionProposal, TransitionRejection,
    TransitionVerdict, ValidationContext, build_host, build_host_context, build_prepared_host,
    extend_late_plugins, prepare_host, prepare_host_context, prepare_setup_host_context,
    resolve_early_plugins, resolve_host_dependency, resolve_host_plugin_catalog,
    retain_host_plugin_catalog, serve_host, setup_host, setup_host_context,
};

/// Versioned protocol-neutral developer-tooling schema and prepared-state projection types.
#[cfg(all(not(target_family = "wasm"), feature = "tooling"))]
pub mod tooling {
    pub use upwell_app::{
        ToolingContributionError, ToolingContributions, ToolingEndpoint, ToolingProbeOutputError,
        ToolingProbeOutputTargetError, ToolingProbeTargetError, ToolingProjectionError,
        ToolingRelationshipKind,
    };
    pub use upwell_tooling_schema::*;
}

/// Internal contracts named by generated application code.
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub mod __private {
    #[cfg(feature = "tooling")]
    pub use upwell_app::tooling::{
        catch_probe_panic, emit_probe_envelope_from_env, install_process_probe_panic_hook,
        probe_host, probe_target_identity_from_env,
    };

    #[cfg(all(feature = "cli", feature = "tooling"))]
    pub use upwell_app::tooling::probe_bootstrapped_host;
}

#[cfg(all(not(target_family = "wasm"), feature = "cli"))]
pub use upwell_app::{
    BootstrapError, BootstrapOptions, BootstrapPolicy, BootstrapState, CliCommand,
    CliDefinitionError, CliDefinitionSource, CliError, CliPhase, ColorChoice, CommandContext,
    CommandContextError, CommandError, ParsedPluginArgs, PluginCliCommand, PluginCliPhase,
    PluginCliProviderKind, PluginCliProviderMetadata, PluginCliRegistrar, PluginCommandContext,
    SelectedPluginCliCommand, bootstrap_application, bootstrap_application_with_policy, clap,
    configure_bootstrap_config, configure_bootstrap_directories, dispatch_cli_command,
    finalize_bootstrap, prepare_cli_context, validate_cli,
};

// The generic `App<P>` / `AppBuilder<P>` and named `app!` definitions are at the root
// (protocol-agnostic core). A protocol's own surface (the RPC daemon's services, client, …) lives
// in its module
// (`upwell::daemon::*`), so the facade root stays free of protocol-specific names.

// ---------------------------------------------------------------------------
// Wire-contract status types and stream item codecs.
// ---------------------------------------------------------------------------
pub use upwell_transport::{
    Flags, PredefinedCode, StatusCode, StreamDecode, StreamDecodeError, StreamEncode,
    StreamEncodeError,
};

// ---------------------------------------------------------------------------
// Procedural macros: the core macros (including the protocol-agnostic `app!`) are
// always available; the RPC daemon macros (`service`/`handlers`/`rpc`) live in the `daemon`
// module behind the `daemon` feature.
// ---------------------------------------------------------------------------
pub use upwell_macros::{app, component, config, injectable, methods, when};

/// Re-exported so macro-generated code can reference the `#[distributed_slice]` attribute
/// through the facade crate without user crates depending on `linkme` directly. The generated
/// registration that names it is server-only (gated out on wasm), so this is too.
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub use upwell_di::linkme;

/// Re-exported so macro-generated code can reach `inventory::collect!`/`submit!`/`iter` (the
/// alternate registration backend, selected on Mach-O targets) through the facade. Server-only,
/// like the `linkme` re-export.
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub use upwell_di::inventory;

// ---------------------------------------------------------------------------
// Transport substrate: server endpoints, wire types, custom-transport traits. The RPC
// *client* lives under `daemon` (it is protocol-specific), not here.
// ---------------------------------------------------------------------------
#[cfg(not(target_family = "wasm"))]
pub use upwell_transport::{
    CallId, CallResult, Connection, IncomingCall, MemoryCall, MemoryClient, MemoryConnection,
    MemoryConnectionHandle, MemoryResponder, MemoryTransport, PeerInfo, Respond, RespondStream,
    ResponseSink, ServerEvent, TcpTransport, Transport, WireMessage, WireOutcome, WireRequest,
    WireResponse,
};

#[cfg(unix)]
pub use upwell_transport::UnixTransport;

/// The full transport substrate, including the framing codec and connection/responder
/// types for building custom transports.
pub mod transport {
    pub use upwell_transport::*;
}

/// The protocol-agnostic **client** core: the [`Transport`](client::Transport) base and the
/// per-call-shape capability traits ([`Unary`](client::Unary),
/// [`ServerStreaming`](client::ServerStreaming), …) generated clients build on.
/// Always available (like [`app`]-layer items); a protocol (the RPC `daemon`, …) implements
/// the capabilities it supports. Generated client code roots here, so it is identical across
/// protocols.
pub mod client {
    pub use upwell_client::*;
}

/// Application directory kinds (`Config`, `Data`, `Cache`, `State`, `Runtime`, `Tmp`), the
/// typed [`Dir`] wrapper, and the [`DirectoriesManager`]. Inject `Dir<dirs::Config>`.
#[cfg(not(target_family = "wasm"))]
pub mod dirs {
    pub use upwell_dirs::*;
}

/// Config source-format markers for `ConfigManager<F>`: [`Toml`](upwell_config::Toml),
/// the format-erased [`Dynamic`](upwell_config::Dynamic), and (with the `yaml` feature)
/// `Yaml`.
#[cfg(not(target_family = "wasm"))]
pub mod config {
    pub use upwell_config::{
        Dynamic, EnvResolver, Format, FormatId, MapResolver, Resolver, ResolverChain, Toml,
    };

    #[cfg(feature = "yaml")]
    pub use upwell_config::Yaml;
}

/// Framework builtins: the seeded [`ShutdownHandle`] and [`RuntimeReloader`] injectables, the opt-in
/// [`ServerConfig`] / [`LoggingConfig`] property structs, and the feature-gated
/// `init_tracing` subscriber helper.
#[cfg(not(target_family = "wasm"))]
pub mod builtins {
    pub use upwell_app::builtins::{LoggingConfig, ServerConfig};

    #[cfg(feature = "tracing-subscriber")]
    pub use upwell_app::builtins::{InitTracingError, init_tracing};
}

/// The native RPC **daemon** plugin surface, namespaced so plugin items never collide with
/// the facade root or with other plugins (`upwell::axum::*`, …).
///
/// Build an RPC app with `use upwell::prelude::*;` (the core framework) plus
/// `use upwell::daemon::prelude::*;` (the common RPC items). The daemon macros
/// (`#[service]`/`#[handlers]`/`#[rpc]`/`app!`) emit `::upwell::daemon::*` paths, so end
/// users depend only on `upwell` — never on `upwell-rpc` directly.
#[cfg(all(feature = "daemon", not(target_family = "wasm")))]
pub mod daemon {
    pub use upwell_rpc::{
        App, AppBuilder, Cancel, Error, ErrorHandler, ErrorResponse, FallibleHandler, FromContext,
        Guard, GuardLayer, GuardService, Handler, Inject, OperationKind, ParameterDescriptor,
        ParameterKind, Payload, Peer, PreparedRpc, RequestStream, ResolvedService, Responder,
        ResponseError, ResponseStream, Result, RouterService, Rpc, RpcAppBuilder, RpcCallContext,
        RpcDescriptor, RpcGroup, RpcHandler, RpcLimits, RpcOutcome, RpcRequest, RpcResponse,
        RpcRouter, RpcRuntime, RpcService, SERVICES, ServiceDescriptor, ServiceRpcs, Streaming,
        dispatch_fallible, dispatch_with,
    };

    /// Service/RPC route resolution, for introspecting the registered surface.
    pub use upwell_rpc::routes::resolved_services;

    /// The RPC component scopes.
    pub use upwell_rpc::scope::{Connection, Request};

    /// The RPC daemon macros, re-exported through `upwell-rpc` (which owns them). With the
    /// facade's `daemon` feature, `upwell-rpc/facade` is on, so their generated code roots
    /// protocol types at `::upwell::daemon::*` and core types at `::upwell::*`.
    /// (`app!` is a protocol-agnostic core macro at the crate root, not here.)
    pub use upwell_rpc::{handlers, rpc, service};

    /// Re-exported so middleware authors can implement `tower::Layer` / `tower::Service`.
    pub use upwell_rpc::tower;

    /// Re-exported so `#[rpc(stream)]` client codegen can project a concrete stream's item
    /// type. Hidden; referenced only by generated code.
    #[doc(hidden)]
    pub use upwell_rpc::__Stream;

    /// The RPC byte-stream transport — the daemon's [`ProtocolTransport`](crate::client) impl
    /// (`StreamClientTransport`), its response stream, and connect helpers. The agnostic client
    /// surface (`Client` API, capability traits) lives at [`upwell::client`](crate::client);
    /// this is the RPC carry that plugs into it. Gated behind the `client` feature.
    #[cfg(feature = "client")]
    pub use upwell_rpc::{RpcResponses, StreamClientTransport, connect_tcp};

    #[cfg(all(feature = "client", unix))]
    pub use upwell_rpc::connect_unix;

    /// Deprecated alias for [`App`]. Renamed in 0.7.0; removed in 1.0.0.
    #[deprecated(since = "0.7.0", note = "renamed to `App`; removed in 1.0.0")]
    pub type Daemon = App;

    /// Deprecated alias for [`AppBuilder`]. Renamed in 0.7.0; removed in 1.0.0.
    #[deprecated(since = "0.7.0", note = "renamed to `AppBuilder`; removed in 1.0.0")]
    pub type DaemonBuilder = AppBuilder;

    /// Common imports for building an RPC daemon: `use upwell::daemon::prelude::*;` (pair
    /// with the crate-root `use upwell::prelude::*;` for the core framework + `app!`).
    pub mod prelude {
        pub use super::{
            App, FromContext, Handler, Inject, Payload, Peer, Responder, Rpc, RpcAppBuilder,
            Streaming, handlers, rpc, service,
        };

        pub use upwell_transport::TcpTransport;

        #[cfg(unix)]
        pub use upwell_transport::UnixTransport;
    }
}

/// The job scheduler: run `async` methods on an interval or cron schedule as supervised
/// background tasks, or schedule work at run time.
///
/// A protocol-agnostic [`Plugin`] — enable the `jobs` feature and register
/// [`JobsPlugin`](jobs::JobsPlugin) alongside any protocol. Mark methods with
/// `#[job(every = "..")]` / `#[job(cron = "..")]` (the `#[job]` codegen emits
/// `::upwell::jobs::*` paths), or inject `Arc<JobScheduler>` and call
/// [`schedule`](jobs::JobScheduler::schedule) for dynamic jobs.
#[cfg(all(feature = "jobs", not(target_family = "wasm")))]
pub mod jobs {
    pub use upwell_jobs::*;
}

/// The axum/HTTP protocol surface, namespaced so plugin items never collide with the facade
/// root or with the RPC `daemon` module.
///
/// Build an HTTP app with `use upwell::prelude::*;` (the core framework + `app!`) plus
/// `use upwell::axum::prelude::*;` (controllers, route attributes, the DI `Inject`
/// extractor, and the common axum extractors). The controller macros
/// (`#[controller]`/`#[handlers]`/`#[get]`/…) emit `::upwell::axum::*` paths, so end users
/// depend only on `upwell` — never on `upwell-axum` directly.
#[cfg(feature = "axum")]
pub mod axum {
    /// The whole `upwell-axum` surface, glob-re-exported so the facade never drifts behind the
    /// protocol crate: the controller types/macros, the framing wrappers, the `axum` crate and
    /// `http` re-exports, the `scope` module, and (with the `client` feature) the `client` module
    /// and `__Stream` the generated client names. The protocol crate's root *is* the curated API.
    /// With the facade's `axum` feature, `upwell-axum/facade` is on, so the macros' generated
    /// code roots protocol types at `::upwell::axum::*` and core types at `::upwell::*`.
    pub use upwell_axum::*;
    #[cfg(feature = "json-ws")]
    pub use upwell_axum_json_ws::*;
    #[cfg(feature = "stomp")]
    pub use upwell_axum_stomp::*;

    #[cfg(all(feature = "stomp", feature = "client"))]
    pub mod stomp_client {
        pub use upwell_axum_stomp::StompStatus;

        #[cfg(feature = "tungstenite")]
        pub use upwell_axum_stomp::{StompClientTransport, StompConnectOptions};
    }

    /// Common imports for building an HTTP controller app: `use upwell::axum::prelude::*;`
    /// (pair with the crate-root `use upwell::prelude::*;` for the core framework + `app!`).
    ///
    /// The controller/route macros are available on every target; the server extractors, `Router`,
    /// `App`, and DI types are the server surface (compiled out on a wasm client build, where the
    /// generated `{Controller}Client` is used directly rather than the prelude).
    pub mod prelude {
        pub use super::{
            controller, delete, dto, get, handlers, head, options, patch, post, put, route,
        };

        #[cfg(not(target_family = "wasm"))]
        pub use super::axum::extract::{Extension, Json, Path, Query, State};
        #[cfg(not(target_family = "wasm"))]
        pub use super::axum::response::IntoResponse;
        #[cfg(not(target_family = "wasm"))]
        pub use super::axum::{Router, http};
        #[cfg(not(target_family = "wasm"))]
        pub use super::scope::HttpRequest;
        #[cfg(not(target_family = "wasm"))]
        pub use super::{App, Axum, AxumAppBuilder, AxumAppServe, AxumConfig, Controller, Inject};

        #[cfg(all(feature = "json-ws", not(target_family = "wasm")))]
        pub use super::JsonWs;
        /// WebSocket controller imports (`#[controller(ws = ..)]` + `#[message]`) and the explicit
        /// connection/message scope markers, available with the `ws` feature.
        #[cfg(all(feature = "ws", not(target_family = "wasm")))]
        pub use super::scope::{WebsocketConnection, WebsocketMessage};
        #[cfg(all(feature = "ws", not(target_family = "wasm")))]
        pub use super::{WebsocketController, WebsocketProtocol, message};

        /// STOMP pub/sub imports (`#[controller(ws = Stomp)]` + `#[topics]`, the typed
        /// [`Publisher`] and [`Topic`]), available with the
        /// `stomp` feature.
        #[cfg(all(feature = "stomp", not(target_family = "wasm")))]
        pub use super::{
            Publisher, Stomp, StompConfig, StompConnect, StompPrincipal, StompSession,
            StompTopicBus, Topic, topics,
        };
    }
}

/// The common imports for the **core framework**: `use upwell::prelude::*;`. Pair with
/// `use upwell::daemon::prelude::*;` (or another plugin's prelude) for the protocol layer.
pub mod prelude {
    // The macros are built for the host and usable from any target (their generated server code is
    // gated out on wasm); the framework *types* are the daemon-stack surface, compiled out on wasm.
    pub use crate::{app, component, config, injectable, methods, when};

    #[cfg(not(target_family = "wasm"))]
    pub use crate::{
        App, Cfg, Component, ConfigManager, ConfigProperties, Deferred, Dep, Dir, DirKind,
        DirectoriesManager, Fresh, FreshFromContainer, Injectable, Lazy, Plugin,
        ProtocolDefinition, ProtocolRuntime, RuntimeDescriptor, Scope, Serve, ServiceComponent,
    };

    #[cfg(all(not(target_family = "wasm"), feature = "cli"))]
    pub use crate::{Built, CliCommand, CommandContext, PreBuild, Setup};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facade_exposes_core_types() {
        let td = TypeDescriptor::of::<u8>("byte");

        assert_eq!(td.name, "byte");
        assert_eq!(td.type_id, type_id_of::<u8>());
    }
}
