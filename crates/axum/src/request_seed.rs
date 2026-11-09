//! Seeding application values, such as an authenticated identity, into the HTTP request scope.
//!
//! A [`RequestSeeder`] runs for every request a controller route serves, before the request
//! scope opens: inside the parent layers, before global middleware and eager request-scoped
//! components. A WebSocket upgrade opens no request scope, so no seeder runs for it. It
//! produces the seed from the request or rejects the request with a response. The seed waits
//! in the request's extensions until the scope opens, and the opening fills the scope's slot of
//! its type with it, so request-scoped components depend on it like on any other seed.
//! Registering a seeder with [`AxumAppBuilder::request_seed`](crate::AxumAppBuilder::request_seed)
//! also claims the seed for the request scope, which preparation checks `#[context]` values
//! against.

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::Extensions;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use upwell_app::Seeds;

use crate::middleware::AxumMiddleware;
use crate::request_scope::is_controller_route;

/// Produces one value seeded into each routed request's [`HttpRequest`](crate::HttpRequest)
/// scope, or rejects the request.
///
/// It is a DI singleton: implement it on a `#[component]` holding what it needs (a token
/// verifier, a session store) and register it with
/// [`request_seed`](crate::AxumAppBuilder::request_seed). An optional authentication returns
/// an explicit anonymous value rather than nothing, so consumers always find the seed.
pub trait RequestSeeder: Send + Sync + 'static {
    /// The seeded value: a by-value injectable such as a `#[context]` type, cloned once into
    /// the request scope when it opens.
    type Seed: Clone + Send + Sync + 'static;

    /// The seed for the request whose head is `request`, or the response that rejects it. The
    /// body stays with the request: a seeder reads the head only.
    fn seed(&self, request: &Parts) -> impl Future<Output = Result<Self::Seed, Response>> + Send;
}

/// Reads the seed a seeder left in a request's extensions into the seeds its scope opens with.
pub(crate) type SeedReader = fn(&Extensions, &mut Seeds);

/// The value a seeder produced for one request, waiting for the request's scope to open.
#[derive(Clone)]
struct PendingSeed<T>(T);

/// Runs the seeder `M` as middleware, leaving its seed in the request's extensions.
pub(crate) struct SeederMiddleware<M>(pub(crate) Arc<M>);

impl<M: RequestSeeder> AxumMiddleware for SeederMiddleware<M> {
    async fn handle(&self, request: Request, next: Next) -> Response {
        let (mut head, body) = request.into_parts();

        if !is_controller_route(&head) {
            return next.run(Request::from_parts(head, body)).await;
        }

        match self.0.seed(&head).await {
            Ok(seed) => {
                head.extensions.insert(PendingSeed(seed));

                next.run(Request::from_parts(head, body)).await
            }

            Err(rejection) => rejection,
        }
    }
}

/// Adds the seed of type `T` waiting in `extensions`, if any, to `seeds`.
pub(crate) fn read_seed<T: Any + Clone + Send + Sync>(extensions: &Extensions, seeds: &mut Seeds) {
    if let Some(PendingSeed(seed)) = extensions.get::<PendingSeed<T>>() {
        seeds.insert(seed.clone());
    }
}

/// The seeds every reader finds in `extensions`.
pub(crate) fn collect(readers: &[SeedReader], extensions: &Extensions) -> Seeds {
    let mut seeds = Seeds::new();

    for read in readers {
        read(extensions, &mut seeds);
    }

    seeds
}

#[cfg(test)]
mod tests;
