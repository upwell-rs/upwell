//! Projection of the prepared Axum plan into protocol-owned tooling resources: controllers and
//! their routes, the routes the protocol mounts itself (OpenAPI, WebSocket upgrades), and the
//! WebSocket endpoints with their message handlers.

use std::collections::BTreeMap;

use upwell_app::{
    ResourceDisplay, ToolingContributions, ToolingEndpoint, ToolingRelationshipKind,
    ValidationContext, tooling_schema::JsonValue,
};
use upwell_core::ItemSource;

use super::PreparedAxum;
use crate::controller::{ControllerDescriptor, HttpRouteDescriptor};

/// One route the protocol mounts itself rather than through a controller, reported beside the
/// controller routes so tooling sees every path the router serves.
#[derive(Clone, Debug)]
pub(crate) struct MountedRoute {
    /// The protocol feature that mounts the route.
    pub(crate) owner: MountedRouteOwner,
    /// Uppercase HTTP method.
    pub(crate) method: &'static str,
    /// Route path relative to the global base path.
    pub(crate) path: String,
    /// Stable handler name, unique per owner and path.
    pub(crate) handler: &'static str,
    /// One-line description of what the route serves.
    pub(crate) summary: String,
    /// The declaration that mounts the route.
    pub(crate) source: ItemSource,
}

/// The protocol feature that mounts a [`MountedRoute`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MountedRouteOwner {
    /// The OpenAPI document and UI.
    #[cfg_attr(not(feature = "openapi"), allow(dead_code))]
    OpenApi,
}

impl MountedRouteOwner {
    fn id(self) -> &'static str {
        match self {
            Self::OpenApi => "openapi",
        }
    }

    fn group(self) -> &'static str {
        match self {
            Self::OpenApi => "HTTP · OpenAPI",
        }
    }
}

/// The routes the protocol mounts itself under the validated configuration in `context`.
pub(super) fn mounted_routes(context: &ValidationContext<'_>) -> Vec<MountedRoute> {
    #[cfg(feature = "openapi")]
    {
        let config = context
            .config::<crate::OpenApiConfig>(crate::AXUM_OPENAPI_CONFIG_PATH)
            .expect("OpenApiConfig missing from config store; Axum should register it")
            .snapshot();

        crate::openapi::mounted_routes(&config)
    }

    #[cfg(not(feature = "openapi"))]
    {
        let _ = context;

        Vec::new()
    }
}

/// Contributes the prepared plan's resources, relationships, and presentation.
pub(super) fn project(prepared: &PreparedAxum, contributions: &mut ToolingContributions) {
    project_owner(prepared, contributions);

    for controller in &prepared.controllers {
        project_controller(&prepared.base_prefix, controller, contributions);
    }

    for route in &prepared.mounted_routes {
        project_mounted_route(&prepared.base_prefix, route, contributions);
    }

    #[cfg(feature = "ws")]
    for (ordinal, endpoint) in prepared.ws_registrations.iter().enumerate() {
        ws::project_endpoint(&prepared.base_prefix, ordinal, endpoint, contributions);
    }
}

