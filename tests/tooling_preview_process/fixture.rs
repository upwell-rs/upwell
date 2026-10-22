//! Probe fixture for what-if transition previews: an `Authenticator` with a flag-gated custom
//! provider and an unconditional fallback, plus a fixed and a live consumer. Its protocol
//! runtime panics, so a probe that tries to build the application fails.

#![allow(dead_code)]

use std::sync::Arc;

use serde::Deserialize;
use upwell::{
    AppRegistry, Dep, PreparedProtocol, ProtocolDefinition, ProtocolRuntime, app, component,
    config, injectable, when,
};

/// Checks login tokens.
#[injectable]
pub trait Authenticator: Send + Sync {}

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
}

/// The unconditional fallback provider.
#[component(provide = dyn Authenticator)]
pub struct DefaultAuthenticator;

impl Authenticator for DefaultAuthenticator {}

/// The primary provider while `auth.mode = "custom"`.
#[component(provide = dyn Authenticator, primary)]
#[when(AuthConfig.mode == AuthMode::Custom)]
pub struct CustomAuthenticator;

impl Authenticator for CustomAuthenticator {}

/// Holds a fixed provider, so a switch reconstructs it.
#[component]
pub struct AuthBanner {
    authenticator: Arc<dyn Authenticator>,
}

/// Holds a live provider, so a switch rebinds it in place.
#[component]
pub struct LoginGate {
    authenticator: Dep<dyn Authenticator>,
}

/// Protocol whose runtime the tooling process must never build.
#[derive(Default)]
struct PreviewProtocol;

impl ProtocolDefinition for PreviewProtocol {
    type Prepared = PreparedPreviewProtocol;
    type Error = upwell_app::Error;

    const ID: upwell::ProtocolId = upwell::namespaced_id!(upwell::ProtocolId, "test/preview");
    const SCOPE_TOPOLOGY: upwell::ScopeTopology = upwell::ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(
        self,
        _context: &upwell::ValidationContext<'_>,
    ) -> Result<Self::Prepared, Self::Error> {
        Ok(PreparedPreviewProtocol)
    }
}

/// Prepared protocol of the preview fixture.
struct PreparedPreviewProtocol;

/// Runtime protocol that the tooling process must never construct.
struct PreviewRuntime;

impl PreparedProtocol for PreparedPreviewProtocol {
    type Runtime = PreviewRuntime;
    type Error = upwell_app::Error;

    fn build(self, _runtime: &upwell::AppRuntime) -> Result<Self::Runtime, Self::Error> {
        panic!("tooling process must not build protocol runtime");
    }
}

impl ProtocolRuntime for PreviewRuntime {
    type Error = upwell_app::Error;
}

app! {
    app PreviewApplication {
        name: "preview-fixture",
        protocol: PreviewProtocol,
        configs: [AuthConfig => "auth"],
        cli: {
            config: {
                default_value: "tests/tooling_preview_process/config",
            },
        },
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = PreviewApplication::run().await {
        eprintln!("upwell tooling preview process failed: {error}");
        std::process::exit(2);
    }
}
