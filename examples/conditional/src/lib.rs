//! A worked example of conditional components and runtime transitions.
//!
//! An injectable [`Authenticator`] has an unconditional [`DefaultAuthenticator`] and a
//! [`CustomAuthenticator`] gated on `auth.mode = "custom"` by a typed `#[when]` condition.
//! Switching the flag at runtime moves every consumer across without a restart:
//!
//! - [`LoginGate`] holds a live `Dep<dyn Authenticator>` and keeps its instance;
//! - [`AuthBanner`] holds a fixed `Arc<dyn Authenticator>` and is reconstructed;
//! - [`SessionLedger`] is replaced by its `#[replace]` recipe, which carries the open sessions
//!   and fails when they exceed the new `auth.max_sessions`, rolling the switch back;
//! - [`TransitionAudit`] inspects each proposed change and observes how it ended.
//!
//! [`Demo`] drives the scenario that `main` prints and the tests assert.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use upwell::config::Toml;
use upwell::{App, AppError, AppRuntime, ConfigError, ConfigManager, RuntimeTransitionReport};

mod audit;
mod auth;
mod consumers;

pub use audit::{AuditEvent, TransitionAudit};
pub use auth::{AuthConfig, AuthMode, Authenticator, CustomAuthenticator, DefaultAuthenticator};
pub use consumers::{AuthBanner, LoginGate, SessionLedger, SessionLimitExceeded};

/// Why the demo could not run.
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error("failed to write the demo config")]
    Io(#[from] std::io::Error),
    #[error("failed to load the demo config")]
    Config(#[from] ConfigError),
    #[error(transparent)]
    App(#[from] AppError),
}

/// The example application over its own temporary config directory.
pub struct Demo {
    _dir: TempDir,
    config_dir: PathBuf,
    app: App<()>,
}

impl Demo {
    /// Builds the application with `auth.mode = mode` and `auth.max_sessions = max_sessions`.
    pub async fn start(mode: AuthMode, max_sessions: usize) -> Result<Self, DemoError> {
        let dir = tempfile::Builder::new()
            .prefix("upwell-example-conditional-")
            .tempdir()?;
        let config_dir = dir.path().join("config");

        fs::create_dir_all(&config_dir)?;
        write_config(&config_dir, mode, max_sessions)?;

        let manager = ConfigManager::<Toml>::load_in_explicit(&config_dir, &[])?;
        let app = App::<()>::builder("conditional-example")
            .config_source(manager)
            .config::<AuthConfig>("auth")
            .component::<DefaultAuthenticator>()
            .component::<CustomAuthenticator>()
            .component::<LoginGate>()
            .component::<AuthBanner>()
            .component::<SessionLedger>()
            .component::<TransitionAudit>()
            .provider::<DefaultAuthenticator>()
            .provider::<CustomAuthenticator>()
            .build()
            .await?;

        Ok(Self {
            _dir: dir,
            config_dir,
            app,
        })
    }

    /// Rewrites the config and reloads it: the transition either commits, or is rejected
    /// and leaves every instance as it was.
    pub async fn switch(
        &self,
        mode: AuthMode,
        max_sessions: usize,
    ) -> Result<RuntimeTransitionReport, DemoError> {
        write_config(&self.config_dir, mode, max_sessions)?;

        let report = self.runtime().reload_config().await?;

        Ok(report)
    }

    /// The running application's runtime.
    pub fn runtime(&self) -> &AppRuntime {
        self.app.runtime()
    }

    /// The current login gate.
    pub fn gate(&self) -> Arc<LoginGate> {
        self.current()
    }

    /// The current banner.
    pub fn banner(&self) -> Arc<AuthBanner> {
        self.current()
    }

    /// The current session ledger.
    pub fn ledger(&self) -> Arc<SessionLedger> {
        self.current()
    }

    /// The transition audit.
    pub fn audit(&self) -> Arc<TransitionAudit> {
        self.current()
    }

    fn current<T: upwell::Component<Handle = Arc<T>>>(&self) -> Arc<T> {
        self.runtime()
            .root()
            .get::<T>()
            .expect("every demo component is registered and eligible")
    }
}

fn write_config(
    config_dir: &std::path::Path,
    mode: AuthMode,
    max_sessions: usize,
) -> std::io::Result<()> {
    let mode = match mode {
        AuthMode::Default => "default",
        AuthMode::Custom => "custom",
    };

    fs::write(
        config_dir.join("application.toml"),
        format!("[auth]\nmode = \"{mode}\"\nmax_sessions = {max_sessions}\n"),
    )
}
