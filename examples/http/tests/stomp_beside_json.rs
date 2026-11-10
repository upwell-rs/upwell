//! A STOMP handler injecting STOMP's own message seeds while a JSON-WS endpoint, which fills
//! none of them, is mounted beside it (#128): each endpoint is checked only for the handlers it
//! serves.

use upwell::axum::prelude::*;
use upwell::axum::{CodecError, StompHeaders, StompSession};
use upwell::prelude::*;
use upwell_test_utils::TestEnvironment;

app! {
    /// Generated host for the mixed-endpoint test.
    app MixedEndpointsApplication {
        name: "stomp-beside-json-test",
        protocol: upwell::axum::Axum,
    }
}

/// A request a STOMP client sends.
#[dto]
struct Ask {}

/// A STOMP controller whose handler reads the frame's headers and its session.
#[controller(ws = Stomp)]
struct StompHandlers {}

#[handlers(ws = Stomp)]
impl StompHandlers {
    #[message("/app/destination")]
    async fn destination(
        &self,
        _ask: Ask,
        Inject(headers): Inject<StompHeaders>,
        Inject(_session): Inject<StompSession>,
    ) -> Result<String, CodecError> {
        Ok(headers.get("destination").unwrap_or_default().to_owned())
    }
}

/// A JSON-WS controller that injects nothing STOMP seeds.
#[controller(ws = JsonWs)]
struct JsonHandlers;

#[handlers(ws = JsonWs)]
impl JsonHandlers {
    #[message("ping")]
    async fn ping(&self) -> u32 {
        0
    }
}

#[tokio::test]
async fn a_stomp_handler_reads_stomp_seeds_beside_a_json_endpoint() {
    let environment = TestEnvironment::new("upwell-http-stomp-beside-json-");

    MixedEndpointsApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<Stomp>("/stomp")
        .register_ws::<JsonWs>("/json")
        .build()
        .await
        .expect("the JSON-WS endpoint serves no handler that reads a STOMP seed");
}
