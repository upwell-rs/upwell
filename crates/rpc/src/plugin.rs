//! The native RPC protocol definition and its builder extension.

use std::sync::Arc;

use tower::{Layer, Service};
use upwell_app::{
    AppBuilder, AppRegistry, AppRuntime, PreparedProtocol, ProtocolDefinition, ValidationContext,
};
use upwell_core::{Descriptor, TypeDescriptor};
use upwell_di::{ComponentDescriptor, ServiceComponent};
use upwell_transport::PeerInfo;

use crate::descriptors::{RpcOutcome, SERVICES, ServiceDescriptor};
use crate::extract::ErrorResponse;
use crate::middleware::{ErrorHandler, Guard, GuardLayer, RouterService, RpcRequest, RpcService};
use crate::protocol::{RpcLimits, RpcRuntime};
use crate::router::RpcRouter;
use crate::scope::{Connection as ConnectionScope, SCOPE_TOPOLOGY};

/// A registered middleware step: wraps the current dispatch service in one more layer.
/// Collected in registration order and applied outermost-first when the app is built.
type LayerApplier = Box<dyn FnOnce(RpcService) -> RpcService + Send>;

/// The framework-provided connection-scoped injectable for the remote peer.
///
/// Seeded into every connection scope with the actual `PeerInfo`, so a connection-scoped
/// component can depend on `PeerInfo` (e.g. to authenticate in its constructor). This
/// descriptor intentionally has no factory: it declares the connection scope as the only
/// valid runtime seed destination.
static PEER_INFO_DESCRIPTOR: ComponentDescriptor = ComponentDescriptor::manual(
    "__upwell_peer_info",
    "PeerInfo",
    TypeDescriptor::of::<PeerInfo>("PeerInfo"),
    &ConnectionScope,
);

/// The native RPC protocol definition.
///
/// Accumulates the RPC-specific builder state — the discovered/registered services, the
/// middleware layers, and the global error handler — seeds the connection-scoped
/// `PeerInfo`, and prepares the validated service plan consumed by [`RpcRuntime`].
#[derive(Default)]
pub struct Rpc {
    services: Vec<ServiceDescriptor>,
    layers: Vec<LayerApplier>,
    error_handler: Option<Arc<dyn ErrorHandler>>,
    limits: RpcLimits,
}

/// The validated RPC service and middleware plan awaiting runtime construction.
pub struct PreparedRpc {
    resolved_services: Vec<crate::routes::ResolvedService>,
    layers: Vec<LayerApplier>,
    error_handler: Option<Arc<dyn ErrorHandler>>,
    needs_peer: bool,
    limits: RpcLimits,
}

impl ProtocolDefinition for Rpc {
    type Prepared = PreparedRpc;
    type Error = crate::Error;

    const ID: upwell_app::ProtocolId =
        upwell_core::namespaced_id!(upwell_app::ProtocolId, "upwell/rpc");
    const SCOPE_TOPOLOGY: upwell_app::ScopeTopology = SCOPE_TOPOLOGY;

    fn register(&self, registry: &mut AppRegistry) {
        registry.components.push(PEER_INFO_DESCRIPTOR);
    }

    fn prepare(self, context: &ValidationContext<'_>) -> crate::Result<Self::Prepared> {
        let resolved = crate::routes::resolved_services(&self.services);

        crate::routes::validate_services(&resolved)?;

        let peer_id = PEER_INFO_DESCRIPTOR.ty.type_id;
        let needs_peer = context.resolved_components().iter().any(|component| {
            component.ty.type_id != peer_id
                && component
                    .dependencies()
                    .iter()
                    .any(|dependency| dependency.ty.type_id == peer_id)
        });

        Ok(PreparedRpc {
            resolved_services: resolved,
            layers: self.layers,
            error_handler: self.error_handler,
            needs_peer,
            limits: self.limits,
        })
    }

    fn auto_discover(&mut self) {
        self.services.extend(SERVICES.iter().copied());
    }
}

