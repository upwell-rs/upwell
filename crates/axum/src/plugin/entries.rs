//! The entries Axum contributes: one per controller route.

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
