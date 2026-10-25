//! The entries Axum contributes: one per controller route and, for mounted WebSocket
//! protocols, one per described message handler.

use upwell_app::{EntryDescriptor, EntryOwner};
use upwell_core::Scope;

use crate::controller::ControllerDescriptor;
use crate::scope::HttpRequest;

/// One entry per route of every controller, running in the HTTP request scope and owned by
/// the controller and its handler.
pub(super) fn route_entries(
    controllers: &[ControllerDescriptor],
) -> impl Iterator<Item = EntryDescriptor> + '_ {
    controllers.iter().flat_map(|controller| {
        (controller.routes)()
            .into_iter()
            .map(move |route| EntryDescriptor {
                key: route.key,
                scope: HttpRequest::KEY,
                targets: route.targets,
                owner: Some(EntryOwner {
                    ty: controller.ty,
                    operation: route.handler,
                }),
                source: route.source,
            })
    })
}

/// One entry per described message handler of every WebSocket controller whose protocol is
/// mounted, running in the message scope and owned by the controller and its handler.
#[cfg(feature = "ws")]
pub(super) fn message_entries<'a>(
    controllers: &'a [crate::ws::WsControllerRegistration],
    mounted: &'a std::collections::HashSet<std::any::TypeId>,
) -> impl Iterator<Item = EntryDescriptor> + 'a {
    controllers
        .iter()
        .filter(|controller| mounted.contains(&(controller.protocol)()))
        .flat_map(|controller| {
            (controller.routes)()
                .into_iter()
                .filter_map(|route| route.message().copied())
                .map(move |message| EntryDescriptor {
                    key: message.key,
                    scope: crate::scope::WebsocketMessage::KEY,
                    targets: message.targets,
                    owner: Some(EntryOwner {
                        ty: controller.ty,
                        operation: message.handler,
                    }),
                    source: message.source,
                })
        })
}
