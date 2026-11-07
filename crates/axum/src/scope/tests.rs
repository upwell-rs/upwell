use upwell_app::ScopeParent;
use upwell_core::Scope;

use super::{HttpRequest, SCOPE_TOPOLOGY};

#[cfg(feature = "ws")]
use super::{WebsocketConnection, WebsocketMessage};

#[test]
fn http_request_has_stable_identity_and_root_parent() {
    let topology = SCOPE_TOPOLOGY.prepare().expect("Axum topology is valid");

    assert_eq!(HttpRequest::NAME, "HttpRequest");
    assert_eq!(
        topology.parents_of(&HttpRequest::ID),
        Some(&[ScopeParent::Root][..])
    );
}

#[cfg(feature = "ws")]
#[test]
fn websocket_scopes_are_distinct_and_messages_descend_from_connections() {
    let topology = SCOPE_TOPOLOGY.prepare().expect("Axum topology is valid");

    assert_ne!(
        <HttpRequest as Scope>::ID,
        <WebsocketConnection as Scope>::ID
    );
    assert_ne!(<HttpRequest as Scope>::ID, <WebsocketMessage as Scope>::ID);
    assert_ne!(
        <WebsocketConnection as Scope>::ID,
        <WebsocketMessage as Scope>::ID
    );
    assert!(topology.is_reachable(
        &<WebsocketMessage as Scope>::ID,
        &<WebsocketConnection as Scope>::ID
    ));
    assert!(!topology.is_reachable(
        &<HttpRequest as Scope>::ID,
        &<WebsocketConnection as Scope>::ID
    ));
}