impl PreparedProtocol for PreparedRpc {
    type Runtime = RpcRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &AppRuntime) -> crate::Result<Self::Runtime> {
        let router = Arc::new(RpcRouter::from_services(&self.resolved_services));

        // Fold the registered layers onto the terminal router service. Appliers are
        // pushed in registration order, so applying them in reverse makes the
        // first-registered layer the outermost wrapper.
        let mut service: RpcService = RpcService::new(RouterService::new(Arc::clone(&router)));

        for applier in self.layers.into_iter().rev() {
            service = applier(service);
        }

        Ok(RpcRuntime::new(
            router,
            service,
            self.error_handler,
            self.needs_peer,
            self.limits,
        ))
    }

    #[cfg(feature = "tooling")]
    fn tooling(&self, contributions: &mut upwell_app::ToolingContributions) {
        use std::collections::BTreeMap;

        use upwell_app::{ResourceDisplay, ToolingEndpoint, ToolingRelationshipKind};

        let route_count = self
            .resolved_services
            .iter()
            .map(|service| service.rpcs.len())
            .sum::<usize>();

        contributions.display(ResourceDisplay {
            label: Some(String::from("RPC")),
            group: Some(String::from("Protocols")),
            summary: Some(format!(
                "{} services, {route_count} operations",
                self.resolved_services.len()
            )),
            details: BTreeMap::from([(String::from("middleware"), self.layers.len().to_string())]),
        });

        contributions.facet(
            "summary",
            1,
            upwell_app::tooling_schema::JsonValue::Object(
                [
                    (
                        String::from("service_count"),
                        self.resolved_services.len().into(),
                    ),
                    (
                        String::from("route_count"),
                        self.resolved_services
                            .iter()
                            .map(|service| service.rpcs.len())
                            .sum::<usize>()
                            .into(),
                    ),
                    (String::from("middleware_count"), self.layers.len().into()),
                ]
                .into_iter()
                .collect(),
            ),
        );

        for service in &self.resolved_services {
            let service_id = format!("service/{}", service.descriptor.id);

            contributions.resource_with_labels(
                &service_id,
                service.descriptor.name,
                BTreeMap::from([
                    (String::from("kind"), String::from("rpc-service")),
                    (String::from("route-count"), service.rpcs.len().to_string()),
                ]),
            );

            if let Some(source) = service.descriptor.ty.source {
                contributions.resource_source(&service_id, source);
            }
            contributions.resource_display(
                &service_id,
                ResourceDisplay {
                    label: Some(service.descriptor.name.to_string()),
                    group: Some(String::from("RPC services")),
                    summary: Some(format!("{} operations", service.rpcs.len())),
                    details: BTreeMap::from([
                        (
                            String::from("rust-type"),
                            (service.descriptor.ty.type_name)().to_string(),
                        ),
                        (
                            String::from("version"),
                            service
                                .descriptor
                                .version
                                .unwrap_or("unversioned")
                                .to_string(),
                        ),
                    ]),
                },
            );
            contributions.relationship(
                ToolingRelationshipKind::Contains,
                ToolingEndpoint::Owner,
                ToolingEndpoint::Resource(&service_id),
            );

            for rpc in &service.rpcs {
                let rpc_id = format!("route/{}/{}", service.descriptor.id, rpc.name);

                contributions.resource_with_labels(
                    &rpc_id,
                    rpc.name,
                    BTreeMap::from([(String::from("kind"), String::from("rpc-route"))]),
                );

                if let Some(source) = rpc.source {
                    contributions.resource_source(&rpc_id, source);
                }
                let operation = operation_kind(rpc.operation);
                let parameters = rpc
                    .parameters
                    .iter()
                    .map(|parameter| format!("{}: {}", parameter.name, (parameter.ty.type_name)()))
                    .collect::<Vec<_>>()
                    .join(", ");

                let parameter_summary = if parameters.is_empty() {
                    String::from("none")
                } else {
                    parameters.clone()
                };

                contributions.resource_display(
                    &rpc_id,
                    ResourceDisplay {
                        label: Some(format!(
                            "{operation} {}.{}",
                            service.descriptor.name, rpc.name
                        )),
                        group: Some(format!("RPC · {}", service.descriptor.name)),
                        summary: Some(format!(
                            "{} → {}",
                            parameter_summary,
                            (rpc.output.type_name)()
                        )),
                        details: BTreeMap::from([
                            (String::from("operation"), operation.to_string()),
                            (
                                String::from("parameters"),
                                if parameters.is_empty() {
                                    String::from("none")
                                } else {
                                    parameters
                                },
                            ),
                            (String::from("output"), (rpc.output.type_name)().to_string()),
                        ]),
                    },
                );
                contributions.relationship(
                    ToolingRelationshipKind::Contains,
                    ToolingEndpoint::Resource(&service_id),
                    ToolingEndpoint::Resource(&rpc_id),
                );
            }
        }
    }
}

