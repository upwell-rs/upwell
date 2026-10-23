use std::fmt;
use std::sync::Arc;

use futures::FutureExt;
use tracing::info;
use upwell_config::{ReloadTriggers, spawn_reload_triggers, stop_reload_triggers};
use upwell_di::ScopeContainer;
use upwell_hooks::HookManager;

use crate::builtins::RuntimeReloader;
use crate::lifecycle::{ShutdownHandle, ShutdownSignal};
use crate::plugin::EffectivePluginPlan;
use crate::protocol::{PreparedProtocol, ProtocolDefinition, ProtocolRuntime, Serve};
use crate::registry::AppRegistry;
use crate::runtime::AppRuntime;

mod builder;
mod lifecycle;
mod prepare;
mod prepared;

pub use builder::AppBuilder;
pub use prepared::{HostLifecycleCapabilities, PreparedApp};

use lifecycle::{run_shutdown, run_startup};

/// A fully assembled app, ready to serve its protocol.
///
/// Holds the agnostic [`AppRuntime`] (DI container, scope orders, hooks) and the built
/// [`ProtocolRuntime`], plus the shutdown signal, the runtime reloader handle, and the
/// reload triggers the serve envelope spawns against the runtime.
pub struct App<D: ProtocolDefinition> {
    pub name: String,
    pub registry: AppRegistry,
    runtime: AppRuntime,
    protocol: <D::Prepared as PreparedProtocol>::Runtime,
    plugin_plan: EffectivePluginPlan,
    shutdown: ShutdownSignal,
    reloader: RuntimeReloader,
    reload_triggers: ReloadTriggers,
}

impl<D: ProtocolDefinition> fmt::Debug for App<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("name", &self.name)
            .field("components", &self.registry.components.len())
            .finish_non_exhaustive()
    }
}

impl<D: ProtocolDefinition> fmt::Display for App<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "App: {}", self.name)?;
        write!(f, "{}", self.registry)?;

        Ok(())
    }
}

impl<D: ProtocolDefinition> App<D> {
    /// The application name declared by its builder.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Starts building an app for protocol definition `D`. Most protocols expose a pinned
    /// alias (e.g. `upwell_rpc::App = App<Rpc>`) so `App::builder(name)` resolves
    /// without a turbofish.
    pub fn builder(name: impl Into<String>) -> AppBuilder<D> {
        AppBuilder::new(name)
    }

    /// The root (singleton) scope container.
    pub fn container(&self) -> Arc<ScopeContainer> {
        self.runtime.root()
    }

    /// The protocol-facing runtime handle (DI container, scope orders, hooks).
    pub fn runtime(&self) -> &AppRuntime {
        &self.runtime
    }

    /// The installed protocol.
    pub fn protocol(&self) -> &<D::Prepared as PreparedProtocol>::Runtime {
        &self.protocol
    }

    /// The immutable effective plugin plan used to assemble this application.
    pub fn plugin_plan(&self) -> &EffectivePluginPlan {
        &self.plugin_plan
    }

    /// The stable identity of the selected protocol definition.
    pub const fn protocol_id(&self) -> crate::ProtocolId {
        D::ID
    }

    /// Returns a handle that can trigger graceful shutdown from any spawned task.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.shutdown.handle()
    }

    /// The handle that requests a transactional config-and-graph reload of this app — the
    /// same [`RuntimeReloader`] components inject and the watch and signal triggers'
    /// [`AppRuntime::reload_config`] path.
    pub fn reloader(&self) -> RuntimeReloader {
        self.reloader.clone()
    }

    /// The hook manager of the currently committed runtime generation, for running
    /// lifecycle/event hooks by kind.
    pub fn hook_manager(&self) -> HookManager {
        self.runtime.hooks()
    }

    /// Serves the app's protocol over `endpoint` until ctrl-c or a shutdown signal.
    ///
    /// The agnostic envelope: runs startup hooks, spawns config-reload triggers, bridges
    /// ctrl-c to the shutdown signal, then hands the runtime + shutdown signal to the
    /// protocol's [`Serve`] impl. Shutdown hooks run on the way out, on the generation
    /// current when shutdown begins rather than the one pinned at startup.
    pub async fn serve<E>(
        self,
        endpoint: E,
    ) -> Result<(), <<D::Prepared as PreparedProtocol>::Runtime as ProtocolRuntime>::Error>
    where
        <D::Prepared as PreparedProtocol>::Runtime: Serve<E>,
        <<D::Prepared as PreparedProtocol>::Runtime as ProtocolRuntime>::Error: From<crate::Error>,
    {
        let App {
            runtime,
            protocol,
            shutdown,
            reload_triggers,
            ..
        } = self;

        let started = match run_startup(&runtime).await {
            Ok(started) => started,
            Err((error, started)) => {
                run_shutdown(&runtime, &started).await;

                return Err(error.into());
            }
        };

        let trigger_tasks = spawn_reload_triggers(runtime.clone(), reload_triggers);

        let shutdown_handle = shutdown.handle();
        let ctrlc = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!(target: "upwell::app", "ctrl-c received, shutting down");
                shutdown_handle.shutdown();
            }
        });

        let result = std::panic::AssertUnwindSafe(async {
            protocol.serve(runtime.clone(), shutdown, endpoint).await
        })
        .catch_unwind()
        .await;

        ctrlc.abort();
        let _ = ctrlc.await;

        stop_reload_triggers(trigger_tasks).await;

        run_shutdown(&runtime, &started).await;

        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// Waits for ctrl-c or a shutdown signal without serving any endpoint.
    pub async fn run(self) -> crate::Result<()> {
        let App {
            runtime,
            mut shutdown,
            reload_triggers,
            ..
        } = self;

        let started = match run_startup(&runtime).await {
            Ok(started) => started,
            Err((error, started)) => {
                run_shutdown(&runtime, &started).await;

                return Err(error);
            }
        };

        let trigger_tasks = spawn_reload_triggers(runtime.clone(), reload_triggers);

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = shutdown.wait() => {},
        }

        stop_reload_triggers(trigger_tasks).await;

        run_shutdown(&runtime, &started).await;

        Ok(())
    }
}

#[cfg(test)]
mod tests;
