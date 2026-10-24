#[cfg(feature = "tooling")]
#[test]
fn prepared_axum_projects_only_retained_controller_and_middleware_facts() {
    let document = upwell_app::App::<super::Axum>::builder("axum-tooling")
        .config_source(upwell_config::ConfigManager::<upwell_config::Dynamic>::empty())
        .prepare()
        .expect("Axum prepares")
        .tooling_document()
        .expect("Axum tooling projects");
    let protocol = document
        .resources
        .iter()
        .find(|resource| resource.id == "protocol:upwell/axum")
        .expect("Axum protocol resource exists");
    let summary = &protocol.facets["protocol:upwell/axum/tooling/summary"].value;

    assert_eq!(summary["controller_count"], 0);
    assert_eq!(summary["middleware_count"], 0);
    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("Axum HTTP")
    );
    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.summary.as_deref()),
        Some("0 controllers, 0 middleware layers")
    );
    assert!(
        document
            .relationships
            .iter()
            .filter(|relationship| {
                relationship.from == "protocol:upwell/axum"
                    && relationship.to.starts_with("protocol:upwell/axum/tooling/")
            })
            .all(|relationship| {
                relationship.kind == upwell_app::tooling_schema::RelationshipKind::Contains
            })
    );
}

#[cfg(feature = "tooling")]
#[test]
fn prepared_axum_projects_static_http_routes_without_building_runtime() {
    struct ToolingController;

    fn routes() -> Vec<crate::HttpRouteDescriptor> {
        vec![
            crate::HttpRouteDescriptor {
                handler: "health",
                method: "GET",
                path: "/health",
                path_parameters: &[],
                inputs: &[],
                output: crate::HttpOutputDescriptor {
                    ty: Some(upwell_core::TypeDescriptor::of::<()>("()")),
                    declared: "()",
                    shape: crate::HttpOutputShape::Unary,
                    responses: &[],
                },
                source: Some(upwell_core::item_source!("ToolingController::health")),
            },
            crate::HttpRouteDescriptor {
                handler: "health",
                method: "GET",
                path: "/ready",
                path_parameters: &[],
                inputs: &[],
                output: crate::HttpOutputDescriptor {
                    ty: Some(upwell_core::TypeDescriptor::of::<()>("()")),
                    declared: "()",
                    shape: crate::HttpOutputShape::Unary,
                    responses: &[],
                },
                source: None,
            },
        ]
    }

    let controller = crate::ControllerDescriptor {
        id: "tooling-controller",
        name: "ToolingController",
        ty: upwell_core::TypeDescriptor::of::<ToolingController>("ToolingController"),
        base: "/api",
        router: |_| panic!("tooling must not build controller router"),
        routes,
    };
    use super::AxumAppBuilder as _;

    let controller = Box::leak(Box::new(controller));

    let document = upwell_app::App::<super::Axum>::builder("axum-route-tooling")
        .config_source(upwell_config::ConfigManager::<upwell_config::Dynamic>::empty())
        .controller_descriptor(controller)
        .prepare()
        .expect("Axum prepares")
        .tooling_document()
        .expect("Axum tooling projects");
    let routes = document
        .resources
        .iter()
        .filter(|resource| resource.labels.get("kind").map(String::as_str) == Some("http-route"))
        .collect::<Vec<_>>();
    let route = routes
        .iter()
        .find(|resource| {
            resource
                .display
                .as_ref()
                .and_then(|display| display.label.as_deref())
                == Some("GET /api/health")
        })
        .expect("health route resource exists");

    assert_eq!(routes.len(), 2);
    assert_ne!(routes[0].id, routes[1].id);
    assert_eq!(
        route
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("GET /api/health")
    );
    assert_eq!(route.display.as_ref().unwrap().details["handler"], "health");

    let source = route
        .provenance
        .as_ref()
        .and_then(|provenance| provenance.source.as_ref())
        .expect("the macro-recorded route source is projected");

    assert!(
        std::path::Path::new(&source.file).ends_with("plugin/tests.rs"),
        "{}",
        source.file
    );
    assert_eq!(
        source.rust_path.as_deref(),
        Some("upwell_axum::plugin::tests::ToolingController::health")
    );
}

