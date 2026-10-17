//! Procedural macros for the Upwell framework.
//!
//! These protocol-neutral macros are re-exported from the `upwell` facade crate; depend on that
//! rather than this crate directly. Protocol-specific service, handler, and RPC macros live in
//! their protocol crates.
//!
//! | Macro                       | Applies to | Produces |
//! |-----------------------------|------------|----------|
//! | `#[component]`              | struct     | `Component` impl + a factory (field-injection default, `factory = path`, or `default_factory = false` for a manual instance) |
//! | `#[methods]`                | impl block | lifecycle methods — an `#[init]` constructor (an explicit factory) |
//! | `#[init]`                   | method     | marker consumed by `#[methods]` |
//! | `#[injectable]`             | trait      | `Provide<dyn Trait>` impl (under `di-check`) |
//! | `#[config]`                 | struct/enum | `ConfigProperties` impl (with field `#[default = ".."]` templated defaults); auto-registers a binding when given `#[config(path = "..")]` |
//! | [`app!`]                    | declaration | named typestate host, configured builder, and feature-gated CLI/tooling entry |
//!
//! # Components: two ways to provide one
//!
//! A *component* is a singleton dependency, resolved by type. There are two ways
//! to get one into the daemon's container:
//!
//! 1. **System-constructed** — annotate the type with `#[component]` (or use the selected
//!    protocol's component-producing macro). The macro registers a factory; the container builds
//!    the instance from its dependencies during startup.
//! 2. **Manually provided** — construct the instance yourself and hand it to
//!    `AppBuilder::with_component`. Annotate the type
//!    `#[component(default_factory = false)]`, which emits the `Component` metadata
//!    with no factory.
//!
//! Both forms register a descriptor in the `DescriptorRegistry`; the difference
//! is whether the descriptor carries a factory or expects a provided instance.
//!
//! # Field injection
//!
//! `#[component]` and `#[service]` build their factory by *field injection*. For
//! each field of the struct:
//!
//! - an `Arc<T>` field is treated as a **dependency** and resolved from the
//!   container (`cx.resolve::<T>()`);
//! - a `Cfg<T>` field carrying `#[config("path")]` is a **config binding** resolved
//!   by property path (omit the path for the sole-binding shorthand);
//! - a `#[default]` field is **owned state**, built with `Default::default()` — so
//!   its type must implement `Default`, otherwise construct the component another
//!   way (an `#[init]` constructor, or `with_component`).
//!
//! # Implementation
//!
//! Each `#[proc_macro_*]` entry point here is a thin shim: it forwards its token streams to
//! the matching `expand` function in [`upwell_macros_core`], the ordinary library that
//! holds all the parsing and codegen (a proc-macro crate can only export proc-macros, so the
//! reusable machinery lives there). Errors are surfaced as `compile_error!` by the core, not
//! by panicking.

extern crate proc_macro;

use proc_macro::TokenStream;

/// Declares a **system-constructed singleton component** on a struct.
///
/// The container builds the instance during startup by *field injection*: each
/// field is resolved from the container as a dependency (its type is an injectable
/// handle — `Arc<T>`, `Cfg<T>` for config, a trait-object collection, …), unless it
/// carries `#[default]`, which makes it owned state built with `Default::default()`.
/// Use this for dependencies the system can assemble itself (pools, clients composed
/// from other components, …). For an instance you must build yourself, use
/// `#[component(default_factory = false)]` and provide it via
/// `AppBuilder::with_component`.
///
/// # Arguments
///
/// All optional:
/// - `id` — unique component id. Defaults to the lowercased type name.
/// - `name` — display name. Defaults to the type name.
/// - `factory = path` — register `path` (an async `Factory`) as the constructor
///   instead of field injection; its parameters are its dependencies.
/// - `default_factory = false` — emit no factory (a **manual** instance, provided
///   via `AppBuilder::with_component`).
/// - `factory_slice = Ident` — override the generated `{Type}Factories` slice name.
/// - `priority = <const i64 expression>` — collection-provider order among providers whose
///   `before` / `after` constraints are satisfied; lower values run first. Constants and
///   associated constants are accepted.
/// - `before` / `after` — relative ordering constraints for traits shared by both providers;
///   use `as dyn Trait` to require and restrict the relationship to a specific trait.
///
/// ```ignore
/// #[component]                          // id = "dbpool", name = "DbPool"
/// #[component(id = "db", name = "Db")]  // explicit
/// #[component(factory = Db::connect)]   // explicit async factory
/// #[component(default_factory = false)] // manual, via with_component
/// ```
///
/// # What it generates
///
/// - `impl Component for T` (carrying `ID`/`NAME`);
/// - a `ComponentDescriptor` registered into the `COMPONENTS` slice (picked up by
///   `auto_discover`), pointing at the type's `{Type}Factories` slice — which holds
///   the field-injection default (unless suppressed) plus any `factory =` / `#[init]`
///   entry.
///
/// # Example
///
/// ```ignore
/// use upwell::prelude::*;
/// use std::sync::Arc;
///
/// #[component(default_factory = false)]
/// struct Config { url: String }
///
/// /// Built from `Config` (resolved) plus owned state (`#[default]`).
/// #[component]
/// struct Pool {
///     config: Arc<Config>,   // dependency, resolved from the container
///     #[default]
///     hits: std::sync::atomic::AtomicU64, // owned state, Default-built
/// }
/// ```
///
/// # Errors
///
/// Emits a `compile_error!` if applied to anything but a struct. If a `#[default]`
/// field doesn't implement `Default`, the *generated* factory fails to compile.
///
/// A component the system should *build* uses field injection by default; for one
/// you construct yourself, `#[component(default_factory = false)]` emits the
/// metadata with no factory (provide it via `AppBuilder::with_component`), and
/// `#[component(factory = path)]` registers an explicit async factory.
///
/// # See also
///
/// The selected protocol's component-producing macros and `#[methods]` (an `#[init]` constructor
/// for any component).
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    upwell_macros_core::component(attr.into(), item.into()).into()
}

