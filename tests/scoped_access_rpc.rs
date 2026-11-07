#![cfg(feature = "daemon")]

//! End-to-end tests for declared access to an RPC call's scope from a singleton whose handler
//! injects nothing call-scoped, so only the accessor reaches the call scope.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use common::{MemoryServer, deadline};
use upwell::context::{Current, Scoped, ScopedError};
use upwell::daemon::{App, Inject, Request, handlers, service};
use upwell::{CallResult, MemoryConnectionHandle, component, methods};

/// Numbers handed out to [`CallTicket`]s.
static ISSUED: AtomicU32 = AtomicU32::new(1);

/// A call-scoped component numbered per call, reached only through accessors.
#[component(scope = Request)]
struct CallTicket {
    #[default]
    number: u32,
}

#[methods]
impl CallTicket {
    #[init]
    async fn init() -> Self {
        Self {
            number: ISSUED.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// A singleton reading the call's ticket on each call, closest and pinned.
#[component]
struct Counter {
    closest: Current<Arc<CallTicket>>,
    pinned: Scoped<Request, Arc<CallTicket>>,
}

impl Counter {
    async fn numbers(&self) -> Result<(u32, u32), ScopedError> {
        let closest = self.closest.get().await?;
        let pinned = self.pinned.get().await?;

        Ok((closest.number, pinned.number))
    }
}

/// Handlers injecting only the singleton.
#[service(id = "access_svc", version = "0.1")]
struct AccessSvc;

#[handlers]
impl AccessSvc {
    #[rpc]
    async fn read(Inject(counter): Inject<Arc<Counter>>) -> (u32, u32) {
        counter
            .numbers()
            .await
            .expect("the singleton reads the call's ticket")
    }
}

async fn read(connection: &MemoryConnectionHandle) -> (u32, u32) {
    let body = postcard::to_allocvec(&()).expect("the request encodes");
    let result = deadline("the call", connection.call("AccessSvc.read", body))
        .await
        .expect("the transport responds");

    match result {
        CallResult::Ok(body) => postcard::from_bytes(&body).expect("two numbers"),
        CallResult::Err { code, .. } => panic!("the call failed with {code:?}"),
    }
}

#[tokio::test]
async fn a_singleton_reads_the_call_scope_its_handler_never_injects() {
    let app = App::builder("scoped-access-rpc-test")
        .auto_discover()
        .build()
        .await
        .expect("the app builds");
    let server = MemoryServer::start(app);
    let connection = server.connect().await;

    let (first_closest, first_pinned) = read(&connection).await;
    let (second_closest, second_pinned) = read(&connection).await;

    assert_eq!(
        first_closest, first_pinned,
        "both accessors read one ticket"
    );
    assert_eq!(
        second_closest, second_pinned,
        "both accessors read one ticket"
    );
    assert_ne!(
        first_closest, second_closest,
        "each call has its own ticket"
    );

    server.shutdown([connection]).await;
}
