use std::sync::Arc;

use serde::Serializer;
use serde::ser::Error as _;
use upwell_app::{App, EntryDescriptor};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::EntryKey;
use upwell_di::Component;
use upwell_transport::{PeerInfo, PredefinedCode};

use super::*;
use crate::Rpc;
use crate::scope::{Connection, Request};

/// A type whose serialization always fails, to exercise the FR-011 fallback.
struct Unserializable;

impl Serialize for Unserializable {
    fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(S::Error::custom("intentional serialization failure"))
    }
}

#[test]
fn body_serialization_failure_preserves_code_with_empty_body() {
    // FR-011: a body that fails to serialize yields the intended code with a
    // fallback (empty) body rather than dropping the code or panicking.
    let code = StatusCode::from(PredefinedCode::NotFound);
    let response = ErrorResponse::with_serialized_body(code, &Unserializable);

    assert_eq!(response.code, code);
    assert!(response.body.is_empty());
}

#[test]
fn serializable_error_uses_blanket_impl() {
    // A `Serialize` error gets the blanket `ResponseError`: the default
    // `Internal` code, with the error value itself as the body.
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct PlainError {
        reason: String,
    }

    let error = PlainError {
        reason: "boom".to_string(),
    };
    let response = error.error_response();
    let decoded: PlainError = postcard::from_bytes(&response.body).expect("decode body");

    assert_eq!(decoded.reason, "boom");
    assert_eq!(response.code.predefined(), PredefinedCode::Internal);
}

/// A singleton a call's handler injects.
struct Clock;

impl Component for Clock {
    type Handle = Arc<Self>;

    const ID: &'static str = "rpc_extract_clock";
    const NAME: &'static str = "Clock";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

#[tokio::test]
async fn injecting_a_planned_singleton_leaves_a_deferred_call_scope_unopened() {
    let key = EntryKey::new("rpc/test.clock").expect("the entry key is valid");
    let app = App::<Rpc>::builder("rpc-inject-test")
        .config_source(ConfigManager::<Dynamic>::empty())
        .with_component(Clock)
        .entry_descriptor(EntryDescriptor::new::<Request, (Arc<Clock>,)>(key))
        .build()
        .await
        .expect("the app builds");
    let connection = app
        .runtime()
        .opener::<Connection>()
        .expect("the connection scope is declared")
        .builder()
        .open_on_demand()
        .await
        .expect("the connection opens");
    let calls = app
        .runtime()
        .opener::<Request>()
        .expect("the call scope is declared");
    let entry = calls.entry(key).expect("the entry resolves");
    let scope = calls
        .builder()
        .under(&connection)
        .open_when_needed(Some(&entry))
        .await
        .expect("the call scope opens deferred");
    let ctx = RpcCallContext::new(
        Vec::new(),
        PeerInfo { addr: None },
        scope,
        None,
        CancellationToken::new(),
    );

    Inject::<Arc<Clock>>::from_context(&ctx)
        .await
        .expect("the singleton injects");

    assert!(
        !ctx.scope().is_open(),
        "injecting a planned singleton opened the call scope"
    );
}
