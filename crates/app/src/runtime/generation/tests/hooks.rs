//! Hook resolution through each generation's own root-seeded manager.

use super::*;

/// A hook kind whose outcome reports which generation root resolved the hook.
struct ProbeKind;

impl HookKind for ProbeKind {
    type Output = &'static str;
    type Cx = ();

    const NAME: &'static str = "probe";
}

/// A resolver seeded into one generation's root, identifying it to probe hooks.
#[derive(Clone)]
struct GenerationProbe(&'static str);

impl Resolver for GenerationProbe {}

/// The hook's nominal receiver type; the probe call never resolves a component.
struct ProbeComponent;

fn probe_call<'a>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    _: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(async move {
        let tag = ctx
            .get_resolver::<GenerationProbe>()
            .map(|probe| probe.0)
            .unwrap_or("<unresolved>");

        Ok(Box::new(tag) as Box<dyn Any + Send>)
    })
}

static PROBE_HOOK: HookDescriptor = HookDescriptor::of::<ProbeKind>(
    1,
    TypeDescriptor::of::<ProbeComponent>("ProbeComponent"),
    probe_call,
);

fn probe_manager() -> HookManager {
    HookManager::new(vec![PROBE_HOOK])
}

fn probe_externals(tag: &'static str) -> ResolverSet {
    let mut externals = ResolverSet::new();

    externals.insert(Arc::new(GenerationProbe(tag)));

    externals
}

async fn probe_tags(hooks: &HookManager) -> Vec<&'static str> {
    hooks
        .fire_all::<ProbeKind>(&())
        .await
        .participants
        .into_iter()
        .map(|(_, outcome)| outcome.expect("probe hook resolves through its root"))
        .collect()
}

#[tokio::test]
async fn pinned_views_resolve_hooks_through_their_own_generation_manager() {
    let coordinator = RuntimeTransitionCoordinator::new(
        prepared_with_hooks(
            empty_condition(),
            probe_manager(),
            probe_externals("initial"),
        )
        .await,
    );
    let old = coordinator.current();

    let candidate = prepared_with_hooks(
        empty_condition(),
        probe_manager(),
        probe_externals("candidate"),
    )
    .await;
    let committed = coordinator
        .begin()
        .await
        .publish(candidate)
        .expect("candidate publishes");

    assert_eq!(probe_tags(old.hooks()).await, ["initial"]);
    assert_eq!(
        probe_tags(coordinator.current().hooks()).await,
        ["candidate"]
    );
    assert_eq!(probe_tags(committed.hooks()).await, ["candidate"]);
}

#[tokio::test]
async fn prepared_generation_binds_the_root_seeded_hook_manager() {
    let coordinator = RuntimeTransitionCoordinator::new(
        prepared_with_hooks(
            empty_condition(),
            probe_manager(),
            probe_externals("seeded"),
        )
        .await,
    );
    let view = coordinator.current();
    let root_hooks = view
        .root()
        .get::<HookManager>()
        .expect("every prepared root seeds the framework hook manager");

    assert!(root_hooks.has::<ProbeKind>());
    assert!(view.hooks().has::<ProbeKind>());
    assert!(root_hooks.component_participates::<ProbeKind>(TypeId::of::<ProbeComponent>()));
    assert!(
        view.hooks()
            .component_participates::<ProbeKind>(TypeId::of::<ProbeComponent>())
    );

    // The generation attached the root-resolved manager, so both handles route hook
    // receivers through this generation's root.
    assert_eq!(probe_tags(&root_hooks).await, ["seeded"]);
    assert_eq!(probe_tags(view.hooks()).await, ["seeded"]);
}

#[tokio::test]
async fn cloned_lifecycle_manager_survives_publication_while_its_view_is_held() {
    let coordinator = RuntimeTransitionCoordinator::new(
        prepared_with_hooks(
            empty_condition(),
            probe_manager(),
            probe_externals("initial"),
        )
        .await,
    );
    let lifecycle_view = coordinator.current();
    let hooks = lifecycle_view.hooks().clone();
    let root = Arc::downgrade(lifecycle_view.root());

    let candidate = prepared_with_hooks(
        empty_condition(),
        probe_manager(),
        probe_externals("candidate"),
    )
    .await;
    coordinator
        .begin()
        .await
        .publish(candidate)
        .expect("candidate publishes");

    // While the old view is held, the cloned manager still routes through its own root.
    assert_eq!(probe_tags(&hooks).await, ["initial"]);
    assert!(
        root.upgrade().is_some(),
        "the pinned view keeps the generation root alive"
    );

    // Dropping the view and the manager must release the root: the manager retains only
    // a weak reference, so no strong cycle keeps the generation alive.
    drop(hooks);
    drop(lifecycle_view);

    assert!(
        root.upgrade().is_none(),
        "the cloned manager must not retain its generation root"
    );
}
