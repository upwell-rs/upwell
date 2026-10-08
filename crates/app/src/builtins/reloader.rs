//! The framework-seeded [`RuntimeReloader`] singleton injectable.
//!
//! [`RuntimeReloader`] is seeded into the root scope before the runtime exists and is
//! attached to it once the application is built. It holds the runtime weakly, so a
//! handle stored in the runtime's own root never keeps that runtime alive. Retained
//! singletons carry the same handle across runtime generations.

use std::sync::{Arc, OnceLock};

use upwell_di::{Component, Injectable};

use crate::runtime::{AppRuntime, RuntimeReloadReport, WeakAppRuntime};

/// The stable component id of the seeded [`RuntimeReloader`] singleton.
pub const RUNTIME_RELOADER_ID: &str = "upwell:runtime-reloader";

/// The display name of the seeded [`RuntimeReloader`] singleton.
pub const RUNTIME_RELOADER_NAME: &str = "RuntimeReloader";

/// The handle that requests a transactional config-and-graph reload of its application.
///
/// Inject it (`reloader: RuntimeReloader`) or take it from
/// [`App::reloader`](crate::App::reloader). Every reload, including `SIGHUP` and
/// file-watch triggers, runs through [`AppRuntime::reload_config`], so there is one reload
/// pipeline.
///
/// A config-reload hook or candidate factory must never await
/// [`reload`](Self::reload): the reload that runs it holds the non-reentrant
/// serialization lease, so the nested call deadlocks.
#[derive(Clone)]
pub struct RuntimeReloader {
    runtime: Arc<OnceLock<WeakAppRuntime>>,
}

impl RuntimeReloader {
    /// A fresh, unattached handle, seeded before the runtime exists.
    pub(crate) fn new() -> Self {
        Self {
            runtime: Arc::new(OnceLock::new()),
        }
    }

    /// Attaches the built runtime. Idempotent; a second attach is ignored.
    pub(crate) fn attach(&self, runtime: &AppRuntime) {
        let _ = self.runtime.set(runtime.downgrade());
    }

    /// Re-reads configuration and transactionally republishes changed bindings and the
    /// derived component graph through [`AppRuntime::reload_config`].
    ///
    /// Fails with [`RuntimeUnavailable`](crate::Error::RuntimeUnavailable) while the
    /// application is still being built or after its runtime has been dropped.
    pub async fn reload(&self) -> crate::Result<RuntimeReloadReport> {
        let runtime = self
            .runtime
            .get()
            .and_then(WeakAppRuntime::upgrade)
            .ok_or(crate::Error::RuntimeUnavailable)?;

        runtime.reload_config().await
    }
}

impl Component for RuntimeReloader {
    type Handle = RuntimeReloader;

    const ID: &'static str = RUNTIME_RELOADER_ID;
    const NAME: &'static str = RUNTIME_RELOADER_NAME;

    fn into_handle(self) -> Self::Handle {
        self
    }
}

impl Injectable for RuntimeReloader {
    type Target = RuntimeReloader;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }

    fn snapshot_stored(stored: &Self) -> Option<Self> {
        Some(stored.clone())
    }
}

/// Under `di-check`, the reloader is framework-seeded, so it is always provided.
#[cfg(feature = "di-check")]
impl upwell_di::Provide<RuntimeReloader> for upwell_di::Wiring {}
