//! The protocol-wide request timeout.

use std::convert::Infallible;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::FutureExt;
use futures::future::Map;
use tokio::time::Timeout;
use tokio::time::error::Elapsed;
use tower::{Layer, Service};

/// Answers `408 Request Timeout` when user middleware and the handler together run longer
/// than the configured timeout.
#[derive(Clone, Copy)]
pub(super) struct RequestTimeoutLayer {
    timeout: Duration,
}

/// The service [`RequestTimeoutLayer`] wraps around the router's routes.
#[derive(Clone)]
pub(super) struct RequestTimeout<S> {
    inner: S,
    timeout: Duration,
}

/// The outcome of a timed request, as the response it becomes.
type Timed = fn(Result<Result<Response, Infallible>, Elapsed>) -> Result<Response, Infallible>;

impl RequestTimeoutLayer {
    pub(super) fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl<S> Layer<S> for RequestTimeoutLayer {
    type Service = RequestTimeout<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestTimeout {
            inner,
            timeout: self.timeout,
        }
    }
}

impl<S> Service<Request> for RequestTimeout<S>
where
    S: Service<Request, Response = Response, Error = Infallible>,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Map<Timeout<S::Future>, Timed>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        tokio::time::timeout(self.timeout, self.inner.call(request)).map(timed)
    }
}

fn timed(outcome: Result<Result<Response, Infallible>, Elapsed>) -> Result<Response, Infallible> {
    outcome.unwrap_or_else(|_| Ok(StatusCode::REQUEST_TIMEOUT.into_response()))
}