#[cfg(feature = "tooling")]
fn operation_kind(kind: crate::OperationKind) -> &'static str {
    match kind {
        crate::OperationKind::Unary => "unary",
        crate::OperationKind::ServerStream => "server-stream",
        crate::OperationKind::ClientStream => "client-stream",
        crate::OperationKind::BidiStream => "bidi-stream",
    }
}

/// RPC-specific builder methods, contributed to [`AppBuilder<Rpc>`] as an extension
/// trait (a foreign crate cannot add inherent methods to a generic type). Bring it into
/// scope to register services, middleware, guards, and the error handler; it is in the
/// prelude.
pub trait RpcAppBuilder {
    /// Registers service type `T` by type: its identity header (carrying its RPC surface)
    /// and its construction factory.
    fn service<T>(self) -> Self
    where
        T: Descriptor<ServiceDescriptor> + Descriptor<ComponentDescriptor>;

    /// Registers a pre-built service singleton: its identity header and the instance.
    fn with_service<T>(self, value: T) -> Self
    where
        T: ServiceComponent + Descriptor<ServiceDescriptor>;

    /// Manually registers a raw service header (prefer [`service`](Self::service) by type).
    fn service_descriptor(self, descriptor: &'static ServiceDescriptor) -> Self;

    /// Wraps the dispatch path in a [`tower::Layer`], running on every call. The first
    /// layer registered is the outermost.
    fn middleware<L>(self, layer: L) -> Self
    where
        L: Layer<RpcService> + Send + 'static,
        L::Service: Service<RpcRequest, Response = RpcOutcome, Error = ErrorResponse>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<RpcRequest>>::Future: Send + 'static;

    /// Registers a [`Guard`] as a pre-handler admit/reject check.
    fn guard<G: Guard>(self, guard: G) -> Self;

    /// Sets the single global [`ErrorHandler`] applied to every error response.
    fn error_handler<H: ErrorHandler>(self, handler: H) -> Self;

    /// Sets connection and per-connection call admission limits.
    fn rpc_limits(self, limits: RpcLimits) -> Self;
}

impl RpcAppBuilder for AppBuilder<Rpc> {
    fn service<T>(mut self) -> Self
    where
        T: Descriptor<ServiceDescriptor> + Descriptor<ComponentDescriptor>,
    {
        self.protocol_mut()
            .services
            .push(<T as Descriptor<ServiceDescriptor>>::DESCRIPTOR);

        self.component::<T>()
    }

    fn with_service<T>(mut self, value: T) -> Self
    where
        T: ServiceComponent + Descriptor<ServiceDescriptor>,
    {
        self.protocol_mut()
            .services
            .push(<T as Descriptor<ServiceDescriptor>>::DESCRIPTOR);

        self.with_component(value)
    }

    fn service_descriptor(mut self, descriptor: &'static ServiceDescriptor) -> Self {
        self.protocol_mut().services.push(*descriptor);

        self
    }

    fn middleware<L>(mut self, layer: L) -> Self
    where
        L: Layer<RpcService> + Send + 'static,
        L::Service: Service<RpcRequest, Response = RpcOutcome, Error = ErrorResponse>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<RpcRequest>>::Future: Send + 'static,
    {
        self.protocol_mut()
            .layers
            .push(Box::new(move |inner| RpcService::new(layer.layer(inner))));

        self
    }

    fn guard<G: Guard>(self, guard: G) -> Self {
        self.middleware(GuardLayer::new(Arc::new(guard)))
    }

    fn error_handler<H: ErrorHandler>(mut self, handler: H) -> Self {
        self.protocol_mut().error_handler = Some(Arc::new(handler));

        self
    }

    fn rpc_limits(mut self, limits: RpcLimits) -> Self {
        self.protocol_mut().limits = limits;

        self
    }
}

#[cfg(test)]
mod tests;
