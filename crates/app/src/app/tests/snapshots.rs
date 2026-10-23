//! Which framework and user singletons can be retained across generations.

use super::*;

#[test]
fn framework_singletons_are_snapshot_capable_and_user_prebuilts_are_not() {
    assert!(
        crate::app::prepare::seed::SHUTDOWN_HANDLE_DESCRIPTOR
            .generation_snapshot
            .is_some(),
        "the shutdown handle must be retainable across generations"
    );
    assert!(
        crate::app::prepare::seed::RUNTIME_RELOADER_DESCRIPTOR
            .generation_snapshot
            .is_some(),
        "the runtime reloader must be retainable across generations"
    );
    assert!(
        crate::app::prepare::seed::HOOK_MANAGER_DESCRIPTOR
            .generation_snapshot
            .is_none(),
        "the hook manager catalog and resolver routing stay generation-local"
    );
    assert!(
        root_resolver_descriptor().generation_snapshot.is_none(),
        "the root resolver stays generation-local"
    );

    let mut registry = AppRegistry::default();
    let mut instances = Vec::new();
    let dirs =
        DirectoriesManager::from_path(std::env::temp_dir().join("upwell-app-descriptor-policy"));

    crate::app::prepare::seed::seed_directories(&dirs, &mut registry, &mut instances);

    for descriptor in &registry.components {
        assert!(
            descriptor.generation_snapshot.is_some(),
            "directories descriptor '{}' must be snapshot-capable",
            descriptor.id
        );
    }

    let builder =
        App::<BoundaryProtocol>::builder("descriptor-policy").with_component(UserProvided);
    let seeded = builder
        .registry
        .components
        .last()
        .expect("with_component registers a descriptor");

    assert_eq!(seeded.id, UserProvided::ID);
    assert!(
        seeded.generation_snapshot.is_none(),
        "user pre-built instances have no transition contract"
    );
}

#[tokio::test]
async fn candidate_metadata_cannot_upgrade_user_prebuilt_provenance() {
    let _boundary_guard = BOUNDARY_GUARD.lock().await;

    // The protocol runtime asserts the absolute construction count, so the shared
    // counters must start from zero even when another boundary test ran first.
    FACTORY_CALLS.store(0, Ordering::SeqCst);
    PRE_BUILD_CALLS.store(0, Ordering::SeqCst);
    PROTOCOL_BUILD_CALLS.store(0, Ordering::SeqCst);

    let app = App::<BoundaryProtocol>::builder("prepare-boundary-test")
        .config_source(
            ConfigManager::<Toml>::from_str(
                r#"
                    [logging]
                    level = "debug"
                    format = "compact"
                    ansi = false
                "#,
            )
            .expect("test config parses"),
        )
        .component_descriptor(&TYPED_USER_PROVIDED)
        .with_component(UserProvided)
        .build()
        .await
        .expect("app builds");

    let error = app
        .container()
        .snapshot_singleton(TYPED_USER_PROVIDED)
        .expect_err("the active user seed remains non-retainable");

    assert!(matches!(
        error,
        upwell_di::Error::SnapshotUnavailable { .. }
    ));
}

#[tokio::test]
async fn framework_singletons_snapshot_out_of_a_built_root() {
    let shutdown = ShutdownSignal::new();
    let hooks = HookManager::new(Vec::new());
    let reloader = RuntimeReloader::new();
    let dirs = DirectoriesManager::from_path(std::env::temp_dir().join("upwell-app-snapshot-test"));

    let descriptors = [
        crate::app::prepare::seed::SHUTDOWN_HANDLE_DESCRIPTOR,
        crate::app::prepare::seed::RUNTIME_RELOADER_DESCRIPTOR,
        crate::app::prepare::seed::HOOK_MANAGER_DESCRIPTOR,
        ComponentDescriptor::of::<DirectoriesManager>(),
        ComponentDescriptor::of::<Dir<ConfigDir>>(),
        root_resolver_descriptor(),
    ];
    let seeds = vec![
        BoxedComponent {
            ty: descriptors[0].ty,
            value: Box::new(shutdown.handle()),
        },
        BoxedComponent {
            ty: descriptors[1].ty,
            value: Box::new(Injectable::into_stored(reloader.clone())),
        },
        BoxedComponent {
            ty: descriptors[2].ty,
            value: Box::new(Injectable::into_stored(hooks.clone())),
        },
        BoxedComponent {
            ty: descriptors[3].ty,
            value: Box::new(dirs.clone()),
        },
        BoxedComponent {
            ty: descriptors[4].ty,
            value: Box::new(dirs.dir::<ConfigDir>()),
        },
        BoxedComponent {
            ty: descriptors[5].ty,
            value: Box::new(Injectable::into_stored(RootResolver::new())),
        },
    ];
    let components = descriptors
        .iter()
        .map(|descriptor| (descriptor.ty.type_id, *descriptor))
        .collect();
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), components, Vec::new(), HashMap::new())
            .expect("framework registry validates"),
    );
    let root = ScopeContainer::build_root(&descriptors, seeds, ResolverSet::new(), registry)
        .await
        .expect("framework root builds");

    for descriptor in [
        descriptors[0],
        descriptors[1],
        descriptors[3],
        descriptors[4],
    ] {
        root.snapshot_singleton(descriptor)
            .expect("framework singletons are snapshot-capable");
    }

    let hook_error = root
        .snapshot_singleton(descriptors[2])
        .expect_err("the hook manager is generation-local");

    assert!(matches!(
        hook_error,
        upwell_di::Error::SnapshotUnavailable { .. }
    ));

    let error = root
        .snapshot_singleton(root_resolver_descriptor())
        .expect_err("the root resolver is not snapshot-capable");

    assert!(matches!(
        error,
        upwell_di::Error::SnapshotUnavailable { .. }
    ));
}
