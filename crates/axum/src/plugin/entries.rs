//! The entries Axum contributes: one per controller route and, for mounted WebSocket
//! protocols, one per described message handler.

use axum::http::Method;
use upwell_app::{EntryDescriptor, EntryOwner, ScopeOpener};
use upwell_core::Scope;

use crate::controller::ControllerDescriptor;
use crate::request_scope::RouteEntries;
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

/// The entry of every controller route, by method and the path the router matches it under
/// below `base_prefix`.
pub(super) fn route_entries_by_path(
    controllers: &[ControllerDescriptor],
    base_prefix: &str,
    opener: &ScopeOpener<HttpRequest>,
) -> crate::Result<RouteEntries> {
    let mut entries = RouteEntries::default();

    for controller in controllers {
        for route in (controller.routes)() {
            let method = Method::from_bytes(route.method.as_bytes()).map_err(|_| {
                crate::Error::Config(format!(
                    "route `{}` declares the invalid HTTP method `{}`",
                    route.key, route.method
                ))
            })?;
            let path = join_http_paths(base_prefix, controller.base, route.path);

            entries.insert(method, path, opener.entry(route.key)?);
        }
    }

    Ok(entries)
}

/// Joins a global prefix, a controller base and a route path into one normalized path. A
/// route path ending in `/` keeps it, as the router matches it.
pub(super) fn join_http_paths(global: &str, controller: &str, route: &str) -> String {
    let segments = [global, controller, route]
        .into_iter()
        .flat_map(|path| path.split('/'))
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        return String::from("/");
    }

    let trailing = if route.len() > 1 && route.ends_with('/') {
        "/"
    } else {
        ""
    };

    format!("/{}{trailing}", segments.join("/"))
}
