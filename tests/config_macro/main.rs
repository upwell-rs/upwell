//! End-to-end tests for the `#[config]` macro's field defaults and enum support,
//! exercised through `ConfigManager::get_config` with the directory namespace wired in.
//! These cover the full path: macro-emitted `defaults()` -> merge -> templated
//! resolution -> typed value.

use std::path::PathBuf;

use serde::Deserialize;
use upwell::config::Toml;
use upwell::{
    ConditionFactSource, ConditionFacts, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, ConfigManager, DescriptorSource, DirectoriesManager,
    config,
};
use upwell_config::ResolverChain;

/// Resolves directory placeholders against a fixed root, so `${@runtime}` becomes
/// `<root>/runtime` deterministically.
fn manager(text: &str) -> ConfigManager {
    let dirs = DirectoriesManager::from_path(PathBuf::from("/base"));

    ConfigManager::<Toml>::from_str(text)
        .expect("parse config")
        .with_resolvers(ResolverChain::empty())
        .with_directories(&dirs)
        .into_dynamic()
}

/// Like [`manager`], but auto-discovers every `#[config(path)]` type and seeds their
/// defaults into the tree — so a default may reference another type's (or its own sibling's)
/// path even when that value is itself only a default.
fn seeded_manager(text: &str) -> ConfigManager {
    let dirs = DirectoriesManager::from_path(PathBuf::from("/base"));

    ConfigManager::<Toml>::from_str(text)
        .expect("parse config")
        .with_resolvers(ResolverChain::empty())
        .with_directories(&dirs)
        .auto_discover()
        .into_dynamic()
}

mod defaults;
mod facts;
mod references;
mod tagged;
