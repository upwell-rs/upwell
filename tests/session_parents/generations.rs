//! What a request under a session sees of the session's runtime generation: eager components,
//! accessors and singletons, before and after a reload.

use super::*;

#[tokio::test]
async fn eager_request_components_and_scoped_accessors_see_the_session() {
    let dir = config_dir(1);
    let (router, _runtime) = build(
        with_sessions(dir.path())
            .controller::<ScopedController>()
            .component::<EagerProbe>()
            .component::<SessionReader>(),
    )
    .await;

    assert_eq!(get(&router, "/scoped/read", Some("dee")).await, "dee");
    assert_eq!(
        EAGER_SAW
            .lock()
            .expect("the probe lock is healthy")
            .as_deref(),
        Some("dee"),
        "the eager opening ran under the session"
    );
    assert_eq!(get(&router, "/scoped/read", None).await, "outside");
    assert_eq!(
        EAGER_SAW
            .lock()
            .expect("the probe lock is healthy")
            .as_deref(),
        Some("root")
    );
}

#[tokio::test]
async fn a_session_keeps_its_generation_after_a_reload() {
    let dir = config_dir(1);
    let (router, runtime) = build(with_sessions(dir.path())).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");

    assert_eq!(get(&router, "/session/view", Some("eve")).await, "eve@1");

    write_value(dir.path(), 2);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(
        report.is_committed(),
        "a changed value commits a generation"
    );

    let session = sessions.get("eve").expect("the session is cached");

    assert_eq!(
        runtime.generations_behind(session.container()),
        Some(1),
        "the session reports how far behind it is"
    );
    assert_eq!(
        get(&router, "/session/view", Some("eve")).await,
        "eve@1",
        "a request under the session runs on the session's generation"
    );
    assert_eq!(
        get(&router, "/session/view", None).await,
        "root@2",
        "a request under the root runs on the current generation"
    );
    assert_eq!(
        get(&router, "/session/view", Some("fay")).await,
        "fay@2",
        "a session opened after the reload runs on the current generation"
    );
}

#[tokio::test]
async fn an_old_session_opens_its_requests_at_once_while_its_generation_has_an_accessor() {
    let dir = config_dir(1);
    let (router, runtime) = build(
        with_sessions(dir.path())
            .controller::<WatchController>()
            .component::<RequestValue>()
            .component::<Watcher>()
            .component::<SessionWatcher>(),
    )
    .await;

    assert_eq!(get(&router, "/watch/warm", Some("ivy")).await, "watching");
    assert_eq!(
        get(&router, "/watch/read", Some("ivy")).await,
        "request value"
    );

    write_config(dir.path(), 1, Gate::Closed);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(
        report.is_committed(),
        "closing the gate commits a generation"
    );
    assert_eq!(
        get(&router, "/watch/warm", Some("jo")).await,
        "unwatched",
        "the current generation has no watcher"
    );
    assert_eq!(
        get(&router, "/watch/read", Some("ivy")).await,
        "request value",
        "the old session's generation still reads the request scope, so it opens at once"
    );
}
