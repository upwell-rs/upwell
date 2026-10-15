//! Generation override seeds for framework singletons.

use super::*;

/// The framework-seeded hook-manager descriptor: typed identity, no factory, no
/// snapshot adapter — generation-local by contract.
fn hook_manager_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        HOOK_MANAGER_ID,
        HOOK_MANAGER_NAME,
        TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        &Singleton,
    )
}

/// A hook kind registered only on override managers, so an override manager is
/// distinguishable from the active generation's manager by catalog.
struct OverrideProbeKind;

impl HookKind for OverrideProbeKind {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "override-probe";
}

/// The probe hook's nominal receiver type; the call is never invoked.
struct OverrideProbeComponent;

/// The boxed future shape of an erased hook call.
type OverrideCallFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

fn unreachable_override_call<'a>(
    _: &'a (dyn ResolverCtx + Send + Sync),
    _: &'a (dyn Any + Send + Sync),
) -> OverrideCallFuture<'a> {
    Box::pin(async { unreachable!("override probe hooks are never invoked") })
}

fn override_probe_manager() -> HookManager {
    HookManager::new(vec![HookDescriptor::of::<OverrideProbeKind>(
        1,
        TypeDescriptor::of::<OverrideProbeComponent>("OverrideProbeComponent"),
        unreachable_override_call,
    )])
}

/// An active root holding a snapshot-capable retained singleton, a reconstructed
/// singleton, and the framework hook-manager seed, plus the candidate plan that
/// reconstructs `replaceable`.
struct HookOverrideFixture {
    active: RuntimeView,
    active_root: Arc<ScopeContainer>,
    active_instance: Arc<Retainable>,
    candidate: CandidateGraph,
    resolved: ResolvedTransitionPlan,
}

async fn hook_override_fixture() -> HookOverrideFixture {
    let active_instance = Arc::new(Retainable {
        label: "active-instance",
    });
    let active_hooks = HookManager::new(Vec::new());

    let active_registry = registry(
        [
            retainable_descriptor(),
            replaceable(ACTIVE_FACTORIES),
            hook_manager_descriptor(),
        ],
        [],
    );
    let (active_root, active_scopes) = active_root(
        &active_registry,
        vec![
            seed_retainable(&active_instance),
            seed_hook_manager(&active_hooks),
        ],
    )
    .await;
    let active = runtime_view(
        Arc::clone(&active_root),
        active_scopes,
        active_registry.components.clone(),
        active_graph(&active_registry),
    );
    let candidate = candidate_graph(&registry(
        [
            retainable_descriptor(),
            replaceable(CANDIDATE_FACTORIES),
            hook_manager_descriptor(),
        ],
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
        .expect("mixed retain and reconstruct resolves");

    HookOverrideFixture {
        active,
        active_root,
        active_instance,
        candidate,
        resolved,
    }
}

#[tokio::test]
async fn hook_manager_override_replaces_only_the_framework_seed() {
    let fixture = hook_override_fixture().await;

    let (root, scopes) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            vec![seed_hook_manager(&override_probe_manager())],
        )
        .await
        .expect("overridden candidate root builds");

    assert!(root.belongs_to_registry(&scopes));

    let hooks = root
        .get::<HookManager>()
        .expect("candidate root stores a hook manager");

    assert!(
        hooks.has::<OverrideProbeKind>(),
        "the override manager must replace the retained framework seed"
    );

    let retained = root
        .resolve::<Arc<Retainable>>()
        .await
        .expect("candidate root resolves")
        .expect("retained component is present");

    assert!(
        Arc::ptr_eq(&retained, &fixture.active_instance),
        "the non-overridden retained singleton must remain snapshot-derived"
    );

    let reconstructed = root
        .resolve::<Arc<Replaceable>>()
        .await
        .expect("candidate root resolves")
        .expect("reconstructed component is present");

    assert_eq!(reconstructed.label, "candidate");

    let active_hooks = fixture
        .active_root
        .get::<HookManager>()
        .expect("active root stores its hook manager");

    assert!(
        !active_hooks.has::<OverrideProbeKind>(),
        "the active root's manager is untouched by candidate preparation"
    );
}

#[tokio::test]
async fn duplicate_generation_overrides_are_rejected() {
    let fixture = hook_override_fixture().await;

    let Err(error) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            vec![
                seed_hook_manager(&override_probe_manager()),
                seed_hook_manager(&override_probe_manager()),
            ],
        )
        .await
    else {
        panic!("duplicate generation overrides are invalid");
    };

    assert!(matches!(
        error,
        Error::DuplicateGenerationOverride {
            type_name: HOOK_MANAGER_NAME
        }
    ));
}

#[tokio::test]
async fn generation_overrides_must_replace_retained_singletons() {
    let fixture = hook_override_fixture().await;
    let reconstructed_override = BoxedComponent {
        ty: TypeDescriptor::of::<Replaceable>("Replaceable"),
        value: Box::new(Injectable::into_stored(Arc::new(Replaceable {
            label: "override",
        }))),
    };

    let Err(error) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            vec![reconstructed_override],
        )
        .await
    else {
        panic!("an override for a reconstructed component is invalid");
    };

    assert!(matches!(
        error,
        Error::InvalidGenerationOverride {
            type_name: "Replaceable"
        }
    ));
}

#[tokio::test]
async fn unoverridden_hook_manager_retention_still_requires_restart() {
    let fixture = hook_override_fixture().await;

    let Err(error) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            Vec::new(),
        )
        .await
    else {
        panic!("the hook manager has no snapshot contract");
    };

    assert!(matches!(
        error,
        Error::RestartRequired(RestartRequired {
            component: HOOK_MANAGER_ID,
            required: Some(NodeAction::Retain),
            reason: RestartReason::ManualInstanceUnsupported,
        })
    ));
}
