//! Serving one accepted connection: its scope and the tasks that drive its calls.

use std::sync::Arc;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, instrument, warn};
use upwell_core::Scope;
use upwell_di::context::TargetRef;
use upwell_transport::Connection;

use super::RpcScopes;
use super::call::drive_call;
use crate::middleware::{ErrorHandler, RpcService};
use crate::scope::{Connection as ConnectionScope, Request as RequestScope};
use crate::target::RpcTarget;

/// Serves one connection: builds the connection scope (seeding the peer when a
/// component depends on it), then drives each inbound call on its own task so
/// streaming calls run concurrently and the connection keeps reading inbound frames
/// while handlers run.
#[instrument(
    target = "upwell::daemon",
    level = "debug",
    skip_all,
    fields(peer = ?conn.peer().addr),
    name = "connection"
)]
pub(super) async fn serve_connection<C: Connection>(
    mut conn: C,
    service: RpcService,
    error_handler: Option<Arc<dyn ErrorHandler>>,
    needs_peer: bool,
    scopes: Arc<RpcScopes>,
    shutdown: CancellationToken,
    max_calls: usize,
) {
    debug!(target: "upwell::daemon", "connection established");

    // The peer (by value — the framework's connection-scoped injectable) is seeded
    // only when a component depends on it; handlers reach it through the `Peer`
    // extractor regardless, so an otherwise-empty connection scope is skipped. A
    // failed factory closes the connection.
    let peer = conn.peer().clone();
    let mut builder = scopes.connections.builder(ConnectionScope);

    if needs_peer {
        builder = builder.seed(peer.clone());
    }

    let connection_scope = match builder.open_on_demand().await {
        Ok(scope) => scope,

        Err(e) => {
            error!(target: "upwell::daemon", error = %e, "connection scope build failed, closing");
            return;
        }
    };

    let mut tasks: JoinSet<()> = JoinSet::new();

    debug!(target: "upwell::daemon", "connection ready");

    loop {
        tokio::select! {
            recv = conn.recv() => match recv {
                Ok(Some((call, responder))) => {
                    while let Some(result) = tasks.try_join_next() {
                        observe_call_task(result);
                    }

                    if tasks.len() >= max_calls {
                        warn!(
                            target: "upwell::daemon",
                            max_calls,
                            "connection exceeded its in-flight call limit; closing"
                        );
                        call.cancel.cancel();
                        break;
                    }

                    let path = call.path;
                    let service = service.clone();
                    let error_handler = error_handler.clone();
                    let connection_scope = connection_scope.clone();
                    let scopes = Arc::clone(&scopes);
                    let peer = peer.clone();
                    let context = scopes
                        .calls
                        .context(TargetRef::from_static(&RpcTarget))
                        .with_pending_scope(
                            RequestScope::KEY,
                            Some(Arc::clone(connection_scope.container())),
                        );

                    debug!(target: "upwell::daemon", %path, "dispatching call");

                    tasks.spawn(context.enter(drive_call(
                        path,
                        call.payload,
                        call.requests,
                        call.cancel,
                        peer,
                        connection_scope,
                        scopes,
                        responder,
                        service,
                        error_handler,
                    )));
                }

                Ok(None) => {
                    debug!(target: "upwell::daemon", "connection closed by peer");
                    break;
                }

                Err(e) => {
                    warn!(target: "upwell::daemon", error = %e, "connection error");
                    break;
                }
            },

            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(result) = result {
                    observe_call_task(result);
                }
            }

            _ = shutdown.cancelled() => {
                debug!(target: "upwell::daemon", "connection shutdown requested");
                break;
            }
        }
    }

    // The connection (and its call table) is dropped here, cancelling in-flight
    // calls via their tokens; abort any handler tasks still winding down.
    tasks.shutdown().await;

    debug!(target: "upwell::daemon", "connection ended");
}

fn observe_call_task(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        warn!(target: "upwell::daemon", %error, "call task failed");
    }
}
