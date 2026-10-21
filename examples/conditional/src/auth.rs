//! The injectable `Authenticator`, its typed config, and its two providers.

use serde::Deserialize;
use upwell::{component, config, injectable, when};

/// Checks login tokens. Consumers inject it as `Arc<dyn Authenticator>` or
/// `Dep<dyn Authenticator>`.
#[injectable]
pub trait Authenticator: Send + Sync {
    /// A short name for the scheme, for display.
    fn scheme(&self) -> &'static str;

    /// Whether `token` is accepted.
    fn accepts(&self, token: &str) -> bool;
}

/// Which provider authenticates requests.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Default,
    Custom,
}

/// The `auth` config section.
#[config(path = "auth")]
#[derive(Deserialize)]
pub struct AuthConfig {
    /// The provider to use.
    pub mode: AuthMode,
    /// How many sessions the [`SessionLedger`](crate::SessionLedger) may hold.
    pub max_sessions: usize,
}

/// The framework-shipped fallback: unconditional, so it is selected whenever the custom
/// provider is not eligible.
#[component(provide = dyn Authenticator)]
pub struct DefaultAuthenticator;

impl Authenticator for DefaultAuthenticator {
    fn scheme(&self) -> &'static str {
        "default"
    }

    fn accepts(&self, token: &str) -> bool {
        !token.is_empty()
    }
}

/// The application's own provider: primary whenever `auth.mode = "custom"`.
#[component(provide = dyn Authenticator, primary)]
#[when(AuthConfig.mode == AuthMode::Custom)]
pub struct CustomAuthenticator;

impl Authenticator for CustomAuthenticator {
    fn scheme(&self) -> &'static str {
        "custom"
    }

    fn accepts(&self, token: &str) -> bool {
        token.starts_with("custom:")
    }
}
