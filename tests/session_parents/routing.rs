//! Which parent a request opens under, what a session shares, and which requests a parent
//! layer sees.

use super::*;

#[tokio::test]
async fn requests_open_under_their_session_or_the_root_in_one_app() {
    let dir = config_dir(1);
    let (router, _runtime) = build(with_sessions(dir.path())).await;

    assert_eq!(get(&router, "/session/view", Some("ada")).await, "ada@1");
    assert_eq!(get(&router, "/session/view", None).await, "root@1");
    assert_eq!(get(&router, "/session/view", Some("bob")).await, "bob@1");
    assert_eq!(get(&router, "/session/client", Some("ada")).await, "ada");
    assert_eq!(get(&router, "/session/client", None).await, "root");
}

#[tokio::test]
async fn a_session_builds_its_components_once_across_its_requests() {
    let dir = config_dir(1);
    let (router, runtime) = build(with_sessions(dir.path())).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");

    for _ in 0..3 {
        assert_eq!(get(&router, "/session/view", Some("cy")).await, "cy@1");
    }

    assert_eq!(get(&router, "/session/client", Some("cy")).await, "cy");
    assert_eq!(get(&router, "/session/view", Some("dan")).await, "dan@1");

    let client = sessions
        .get("cy")
        .expect("the session is cached")
        .container()
        .get::<SessionClient>()
        .expect("the session holds its client");

    assert_eq!(client.user, "cy");
    assert_eq!(
        CLIENT_BUILDS.load(Ordering::SeqCst),
        2,
        "each session built its client once, whatever its requests injected"
    );
}

#[tokio::test]
async fn without_a_parent_layer_a_request_parent_is_ignored() {
    let dir = config_dir(1);
    let (sessions_router, runtime) = build(with_sessions(dir.path())).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");
    let (router, _runtime) = build(
        app(dir.path())
            .layer(upwell::axum::axum::Extension(sessions))
            .layer(axum_middleware::from_fn(stray_parent)),
    )
    .await;

    assert_eq!(get(&sessions_router, "/session/view", None).await, "root@1");
    assert_eq!(
        get(&router, "/session/view", Some("gus")).await,
        "root@1",
        "the request opens under the root, as without the feature"
    );
}

#[tokio::test]
async fn a_request_no_route_matches_never_reaches_a_parent_layer() {
    let dir = config_dir(1);
    let (router, _runtime) = build(with_sessions(dir.path())).await;

    assert_eq!(
        send(&router, "/nowhere", Some("hal")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        PARENT_CALLS.load(Ordering::SeqCst),
        0,
        "an unmatched request takes the plain 404 path"
    );
    assert_eq!(get(&router, "/session/view", Some("hal")).await, "hal@1");
    assert_eq!(PARENT_CALLS.load(Ordering::SeqCst), 1);
}
