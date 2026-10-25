//! Scope opening: declared boundaries, parents, seeds and generation pinning.

use super::*;

#[tokio::test]
async fn open_rejects_undeclared_and_wrong_parent_boundaries() {
    let (runtime, _) = build_runtime(HashMap::new()).await;

    let undeclared = expect_app_error(
        runtime.open_scope_from_root(Other::KEY, Vec::new()).await,
        "undeclared boundary was accepted",
    );
    let wrong_parent = expect_app_error(
        runtime
            .open_scope(Request::KEY, runtime.root(), Vec::new())
            .await,
        "request opened without its session parent",
    );

    assert!(matches!(
        undeclared,
        Error::UndeclaredScopeOpen { scope: OTHER_ID }
    ));
    assert!(matches!(
        wrong_parent,
        Error::UnpinnedScopeParent {
            child: REQUEST_ID,
            parent: <upwell_core::Singleton as Scope>::ID,
        }
    ));
}

#[tokio::test]
async fn multi_parent_boundary_opens_under_each_declared_parent_only() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let session = runtime
        .open_scope_from_root(Session::KEY, Vec::new())
        .await
        .expect("session opens");
    let request = runtime
        .open_scope(Request::KEY, Arc::clone(&session), Vec::new())
        .await
        .expect("request opens under its session");

    let under_root = runtime
        .open_scope_from_root(Shared::KEY, Vec::new())
        .await
        .expect("shared opens under the root");
    let under_session = runtime
        .open_scope(Shared::KEY, session, Vec::new())
        .await
        .expect("shared opens under a session");
    let under_request = expect_app_error(
        runtime.open_scope(Shared::KEY, request, Vec::new()).await,
        "shared opened under an undeclared request parent",
    );

    assert_eq!(under_root.scope(), Shared::KEY);
    assert_eq!(under_session.scope(), Shared::KEY);
    assert!(matches!(
        &under_request,
        Error::InvalidScopeParent { child: SHARED_ID, expected, actual: REQUEST_ID }
            if **expected == [<upwell_core::Singleton as Scope>::ID, SESSION_ID]
    ));
    assert_eq!(
        under_request.to_string(),
        "scope 'test/shared' opens only under 'upwell/singleton', 'test/session', but parent 'test/request' was supplied"
    );
}

#[tokio::test]
async fn open_rejects_foreign_runtime_and_noncanonical_root_parents() {
    let (runtime, registry) = build_runtime(HashMap::new()).await;
    let (foreign, _) = build_runtime(HashMap::new()).await;
    let alternate_root = ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), registry)
        .await
        .expect("alternate root builds");

    let foreign_error = expect_app_error(
        runtime
            .open_scope(Session::KEY, foreign.root(), Vec::new())
            .await,
        "foreign runtime root was accepted",
    );
    let alternate_error = expect_app_error(
        runtime
            .open_scope(Session::KEY, alternate_root, Vec::new())
            .await,
        "same-registry noncanonical root was accepted",
    );

    assert!(matches!(
        foreign_error,
        Error::ForeignScopeParent {
            child: SESSION_ID,
            parent: <upwell_core::Singleton as Scope>::ID,
        }
    ));
    assert!(matches!(
        alternate_error,
        Error::UnpinnedScopeParent {
            child: SESSION_ID,
            parent: <upwell_core::Singleton as Scope>::ID,
        }
    ));
}

#[tokio::test]
async fn open_uses_declared_scope_metadata_and_retains_empty_boundaries() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let session = runtime
        .open_scope_from_root(Session::KEY, Vec::new())
        .await
        .expect("session opens");
    let request = runtime
        .open_scope(RequestAlias::KEY, session, Vec::new())
        .await
        .expect("request opens by stable identity");

    assert_eq!(request.scope().id(), REQUEST_ID);
    assert_eq!(request.scope().name(), Request::NAME);
    assert_eq!(request.scope().rank(), Request::RANK);
    assert!(!Arc::ptr_eq(&request, &runtime.root()));
}

