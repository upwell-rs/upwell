//! Each WebSocket endpoint as an opener of the connection and message scopes.

use upwell_app::OpenerSeeds;
use upwell_core::{EntryKey, Scope, ScopeKey};

use super::WsRegistration;
use crate::scope::{WebsocketConnection, WebsocketMessage};
use crate::ws::WsControllerRegistration;

/// The endpoint `registration` as an opener of the connection and message scopes: the seeds
/// it fills in each, and the message handlers of `controllers` it serves, so a requirement
/// only another endpoint's handlers reach is not checked against it.
pub(super) fn of(
    registration: &WsRegistration,
    controllers: &[WsControllerRegistration],
) -> [OpenerSeeds; 2] {
    let opener = format!("{} at {}", registration.protocol_name, registration.path);
    let entries = served_entries(registration, controllers);
    let seeds = |scope: ScopeKey| {
        registration
            .claims
            .iter()
            .filter(move |claim| claim.scope.id() == scope.id())
            .map(|claim| claim.ty)
    };

    [
        OpenerSeeds {
            opener: opener.clone(),
            scope: WebsocketConnection::KEY,
            seeds: crate::ws::upgrade_seeds()
                .into_iter()
                .chain(seeds(WebsocketConnection::KEY))
                .collect(),
            entries: Some(entries.clone()),
        },
        OpenerSeeds {
            opener,
            scope: WebsocketMessage::KEY,
            seeds: seeds(WebsocketMessage::KEY).collect(),
            entries: Some(entries),
        },
    ]
}

/// The entries of every described message handler of the controllers speaking the protocol
/// `registration` mounts.
fn served_entries(
    registration: &WsRegistration,
    controllers: &[WsControllerRegistration],
) -> Vec<EntryKey> {
    controllers
        .iter()
        .filter(|controller| (controller.protocol)() == registration.protocol)
        .flat_map(|controller| (controller.routes)())
        .filter_map(|route| route.message().map(|message| message.key))
        .collect()
}
