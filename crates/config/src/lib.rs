//! Configuration for the Upwell framework.
//!
//! Two layers live here. The **parser** is the format-agnostic core: a normalized
//! [`ConfigValue`] tree, a placeholder grammar (`${KEY}` / `${KEY:default}`), a
//! [`Resolver`] chain, and a custom [`from_value`] deserializer that resolves placeholders
//! *while* deserializing. The **managed** layer (this crate's `managed` module) integrates
//! that parser with dependency injection: [`Cfg<T>`] injectables, the [`ConfigManager`]
//! that loads and merges files, the [`ConfigStore`] resolver the DI container reaches
//! config through, and the two-phase [`ConfigReloader`].
//!
//! The parser's substitution failure type is [`TemplateError`]; the managed layer's
//! load/bind failure type is [`ConfigError`].

mod de;
mod defaults;
mod error;
pub mod format;
mod managed;
mod parse;
mod resolve;
mod value;

pub use de::{ValueDeserializer, from_value, from_value_in};
pub use defaults::{DefaultSpec, EnumTag};
pub use error::{TemplateError, TemplateErrorKind};
pub use resolve::{EnvResolver, MapResolver, ResolveCtx, Resolver, ResolverChain};
pub use value::{ConfigStr, ConfigValue, Placeholder, Segment};

pub use managed::Toml;
#[cfg(feature = "yaml")]
pub use managed::Yaml;
pub use managed::{
    BindingFacts, CONFIG_BINDINGS, CONFIG_RELOADER_ID, CONFIG_RELOADER_NAME, Cfg, CfgNext,
    ChangedBinding, ComponentHookReport, ConditionFactSource, ConditionFacts, ConfigBinding,
    ConfigBindingDescriptor, ConfigDefaults, ConfigError, ConfigManager, ConfigProperties,
    ConfigReload, ConfigReloadError, ConfigReloadReport, ConfigReloader, ConfigStore,
    ContainerConfigExt, DirectoriesResolver, Dynamic, Format, FormatId, HookOutcome,
    ReloadProposal, ReloadTriggers, ReloadableConfig, StagedConfig, StagedReload,
    spawn_reload_triggers, stop_reload_triggers,
};
