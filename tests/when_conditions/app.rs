//! The `#[when]` `Authenticator` application: an unconditional default provider, a primary
//! custom provider gated on `auth.mode`, an audit transport gated on `auth.audit`, and an
//! audit exporter that combines a typed binding, the transport's eligibility and a raw
//! config path. Every condition is a `#[when]` attribute.
//!
//! The app registers its components explicitly and never binds `AuthConfig`: the conditions
//! alone bind it at its declared `#[config(path = ..)]` path.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use tempfile::TempDir;
use upwell::condition::resolve;
use upwell::config::Toml;
use upwell::{App, AppBuilder, AppError, ConfigManager, component, config, injectable, when};
use upwell_config::ResolverChain;

/// The trait both providers implement and the gate consumes.
#[injectable]
pub trait Authenticator: Send + Sync {
    fn scheme(&self) -> &'static str;
}

/// How requests authenticate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Default,
    Custom,
}

#[config(path = "auth")]
#[derive(Deserialize)]
pub struct AuthConfig {
    pub mode: AuthMode,
    pub audit: bool,
}

/// The fallback provider: unconditional, selected while it is the only eligible one.
#[component(provide = dyn Authenticator)]
pub struct DefaultAuthenticator;

impl Authenticator for DefaultAuthenticator {
    fn scheme(&self) -> &'static str {
        "default"
    }
}

/// The application's own provider, primary whenever it is eligible.
#[component(provide = dyn Authenticator, primary)]
#[when(AuthConfig.mode == AuthMode::Custom)]
pub struct CustomAuthenticator;

impl Authenticator for CustomAuthenticator {
    fn scheme(&self) -> &'static str {
        "custom"
    }
}

/// Ships audit records while auditing is enabled.
#[component]
#[when(AuthConfig.audit)]
pub struct AuditTransport;

/// Exports audit records when auditing runs over an eligible transport and the raw
/// `features.audit_export` flag is set.
#[component]
#[when(AuthConfig.audit && AuditTransport.eligible())]
#[when(resolve("features.audit_export") == Some(true))]
pub struct AuditExporter;

/// [`AuditExporter`]'s conditions stacked in the opposite order.
#[component]
#[when(resolve("features.audit_export") == Some(true))]
#[when(AuthConfig.audit && AuditTransport.eligible())]
pub struct ReorderedAuditExporter;

/// A condition with an explicit id.
#[component]
#[when(id = "custom-mode", AuthConfig.mode == AuthMode::Custom)]
pub struct NamedCondition;

/// A condition outside the tooling-evaluable subset: it calls a macro.
#[component]
#[when(matches!(AuthConfig.mode, AuthMode::Custom))]
pub struct TrustedCondition;

/// A fixed consumer that captured the effective provider at construction.
#[component]
pub struct AuthGate {
    authenticator: Arc<dyn Authenticator>,
}

impl AuthGate {
    pub fn scheme(&self) -> &'static str {
        self.authenticator.scheme()
    }
}

/// One `application.toml`.
pub struct Settings {
    pub mode: &'static str,
    pub audit: bool,
    /// The raw `features.audit_export` value as TOML, or `None` to omit the key.
    pub export: Option<&'static str>,
}

pub fn write_settings(config_dir: &Path, settings: &Settings) {
    let mut toml = format!(
        "[auth]\nmode = \"{}\"\naudit = {}\n",
        settings.mode, settings.audit
    );

    if let Some(export) = settings.export {
        toml.push_str(&format!("\n[features]\naudit_export = {export}\n"));
    }

    fs::write(config_dir.join("application.toml"), toml).expect("write application.toml");
}

/// Builds the app over a fresh config directory holding `settings`.
pub async fn build(settings: &Settings) -> (TempDir, PathBuf, Result<App<()>, AppError>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-when-conditions-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = dir.path().join("config");

    fs::create_dir_all(&config_dir).expect("create config dir");
    write_settings(&config_dir, settings);

    let app = builder(&config_dir).build().await;

    (dir, config_dir, app)
}

/// The app's builder over the config in `config_dir`.
pub fn builder(config_dir: &Path) -> AppBuilder<()> {
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(config_dir, &[], ResolverChain::empty())
            .expect("load config");

    App::<()>::builder("when-conditions-test")
        .config_source(manager)
        .component::<DefaultAuthenticator>()
        .component::<CustomAuthenticator>()
        .component::<AuditTransport>()
        .component::<AuditExporter>()
        .component::<ReorderedAuditExporter>()
        .component::<NamedCondition>()
        .component::<TrustedCondition>()
        .component::<AuthGate>()
        .provider::<DefaultAuthenticator>()
        .provider::<CustomAuthenticator>()
}
