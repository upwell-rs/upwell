use std::collections::HashMap;

use tracing::{debug, instrument, trace, warn};
use upwell_core::EntryKey;

use crate::{
    Error,
    descriptors::{RpcCallContext, RpcHandler, RpcOutcome},
    extract::ErrorResponse,
    routes::ResolvedService,
};

/// Routes incoming RPC calls to their registered handlers by path.
///
/// Paths take the form `ServiceName.rpc_name`, matching the convention used
/// in registry validation.
pub struct RpcRouter {
    routes: HashMap<String, RpcHandler>,
    entries: HashMap<String, EntryKey>,
}

impl RpcRouter {
    pub fn from_services(services: &[ResolvedService]) -> Self {
        let mut routes = HashMap::new();
        let mut entries = HashMap::new();

        for service in services {
            for rpc in &service.rpcs {
                let path = format!("{}.{}", service.descriptor.name, rpc.name);
                debug!(%path, "registered route");
                entries.insert(path.clone(), rpc.key);
                routes.insert(path, rpc.handler);
            }
        }

        debug!(count = routes.len(), "router built");

        Self { routes, entries }
    }

    #[instrument(level = "debug", skip_all, fields(%path))]
    pub async fn dispatch(
        &self,
        path: &str,
        ctx: RpcCallContext,
    ) -> Result<RpcOutcome, ErrorResponse> {
        trace!("looking up handler");

        let Some(handler) = self.routes.get(path) else {
            warn!("route not found");
            return Err(Error::RouteNotFound(path.to_string()).into());
        };

        trace!("invoking handler");

        let result = handler(ctx).await;

        match &result {
            Ok(_) => trace!("handler succeeded"),
            Err(e) => warn!(code = ?e.code, "handler returned error"),
        }

        result
    }

    /// Returns the number of registered routes.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Returns each route path with the stable key of the entry it runs.
    pub fn entries(&self) -> impl Iterator<Item = (&str, EntryKey)> {
        self.entries.iter().map(|(path, key)| (path.as_str(), *key))
    }

    /// Returns an iterator over all registered route paths.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.routes.keys().map(String::as_str)
    }
}