/// Implements the `ConfigProperties` trait for a config `struct` or `enum`, making it
/// injectable as `Cfg<T>` from a property path.
///
/// The type must also derive `Deserialize`, and `#[config]` must sit *above* the derive so
/// it strips any field `#[default]` before the derive runs. With `#[config(path = "..")]`
/// the binding is auto-registered (picked up by `auto_discover`); without a path, bind it
/// explicitly with `AppBuilder::config::<T>(path)` — needed when the same type is
/// bound at several paths. `#[config(name = "..")]` overrides the display name.
///
/// A named field may carry `#[default = ".."]`: the literal is a template string merged
/// under the config before deserialization, so a missing field falls back to it and
/// resolves through the normal `${..}` pipeline (env vars, `${other.path}` refs, and the
/// `${@runtime}` directory namespace). Unlike a hand-written `Default`, the default keeps
/// full templating power. On an enum, defaults may sit on variant fields and apply only to
/// the variant present in the config, and a variant may be marked with a bare `#[default]`
/// to select it when the config names no variant.
///
/// Distinct from the **field-level** `#[config("path")]` inside a `#[component]` /
/// `#[service]` struct, which marks a `Cfg<T>` injection site (consumed by that
/// macro's expansion); this struct-level form declares the config type itself.
///
/// ```ignore
/// #[config(path = "app.server")]
/// #[derive(Deserialize)]
/// struct ServerConfig {
///     #[default = "${tcp.ip}:8080"]
///     addr: SocketAddr,
/// }
/// ```
#[proc_macro_attribute]
pub fn config(attr: TokenStream, item: TokenStream) -> TokenStream {
    upwell_macros_core::config(attr.into(), item.into()).into()
}

/// Registers a component's lifecycle methods from an inherent `impl` block.
///
/// Today that is the `#[init]` constructor — an explicit factory that overrides the
/// field-injection default. Works on any component (`#[component]` or `#[service]`),
/// so a plain component gets a full-flexibility constructor (sync or async, any
/// injectable parameter — `Arc<T>`, `Cfg<T>`, `Vec<Arc<dyn Tr>>`, a by-value
/// injectable) without the async-only `factory = ..` form.
///
/// The constructor's parameters are its dependencies (resolved from the container)
/// and its return is `Self` or `Result<Self, E>`; a non-`async` constructor is
/// wrapped to async. Two `#[init]`s on one type is a compile error.
///
/// # Arguments
///
/// Optional `factory_slice = Ident` — the per-type factory slice to append to,
/// matching the owning `#[component]`/`#[service]`'s `factory_slice` when overridden
/// (defaults to `{Type}Factories`).
///
/// ```ignore
/// #[methods]
/// impl Greeter {
///     #[init]
///     async fn new(config: Arc<Config>) -> Result<Self> { Ok(Self { config }) }
/// }
/// ```
#[proc_macro_attribute]
pub fn methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    upwell_macros_core::methods(attr.into(), item.into()).into()
}

