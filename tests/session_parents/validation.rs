//! Which edges from a request into a longer-lived application scope validate: an optional edge
//! only into a scope that encloses the request on some chain, and a required edge only into
//! one on every chain.
#![allow(dead_code)]

use super::*;

/// A tenant scope under the root that requests never open under: it names no children.
#[scope(parents = [Root])]
pub struct Tenant;

/// A component of [`Tenant`].
#[component(scope = Tenant)]
struct TenantClient {}

/// A request component taking the tenant's client optionally, which can never be present.
#[component(scope = HttpRequest)]
struct TenantView {
    client: Option<Arc<TenantClient>>,
}

/// A request component requiring the session's client, which a request under the root lacks.
#[component(scope = HttpRequest)]
struct RequiredSession {
    client: Arc<SessionClient>,
}

/// A route injecting the tenant's client optionally.
#[controller(path = "/tenant")]
struct TenantController;

#[handlers]
impl TenantController {
    #[get("/client")]
    async fn client(&self, Inject(client): Inject<Option<Arc<TenantClient>>>) -> String {
        client.map_or_else(|| String::from("none"), |_| String::from("tenant"))
    }
}

/// The error building `builder` fails with.
async fn build_error(builder: AppBuilder<Axum>) -> String {
    match builder.build().await {
        Ok(_) => panic!("the app is rejected"),
        Err(error) => format!("{error:?}"),
    }
}

#[tokio::test]
async fn an_optional_edge_into_a_scope_that_never_encloses_the_request_is_rejected() {
    let dir = config_dir(1);
    let error = build_error(
        with_sessions(dir.path())
            .scope::<Tenant>()
            .component::<TenantClient>()
            .component::<TenantView>(),
    )
    .await;

    assert!(error.contains("ScopeViolation"), "{error}");
    assert!(error.contains("TenantView"), "{error}");
}

#[tokio::test]
async fn an_optional_entry_parameter_from_a_scope_that_never_encloses_the_request_is_rejected() {
    let dir = config_dir(1);
    let error = build_error(
        with_sessions(dir.path())
            .scope::<Tenant>()
            .controller::<TenantController>()
            .component::<TenantClient>(),
    )
    .await;

    assert!(error.contains("TenantClient"), "{error}");
}

#[tokio::test]
async fn a_required_edge_into_a_scope_on_only_some_chains_is_rejected() {
    let dir = config_dir(1);
    let error = build_error(with_sessions(dir.path()).component::<RequiredSession>()).await;

    assert!(error.contains("ScopeViolation"), "{error}");
    assert!(error.contains("RequiredSession"), "{error}");
}

#[tokio::test]
async fn optional_edges_into_a_scope_that_encloses_the_request_on_some_chain_validate() {
    let dir = config_dir(1);
    let (router, _runtime) = build(with_sessions(dir.path())).await;

    assert_eq!(get(&router, "/session/view", Some("kai")).await, "kai@1");
    assert_eq!(get(&router, "/session/client", Some("kai")).await, "kai");
}
