use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "tooling")]
use upwell_app::tooling_schema::RelationshipKind;
use upwell_app::{App, ProtocolDefinition, ScopeParent};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::{Scope, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole, Injectable, Singleton,
};

use super::{Rpc, RpcAppBuilder};
use crate::scope::{Connection as ConnectionScope, Request as RequestScope, SCOPE_TOPOLOGY};
use crate::{Error, ServiceDescriptor};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Component proving protocol validation runs before singleton construction.
struct SentinelComponent;

impl Component for SentinelComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "rpc_validation_sentinel";
    const NAME: &'static str = "RpcValidationSentinel";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

fn construct_sentinel(
    _context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<SentinelComponent>(SentinelComponent::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(SentinelComponent))),
        })
    })
}

fn no_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    Vec::new()
}

static SENTINEL_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: construct_sentinel,
    construct_ready: None,
    dependencies: no_dependencies,
    role: ComponentFactoryRole::Default,
}];

fn sentinel_factories() -> &'static [ComponentFactoryDescriptor] {
    &SENTINEL_FACTORIES
}

static SENTINEL_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: SentinelComponent::ID,
    name: SentinelComponent::NAME,
    ty: TypeDescriptor::of::<SentinelComponent>(SentinelComponent::NAME),
    scope: Singleton::KEY,
    condition: None,
    factories: sentinel_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

struct EmptyService;

fn no_rpc_groups() -> &'static [crate::RpcGroup] {
    &[]
}

static EMPTY_SERVICE: ServiceDescriptor = ServiceDescriptor {
    id: "empty",
    name: "EmptyService",
    ty: TypeDescriptor::of::<EmptyService>("EmptyService"),
    version: None,
    rpcs: no_rpc_groups,
};

#[test]
fn empty_service_fails_during_prepare_before_component_construction() {
    FACTORY_CALLS.store(0, Ordering::SeqCst);

    let result = App::<Rpc>::builder("invalid-rpc-test")
        .config_source(ConfigManager::<Dynamic>::empty())
        .component_descriptor(&SENTINEL_COMPONENT)
        .service_descriptor(&EMPTY_SERVICE)
        .prepare();

    let error = match result {
        Ok(_) => panic!("empty service was not rejected during preparation"),
        Err(error) => error,
    };

    assert!(matches!(error, Error::EmptyService(service) if service == "EmptyService"));
    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn rpc_scope_topology_declares_connection_and_request_path() {
    let topology = SCOPE_TOPOLOGY.prepare().expect("RPC topology is valid");
    let connection = topology
        .boundary(&<ConnectionScope as Scope>::ID)
        .expect("connection boundary is declared");
    let request = topology
        .boundary(&<RequestScope as Scope>::ID)
        .expect("request boundary is declared");

    assert_eq!(Rpc::SCOPE_TOPOLOGY.boundaries().len(), 2);
    assert_eq!(connection.parents(), [ScopeParent::Root]);
    assert_eq!(request.parents(), [ScopeParent::of::<ConnectionScope>()]);
}

#[cfg(feature = "tooling")]
#[test]
fn prepared_rpc_projects_only_retained_service_and_route_facts() {
    let document = App::<Rpc>::builder("rpc-tooling")
        .config_source(ConfigManager::<Dynamic>::empty())
        .prepare()
        .expect("RPC prepares")
        .tooling_document()
        .expect("RPC tooling projects");
    let protocol = document
        .resources
        .iter()
        .find(|resource| resource.id == "protocol:upwell/rpc")
        .expect("RPC protocol resource exists");
    let summary = &protocol.facets["protocol:upwell/rpc/tooling/summary"].value;
    let peer = document
        .resources
        .iter()
        .find(|resource| resource.id == "component:__upwell_peer_info")
        .expect("peer seed projects");

    assert_eq!(summary["service_count"], 0);
    assert_eq!(summary["route_count"], 0);
    assert_eq!(summary["middleware_count"], 0);
    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("RPC")
    );
    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.summary.as_deref()),
        Some("0 services, 0 operations")
    );
    assert_eq!(peer.labels["construction"], "scope-seed");
    assert!(!peer.labels.contains_key("plan-ordinal"));
    assert!(
        document
            .relationships
            .iter()
            .filter(|relationship| {
                relationship.from == "protocol:upwell/rpc"
                    && relationship.to.starts_with("protocol:upwell/rpc/tooling/")
            })
            .all(|relationship| relationship.kind == RelationshipKind::Contains)
    );
}

#[tokio::test]
async fn peer_info_seeds_the_connection_scope_a_request_opens_under() {
    let app = App::<Rpc>::builder("rpc-scope-seed-test")
        .config_source(ConfigManager::<Dynamic>::empty())
        .build()
        .await
        .expect("RPC app builds");
    let runtime = app.runtime();
    let peer = upwell_transport::PeerInfo {
        addr: Some("127.0.0.1:1234".parse().expect("valid test address")),
    };
    let connection = runtime
        .opener::<ConnectionScope>()
        .expect("the connection scope is declared")
        .builder()
        .seed(peer.clone())
        .open_default()
        .await
        .expect("registered peer seed opens the connection scope");
    let resolved = connection
        .container()
        .resolve::<upwell_transport::PeerInfo>()
        .await
        .expect("peer resolution succeeds")
        .expect("peer is seeded");

    let request = runtime
        .opener::<RequestScope>()
        .expect("the request scope is declared")
        .builder()
        .under(&connection)
        .open_default()
        .await
        .expect("request opens under its declared connection parent");

    assert_eq!(resolved.addr, peer.addr);
    assert_eq!(
        request.container().scope().id(),
        <RequestScope as Scope>::ID
    );
}
