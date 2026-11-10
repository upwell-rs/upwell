//! Each WebSocket endpoint as an opener of the connection and message scopes.

use upwell_app::OpenerSeeds;
use upwell_core::{EntryKey, Scope, ScopeKey};

use super::WsRegistration;
use crate::scope::{WebsocketConnection, WebsocketMessage};
use crate::ws::WsControllerRegistration;

/// The endpoint `registration` as an opener of the connection and message scopes: the seeds
/// it fills in each, and the message handlers of `controllers` it serves, so a requirement
/// only another endpoint's handlers reach is not checked against it. An endpoint with an
/// undescribed handler serves everything.
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
            entries: entries.clone(),
            roots: Vec::new(),
        },
        OpenerSeeds {
            opener,
            scope: WebsocketMessage::KEY,
            seeds: seeds(WebsocketMessage::KEY).collect(),
            entries,
            roots: Vec::new(),
        },
    ]
}

/// The entries of the message handlers of the controllers speaking the protocol
/// `registration` mounts, or `None` when one of them is not described: a message to it opens
/// its scope without an entry, which builds every slot, so the endpoint may reach anything.
fn served_entries(
    registration: &WsRegistration,
    controllers: &[WsControllerRegistration],
) -> Option<Vec<EntryKey>> {
    let routes = controllers
        .iter()
        .filter(|controller| (controller.protocol)() == registration.protocol)
        .flat_map(|controller| (controller.routes)());

    served(routes.map(|route| route.message().map(|message| message.key)))
}

/// Every entry of `routes`, or `None` when a route has none.
fn served(routes: impl Iterator<Item = Option<EntryKey>>) -> Option<Vec<EntryKey>> {
    routes.collect()
}

#[cfg(test)]
mod tests;