/// Declares a reusable, protocol-parameterized application host.
///
/// This is the authoritative reference for the named `app!` declaration. A complete expansion
/// needs a real `ProtocolDefinition`, so the examples below that invoke the macro are not doctested;
/// working end-to-end declarations live in
/// [`examples/daemon`](https://github.com/upwell-rs/upwell/tree/main/examples/daemon),
/// [`examples/http`](https://github.com/upwell-rs/upwell/blob/main/examples/http/src/main.rs),
/// and the
/// [application tests](https://github.com/upwell-rs/upwell/blob/main/tests/app_definition.rs).
///
/// # Declaration envelope
///
/// ```text
/// app! {
///     OUTER_DOC_ATTRIBUTES
///     VISIBILITY app RustTypeName {
///         name: APPLICATION_NAME,
///         protocol: PROTOCOL_TYPE,
///         ...
///     }
/// }
/// ```
///
/// `VISIBILITY` is any Rust visibility, including empty, `pub`, or `pub(crate)`. The only
/// attributes accepted between `app! {` and the visibility are outer `#[doc = ...]` attributes,
/// including those produced by `///`; put `cfg` and other item attributes around a containing module
/// or macro invocation instead. Every body key may appear at most once and entries may have trailing
/// commas.
///
/// `name` and `protocol` are mandatory. `protocol` is a Rust type implementing
/// `ProtocolDefinition`; generated lifecycle and command paths also require it to be `Send`. `name`
/// parses as a Rust expression and is evaluated whenever the generated `builder()` is called. When
/// either `cli` or `tooling` is enabled, it must instead be a string literal because it also defines
/// stable parser and tooling identity. With both features absent, a non-literal item-scope
/// expression is accepted, but like every declaration expression it cannot capture a local from
/// `main`.
///
/// # Top-level grammar
///
/// | Key | Value | Effect |
/// |---|---|---|
/// | `name` | expression; string literal with `cli` or `tooling` | Application name passed to `App::<P>::builder` and used for generated identity. |
/// | `protocol` | type | Selects the one `ProtocolDefinition` for this host. |
/// | `services` | `[Type, ...]` | Under `di-check`, asserts each type implements `Wired`; it does **not** register runtime services. Protocol auto-discovery performs runtime service/controller discovery. |
/// | `components` | `[expression, ...]` | Calls `with_component(expression)` in order for pre-built singleton instances. |
/// | `configs` | `[Type => "property.path", ...]` | Calls `config::<Type>(path)`; the same type may be bound at multiple paths. A path-bearing `#[config]` type can be auto-discovered instead. |
/// | `managers` | `{ config: MANAGER, directories: MANAGER }` | Supplies or constructs framework config/directory managers; see below. |
/// | `middleware` | `[expression, ...]` | Calls the selected protocol builder extension's `middleware` method in order. |
/// | `guards` | `[expression, ...]` | Calls the selected protocol builder extension's `guard` method in order. |
/// | `error_handler` | expression | Calls the selected protocol builder extension's `error_handler` method. |
/// | `plugins` | `[Type, replace SLOT => Type, suppress SLOT, ...]` | Resolves parser-visible static plugins and protocol-default slots before bootstrap. |
/// | `cli` | reserved-slot block | Customizes framework-owned Clap arguments and the generated serve command. |
/// | `args` | `{ field: Type, ... }` | Flattens application-owned global `clap::Args` groups. |
/// | `commands` | `{ name: Type, namespace: { ... }, ... }` | Declares typed leaf commands and nested command namespaces. |
/// | `upwell` | path | Overrides the generated core-framework path; normally omit it when using the `upwell` facade. |
/// | `setup` | `= path` or `(context) { ... }` | Defines the setup callback. |
/// | `configure` | `= path` or `(context, builder) { ... }` | Defines the first builder callback. |
/// | `before_build` | `= path` or `(context, builder) { ... }` | Defines the final callback before preparation and validation. |
/// | `after_build` | `= path` or `(context, app) { ... }` | Defines the callback after construction. |
/// | `serve` | `= path` or `(context, app, dependency: Type, ...) { ... }` | Defines serving; only inline serve accepts additional DI parameters. |
///
/// `middleware`, `guards`, and `error_handler` are protocol-extension method calls, not core
/// `AppBuilder` concepts. The relevant extension trait must be in scope and the selected protocol
/// decides accepted values and behavior. There is deliberately no protocol-specific nested grammar
/// inside `app!`; use `configure` for other protocol builder methods.
///
/// # Builder and managers
///
/// `Host::builder()` starts with `App::<Protocol>::builder(name).auto_discover()`, then applies
/// components, config bindings, explicit managers, middleware, guards, and the error handler in
/// declaration order. It returns `Result<AppBuilder<Protocol>, ConfigError>` because a
/// macro-constructed config manager may read and parse files. Every item-scope expression is
/// evaluated afresh on every `builder()` call; it cannot capture function locals. Use the direct
/// builder API when construction requires runtime-local values.
///
/// Each manager accepts either an instance expression or a construction block:
///
/// ```text
/// managers: {
///     directories: directories_expression,
///     config: config_manager_expression,
/// }
///
/// managers: {
///     directories: { app: application_name_expression, root: root_path_expression },
///     config: {
///         source: config_manager_expression,
///         profiles: profiles_expression,
///         sighup: false,
///         watch: false,
///         debounce: duration_expression,
///     },
/// }
/// ```
///
/// A directories block calls `DirectoriesManager::for_app(app)` or
/// `DirectoriesManager::from_path(root)` and needs exactly one of those settings; combining `app`
/// and `root` is rejected. A config block with `source` starts from that expression. Without
/// `source`, it requires an explicitly declared directories manager and calls
/// `ConfigManager::<Dynamic>::load_from(&directories, profiles)`, where omitted `profiles` is
/// `&[]`. `profiles` is used only by that directory-backed load. `sighup: true` enables Unix SIGHUP
/// reload, `watch: true` requests source-file watching, and `debounce` calls
/// `config_reload_debounce` (the manager default is 250 ms). These settings configure the manager,
/// not a protocol. Omitted `sighup` and `watch` are `false`; omitting `debounce` leaves the manager's
/// 250 ms default unchanged. Combining `source` and `profiles` is rejected; each setting can appear
/// only once.
///
/// Explicit `config` or `directories` entries make the declaration own that manager, so generated
/// CLI bootstrap does not overwrite it. When omitted, `run`/`run_with` supply platform directories
/// and selected configuration through bootstrap. Direct typestate entry does not run CLI bootstrap;
/// the lower-level builder then derives directories from the app name and loads its normal config
/// directory defaults during preparation. An explicit config path that cannot be read or parsed,
/// missing directories for a source-less config block, or an empty directories block is an error.
/// With `watch` disabled at compile time, a requested watcher is logged and ignored when startup
/// handles reload triggers; manual reload remains available.
///
/// # Lifecycle callbacks
///
/// Function-path callbacks are called and awaited. Their exact contracts are futures with these
/// outputs, where `E: Error + Send + Sync + 'static`:
///
/// | Phase | Parameters | Future output |
/// |---|---|---|
/// | `setup` | `BootstrapContext` | `Result<BootstrapContext, E>` |
/// | `configure` | `&mut BootstrapContext, AppBuilder<P>` | `Result<AppBuilder<P>, E>` |
/// | `before_build` | `&mut BootstrapContext, AppBuilder<P>` | `Result<AppBuilder<P>, E>` |
/// | `after_build` | `&mut BootstrapContext, App<P>` | `Result<App<P>, E>` |
/// | `serve` | `BootstrapContext, App<P>` | `Result<(), E>` |
///
/// Inline bodies use the same contracts and are wrapped in an awaited `async move` block. Their
/// parameter names are arbitrary, but the first parameters cannot have type annotations. Only an
/// inline `serve` body may add parameters; each must have an explicit `Injectable` type and is
/// resolved, in order, from the built root container before the body starts. A path-form serve
/// callback performs its own DI resolution if needed. Omitted setup/build callbacks are identity
/// transitions; omitting `serve` means no inherent serving method or framework serve command is
/// generated.
///
/// ```rust,ignore
/// app! {
///     app ServiceApplication {
///         name: "service",
///         protocol: Rpc,
///         configure(_context, builder) {
///             Ok::<_, std::convert::Infallible>(builder)
///         },
///         serve(_context, app, server: Cfg<ServerConfig>) {
///             let server = server.snapshot();
///             let transport = TcpTransport::bind((server.bind.as_str(), server.port)).await?;
///
///             app.serve(transport).await
///         },
///     }
/// }
/// ```
///
/// The lifecycle stages own concrete state and transitions consume `self`:
///
/// | Host stage | Owned state | Concrete work completed |
/// |---|---|---|
/// | `Host<Initial>` | `ExecutionMode` | Nothing else: no CLI bootstrap, directories, config, builder, discovery, validation, construction, or serving. |
/// | `Host<Setup>` | `BootstrapContext` | `setup` and generated tracing finalization completed; no builder exists. |
/// | `Host<PreBuild>` | `(BootstrapContext, PreparedApp<P>)` | Builder assembly, early plugin resolution, `configure`, `before_build`, discovery, protocol/framework registration, directory and config resolution, descriptor/provider/scope/config/protocol validation, and construction planning completed. Ordinary components, root DI, `AppRuntime`, and protocol runtime do not exist. |
/// | `Host<Built>` | `(BootstrapContext, App<P>)` | Singleton and root-container construction, hook/root-resolver attachment, `AppRuntime` creation, protocol runtime construction, and `after_build` completed. Serve, transport startup, startup hooks, and shutdown waiting have not run. |
///
/// The generated methods are:
///
/// | Stage | Methods |
/// |---|---|
/// | every stage | `from_state(Stage::State)`, `into_state()` |
/// | `Initial` | `new(ExecutionMode)`, fallible `builder()`, `setup()`, `prepare()`, `build()`, and `serve()` when declared; with `cli`, also `run()` and `run_with(args)` |
/// | `Setup` | `context()`, `prepare()`, `build()`, and declared `serve()` |
/// | `PreBuild` | `context()`, `app()` returning `&PreparedApp<P>`, `into_parts()`, `build()`, and declared `serve()` |
/// | `Built` | `context()`, `app()` returning `&App<P>`, async `resolve::<T>()`, `into_parts()`, and declared `serve()` |
///
/// Fast-forward methods execute every remaining stage in order; for example, `Initial::build()` is
/// setup plus prepare plus build, and `Initial::serve()` additionally serves. `from_state`,
/// `into_state`, and `into_parts` are custom-runtime escape hatches and perform no hidden work.
/// `Initial` implements `AppHost`, so generic `setup_host`, `prepare_host`, and `build_host` helpers
/// drive the same callbacks and ordering.
///
/// # Generated CLI
///
/// The default `cli` feature generates `HostBootstrapArgs` and `HostCli` for **every** named app,
/// even one with no custom args, commands, or serve phase. It generates `HostCommand` when a
/// framework serve command or application command exists, plus nested enums named from the
/// path, such as `HostApiUsersCommand`. All these types have the host's visibility. A public host's
/// argument and leaf-command types must therefore also be publicly usable.
///
/// Every crate expanding a named app while `cli` is enabled needs a direct Clap dependency because
/// generated code derives and names `::clap` types; a transitive Upwell dependency is not enough:
///
/// ```toml
/// clap = { version = "4", features = ["derive"] }
/// ```
///
/// `HostCli` has one flattened public `bootstrap: HostBootstrapArgs`, one public flattened field for
/// each `args` entry, and at most one public `command: Option<HostCommand>` marked as the subcommand.
/// The exact omitted defaults are:
///
/// | Canonical slot | Default parser shape |
/// |---|---|
/// | `config` | ID/field `config`, `Option<PathBuf>`, global `--config`, `-c`, value `PATH`, help `Configuration file or directory.` |
/// | `profile` | ID/field `profiles`, `Vec<String>`, global repeatable `--profile`, `-p`, value `PROFILE`, help `Ordered configuration profile; may be repeated.` |
/// | `log` | ID/field `log`, `Option<String>`, global `--log`, value `FILTER`, help `EnvFilter-compatible tracing directive.` |
/// | `log_format` | ID/field `log_format`, `Option<LogFormat>`, global value-enum `--log-format`, value `FORMAT`, help `Tracing output formatter.` |
/// | `color` | ID/field `color`, `Option<ColorChoice>`, global value-enum `--color`, value `WHEN`, help `ANSI color behavior.` |
/// | `serve` | command displayed as `serve`, help `Build and serve the application.`; enabled only when `serve` exists and selected by default when no command is given |
///
/// Clap's generated `help` and `version` arguments/commands are also framework-owned. Tooling keeps
/// the canonical serve command ID `serve` even when its display name changes; bootstrap argument IDs
/// are the generated field IDs shown above. Display names, aliases, and canonical ownership are
/// therefore separate.
///
/// `cli` has these exact slots and settings:
///
/// ```text
/// cli: {
///     config: false | true | { ARGUMENT_SETTINGS },
///     profile: false | true | { ARGUMENT_SETTINGS },
///     log: false | true | { ARGUMENT_SETTINGS },
///     log_format: false | true | { ARGUMENT_SETTINGS },
///     color: false | true | { ARGUMENT_SETTINGS },
///     serve: false | true | { SERVE_SETTINGS },
/// }
///
/// ARGUMENT_SETTINGS =
///     enabled: bool,
///     name: "long-name-without-dashes",
///     short: 'x' | false,
///     aliases: ["hidden-alias", ...],
///     visible_aliases: ["visible-alias", ...],
///     hidden: bool,
///     help: "...",
///     value_name: "VALUE",
///     exactly one shape-appropriate Clap default
///
/// SERVE_SETTINGS =
///     enabled: bool,
///     name: "command-name",
///     aliases: ["hidden-alias", ...],
///     visible_aliases: ["visible-alias", ...],
///     hidden: bool,
///     help: "...",
///     default_command: bool
/// ```
///
/// `false` disables only that parser source; `true` explicitly preserves its normal defaults.
/// Disabling `serve` does not remove typestate `serve()`. Enabling the slot without a serve phase,
/// making a disabled serve the default, or giving a disabled argument a parser default is rejected.
/// Names omit leading dashes. `short: false` removes a default short; `short: true` is invalid.
///
/// The four default settings map unchanged to Clap. Scalar `config`, `log`, `log_format`, and
/// `color` accept exactly one of `default_value: "literal"` or `default_value_t: expression`.
/// Repeated `profile` accepts exactly one of `default_values: ["literal", ...]` or
/// `default_values_t: expression`. Singular/plural forms cannot be mixed or used on the wrong
/// field. Literal `log_format` values are `full`, `compact`, `pretty`, or `json`; literal `color`
/// values are `auto`, `always`, or `never`.
///
/// Typed defaults are still Clap defaults: their type must satisfy Clap's generated requirements,
/// including `Display` where required, and the displayed value must round-trip through that field's
/// parser. `LogFormat` supports this. `PathBuf` and the current `ColorChoice` do not implement
/// `Display`, so use literal `default_value` for those fields. Defaults appear in help and are parsed
/// rather than bypassing validation.
///
/// Generated parsing captures each field's `clap::parser::ValueSource` before typed extraction.
/// A parser `DefaultValue` therefore does not masquerade as an explicit `CommandLine` value. The
/// effective precedence is exact per setting:
///
/// | Setting | Highest to lowest precedence |
/// |---|---|
/// | config location | explicit CLI, `UPWELL_CONFIG`, parser default, platform config directory |
/// | profiles | explicit CLI, comma-separated `UPWELL_PROFILES`, parser defaults, empty list |
/// | log filter | explicit CLI, `RUST_LOG`, `logging.level` config, parser default when config omits it, `LoggingConfig` default |
/// | log format | explicit CLI, `UPWELL_LOG_FORMAT`, `logging.format` config, parser default when config omits it, `LoggingConfig` default (`full`) |
/// | color | explicit CLI, `NO_COLOR`, nonzero `CLICOLOR_FORCE`, `logging.ansi` config, parser default, terminal-detected `auto` |
///
/// `run()` recognizes the private tooling probe first when enabled, otherwise parses
/// `std::env::args_os()`. Clap help/version/usage output uses `clap::Error::exit`; non-Clap failures
/// return `CliError`. `run_with(args)` composes the same effective parser, never prints or exits, and
/// returns `CliError::Clap`. If serve is the default, no subcommand selects it. Otherwise an
/// application or plugin command is required. The generated serve command bootstraps through
/// `Built`, then calls the declared serve phase.
///
/// # Arguments and commands
///
/// ```text
/// args: {
///     /// Documentation copied to the generated field.
///     field_name: ClapArgsType,
/// }
///
/// commands: {
///     leaf_name: ClapArgsAndCliCommandType,
///     namespace_name: {
///         nested_leaf: AnotherCommandType,
///     },
/// }
/// ```
///
/// An `args` type must implement `clap::Args`, is flattened once into the root parser, and is stored
/// by concrete type in `BootstrapContext`; aliases `bootstrap` and `command`, duplicate aliases, and
/// duplicate types are rejected. Only doc attributes are accepted on these fields. Put
/// `#[arg(global = true)]` on fields that must also parse after a subcommand.
///
/// Command identifiers normalize to lowercase kebab-case (`print_config` becomes `print-config`)
/// and generate PascalCase Rust variants. A namespace creates a public enum named by its full path
/// and contains its own single subcommand field. Empty command blocks/namespaces, normalized or Rust
/// variant collisions, framework-reserved names/options, and names beginning `__upwell` are
/// rejected.
///
/// Command entries accept doc attributes and a deliberately bounded set of non-structural Clap
/// `#[command(...)]` settings: help/version/display metadata, aliases and command flags, help
/// layout/styling, usage text, and local parser behavior. The
/// [complete allowlist is maintained beside the parser](https://github.com/upwell-rs/upwell/blob/main/crates/macros-core/src/app/command.rs#L315-L427).
/// `name`, `ignore_errors`, `rename_all`, `rename_all_env`, `flatten`, `subcommand`,
/// `external_subcommand`, `skip`, `allow_external_subcommands`, and `subcommand_required` are
/// explicitly rejected because they can replace or bypass generated typed dispatch; settings
/// outside the allowlist are also rejected.
///
/// Leaf types implement `clap::Args + CliCommand<Host>` and select exactly one sealed phase:
/// `Setup`, `PreBuild`, or `Built`. Every `CommandContext` exposes `bootstrap()`,
/// `bootstrap_mut()`, and `require::<T>()`; `PreBuild` adds `prepared()`, while `Built` adds `app()`
/// and async `resolve::<T>()`. This makes invalid stage access a compile-time error. Command errors
/// are returned as `CliError::Command` with the complete normalized command path.
///
/// # Static plugins
///
/// `plugins: [PluginType]` installs an application plugin. `replace SLOT => ReplacementType`
/// replaces a protocol-owned replaceable default slot, and `suppress SLOT` removes a
/// protocol-owned optional default slot. Installed and replacement types must implement
/// `Plugin + Default`; construction is synchronous (`P::default()`), occurs before parser
/// composition, and the same retained instance contributes CLI facets and is consumed during
/// preparation. Plugin constructors are not async.
///
/// Slot constants belong to protocol crates. Replacement/suppression fails for unknown, mandatory,
/// wrong-policy, duplicate, or conflicting slots. The built-in RPC and Axum protocols currently
/// declare no default plugin slots, so their applications can install plugins but have no built-in
/// slot constant to replace or suppress. Protocol authors expose their own namespaced
/// `PluginSlotId` constants when they add defaults.
///
/// A plugin's `Plugin::cli` may register `args::<T>(ContributionId)`, one named
/// `command::<T>(ContributionId, name)`, or a flattened native
/// `commands::<T>(ContributionId)`. Stable provider IDs and the fully composed parser are checked
/// for collisions across framework, application, and plugin ownership. Plugin commands implement
/// `PluginCliCommand` and receive protocol-neutral `PluginCommandContext<Setup | PreBuild | Built>`.
/// Setup exposes bootstrap values; pre-build adds application name, registry, and plugin plan;
/// built adds application name, plugin plan, and DI resolution, but deliberately not a concrete
/// protocol app.
///
/// Plugins added later in `configure`, `before_build`, or direct builder code through
/// `register_plugin::<T>()`, `register_plugin_with_options::<T>(options)`, or
/// `with_plugin(instance)` participate only in runtime preparation. They cannot change the parser
/// or replace/suppress the already resolved early catalog.
///
/// # Tooling
///
/// With `tooling`, each named app gets a target-local `#[doc(hidden)] tooling_probe(target)` seam.
/// With `cli + tooling`, `run()` also recognizes exactly one private process invocation containing
/// only `--__upwell-tooling-probe-v1`; this is not an end-user command. Tooling requires a
/// literal `name` even when CLI generation is disabled.
///
/// The probe resolves static protocol/application plugins, composes and validates the effective
/// Clap parser when `cli` is present, parses only generated bootstrap defaults, runs bootstrap in
/// `ExecutionMode::Tooling`, then runs `setup`, `configure`, `before_build`, and
/// `AppBuilder::prepare`. It projects that exact immutable `PreparedApp` into a deterministic,
/// versioned tooling document. CLI metadata is derived from the final executable Clap tree after
/// plugin augmentation, including displayed names, aliases, defaults, cardinality, global flags,
/// command nesting, canonical framework IDs, and framework/application/plugin ownership. Without
/// `cli`, the tooling document has no CLI section.
///
/// The probe does not build ordinary components, the root container, `AppRuntime`, or the protocol
/// runtime; it does not run `after_build` or `serve`, open listeners, run startup hooks, start config
/// watchers, or wait for shutdown. Static plugin instances are still synchronously constructed and
/// consumed because their parser and prepared-plan contributions are being inspected. Application
/// callbacks are application code, so guard side effects with `context.mode().is_run()`.
///
/// The callable seam catches unwind and returns a stable envelope but cannot replace an embedding
/// process's panic hook. The dedicated process path installs a payload-free hook. At the user-facing
/// #152 boundary, Cargo tooling, not `app!`, selects one package and binary target, builds and invokes
/// it, supplies target identity and a private response directory, and validates the atomically
/// published, no-clobber envelope. The private directory provides isolation and stale temporary
/// cleanup, not publication correctness. Stdout/stderr remain separate. There is no process-global
/// app registry and tooling does not parse source or infer a binary from a compile-time crate name.
///
/// Protocols and plugins may add validated owner-scoped resources, labels, relationships, and
/// namespaced versioned JSON facets from prepared descriptors. Projection reads retained prepared
/// metadata; it does not reconstruct metadata by building runtime objects.
///
/// # Feature behavior
///
/// | Feature | Present | Absent |
/// |---|---|---|
/// | `cli` *(default)* | Generates Clap types, bootstrap, command dispatch, `run`, and `run_with`; requires direct Clap. | Generates only typestate lifecycle/builder APIs; no parser or `CliCommand` surface. |
/// | `tooling` | Generates the callable/private process probe and prepared-plan projection. | No probe entry or tooling document projection from the host. |
/// | `di-check` *(facade default)* | `services` emits `Wired` assertions and component macros emit compile-time provider bounds. | Runtime preparation still validates the effective graph; `services` has no effect. |
/// | `watch` | Config managers can start requested file watchers. | `watch: true` remains a valid manager request but is logged and ignored at startup. |
/// | `tracing-subscriber` | Generated bootstrap can accept setup-contributed layers and install the resolved global subscriber after setup. | Logging/color policy is still resolved, but generated bootstrap does not install a subscriber and layer APIs are absent. |
/// | `hybrid-registry` | Forces generated metadata registration through `inventory`. | Uses `linkme`, except Apple/Mach-O hosts automatically select `inventory`; cross-compiling to Mach-O should force this feature. |
///
/// `tooling` is independent of `cli`; CLI metadata exists only when both are enabled. `watch`,
/// `tracing-subscriber`, and `hybrid-registry` are facade-level features forwarded to the relevant
/// core/macro crates and do not select a protocol.
///
/// # Errors and escape hatches
///
/// Syntax errors are source-local `compile_error!` diagnostics: unknown/duplicate keys, invalid
/// manager or lifecycle grammar, reserved CLI collisions, invalid default shapes, unsupported
/// command attributes, empty namespaces, and malformed plugin directives point at the offending
/// declaration token or block rather than the whole macro invocation. User-provided application,
/// protocol, callback, manager, plugin, argument, command, and path tokens are reused directly in
/// generated Rust. DSL keys that correspond to generated Rust declarations are each mapped to one
/// principal declaration; differently spelled declarations retain generated resolution while using
/// the source key's location. This semantic mapping is best effort through stable proc-macro spans:
/// rust-analyzer highlighting, hover, and go-to-definition depend on the analyzer and toolchain's
/// span support and are not guaranteed. Other synthesized items are anchored to the nearest
/// application, namespace, or command identifier. Compiler diagnostics involving directly reused
/// user tokens remain source-local.
/// Rust then reports type-level errors at generated uses: wrong protocol/extension types,
/// inaccessible public command types, invalid Clap defaults, non-`Injectable` serve dependencies,
/// callback contract mismatches, and `Wired` failures.
///
/// At runtime, `builder()` returns config-loading errors. `run_with` distinguishes plugin-catalog,
/// parser-definition, Clap, bootstrap, phase-tagged lifecycle, and command failures through
/// `CliError`; direct lifecycle methods return `PhaseError`, whose `phase()` identifies the failed
/// boundary. Preparation reports config, plugin composition, descriptor/provider, dependency,
/// scope, and protocol validation errors before ordinary construction. Tooling rejects any build
/// transition before construction begins.
///
/// Use a named host with direct `new(ExecutionMode::Run)` transitions when only `main`, runtime, or
/// error policy is custom. Use `App::<P>::builder(name).auto_discover()` directly when assembly must
/// capture local values, use plugin instances/options unavailable at item scope, or mutate the
/// builder outside declared callbacks. The direct builder does not generate the typestate host,
/// callbacks, CLI tree, static parser-visible plugin boundary, runner, or private tooling entry.
#[proc_macro]
pub fn app(input: TokenStream) -> TokenStream {
    upwell_macros_core::app(input.into()).into()
}