#[tokio::test]
async fn open_validates_seed_registration_destination_and_uniqueness() {
    /// A seed declared for the session boundary.
    struct SessionSeed;
    /// A seed declared for the request boundary.
    struct RequestSeed;
    /// A seed absent from the application registry.
    struct UnknownSeed;

    let destinations = HashMap::from([
        (
            TypeId::of::<SessionSeed>(),
            SeedDestination {
                scope: SESSION_ID,
                type_name: std::any::type_name::<SessionSeed>(),
            },
        ),
        (
            TypeId::of::<RequestSeed>(),
            SeedDestination {
                scope: REQUEST_ID,
                type_name: std::any::type_name::<RequestSeed>(),
            },
        ),
    ]);
    let (runtime, _) = build_runtime(destinations).await;

    let wrong_destination = expect_app_error(
        runtime
            .open_scope_from_root(Session::KEY, vec![seed::<RequestSeed>()])
            .await,
        "wrong-destination seed was accepted",
    );
    let unregistered = expect_app_error(
        runtime
            .open_scope_from_root(Session::KEY, vec![seed::<UnknownSeed>()])
            .await,
        "unregistered seed was accepted",
    );
    let duplicate = expect_app_error(
        runtime
            .open_scope_from_root(
                Session::KEY,
                vec![seed::<SessionSeed>(), seed::<SessionSeed>()],
            )
            .await,
        "duplicate seed type was accepted",
    );

    assert!(matches!(
        wrong_destination,
        Error::InvalidSeedDestination {
            expected: REQUEST_ID,
            actual: SESSION_ID,
            ..
        }
    ));
    assert!(matches!(
        unregistered,
        Error::UnregisteredSeed {
            scope: SESSION_ID,
            ..
        }
    ));
    assert!(matches!(
        duplicate,
        Error::DuplicateSeedType {
            scope: SESSION_ID,
            ..
        }
    ));
}

#[tokio::test]
async fn nested_opening_inherits_its_parent_generation_after_publication() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let old_session = runtime
        .open_scope_from_root(Session::KEY, Vec::new())
        .await
        .expect("old session opens");
    let old_generation = old_session
        .generation_lease::<RuntimeGeneration>()
        .map(RuntimeView::from_generation)
        .expect("session pins a runtime generation");

    let committed = publish_next(&runtime).await;
    let old_request = runtime
        .open_scope(Request::KEY, old_session, Vec::new())
        .await
        .expect("request opens under old session");
    let old_request_generation = old_request
        .generation_lease::<RuntimeGeneration>()
        .map(RuntimeView::from_generation)
        .expect("request inherits a runtime generation");
    let new_session = runtime
        .open_scope_from_root(Session::KEY, Vec::new())
        .await
        .expect("new session opens");
    let new_generation = new_session
        .generation_lease::<RuntimeGeneration>()
        .map(RuntimeView::from_generation)
        .expect("new session pins a runtime generation");

    assert_eq!(old_generation.id(), RuntimeGenerationId::INITIAL);
    assert_eq!(old_request_generation.id(), RuntimeGenerationId::INITIAL);
    assert_eq!(new_generation.id(), committed.id());
    assert_eq!(new_generation.id(), RuntimeGenerationId::new(1));
}

#[tokio::test]
async fn nested_opening_rejects_an_unpinned_root_parent() {
    let (runtime, _) = build_runtime(HashMap::new()).await;

    let error = expect_app_error(
        runtime
            .open_scope(Session::KEY, runtime.root(), Vec::new())
            .await,
        "root parent was accepted by the nested-opening API",
    );

    assert!(matches!(
        error,
        Error::UnpinnedScopeParent {
            child: SESSION_ID,
            parent: <upwell_core::Singleton as Scope>::ID,
        }
    ));
}