fn project_owner(prepared: &PreparedAxum, contributions: &mut ToolingContributions) {
    let base_path = if prepared.base_prefix.is_empty() {
        String::from("/")
    } else {
        prepared.base_prefix.clone()
    };

    contributions.display(ResourceDisplay {
        label: Some(String::from("Axum HTTP")),
        group: Some(String::from("Protocols")),
        summary: Some(format!(
            "{} controllers, {} middleware layers",
            prepared.controllers.len(),
            prepared.middleware.len()
        )),
        details: BTreeMap::from([(String::from("base-path"), base_path)]),
    });
    contributions.facet(
        "summary",
        1,
        JsonValue::Object(
            [
                (
                    String::from("controller_count"),
                    prepared.controllers.len().into(),
                ),
                (
                    String::from("middleware_count"),
                    prepared.middleware.len().into(),
                ),
                (
                    String::from("mounted_route_count"),
                    prepared.mounted_routes.len().into(),
                ),
                #[cfg(feature = "ws")]
                (
                    String::from("websocket_endpoint_count"),
                    prepared.ws_registrations.len().into(),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    );
}

fn project_controller(
    base_prefix: &str,
    controller: &ControllerDescriptor,
    contributions: &mut ToolingContributions,
) {
    let id = format!("controller/{}", controller.id);
    let rust_type = (controller.ty.type_name)().to_string();

    contributions.resource_with_labels(
        &id,
        controller.name,
        BTreeMap::from([
            (String::from("base-path"), controller.base.to_string()),
            (String::from("kind"), String::from("http-controller")),
            (String::from("rust-type"), rust_type.clone()),
        ]),
    );

    if let Some(source) = controller.ty.source {
        contributions.resource_source(&id, source);
    }

    contributions.resource_display(
        &id,
        ResourceDisplay {
            label: Some(controller.name.to_string()),
            group: Some(String::from("HTTP controllers")),
            summary: Some(format!("mounted at {}", controller.base)),
            details: BTreeMap::from([
                (String::from("base-path"), controller.base.to_string()),
                (String::from("rust-type"), rust_type),
            ]),
        },
    );
    contributions.relationship(
        ToolingRelationshipKind::Contains,
        ToolingEndpoint::Owner,
        ToolingEndpoint::Resource(&id),
    );

    for route in (controller.routes)() {
        project_route(base_prefix, controller, &id, &route, contributions);
    }
}

fn project_route(
    base_prefix: &str,
    controller: &ControllerDescriptor,
    controller_id: &str,
    route: &HttpRouteDescriptor,
    contributions: &mut ToolingContributions,
) {
    let route_id = format!(
        "route/{}/{}/{}/{}",
        controller.id,
        route.method.to_ascii_lowercase(),
        route.path,
        route.handler
    );
    let path = join_http_paths(base_prefix, controller.base, route.path);

    contributions.resource_with_labels(
        &route_id,
        route.handler,
        BTreeMap::from([(String::from("kind"), String::from("http-route"))]),
    );

    if let Some(source) = route.source {
        contributions.resource_source(&route_id, source);
    }

    contributions.resource_display(
        &route_id,
        ResourceDisplay {
            label: Some(format!("{} {path}", route.method)),
            group: Some(format!("HTTP · {}", controller.name)),
            summary: Some(format!("handler {}", route.handler)),
            details: route_details(route, path),
        },
    );
    contributions.relationship(
        ToolingRelationshipKind::Contains,
        ToolingEndpoint::Resource(controller_id),
        ToolingEndpoint::Resource(&route_id),
    );
}

fn route_details(route: &HttpRouteDescriptor, path: String) -> BTreeMap<String, String> {
    let output = route
        .output
        .ty
        .map(|output| (output.type_name)().to_string())
        .unwrap_or_else(|| route.output.declared.to_string());
    let mut details = BTreeMap::from([
        (String::from("handler"), route.handler.to_string()),
        (String::from("method"), route.method.to_string()),
        (String::from("output"), output),
        (
            String::from("output-shape"),
            format!("{:?}", route.output.shape),
        ),
        (String::from("path"), path),
    ]);
    let optional = [
        ("inputs", route_inputs(route)),
        ("path-parameters", route_path_parameters(route)),
        ("responses", route_responses(route)),
    ];

    for (key, value) in optional {
        if !value.is_empty() {
            details.insert(String::from(key), value);
        }
    }

    details
}

fn route_inputs(route: &HttpRouteDescriptor) -> String {
    route
        .inputs
        .iter()
        .map(|input| {
            format!(
                "{} ({:?}): {}",
                input.name,
                input.source,
                (input.ty.type_name)()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn route_path_parameters(route: &HttpRouteDescriptor) -> String {
    route
        .path_parameters
        .iter()
        .map(|parameter| {
            if parameter.catch_all {
                format!("*{}", parameter.name)
            } else {
                parameter.name.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn route_responses(route: &HttpRouteDescriptor) -> String {
    route
        .output
        .responses
        .iter()
        .map(|response| {
            let mut value = response.status.to_string();

            if let Some(redirect) = response.redirect {
                value.push_str(" redirect ");
                value.push_str(redirect);
            }

            match response.body {
                crate::HttpResponseBodyDescriptor::Empty => value.push_str(" empty"),
                crate::HttpResponseBodyDescriptor::Typed(body) => {
                    value.push_str(" body ");
                    value.push_str((body.type_name)());
                }
                crate::HttpResponseBodyDescriptor::Opaque => value.push_str(" opaque"),
            }

            value
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn project_mounted_route(
    base_prefix: &str,
    route: &MountedRoute,
    contributions: &mut ToolingContributions,
) {
    let owner = route.owner.id();
    let route_id = format!(
        "route/{owner}/{}/{}/{}",
        route.method.to_ascii_lowercase(),
        route.path,
        route.handler
    );
    let path = join_http_paths(base_prefix, "", &route.path);

    contributions.resource_with_labels(
        &route_id,
        route.handler,
        BTreeMap::from([
            (String::from("kind"), String::from("http-route")),
            (String::from("mounted-by"), owner.to_string()),
        ]),
    );
    contributions.resource_source(&route_id, route.source);
    contributions.resource_display(
        &route_id,
        ResourceDisplay {
            label: Some(format!("{} {path}", route.method)),
            group: Some(route.owner.group().to_string()),
            summary: Some(route.summary.clone()),
            details: BTreeMap::from([
                (String::from("handler"), route.handler.to_string()),
                (String::from("method"), route.method.to_string()),
                (String::from("mounted-by"), owner.to_string()),
                (String::from("path"), path),
            ]),
        },
    );
    contributions.relationship(
        ToolingRelationshipKind::Contains,
        ToolingEndpoint::Owner,
        ToolingEndpoint::Resource(&route_id),
    );
}

fn join_http_paths(global: &str, controller: &str, route: &str) -> String {
    let segments = [global, controller, route]
        .into_iter()
        .flat_map(|path| path.split('/'))
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        String::from("/")
    } else {
        format!("/{}", segments.join("/"))
    }
}

#[cfg(feature = "ws")]
mod ws;
