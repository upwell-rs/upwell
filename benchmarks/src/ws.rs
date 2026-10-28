//! An in-process WebSocket fixture: a real axum application whose messages open their
//! `WebsocketMessage` scope under one `WebsocketConnection` scope, as a WebSocket protocol does
//! for every inbound message.
//!
//! No socket or frame codec is involved. Destination lookup and payload framing are the
//! protocol's own work and the same for every case, so a bench measures the per-message scope
//! and resolution alone. The handler future is boxed, as every WebSocket handler's is.

use std::sync::Arc;

use futures::future::BoxFuture;
use upwell_axum::Axum;
use upwell_axum::scope::{WebsocketConnection, WebsocketMessage};
use upwell_core::Scope;
use upwell_di::ScopeContainer;

use crate::scoped::{self, CallScope, CallScoped, Route, ScopedCalls, ScopedProtocol};

fn handler(route: Route, scope: Arc<ScopeContainer>) -> BoxFuture<'static, bool> {
    Box::pin(async move {
        match route {
            Route::Hello => true,
            Route::Scoped => scope.get::<CallScoped<0>>().is_some(),
        }
    })
}

/// An axum application whose messages open a `WebsocketMessage` scope under one connection.
pub struct WsMessages {
    calls: ScopedCalls<Axum>,
}

impl ScopedProtocol for WsMessages {
    async fn build(count: usize) -> Self {
        let calls = ScopedCalls::build(
            scoped::builder("ws-message-bench"),
            WebsocketConnection::KEY,
            WebsocketMessage::KEY,
            count,
        )
        .await;

        Self { calls }
    }

    async fn call(&self, scope: CallScope, route: Route) -> bool {
        handler(route, self.calls.scope(scope).await).await
    }
}
