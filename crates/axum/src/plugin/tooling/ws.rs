//! WebSocket endpoint, controller, and message-handler tooling resources.

use std::collections::BTreeMap;

use upwell_app::{ResourceDisplay, ToolingContributions, ToolingEndpoint, ToolingRelationshipKind};
use upwell_core::ItemSource;

use super::super::entries::join_http_paths;
use crate::plugin::PreparedWsRegistration;
use crate::ws::{WsControllerDescriptor, WsMessageDescriptor, WsRouteDescriptor};

/// The in-module path the endpoint source names: the builder call that mounted it.
const REGISTRATION_ITEM: &str = "AxumAppBuilder::register_ws";

pub(super) fn project_endpoint(
    base_prefix: &str,
    ordinal: usize,
    endpoint: &PreparedWsRegistration,
    contributions: &mut ToolingContributions,
) {
    let id = format!("websocket/{ordinal}");
    let controllers = endpoint.controllers.len().to_string();

    contributions.resource_with_labels(
        &id,
        endpoint.protocol_name,
        BTreeMap::from([
            (String::from("kind"), String::from("websocket-endpoint")),
            (String::from("path"), endpoint.path.clone()),
            (
                String::from("protocol-name"),
                endpoint.protocol_name.to_string(),
            ),
            (String::from("controller-count"), controllers.clone()),
        ]),
    );
    contributions.resource_source(
        &id,
        ItemSource::new(endpoint.source, "upwell_axum", REGISTRATION_ITEM),
    );
    contributions.resource_display(
        &id,
        ResourceDisplay {
            label: Some(format!("WebSocket {}", endpoint.protocol_name)),
            group: Some(String::from("WebSocket endpoints")),
            summary: Some(format!("mounted at {}", endpoint.path)),
            details: BTreeMap::from([
                (String::from("path"), endpoint.path.clone()),
                (String::from("controllers"), controllers),
                (String::from("method"), String::from("GET")),
                (
                    String::from("upgrade-path"),
                    join_http_paths(base_prefix, "", &endpoint.path),
                ),
            ]),
        },
    );
    contributions.relationship(
        ToolingRelationshipKind::Contains,
        ToolingEndpoint::Owner,
        ToolingEndpoint::Resource(&id),
    );

    for controller in &endpoint.controllers {
        project_controller(&id, endpoint.protocol_name, controller, contributions);
    }
}

fn project_controller(
    endpoint_id: &str,
    protocol_name: &str,
    controller: &WsControllerDescriptor,
    contributions: &mut ToolingContributions,
) {
    let controller_id = format!("{endpoint_id}/controller/{}", controller.id);

    contributions.resource_with_labels(
        &controller_id,
        controller.name,
        BTreeMap::from([(String::from("kind"), String::from("websocket-controller"))]),
    );

    if let Some(source) = controller.ty.source {
        contributions.resource_source(&controller_id, source);
    }

    contributions.resource_display(
        &controller_id,
        ResourceDisplay {
            label: Some(controller.name.to_string()),
            group: Some(format!("WebSocket · {protocol_name}")),
            summary: Some(format!("{} message handlers", controller.routes().len())),
            details: BTreeMap::from([(
                String::from("rust-type"),
                (controller.ty.type_name)().to_string(),
            )]),
        },
    );
    contributions.relationship(
        ToolingRelationshipKind::Contains,
        ToolingEndpoint::Resource(endpoint_id),
        ToolingEndpoint::Resource(&controller_id),
    );

    for route in controller.routes() {
        project_message(&controller_id, controller, route, contributions);
    }
}

fn project_message(
    controller_id: &str,
    controller: &WsControllerDescriptor,
    route: &WsRouteDescriptor,
    contributions: &mut ToolingContributions,
) {
    let message_id = format!("{controller_id}/message/{}", route.destination());
    let message = route.message();

    contributions.resource_with_labels(
        &message_id,
        route.destination(),
        BTreeMap::from([(String::from("kind"), String::from("websocket-message"))]),
    );

    if let Some(source) = message.and_then(|message| message.source) {
        contributions.resource_source(&message_id, source);
    }

    contributions.resource_display(
        &message_id,
        ResourceDisplay {
            label: Some(format!("message {}", route.destination())),
            group: Some(format!("WebSocket · {}", controller.name)),
            summary: Some(
                message
                    .map(|message| {
                        format!("{:?} via {}", message.mode, (message.codec.type_name)())
                    })
                    .unwrap_or_else(|| String::from("custom message handler")),
            ),
            details: message.map(message_details).unwrap_or_default(),
        },
    );
    contributions.relationship(
        ToolingRelationshipKind::Contains,
        ToolingEndpoint::Resource(controller_id),
        ToolingEndpoint::Resource(&message_id),
    );
}

fn message_details(message: &WsMessageDescriptor) -> BTreeMap<String, String> {
    let type_or_none = |ty: Option<upwell_core::TypeDescriptor>| {
        ty.map(|ty| (ty.type_name)().to_string())
            .unwrap_or_else(|| String::from("none"))
    };

    BTreeMap::from([
        (String::from("handler"), message.handler.to_string()),
        (String::from("mode"), format!("{:?}", message.mode)),
        (String::from("payload"), type_or_none(message.payload)),
        (String::from("reply"), type_or_none(message.reply)),
        (
            String::from("codec"),
            (message.codec.type_name)().to_string(),
        ),
    ])
}
