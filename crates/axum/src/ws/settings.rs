//! Framework-owned WebSocket limits: admission, liveness and per-connection settings.

use std::sync::Arc;

use tokio::time::Duration;
use upwell_config::ContainerConfigExt;
use upwell_di::ScopeContainer;

/// Framework-owned controls resolved from each WebSocket connection's config store. This remains
/// internal so adding operational controls does not change the public protocol implementation
/// contract.
#[derive(Clone, Copy, Debug)]
pub struct WsConnectionSettings {
    idle_timeout: Option<Duration>,
}

impl WsConnectionSettings {
    /// Reads framework-owned WebSocket settings from a connection scope.
    pub fn from_connection(connection: &ScopeContainer) -> Self {
        let timeout_ms = connection
            .config::<crate::AxumConfig>(crate::AXUM_CONFIG_PATH)
            .map(|config| config.snapshot().websocket_idle_timeout_ms)
            .unwrap_or_else(|| crate::AxumConfig::default().websocket_idle_timeout_ms);

        Self {
            idle_timeout: (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms)),
        }
    }

    /// The configured idle interval, or `None` when liveness probing is disabled.
    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }
}

/// Tracks peer activity without spawning a feeder or timer task. A silent connection is probed
/// once, then dropped if no frame (normally a pong) arrives during the following interval.
pub struct WsIdle {
    timeout: Option<Duration>,
    deadline: Option<tokio::time::Instant>,
    awaiting_probe_reply: bool,
}

impl WsIdle {
    /// Builds the liveness state from one connection's framework settings.
    pub fn from_connection(connection: &ScopeContainer) -> Self {
        Self::new(WsConnectionSettings::from_connection(connection).idle_timeout)
    }

    /// Builds liveness state for an explicit idle interval.
    pub fn new(timeout: Option<Duration>) -> Self {
        let deadline = timeout.map(|timeout| tokio::time::Instant::now() + timeout);

        Self {
            timeout,
            deadline,
            awaiting_probe_reply: false,
        }
    }

    /// Waits until the current idle deadline, or forever when probing is disabled.
    pub async fn wait(&self) {
        match self.deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending().await,
        }
    }

    /// Records peer activity and resets the liveness deadline.
    pub fn on_activity(&mut self) {
        self.awaiting_probe_reply = false;
        self.reset_deadline();
    }

    /// Returns `true` when the peer already ignored one probe and must be disconnected.
    pub fn on_timeout(&mut self) -> bool {
        if self.awaiting_probe_reply {
            return true;
        }

        self.awaiting_probe_reply = true;
        self.reset_deadline();

        false
    }

    fn reset_deadline(&mut self) {
        self.deadline = self
            .timeout
            .map(|timeout| tokio::time::Instant::now() + timeout);
    }
}

/// Per-endpoint WebSocket admission gate. An owned permit is captured by the upgrade future and is
/// released automatically when that connection finishes, including scope-build and panic unwind.
#[derive(Clone)]
pub(super) struct WsAdmission {
    permits: Option<Arc<tokio::sync::Semaphore>>,
}

impl WsAdmission {
    pub(super) fn new(max_connections: usize) -> crate::Result<Self> {
        if max_connections > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(crate::Error::Config(format!(
                "max_websocket_connections ({max_connections}) exceeds Tokio's semaphore limit ({})",
                tokio::sync::Semaphore::MAX_PERMITS
            )));
        }

        Ok(Self {
            permits: (max_connections > 0)
                .then(|| Arc::new(tokio::sync::Semaphore::new(max_connections))),
        })
    }

    pub(super) fn try_acquire(
        &self,
    ) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, tokio::sync::TryAcquireError> {
        self.permits
            .as_ref()
            .map(|permits| Arc::clone(permits).try_acquire_owned())
            .transpose()
    }
}

/// Temporary protocol-level guard until generic extraction-time validation is available.
pub(crate) fn validate_config(config: &crate::AxumConfig) -> crate::Result<()> {
    if config.max_websocket_message_bytes == 0 || config.max_websocket_frame_bytes == 0 {
        return Err(crate::Error::Config(
            "WebSocket message and frame byte limits must both be greater than zero".to_owned(),
        ));
    }

    if config.max_websocket_connections > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(crate::Error::Config(format!(
            "max_websocket_connections ({}) exceeds Tokio's semaphore limit ({})",
            config.max_websocket_connections,
            tokio::sync::Semaphore::MAX_PERMITS
        )));
    }

    Ok(())
}