#[cfg(all(feature = "tooling", feature = "openapi"))]
#[test]
fn prepared_axum_projects_enabled_openapi_routes_beside_controller_routes() {
    let config = upwell_config::ConfigManager::<upwell_config::Toml>::from_str(
        "[axum]\nbase_path = \"/api\"\n\n[axum.openapi]\nenabled = true\njson_path = \"/spec.json\"\n",
    )
    .expect("test config parses");
    let document = upwell_app::App::<super::Axum>::builder("axum-openapi-tooling")
        .config_source(config)
        .prepare()
        .expect("Axum prepares")
        .tooling_document()
        .expect("Axum tooling projects");
    let route = document
        .resources
        .iter()
        .find(|resource| {
            resource.labels.get("kind").map(String::as_str) == Some("http-route")
                && resource.labels.get("mounted-by").map(String::as_str) == Some("openapi")
        })
        .expect("the OpenAPI document route is projected as an HTTP route");
    let display = route.display.as_ref().expect("route display");
    let source = route
        .provenance
        .as_ref()
        .and_then(|provenance| provenance.source.as_ref())
        .expect("the mounting declaration is projected");

    assert_eq!(display.label.as_deref(), Some("GET /api/spec.json"));
    assert_eq!(display.details["handler"], "openapi_json");
    assert_eq!(
        source.rust_path.as_deref(),
        Some("upwell_axum::openapi::serve_json")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.from == "protocol:upwell/axum" && relationship.to == route.id
    }));
}

#[cfg(all(feature = "tooling", feature = "ws"))]
#[test]
fn prepared_axum_projects_websocket_path_and_protocol_identity() {
    use std::sync::Arc;

    use axum::extract::ws::WebSocket;
    use upwell_app::AppRuntime;
    use upwell_di::ScopeContainer;

    /// WebSocket protocol used only to retain endpoint metadata.
    struct ToolingWsProtocol;

    impl crate::WebsocketProtocol for ToolingWsProtocol {
        type Payload = ();
        type Outcome = ();
        type Options = ();
        type BuildError = std::convert::Infallible;

        fn build(
            _controllers: &[crate::WsControllerDescriptor],
            _runtime: &AppRuntime,
            _options: Self::Options,
        ) -> Result<Self, Self::BuildError> {
            Ok(Self)
        }

        async fn serve(
            self: Arc<Self>,
            _socket: WebSocket,
            _connection: Arc<ScopeContainer>,
            _shutdown: crate::WsShutdown,
        ) {
        }
    }

    use super::AxumAppBuilder as _;

    let registered_at = line!() + 3;
    let document = upwell_app::App::<super::Axum>::builder("axum-websocket-tooling")
        .config_source(upwell_config::ConfigManager::<upwell_config::Dynamic>::empty())
        .register_ws::<ToolingWsProtocol>("/events")
        .prepare()
        .expect("Axum websocket prepares")
        .tooling_document()
        .expect("Axum websocket tooling projects");
    let endpoint = document
        .resources
        .iter()
        .find(|resource| resource.labels.get("kind") == Some(&String::from("websocket-endpoint")))
        .expect("websocket endpoint resource exists");

    let source = endpoint
        .provenance
        .as_ref()
        .and_then(|provenance| provenance.source.as_ref())
        .expect("the register_ws call site is projected");

    assert_eq!(endpoint.labels["path"], "/events");
    assert!(
        std::path::Path::new(&source.file).ends_with("plugin/tests.rs"),
        "{}",
        source.file
    );
    assert_eq!(source.line, Some(registered_at));
    assert!(
        endpoint
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref())
            .is_some_and(|label| label.ends_with("ToolingWsProtocol"))
    );
    assert!(
        endpoint.labels["protocol-type"].ends_with("ToolingWsProtocol"),
        "prepared endpoint retains stable protocol type identity"
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == upwell_app::tooling_schema::RelationshipKind::Contains
            && relationship.from == "protocol:upwell/axum"
            && relationship.to == endpoint.id
    }));
}
