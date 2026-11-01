use proc_macro2::Span;
use syn::{Attribute, Block, Expr, Ident, LitStr, Path, Type, Visibility};

/// A reusable named application definition.
pub(crate) struct NamedApp {
    pub(super) attributes: Vec<Attribute>,
    pub(super) visibility: Visibility,
    pub(super) ident: Ident,
    pub(super) assembly: AppAssembly,
}

/// A parsed DSL value together with the exact key that declared it.
pub(super) struct Declared<T> {
    pub(super) key: Ident,
    pub(super) value: T,
}

/// The protocol-specific builder assembly declared by a named application.
pub(crate) struct AppAssembly {
    pub(super) name: Declared<Expr>,
    pub(super) protocol: Declared<Type>,
    pub(super) services: Vec<Type>,
    pub(super) components: Vec<Expr>,
    pub(super) configs: Vec<ConfigEntry>,
    pub(super) config_manager: Option<ManagerSource<ConfigSettings>>,
    pub(super) directories_manager: Option<ManagerSource<DirSettings>>,
    pub(super) middleware: Vec<Expr>,
    pub(super) guards: Vec<Expr>,
    pub(super) error_handler: Option<Expr>,
    pub(super) deny_unplanned: bool,
    pub(super) plugins: Vec<PluginDirective>,
    pub(super) upwell: Option<Path>,
    pub(super) phases: AppPhases,
    pub(super) cli_policy: super::policy::CliPolicy,
    #[cfg_attr(not(feature = "cli"), allow(dead_code))]
    pub(super) cli: CliDeclarations,
}

/// One parser-visible static application plugin directive.
#[allow(clippy::large_enum_variant)]
pub(super) enum PluginDirective {
    Install(Type),
    Replace { slot: Expr, plugin: Type },
    Suppress(Expr),
}

/// Application-owned global argument groups and command tree.
#[derive(Default)]
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
pub(super) struct CliDeclarations {
    pub(super) args: Vec<GlobalArgsEntry>,
    pub(super) commands: Vec<CommandEntry>,
}

/// One flattened global Clap argument group.
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
pub(super) struct GlobalArgsEntry {
    pub(super) attributes: Vec<Attribute>,
    pub(super) alias: Ident,
    pub(super) ty: Type,
}

/// One application command or nested command namespace.
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
pub(super) struct CommandEntry {
    pub(super) attributes: Vec<Attribute>,
    pub(super) name: Ident,
    pub(super) kind: CommandEntryKind,
}

/// The value associated with a command name.
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
#[allow(clippy::large_enum_variant)]
pub(super) enum CommandEntryKind {
    Leaf(Type),
    Namespace(Vec<CommandEntry>),
}

/// Application lifecycle phase definitions.
#[derive(Default)]
pub(super) struct AppPhases {
    pub(super) setup: Option<Declared<PhaseInput>>,
    pub(super) configure: Option<Declared<PhaseInput>>,
    pub(super) before_build: Option<Declared<PhaseInput>>,
    pub(super) after_build: Option<Declared<PhaseInput>>,
    pub(super) serve: Option<Declared<PhaseInput>>,
}

/// A lifecycle phase implemented by a function or inline block.
pub(super) enum PhaseInput {
    Path(Path),
    Inline {
        arguments: Vec<PhaseArgument>,
        body: Block,
    },
}

/// One named lifecycle value or typed dependency resolved for an inline phase.
pub(super) struct PhaseArgument {
    pub(super) ident: Ident,
    pub(super) ty: Option<Type>,
}

/// How a manager is supplied in the `managers` block.
pub(super) struct ManagerSource<S> {
    pub(super) key_span: Span,
    pub(super) value: ManagerValue<S>,
}

/// The instance expression or configuration block supplying a manager.
#[allow(clippy::large_enum_variant)]
pub(super) enum ManagerValue<S> {
    Instance(Expr),
    Configure { block_span: Span, settings: S },
}

/// One manager configuration value and the key that declared it.
pub(super) struct ManagerSetting<T> {
    pub(super) key_span: Span,
    pub(super) value: T,
}

/// Settings for a macro-constructed `ConfigManager`.
pub(super) struct ConfigSettings {
    pub(super) source: Option<ManagerSetting<Expr>>,
    pub(super) profiles: Option<ManagerSetting<Expr>>,
    pub(super) sighup: Option<ManagerSetting<bool>>,
    pub(super) watch: Option<ManagerSetting<bool>>,
    pub(super) debounce: Option<ManagerSetting<Expr>>,
}

/// Settings for a macro-constructed `DirectoriesManager`.
pub(super) struct DirSettings {
    pub(super) app: Option<ManagerSetting<Expr>>,
    pub(super) root: Option<ManagerSetting<Expr>>,
}

/// One `configs:` entry containing a type and property path.
pub(super) struct ConfigEntry {
    pub(super) ty: Type,
    pub(super) path: LitStr,
}
