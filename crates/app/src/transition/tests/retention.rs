//! Retained candidate preparation from the pinned active root.

use super::*;

#[tokio::test]
async fn mixed_retain_and_reconstruct_builds_a_complete_candidate_root() {
    let active = registry(
        [
            retainable_descriptor(),
            replaceable(ACTIVE_FACTORIES),
            root_resolver_descriptor(),
        ],
        [svc_provider()],
    );
    let candidate = candidate_graph(&registry(
        [
            retainable_descriptor(),
            replaceable(CANDIDATE_FACTORIES),
            root_resolver_descriptor(),
        ],
        [svc_provider()],
    ));
    let active_instance = Arc::new(Retainable {
        label: "active-instance",
    });
    let active_hooks = HookManager::new(Vec::new());
    let (active_root, active_scopes) = active_root(
        &active,
        vec![
            seed_retainable(&active_instance),
            seed_root_resolver(),
            seed_hook_manager(&active_hooks),
        ],
    )
    .await;
    let foreign = active_view(
        &active,
        vec![
            seed_retainable(&active_instance),
            seed_root_resolver(),
            seed_hook_manager(&HookManager::new(Vec::new())),
        ],
    )
    .await;
    let active = runtime_view(
        Arc::clone(&active_root),
        active_scopes,
        active.components.clone(),
        active_graph(&active),
    );

    let resolved = candidate
        .resolve_transition(
            active.effective_graph(),
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("mixed retain and reconstruct resolves");

    let Err(foreign_error) = resolved
        .build_candidate_root_from_active(&candidate, &foreign, ResolverSet::new())
        .await
    else {
        panic!("a foreign runtime view cannot supply retained instances");
    };

    assert!(matches!(foreign_error, Error::StaleGraphCandidate(_)));

    let (root, scopes) = resolved
        .build_candidate_root_from_active(&candidate, &active, ResolverSet::new())
        .await
        .expect("retained candidate root builds");

    assert!(root.belongs_to_registry(&scopes));

    let retained = root
        .resolve::<Arc<Retainable>>()
        .await
        .expect("candidate root resolves")
        .expect("retained component is present");

    assert!(
        Arc::ptr_eq(&retained, &active_instance),
        "the retained singleton must share the active Arc instance"
    );

    let aliased = root
        .resolve::<Arc<dyn Svc>>()
        .await
        .expect("candidate root resolves the provider")
        .expect("candidate provider alias is present");

    assert_eq!(aliased.label(), "active-instance");
    assert_eq!(
        Arc::as_ptr(&aliased) as *const u8,
        Arc::as_ptr(&active_instance) as *const u8,
        "the candidate provider alias must be rebuilt from the retained concrete"
    );

    let reconstructed = root
        .resolve::<Arc<Replaceable>>()
        .await
        .expect("candidate root resolves")
        .expect("reconstructed component is present");

    assert_eq!(reconstructed.label, "candidate");

    let resolver = root
        .get::<RootResolver>()
        .expect("candidate resolver is recreated");
    let resolved_from_candidate = resolver
        .extract::<Arc<Replaceable>>()
        .await
        .expect("candidate resolver resolves the candidate root");

    assert_eq!(
        resolved_from_candidate.label, "candidate",
        "the root resolver must be recreated for the candidate generation"
    );

    let active_replaceable = active_root
        .resolve::<Arc<Replaceable>>()
        .await
        .expect("active root resolves")
        .expect("active replaceable is present");

    assert_eq!(
        active_replaceable.label, "active",
        "the active root is untouched by candidate preparation"
    );
}

#[tokio::test]
async fn raw_manual_singleton_requires_restart() {
    let user_provided = || {
        ComponentDescriptor::manual(
            "user-provided",
            "UserProvided",
            TypeDescriptor::of::<u8>("UserProvided"),
            &Singleton,
        )
    };
    let active_hooks = HookManager::new(Vec::new());
    let active = active_view(
        &registry([replaceable(ACTIVE_FACTORIES), user_provided()], []),
        vec![
            BoxedComponent {
                ty: TypeDescriptor::of::<u8>("UserProvided"),
                value: Box::new(8u8),
            },
            seed_hook_manager(&active_hooks),
        ],
    )
    .await;
    let candidate = candidate_graph(&registry(
        [replaceable(CANDIDATE_FACTORIES), user_provided()],
        [],
    ));
    let resolved = candidate
        .resolve_transition(
            active.effective_graph(),
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("the unchanged manual component receives a retain disposition");

    let Err(error) = resolved
        .build_candidate_root_from_active(&candidate, &active, ResolverSet::new())
        .await
    else {
        panic!("a raw manual singleton has no transition contract");
    };

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            component: "user-provided",
            required: Some(NodeAction::Retain),
            reason: RestartReason::ManualInstanceUnsupported,
        })
    ));
}

#[derive(Clone)]
struct UnretainedByValue;

impl Component for UnretainedByValue {
    type Handle = Self;

    const ID: &'static str = "unretained-by-value";
    const NAME: &'static str = "UnretainedByValue";

    fn into_handle(self) -> Self {
        self
    }
}

impl Injectable for UnretainedByValue {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[tokio::test]
async fn typed_handle_without_snapshot_contract_requires_distinct_restart() {
    let unretained = || {
        ComponentDescriptor::of::<UnretainedByValue>()
            .with_identity(UnretainedByValue::ID, UnretainedByValue::NAME)
    };
    let active_hooks = HookManager::new(Vec::new());
    let active = active_view(
        &registry([replaceable(ACTIVE_FACTORIES), unretained()], []),
        vec![
            BoxedComponent {
                ty: unretained().ty,
                value: Box::new(UnretainedByValue),
            },
            seed_hook_manager(&active_hooks),
        ],
    )
    .await;
    let candidate = candidate_graph(&registry(
        [replaceable(CANDIDATE_FACTORIES), unretained()],
        [],
    ));
    let resolved = candidate
        .resolve_transition(
            active.effective_graph(),
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("unchanged typed component receives a retain disposition");

    let Err(error) = resolved
        .build_candidate_root_from_active(&candidate, &active, ResolverSet::new())
        .await
    else {
        panic!("a typed handle without a snapshot contract must restart");
    };

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            component: "unretained-by-value",
            required: Some(NodeAction::Retain),
            reason: RestartReason::HandleRetentionUnsupported,
        })
    ));
}