/// Marks a trait as injectable as `Arc<dyn Trait>` (providers register with
/// `#[component(provide = dyn Trait)]`).
///
/// On native targets the trait also extends
/// `RuntimeDescriptor<ComponentDescriptor>`, allowing a provider's component
/// descriptor to be read through the trait object. Wasm targets retain the
/// original trait because the DI descriptor types are native-only.
///
/// Under the `di-check` feature it emits `impl Provide<dyn Trait> for Wiring` so
/// a single `Arc<dyn Trait>` dependency type-checks; the trait must be `Send +
/// Sync` (state it as a supertrait) and object-safe.
#[proc_macro_attribute]
pub fn injectable(attr: TokenStream, item: TokenStream) -> TokenStream {
    upwell_macros_core::injectable(attr.into(), item.into()).into()
}

/// Makes a component eligible only while a boolean condition holds.
///
/// Place `#[when(<expr>)]` **below** `#[component]`, `#[service]` or `#[controller]`; the
/// component macro reads it. The body is an ordinary Rust expression of type `bool`, kept
/// with its exact semantics (short-circuiting included), and may call any function. Its
/// inputs are derived from the syntax:
///
/// - An UpperCamelCase type used as a field or method receiver is an input. A
///   `#[config(path = ..)]` type reads as its staged value (`AuthConfig.mode`); a component
///   reads as its eligibility (`AuditTransport.eligible()`). Any other type is a compile
///   error at that token. `Type::Variant` paths are left alone.
/// - `resolve::<T>("<path>")` (from `upwell::condition`) reads a config path that no type
///   binds. The path must be a string literal. A missing key gives `None`; a value that
///   does not deserialize into `T` rejects the candidate.
///
/// Evaluation answers only these declared inputs, so a helper function that reads another
/// input fails the evaluation instead of creating a hidden dependency. Inputs follow Rust
/// privacy: read `pub`/`pub(crate)` fields or getters.
///
/// Stacked `#[when]`s combine with AND. Each condition's id is derived from its expression,
/// or set with `#[when(id = "..", <expr>)]`, so declaration order never changes selection.
/// Conditions apply to singleton components only.
///
/// ```ignore
/// use upwell::condition::resolve;
/// use upwell::{component, when};
///
/// #[component(provide = dyn Authenticator)]
/// pub struct DefaultAuthenticator;
///
/// #[component(provide = dyn Authenticator, primary)]
/// #[when(AuthConfig.mode == AuthMode::Custom)]
/// pub struct CustomAuthenticator;
///
/// #[component]
/// #[when(AuthConfig.audit && AuditTransport.eligible())]
/// #[when(resolve("features.audit_export") == Some(true))]
/// pub struct AuditExporter;
/// ```
///
/// # Errors
///
/// Used without a component macro, or above one, the attribute emits a `compile_error!`.
#[proc_macro_attribute]
pub fn when(attr: TokenStream, item: TokenStream) -> TokenStream {
    upwell_macros_core::when(attr.into(), item.into()).into()
}
