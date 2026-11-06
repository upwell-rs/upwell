//! The native RPC protocol.
//!
//! [`RpcRuntime`] implements the [`ProtocolRuntime`]/[`Serve`] traits from `upwell-app`: it owns the
//! router + middleware stack and drives the per-connection / per-call loop over any
//! [`Transport`](upwell_transport::Transport), opening connection and request scopes
//! through [`ScopeOpener`]s prepared from the [`AppRuntime`] once per serve. The serve envelope (lifecycle hooks, reload triggers,
//! ctrl-c) is run by `App::serve`, so this loop only watches its transport and the
//! shutdown signal.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use upwell_app::{AppRuntime, ProtocolRuntime, ScopeEntry, ScopeOpener, Serve, ShutdownSignal};
use upwell_transport::{Connection, Error as TransportError, Transport};

use crate::middleware::{ErrorHandler, RpcService};
use crate::router::RpcRouter;
use crate::scope::{Connection as ConnectionScope, Request as RequestScope};

mod accept;
mod call;
mod connection;

use accept::is_transient_accept_error;
use connection::serve_connection;

/// Default number of concurrently served RPC connections.
pub const DEFAULT_MAX_CONNECTIONS: usize = 1024;

/// Default number of handler tasks allowed on one RPC connection.
pub const DEFAULT_MAX_CALLS_PER_CONNECTION: usize = 256;

const DEFAULT_ACCEPT_RETRY_INITIAL: Duration = Duration::from_millis(25);
const DEFAULT_ACCEPT_RETRY_MAX: Duration = Duration::from_secs(1);

/// Admission and transient-accept limits for the RPC server.
#[derive(Clone, Copy, Debug)]
pub struct RpcLimits {
    max_connections: usize,
    max_calls_per_connection: usize,
    accept_retry_initial: Duration,
    accept_retry_max: Duration,
}

impl RpcLimits {
    pub fn new(max_connections: usize, max_calls_per_connection: usize) -> Self {
        assert!(max_connections > 0, "maximum connections must be non-zero");
        assert!(
            max_calls_per_connection > 0,
            "maximum calls per connection must be non-zero"
        );

        Self {
            max_connections,
            max_calls_per_connection,
            accept_retry_initial: DEFAULT_ACCEPT_RETRY_INITIAL,
            accept_retry_max: DEFAULT_ACCEPT_RETRY_MAX,
        }
    }

    pub fn with_accept_backoff(mut self, initial: Duration, maximum: Duration) -> Self {
        assert!(
            !initial.is_zero(),
            "initial accept backoff must be non-zero"
        );
        assert!(
            maximum >= initial,
            "maximum accept backoff is below initial"
        );
        self.accept_retry_initial = initial;
        self.accept_retry_max = maximum;
        self
    }

    pub fn max_connections(self) -> usize {
        self.max_connections
    }

    pub fn max_calls_per_connection(self) -> usize {
        self.max_calls_per_connection
    }
}

impl Default for RpcLimits {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_CALLS_PER_CONNECTION)
    }
}

/// The built native RPC runtime served over a [`Transport`].
pub struct RpcRuntime {
    router: Arc<RpcRouter>,
    service: RpcService,
    error_handler: Option<Arc<dyn ErrorHandler>>,
    needs_peer: bool,
    limits: RpcLimits,
}

impl RpcRuntime {
    pub(crate) fn new(
        router: Arc<RpcRouter>,
        service: RpcService,
        error_handler: Option<Arc<dyn ErrorHandler>>,
        needs_peer: bool,
        limits: RpcLimits,
    ) -> Self {
        Self {
            router,
            service,
            error_handler,
            needs_peer,
            limits,
        }
    }

    /// The number of routes the dispatcher resolves.
    pub fn route_count(&self) -> usize {
        self.router.route_count()
    }

    pub fn limits(&self) -> RpcLimits {
        self.limits
    }
}

impl ProtocolRuntime for RpcRuntime {
    type Error = crate::Error;
}

