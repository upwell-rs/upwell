//! A request scope none of whose components captures it is built without a resolver slot,
//! and still hands request-time handles a slot onto the built scope.
#![cfg(feature = "daemon")]

mod common;

use std::sync::Arc;

use upwell::daemon::{App, Inject, handlers, service};
use upwell::{CallResult, Lazy, component};

use common::{MemoryServer, deadline};

/// A request-scoped component with only eager fields, so its scope captures nothing.
#[component(scope = upwell::daemon::Request)]
struct Plain {
    #[default]
    id: u64,
}

#[service(id = "capture", version = "0.1")]
struct CaptureSvc;

#[handlers]
impl CaptureSvc {
    /// Resolves the request component through a lazy handle injected after the scope is
    /// built.
    #[rpc]
    async fn lazy(Inject(plain): Inject<Lazy<Arc<Plain>>>) -> upwell::daemon::Result<bool> {
        Ok(plain.get_or_create().await.is_ok_and(|plain| plain.id == 0))
    }
}

#[tokio::test]
async fn a_lazy_handle_injected_after_a_non_capturing_build_resolves() {
    let daemon = App::builder("scope-capture-test")
        .auto_discover()
        .build()
        .await
        .expect("build daemon");
    let server = MemoryServer::start(daemon);
    let conn = server.connect().await;
    let result = deadline(
        "CaptureSvc.lazy call",
        conn.call(
            "CaptureSvc.lazy",
            postcard::to_allocvec(&()).expect("encodes"),
        ),
    )
    .await
    .expect("lazy call returns a response");
    let resolved = match result {
        CallResult::Ok(body) => postcard::from_bytes::<bool>(&body).expect("decodes"),
        CallResult::Err { .. } => panic!("lazy call errored"),
    };

    assert!(resolved, "the built scope's slot resolves the lazy handle");

    server.shutdown([conn]).await;
}