impl<T> Serve<T> for RpcRuntime
where
    T: Transport,
    T::Connection: 'static,
{
    async fn serve(
        self,
        runtime: AppRuntime,
        mut shutdown: ShutdownSignal,
        mut transport: T,
    ) -> crate::Result<()> {
        let transport_name = std::any::type_name::<T>();

        info!(target: "upwell::daemon", app = runtime.name(), transport = transport_name, "serve starting");

        let calls = runtime.opener::<RequestScope>()?;
        let entries = self
            .router
            .entries()
            .map(|(path, key)| Ok((path.to_owned(), calls.entry(key)?)))
            .collect::<crate::Result<_>>()?;
        let scopes = Arc::new(RpcScopes {
            connections: runtime.opener()?,
            calls,
            entries,
        });
        let connection_cancel = CancellationToken::new();
        let mut connections = JoinSet::new();
        let mut serve_error = None;
        let mut accept_retry = self.limits.accept_retry_initial;

        'serve: loop {
            tokio::select! {
                result = transport.accept(), if connections.len() < self.limits.max_connections => {
                    match result {
                        Ok(conn) => {
                            accept_retry = self.limits.accept_retry_initial;
                            debug!(target: "upwell::daemon", peer = ?conn.peer().addr, "connection accepted, spawning task");

                            let service = self.service.clone();
                            let error_handler = self.error_handler.clone();
                            let needs_peer = self.needs_peer;
                            let scopes = Arc::clone(&scopes);
                            let cancel = connection_cancel.clone();
                            let max_calls = self.limits.max_calls_per_connection;
                            connections.spawn(async move {
                                serve_connection(conn, service, error_handler, needs_peer, scopes, cancel, max_calls)
                                    .await;
                            });
                        }

                        Err(TransportError::Io(e)) if is_transient_accept_error(&e) => {
                            warn!(target: "upwell::daemon", error = %e, retry_in = ?accept_retry, "transient transport accept failure");

                            tokio::select! {
                                _ = tokio::time::sleep(accept_retry) => {}
                                _ = shutdown.wait() => {
                                    info!(target: "upwell::daemon", "shutdown signal received during accept backoff");
                                    connection_cancel.cancel();
                                    break 'serve;
                                }
                            }

                            accept_retry = accept_retry
                                .saturating_mul(2)
                                .min(self.limits.accept_retry_max);
                        }

                        Err(TransportError::Closed) => break,

                        Err(e) => {
                            error!(target: "upwell::daemon", error = %e, "terminal transport accept failure");
                            connection_cancel.cancel();
                            serve_error = Some(e.into());
                            break;
                        }
                    }
                }

                result = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(e)) = result {
                        warn!(target: "upwell::daemon", error = %e, "connection task failed");
                    }
                }

                _ = shutdown.wait() => {
                    info!(target: "upwell::daemon", "shutdown signal received");
                    connection_cancel.cancel();
                    break;
                }
            }
        }

        while !connections.is_empty() {
            tokio::select! {
                result = connections.join_next() => {
                    if let Some(Err(e)) = result {
                        warn!(target: "upwell::daemon", error = %e, "connection task failed during shutdown");
                    }
                }

                _ = shutdown.wait() => {
                    connection_cancel.cancel();
                    break;
                }
            }
        }

        while let Some(result) = connections.join_next().await {
            if let Err(e) = result {
                warn!(target: "upwell::daemon", error = %e, "connection task failed during shutdown");
            }
        }

        info!(target: "upwell::daemon", transport = transport_name, "serve stopped");

        match serve_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// The openers of the scopes the RPC protocol opens, prepared once per serve.
///
/// A call to a routed method opens its request scope with the method's entry plan.
pub(super) struct RpcScopes {
    pub(super) connections: ScopeOpener<ConnectionScope>,
    pub(super) calls: ScopeOpener<RequestScope>,
    pub(super) entries: HashMap<String, ScopeEntry<RequestScope>>,
}

#[cfg(test)]
mod tests;
